//! Recent owner input through a listen-only tap (`ferb/sense/watchers/owner_input.py`). It never suppresses or
//! replays input. FERB's own marked events don't count as owner activity; field capture receives their mark. Recent
//! activity is a fact for yielding, never a saved focus target; input quiet time is not "away from the Mac".
//!
//! The tap itself (a `CGEventTap` on its own run-loop thread) is [`crate::mac`]'s, behind [`TapBackend`]; it starts
//! only once Input Monitoring is granted (SENSE-405), so macOS never prompts.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::manager::OwnerInputTap;

/// owner_input.py:14: the mark FERB's own synthetic events carry ("FERB").
pub const FERB_EVENT_MARK: i64 = 0x4645_5242;

/// A key-down event as text capture reads it, valid only during the tap's callback. Its text is extracted only on
/// request, after capture has checked Secure Input and the focused field (rule 7).
pub trait KeyEvent {
    /// `CGEventGetFlags`.
    fn flags(&self) -> u64;
    /// The text the key carries (`CGEventKeyboardGetUnicodeString`, the whole of it).
    fn text(&self) -> String;
}

/// Text capture's hook: (the key event, whether FERB marked it).
pub type OnText = Arc<dyn Fn(&dyn KeyEvent, bool) + Send + Sync>;

/// What the native tap calls on its thread.
pub trait TapSink: Send + Sync {
    /// One event: a key-down carries its key; `marked` is FERB's own.
    fn event(&self, marked: bool, key: Option<&dyn KeyEvent>);
    /// The tap is meant to run (macOS turned it off: it is turned back on, not while stopping).
    fn wanted(&self) -> bool;
}

/// The native tap.
pub trait TapBackend: Send + Sync {
    /// Create the listen-only tap and its thread; false when macOS didn't create it.
    fn create(&self, sink: Arc<dyn TapSink>) -> bool;
    fn destroy(&self);
}

struct Times {
    last_owner: Option<f64>,
    last_owner_wall: Option<f64>,
}

/// owner_input.py:25.
pub struct OwnerInput {
    pub quiet_s: f64,
    now: Box<dyn Fn() -> f64 + Send + Sync>,
    wall: Box<dyn Fn() -> f64 + Send + Sync>,
    allowed: Box<dyn Fn() -> bool + Send + Sync>,
    times: Mutex<Times>,
    running: AtomicBool,
    on_text: Mutex<Option<OnText>>,
    backend: Box<dyn TapBackend>,
}

struct Sink(Arc<OwnerInput>);

impl TapSink for Sink {
    fn event(&self, marked: bool, key: Option<&dyn KeyEvent>) {
        let hook = self
            .0
            .on_text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let (Some(hook), Some(key)) = (hook, key) {
            // capture must never disturb input delivery
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook(key, marked)));
        }
        if !marked {
            self.0.owner_event();
        }
    }

    fn wanted(&self) -> bool {
        self.0.running.load(Ordering::SeqCst)
    }
}

/// The sink a native tap calls for this owner input (also what a test drives).
pub fn sink(oi: Arc<OwnerInput>) -> Arc<dyn TapSink> {
    Arc::new(Sink(oi))
}

impl OwnerInput {
    /// `now`: monotonic seconds; `wall`: the daemon's clock as a POSIX time; `allowed`: Input Monitoring granted.
    pub fn new(
        now: impl Fn() -> f64 + Send + Sync + 'static,
        wall: impl Fn() -> f64 + Send + Sync + 'static,
        allowed: impl Fn() -> bool + Send + Sync + 'static,
        backend: Box<dyn TapBackend>,
    ) -> Arc<OwnerInput> {
        Arc::new(OwnerInput {
            quiet_s: 1.5,
            now: Box::new(now),
            wall: Box::new(wall),
            allowed: Box::new(allowed),
            times: Mutex::new(Times {
                last_owner: None,
                last_owner_wall: None,
            }),
            running: AtomicBool::new(false),
            on_text: Mutex::new(None),
            backend,
        })
    }

    /// owner_input.py:42.
    pub fn owner_event(&self) {
        let mut t = self
            .times
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        t.last_owner = Some((self.now)());
        t.last_owner_wall = Some((self.wall)());
    }

    /// owner_input.py:46: the owner touched input within the quiet period: GUI work must wait.
    pub fn active(&self) -> bool {
        let t = self
            .times
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        t.last_owner
            .is_some_and(|l| (self.now)() - l < self.quiet_s)
    }

    /// The wall-clock time of the owner's last input (the time header, idle).
    pub fn last_owner_wall(&self) -> Option<f64> {
        self.times
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last_owner_wall
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Text capture's hook (None: no capture).
    pub fn set_on_text(&self, hook: Option<OnText>) {
        *self
            .on_text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = hook;
    }

    /// owner_input.py:50: no tap without the grant (macOS never prompts; setup asks for it).
    pub fn start_tap(self: &Arc<Self>) -> bool {
        if self.is_running() {
            return true;
        }
        if !(self.allowed)() {
            return false;
        }
        self.running.store(true, Ordering::SeqCst);
        let created = self.backend.create(Arc::new(Sink(self.clone())));
        if !created {
            self.running.store(false, Ordering::SeqCst);
        }
        created
    }

    /// owner_input.py:108: not running first, so the callback never turns it back on.
    pub fn stop_tap(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.backend.destroy();
    }
}

/// The manager's view (manager.py `_tap_state`).
pub struct Tap(pub Arc<OwnerInput>);

impl OwnerInputTap for Tap {
    fn allowed(&self) -> Result<bool, String> {
        Ok((self.0.allowed)())
    }
    fn running(&self) -> bool {
        self.0.is_running()
    }
    fn start(&self) -> Result<bool, String> {
        Ok(self.0.start_tap())
    }
    fn stop(&self) {
        self.0.stop_tap()
    }
}
