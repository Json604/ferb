//! ext_protocol.py and the small rules of ext_bridge.py against rust/fixtures/browser/protocol.json
//! (made by the Python build) and crypto.json's protocol.js constants.

mod common;

use std::collections::BTreeSet;

use ferb_browser::lax;
use ferb_browser::protocol::{self, CmdResult, Event, ExtMessage, ExtType, Hello, TabEvent};
use ferb_browser::rules::{self, BrowserApps, BrowserInfo};
use serde_json::{json, Map, Value};

use common::{b64d, fixture};

#[test]
fn constants_match_python_and_the_extension() {
    let f = fixture("protocol.json");
    let js = fixture("crypto.json");
    let c = &f["constants"];
    assert_eq!(c["protocol"], json!(protocol::PROTOCOL_VERSION));
    assert_eq!(
        js["constants"]["protocol"],
        json!(protocol::PROTOCOL_VERSION)
    );
    assert_eq!(c["min_version"], json!(protocol::MIN_EXTENSION_VERSION));
    assert_eq!(c["extension_id"], json!(protocol::FERB_EXTENSION_ID));
    assert_eq!(c["max_message"], json!(protocol::MAX_MESSAGE));
    assert_eq!(c["ext_types"], json!(protocol::EXT_TYPES));
    assert_eq!(c["pdf"], json!(protocol::PDF_EXTENSION_VERSION));
    assert_eq!(c["html"], json!(protocol::HTML_EXTENSION_VERSION));
    assert_eq!(
        c["owner_view"],
        json!(protocol::OWNER_VIEW_EXTENSION_VERSION)
    );
    assert_eq!(
        c["upload_bytes"],
        json!(protocol::UPLOAD_BYTES_EXTENSION_VERSION)
    );
    assert_eq!(c["input"], json!(protocol::INPUT_EXTENSION_VERSION));
    assert_eq!(c["upload_part"], json!(protocol::UPLOAD_PART));
    assert_eq!(js["constants"]["upload_part"], json!(protocol::UPLOAD_PART));
    assert_eq!(c["token_ttl_s"], json!(ferb_browser::pairing::TOKEN_TTL_S));
    // The op set is the extension's (protocol.js OPS); Python's `Op` type misses html and owner_view.
    assert_eq!(js["constants"]["ops"], json!(protocol::OPS.to_vec()));
    let python: BTreeSet<String> = serde_json::from_value(c["ops"].clone()).unwrap();
    let ours: BTreeSet<String> = protocol::OPS.iter().map(|s| s.to_string()).collect();
    let missing: Vec<&String> = ours.difference(&python).collect();
    assert_eq!(missing, ["html", "owner_view"]);
    assert!(python.is_subset(&ours));
    assert_eq!(c["reads"], json!(protocol::READS));
    assert_eq!(js["constants"]["read_ops"], json!(protocol::READS));
    assert_eq!(js["constants"]["daemon_signed"], json!(["cmd", "welcome"]));
}

fn outcome<T: serde::Serialize>(r: Result<T, lax::Invalid>) -> Value {
    match r {
        Ok(v) => json!({"ok": v}),
        Err(_) => json!({"error": "ValueError"}),
    }
}

