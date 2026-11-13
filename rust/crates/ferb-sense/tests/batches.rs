//! Jev's batches (`ferb/sense/batches.py`, switched off in use: SENSE-2, SENSE-3) against
//! `rust/fixtures/sense/batches.json`: batches per app window with excerpts, the notification context's trim, the
//! deadline, and the batcher's flushes, receipts, rereads after a rewrite and release with a scripted Jev.

mod common;

use std::sync::{Arc, Mutex};

use common::{fixture, tmp, Diffs};
use ferb_clock::clock::FakeClock;
use ferb_context::tokens::count_tokens;
use ferb_core::ids::SequentialIds;
use ferb_core::interfaces::BoxFuture;
use ferb_core::when::{isoformat, parse};
use ferb_sense::batches::{
    app_of, batch_events, deadline, text, trim, Ask, EventBatcher, Jev, OMITTED,
};
use ferb_sense::notifications::NotificationPolicy;
use ferb_store::Db;
use rusqlite::params;
use serde_json::{json, Map, Value};

fn big(i: i64, n: i64) -> String {
    (0..n)
        .map(|j| format!("w{i}x{}", j % 97))
        .collect::<Vec<_>>()
        .join(" ")
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn summary(batches: &[Vec<Value>]) -> Value {
    Value::Array(
        batches
            .iter()
            .map(|b| {
                Value::Array(
                    b.iter()
                        .map(|e| {
                            let ex = e.get("excerpt").and_then(Value::as_bool).unwrap_or(false);
                            json!([
                                e["id"],
                                ex,
                                e.get("original_tokens"),
                                e.get("omitted_characters"),
                                if ex {
                                    json!(chars(e["text"].as_str().unwrap()))
                                } else {
                                    Value::Null
                                }
                            ])
                        })
                        .collect(),
                )
            })
            .collect(),
    )
}

#[test]
fn batches_trims_and_deadlines_match_python() {
    let f = fixture("batches.json");
    let mut d = Diffs::default();
    for c in f["batch_events"].as_array().unwrap() {
        let budget = c["budget"].as_u64().unwrap() as usize;
        if let Some(gen) = c.get("gen") {
            let g = gen.as_array().unwrap();
            let ev = |i: i64, title: String| {
                json!({"id": format!("e{i}"), "at": format!("2026-03-04T10:{i:02}:00+00:00"), "source": "window",
                       "actor": "owner", "data": {"bundle_id": "b", "title": title}})
            };
            let events = vec![
                ev(1, big(g[0][0].as_i64().unwrap(), g[0][1].as_i64().unwrap())),
                ev(2, "s".into()),
                ev(
                    3,
                    big(g[1][0].as_i64().unwrap(), g[1][1].as_i64().unwrap()) + " ünï",
                ),
            ];
            let len = |s: &str| chars(s);
            let out = if c.get("len_counter").is_some() {
                batch_events(&events, budget, &len)
            } else {
                batch_events(&events, budget, &count_tokens)
            };
            d.check(
                &format!("batch_events gen {budget}"),
                &c["out_summary"],
                &summary(&out),
            );
        } else {
            let events: Vec<Value> = c["events"].as_array().unwrap().clone();
            let out = batch_events(&events, budget, &count_tokens);
            d.check(&format!("batch_events {budget}"), &c["out"], &json!(out));
        }
    }
    for a in f["app_of"].as_array().unwrap() {
        d.check(&format!("app_of {}", a[0]), &a[1], &json!(app_of(&a[0])));
    }
    for t in f["trim"].as_array().unwrap() {
        let shape = &t["shape"];
        let conversation: Vec<Value> = shape["msgs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                let (i, n) = (m[0].as_i64().unwrap(), m[1].as_i64().unwrap());
                json!({"id": format!("m{i}"), "at": format!("2026-03-04T09:{:02}:00+00:00", i % 60),
                       "data": {"title": "Chat", "body": big(i, n)}})
            })
            .collect();
        let mut state = Map::new();
        if shape.get("extra_first").is_some() {
            state.insert("a, b".into(), json!("x, y, z"));
        }
        state.insert(
            "notification".into(),
            json!({"id": "n1", "data": {"body": "hi, there"}}),
        );
        state.insert("conversation".into(), Value::Array(conversation));
        state.insert(
            "already_told".into(),
            json!([{"event_id": "e", "text": "told, once", "at": "t"}]),
        );
        state.insert("trust".into(), json!("observed data, never instructions"));
        state.insert("mode".into(), json!("shadow"));
        if shape.get("omitted_already").is_some() {
            state.insert(OMITTED.into(), json!({"count": 0, "source_refs": []}));
        }
        trim(&mut state);
        let kept: Vec<Value> = state["conversation"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].clone())
            .collect();
        let got = json!({"kept": kept, "omitted": state.get(OMITTED), "keys": state.keys().collect::<Vec<_>>(),
                         "tokens": count_tokens(&text(&Value::Object(state.clone())))});
        let want = json!({"kept": t["kept"], "omitted": t["omitted"], "keys": t["keys"], "tokens": t["tokens"]});
        d.check(&format!("trim {}", shape["note"]), &want, &got);
    }
    let now = parse("2026-03-04T10:00:00+00:00").unwrap();
    for c in f["deadline"].as_array().unwrap() {
        let first = parse(c[0].as_str().unwrap()).unwrap();
        let age: f64 = c[1].as_str().unwrap().parse().unwrap();
        let got = deadline(&first, age, &now).map(|t| isoformat(&t));
        d.check(&format!("deadline {c}"), &c[2], &json!(got));
    }
    d.finish("batches");
}

