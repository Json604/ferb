//! Keeping the Mac awake for FERB's work (kb T-1274, APP-980..APP-985): the portable seams the daemon's insomnia area
//! holds its power assertions and reads the Mac's power facts through. The Mac's own implementation (IOKit's power
//! assertions, the battery, the lid, the heat, the lock) is `ferb_platform_macos::power`; tests use stand-ins.
//!
//! A hold is a lease: [`KeepAwake::hold`] takes or renews it for `lease_s` seconds, after which macOS lets it go by
//! itself (`kIOPMAssertionTimeoutActionRelease`), and macOS lets every hold go when the process that took it dies. So a
//! daemon that hangs or crashes never keeps the Mac awake for long.

use std::fmt;

/// What one hold keeps from happening.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Hold {
    /// The Mac's sleep after the owner was idle (`PreventUserIdleSystemSleep`, `caffeinate -i`). The display may still
    /// sleep (and lock).
    IdleSleep,
    /// The Mac's sleep on AC power, also with the lid closed (`PreventSystemSleep`, `caffeinate -s`). macOS honours it
    /// only on AC power.
    SystemSleep,
    /// The display's idle sleep (`PreventUserIdleDisplaySleep`, `caffeinate -d`): only while the insomnia shield
    /// covers the screen (the Mac stays unlocked under it; a sleeping display would lock it). Never otherwise.
    Display,
}

impl Hold {
    pub const ALL: [Hold; 3] = [Hold::IdleSleep, Hold::SystemSleep, Hold::Display];

    pub fn as_str(self) -> &'static str {
        match self {
            Hold::IdleSleep => "idle_sleep",
            Hold::SystemSleep => "system_sleep",
            Hold::Display => "display",
        }
    }
}

impl fmt::Display for Hold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The power assertions FERB's daemon holds.
pub trait KeepAwake: Send + Sync {
    /// Takes the hold, or renews it, for `lease_s` seconds. `reason` is shown by macOS (`pmset -g assertions`, the power
    /// log): generic words only, never a task's title.
    fn hold(&self, hold: Hold, reason: &str, lease_s: u64) -> Result<(), String>;
    /// Lets the hold go now (nothing when none is held).
    fn release(&self, hold: Hold);
}

/// How hot the Mac is, as macOS's thermal pressure says it (`com.apple.system.thermalpressurelevel`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Thermal {
    Nominal,
    /// "moderate".
    Fair,
    /// "heavy": macOS slows the Mac down.
    Serious,
    /// "trapping" or "sleeping": macOS is about to put the Mac to sleep.
    Critical,
}

impl Thermal {
    pub fn as_str(self) -> &'static str {
        match self {
            Thermal::Nominal => "nominal",
            Thermal::Fair => "fair",
            Thermal::Serious => "serious",
            Thermal::Critical => "critical",
        }
    }

    /// The level macOS's thermal pressure notification gives (`OSThermalPressureLevel`: 0 nominal, 1 moderate, 2
    /// heavy, 3 trapping, 4 sleeping).
    pub fn from_pressure(level: u64) -> Thermal {
        match level {
            0 => Thermal::Nominal,
            1 => Thermal::Fair,
            2 => Thermal::Serious,
            _ => Thermal::Critical,
        }
    }
}

/// The Mac's power facts now; each one `None` when it couldn't be read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PowerReading {
    /// 0-100, the internal battery (none on a Mac without one).
    pub battery: Option<i64>,
    /// Running on the battery (not on AC power).
    pub on_battery: Option<bool>,
    /// The built-in display's lid is closed (`AppleClamshellState`).
    pub lid_closed: Option<bool>,
    pub thermal: Option<Thermal>,
    /// The login session's screen is locked (`CGSSessionScreenIsLocked`).
    pub screen_locked: Option<bool>,
}

/// Reads the Mac's power facts.
pub trait PowerFacts: Send + Sync {
    fn read(&self) -> PowerReading;
}

