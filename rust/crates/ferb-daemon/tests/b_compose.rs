//! `ferb/daemon/areas/compose.py` against `rust/fixtures/daemon-b/compose.json` (made by `compose.py` beside it): a
//! question's kind and first line, the journal rows of questions asked and answered, `_publish`, the consumers'
//! order over the real events, the activity device id and the privacy lists in the redactor, byte for byte.

mod b_support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use b_support::{fixture, Home};
use ferb_daemon::areas_b::compose::{
    self, ActivityDevice, Approvals, Asked, Consumers, QuestionRecords,
};
use ferb_daemon::areas_b::events::{Events, Redact};
use serde_json::{json, Map, Value};

fn dumps(v: &Value) -> String {
    ferb_py::json::dumps(v)
}

fn s(v: &Value) -> String {
    v.as_str().map(str::to_owned).unwrap_or_default()
}

#[derive(Default)]
struct Store(Mutex<HashMap<String, Asked>>);

impl Approvals for Store {
    fn get(&self, id: &str) -> Option<Asked> {
        self.0.lock().unwrap().get(id).cloned()
    }
}

fn asked(a: &Value) -> Asked {
    let opt = |k: &str| a.get(k).and_then(Value::as_str).map(str::to_owned);
    Asked {
        id: s(&a["id"]),
        capability: s(&a["capability"]),
        kind: opt("kind").unwrap_or_else(|| "owner_question".into()),
        reason: s(&a["reason"]),
        summary: opt("summary").unwrap_or_default(),
        exact: opt("exact").unwrap_or_default(),
        task_id: opt("task_id").unwrap_or_default(),
        request_id: s(&a["request_id"]),
        state: opt("state").unwrap_or_else(|| "pending".into()),
        choice: opt("choice"),
        answer: opt("answer"),
        hidden: vec![],
    }
}

fn store(list: &Value) -> Arc<Store> {
    let st = Store::default();
    for a in list.as_array().unwrap() {
        st.0.lock().unwrap().insert(s(&a["id"]), asked(a));
    }
    Arc::new(st)
}