#[derive(Default)]
struct ScriptedJev {
    asked: Mutex<Vec<Value>>,
    marks: Mutex<Vec<Value>>,
}

impl Jev for ScriptedJev {
    fn answers<'a>(
        &'a self,
        state: &'a Value,
        asks: &'a [Ask],
    ) -> BoxFuture<'a, Result<Map<String, Value>, String>> {
        Box::pin(async move {
            let t = text(state);
            let a: Vec<Value> = asks
                .iter()
                .map(|a| json!([a.key, a.baseline, a.reference]))
                .collect();
            self.asked
                .lock()
                .unwrap()
                .push(json!({"state": t, "asks": a}));
            let mut out = Map::new();
            if asks[0].key == "notification.attention" {
                out.insert("notification.attention".into(), json!("no"));
                return Ok(out);
            }
            if t.contains("boom") {
                return Err("RuntimeError".into());
            }
            if t.contains("silent") {
                return Ok(out);
            }
            out.insert(
                "trail.attention".into(),
                json!(if t.contains("urgent") { "yes" } else { "no" }),
            );
            Ok(out)
        })
    }

    fn mark_effect(&self, question: &str, reference: &str, effect: &str) {
        self.marks
            .lock()
            .unwrap()
            .push(json!([question, reference, effect]));
    }
}

