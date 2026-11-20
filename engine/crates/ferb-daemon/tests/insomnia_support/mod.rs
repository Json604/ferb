//! Stand-ins for insomnia mode's seams (T-1274): the power assertions, the power facts, the lock, the work, the
//! owner's unlock, the journal, the bus and the owner's notices, with a clock the test moves.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use ferb_core::power::{
    HelperState, Hold, KeepAwake, LockMac, PowerFacts, PowerReading, RootHelper,
};
use ferb_core::when::DateTime;
use ferb_daemon::insomnia::{Insomnia, Persist, Saved, Seams, Settings, JOURNAL_KIND};
use serde_json::Value;

#[derive(Default)]
pub struct Mac {
    /// Every hold call: (hold, reason, lease).
    pub holds: Mutex<Vec<(Hold, String, u64)>>,
    pub releases: Mutex<Vec<Hold>>,
    /// What is held now (taken and not released).
    pub held: Mutex<Vec<Hold>>,
    pub reading: Mutex<PowerReading>,
    pub locks: Mutex<u32>,
    pub lock_answer: Mutex<Option<Result<bool, String>>>,
    pub working: Mutex<usize>,
    pub unlocked_at: Mutex<Option<DateTime>>,
    pub settings: Mutex<Settings>,
    pub saved: Mutex<Option<Saved>>,
    pub journal: Mutex<Vec<(String, Value)>>,
    pub published: Mutex<Vec<(String, Value)>>,
    pub told: Mutex<Vec<(String, String)>>,
    /// Wall seconds after the start time, and monotonic seconds.
    pub wall: Mutex<f64>,
    pub mono: Mutex<f64>,
    pub ids: Mutex<u64>,
    pub root: Arc<Root>,
}

pub fn t0() -> DateTime {
    ferb_core::when::parse("2026-11-20T15:00:00+05:30").unwrap()
}

impl Mac {
    pub fn new() -> Arc<Mac> {
        let m = Arc::new(Mac::default());
        *m.reading.lock().unwrap() = PowerReading {
            battery: Some(80),
            on_battery: Some(false),
            lid_closed: Some(false),
            thermal: Some(ferb_core::power::Thermal::Nominal),
            screen_locked: Some(false),
        };
        *m.lock_answer.lock().unwrap() = Some(Ok(true));
        m
    }

    /// Both clocks move together (the Mac awake).
    pub fn advance(&self, s: f64) {
        *self.wall.lock().unwrap() += s;
        *self.mono.lock().unwrap() += s;
    }

    /// The Mac slept `s` seconds: the wall clock moved, the monotonic one didn't.
    pub fn sleep(&self, s: f64) {
        *self.wall.lock().unwrap() += s;
    }

    pub fn now(&self) -> DateTime {
        let w = *self.wall.lock().unwrap();
        t0() + chrono::Duration::milliseconds((w * 1000.0) as i64)
    }

    pub fn held_now(&self) -> Vec<Hold> {
        let mut h = self.held.lock().unwrap().clone();
        h.sort();
        h
    }

    pub fn set(&self, f: impl FnOnce(&mut PowerReading)) {
        f(&mut self.reading.lock().unwrap());
    }

    pub fn journal_kinds(&self) -> Vec<String> {
        self.journal
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| match v.get("state").and_then(Value::as_str) {
                Some(s) => format!("{k}:{s}"),
                None => k.clone(),
            })
            .collect()
    }

    pub fn topics(&self, topic: &str) -> Vec<Value> {
        self.published
            .lock()
            .unwrap()
            .iter()
            .filter(|(t, _)| t == topic)
            .map(|(_, v)| v.clone())
            .collect()
    }

    pub fn told_texts(&self) -> Vec<String> {
        self.told
            .lock()
            .unwrap()
            .iter()
            .map(|(_, t)| t.clone())
            .collect()
    }
}

/// FERB's root helper (APP-986): absent unless a test approves it; what it was asked.
#[derive(Default)]
pub struct Root {
    pub ready: Mutex<bool>,
    pub leased: Mutex<bool>,
    pub calls: Mutex<Vec<String>>,
    pub wakes: Mutex<Vec<i64>>,
}