fn obj(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

fn events_for(home: &Home) -> Events {
    let r = home.host.redactor.clone();
    Events::new(move || {
        let r = r.clone();
        let f: Redact = Arc::new(move |v: &Value| r.value(v));
        f
    })
}

fn journal(home: &Home) -> Value {
    Value::Array(
        home.journal_rows()
            .into_iter()
            .map(|r| json!([r[0], r[1]]))
            .collect(),
    )
}

struct Diffs(Vec<String>, usize);

impl Diffs {
    fn eq(&mut self, what: &str, python: String, rust: String) {
        if python == rust {
            self.1 += 1;
        } else {
            self.0
                .push(format!("{what}:\n  python: {python}\n  rust:   {rust}"));
        }
    }
}

struct Log {
    log: Mutex<Vec<Value>>,
    supervisor: bool,
    shutting: bool,
    device: String,
}

impl Log {
    fn push(&self, v: Value) {
        self.log.lock().unwrap().push(v);
    }
}

impl Consumers for Log {
    fn has_supervisor(&self) -> bool {
        self.supervisor
    }
    fn plan_event(&self, topic: &str, data: Map<String, Value>) {
        self.push(json!(["plans", topic, data]));
    }
    fn record_activity(&self, row: Map<String, Value>) {
        self.push(json!(["activity", row]));
    }
    fn observe_conversation(&self, data: Map<String, Value>) {
        self.push(json!(["conversations", data]));
    }
    fn shutting_down(&self) -> bool {
        self.shutting
    }
    fn attention_arrived(&self, data: Map<String, Value>) {
        self.push(json!(["attention", data]));
    }
    fn bus(&self, topic: &str, data: Map<String, Value>) {
        self.push(json!(["bus", topic, data]));
    }
    fn approval_notice(&self, data: Map<String, Value>) {
        self.push(json!(["approval_notice", data]));
    }
    fn spawn_tell_refused(&self, data: Map<String, Value>) {
        self.push(json!(["tell_refused", data]));
    }
    fn spawn_task_paused(&self, data: Map<String, Value>) {
        self.push(json!(["task_paused", data]));
    }
    fn spawn_task_resumed(&self, data: Map<String, Value>) {
        self.push(json!(["task_resumed", data]));
    }
    fn device_id(&self) -> String {
        self.device.clone()
    }
}

#[test]
fn compose_matches_python() {
    let fx = fixture("daemon-b/compose.json");
    let mut d = Diffs(vec![], 0);

    for k in fx["kinds"].as_array().unwrap() {
        d.eq(
            &format!("question_kind {k}"),
            s(&k[2]),
            compose::question_kind(&s(&k[0]), &s(&k[1])),
        );
    }
    for f in fx["first_lines"].as_array().unwrap() {
        d.eq(
            &format!("first_line {}", f[0]),
            s(&f[1]),
            compose::first_line(f[0].as_str()),
        );
    }

    for sc in fx["questions"].as_array().unwrap() {
        let home = Home::new("");
        let st = store(&sc["approvals"]);
        let q = QuestionRecords::new(st.clone());
        for step in sc["steps"].as_array().unwrap() {
            if step[0] == "set" {
                let mut m = st.0.lock().unwrap();
                let ap = m.get_mut(&s(&step[1])).unwrap();
                for (k, v) in step[2].as_object().unwrap() {
                    match k.as_str() {
                        "state" => ap.state = s(v),
                        "answer" => ap.answer = v.as_str().map(str::to_owned),
                        "choice" => ap.choice = v.as_str().map(str::to_owned),
                        "hidden" => ap.hidden = v.as_array().unwrap().iter().map(s).collect(),
                        other => panic!("{other}"),
                    }
                }
            } else {
                q.record(&home.host, step[0].as_str().unwrap(), &obj(&step[1]))
                    .unwrap();
            }
        }
        let name = s(&sc["name"]);
        d.eq(
            &format!("questions {name}"),
            dumps(&sc["journal"]),
            dumps(&journal(&home)),
        );
        let kinds: std::collections::BTreeMap<String, String> =
            q.asked_kinds().into_iter().collect();
        d.eq(
            &format!("asked_kinds {name}"),
            dumps(&sc["asked_kinds"]),
            dumps(&json!(kinds)),
        );
    }

    for sc in fx["publish"].as_array().unwrap() {
        let home = Home::new("");
        let st = store(&sc["approvals"]);
        let ev = Events::new(|| {
            let f: Redact = Arc::new(|v: &Value| v.clone());
            f
        });
        let got = Arc::new(Mutex::new(vec![]));
        let g = got.clone();
        ev.on(
            None,
            Arc::new(move |t, data| g.lock().unwrap().push(json!(["events", t, data]))),
        );
        compose::publish(&ev, st.as_ref(), &s(&sc["event"][0]), obj(&sc["event"][1]));
        drop(home);
        d.eq(
            &format!("publish {}", sc["name"]),
            dumps(&sc["got"]),
            dumps(&Value::Array(got.lock().unwrap().clone())),
        );
    }

    for sc in fx["wiring"].as_array().unwrap() {
        let home = Home::new("");
        let st = store(&sc["approvals"]);
        let ev = events_for(&home);
        let log = Arc::new(Log {
            log: Mutex::new(vec![]),
            supervisor: sc["supervisor"].as_bool().unwrap(),
            shutting: sc["shutting_down"].as_bool().unwrap(),
            device: "mac-1".into(),
        });
        compose::wire_events(
            &ev,
            home.host.clone(),
            log.clone(),
            Arc::new(QuestionRecords::new(st)),
            Arc::new(ActivityDevice::default()),
        );
        for e in sc["events"].as_array().unwrap() {
            ev.publish(&s(&e[0]), &obj(&e[1]));
        }
        let name = s(&sc["name"]);
        d.eq(
            &format!("wiring {name}"),
            dumps(&sc["log"]),
            dumps(&Value::Array(log.log.lock().unwrap().clone())),
        );
        d.eq(
            &format!("wiring journal {name}"),
            dumps(&sc["journal"]),
            dumps(&journal(&home)),
        );
    }

    let home = Home::new("");
    let dev = ActivityDevice::default();
    for pair in fx["activity_device"].as_array().unwrap() {
        d.eq(
            &format!("activity_device {}", pair[0]),
            dumps(&pair[1]),
            dumps(&dev.get(&home.host, &s(&pair[0]))),
        );
    }

    for sc in fx["privacy"].as_array().unwrap() {
        let home = Home::new(&s(&sc["config"]));
        let device = compose::load_config(&home.host, false).unwrap();
        assert_eq!(device, "unknown-device");
        for p in sc["probes"].as_array().unwrap() {
            let probe = s(&p[0]);
            let host = probe.split('/').next().unwrap();
            let got = json!([
                probe,
                home.host.redact_text(&probe),
                home.host.redactor.excluded_host(host)
            ]);
            d.eq(&format!("privacy {}", sc["name"]), dumps(p), dumps(&got));
        }
    }

    eprintln!("compose: {} comparisons equal", d.1);
    assert!(
        d.0.is_empty(),
        "{} difference(s):\n{}",
        d.0.len(),
        d.0.join("\n")
    );
}

#[test]
fn apply_privacy_overridden_keeps_the_redactor_as_is() {
    let home = Home::new("[privacy]\nexclude_domains = [\"excluded.example\"]\n");
    compose::apply_privacy(&home.host, true).unwrap();
    assert!(!home.host.redactor.excluded_host("excluded.example"));
    compose::apply_privacy(&home.host, false).unwrap();
    assert!(home.host.redactor.excluded_host("excluded.example"));
}
