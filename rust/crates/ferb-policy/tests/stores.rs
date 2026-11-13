//! The policy's stores against what the Python stores did (decisions: ENG-3, GATE-205, GATE-218, GATE-221, D30):
//! `rust/fixtures/policy/make.py` drove KnownDestinations, Confirmations and StandingPermissions on a real Database
//! in a temporary folder with a fixed clock and sequential ids, and seeded rows for the daily digest and the sites a
//! tree contacted. Each scenario is replayed here on a store the Rust code made; returns and rows must be the same,
//! text columns byte for byte. Temporary folders only.

#![cfg(target_os = "macos")]

mod common;

use std::sync::Mutex;

use common::*;
use ferb_core::ids::SequentialIds;
use ferb_core::interfaces::{BoxFuture, Clock};
use ferb_core::types::ActionSpec;
use ferb_core::when::{self, DateTime};
use ferb_policy::standing::{self, SendScope};
use ferb_policy::stores::{self, Confirmations, KnownDestinations, StandingPermissions};
use ferb_store::Db;
use rusqlite::types::Value as Sql;
use serde_json::{json, Map, Value};

/// make.py's clock: each reading one second later, with 0, 250, 500 or 750 ms.
struct StepClock(Mutex<i64>);

impl Clock for StepClock {
    fn now(&self) -> DateTime {
        let mut n = self.0.lock().unwrap();
        *n += 1;
        let base = when::parse("2026-01-02T03:04:05+00:00").unwrap();
        base + chrono::TimeDelta::seconds(*n) + chrono::TimeDelta::microseconds(250_000 * (*n % 4))
    }

    fn monotonic(&self) -> f64 {
        0.0
    }