impl RootHelper for Root {
    fn state(&self) -> HelperState {
        if *self.ready.lock().unwrap() {
            HelperState::Ready
        } else {
            HelperState::Absent
        }
    }
    fn lease(&self, seconds: u64) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("lease {seconds}"));
        if !*self.ready.lock().unwrap() {
            return Err("absent".into());
        }
        *self.leased.lock().unwrap() = true;
        Ok(())
    }
    fn release(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("release".into());
        *self.leased.lock().unwrap() = false;
        Ok(())
    }
    fn wakes(&self) -> Result<Vec<i64>, String> {
        Ok(self.wakes.lock().unwrap().clone())
    }
    fn wake(&self, at: i64) -> Result<(), String> {
        self.wakes.lock().unwrap().push(at);
        Ok(())
    }
    fn unwake(&self, at: i64) -> Result<(), String> {
        self.wakes.lock().unwrap().retain(|w| *w != at);
        Ok(())
    }
}

pub struct Keep(pub Arc<Mac>);
impl KeepAwake for Keep {
    fn hold(&self, hold: Hold, reason: &str, lease_s: u64) -> Result<(), String> {
        self.0
            .holds
            .lock()
            .unwrap()
            .push((hold, reason.into(), lease_s));
        let mut h = self.0.held.lock().unwrap();
        if !h.contains(&hold) {
            h.push(hold);
        }
        Ok(())
    }
    fn release(&self, hold: Hold) {
        self.0.releases.lock().unwrap().push(hold);
        self.0.held.lock().unwrap().retain(|h| *h != hold);
    }
}

pub struct Facts(pub Arc<Mac>);
impl PowerFacts for Facts {
    fn read(&self) -> PowerReading {
        self.0.reading.lock().unwrap().clone()
    }
}

pub struct Lock(pub Arc<Mac>);
impl LockMac for Lock {
    fn lock_now(&self) -> Result<bool, String> {
        *self.0.locks.lock().unwrap() += 1;
        let answer = self.0.lock_answer.lock().unwrap().clone().unwrap();
        if answer == Ok(true) {
            self.0.reading.lock().unwrap().screen_locked = Some(true);
        }
        answer
    }
}

pub struct Store(pub Arc<Mac>);
impl Persist for Store {
    fn load(&self) -> Option<Saved> {
        self.0.saved.lock().unwrap().clone()
    }
    fn note(&self, kind: &str, data: Value) {
        if kind == JOURNAL_KIND {
            let state = data.get("state").and_then(Value::as_str).unwrap_or("");
            let mut saved = self.0.saved.lock().unwrap();
            match state {
                "started" | "resumed" => *saved = Saved::from_json(&data),
                "ended" => *saved = None,
                _ => {}
            }
        }
        self.0.journal.lock().unwrap().push((kind.into(), data));
    }
}

pub fn seams(mac: &Arc<Mac>) -> Seams {
    let m = mac.clone();
    let working = Arc::new(move || *m.working.lock().unwrap());
    let m = mac.clone();
    let unlocked_after =
        Arc::new(move |since: &DateTime| m.unlocked_at.lock().unwrap().is_some_and(|t| t > *since));
    let m = mac.clone();
    let settings = Arc::new(move || *m.settings.lock().unwrap());
    let m = mac.clone();
    let publish =
        Arc::new(move |t: &str, v: Value| m.published.lock().unwrap().push((t.into(), v)));
    let m = mac.clone();
    let tell = Arc::new(move |i: &str, t: &str| {
        let mut told = m.told.lock().unwrap();
        if !told.iter().any(|(id, _)| id == i) {
            told.push((i.into(), t.into()));
        }
    });
    let m = mac.clone();
    let now = Arc::new(move || m.now());
    let m = mac.clone();
    let mono = Arc::new(move || *m.mono.lock().unwrap());
    let m = mac.clone();
    let new_id = Arc::new(move || {
        let mut n = m.ids.lock().unwrap();
        *n += 1;
        format!("id{n}")
    });
    Seams {
        keep: Arc::new(Keep(mac.clone())),
        root: mac.root.clone(),
        facts: Arc::new(Facts(mac.clone())),
        lock: Arc::new(Lock(mac.clone())),
        working,
        unlocked_after,
        settings,
        persist: Arc::new(Store(mac.clone())),
        publish,
        tell,
        now,
        mono,
        new_id,
    }
}

pub fn insomnia() -> (Arc<Mac>, Insomnia) {
    let mac = Mac::new();
    let ins = Insomnia::new(seams(&mac));
    (mac, ins)
}
