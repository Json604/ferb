//! pairing.py against rust/fixtures/browser/pairing.json: the same calls on a store made by the real
//! migrations give the same verdicts and the same ext_installs rows, byte for byte (column types
//! included), so pairings made by either daemon survive the other.
#![cfg(target_os = "macos")]

mod common;

use std::sync::{Arc, Mutex};

use ferb_browser::pairing::{sqlite::SqliteInstalls, Pairings, Verdict};
use ferb_browser::protocol;
use ferb_core::interfaces::{BoxFuture, Clock};
use ferb_core::types::Identity;
use ferb_core::when::DateTime;
use serde_json::{json, Value};

use common::{fixture, Counter};

struct FakeClock {
    now: Mutex<(DateTime, f64)>,
}

impl FakeClock {
    fn advance(&self, s: f64) {
        let mut g = self.now.lock().unwrap();
        g.0 += chrono::Duration::microseconds((s * 1e6).round() as i64);
        g.1 += s;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime {
        self.now.lock().unwrap().0
    }
    fn monotonic(&self) -> f64 {
        self.now.lock().unwrap().1
    }
    fn sleep_until(&self, _: DateTime) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

fn rows(path: &std::path::Path) -> Value {
    let conn = rusqlite::Connection::open(path).unwrap();
    let mut st = conn
        .prepare("SELECT install_id, browser, folder, identity, hex(key), typeof(key), last_seq, typeof(last_seq), \
                  paired_at, ext_version FROM ext_installs ORDER BY install_id")
        .unwrap();
    let out: Vec<Value> = st
        .query_map([], |r| {
            Ok(
                json!({"install_id": r.get::<_, String>(0)?, "browser": r.get::<_, String>(1)?,
                      "folder": r.get::<_, String>(2)?, "identity": r.get::<_, String>(3)?,
                      "key": r.get::<_, String>(4)?, "key_type": r.get::<_, String>(5)?,
                      "last_seq": r.get::<_, i64>(6)?, "seq_type": r.get::<_, String>(7)?,
                      "paired_at": r.get::<_, String>(8)?, "ext_version": r.get::<_, String>(9)?}),
            )
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    Value::Array(out)
}

fn verdict(v: &Verdict) -> Value {
    json!({"accepted": v.accepted, "reason": v.reason,
           "reply": v.reply.as_ref().map(|r| serde_json::from_str::<Value>(&r.wire()).unwrap()),
           "binding": v.binding.as_ref().map(|b| json!({"install_id": b.install_id, "browser": b.browser,
                                                         "folder": b.folder, "identity": b.identity.as_str()}))})
}

fn identity(s: &str) -> Identity {
    if s == "ferb" {
        Identity::Ferb
    } else {
        Identity::Owner
    }
}

// decisions: HAND-222, HAND-406
#[test]
fn pairing_matches_python_row_for_row() {
    let f = fixture("pairing.json");
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("ferb.db");
    let db = ferb_store::Db::open(&path).unwrap();
    db.migrate().unwrap();
    db.close().unwrap();
    let clock = Arc::new(FakeClock {
        now: Mutex::new((
            ferb_core::when::parse(f["start"].as_str().unwrap()).unwrap(),
            f["monotonic_start"].as_f64().unwrap(),
        )),
    });
    let store = Arc::new(SqliteInstalls::new(
        rusqlite::Connection::open(&path).unwrap(),
    ));
    let p = Pairings::new(store, clock.clone(), Arc::new(Counter::new()));
    let mut n = 0;
    for step in f["steps"].as_array().unwrap() {
        let a = &step["args"];
        let s = |k: &str| a[k].as_str().unwrap();
        let got = match step["op"].as_str().unwrap() {
            "issue_token" => {
                json!({"value": p.issue_token(s("browser"), s("folder"), identity(s("identity")))})
            }
            "pending" => json!({"value": p.pending(s("browser"), s("folder"))}),
            "handle" => {
                let m = protocol::parse(&a["raw"]).expect("a valid frame");
                json!({"verdict": verdict(&p.handle(&m).unwrap())})
            }
            "key" => match p.key(s("install_id")).unwrap() {
                Some(k) => {
                    json!({"bytes": k.expose().iter().map(|b| format!("{b:02x}")).collect::<String>()})
                }
                None => json!({"value": null}),
            },
            "version" => json!({"value": p.version(s("install_id")).unwrap()}),
            "record_version" => {
                p.record_version(s("install_id"), s("version")).unwrap();
                json!({"value": null})
            }
            "advance" => {
                assert_eq!(rows(&path), step["rows"]);
                clock.advance(a["seconds"].as_f64().unwrap());
                n += 1;
                continue;
            }
            "revoke" => {
                p.revoke(s("token"));
                json!({"value": null})
            }
            "binding" => {
                let b = p.binding(s("install_id")).unwrap().unwrap();
                json!({"binding": {"install_id": b.install_id, "browser": b.browser, "folder": b.folder,
                                   "identity": b.identity.as_str()}})
            }
            "for_profile" => match p.for_profile(s("browser"), s("folder")).unwrap() {
                Some(b) => {
                    json!({"binding": {"install_id": b.install_id, "browser": b.browser, "folder": b.folder,
                                              "identity": b.identity.as_str()}})
                }
                None => json!({"value": null}),
            },
            "all" => {
                let all = p.all().unwrap();
                if all.is_empty() {
                    json!({"value": null})
                } else {
                    json!({"bindings": all.iter().map(|b| json!([b.install_id, b.browser, b.folder, b.identity.as_str()]))
                                            .collect::<Vec<_>>()})
                }
            }
            "unpair" => json!({"value": p.unpair(s("browser"), s("folder")).unwrap()}),
            "paired" => json!({"value": p.paired_tokens()}),
            other => panic!("unknown step {other}"),
        };
        for (k, v) in got.as_object().unwrap() {
            assert_eq!(v, &step[k], "step {n} {}", step["op"]);
        }
        assert_eq!(
            rows(&path),
            step["rows"],
            "rows after step {n} {}",
            step["op"]
        );
        n += 1;
    }
    assert!(n > 50);
}

#[test]
fn keys_and_replies_never_print() {
    let k = ferb_browser::pairing::PairingKey::new(vec![1, 2, 3]);
    assert_eq!(format!("{k:?}"), "<pairing key>");
}
