//! Stage 3 S3 (T-1202): the phone area is part of the composed `ferbd daemon`. FERB.app's Settings → Phone answers
//! (no phone paired, at most five, no pairing waiting, an empty audit); a migrated store has the phone tables; and no
//! caller of this test is the phone link's helper, so a hello naming "phone" or "link" gets Python's refusal of an
//! unknown role, whatever device it names.

mod support;

use serde_json::{json, Value};
use support::*;

fn call(run: &Run, method: &str, params: Value) -> Value {
    let mut c = Raw::connect(&run.socket()).unwrap();
    c.hello("cli").unwrap();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string());
    serde_json::from_str(&c.recv().expect("an answer")).unwrap()
}

// decision: PHN-419
// decision: PHN-441
// decision: PHN-468
// decision: PHN-469
#[test]
fn the_phone_area_is_wired_and_only_the_helper_may_open_a_phone_session() {
    let tmp = root();
    let mut run = Run::start(tmp.path(), "01", json!({}));
    let listed = call(&run, "phone.list", json!({}));
    assert_eq!(listed["result"]["phones"], json!([]), "{listed}");
    assert_eq!(listed["result"]["max"], 5);
    assert_eq!(listed["result"]["pairing"], Value::Null);
    let audit = call(&run, "phone.audit", json!({}));
    assert_eq!(audit["result"]["rows"], json!([]), "{audit}");
    // FERB.app may not make the helper's calls
    let refused = call(&run, "link.devices", json!({}));
    assert_eq!(refused["error"]["code"], -32001, "{refused}");
    for role in ["phone", "link"] {
        let mut c = Raw::connect(&run.socket()).unwrap();
        c.send(
            &json!({"jsonrpc": "2.0", "id": 0, "method": "auth.hello",
                    "params": {"role": role, "device": "any", "session": "s"}})
            .to_string(),
        );
        let v: Value = serde_json::from_str(&c.recv().unwrap()).unwrap();
        assert_eq!(v["error"]["code"], -32600, "{v}");
    }
    run.stop();
    let db = rusqlite::Connection::open(run.home.join("ferb.db")).unwrap();
    let tables: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('phone_devices', 'phone_audit')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 2);
}
