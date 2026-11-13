//! The trail (`ferb/sense/trail.py`) against `rust/fixtures/sense/trail.json`: the ordered record path with
//! exclusions before anything is kept, browser pages, attribution, test runs and pause; the rows, events and
//! publishes; queries, days and both purges.

mod common;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;

use chrono::NaiveDate;
use common::{fixture, input, input_value, tmp, Diffs};
use ferb_clock::clock::FakeClock;
use ferb_clock::zone::Zone;
use ferb_core::ids::SequentialIds;
use ferb_core::types::TrailEvent;
use ferb_core::when::isoformat;
use ferb_sense::attribution::Attribution;
use ferb_sense::exclusions::Exclusions;
use ferb_sense::observation::Observation;
use ferb_sense::trail::{browser_key, Query, Seams, Trail};
use ferb_store::Db;
use rusqlite::params;
use serde_json::{json, Map, Value};

fn db(name: &str) -> Db {
    let path = tmp().join("db").join(format!("{name}.db"));
    let _ = std::fs::remove_file(&path);
    let db = Db::open(path).unwrap();
    db.migrate().unwrap();
    db
}

fn event_json(e: &Option<TrailEvent>) -> Value {
    match e {
        None => Value::Null,
        Some(e) => json!({"id": e.id, "at": isoformat(&e.at), "device_id": e.device_id,
                          "actor": e.actor.as_str(), "task_id": e.task_id, "source": e.source, "data": e.data}),
    }
}

fn rows(db: &Db) -> Value {
    let mut st = db
        .conn()
        .prepare("SELECT seq, id, at, day, device_id, actor, task_id, source, data_json FROM trail ORDER BY seq")
        .unwrap();
    let r: Vec<Value> = st
        .query_map([], |r| {
            Ok(json!({"seq": r.get::<_, i64>(0)?, "id": r.get::<_, String>(1)?, "at": r.get::<_, String>(2)?,
                      "day": r.get::<_, String>(3)?, "device_id": r.get::<_, String>(4)?,
                      "actor": r.get::<_, String>(5)?, "task_id": r.get::<_, Option<String>>(6)?,
                      "source": r.get::<_, String>(7)?, "data_json": r.get::<_, String>(8)?}))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    Value::Array(r)
}

fn obj(v: &Value) -> Map<String, Value> {
    input_value(v).as_object().cloned().unwrap_or_default()
}

fn kolkata() -> Zone {
    Zone::named("Asia/Kolkata").unwrap()
}

fn seams<'a>(
    excl: &'a Exclusions,
    published: &'a RefCell<Vec<Value>>,
    paused: &'a Cell<bool>,
    device: &'a str,
) -> Seams<'a> {
    Seams {
        exclusions: Box::new(move || excl.clone()),
        publish: Box::new(move |topic, data| published.borrow_mut().push(json!([topic, data]))),
        device_id: Box::new(move || device.to_string()),
        paused: Box::new(move || paused.get()),
        test_task: Box::new(|t| t.is_some_and(|t| t.starts_with("test-"))),
        is_browser: Box::new(common::known_browser),
    }
}

