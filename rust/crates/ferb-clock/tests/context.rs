//! (e) The clock text given to models (`ferb/clock/context.py`) against `rust/fixtures/clock/context.json`.

mod common;

use common::{fixture, opt_time, time, Diffs};
use ferb_clock::context::{footer, header, time_block, Facts};
use serde_json::json;

#[test]
fn the_time_facts_read_as_python_writes_them() {
    let f = fixture("context.json");
    let mut d = Diffs::default();
    for case in f.as_array().unwrap() {
        let now = time(&case["now"]);
        let name = case["now_name"].as_str().unwrap_or("");
        let started = opt_time(&case["task_started"]);
        let deadline = opt_time(&case["deadline"]);
        let active = opt_time(&case["owner_last_active"]);
        let trail = opt_time(&case["last_trail_event"]);
        let ages: Vec<(String, _)> = case["ages"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), time(v)))
            .collect();
        let since: Option<Vec<String>> = case["since_last_talk"]
            .as_array()
            .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect());
        let what = format!("{} at {}", case["zone"], case["now"]);
        d.check(
            &format!("header {what}"),
            &case["header"],
            &json!(header(
                &now,
                name,
                started.as_ref(),
                deadline.as_ref(),
                active.as_ref(),
                trail.as_ref()
            )),
        );
        d.check(
            &format!("footer {what}"),
            &case["footer"],
            &json!(footer(&now, name, started.as_ref(), deadline.as_ref())),
        );
        let facts = Facts {
            task_started: started.as_ref(),
            deadline: deadline.as_ref(),
            owner_last_active: active.as_ref(),
            last_trail_event: trail.as_ref(),
            ages: &ages,
            since_last_talk: since.as_deref(),
        };
        d.check(
            &format!("block {what}"),
            &case["block"],
            &json!(time_block(&now, name, &facts)),
        );
    }
    d.finish("context");
}
