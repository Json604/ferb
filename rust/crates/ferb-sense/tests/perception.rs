//! Perception's notification path, the install floor, FERB's own banners, the installed apps, the 7-day text
//! expiry and status (`ferb/sense/perception.py`, `notifications.py`) against `rust/fixtures/sense/perception.json`.

mod common;

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::sync::Arc;

use common::{fixture, tmp, Diffs};
use ferb_clock::clock::FakeClock;
use ferb_clock::zone::Zone;
use ferb_core::ids::SequentialIds;
use ferb_core::when::parse;
use ferb_sense::attribution::Attribution;
use ferb_sense::batches::EventBatcher;
use ferb_sense::exclusions::Exclusions;
use ferb_sense::notifications::NotificationPolicy;
use ferb_sense::observation::Observation;
use ferb_sense::perception::{Perception, Seams};
use ferb_sense::trail::{self, Trail};
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

fn table(db: &Db, sql: &str) -> Value {
    let mut st = db.conn().prepare(sql).unwrap();
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

fn trail_rows(db: &Db) -> Value {
    table(db, "SELECT seq, id, at, day, device_id, actor, task_id, source, data_json FROM trail ORDER BY seq")
}

fn note(data: &Value) -> Observation {
    Observation::by("notification", data.as_object().unwrap().clone(), "unknown")
}

fn n(ext: &str, bundle: &str, title: &str, body: &str) -> Observation {
    note(
        &json!({"external_id": ext, "bundle_id": bundle, "title": title, "body": body,
                 "conversation": title, "conversation_source": "title"}),
    )
}

fn banners(p: &Perception<'_, '_>) -> Value {
    Value::Array(
        p.banners
            .borrow()
            .iter()
            .map(|b| json!({"title": b.title, "body": b.body, "poster": b.poster, "at": b.at}))
            .collect(),
    )
}

// decision: SENSE-2, SENSE-401, SENSE-403
#[test]
fn notifications_banners_floor_and_expiry_match_python() {
    let f = fixture("perception.json");
    let mut d = Diffs::default();
    let store = db("perception");
    let clock = FakeClock::new(parse("2026-03-04T18:20:05.250000+00:00"))
        .with_zone(Zone::named("Asia/Kolkata").unwrap());
    let ids = SequentialIds::new("p");
    let trail_ids = SequentialIds::new("tr");
    let redactor = ferb_sanitize::Redactor::default();
    let published = RefCell::new(Vec::<Value>::new());
    let journal = RefCell::new(Vec::<Value>::new());
    let sense_paused = Cell::new(false);
    let excl = Exclusions::build(&["com.example.hidden"], &[] as &[&str], &[] as &[&str]);
    let trail = Trail::new(
        &store,
        &clock,
        &trail_ids,
        &redactor,
        trail::Seams {
            exclusions: Box::new(|| excl.clone()),
            publish: Box::new(|t, v| published.borrow_mut().push(json!([t, v]))),
            device_id: Box::new(|| "dev".into()),
            paused: Box::new(|| false),
            test_task: Box::new(|_| false),
            is_browser: Box::new(|_| false),
        },
        Arc::new(Attribution::new(|| 0.0)),
    );
    let rolled = Cell::new(0);
    let expired: RefCell<Vec<Value>> = RefCell::default();
    let p = Perception::new(
        &store,
        &clock,
        &trail,
        NotificationPolicy::new(&store, &clock, &ids, &redactor),
        Seams {
            journal: Box::new(|k, v| journal.borrow_mut().push(json!([k, v]))),
            paused: Box::new(|| sense_paused.get()),
            roll_over: Box::new(|| rolled.set(rolled.get() + 1)),
            expire_derived: Box::new(|refs: &BTreeSet<String>, text, now| {
                expired.borrow_mut().push(json!([refs, text, now]))
            }),
        },
    );
    let log = f["notifications"].as_array().unwrap();
    let first = p
        .notification_floor(vec![
            n("x1", "com.chat", "A", "a"),
            n("x2", "com.chat", "B", "b"),
        ])
        .unwrap();
    d.check("floor", &json!({"op": "floor", "out": log[0]["out"], "seen": log[0]["seen"], "floor": log[0]["floor"]}),
            &json!({"op": "floor", "out": first.len(),
                    "seen": table(&store, "SELECT * FROM perception_seen ORDER BY rowid"),
                    "floor": table(&store, "SELECT source, seq FROM perception_floor ORDER BY source")}));
    let second = p
        .notification_floor(vec![n("x3", "com.chat", "C", "c")])
        .unwrap();
    let ids_out: Vec<Value> = second
        .iter()
        .map(|o| o.data["external_id"].clone())
        .collect();
    d.check("floor again", &log[1]["out"], &Value::Array(ids_out));
    let banner = p.banner_posted("FERB", "Done: filed it", "com.ferb.ferb");
    let other = p.banner_posted("FERB", "never shown", "com.ferb.ferb");
    p.banner_failed(&other);
    d.check("banners", &log[2]["banners"], &banners(&p));
    d.check("banner", &log[2]["banner"],
            &json!({"title": banner.title, "body": banner.body, "poster": banner.poster, "at": banner.at}));
    for entry in &log[3..log.len() - 1] {
        let before = published.borrow().len();
        p.notification(&note(&entry["data"])).unwrap();
        let what = format!("notification {}", entry["data"]);
        d.check(
            &format!("{what} published"),
            &entry["published"],
            &Value::Array(published.borrow()[before..].to_vec()),
        );
        d.check(
            &format!("{what} seen"),
            &entry["seen"],
            &table(&store, "SELECT * FROM perception_seen ORDER BY rowid"),
        );
        d.check(&format!("{what} banners"), &entry["banners"], &banners(&p));
    }
    sense_paused.set(true);
    p.notification(&n("z1", "com.chat", "Bob", "while paused"))
        .unwrap();
    sense_paused.set(false);
    store
        .conn()
        .execute(
            "INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES \
             ('pre', '2026-03-04T00:00:00+00:00', '2026-03-04', 'dev', 'unknown', NULL, 'notification', ?)",
            params![ferb_py::json::dumps(&json!({"external_id": "z2"}))],
        )
        .unwrap();
    p.notification(&n("z2", "com.chat", "Bob", "recorded before the receipt"))
        .unwrap();
    d.check(
        "paused and pre-recorded",
        &log[log.len() - 1]["seen"],
        &table(&store, "SELECT * FROM perception_seen ORDER BY rowid"),
    );
    d.check("trail rows", &f["trail_rows"], &trail_rows(&store));
    let installs = [
        json!({"change": "installed", "bundle_id": "a.one", "path": "/Applications/One.app"}),
        json!({"change": "installed", "bundle_id": "a.two", "path": "/Applications/Two.app"}),
        json!({"change": "installed", "bundle_id": "a.one", "path": "/Applications/One Copy.app"}),
        json!({"change": "removed", "path": "/Applications/Two.app"}),
        json!({"change": "installed", "bundle_id": "", "path": "/Applications/X.app"}),
        json!({"change": "installed", "bundle_id": "a.three", "path": "/Applications/Three.app"}),
        json!({"change": "removed", "path": "/Applications/One Copy.app"}),
    ];
    for (i, data) in installs.iter().enumerate() {
        store
            .conn()
            .execute(
                "INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES \
                 (?, '2026-03-04T00:00:00+00:00', '2026-03-04', 'dev', 'unknown', NULL, 'app_install', ?)",
                params![format!("ai{i}"), ferb_py::json::dumps(data)],
            )
            .unwrap();
    }
    d.check("installed", &f["installed"], &json!(p.installed().unwrap()));

    // the 7-day expiry, on a store of its own
    let tdb = db("expiry");
    let rows = [
        (
            "typed_text",
            "2026-02-20T10:00:00+00:00",
            json!({"bundle_id": "b", "app": "A", "pid": 3, "window_id": "w",
            "field_id": "f", "role": "AXTextField", "field": "To", "text": "old"}),
        ),
        (
            "clipboard",
            "2026-02-25T18:20:05+00:00",
            json!({"bundle_id": "b", "change_count": 4, "text": "x", "app_context": "y"}),
        ),
        (
            "clipboard",
            "2026-02-25T18:20:06+00:00",
            json!({"text": "just inside"}),
        ),
        (
            "typed_text",
            "2026-02-01T00:00:00+00:00",
            json!({"content_expired": true}),
        ),
        (
            "window",
            "2026-01-01T00:00:00+00:00",
            json!({"title": "old window"}),
        ),
    ];
    for (i, (source, at, data)) in rows.iter().enumerate() {
        tdb.conn()
            .execute(
                "INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES \
                 (?, ?, ?, 'dev', 'owner', NULL, ?, ?)",
                params![format!("x{i}"), at, &at[..10], source, ferb_py::json::dumps(data)],
            )
            .unwrap();
    }
    let ptrail = Trail::new(
        &tdb,
        &clock,
        &trail_ids,
        &redactor,
        trail::Seams {
            exclusions: Box::new(Exclusions::default),
            publish: Box::new(|_, _| {}),
            device_id: Box::new(|| "dev".into()),
            paused: Box::new(|| false),
            test_task: Box::new(|_| false),
            is_browser: Box::new(|_| false),
        },
        Arc::new(Attribution::new(|| 0.0)),
    );
    let tp = Perception::new(
        &tdb,
        &clock,
        &ptrail,
        NotificationPolicy::new(&tdb, &clock, &ids, &redactor),
        Seams {
            journal: Box::new(|_, _| {}),
            paused: Box::new(|| false),
            roll_over: Box::new(|| rolled.set(rolled.get() + 1)),
            expire_derived: Box::new(|refs: &BTreeSet<String>, text, now| {
                expired.borrow_mut().push(json!([refs, text, now]))
            }),
        },
    );
    let pt = &f["purge_text"];
    let n_expired = tp.purge_text().unwrap();
    d.check("purge_text expired", &pt["expired"], &json!(n_expired));
    d.check("purge_text rows", &pt["rows"], &trail_rows(&tdb));
    d.check("purge_text rolled", &pt["rolled"], &json!(rolled.get()));
    d.check(
        "purge_text again",
        &pt["again"],
        &json!(tp.purge_text().unwrap()),
    );
    d.check(
        "purge_text rolled after",
        &pt["rolled_after"],
        &json!(rolled.get()),
    );
    // the notices themselves are the orchestrator's (expire_derived); its arguments, as Python passes them
    let text = "This notice came from typed or copied text, which expires after 7 days.";
    d.check(
        "expire_derived",
        &json!([
            [
                ["trail:x0", "trail:x1", "trail:x3"],
                text,
                "2026-03-04T18:20:05.250000+00:00"
            ],
            [
                ["trail:x0", "trail:x1", "trail:x3"],
                text,
                "2026-03-04T18:20:05.250000+00:00"
            ]
        ]),
        &json!(*expired.borrow()),
    );
    let notice = &pt["notices"][0];
    d.eq(
        "the fixture's notice was cleared with that text",
        Some(text),
        notice["text"].as_str(),
    );

    // measurements, context, status
    let pol = NotificationPolicy::new(&store, &clock, &ids, &redactor);
    store
        .conn()
        .execute_batch(
            "INSERT INTO perception_labels VALUES ('nl1','yes','important','t'), ('nl2','no','x','t'), \
             ('nl3','yes','y','t'), ('nl4','no','z','t')",
        )
        .unwrap();
    for (i, (r, jev)) in [
        ("trail:nl1", Some("no")),
        ("trail:nl1", Some("yes")),
        ("trail:nl2", Some("yes")),
        ("trail:nl3", Some("no")),
        ("trail:nl4", None),
        ("trail:nl4", Some("no")),
    ]
    .iter()
    .enumerate()
    {
        store
            .conn()
            .execute(
                "INSERT INTO decisions (id, at, question, ref, baseline, jev, mode) VALUES \
                 (?, 't', 'notification.attention', ?, 'no', ?, 'shadow')",
                params![format!("d{i}"), r, jev],
            )
            .unwrap();
    }
    d.check(
        "measurements",
        &f["measurements"],
        &pol.measurements().unwrap(),
    );
    let ann: String = store
        .conn()
        .query_row(
            "SELECT id FROM trail WHERE json_extract(data_json,'$.external_id')='y1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    pol.told(
        "com.chat",
        "thread-1",
        &ann,
        "told, with 4111 1111 1111 1111",
    )
    .unwrap();
    let contexts = [
        json!({"id": ann, "data": {"bundle_id": "com.chat", "conversation": "thread-1"}}),
        json!({"id": "missing", "data": {}}),
        json!({"id": "missing", "data": {"bundle_id": "com.chat", "conversation": "Ann"}}),
    ];
    let got: Vec<Value> = contexts
        .iter()
        .map(|e| Value::Object(pol.context(e).unwrap()))
        .collect();
    d.check("context", &f["context"], &Value::Array(got));
    store
        .conn()
        .execute_batch(
            "INSERT INTO perception_batches VALUES ('pb1','t','age','done','[]',NULL), ('pb2','t','age','pending','[]',NULL), \
             ('pb3','t','tokens','done','[]',NULL)",
        )
        .unwrap();
    let batcher = EventBatcher::new(
        &store,
        &clock,
        &ids,
        || None,
        NotificationPolicy::new(&store, &clock, &ids, &redactor),
        |_, _| {},
    );
    let status = p
        .status(batcher.pending_count().unwrap(), (batcher.oldest_seconds)())
        .unwrap();
    d.check("status", &f["status"], &status);
    d.check(
        "told rows",
        &f["told_rows"],
        &table(&store, "SELECT * FROM notification_told ORDER BY rowid"),
    );
    d.check("journal", &f["journal"], &json!(*journal.borrow()));
    d.finish("perception");
}
