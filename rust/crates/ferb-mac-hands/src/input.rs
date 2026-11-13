//! Process-directed and explicitly authorized foreground input (ferb/act/mac/input.py `RealInput`, `Step`). Never
//! suppress owner events or choose recovery; foreground input stops when the owner's activity begins. Background
//! sequences serialize on the GUI lock and release a held button when interrupted.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::error::{HandError, Result};
use crate::keys::{self, Button, Event, Planned, Route};
use crate::locks::GuiLock;

pub const STEP_S: f64 = 0.02;
/// HAND-409.
pub const PAUSED: &str = "FERB is paused: no foreground input";
pub const UNMONITORED: &str = "FERB can't tell whether the owner is using the Mac (its owner-input watcher isn't \
running: the owner allows Input Monitoring for FERB in System Settings → Privacy & Security → Input Monitoring), so it \
sends no foreground input";

/// One step: posted in a worker thread (its checks read Accessibility and Quartz).
pub struct Step {
    pub what: String,
    pub post: Box<dyn FnMut() -> Result<()> + Send>,
}

impl Step {
    pub fn new(what: impl Into<String>, post: impl FnMut() -> Result<()> + Send + 'static) -> Step {
        Step {
            what: what.into(),
            post: Box::new(post),
        }
    }

    /// This step with `check` run right before it posts (a guard).
    pub fn guarded(mut self, mut check: impl FnMut() -> Result<()> + Send + 'static) -> Step {
        let what = self.what.clone();
        Step::new(what, move || {
            check()?;
            (self.post)()
        })
    }
}

impl std::fmt::Debug for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Step({:?})", self.what)
    }
}

/// The events FERB posts, each marked with FERB's mark (`native::events`).
pub trait Events: Send + Sync {
    fn post(&self, event: &Event, route: &Route) -> Result<()>;
}

/// How gestures become steps: the real ones post `keys`' events; a test's record what would have gone.
pub trait Gestures: Send + Sync {
    fn click_steps(&self, x: f64, y: f64, pid: Option<i32>) -> Vec<Step>;
    fn type_steps(&self, text: &str, pid: Option<i32>) -> Vec<Step>;
    fn key_steps(&self, combo: &str, pid: Option<i32>) -> Vec<Step>;
    #[allow(clippy::too_many_arguments)]
    fn wheel_steps(
        &self,
        x: f64,
        y: f64,
        lines: i32,
        pid: Option<i32>,
        horizontal: bool,
        window: Option<u32>,
        route: bool,
        pixels: bool,
        origin: (f64, f64),
    ) -> Vec<Step>;
    /// The steps and the cleanup that releases a held button.
    fn pointer_steps(
        &self,
        start: (f64, f64),
        end: (f64, f64),
        kind: &str,
        pid: i32,
        window: Option<u32>,
        route: bool,
        origin: (f64, f64),
    ) -> Result<(Vec<Step>, Cleanup)>;
}

pub type Cleanup = Arc<dyn Fn() + Send + Sync>;

/// The real gestures: `keys`' plans posted through `Events`.
pub struct RealGestures {
    pub events: Arc<dyn Events>,
}

type Held = Arc<Mutex<Option<(Button, (f64, f64), Route)>>>;

