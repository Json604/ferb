//! Start and stop the watchers (`ferb/sense/manager.py`). It never runs a task from a raw observation.
//!
//! `ferb pause` stops every watcher (the owner-input tap and the Mac's presence watcher included) and the trail
//! records nothing until resume; the pause survives a restart (a marker file). The owner-input tap starts as soon as
//! Input Monitoring is granted: it is checked again every `tap_recheck_s` and each change is reported once
//! (SENSE-406). A watcher that fails tries again later, from `retry_s` doubling to 15 minutes (SENSE-405).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::interfaces::BoxFuture;
use serde_json::Value;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::observation::{dict, Observation};

/// manager.py:18: why the owner-input tap isn't running; while it isn't, FERB can't tell whether the owner is using
/// the Mac (SENSE-406).
pub const INPUT_MONITORING_OFF: &str = "Input Monitoring isn't granted to FERB";
pub const TAP_REFUSED: &str = "macOS didn't create the input tap";

/// What a watcher hands an observation to (Python's `emit`; the daemon records it in the trail on its loop).
pub type Emit = Arc<dyn Fn(Observation) + Send + Sync>;

#[derive(Default)]
struct StopInner {
    set: AtomicBool,
    notify: Notify,
}

/// An `asyncio.Event` used as a stop signal: set once, seen by every waiter.
#[derive(Clone, Default)]
pub struct Stop(Arc<StopInner>);

impl Stop {
    pub fn new() -> Stop {
        Stop::default()
    }

    pub fn set(&self) {
        self.0.set.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }

    pub fn is_set(&self) -> bool {
        self.0.set.load(Ordering::SeqCst)
    }

    /// Until set.
    pub async fn wait(&self) {
        loop {
            let notified = self.0.notify.notified();
            if self.is_set() {
                return;
            }
            notified.await;
        }
    }

    /// `asyncio.wait_for(stop.wait(), seconds)`: true when set within the time.
    pub async fn wait_for(&self, seconds: f64) -> bool {
        let d = Duration::from_secs_f64(seconds.max(0.0));
        tokio::time::timeout(d, self.wait()).await.is_ok()
    }
}

/// watchers/base.py:21 `Watcher`: runs until `stop` is set; `Err` is why it failed (`"Type: message"`, as Python's
/// `f"{type(e).__name__}: {e}"`).
pub trait Watcher: Send + Sync {
    fn source(&self) -> &str;
    fn run(self: Arc<Self>, emit: Emit, stop: Stop) -> BoxFuture<'static, Result<(), String>>;
}

/// The owner-input tap as the manager sees it (`watchers/owner_input.py` `OwnerInput`).
pub trait OwnerInputTap: Send + Sync {
    /// Input Monitoring is granted. `Err`: the check itself failed (its why).
    fn allowed(&self) -> Result<bool, String>;
    fn running(&self) -> bool;
    /// Start the listen-only tap; false when macOS didn't create it.
    fn start(&self) -> Result<bool, String>;
    fn stop(&self);
}

/// The Mac's presence watcher as the manager sees it (`watchers/presence.py` `MacPresence`).
pub trait PresenceWatch: Send + Sync {
    fn start(&self);
    fn stop(&self);
}

type Callback1 = Arc<dyn Fn(&str) + Send + Sync>;
type Callback2 = Arc<dyn Fn(&str, &str) + Send + Sync>;
type Publish = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// What the manager tells the daemon.
#[derive(Clone)]
pub struct Hooks {
    /// paused (true) or resumed (false)
    pub on_pause: Arc<dyn Fn(bool) + Send + Sync>,
    /// (source, why) a watcher failed, each new reason once
    pub on_fail: Callback2,
    /// the tap's state changed: why it's down, or ""
    pub on_tap: Callback1,
    /// the trail's publish (`sense.watcher_failed`)
    pub publish: Publish,
}

impl Default for Hooks {
    fn default() -> Self {
        Hooks {
            on_pause: Arc::new(|_| {}),
            on_fail: Arc::new(|_, _| {}),
            on_tap: Arc::new(|_| {}),
            publish: Arc::new(|_, _| {}),
        }
    }
}

struct Shared {
    failed: Mutex<BTreeMap<String, String>>,
    tap_down: Mutex<String>,
}

/// manager.py:22 `Sense`.
pub struct Sense {
    pub marker: PathBuf,
    watchers: Vec<Arc<dyn Watcher>>,
    owner_input: Option<Arc<dyn OwnerInputTap>>,
    presence: Option<Arc<dyn PresenceWatch>>,
    emit: Emit,
    hooks: Hooks,
    shared: Arc<Shared>,
    /// a failed watcher tries again after this, doubling to 15 minutes
    pub retry_s: f64,
    /// the tap's permission is checked again this often
    pub tap_recheck_s: f64,
    stop: Mutex<Option<Stop>>,
    tasks: Mutex<Vec<(String, JoinHandle<()>)>>,
    tap_task: Mutex<Option<JoinHandle<()>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// manager.py:64 `_tap_state` (blocking): start the tap if it's down and Input Monitoring is granted; why it isn't
/// running now, or "". A tap whose grant was taken away is stopped.
pub fn tap_state(oi: &dyn OwnerInputTap) -> String {
    match oi.allowed() {
        Err(e) => format!("{TAP_REFUSED}: {e}"),
        Ok(false) => {
            if oi.running() {
                oi.stop();
            }
            INPUT_MONITORING_OFF.to_string()
        }
        Ok(true) => {
            if oi.running() {
                return String::new();
            }
            match oi.start() {
                Ok(true) => String::new(),
                Ok(false) => TAP_REFUSED.to_string(),
                Err(e) => format!("{TAP_REFUSED}: {e}"),
            }
        }
    }
}

impl Sense {
    /// `home`: FERB's home (the PAUSED marker lives there).
    pub fn new(
        home: PathBuf,
        watchers: Vec<Arc<dyn Watcher>>,
        emit: Emit,
        owner_input: Option<Arc<dyn OwnerInputTap>>,
        presence: Option<Arc<dyn PresenceWatch>>,
        hooks: Hooks,
    ) -> Sense {
        Sense {
            marker: home.join("PAUSED"),
            watchers,
            owner_input,
            presence,
            emit,
            hooks,
            shared: Arc::new(Shared {
                failed: Mutex::new(BTreeMap::new()),
                tap_down: Mutex::new(String::new()),
            }),
            retry_s: 60.0,
            tap_recheck_s: 30.0,
            stop: Mutex::new(None),
            tasks: Mutex::new(Vec::new()),
            tap_task: Mutex::new(None),
        }
    }