// decision: SENSE-403, SENSE-5, GATE-550
#[test]
fn the_record_path_keeps_what_python_keeps() {
    let f = fixture("trail.json");
    let mut d = Diffs::default();
    std::fs::create_dir_all(tmp().join("work/real")).unwrap();
    let _ = std::os::unix::fs::symlink(tmp().join("work/real"), tmp().join("work/alias"));
    let store = db("trail");
    let start = ferb_core::when::parse(f["start"].as_str().unwrap()).unwrap();
    let clock = FakeClock::new(Some(start)).with_zone(kolkata());
    let ids = SequentialIds::new("tr");
    let redactor = ferb_sanitize::Redactor::default();
    let ex = &f["exclusions"];
    let excl = Exclusions::build(
        &[ex["apps"][0].as_str().unwrap()],
        &[ex["domains"][0].as_str().unwrap()],
        &[input(ex["paths"][0].as_str().unwrap())],
    );
    let published = RefCell::new(Vec::new());
    let paused = Cell::new(false);
    let mono = Arc::new(std::sync::Mutex::new(f["mono_start"].as_f64().unwrap()));
    let m = mono.clone();
    let attribution = Arc::new(Attribution::new(move || *m.lock().unwrap()));
    let cwds: Arc<std::sync::Mutex<Vec<(String, String)>>> = Arc::default();
    let c = cwds.clone();
    *attribution.shell_cwds.lock().unwrap() = Box::new(move || c.lock().unwrap().clone());
    let trail = Trail::new(
        &store,
        &clock,
        &ids,
        &redactor,
        seams(&excl, &published, &paused, "dev-1"),
        attribution.clone(),
    );
    let tagged: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let t = tagged.clone();
    *trail.tag.borrow_mut() = Box::new(move |e| t.lock().unwrap().push(e.id.clone()));
    let mut acting = HashMap::new();
    for (n, entry) in f["steps"].as_array().unwrap().iter().enumerate() {
        let s = &entry["step"];
        let what = format!("step {n} {s}");
        match s["op"].as_str().unwrap() {
            "record" => {
                let before = published.borrow().len();
                let o = Observation::with_hints(
                    s["source"].as_str().unwrap(),
                    obj(&s["data"]),
                    obj(&s["hints"]),
                );
                let e = trail.record(&o).unwrap();
                d.check(&format!("{what} event"), &entry["event"], &event_json(&e));
                d.check(
                    &format!("{what} published"),
                    &entry["published"],
                    &Value::Array(published.borrow()[before..].to_vec()),
                );
                d.check(
                    &format!("{what} tagged"),
                    &entry["tagged"],
                    &json!(*tagged.lock().unwrap()),
                );
                d.check(
                    &format!("{what} last event"),
                    &entry["last_event_at"],
                    &json!(trail.last_event_at.get().map(|t| isoformat(&t))),
                );
            }
            "advance" => {
                clock.advance(s["seconds"].as_f64().unwrap());
                *mono.lock().unwrap() += s["mono"].as_f64().unwrap();
            }
            "ferb_activated" => {
                attribution.ferb_activated(s["bundle"].as_str().unwrap(), s["task"].as_str())
            }
            "ferb_touched" => {
                attribution.ferb_touched(&input(s["path"].as_str().unwrap()), s["task"].as_str());
                let got: Vec<Value> = attribution
                    .touched()
                    .into_iter()
                    .map(|(k, t, u)| json!([k, t, u]))
                    .collect();
                d.check(
                    &format!("{what} paths"),
                    &entry["paths"],
                    &Value::Array(got),
                );
            }
            "acting_enter" => {
                let key = (
                    s["bundle"].as_str().unwrap().to_string(),
                    s["task"].as_str().map(String::from),
                );
                acting.insert(
                    key,
                    attribution.acting(s["bundle"].as_str().unwrap(), s["task"].as_str()),
                );
            }
            "acting_exit" => {
                let key = (
                    s["bundle"].as_str().unwrap().to_string(),
                    s["task"].as_str().map(String::from),
                );
                drop(acting.remove(&key));
                let now: Map<String, Value> = attribution
                    .acting_now()
                    .into_iter()
                    .map(|(k, v)| (k, json!(v)))
                    .collect();
                d.check(
                    &format!("{what} acting"),
                    &entry["acting"],
                    &Value::Object(now),
                );
            }
            "shell_cwds" => {
                *cwds.lock().unwrap() = s["cwds"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| {
                        (
                            input(c[0].as_str().unwrap()),
                            c[1].as_str().unwrap().to_string(),
                        )
                    })
                    .collect();
            }
            "pause" => paused.set(s["on"].as_bool().unwrap()),
            other => panic!("unknown step {other}"),
        }
    }
    d.check("rows", &f["rows"], &rows(&store));
    let pages: Map<String, Value> = trail
        .pages
        .borrow()
        .iter()
        .map(|(k, (h, i))| (k.clone(), json!({"host": h, "incognito": i})))
        .collect();
    // Python's dict keeps insertion order; compare as a set of entries
    let mut want: Vec<(String, Value)> = f["pages"]
        .as_object()
        .unwrap()
        .clone()
        .into_iter()
        .collect();
    want.sort_by(|a, b| a.0.cmp(&b.0));
    d.check(
        "pages",
        &json!(want),
        &json!(pages.into_iter().collect::<Vec<_>>()),
    );
    for b in f["browser_key"].as_array().unwrap() {
        d.check(
            &format!("browser_key {b}"),
            &b[1],
            &json!(browser_key(b[0].as_str().unwrap())),
        );
    }
    d.finish("trail record");
}

fn seed(store: &Db, f: &Value) {
    for r in f["query_rows"].as_array().unwrap() {
        store
            .conn()
            .execute(
                "INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES (?,?,?,?,?,?,?,?)",
                params![
                    r["id"].as_str(), r["at"].as_str(), r["day"].as_str(), r["device_id"].as_str(),
                    r["actor"].as_str(), r["task_id"].as_str(), r["source"].as_str(), r["data_json"].as_str()
                ],
            )
            .unwrap();
    }
}