/// What the Mac's power watch tells (DMN-613): the power source or the lid changed, the Mac is about to sleep, or it
/// woke.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerEvent {
    /// The power source (AC or battery), the battery level or the lid changed.
    Changed,
    /// The Mac is about to sleep (macOS asks; the watch answers after the handler's wait).
    WillSleep,
    /// The Mac woke.
    Woke,
}

/// The handler of the power watch: answers how many milliseconds macOS's sleep should wait for it (only
/// [`PowerEvent::WillSleep`]'s answer counts; macOS allows at most 30 s).
pub type PowerHandler = std::sync::Arc<dyn Fn(PowerEvent) -> u64 + Send + Sync>;

/// Tells the daemon at once when the power source, the lid or sleep changes, instead of at its next look.
pub trait PowerWatch: Send + Sync {
    /// Starts watching for this process's life; false when it couldn't.
    fn start(&self, handler: PowerHandler) -> bool;
}

/// The owner's "Lock my Mac" (APP-983): FERB never locks by itself. Puts the display to sleep (`pmset displaysleepnow`,
/// no root), which locks a Mac that asks for the password at once, and reads the lock back.
pub trait LockMac: Send + Sync {
    /// Ok(true): the screen is locked now; Ok(false): the display slept but the screen isn't locked (the password is
    /// not asked at once).
    fn lock_now(&self) -> Result<bool, String>;
}

/// APP-986: FERB's root helper (`com.ferb.power`, inside FERB.app, registered with `SMAppService.daemon` and approved
/// once by the owner), as the daemon reaches it.
pub trait RootHelper: Send + Sync {
    /// Whether the helper answers (registered, approved and running) and what it holds.
    fn state(&self) -> HelperState;
    /// Keeps the Mac from sleeping at all (also with the lid closed on battery) for `seconds` more; the helper lets it
    /// go by itself when the lease isn't renewed.
    fn lease(&self, seconds: u64) -> Result<(), String>;
    /// Lets the lease go now.
    fn release(&self) -> Result<(), String>;
    /// FERB's own scheduled wakes now (POSIX seconds).
    fn wakes(&self) -> Result<Vec<i64>, String>;
    /// A wake at `at` (POSIX seconds, in the future).
    fn wake(&self, at: i64) -> Result<(), String>;
    /// Cancels FERB's wake at `at`.
    fn unwake(&self, at: i64) -> Result<(), String>;
}

/// What the daemon knows of the helper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperState {
    /// It answers: a closed Mac on battery can be kept working, and scheduled wakes can be set.
    Ready,
    /// It doesn't answer (not registered, not approved in System Settings, or not this build's).
    Absent,
}

impl HelperState {
    pub fn as_str(self) -> &'static str {
        match self {
            HelperState::Ready => "ready",
            HelperState::Absent => "absent",
        }
    }
}

/// The root helper's fixed interface (APP-986): one JSON object per line each way. Nothing else is accepted.
pub mod helper {
    use serde_json::{json, Value};

    /// Its launchd label (FERB.app's `Contents/Library/LaunchDaemons/com.ferb.power.plist`).
    pub const LABEL: &str = "com.ferb.power";
    /// The plist inside FERB.app (`SMAppService.daemon(plistName:)`), under rule 6's protection (GATE-1).
    pub const BUNDLE_PLIST: &str = "Contents/Library/LaunchDaemons/com.ferb.power.plist";
    /// The helper's program inside FERB.app.
    pub const BUNDLE_PROGRAM: &str = "Contents/Helpers/ferb-power";
    /// Where it listens (root-owned; any process may connect, only ferbd's signature is answered). The shell's sandbox
    /// denies it.
    pub const SOCKET: &str = "/var/run/com.ferb.power.sock";
    /// The only caller it answers: the daemon's signing identifier (FERB's `ferb` tool program is refused).
    pub const CALLER_ID: &str = "com.ferb.daemon";
    /// The longest lease one call takes (seconds): a daemon that stops renewing lets the Mac sleep within it.
    pub const MAX_LEASE_S: u64 = 300;
    /// The most wakes FERB keeps pending (the owner's number to confirm, rule 18).
    pub const MAX_WAKES: usize = 8;
    /// How far ahead a wake may be (seconds): 8 days.
    pub const MAX_AHEAD_S: i64 = 8 * 24 * 3600;

