//! Port of `ferb/clock/clock.py`: wall time for timestamps and schedules, monotonic time for elapsed time and
//! timeouts. Every part of FERB takes its time from a [`Clock`] it is given (TEST-508): the daemon gives
//! [`SystemClock`], tests give [`FakeClock`], which moves only when told.
//!
//! The trait is `ferb_core::interfaces::Clock`; [`ZonedClock`] adds the zone the wall clock reads in (Python's
//! `astimezone()` and `local_zone()` read the Mac's), so text shown to models names it.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{FixedOffset, TimeZone};
use tokio::sync::watch;

pub use ferb_core::interfaces::{BoxFuture, Clock};

use crate::py::{self, Time};
use crate::zone::Zone;

/// A clock that also says which zone its wall time is in.
pub trait ZonedClock: Clock {
    fn zone(&self) -> Zone;

    /// `now.strftime("%Z")`: the zone's abbreviation now.
    fn tzname(&self) -> String {
        self.zone().abbreviation(&self.now())
    }
}

/// The re-check slice of [`SystemClock::sleep_until`] (`clock.py:21`): sleep and clock changes are noticed.
pub const SLEEP_SLICE_S: f64 = 30.0;

/// The real clock: `datetime.now().astimezone()` and `time.monotonic()`.
pub struct SystemClock {
    zone: Zone,
    started: Instant,
}

impl SystemClock {
    /// In the given zone (the daemon passes the Mac's, `zone::local_zone()`).
    pub fn new(zone: Zone) -> SystemClock {
        SystemClock {
            zone,
            started: Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Time {
        let utc = chrono::Utc::now();
        let t = FixedOffset::east_opt(0)
            .expect("zero offset")
            .from_utc_datetime(&utc.naive_utc());
        // microseconds, as Python's datetime holds
        let t =
            t - chrono::TimeDelta::nanoseconds(i64::from(chrono::Timelike::nanosecond(&t) % 1000));
        self.zone.local(&t)
    }

    fn monotonic(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn sleep_until(&self, t: Time) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            loop {
                let delay = py::total_seconds(&t, &self.now());
                if delay <= 0.0 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs_f64(delay.min(SLEEP_SLICE_S)))
                    .await;
            }
        })
    }
}

impl ZonedClock for SystemClock {
    fn zone(&self) -> Zone {
        self.zone
    }
}

struct FakeState {
    now: Time,
    mono: f64,
}

/// The test clock (`clock.py:23`): time moves only by [`FakeClock::advance`] or [`FakeClock::set_wall`]; sleepers
/// wake when time passes their deadline. Cheap to clone; clones share the time.
#[derive(Clone)]
pub struct FakeClock {
    state: Arc<Mutex<FakeState>>,
    changed: watch::Sender<u64>,
    zone: Zone,
}

impl FakeClock {
    /// Starting at `start` (Python's default: 2026-01-01 09:00 UTC), monotonic at 1000.0.
    pub fn new(start: Option<Time>) -> FakeClock {
        let start = start.unwrap_or_else(|| {
            FixedOffset::east_opt(0)
                .expect("zero offset")
                .with_ymd_and_hms(2026, 1, 1, 9, 0, 0)
                .single()
                .expect("a valid start")
        });
        let (changed, _) = watch::channel(0);
        FakeClock {
            state: Arc::new(Mutex::new(FakeState {
                now: start,
                mono: 1000.0,
            })),
            changed,
            zone: Zone::utc(),
        }
    }

    /// The zone [`ZonedClock::zone`] reports (UTC unless set).
    pub fn with_zone(mut self, zone: Zone) -> FakeClock {
        self.zone = zone;
        self
    }

    fn bump(&self) {
        self.changed.send_modify(|n| *n += 1);
    }

    /// `advance(seconds)`: wall and monotonic time move together (a `timedelta(seconds=...)`, rounded as Python's).
    pub fn advance(&self, seconds: f64) {
        {
            let mut s = self.state.lock().expect("clock lock");
            let us = py::timedelta_us(seconds, py::US_PER_SECOND).expect("a span a test can use");
            s.now = py::plus_us(&s.now, us).expect("a time in range");
            s.mono += seconds;
        }
        self.bump();
    }

    /// `set_wall(t)`: a wall-clock change (time zone, NTP); monotonic time is unaffected.
    pub fn set_wall(&self, t: Time) {
        self.state.lock().expect("clock lock").now = t;
        self.bump();
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Time {
        self.state.lock().expect("clock lock").now
    }

    fn monotonic(&self) -> f64 {
        self.state.lock().expect("clock lock").mono
    }

    fn sleep_until(&self, t: Time) -> BoxFuture<'_, ()> {
        let mut rx = self.changed.subscribe();
        Box::pin(async move {
            while self.now() < t {
                if rx.changed().await.is_err() {
                    break;
                }
            }
        })
    }
}

impl ZonedClock for FakeClock {
    fn zone(&self) -> Zone {
        self.zone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // decision: TEST-508
    #[tokio::test]
    async fn a_fake_sleeper_wakes_only_when_time_passes_its_deadline() {
        let clock = FakeClock::new(None);
        let deadline = py::plus_us(&clock.now(), 90 * py::US_PER_SECOND).unwrap();
        let c2 = clock.clone();
        let sleeper = tokio::spawn(async move { c2.sleep_until(deadline).await });
        tokio::task::yield_now().await;
        clock.advance(60.0);
        tokio::task::yield_now().await;
        assert!(!sleeper.is_finished());
        clock.set_wall(py::plus_us(&clock.now(), 31 * py::US_PER_SECOND).unwrap());
        sleeper.await.unwrap();
        assert_eq!(clock.monotonic(), 1060.0);
    }
}
