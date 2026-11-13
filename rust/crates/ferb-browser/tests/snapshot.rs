//! snapshot.py against rust/fixtures/browser/snapshot.json: elements as the extension sends them,
//! previews and full forms, bot-check frames.

mod common;

use ferb_browser::snapshot;
use ferb_core::types::{PageElement, PageSnapshot};
use serde_json::Value;

use common::fixture;

fn elements(raws: &[Value], snap: u32) -> Vec<PageElement> {
    raws.iter()
        .enumerate()
        .map(|(i, r)| {
            let frame = if snap == 1 {
                if i % 3 == 0 {
                    "2"
                } else {
                    "0"
                }
            } else {
                "0"
            };
            snapshot::element(&format!("s{snap}e{}", i + 1), frame, r.as_object().unwrap()).unwrap()
        })
        .collect()
}

// decisions: HAND-208, HAND-300, HAND-483, HAND-513
#[test]
fn snapshots_project_as_python_projects_them() {
    let f = fixture("snapshot.json");
    let raws: Vec<Value> = f["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["raw"].clone())
        .collect();
    let small = elements(&raws, 1);
    for (case, e) in f["elements"].as_array().unwrap().iter().zip(&small) {
        assert_eq!(
            serde_json::to_value(e).unwrap(),
            case["element"],
            "{}",
            case["raw"]
        );
    }
    let many = elements(f["many_raw"].as_array().unwrap(), 2);
    let mut n = 0;
    for (name, case) in f["snapshots"].as_object().unwrap() {
        let mut s = case["snapshot"].clone();
        let which = s["elements"].as_str().unwrap().to_string();
        s["elements"] = Value::Array(vec![]);
        let mut snap: PageSnapshot = serde_json::from_value(s).unwrap();
        snap.elements = if which == "many" {
            many.clone()
        } else {
            small.clone()
        };
        for (form, full, probe) in [
            ("preview", false, false),
            ("full", true, false),
            ("probe", false, true),
        ] {
            if case[form].is_null() {
                continue;
            }
            let got = Value::Object(snapshot::for_model(&snap, full, probe));
            assert_eq!(
                ferb_py::json::dumps_utf8(&got),
                ferb_py::json::dumps_utf8(&case[form]),
                "{name} {form}"
            );
            n += 1;
        }
    }
    assert_eq!(n, 7);
    for case in f["bot_check"].as_array().unwrap() {
        let urls: Vec<String> = serde_json::from_value(case["urls"].clone()).unwrap();
        assert_eq!(
            snapshot::bot_check(&urls).unwrap_or(Value::Null),
            case["check"],
            "{urls:?}"
        );
    }
}

#[test]
fn a_rect_that_isnt_numbers_fails_as_in_python() {
    let raw = serde_json::json!({"rect": [1, 2, 3]});
    assert!(snapshot::element("s1e1", "0", raw.as_object().unwrap()).is_err());
    let raw = serde_json::json!({"rect": ["x", 2, 3, 4]});
    assert!(snapshot::element("s1e1", "0", raw.as_object().unwrap()).is_err());
}