// decision: SENSE-408
#[test]
fn queries_days_and_purges_read_and_delete_what_python_does() {
    let f = fixture("trail.json");
    let mut d = Diffs::default();
    let store = db("query");
    seed(&store, &f);
    d.check("seeded", &f["query_rows"], &rows(&store));
    let start = ferb_core::when::parse(f["start"].as_str().unwrap()).unwrap();
    let clock = FakeClock::new(Some(start)).with_zone(kolkata());
    let ids = SequentialIds::new("q");
    let redact = ferb_sense::observation::NoRedact;
    let excl = Exclusions::default();
    let published = RefCell::new(Vec::new());
    let paused = Cell::new(false);
    let trail = Trail::new(
        &store,
        &clock,
        &ids,
        &redact,
        seams(&excl, &published, &paused, "dev-q"),
        Arc::new(Attribution::new(|| 0.0)),
    );
    for q in f["queries"].as_array().unwrap() {
        let a = &q["args"];
        let st = |k: &str| a.get(k).and_then(Value::as_str);
        let query = Query {
            since: st("since").unwrap_or(""),
            until: st("until"),
            source: st("source"),
            actor: st("actor"),
            host: st("host"),
            limit: a.get("limit").and_then(Value::as_i64).unwrap_or(100),
            before_seq: a.get("before_seq").and_then(Value::as_i64),
            text: st("text"),
            event_id: st("event_id"),
        };
        let got = match trail.query(&query) {
            Ok(v) => json!({"args": a, "out": v}),
            Err(e) => json!({"args": a, "error": e.to_string()}),
        };
        d.check(&format!("query {a}"), q, &got);
    }
    for e in f["events"].as_array().unwrap() {
        let a = &e["args"];
        let got: Vec<Value> = trail
            .events(
                a.get("source").and_then(Value::as_str),
                a.get("actor").and_then(Value::as_str),
                a.get("limit").and_then(Value::as_i64).unwrap_or(1000),
            )
            .unwrap()
            .into_iter()
            .map(|e| event_json(&Some(e)))
            .collect();
        d.check(&format!("events {a}"), &e["out"], &Value::Array(got));
    }
    for (n, j) in f["journal"].as_array().unwrap().iter().enumerate() {
        store
            .conn()
            .execute(
                "INSERT INTO journal (id, at, kind, data_json) VALUES (?,?,?,?)",
                params![
                    j[0].as_str(),
                    format!("2026-03-0{}T23:00:00+00:00", n % 4 + 1),
                    j[1].as_str(),
                    ferb_py::json::dumps(&j[2])
                ],
            )
            .unwrap();
    }
    for c in f["days"].as_array().unwrap() {
        let a = &c["args"];
        let got = trail
            .days(
                a.get("text").and_then(Value::as_str),
                a.get("since_days").and_then(Value::as_i64).unwrap_or(30),
                a.get("limit").and_then(Value::as_i64).unwrap_or(10),
            )
            .unwrap();
        d.check(&format!("days {a}"), &c["out"], &Value::Array(got));
    }
    d.check(
        "summarized",
        &f["summarized_days"],
        &json!(trail.summarized_days().unwrap()),
    );
    for p in f["purge"].as_array().unwrap() {
        let today = NaiveDate::parse_from_str(p["today"].as_str().unwrap(), "%Y-%m-%d").unwrap();
        let (purged, refused) = trail
            .purge(p["retention_days"].as_i64().unwrap(), Some(today))
            .unwrap();
        let left: Vec<Value> = rows(&store)
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].clone())
            .collect();
        d.check(
            &format!("purge {p}"),
            &json!([p["purged"], p["refused"], p["left"]]),
            &json!([purged, refused, left]),
        );
    }
    // accessibility rows go after 7 days, summarized or not, two rows a step, with their receipts (SENSE-4)
    let pdb = db("purge");
    let pt = Trail::new(
        &pdb,
        &clock,
        &ids,
        &redact,
        seams(&excl, &published, &paused, "dev"),
        Arc::new(Attribution::new(|| 0.0)),
    );
    pdb.conn()
        .execute("INSERT INTO perception_batches VALUES('b1','2026-01-01T00:00:00+00:00','age','done','[]',NULL)", [])
        .unwrap();
    let ps = &f["purge_source"];
    for (n, r) in ps["rows"].as_array().unwrap().iter().enumerate() {
        let day = r[2].as_str().unwrap();
        pdb.conn()
            .execute(
                "INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES (?,?,?,?,?,?,?,?)",
                params![r[0].as_str(), format!("{day}T10:00:00+00:00"), day, "dev", "unknown", None::<String>,
                        r[1].as_str(), "{}"],
            )
            .unwrap();
        if n % 2 == 0 {
            pdb.conn()
                .execute(
                    "INSERT INTO perception_receipts VALUES(?, 'b1')",
                    params![r[0].as_str()],
                )
                .unwrap();
        }
    }
    let today = NaiveDate::from_ymd_opt(2026, 10, 31);
    let steps: Vec<usize> = pt
        .purge_source_steps("accessibility", 7, today, 2)
        .map(|s| s.unwrap())
        .collect();
    d.check("purge_source steps", &ps["steps"], &json!(steps));
    let left: Vec<Value> = rows(&pdb)
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].clone())
        .collect();
    d.check("purge_source left", &ps["left"], &Value::Array(left));
    let mut st = pdb
        .conn()
        .prepare("SELECT event_id FROM perception_receipts ORDER BY rowid")
        .unwrap();
    let receipts: Vec<String> = st
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    d.check("purge_source receipts", &ps["receipts"], &json!(receipts));
    d.finish("trail reads");
}
