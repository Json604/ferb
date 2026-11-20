//! FERB's root helper (kb APP-986, T-1274 phase 2): the smallest program that runs as root, inside FERB.app, registered
//! with `SMAppService.daemon` and approved once by the owner in System Settings. Its whole interface is fixed
//! (`ferb_core::power::helper::Request`):
//!
//! - a sleep-disable lease: while the daemon renews it the Mac doesn't sleep at all, also with the lid closed on
//!   battery (macOS's `SleepDisabled`, what `pmset disablesleep 1` sets). It ends by itself when the daemon stops
//!   renewing it, and a lease left behind (a crash, a power cut) is cleared when the helper starts, at boot;
//! - FERB's own scheduled wakes (`IOPMSchedulePowerEvent` under FERB's own id), in the future, at most
//!   [`helper::MAX_WAKES`] pending, at most [`helper::MAX_AHEAD_S`] ahead.
//!
//! It answers only ferbd: the caller's running code must be signed with the daemon's identifier
//! ([`helper::CALLER_ID`]) by the same certificate as the helper itself (FERB's `ferb` tool program, the shell and every
//! other process are refused); the shell's sandbox can't reach its socket; its program and plist are under rule 6's
//! protection (GATE-1). It changes nothing else: no FERB config, records, protections or service definitions.
//!
//! [`Helper`] is the logic, over the [`RootPower`] and [`Marker`] seams; `mac` is the Mac's (IOKit, the marker file, the
//! caller's signature).

use std::sync::{Mutex, PoisonError};

pub use ferb_core::power::helper;
use helper::Request;
use serde_json::{json, Value};

#[cfg(target_os = "macos")]
pub mod mac;

/// What only root may do, as the helper uses it.
pub trait RootPower: Send + Sync {
    /// Sets or clears the Mac's sleep kill switch (`SleepDisabled`).
    fn set_sleep_disabled(&self, on: bool) -> Result<(), String>;
    /// FERB's own scheduled wakes (POSIX seconds); never another app's.
    fn scheduled(&self) -> Result<Vec<i64>, String>;
    fn schedule(&self, at: i64) -> Result<(), String>;
    fn cancel(&self, at: i64) -> Result<(), String>;
}

/// The record that a lease is in force (a root-owned file): set before the switch is set, cleared after it is cleared,
/// so a lease a crash left behind is found at the next start.
pub trait Marker: Send + Sync {
    fn exists(&self) -> bool;
    fn set(&self, on: bool) -> Result<(), String>;
}

/// The helper's state: the lease's end (monotonic seconds) while one is in force.
pub struct Helper<P: RootPower, M: Marker> {
    power: P,
    marker: M,
    lease_until: Mutex<Option<f64>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn failed(e: impl Into<String>) -> Value {
    json!({"ok": false, "error": e.into()})
}

impl<P: RootPower, M: Marker> Helper<P, M> {
    pub fn new(power: P, marker: M) -> Self {
        Helper {
            power,
            marker,
            lease_until: Mutex::new(None),
        }
    }

    /// At the helper's start (at boot, or after it was restarted): a lease left behind is cleared.
    pub fn boot(&self) -> bool {
        if !self.marker.exists() {
            return false;
        }
        let _ = self.power.set_sleep_disabled(false);
        let _ = self.marker.set(false);
        true
    }

    /// Whether a lease is in force.
    pub fn leased(&self) -> bool {
        lock(&self.lease_until).is_some()
    }

    fn end_lease(&self) -> Result<(), String> {
        let mut l = lock(&self.lease_until);
        if l.take().is_some() || self.marker.exists() {
            self.power.set_sleep_disabled(false)?;
            self.marker.set(false)?;
        }
        Ok(())
    }

    /// The watchdog: a lease not renewed in time ends.
    pub fn tick(&self, now: f64) {
        let over = lock(&self.lease_until).is_some_and(|u| now >= u);
        if over {
            let _ = self.end_lease();
        }
    }

    /// The helper stops (launchd's SIGTERM, an unregister): its lease goes with it.
    pub fn stop(&self) {
        let _ = self.end_lease();
    }

    /// One request of the daemon's: `now` monotonic seconds, `wall` POSIX seconds.
    pub fn handle(&self, req: &Request, now: f64, wall: i64) -> Value {
        match req {
            Request::Lease { seconds } => {
                let mut l = lock(&self.lease_until);
                if l.is_none() {
                    // the marker first: a crash between the two leaves a marker, which the next start clears
                    if let Err(e) = self.marker.set(true) {
                        return failed(e);
                    }
                    if let Err(e) = self.power.set_sleep_disabled(true) {
                        let _ = self.marker.set(false);
                        return failed(e);
                    }
                }
                *l = Some(now + *seconds as f64);
                json!({"ok": true, "seconds": seconds})
            }
            Request::Release => match self.end_lease() {
                Ok(()) => json!({"ok": true}),
                Err(e) => failed(e),
            },
            Request::Wakes => match self.power.scheduled() {
                Ok(w) => json!({"ok": true, "wakes": w}),
                Err(e) => failed(e),
            },
            Request::Wake { at } => {
                if *at <= wall + 30 || *at > wall + helper::MAX_AHEAD_S {
                    return failed("a wake must be between 30 seconds and 8 days ahead");
                }
                let have = match self.power.scheduled() {
                    Ok(w) => w,
                    Err(e) => return failed(e),
                };
                if have.contains(at) {
                    return json!({"ok": true});
                }
                if have.len() >= helper::MAX_WAKES {
                    return failed(format!(
                        "FERB keeps at most {} wakes pending",
                        helper::MAX_WAKES
                    ));
                }
                match self.power.schedule(*at) {
                    Ok(()) => json!({"ok": true}),
                    Err(e) => failed(e),
                }
            }
            Request::Unwake { at } => match self.power.cancel(*at) {
                Ok(()) => json!({"ok": true}),
                Err(e) => failed(e),
            },
            Request::Status => {
                let wakes = self.power.scheduled().map(|w| w.len()).unwrap_or(0);
                json!({"ok": true, "leased": self.leased(), "wakes": wakes})
            }
        }
    }

    /// One line of the daemon's: its answer line (anything outside the interface is refused).
    pub fn line(&self, line: &str, now: f64, wall: i64) -> String {
        let answer = match Request::parse(line) {
            Ok(req) => self.handle(&req, now, wall),
            Err(e) => failed(e),
        };
        format!("{answer}\n")
    }
}
