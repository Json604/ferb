//! T-1343 (kb/plans/front-door.md, "Keeping the runtime ready"; ORCH-726 step 6): a model program ready before the
//! message arrives, on every runtime, behind one mechanism.
//!
//! The owner decided the mechanism and its number: a program is ready before the message, and an idle one is kept 5
//! minutes ([`IDLE_S`]). Everything else here is the agent's: one [`Keeper`] in the daemon owns the ready programs,
//! one slot per role, and is the only thing that starts or ends them. A slot holds at most one idle program (beside
//! those in use), made for one set of settings (runtime, model and everything that shapes the program). It ends that
//! program after 5 idle minutes, when the next call's settings differ (the model, effort, sign-in or settings
//! changed), when it was found dead or its sign-in changed (a crash: a new one is started), when the keeper is
//! switched off, and when the daemon stops. A program used for one message only (Claude Code) is ended after it by
//! today's code, and the keeper starts the next one ahead.
//!
//! What "ready" means is each runtime's ([`Keepable`]): Codex keeps its app-server and takes each message on a fresh
//! thread state (`thread/unsubscribe` after the message, the template rewritten and `thread/resume` before the next);
//! Claude Code has its next program started and initialised; the API loop keeps its HTTP connection pool. A call
//! takes the idle program when one fits and starts a new one when none does, so two calls at once never share a
//! program, and only one message at a time ever runs in one.
//!
//! The clock is injected (TEST-508). The switch ([`Keeper::set_on`]) is on in a new keeper; off, every call is
//! today's start, unchanged.

use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{ModelRequest, Task};
use tokio::task::JoinHandle;

use crate::error::RuntimeError;
use crate::run::{Adapter, Started};

/// The owner's number (ORCH-726, "an idle one is kept 5 minutes"): how long a ready program waits for a message.
pub const IDLE_S: f64 = 300.0;

pub type Clock = Arc<dyn Fn() -> f64 + Send + Sync>;

/// A program started ahead of its message, or handed back after one.
pub trait Ready: Send + Sync + 'static {
    /// What it was made for ([`Keepable::keep_settings`]); a call with other settings never takes it.
    fn settings(&self) -> &str;
    /// Still usable: its process runs, its connection is open, and its sign-in is the one it started with.
    fn alive(&self) -> bool;
    /// Ends it with today's code for its runtime.
    fn end(self: Arc<Self>) -> BoxFuture<'static, ()>;
    fn as_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;
}

/// How a program that may take another message goes back to its keeper once its message ended.
pub type Back = Arc<dyn Fn(Arc<dyn Ready>) + Send + Sync>;

/// A runtime adapter whose program can be made ready ahead of its message.
pub trait Keepable: Adapter {
    /// Everything that shapes the program a call with this request and token would start (runtime, model, mode,
    /// effort, whether it gets FERB's tools, ...), as one string; None when such a call never uses a kept program
    /// (it starts as today). `request` is None when the keeper prepares ahead of any message.
    fn keep_settings(&self, request: Option<&ModelRequest>, has_token: bool) -> Option<String>;
    /// Starts a program for these settings, with no message yet. `back` is how it returns once a message ended,
    /// when it can take another.
    fn prepare<'a>(
        &'a self,
        settings: &'a str,
        back: Back,
    ) -> BoxFuture<'a, Result<Arc<dyn Ready>, RuntimeError>>;
    /// Gives `ready` (made by this runtime, for this call's settings) this call's message: the same shape as
    /// [`Adapter::start`]. A failure before the program was touched hands it back; any later one ends it.
    fn take<'a>(
        &'a self,
        ready: Arc<dyn Ready>,
        back: Back,
        task: &'a Task,
        request: &'a ModelRequest,
        run_token: &'a str,
    ) -> BoxFuture<'a, Result<Started, RuntimeError>>;
    /// A program used for one message only (Claude Code: a second message would carry the first one's history).
    fn once(&self) -> bool {
        false
    }
}

/// One role's ready program.
struct Slot {
    settings: String,
    idle: Option<(Arc<dyn Ready>, f64)>,
    /// The adapter of the role's latest call, to prepare the next program ahead (for that call's settings).
    last: Option<Arc<dyn Keepable>>,
    preparing: bool,
}

struct Inner {
    clock: Clock,
    idle_s: f64,
    on: AtomicBool,
    closed: AtomicBool,
    slots: Mutex<HashMap<String, Slot>>,
    /// Programs being ended or prepared in the background (joined by `shut_down`).
    jobs: Mutex<Vec<JoinHandle<()>>>,
}

/// The daemon's one owner of ready programs. Cheap to clone.
#[derive(Clone)]
pub struct Keeper {
    inner: Arc<Inner>,
}

impl Keeper {
    /// A keeper with the owner's 5 minutes, switched on.
    pub fn new(clock: Clock) -> Keeper {
        Keeper::with_idle(clock, IDLE_S)
    }