#[test]
fn versions_compare_as_python_reads_them() {
    let f = fixture("protocol.json");
    for case in f["versions"].as_array().unwrap() {
        let v = case["v"].as_str().unwrap();
        let tuple = protocol::version_tuple(v).map(|t| {
            t.iter()
                .map(|p| p.as_str().parse::<u64>().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(outcome(tuple), case["tuple"], "{v:?}");
        assert_eq!(
            outcome(protocol::version_ok(v, protocol::MIN_EXTENSION_VERSION)),
            case["ok"],
            "{v:?}"
        );
        assert_eq!(
            outcome(protocol::version_ok(v, protocol::INPUT_EXTENSION_VERSION)),
            case["ok_input"],
            "{v:?}"
        );
    }
}

#[test]
fn lax_rules_are_pydantics() {
    let f = fixture("protocol.json");
    let mut n = 0;
    for (kind, cases) in f["lax"].as_object().unwrap() {
        for case in cases.as_array().unwrap() {
            let input = &case["in"];
            let got = match kind.as_str() {
                "int" => lax::int(input).map(Value::from),
                "bool" => lax::boolean(input).map(Value::from),
                "str" => lax::string(input).map(Value::from),
                _ => unreachable!(),
            };
            let want = if case.get("error").is_some() {
                None
            } else {
                Some(case["out"].clone())
            };
            // A float input pydantic took (5.0) comes back an int: compare as numbers.
            assert_eq!(got.ok(), want, "{kind} {input}");
            n += 1;
        }
    }
    assert!(n > 90);
}

fn big(case: &Value) -> Value {
    let mut raw = case["raw"].clone();
    if let Some(b) = case["big_body"].as_array() {
        let n = b[0].as_u64().unwrap() as usize;
        raw["body_json"] = Value::String(b[1].as_str().unwrap().repeat(n));
    }
    raw
}

#[test]
fn messages_parse_as_python_parses_them() {
    let f = fixture("protocol.json");
    for case in f["parse"].as_array().unwrap() {
        let raw = big(case);
        let got = protocol::parse(&raw).map(|m: ExtMessage| {
            json!({"install_id": m.install_id, "type": m.mtype.as_str(),
                   "body_json": if case["big_body"].is_null() { Value::from(m.body_json) } else { Value::Null },
                   "seq": m.seq, "mac": m.mac})
        });
        assert_eq!(
            got.unwrap_or(Value::Null),
            case["parsed"],
            "{}",
            case["raw"]
        );
    }
}

#[test]
fn bodies_are_objects() {
    let f = fixture("protocol.json");
    for case in f["body"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let m = ExtMessage {
            install_id: "inst-0123456789".into(),
            mtype: ExtType::Hello,
            body_json: text.into(),
            seq: None,
            mac: None,
        };
        let got = m.body();
        // Known differences (rust/JOURNAL.md, ferb-py): Python's json.loads reads NaN, lone surrogates and
        // integers beyond 64 bits; serde_json refuses the first two and reads the third as a float.
        if text.contains("NaN") || text.contains("ud800") {
            assert!(got.is_err() && case.get("error").is_none(), "{text}");
            continue;
        }
        if text.contains("123456789012345678901234567890") {
            assert!(got.is_ok());
            continue;
        }
        match case.get("error") {
            Some(_) => assert!(got.is_err(), "{text}"),
            None => {
                let body = Value::Object(got.unwrap());
                assert_eq!(
                    ferb_py::json::dumps(&body),
                    case["dumped"].as_str().unwrap(),
                    "{text}"
                );
            }
        }
    }
}

fn models(f: &Value, name: &str, check: impl Fn(&Map<String, Value>) -> Option<Value>) {
    for case in f["models"][name].as_array().unwrap() {
        let got = check(case["body"].as_object().unwrap());
        let want = if case.get("error").is_some() {
            None
        } else {
            Some(case["dump"].clone())
        };
        assert_eq!(got, want, "{name} {}", case["body"]);
    }
}

#[test]
fn models_validate_and_dump_as_pydantic() {
    let f = fixture("protocol.json");
    models(&f, "hello", |b| Hello::validate(b).ok().map(|h| h.dump()));
    models(&f, "result", |b| {
        CmdResult::validate(b).ok().map(|r| r.dump())
    });
    models(&f, "tab_event", |b| {
        TabEvent::validate(b).ok().map(|e| e.dump())
    });
    models(&f, "event", |b| Event::validate(b).ok().map(|e| e.dump()));
}

// decision: HAND-222
#[test]
fn both_macs_and_the_envelope_match_python() {
    let f = fixture("protocol.json");
    for case in f["signed"].as_array().unwrap() {
        let key = b64d(case["key"].as_str().unwrap());
        let (inst, seq, ty, body) = (
            case["install_id"].as_str().unwrap(),
            case["seq"].as_i64().unwrap(),
            case["type"].as_str().unwrap(),
            case["body_json"].as_str().unwrap(),
        );
        assert_eq!(
            String::from_utf8(protocol::mac_text(inst, seq, ty, body)).unwrap(),
            case["text"].as_str().unwrap()
        );
        let mac = protocol::sign(&key, inst, seq, ty, body);
        assert_eq!(mac, case["mac"].as_str().unwrap());
        let m = ExtMessage {
            install_id: inst.into(),
            mtype: ExtType::parse(ty).unwrap(),
            body_json: body.into(),
            seq: Some(seq),
            mac: Some(mac.clone()),
        };
        assert_eq!(
            protocol::authentic(&key, &m),
            Ok(case["authentic"].as_bool().unwrap())
        );
        let tampered = ExtMessage {
            body_json: format!("{body} "),
            ..m.clone()
        };
        assert_eq!(
            protocol::authentic(&key, &tampered),
            Ok(case["tampered_authentic"].as_bool().unwrap())
        );
        // hmac.compare_digest refuses text beyond ASCII (Python's TypeError)
        let odd = ExtMessage {
            mac: Some("é".into()),
            ..m
        };
        assert!(protocol::authentic(&key, &odd).is_err());
    }
    let unsigned = ExtMessage {
        install_id: "inst-0123456789abcdef".into(),
        mtype: ExtType::Hello,
        body_json: "{}".into(),
        seq: None,
        mac: None,
    };
    assert_eq!(
        protocol::authentic(&[0u8; 32], &unsigned),
        Ok(f["unsigned_authentic"].as_bool().unwrap())
    );
    for case in f["envelopes"].as_array().unwrap() {
        let key = b64d(case["key"].as_str().unwrap());
        let ty: &'static str = if case["type"] == "cmd" {
            "cmd"
        } else {
            "welcome"
        };
        let env = protocol::daemon_envelope(
            &key,
            case["nonce"].as_str().unwrap(),
            case["seq"].as_i64().unwrap(),
            ty,
            &case["body"],
        );
        assert_eq!(env.value(), case["envelope"]);
        assert_eq!(env.mac, case["mac"].as_str().unwrap());
    }
    assert_eq!(
        protocol::command("c-1", 3, "snapshot", json!({"tab_id": 4, "snap": 1}), 30000),
        f["command"]
    );
}

struct Apps(Value);

impl BrowserApps for Apps {
    fn known(&self, bundle_id: &str) -> bool {
        self.0["known"][bundle_id]
            .as_bool()
            .expect("every app the fixture asks about")
    }

    fn browser(&self, key: &str) -> Option<BrowserInfo> {
        let b = self.0["browsers"].get(key)?;
        Some(BrowserInfo {
            bundle_id: b["bundle_id"].as_str()?.into(),
            label: b["label"].as_str()?.into(),
        })
    }
}

// decisions: HAND-218, HAND-201, HAND-406, HAND-223, HAND-435, HAND-441
#[test]
fn text_rules_match_python() {
    let f = fixture("protocol.json");
    let t = &f["text"];
    // the daemon follows the extension's entersText (tests/keys.rs); Python's differs on these six
    let fixed = ["Cmd+V", "cmd+shift+v", "a+", "é", "hyper+a", "x+a"];
    for case in t["enters_text"].as_array().unwrap() {
        let spec = case["spec"].as_str();
        let python = case["enters"].as_bool().unwrap();
        let differs = spec.is_some_and(|s| fixed.contains(&s));
        assert_eq!(rules::enters_text(spec), python != differs, "{spec:?}");
    }
    for case in t["group_title"].as_array().unwrap() {
        assert_eq!(
            rules::group_title(case["text"].as_str().unwrap()),
            case["title"].as_str().unwrap()
        );
    }
    assert_eq!(rules::too_old("0.18.2"), t["too_old"].as_str().unwrap());
    for case in t["version_refusal"].as_array().unwrap() {
        let (code, why) = rules::version_refusal(
            case["version"].as_str().unwrap(),
            case["protocol"].as_i64().unwrap(),
        );
        assert_eq!(json!([code, why]), case["out"]);
    }
    let apps = Apps(t.clone());
    for case in t["peer_refusal"].as_array().unwrap() {
        let got = rules::peer_refusal(&apps, case["app"].as_str(), case["browser"].as_str());
        assert_eq!(
            got.map(Value::from).unwrap_or(Value::Null),
            case["why"],
            "{case}"
        );
    }
    for case in t["not_responding"].as_array().unwrap() {
        let steps = (!case["steps"].is_null()).then(|| &case["steps"]);
        let e = rules::not_responding(
            case["op"].as_str().unwrap(),
            case["timeout"].as_f64().unwrap(),
            case["step"].as_str(),
            steps,
        );
        assert_eq!(e.message(), case["message"].as_str().unwrap());
        assert_eq!(Value::Object(e.facts().unwrap().clone()), case["facts"]);
        assert_eq!(e.timing.unwrap_or(Value::Null), case["timing"]);
    }
    let steps: Map<String, Value> = rules::READ_STEPS
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect();
    assert_eq!(Value::Object(steps), t["read_steps"]);
}

#[test]
fn fold_is_casefold_where_it_can_meet_ascii() {
    assert_eq!(rules::fold("ſpace"), "space");
    assert_eq!(rules::fold("CMD"), "cmd");
    assert_eq!(rules::fold("Straße"), "strasse");
    // the extension's table (keys.rs): "ſpace" names no key there, and the Kelvin sign is no modifier
    assert!(!rules::enters_text(Some("ſpace")));
    assert!(!rules::enters_text(Some("\u{212A}+v")));
    assert_eq!(rules::fmt_g(30.0), "30");
    assert_eq!(rules::fmt_g(0.25), "0.25");
    assert_eq!(rules::fmt_g(1e-5), "1e-05");
    assert_eq!(rules::fmt_g(1234567.0), "1.23457e+06");
}
