//! ids, errors, timing and call facts against rust/fixtures/core (ids.json, errors.json, timing.json).

mod common;

use std::sync::{Arc, Mutex};

use common::{compact, fixture, strs};
use ferb_core::call_facts;
use ferb_core::errors::FerbError;
use ferb_core::ids::{RandomIds, SequentialIds};
use ferb_core::interfaces::IdSource;
use ferb_core::timing::{CallTimer, Now, Steps};
use serde_json::Value;

fn uuid4_hex(id: &str) -> bool {
    let b = id.as_bytes();
    b.len() == 32
        && b.iter()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
        && b[12] == b'4'
        && matches!(b[16], b'8' | b'9' | b'a' | b'b')
}

#[test]
fn random_ids_have_pythons_shape_and_sequential_ids_match_exactly() {
    let fx = fixture("ids.json");
    for id in strs(&fx["random"]) {
        assert!(uuid4_hex(id), "the fixture's own id {id}");
    }
    let made: std::collections::BTreeSet<String> = (0..500).map(|_| RandomIds.new_id()).collect();
    assert_eq!(made.len(), 500);
    assert!(made.iter().all(|id| uuid4_hex(id)));
    let task = SequentialIds::new("task");
    let got: Vec<String> = (0..12).map(|_| task.new_id()).collect();
    assert_eq!(got, strs(&fx["sequential_task"]));
    let plain = SequentialIds::default();
    assert_eq!(
        vec![plain.new_id(), plain.new_id()],
        strs(&fx["sequential_default"])
    );
}

fn build(row: &Value) -> FerbError {
    let message = row["message"].as_str().unwrap().to_string();
    let args = &row["args"];
    match row["class"].as_str().unwrap() {
        "FerbError" => FerbError::Ferb(message),
        "RuntimeUnavailable" => FerbError::RuntimeUnavailable(message),
        "Unsupported" => FerbError::Unsupported(message),
        "ModelLimitsUnknown" => FerbError::ModelLimitsUnknown(message),
        "InputTooLarge" => {
            let what = message
                .split(" is ")
                .next()
                .filter(|w| *w != "this call's context");
            FerbError::input_too_large(
                args["chars"].as_u64().unwrap(),
                args["max_chars"].as_u64().unwrap(),
                what,
            )
        }
        "PlanRejected" => FerbError::PlanRejected {
            problems: strs(&args["problems"])
                .into_iter()
                .map(String::from)
                .collect(),
        },
        "AuthError" => FerbError::AuthError(message),
        "NotRunning" => FerbError::NotRunning(message),
        "AlreadyRunning" => FerbError::AlreadyRunning(message),
        "Deferred" => FerbError::Deferred(message),
        "Refused" => FerbError::Refused(message),
        "StaleRef" => FerbError::StaleRef(message),
        "NoTarget" => FerbError::NoTarget(message),
        "Withheld" => FerbError::Withheld(message),
        "DriverError" => FerbError::DriverError {
            message,
            facts: args["facts"].as_object().unwrap().clone(),
        },
        other => panic!("no Rust error for {other}"),
    }
}

#[test]
fn errors_keep_pythons_names_messages_and_families() {
    let fx = fixture("errors.json");
    let mut classes = std::collections::BTreeSet::new();
    for row in fx.as_array().unwrap() {
        let e = build(row);
        assert_eq!(e.class(), row["class"].as_str().unwrap());
        assert_eq!(e.to_string(), row["message"].as_str().unwrap());
        assert_eq!(
            e.is_ferb_error(),
            row["ferb_error"].as_bool().unwrap(),
            "{e:?}"
        );
        assert_eq!(
            e.is_value_error(),
            row["value_error"].as_bool().unwrap(),
            "{e:?}"
        );
        assert_eq!(e.is_refused(), row["refused"].as_bool().unwrap(), "{e:?}");
        assert_eq!(
            e.is_unsupported(),
            row["unsupported"].as_bool().unwrap(),
            "{e:?}"
        );
        if let FerbError::InputTooLarge { runtime, .. } = &e {
            assert_eq!(runtime, row["args"]["runtime"].as_str().unwrap());
        }
        classes.insert(e.class());
    }
    assert_eq!(classes.len(), 15, "every error class is in the fixture");
}

fn clock() -> (Arc<Mutex<f64>>, Now) {
    let t = Arc::new(Mutex::new(100.0));
    let read = t.clone();
    (t, Box::new(move || *read.lock().unwrap()))
}

// decision: ORCH-526
#[test]
fn steps_add_up_as_in_python() {
    let fx = fixture("timing.json");
    for case in fx["steps"].as_array().unwrap() {
        let (t, now) = clock();
        let mut steps = Steps::new(now);
        let mut took = Vec::new();
        for op in case["script"].as_array().unwrap() {
            *t.lock().unwrap() = 100.0 + op[1].as_f64().unwrap();
            took.push(steps.mark(op[2].as_str().unwrap()));
        }
        let expected: Vec<i64> = case["took"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap())
            .collect();
        assert_eq!(took, expected);
        assert_eq!(
            serde_json::to_string(&steps.facts()).unwrap(),
            compact(case["facts"].as_str().unwrap())
        );
        assert_eq!(steps.total_ms(), case["total_ms"].as_i64().unwrap());
        assert_eq!(took.iter().sum::<i64>(), steps.total_ms());
    }
}

// decision: RUN-419
#[test]
fn call_timers_report_pythons_timing() {
    let fx = fixture("timing.json");
    for case in fx["calls"].as_array().unwrap() {
        let (t, now) = clock();
        let mut timer = CallTimer::new(now, case["by"].as_str().unwrap());
        let mut done = Vec::new();
        for op in case["script"].as_array().unwrap() {
            *t.lock().unwrap() = 100.0 + op[1].as_f64().unwrap();
            match op[0].as_str().unwrap() {
                "begin" => timer.begin(),
                "sent" => timer.sent(),
                "output" => timer.output(),
                "handed_back" => timer.handed_back(),
                "done" => {
                    let facts = timer.done();
                    let parts: i64 = ["runtime_ms", "ttft_ms", "generate_ms", "tail_ms"]
                        .iter()
                        .map(|k| facts.get(*k).and_then(Value::as_i64).unwrap_or(0))
                        .sum();
                    assert_eq!(parts, facts["total_ms"].as_i64().unwrap());
                    done.push(serde_json::to_string(&facts).unwrap());
                }
                other => panic!("{other}"),
            }
        }
        let expected: Vec<String> = strs(&case["done"]).into_iter().map(compact).collect();
        assert_eq!(done, expected);
    }
}

#[tokio::test]
async fn call_facts_are_kept_for_the_call_that_is_open() {
    let fx = fixture("timing.json");
    call_facts::note("point", "nobody records this");
    assert!(call_facts::current().is_none());
    let (inner, outer) = call_facts::recording(async {
        call_facts::note("point", "ax");
        tokio::task::yield_now().await;
        call_facts::note("point", "ocr");
        // a worker thread doing part of the call carries the record in
        let held = call_facts::current().unwrap();
        tokio::task::spawn_blocking(move || held.enter(|| call_facts::note("route", "background")))
            .await
            .unwrap();
        let ((), inner) = call_facts::recording(async { call_facts::note("point", "inner") }).await;
        call_facts::note("point", "after");
        inner
    })
    .await;
    assert_eq!(
        serde_json::to_string(&outer).unwrap(),
        compact(fx["call_facts"]["outer"].as_str().unwrap())
    );
    assert_eq!(
        serde_json::to_string(&inner).unwrap(),
        compact(fx["call_facts"]["inner"].as_str().unwrap())
    );
}
