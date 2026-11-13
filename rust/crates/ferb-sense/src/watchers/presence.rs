//! Whether the Mac is in use (`ferb/sense/watchers/presence.py`; decision: SENSE-201): asleep, its displays asleep,
//! its screen locked or its login session switched away, or not. It never reads input, windows or content, and
//! input idleness is never "away". The notifications and the periodic read run on a thread of [`crate::mac`]'s
//! ([`PresenceNative`]); this is the state they feed. Without the watcher FERB can't tell: `away()` is false and no
//! return comes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::manager::PresenceWatch;

/// presence.py:23.
pub const POLL_S: f64 = 5.0;
/// presence.py:25: (NSWorkspace notification, flag, value).
pub const WORKSPACE: [(&str, &str, bool); 6] = [
    ("NSWorkspaceWillSleepNotification", "asleep", true),
    ("NSWorkspaceDidWakeNotification", "asleep", false),
    (
        "NSWorkspaceScreensDidSleepNotification",
        "displays_asleep",
        true,
    ),
    (
        "NSWorkspaceScreensDidWakeNotification",
        "displays_asleep",
        false,
    ),
    (
        "NSWorkspaceSessionDidResignActiveNotification",
        "switched_away",
        true,
    ),
    (
        "NSWorkspaceSessionDidBecomeActiveNotification",
        "switched_away",
        false,
    ),
];
/// presence.py:30: (distributed notification, flag, value).
pub const DISTRIBUTED: [(&str, &str, bool); 2] = [
    ("com.apple.screenIsLocked", "locked", true),
    ("com.apple.screenIsUnlocked", "locked", false),
];
/// presence.py:31: the flag that holds the Mac out of use, and the signal its clearing gives.
pub const SIGNALS: [(&str, &str); 4] = [
    ("asleep", "wake"),
    ("displays_asleep", "screens_wake"),
    ("locked", "unlock"),
    ("switched_away", "session_active"),
];

/// presence.py:34 `session()`: the login session as CoreGraphics reports it (each part None when not read).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    pub id: Option<String>,
    pub locked: Option<bool>,
    pub on_console: Option<bool>,
}

/// The native side: notifications and the periodic read on FERB's own run-loop thread, until the handle drops.
pub trait PresenceNative: Send + Sync {
    fn watch(&self, presence: Arc<MacPresence>) -> Option<Box<dyn Send + Sync>>;
}

struct Held {
    flags: [bool; 4],
    present_since: Option<f64>,
}

type OnReturn = Arc<dyn Fn(&str) + Send + Sync>;

/// presence.py:59 `MacPresence`.
pub struct MacPresence {
    wall: Box<dyn Fn() -> f64 + Send + Sync>,
    read_session: Box<dyn Fn() -> Session + Send + Sync>,
    read_displays: Box<dyn Fn() -> Option<bool> + Send + Sync>,
    running: AtomicBool,
    held: Mutex<Held>,
    on_return: Mutex<Option<OnReturn>>,
    native: Option<Box<dyn PresenceNative>>,
    handle: Mutex<Option<Box<dyn Send + Sync>>>,
}

fn index(flag: &str) -> Option<usize> {
    SIGNALS.iter().position(|(f, _)| *f == flag)
}

impl MacPresence {
    /// `wall`: the daemon's clock as a POSIX time; `native` None: no thread (tests tell `changed`).
    pub fn new(
        wall: impl Fn() -> f64 + Send + Sync + 'static,
        read_session: impl Fn() -> Session + Send + Sync + 'static,
        read_displays: impl Fn() -> Option<bool> + Send + Sync + 'static,
        native: Option<Box<dyn PresenceNative>>,
    ) -> Arc<MacPresence> {
        Arc::new(MacPresence {
            wall: Box::new(wall),
            read_session: Box::new(read_session),
            read_displays: Box::new(read_displays),
            running: AtomicBool::new(false),
            held: Mutex::new(Held {
                flags: [false; 4],
                present_since: None,
            }),
            on_return: Mutex::new(None),
            native,
            handle: Mutex::new(None),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Held> {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The Mac came back into use: told once, with the signal (presence.py:67 `on_return`).
    pub fn set_on_return(&self, f: Option<OnReturn>) {
        *self
            .on_return
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = f;
    }

    /// Wall-clock start of the current stretch in use (None: unknown).
    pub fn present_since(&self) -> Option<f64> {
        self.lock().present_since
    }

    pub fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// presence.py:73: out of use now. Not running: false.
    pub fn away(&self) -> bool {
        let h = self.lock();
        self.running() && h.flags.iter().any(|f| *f)
    }

    /// presence.py:78: one fact changed; back in use when nothing holds it out any more.
    pub fn changed(&self, flag: &str, value: bool) {
        let back = {
            let mut h = self.lock();
            let Some(i) = index(flag) else {
                return;
            };
            if !self.running() || h.flags[i] == value {
                return;
            }
            let was = h.flags.iter().any(|f| *f);
            h.flags[i] = value;
            was && !h.flags.iter().any(|f| *f)
        };
        if back {
            self.returned(SIGNALS[index(flag).expect("a flag")].1);
        }
    }

    /// presence.py:90: the Mac came back into use (or the daemon started in a new login session: "login").
    pub fn returned(&self, signal: &str) {
        self.lock().present_since = Some((self.wall)());
        let hook = self
            .on_return
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(hook) = hook {
            // telling FERB never stops the watcher
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook(signal)));
        }
    }

    /// presence.py:99: catch up with what can be read now.
    pub fn read(&self) {
        let s = (self.read_session)();
        if let Some(locked) = s.locked {
            self.changed("locked", locked);
        }
        if let Some(on) = s.on_console {
            self.changed("switched_away", !on);
        }
        if let Some(asleep) = (self.read_displays)() {
            self.changed("displays_asleep", asleep);
        }
    }

    /// presence.py:110: the state read now (a locked screen at start is away), then the notifications.
    pub fn start_watch(self: &Arc<Self>) -> bool {
        if self.running() {
            return true;
        }
        {
            let mut h = self.lock();
            self.running.store(true, Ordering::SeqCst);
            h.flags = [false; 4];
        }
        self.read();
        if let Some(native) = &self.native {
            *self
                .handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = native.watch(self.clone());
        }
        true
    }

    /// presence.py:125.
    pub fn stop_watch(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

/// The manager's view.
pub struct Presence(pub Arc<MacPresence>);

impl PresenceWatch for Presence {
    fn start(&self) {
        self.0.start_watch();
    }
    fn stop(&self) {
        self.0.stop_watch();
    }
}