    pub fn paused(&self) -> bool {
        self.marker.exists()
    }

    /// Why the owner-input tap isn't running ("": it is).
    pub fn tap_down(&self) -> String {
        lock(&self.shared.tap_down).clone()
    }

    /// source -> why it last failed (doctor shows it).
    pub fn failed(&self) -> BTreeMap<String, String> {
        lock(&self.shared.failed).clone()
    }

    /// manager.py:46: watchers running now (not ended, not waiting to try again).
    pub fn running(&self) -> Vec<String> {
        let failed = lock(&self.shared.failed);
        lock(&self.tasks)
            .iter()
            .filter(|(s, t)| !t.is_finished() && !failed.contains_key(s))
            .map(|(s, _)| s.clone())
            .collect()
    }

    fn tap_report(shared: &Shared, on_tap: &Callback1, why: &str) {
        let mut down = lock(&shared.tap_down);
        if *down != why {
            *down = why.to_string();
            drop(down);
            on_tap(why);
        }
    }

    /// manager.py:53.
    pub async fn start(&self) {
        if self.paused() || !lock(&self.tasks).is_empty() {
            return;
        }
        let stop = Stop::new();
        *lock(&self.stop) = Some(stop.clone());
        if let Some(oi) = &self.owner_input {
            // before the watchers that read it (typed text)
            Self::tap_report(&self.shared, &self.hooks.on_tap, &tap_state(oi.as_ref()));
            let (oi, shared, on_tap, stop, every) = (
                oi.clone(),
                self.shared.clone(),
                self.hooks.on_tap.clone(),
                stop.clone(),
                self.tap_recheck_s,
            );
            *lock(&self.tap_task) = Some(tokio::spawn(async move {
                // SENSE-406: the tap comes up once the grant is there, without a restart
                while !stop.is_set() {
                    if stop.wait_for(every).await {
                        return;
                    }
                    let tap = oi.clone();
                    let why = tokio::task::spawn_blocking(move || tap_state(tap.as_ref()))
                        .await
                        .unwrap_or_else(|e| format!("{TAP_REFUSED}: {e}"));
                    Self::tap_report(&shared, &on_tap, &why);
                }
            }));
        }
        if let Some(p) = &self.presence {
            p.start();
        }
        let mut tasks = lock(&self.tasks);
        for w in &self.watchers {
            let source = w.source().to_string();
            let handle = tokio::spawn(guard(
                w.clone(),
                self.emit.clone(),
                stop.clone(),
                self.shared.clone(),
                self.hooks.clone(),
                self.retry_s,
            ));
            tasks.push((source, handle));
        }
    }

    /// manager.py:119.
    pub async fn stop(&self) {
        if let Some(s) = lock(&self.stop).take() {
            s.set();
        }
        let tasks: Vec<(String, JoinHandle<()>)> = std::mem::take(&mut *lock(&self.tasks));
        for (_, mut t) in tasks {
            if tokio::time::timeout(Duration::from_secs(5), &mut t)
                .await
                .is_err()
            {
                t.abort();
            }
        }
        let tap = lock(&self.tap_task).take();
        if let Some(t) = tap {
            let _ = t.await; // a check under way ends first
        }
        if let Some(oi) = &self.owner_input {
            oi.stop();
        }
        if let Some(p) = &self.presence {
            p.stop();
        }
    }

    /// manager.py:136.
    pub async fn pause(&self) -> std::io::Result<()> {
        touch(&self.marker)?;
        self.stop().await;
        (self.hooks.on_pause)(true);
        Ok(())
    }

    /// manager.py:141.
    pub async fn resume(&self) -> std::io::Result<()> {
        match std::fs::remove_file(&self.marker) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        self.start().await;
        (self.hooks.on_pause)(false);
        Ok(())
    }
}

fn touch(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .map(|_| ())
}

/// manager.py:93 `_guard`: runs a watcher; one that fails tries again later, each new reason reported once.
async fn guard(
    w: Arc<dyn Watcher>,
    emit: Emit,
    stop: Stop,
    shared: Arc<Shared>,
    hooks: Hooks,
    retry_s: f64,
) {
    let source = w.source().to_string();
    let (mut wait, mut reported) = (retry_s, String::new());
    while !stop.is_set() {
        lock(&shared.failed).remove(&source); // trying (again): running until it fails
        match w.clone().run(emit.clone(), stop.clone()).await {
            Ok(()) => return,
            Err(why) => {
                lock(&shared.failed).insert(source.clone(), why.clone());
                if why != reported {
                    reported = why.clone();
                    (hooks.publish)(
                        "sense.watcher_failed",
                        Value::Object(dict([
                            ("source", Value::from(source.clone())),
                            ("error", Value::from(why.clone())),
                        ])),
                    );
                    (hooks.on_fail)(&source, &why);
                }
            }
        }
        stop.wait_for(wait).await;
        wait = (wait * 2.0).min(900.0);
    }
}