    fn sleep_until(&self, _t: DateTime) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

fn sql_value(v: &Value) -> Sql {
    match v {
        Value::Null => Sql::Null,
        Value::Bool(b) => Sql::Integer(i64::from(*b)),
        Value::Number(n) if n.is_i64() => Sql::Integer(n.as_i64().unwrap()),
        Value::Number(n) => Sql::Real(n.as_f64().unwrap()),
        Value::String(s) => Sql::Text(s.clone()),
        other => Sql::Text(other.to_string()),
    }
}

fn dump(db: &Db, table: &str) -> Value {
    let mut stmt = db
        .conn()
        .prepare(&format!(
            "SELECT rowid AS _rowid, * FROM {table} ORDER BY rowid"
        ))
        .unwrap();
    let names: Vec<String> = stmt.column_names().map(|s| s.to_string()).collect();
    let rows = stmt
        .query_map([], |r| {
            let mut m = Map::new();
            for (i, n) in names.iter().enumerate() {
                let v = match r.get_ref(i)? {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(x) => json!(x),
                    rusqlite::types::ValueRef::Real(x) => json!(x),
                    rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                    rusqlite::types::ValueRef::Blob(b) => json!(String::from_utf8_lossy(b)),
                };
                m.insert(n.clone(), v);
            }
            Ok(Value::Object(m))
        })
        .unwrap();
    Value::Array(rows.map(Result::unwrap).collect())
}

fn spec(v: &Value) -> ActionSpec {
    ActionSpec {
        target: target(&v["target"]).unwrap(),
        content: opt_s(&v["content"]),
        declared: opt_s(&v["declared"]),
        effect_kind: opt_s(&v["effect_kind"]),
        ..ActionSpec::new(&s(&v["kind"]))
    }
}

// decision: GATE-205, GATE-218, GATE-221, D30
#[test]
fn stores_match_python() {
    let fx = fixture("stores.json");
    let mut d = Diffs::new();
    for (n, scenario) in fx["scenarios"].as_array().unwrap().iter().enumerate() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("ferb.db")).unwrap();
        db.migrate().unwrap();
        let clock = StepClock(Mutex::new(0));
        let ids = SequentialIds::new("id");
        let red = ferb_sanitize::Redactor::new(&[], &[]);
        red.add_secret_value("quietlemon4417");
        let kd = KnownDestinations {
            db: &db,
            clock: &clock,
            ids: &ids,
            redact: &red,
        };
        let cf = Confirmations {
            db: &db,
            clock: &clock,
            ids: &ids,
            redact: &red,
        };
        let sp = StandingPermissions { db: &db };
        for step in scenario["steps"].as_array().unwrap() {
            let a = &step["args"];
            let got: Value = match step["op"].as_str().unwrap() {
                "record_dest" => {
                    kd.record(
                        &s(&a[0]),
                        &s(&a[1]),
                        &s(&a[2]),
                        a[3].as_str(),
                        a[4].as_str(),
                    )
                    .unwrap();
                    Value::Null
                }
                "known" => json!(kd.known(&s(&a[0]), &s(&a[1])).unwrap()),
                "approved" => json!(kd.approved(&s(&a[0]), &s(&a[1])).unwrap()),
                "record_conf" => {
                    json!(cf
                        .record(
                            &s(&a[0]),
                            &s(&a[1]),
                            &s(&a[2]),
                            a[3].as_bool().unwrap(),
                            a[4].as_str()
                        )
                        .unwrap())
                }
                "find" => match cf.find(&s(&a[0]), &s(&a[1]), &strs(&a[2])).unwrap() {
                    None => Value::Null,
                    Some(r) => {
                        json!({"id": r.id, "tree_id": r.tree_id, "rule": r.rule, "key": r.key,
                        "granted": i64::from(r.granted), "ask_span": r.ask_span, "used": i64::from(r.used), "at": r.at})
                    }
                },
                "noes" => json!(cf.noes(&s(&a[0])).unwrap()),
                "refused" => json!(cf.refused(&s(&a[0])).unwrap()),
                "covers" => json!(cf.covers(&s(&a[0]), &s(&a[1])).unwrap()),
                "use" => {
                    cf.use_(&s(&a[0])).unwrap();
                    Value::Null
                }
                "release" => {
                    cf.release(&s(&a[0])).unwrap();
                    Value::Null
                }
                "age" => {
                    let row = db
                        .conn()
                        .query_row(
                            "SELECT id, tree_id, rule, key, granted, ask_span, used, at FROM confirmations WHERE id = ?",
                            [s(&a[0])],
                            |r| {
                                Ok(stores::Confirmation {
                                    id: r.get(0)?,
                                    tree_id: r.get(1)?,
                                    rule: r.get(2)?,
                                    key: r.get(3)?,
                                    granted: r.get::<_, i64>(4)? != 0,
                                    ask_span: r.get(5)?,
                                    used: r.get::<_, i64>(6)? != 0,
                                    at: r.get(7)?,
                                })
                            },
                        )
                        .unwrap();
                    json!(cf.age_s(&row))
                }
                "grant" => {
                    let scopes: Vec<SendScope> = a[1]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|x| serde_json::from_value(x.clone()).unwrap())
                        .collect();
                    sp.grant(&s(&a[0]), &scopes, &s(&a[2])).unwrap();
                    Value::Null
                }
                "covers_standing" => {
                    let sp_spec = spec(&a[3]);
                    let reaches = target(&a[4]);
                    let covered = sp
                        .covers(&s(&a[0]), &s(&a[1]), &s(&a[2]), &sp_spec, reaches.as_ref())
                        .unwrap();
                    d.check(
                        "sends",
                        step,
                        &step["sends"],
                        &json!(standing::sends(&sp_spec)),
                    );
                    json!(covered)
                }
                "scopes" => {
                    let scopes: Vec<Value> = sp
                        .scopes(&s(&a[0]))
                        .unwrap()
                        .iter()
                        .map(|x| serde_json::from_str(&x.dump_json()).unwrap())
                        .collect();
                    json!(scopes)
                }
                "revoke" => {
                    sp.revoke(&s(&a[0])).unwrap();
                    Value::Null
                }
                "revoke_approval" => json!(sp.revoke_approval(&s(&a[0])).unwrap()),
                "held" => json!(sp.held(&s(&a[0])).unwrap()),
                "approvals" => json!(sp.approvals(&s(&a[0])).unwrap()),
                other => panic!("unknown op {other}"),
            };
            let want = step.get("out").cloned().unwrap_or(Value::Null);
            d.check(&format!("scenario {n} {}", step["op"]), step, &want, &got);
        }
        for table in [
            "known_destinations",
            "confirmations",
            "standing_permissions",
        ] {
            // text columns byte for byte (the scope JSON included)
            let got = dump(&db, table);
            let want = &scenario["rows"][table];
            d.check(
                &format!("scenario {n} rows {table}"),
                &json!(table),
                want,
                &got,
            );
            assert_eq!(
                want.to_string(),
                got.to_string(),
                "scenario {n}: {table} rows differ in their bytes"
            );
        }
    }
    let digest = &fx["digest"];
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    for row in digest["seed"].as_array().unwrap() {
        let params: Vec<Sql> = row[1].as_array().unwrap().iter().map(sql_value).collect();
        db.conn()
            .execute(&s(&row[0]), rusqlite::params_from_iter(params))
            .unwrap();
    }
    d.check(
        "security digest",
        &Value::Null,
        &digest["out"],
        &stores::security_digest(&db, "2026-01-02").unwrap(),
    );
    for (tree, want) in digest["contacted"].as_object().unwrap() {
        d.check(
            "sites contacted",
            &json!(tree),
            want,
            &json!(stores::sites_contacted(&db, tree).unwrap()),
        );
    }
    let small = fixture("small.json");
    for c in small["owner_named"].as_array().unwrap() {
        let scope: SendScope = serde_json::from_value(c["scope"].clone()).unwrap();
        d.check(
            "owner_named",
            c,
            &c["out"],
            &json!(standing::owner_named(&scope, &s(&c["text"]))),
        );
    }
    d.finish("stores");
}