fn post_plan(events: &Arc<dyn Events>, p: Planned, held: Option<Held>) -> Step {
    let events = events.clone();
    let what = p.what.clone();
    Step::new(what, move || {
        for (e, r) in &p.events {
            events.post(e, r)?;
            if let Some(h) = &held {
                let mut h = h.lock().expect("lock");
                match e {
                    Event::Down { button, at, .. } => *h = Some((*button, *at, r.clone())),
                    Event::Up { .. } => *h = None,
                    Event::Dragged { at } | Event::Moved { at } => {
                        if let Some(x) = h.as_mut() {
                            x.1 = *at;
                            x.2 = r.clone();
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    })
}

impl Gestures for RealGestures {
    fn click_steps(&self, x: f64, y: f64, pid: Option<i32>) -> Vec<Step> {
        keys::click_steps(x, y, pid)
            .into_iter()
            .map(|p| post_plan(&self.events, p, None))
            .collect()
    }
    fn type_steps(&self, text: &str, pid: Option<i32>) -> Vec<Step> {
        keys::type_steps(text, pid)
            .into_iter()
            .map(|p| post_plan(&self.events, p, None))
            .collect()
    }
    fn key_steps(&self, combo: &str, pid: Option<i32>) -> Vec<Step> {
        keys::key_steps(combo, pid)
            .unwrap_or_default()
            .into_iter()
            .map(|p| post_plan(&self.events, p, None))
            .collect()
    }
    fn wheel_steps(
        &self,
        x: f64,
        y: f64,
        lines: i32,
        pid: Option<i32>,
        horizontal: bool,
        window: Option<u32>,
        route: bool,
        pixels: bool,
        origin: (f64, f64),
    ) -> Vec<Step> {
        keys::wheel_steps(x, y, lines, pid, horizontal, window, route, pixels, origin)
            .into_iter()
            .map(|p| post_plan(&self.events, p, None))
            .collect()
    }
    fn pointer_steps(
        &self,
        start: (f64, f64),
        end: (f64, f64),
        kind: &str,
        pid: i32,
        window: Option<u32>,
        route: bool,
        origin: (f64, f64),
    ) -> Result<(Vec<Step>, Cleanup)> {
        let plans = keys::pointer_steps(start, end, kind, pid, window, route, origin)
            .ok_or_else(|| HandError::Refused("unsupported pointer action".into()))?;
        let held: Held = Arc::new(Mutex::new(None));
        let steps = plans
            .into_iter()
            .map(|p| post_plan(&self.events, p, Some(held.clone())))
            .collect();
        let events = self.events.clone();
        let release: Cleanup = Arc::new(move || {
            let taken = held.lock().expect("lock").take();
            if let Some((button, at, route)) = taken {
                let _ = events.post(
                    &Event::Up {
                        button,
                        at,
                        clicks: 1,
                    },
                    &route,
                );
            }
        });
        Ok((steps, release))
    }
}

pub type Sleep = Arc<dyn Fn(f64) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Runs a blocking closure off the event loop.
pub async fn off<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    match tokio::task::spawn_blocking(f).await {
        Ok(v) => v,
        Err(e) => std::panic::resume_unwind(e.into_panic()),
    }
}

/// input.py:40.
#[derive(Clone)]
pub struct RealInput {
    pub owner_active: Arc<dyn Fn() -> bool + Send + Sync>,
    pub gui: GuiLock,
    pub yield_after_s: f64,
    pub sleep: Sleep,
    pub poll_s: f64,
    pub on_wait: Arc<dyn Fn(&str, bool) + Send + Sync>,
    pub available: Arc<dyn Fn() -> bool + Send + Sync>,
    pub why_unavailable: Arc<dyn Fn() -> String + Send + Sync>,
}

pub fn tokio_sleep() -> Sleep {
    Arc::new(|s| {
        Box::pin(tokio::time::sleep(std::time::Duration::from_secs_f64(
            s.max(0.0),
        )))
    })
}

impl RealInput {
    pub fn new(owner_active: Arc<dyn Fn() -> bool + Send + Sync>, gui: GuiLock) -> Self {
        RealInput {
            owner_active,
            gui,
            yield_after_s: 30.0,
            sleep: tokio_sleep(),
            poll_s: 0.05,
            on_wait: Arc::new(|_, _| {}),
            available: Arc::new(|| true),
            why_unavailable: Arc::new(|| PAUSED.to_string()),
        }
    }

    /// input.py:51: explicit foreground input stops on owner activity; never suppresses or replays owner events.
    pub async fn run(&self, steps: Vec<Step>, holder: &str, what: &str) -> Result<usize> {
        if !(self.available)() {
            return Err(HandError::Refused((self.why_unavailable)()));
        }
        let _held = self.gui.hold(holder).await;
        let total = steps.len();
        for (i, mut step) in steps.into_iter().enumerate() {
            if (self.owner_active)() {
                return Err(HandError::Yielded {
                    done: i,
                    total,
                    what: what.into(),
                });
            }
            off(move || (step.post)()).await?;
            (self.sleep)(STEP_S).await;
        }
        Ok(total)
    }

    /// input.py:63: before bringing an app forward, wait until the owner has been quiet.
    pub async fn quiet(&self, holder: &str, what: &str) -> Result<()> {
        if !(self.owner_active)() {
            return Ok(());
        }
        (self.on_wait)(holder, true);
        let mut waited = 0.0;
        let out = loop {
            if !(self.owner_active)() {
                break Ok(());
            }
            if waited >= self.yield_after_s {
                break Err(HandError::Yielded {
                    done: 0,
                    total: 1,
                    what: what.into(),
                });
            }
            (self.sleep)(self.poll_s).await;
            waited += self.poll_s;
        };
        (self.on_wait)(holder, false);
        out
    }

    /// input.py:78: events posted to one app's process; under the GUI lock, given up between groups of
    /// `release_every` steps (typed characters), never inside a gesture. `cleanup` runs once: at the end, or where it
    /// stopped.
    pub async fn to_process(
        &self,
        steps: Vec<Step>,
        holder: &str,
        cleanup: Option<Cleanup>,
        release_every: Option<usize>,
    ) -> Result<usize> {
        let total = steps.len();
        let size = match release_every {
            Some(n) if n > 0 => n,
            _ => total.max(1),
        };
        let mut groups: Vec<Vec<Step>> = vec![];
        let mut it = steps.into_iter().peekable();
        while it.peek().is_some() {
            groups.push(it.by_ref().take(size).collect());
        }
        if groups.is_empty() {
            groups.push(vec![]);
        }
        let last = groups.len() - 1;
        /// A stop (the future dropped) mid-gesture still releases a held button (Python's `finally`).
        struct Release(Option<Cleanup>);
        impl Drop for Release {
            fn drop(&mut self) {
                if let Some(c) = self.0.take() {
                    c();
                }
            }
        }
        let mut release = Release(cleanup.clone());
        for (gi, group) in groups.into_iter().enumerate() {
            let _held = self.gui.hold(holder).await;
            let mut failed = None;
            for mut step in group {
                match off(move || (step.post)()).await {
                    Ok(()) => (self.sleep)(STEP_S).await,
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                }
            }
            if failed.is_some() || gi == last {
                if let Some(c) = release.0.take() {
                    off(move || c()).await;
                }
            }
            if let Some(e) = failed {
                return Err(e);
            }
        }
        Ok(total)
    }
}
