//! APP-986: the root helper's logic over stand-ins for the sleep switch, the wakes and the marker file. Nothing here
//! touches the Mac's power settings.

use std::sync::{Arc, Mutex};

use ferb_power_helper::helper::{Request, MAX_AHEAD_S, MAX_WAKES};
use ferb_power_helper::{Helper, Marker, RootPower};

#[derive(Default)]
struct Power {
    disabled: Mutex<bool>,
    switches: Mutex<Vec<bool>>,
    wakes: Mutex<Vec<i64>>,
}

struct P(Arc<Power>);
impl RootPower for P {
    fn set_sleep_disabled(&self, on: bool) -> Result<(), String> {
        *self.0.disabled.lock().unwrap() = on;
        self.0.switches.lock().unwrap().push(on);
        Ok(())
    }
    fn scheduled(&self) -> Result<Vec<i64>, String> {
        Ok(self.0.wakes.lock().unwrap().clone())
    }
    fn schedule(&self, at: i64) -> Result<(), String> {
        self.0.wakes.lock().unwrap().push(at);
        Ok(())
    }
    fn cancel(&self, at: i64) -> Result<(), String> {
        self.0.wakes.lock().unwrap().retain(|w| *w != at);
        Ok(())
    }
}

#[derive(Default)]
struct Mark(Mutex<bool>);
struct M(Arc<Mark>);
impl Marker for M {
    fn exists(&self) -> bool {
        *self.0 .0.lock().unwrap()
    }
    fn set(&self, on: bool) -> Result<(), String> {
        *self.0 .0.lock().unwrap() = on;
        Ok(())
    }
}

fn helper() -> (Arc<Power>, M, Helper<P, M>) {
    let (p, m) = (Arc::new(Power::default()), Arc::new(Mark::default()));
    (p.clone(), M(m.clone()), Helper::new(P(p), M(m)))
}

const WALL: i64 = 1_800_000_000;

/// The lease keeps the switch on while renewed and ends by itself when it isn't; the marker is set first and cleared
/// last; a stop ends it.
// decision: DMN-612
// decision: APP-986
#[test]
fn the_lease_ends_when_the_daemon_stops_renewing_it() {
    let (p, m, h) = helper();
    let ok = h.handle(&Request::Lease { seconds: 120 }, 0.0, WALL);
    assert_eq!(ok["ok"], true);
    assert!(*p.disabled.lock().unwrap() && m.exists());
    h.tick(100.0);
    h.handle(&Request::Lease { seconds: 120 }, 100.0, WALL);
    h.tick(200.0);
    assert!(*p.disabled.lock().unwrap(), "renewed at 100: lasts to 220");
    assert_eq!(
        *p.switches.lock().unwrap(),
        vec![true],
        "a renewal doesn't touch the switch"
    );
    h.tick(220.0);
    assert!(!*p.disabled.lock().unwrap() && !m.exists() && !h.leased());
    // a release, and a stop
    h.handle(&Request::Lease { seconds: 60 }, 300.0, WALL);
    assert_eq!(h.handle(&Request::Release, 301.0, WALL)["ok"], true);
    assert!(!*p.disabled.lock().unwrap());
    h.handle(&Request::Lease { seconds: 60 }, 400.0, WALL);
    h.stop();
    assert!(!*p.disabled.lock().unwrap() && !m.exists());
}

/// A lease left behind (a crash, a power cut) is cleared at the helper's start; with none, nothing is touched.
// decision: DMN-612
// decision: APP-986
#[test]
fn a_lease_left_behind_is_cleared_at_boot() {
    let (p, m, h) = helper();
    assert!(!h.boot());
    assert!(
        p.switches.lock().unwrap().is_empty(),
        "not FERB's: left alone"
    );
    m.set(true).unwrap();
    *p.disabled.lock().unwrap() = true;
    assert!(h.boot());
    assert!(!*p.disabled.lock().unwrap() && !m.exists());
}

/// Wakes: only in the future and at most 8 days ahead, at most MAX_WAKES pending, one per time; cancelled by time.
// decision: DMN-612
// decision: APP-986
#[test]
fn wakes_are_future_bounded_and_few() {
    let (p, _m, h) = helper();
    assert_eq!(
        h.handle(&Request::Wake { at: WALL - 10 }, 0.0, WALL)["ok"],
        false
    );
    assert_eq!(
        h.handle(&Request::Wake { at: WALL + 5 }, 0.0, WALL)["ok"],
        false
    );
    assert_eq!(
        h.handle(
            &Request::Wake {
                at: WALL + MAX_AHEAD_S + 1
            },
            0.0,
            WALL
        )["ok"],
        false
    );
    for i in 0..MAX_WAKES as i64 {
        assert_eq!(
            h.handle(
                &Request::Wake {
                    at: WALL + 3600 * (i + 1)
                },
                0.0,
                WALL
            )["ok"],
            true
        );
    }
    assert_eq!(
        h.handle(&Request::Wake { at: WALL + 3600 }, 0.0, WALL)["ok"],
        true,
        "the same one again"
    );
    let full = h.handle(&Request::Wake { at: WALL + 999_999 }, 0.0, WALL);
    assert_eq!(full["ok"], false, "{full}");
    assert_eq!(p.wakes.lock().unwrap().len(), MAX_WAKES);
    h.handle(&Request::Unwake { at: WALL + 3600 }, 0.0, WALL);
    assert_eq!(
        h.handle(&Request::Wakes, 0.0, WALL)["wakes"]
            .as_array()
            .unwrap()
            .len(),
        MAX_WAKES - 1
    );
}

/// Anything outside the fixed interface is refused with nothing done.
// decision: DMN-610
// decision: APP-986
#[test]
fn anything_else_is_refused() {
    let (p, m, h) = helper();
    for line in [
        "{\"op\": \"sleep_disabled\", \"value\": true}\n",
        "{\"op\": \"shell\", \"cmd\": \"id\"}\n",
        "{\"op\": \"lease\", \"seconds\": 86400}\n",
        "not json\n",
    ] {
        let a: serde_json::Value = serde_json::from_str(&h.line(line, 0.0, WALL)).unwrap();
        assert_eq!(a["ok"], false, "{line}");
    }
    assert!(p.switches.lock().unwrap().is_empty() && !m.exists());
    let a: serde_json::Value =
        serde_json::from_str(&h.line("{\"op\": \"status\"}\n", 0.0, WALL)).unwrap();
    assert_eq!(
        (a["leased"].clone(), a["wakes"].clone()),
        (serde_json::json!(false), serde_json::json!(0))
    );
}