    /// One request.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Request {
        Lease { seconds: u64 },
        Release,
        Wakes,
        Wake { at: i64 },
        Unwake { at: i64 },
        Status,
    }

    impl Request {
        pub fn to_line(&self) -> String {
            let v = match self {
                Request::Lease { seconds } => json!({"op": "lease", "seconds": seconds}),
                Request::Release => json!({"op": "release"}),
                Request::Wakes => json!({"op": "wakes"}),
                Request::Wake { at } => json!({"op": "wake", "at": at}),
                Request::Unwake { at } => json!({"op": "unwake", "at": at}),
                Request::Status => json!({"op": "status"}),
            };
            format!("{v}\n")
        }

        /// The request a line holds, or why it isn't one (anything outside the fixed interface).
        pub fn parse(line: &str) -> Result<Request, String> {
            let v: Value =
                serde_json::from_str(line.trim()).map_err(|_| "not a request".to_string())?;
            let obj = v.as_object().ok_or("not a request")?;
            let int = |k: &str| obj.get(k).and_then(Value::as_i64);
            let allowed =
                |keys: &[&str]| obj.keys().all(|k| k == "op" || keys.contains(&k.as_str()));
            match obj.get("op").and_then(Value::as_str) {
                Some("lease") if allowed(&["seconds"]) => match int("seconds") {
                    Some(s) if s > 0 && s as u64 <= MAX_LEASE_S => {
                        Ok(Request::Lease { seconds: s as u64 })
                    }
                    _ => Err(format!("a lease is 1 to {MAX_LEASE_S} seconds")),
                },
                Some("release") if allowed(&[]) => Ok(Request::Release),
                Some("wakes") if allowed(&[]) => Ok(Request::Wakes),
                Some("status") if allowed(&[]) => Ok(Request::Status),
                Some("wake") if allowed(&["at"]) => int("at")
                    .map(|at| Request::Wake { at })
                    .ok_or("a wake needs at".into()),
                Some("unwake") if allowed(&["at"]) => int("at")
                    .map(|at| Request::Unwake { at })
                    .ok_or("an unwake needs at".into()),
                _ => Err("not a request this helper takes".into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_levels_map_to_heat() {
        assert_eq!(Thermal::from_pressure(0), Thermal::Nominal);
        assert_eq!(Thermal::from_pressure(1), Thermal::Fair);
        assert_eq!(Thermal::from_pressure(2), Thermal::Serious);
        assert_eq!(Thermal::from_pressure(3), Thermal::Critical);
        assert_eq!(Thermal::from_pressure(4), Thermal::Critical);
        assert!(Thermal::Serious > Thermal::Fair);
        assert_eq!(Hold::ALL.len(), 3);
    }

    // decision: APP-986
    #[test]
    fn the_helper_takes_its_fixed_requests_only() {
        use helper::Request;
        for r in [
            Request::Lease { seconds: 120 },
            Request::Release,
            Request::Wakes,
            Request::Wake { at: 1_800_000_000 },
            Request::Unwake { at: 1_800_000_000 },
            Request::Status,
        ] {
            assert_eq!(Request::parse(&r.to_line()), Ok(r.clone()));
        }
        for bad in [
            "",
            "[]",
            r#"{"op": "lease", "seconds": 0}"#,
            r#"{"op": "lease", "seconds": 301}"#,
            r#"{"op": "lease", "seconds": 60, "also": 1}"#,
            r#"{"op": "run", "cmd": "sh"}"#,
            r#"{"op": "wake"}"#,
            r#"{"op": "sleep_disabled", "value": true}"#,
        ] {
            assert!(Request::parse(bad).is_err(), "{bad}");
        }
    }
}