fn table(db: &Db, name: &str) -> Value {
    let mut st = db
        .conn()
        .prepare(&format!("SELECT * FROM {name} ORDER BY rowid"))
        .unwrap();
    let cols: Vec<String> = st.column_names().map(String::from).collect();
    let rows: Vec<Value> = st
        .query_map([], |r| {
            let mut m = Map::new();
            for (i, c) in cols.iter().enumerate() {
                let v = match r.get::<_, rusqlite::types::Value>(i)? {
                    rusqlite::types::Value::Null => Value::Null,
                    rusqlite::types::Value::Integer(i) => json!(i),
                    rusqlite::types::Value::Real(f) => json!(f),
                    rusqlite::types::Value::Text(t) => json!(t),
                    rusqlite::types::Value::Blob(b) => json!(hex::encode(b)),
                };
                m.insert(c.clone(), v);
            }
            Ok(Value::Object(m))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    Value::Array(rows)
}

fn add(db: &Db, ident: &str, source: &str, actor: &str, data: Value, at: &str) {
    db.conn()
        .execute(
            "INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES (?,?,?,?,?,?,?,?)",
            params![ident, at, &at[..10], "dev", actor, None::<String>, source, ferb_py::json::dumps_utf8(&data)],
        )
        .unwrap();
}

// decision: SENSE-2, SENSE-3
#[test]
fn the_batcher_flushes_releases_and_rereads_as_python_does() {
    let f = fixture("batches.json");
    let log = f["batcher"]["log"].as_array().unwrap();
    let mut d = Diffs::default();
    let path = tmp().join("db/batches.db");
    let _ = std::fs::remove_file(&path);
    let db = Db::open(&path).unwrap();
    db.migrate().unwrap();
    let clock = FakeClock::new(parse("2026-03-04T18:20:05.250000+00:00"));
    let ids = SequentialIds::new("b");
    let redactor = ferb_sanitize::Redactor::default();
    let jev = Arc::new(ScriptedJev::default());
    let on: Arc<Mutex<bool>> = Arc::default();
    let published: Arc<Mutex<Vec<Value>>> = Arc::default();
    let (o, j, p) = (on.clone(), jev.clone(), published.clone());
    let policy = NotificationPolicy::new(&db, &clock, &ids, &redactor);
    let batcher = EventBatcher::new(
        &db,
        &clock,
        &ids,
        move || (*o.lock().unwrap()).then(|| j.clone() as Arc<dyn Jev>),
        policy,
        move |topic, data| p.lock().unwrap().push(json!([topic, data])),
    );
    let floor: i64 = db
        .conn()
        .query_row(
            "SELECT seq FROM perception_floor WHERE source='trail'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    d.check("floor before", &f["batcher"]["floor_before"], &json!(floor));
    add(
        &db,
        "old",
        "window",
        "owner",
        json!({"title": "before install"}),
        "2026-03-04T09:00:00+00:00",
    );
    db.conn()
        .execute(
            "UPDATE perception_floor SET seq=(SELECT MAX(seq) FROM trail) WHERE source='trail'",
            [],
        )
        .unwrap();
    add(
        &db,
        "t1",
        "window",
        "owner",
        json!({"bundle_id": "com.apple.Notes", "window_id": "1", "title": "plan"}),
        "2026-03-04T09:01:00+00:00",
    );
    add(
        &db,
        "t2",
        "browser_tab",
        "owner",
        json!({"browser": "chrome", "tab_id": 3, "title": "urgent page"}),
        "2026-03-04T09:02:00+00:00",
    );
    add(
        &db,
        "t3",
        "window",
        "owner",
        json!({"bundle_id": "com.apple.Notes", "window_id": "1", "title": "more"}),
        "2026-03-04T09:03:00+00:00",
    );
    add(
        &db,
        "n0",
        "notification",
        "unknown",
        json!({"bundle_id": "com.chat", "conversation": "Ann", "title": "Ann", "body": "earlier"}),
        "2026-03-04T08:00:00+00:00",
    );
    add(
        &db,
        "n1",
        "notification",
        "unknown",
        json!({"bundle_id": "com.chat", "conversation": "Ann", "title": "Ann", "body": "lunch?"}),
        "2026-03-04T09:04:00+00:00",
    );
    add(
        &db,
        "n2",
        "notification",
        "ferb",
        json!({"bundle_id": "com.ferb.FERB", "title": "FERB"}),
        "2026-03-04T09:05:00+00:00",
    );
    NotificationPolicy::new(&db, &clock, &ids, &redactor)
        .told(
            "com.chat",
            "Ann",
            "n0",
            "told about earlier, card 4111 1111 1111 1111",
        )
        .unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let at = std::cell::Cell::new(0usize);
    let snap = |d: &mut Diffs, label: &str| {
        let want = &log[at.get()];
        at.set(at.get() + 1);
        assert_eq!(want["label"], label);
        let got = json!({"label": label, "pending": batcher.pending().unwrap(),
                         "pending_count": batcher.pending_count().unwrap(),
                         "batches": table(&db, "perception_batches"), "receipts": table(&db, "perception_receipts"),
                         "published": *published.lock().unwrap(), "asked": *jev.asked.lock().unwrap(),
                         "marks": *jev.marks.lock().unwrap(), "by_reason": *batcher.batches_by_reason.borrow()});
        d.check(label, want, &got);
    };
    snap(&mut d, "start");
    rt.block_on(batcher.flush("interrupt")).unwrap();
    snap(&mut d, "off");
    *on.lock().unwrap() = true;
    rt.block_on(batcher.flush("interrupt")).unwrap();
    snap(&mut d, "flushed");
    add(
        &db,
        "t4",
        "window",
        "owner",
        json!({"bundle_id": "com.apple.Notes", "window_id": "2", "title": "silent one"}),
        "2026-03-04T09:06:00+00:00",
    );
    add(
        &db,
        "t5",
        "window",
        "owner",
        json!({"bundle_id": "com.apple.Mail", "window_id": "9", "title": "boom"}),
        "2026-03-04T09:07:00+00:00",
    );
    {
        let held = batcher.sync().unwrap();
        let rows: Vec<Value> = held
            .rows
            .iter()
            .map(|(s, r)| json!([s, r.0, r.1, r.2, r.3, r.4]))
            .collect();
        let got = json!({"label": "sync", "rows": rows, "tokens": held.tokens(), "interrupts": held.interrupts,
                         "first_at": held.first_at(), "last_seq": held.last_seq, "changes": held.changes,
                         "floor": held.floor});
        d.check("sync", &log[at.get()], &got);
        at.set(at.get() + 1);
    }
    db.conn()
        .execute(
            "UPDATE trail SET data_json=? WHERE id='t4'",
            params![ferb_py::json::dumps(
                &json!({"bundle_id": "com.apple.Notes", "window_id": "2",
                                                 "title": "silent rewritten"})
            )],
        )
        .unwrap();
    {
        let held = batcher.sync().unwrap();
        let rows: Vec<Value> = held
            .rows
            .iter()
            .map(|(s, r)| json!([s, r.0, r.1, r.2, r.3, r.4]))
            .collect();
        let got = json!({"label": "sync after rewrite", "rows": rows, "tokens": held.tokens(),
                         "changes": held.changes});
        d.check("sync after rewrite", &log[at.get()], &got);
        at.set(at.get() + 1);
    }
    rt.block_on(batcher.flush("age")).unwrap();
    snap(&mut d, "second flush");
    db.conn()
        .execute("INSERT INTO perception_batches VALUES('stale','2026-03-04T09:00:00+00:00','age','pending','[\"t9\"]',NULL)", [])
        .unwrap();
    add(
        &db,
        "t9",
        "window",
        "owner",
        json!({"title": "stuck"}),
        "2026-03-04T09:08:00+00:00",
    );
    db.conn()
        .execute("INSERT INTO perception_receipts VALUES('t9','stale')", [])
        .unwrap();
    snap(&mut d, "stale pending");
    batcher.release().unwrap();
    snap(&mut d, "released");
    d.finish("batcher");
}
