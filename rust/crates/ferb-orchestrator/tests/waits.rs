//! ORCH-642: what a plan's wait step can wait for is FERB's own table, and the planner's instructions name exactly it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ferb_orchestrator::models::{EventMatch, Plan, Step};
use ferb_orchestrator::waits;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if path.is_dir() {
            if name != "target" && name != "tests" {
                rust_sources(&path, out);
            }
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

// Every topic the Rust engine publishes by name is on the bus (so the table above covers what can reach a plan): a
// `publish("…"` or `(…publish)("…"` with a literal topic, anywhere in the crates' sources.
// decision: ORCH-642
#[test]
fn every_topic_the_engine_publishes_is_on_the_bus() {
    let bus: BTreeSet<&str> = ferb_core::bus::CRITICAL
        .iter()
        .chain(ferb_core::bus::TELEMETRY)
        .copied()
        .collect();
    let mut files = Vec::new();
    rust_sources(&repo().join("rust/crates"), &mut files);
    let literal = regex::Regex::new(r#"publish\w*\)?\(\s*"([a-z_]+(?:\.[a-z_]+)+)""#).unwrap();
    let mut seen = BTreeSet::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        for c in literal.captures_iter(&text) {
            let topic = c[1].to_string();
            assert!(
                bus.contains(topic.as_str()),
                "{} publishes {topic}, which isn't on the bus",
                file.display()
            );
            seen.insert(topic);
        }
    }
    assert!(
        seen.contains(waits::TASK_FINISHED),
        "the scan found task.finished: {seen:?}"
    );
    assert!(seen.len() >= 30, "the scan found the publishers: {seen:?}");
}

fn wait(id: &str, event: &str, matched: &[(&str, &str)]) -> Step {
    let mut s = Step::new(id, "wait");
    s.event = Some(event.into());
    s.event_match = matched
        .iter()
        .map(|(f, v)| EventMatch {
            field: (*f).into(),
            value: (*v).into(),
        })
        .collect();
    s
}

fn plan(steps: Vec<Step>) -> Plan {
    Plan::of(steps)
}

// The live case (task 15737cce, 2026-11-14): the planner invented `task_completed`; refused with the names.
// decision: ORCH-642
#[test]
fn a_wait_on_an_event_ferb_never_publishes_is_refused_with_the_names() {
    let p = plan(vec![wait(
        "wait_research",
        "task_completed",
        &[("task_id", "153f1603811f42dcb57fe90de1d620d9")],
    )]);
    let problems = waits::problems(&p);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0]
        .starts_with("wait_research: FERB never publishes an event named 'task_completed'"));
    assert!(problems[0].contains("task.finished (task_id, status:"));
    assert!(problems[0].contains("timer.fired"));
    // the right shape passes
    let ok = plan(vec![wait(
        "wait_research",
        "task.finished",
        &[("task_id", "153f1603811f42dcb57fe90de1d620d9")],
    )]);
    assert!(waits::problems(&ok).is_empty());
    assert_eq!(
        waits::awaited_task(&ok.steps[0]),
        Some("153f1603811f42dcb57fe90de1d620d9")
    );
    // a field the event doesn't carry could never match
    let bad_field = plan(vec![wait("w", "task.finished", &[("task", "x")])]);
    assert_eq!(
        waits::problems(&bad_field),
        vec!["w: task.finished carries only the fields task_id, status; event_match names task, so this wait \
              would never end."
            .to_string()]
    );
    // a time wait and a step with no event are not this check's
    let mut timed = Step::new("t", "wait");
    timed.until = Some("2026-11-15T09:00:00+05:30".into());
    assert!(waits::problems(&plan(vec![timed, Step::new("d", "do")])).is_empty());
}