    pub fn with_idle(clock: Clock, idle_s: f64) -> Keeper {
        Keeper {
            inner: Arc::new(Inner {
                clock,
                idle_s,
                on: AtomicBool::new(true),
                closed: AtomicBool::new(false),
                slots: Mutex::new(HashMap::new()),
                jobs: Mutex::new(Vec::new()),
            }),
        }
    }

    /// The switch. Off: every call starts as today, and the idle programs end now.
    pub fn set_on(&self, on: bool) {
        self.inner.on.store(on, Ordering::SeqCst);
        if !on {
            self.end_idle();
        }
    }

    pub fn is_on(&self) -> bool {
        self.inner.on.load(Ordering::SeqCst) && !self.inner.closed.load(Ordering::SeqCst)
    }

    /// Whether `role` has an idle program now.
    pub fn has_idle(&self, role: &str) -> bool {
        self.lock().get(role).is_some_and(|s| s.idle.is_some())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Slot>> {
        self.inner.slots.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn now(&self) -> f64 {
        (self.inner.clock)()
    }

    fn spawn(&self, job: impl std::future::Future<Output = ()> + Send + 'static) {
        let handle = tokio::spawn(job);
        let mut jobs = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        jobs.retain(|j| !j.is_finished());
        jobs.push(handle);
    }

    fn end_later(&self, ready: Arc<dyn Ready>) {
        self.spawn(ready.end());
    }

    /// How a program of `role` comes back after its message.
    fn back(&self, role: &str) -> Back {
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        let role = role.to_string();
        Arc::new(move |ready: Arc<dyn Ready>| match weak.upgrade() {
            Some(inner) => Keeper { inner }.put_back(&role, ready),
            None => {
                tokio::spawn(ready.end());
            }
        })
    }

    /// A program back from its message: kept as the role's idle one when it still fits and the role has none; ended
    /// otherwise (at most one idle program per role).
    fn put_back(&self, role: &str, ready: Arc<dyn Ready>) {
        let keep = self.is_on() && ready.alive() && {
            let mut slots = self.lock();
            match slots.get_mut(role) {
                Some(slot) if slot.settings == ready.settings() && slot.idle.is_none() => {
                    slot.idle = Some((ready.clone(), self.now()));
                    true
                }
                _ => false,
            }
        };
        if !keep {
            self.end_later(ready);
        }
    }

    /// The idle program of `role` when it fits `settings`; an idle one that doesn't (other settings, past its 5
    /// minutes, dead or signed in differently) is ended.
    fn take_idle(&self, role: &str, settings: &str) -> Option<Arc<dyn Ready>> {
        let now = self.now();
        let (found, stale) = {
            let mut slots = self.lock();
            let slot = slots.entry(role.to_string()).or_insert_with(|| Slot {
                settings: settings.to_string(),
                idle: None,
                last: None,
                preparing: false,
            });
            let idle = slot.idle.take();
            slot.settings = settings.to_string();
            match idle {
                Some((r, since)) if r.settings() == settings && now - since < self.inner.idle_s => {
                    (Some(r), None)
                }
                Some((r, _)) => (None, Some(r)),
                None => (None, None),
            }
        };
        if let Some(r) = stale {
            self.end_later(r);
        }
        match found {
            Some(r) if r.alive() => Some(r),
            Some(r) => {
                self.end_later(r);
                None
            }
            None => None,
        }
    }

    /// A call's start (what [`Kept`]'s `start` does): the role's idle program when it fits, else a new one made
    /// ready now; today's start when the switch is off or the call never uses a kept program.
    pub async fn start(
        &self,
        role: &str,
        adapter: Arc<dyn Keepable>,
        task: &Task,
        request: &ModelRequest,
        run_token: &str,
    ) -> Result<Started, RuntimeError> {
        let has_token = !run_token.is_empty();
        let settings = match adapter.keep_settings(Some(request), has_token) {
            Some(s) if self.is_on() => s,
            _ => return adapter.start(task, request, run_token).await,
        };
        let back = self.back(role);
        let idle = self.take_idle(role, &settings);
        if let Some(slot) = self.lock().get_mut(role) {
            slot.last = Some(adapter.clone());
        }
        let reused = idle.is_some();
        let ready = match idle {
            Some(r) => r,
            None => adapter.prepare(&settings, back.clone()).await?,
        };
        let out = match adapter
            .take(ready.clone(), back.clone(), task, request, run_token)
            .await
        {
            // the ready program died before it took the message (a crash): one new program, once
            Err(_) if reused && !ready.alive() => {
                let fresh = adapter.prepare(&settings, back.clone()).await?;
                adapter.take(fresh, back, task, request, run_token).await
            }
            other => other,
        };
        if out.is_ok() && adapter.once() {
            self.prepare(role);
        }
        out
    }

    /// Starts `role`'s next program ahead (the app said the owner started typing, or a run-once program was just
    /// used), for the settings of the role's latest call; nothing when the role has one idle or being prepared, or
    /// has had no call yet ([`Keeper::prepare_for`] names the adapter then).
    pub fn prepare(&self, role: &str) {
        let last = self
            .lock()
            .get(role)
            .and_then(|s| s.last.clone().map(|a| (a, s.settings.clone())));
        if let Some((adapter, settings)) = last {
            self.prepare_settings(role, adapter, settings);
        }
    }

    /// [`Keeper::prepare`] with the adapter a call of `role` would be built with (before the role's first call), for
    /// a call with or without FERB's tools.
    pub fn prepare_for(&self, role: &str, adapter: Arc<dyn Keepable>, has_token: bool) {
        if let Some(settings) = adapter.keep_settings(None, has_token) {
            self.prepare_settings(role, adapter, settings);
        }
    }

    fn prepare_settings(&self, role: &str, adapter: Arc<dyn Keepable>, settings: String) {
        if !self.is_on() {
            return;
        }
        {
            let mut slots = self.lock();
            let slot = slots.entry(role.to_string()).or_insert_with(|| Slot {
                settings: settings.clone(),
                idle: None,
                last: None,
                preparing: false,
            });
            if slot.preparing || slot.idle.is_some() {
                return;
            }
            slot.settings = settings.clone();
            slot.preparing = true;
            slot.last = Some(adapter.clone());
        }
        let keeper = self.clone();
        let role = role.to_string();
        let back = self.back(&role);
        self.spawn(async move {
            let made = adapter.prepare(&settings, back).await;
            if let Some(slot) = keeper.lock().get_mut(&role) {
                slot.preparing = false;
            }
            if let Ok(ready) = made {
                keeper.put_back(&role, ready);
            }
        });
    }

    /// The idle programs past their 5 minutes end; one found dead or signed in differently ends and a new one is
    /// started in its place (a crash). The daemon calls this on a timer ([`Keeper::run_sweeps`]); every call's start
    /// also looks.
    pub fn sweep(&self) {
        let now = self.now();
        let mut ended: Vec<Arc<dyn Ready>> = Vec::new();
        let mut crashed: Vec<String> = Vec::new();
        {
            let mut slots = self.lock();
            for (role, slot) in slots.iter_mut() {
                let Some((r, since)) = slot.idle.take() else {
                    continue;
                };
                if now - since >= self.inner.idle_s {
                    ended.push(r);
                } else if !r.alive() {
                    ended.push(r);
                    crashed.push(role.clone());
                } else {
                    slot.idle = Some((r, since));
                }
            }
        }
        for r in ended {
            self.end_later(r);
        }
        for role in crashed {
            self.prepare(&role);
        }
    }

    /// [`Keeper::sweep`] every `every_s` seconds until the keeper is shut down.
    pub fn run_sweeps(&self, every_s: f64) -> JoinHandle<()> {
        let weak = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs_f64(every_s.max(0.01))).await;
                let Some(inner) = weak.upgrade() else { return };
                if inner.closed.load(Ordering::SeqCst) {
                    return;
                }
                Keeper { inner }.sweep();
            }
        })
    }

    /// Ends every idle program now (the owner changed a runtime's settings or sign-in; the next call starts one).
    pub fn end_idle(&self) {
        let idle: Vec<Arc<dyn Ready>> = self
            .lock()
            .values_mut()
            .filter_map(|s| s.idle.take().map(|(r, _)| r))
            .collect();
        for r in idle {
            self.end_later(r);
        }
    }

    /// The daemon stops: nothing is kept from now on, every idle program ends, and what is being ended or prepared
    /// is waited for. A program still in use ends with its message, as today.
    pub async fn shut_down(&self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        self.end_idle();
        loop {
            let jobs: Vec<JoinHandle<()>> = self
                .inner
                .jobs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .drain(..)
                .collect();
            if jobs.is_empty() {
                break;
            }
            for j in jobs {
                let _ = j.await;
            }
            // a program prepared while shutting down came back and was ended: wait for that end too
            self.end_idle();
        }
    }
}

/// An adapter whose calls go through the keeper: what the daemon's runtime factory hands the worker for a role it
/// keeps ready. Its `start` takes the role's ready program when one fits and starts a new one when none does.
pub struct Kept {
    pub keeper: Keeper,
    pub role: String,
    pub adapter: Arc<dyn Keepable>,
}

impl Kept {
    pub fn new(keeper: Keeper, role: &str, adapter: Arc<dyn Keepable>) -> Kept {
        Kept {
            keeper,
            role: role.to_string(),
            adapter,
        }
    }
}

impl Adapter for Kept {
    fn name(&self) -> &str {
        self.adapter.name()
    }

    fn start<'a>(
        &'a self,
        task: &'a Task,
        request: &'a ModelRequest,
        run_token: &'a str,
    ) -> BoxFuture<'a, Result<Started, RuntimeError>> {
        Box::pin(
            self.keeper
                .start(&self.role, self.adapter.clone(), task, request, run_token),
        )
    }
}
