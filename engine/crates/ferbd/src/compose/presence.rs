//! Whether the owner is at the Mac (main.py:201-203): the owner-input tap (`OwnerInput`, a listen-only event tap that
//! keeps only when the owner last touched keyboard or mouse, never what was typed: no text hook is set) and the Mac's
//! presence (`MacPresence`: wake, unlock, login, never input idleness, ORCH-317). Both are ferb-sense's, with their
//! native sides; the sense manager starts and stops them with sensing. Their readers here: the timers' presence and
//! the owner's return (`_build_timers`), the start in a new login session (`_mac_started`), the notifier's "away"
//! (`owner_idle`), the disturbance sentinel's owner-input facts and the browser drivers' `owner_input_near`
//! (HAND-511).

use std::sync::Arc;

use ferb_sense::watchers::owner_input::OwnerInput;
use ferb_sense::watchers::presence::MacPresence;
use serde_json::{json, Map, Value};

/// disturbance.py `OWNER_NEAR_S` (HAND-511): input this recent is the owner's.
pub const OWNER_NEAR_S: f64 = 3.0;

/// The Mac's own owner-facing watchers, when its parts are on.
#[derive(Clone)]
pub struct MacOwner {
    pub presence: Arc<MacPresence>,
    pub input: Arc<OwnerInput>,
}

impl MacOwner {
    /// main.py:201-203: both on the daemon's clock's wall time.
    #[cfg(target_os = "macos")]
    pub fn build(clock: Arc<dyn ferb_core::interfaces::Clock>) -> MacOwner {
        let started = std::time::Instant::now();
        let (c1, c2) = (clock.clone(), clock);
        let input = OwnerInput::new(
            move || started.elapsed().as_secs_f64(),
            move || c1.now().timestamp_micros() as f64 / 1e6,
            ferb_sense::mac::system::input_monitoring_granted,
            Box::new(ferb_sense::mac::streams::MacTap::default()),
        );
        let presence = MacPresence::new(
            move || c2.now().timestamp_micros() as f64 / 1e6,
            ferb_sense::mac::system::session,
            ferb_sense::mac::system::displays_asleep,
            Some(Box::new(ferb_sense::mac::streams::MacPresenceNative)),
        );
        MacOwner { presence, input }
    }

    /// `{"monitored", "recent", "last_event_wall"}` (main.py:206-208), for the disturbance sentinel.
    pub fn input_facts(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("monitored".into(), json!(self.input.is_running()));
        m.insert("recent".into(), json!(self.input.active()));
        m.insert(
            "last_event_wall".into(),
            json!(self.input.last_owner_wall()),
        );
        m
    }

    /// browsers.py `_owner_input_near` (HAND-511): the owner's input is under way or ended within
    /// [`OWNER_NEAR_S`]; None when the tap isn't running.
    pub fn input_near(&self, now_wall: f64) -> Option<bool> {
        if !self.input.is_running() {
            return None;
        }
        let last = self.input.last_owner_wall();
        Some(self.input.active() || last.is_some_and(|l| now_wall - l <= OWNER_NEAR_S))
    }
}

/// `timers.owner_away` / `timers.present_since` (schedules.py `_build_timers`): out of use now; when the current
/// stretch of use began (None while not watching: unknown counts as present).
pub struct TimerPresence(pub Arc<MacPresence>);

impl ferb_clock::timers::Presence for TimerPresence {
    fn owner_away(&self) -> bool {
        self.0.away()
    }
    fn present_since(&self) -> Option<ferb_clock::py::Time> {
        if !self.0.running() {
            return None;
        }
        let at = match self.0.present_since() {
            Some(s) if !self.0.away() => s,
            _ => 0.0,
        };
        let utc = chrono::DateTime::from_timestamp_micros((at * 1e6).round() as i64)?;
        // `datetime.fromtimestamp(...).astimezone()`: the Mac's local time
        Some(utc.with_timezone(&chrono::Local).fixed_offset())
    }
}
