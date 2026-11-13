//! The event bus against the Python bus's own answers (rust/fixtures/core/bus.json), on a fake clock.

mod common;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use common::{fixture, strs};
use ferb_core::bus::{
    Bus, BusError, Event, EventClass, Subscription, CRITICAL, DROPPED, TELEMETRY,
};
use ferb_core::interfaces::{BoxFuture, Clock};
use ferb_core::when::{self, DateTime};
use serde_json::{json, Value};

/// Time moves only when the test says so (TEST-508).
struct FakeClock(Mutex<DateTime>);

impl FakeClock {
    fn new() -> Arc<Self> {
        Arc::new(FakeClock(Mutex::new(
            when::parse("2026-05-06T07:08:09+00:00").unwrap(),
        )))
    }

    fn advance(&self, seconds: f64) {
        *self.0.lock().unwrap() += chrono::Duration::microseconds((seconds * 1e6) as i64);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime {
        *self.0.lock().unwrap()
    }

    fn monotonic(&self) -> f64 {
        0.0
    }

    fn sleep_until(&self, _t: DateTime) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

fn frame(ev: &Event) -> Value {
    json!({"topic": ev.topic, "at": when::isoformat(&ev.at), "class": ev.class().as_str(), "data": ev.data})
}

#[test]
fn the_topic_lists_are_pythons() {
    let fx = fixture("bus.json");
    let sorted = |topics: &[&'static str]| {
        let mut v = topics.to_vec();
        v.sort_unstable();
        v
    };
    assert_eq!(sorted(CRITICAL), strs(&fx["critical"]));
    assert_eq!(sorted(TELEMETRY), strs(&fx["telemetry"]));
    assert_eq!(DROPPED, fx["dropped"].as_str().unwrap());
    assert!(CRITICAL.iter().all(|t| !TELEMETRY.contains(t)));
}

// decision: DMN-548, DMN-580, APP-893, ORCH-531
#[test]
fn every_script_gives_what_the_python_bus_gave() {
    let fx = fixture("bus.json");
    for (n, case) in fx["scripts"].as_array().unwrap().iter().enumerate() {
        let clock = FakeClock::new();
        let bus = Bus::new(clock.clone());
        let mut subs: HashMap<String, Subscription> = HashMap::new();
        let mut reported: HashSet<String> = HashSet::new(); // subscribers already told of their overflow
        let script = case["script"].as_array().unwrap();
        for (step, (op, expected)) in script
            .iter()
            .zip(case["out"].as_array().unwrap())
            .enumerate()
        {
            let got = match op[0].as_str().unwrap() {
                "sub" => {
                    let topics: Option<Vec<&str>> = op[2].as_array().map(|_| strs(&op[2]));
                    let sub = bus.subscribe(topics.as_deref(), op[3].as_u64().unwrap() as usize);
                    subs.insert(op[1].as_str().unwrap().to_string(), sub);
                    Value::Null
                }
                "pub" => match bus.publish(op[1].as_str().unwrap(), op[2].as_object().cloned()) {
                    Ok(ev) => frame(&ev),
                    Err(e) => json!({"error": e.to_string()}),
                },
                "advance" => {
                    clock.advance(op[1].as_f64().unwrap());
                    Value::Null
                }
                "close" => {
                    subs[op[1].as_str().unwrap()].close();
                    Value::Null
                }
                "read" => {
                    let sub = &subs[op[1].as_str().unwrap()];
                    let mut events = Vec::new();
                    while let Some(next) = sub.try_next() {
                        match next {
                            Ok(Some(ev)) => events.push(frame(&ev)),
                            Ok(None) => {
                                events.push(json!({"closed": true}));
                                break;
                            }
                            Err(e) => {
                                events.push(json!({"overflow": e.to_string()}));
                                break;
                            }
                        }
                    }
                    json!({"events": events, "dropped": sub.dropped(), "overflowed": sub.overflowed(),
                           "closed": sub.closed(), "subscribers": bus.subscribers()})
                }
                other => panic!("{other}"),
            };
            let mut expected = expected.clone();
            if op[0] == "read" && reported.contains(op[1].as_str().unwrap()) {
                // Python's iterator is a generator: once it has raised Overflow it is finished, and a second read
                // just ends. Here an overflowed subscriber keeps saying so.
                assert_eq!(expected["events"], json!([{"closed": true}]));
                expected["events"] = json!([{"overflow": BusError::Overflow.to_string()}]);
            }
            assert_eq!(got, expected, "script {n}, step {step}: {op}");
            if got["events"]
                .as_array()
                .is_some_and(|e| e.iter().any(|ev| ev.get("overflow").is_some()))
            {
                reported.insert(op[1].as_str().unwrap().to_string());
            }
        }
    }
}

#[tokio::test]
async fn a_waiting_reader_wakes_for_an_event_a_close_and_an_overflow() {
    let bus = Bus::new(FakeClock::new());
    let sub = Arc::new(bus.subscribe(Some(&["task.*"]), 1));
    let reader = tokio::spawn({
        let sub = sub.clone();
        async move { sub.next().await }
    });
    tokio::task::yield_now().await;
    assert!(!reader.is_finished());
    bus.publish("task.created", None).unwrap();
    let ev = reader.await.unwrap().unwrap().unwrap();
    assert_eq!(
        (ev.topic.as_str(), ev.class()),
        ("task.created", EventClass::Critical)
    );

    // a second critical event can't be queued behind the first: the reader is told, never left waiting
    bus.publish("task.created", None).unwrap();
    bus.publish("task.finished", None).unwrap();
    assert_eq!(bus.subscribers(), 0);
    assert_eq!(sub.next().await.unwrap().unwrap().topic, "task.created");
    assert_eq!(sub.next().await, Err(BusError::Overflow));

    let sub = Arc::new(bus.subscribe(None, Bus::DEFAULT_MAXSIZE));
    let reader = tokio::spawn({
        let sub = sub.clone();
        async move { sub.next().await }
    });
    tokio::task::yield_now().await;
    sub.close();
    assert_eq!(reader.await.unwrap(), Ok(None));
    assert!(matches!(
        bus.publish("not.a.topic", None),
        Err(BusError::UnknownTopic(_))
    ));
}
