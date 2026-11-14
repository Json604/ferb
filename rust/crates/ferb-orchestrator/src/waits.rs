//! What a plan's wait step (or a loop's repeat) can wait for (ORCH-642): the events FERB publishes that a plan can
//! use, each with the top-level fields its `event_match` can name. A wait on any other name would never end (FERB
//! stores an event for a plan only when a running plan waits on its topic, and nothing publishes an unknown one), so
//! code refuses it at admission and tells the planner the names (rule 3: argument validity). Every topic on FERB's bus
//! (`ferb_core::bus`) is either here or in [`NOT_WAITED`]; a guard test fails when one is added to neither.

use crate::models::{Plan, Step};

/// One event a plan can wait for.
pub struct WaitEvent {
    /// The bus topic.
    pub topic: &'static str,
    /// The payload's top-level fields an `event_match` can name.
    pub fields: &'static [&'static str],
    /// What it means, in the planner's words.
    pub means: &'static str,
}

/// The event a task tree's end publishes (`task.finished`, once per tree, ORCH-323).
pub const TASK_FINISHED: &str = "task.finished";

/// The events a plan can wait for.
pub const WAIT_EVENTS: &[WaitEvent] = &[
    WaitEvent {
        topic: TASK_FINISHED,
        fields: &["task_id", "status"],
        means:
            "a task ended (status done, done_unverified, failed, cancelled, blocked or needs_user)",
    },
    WaitEvent {
        topic: "timer.fired",
        fields: &["timer_id", "kind", "name"],
        means: "one of the owner's reminders or timers went off",
    },
];

/// The bus topics a plan never waits for: FERB's own telemetry, its questions and records, a client's state. Each is
/// named here on purpose, so a topic added to the bus is a decision about whether a plan can wait on it.
pub const NOT_WAITED: &[&str] = &[
    "task.created",
    "task.started",
    "task.retried",
    "task.steered",
    "approval.requested",
    "approval.resolved",
    "device.changed",
    "browser.profile_paired",
    "memory.committed",
    "timer.changed",
    "runtimes.changed",
    "memory.retired",
    "memory.expires",
    "memory.set_aside",
    "memory.restored",
    "stop.engaged",
    "stop.lifted",
    "sense.paused",
    "sense.resumed",
    "sense.watcher_failed",
    "browser.debugger_detached",
    "browser.owner_stopped",
    "browser.owner_took",
    "task.result",
    "task.resumed_event",
    "conversation.paused",
    "attention.notice",
    "task.paused",
    "task.resumed",
    "loop.status",
    "memory.confirmed_derived",
    "memory.forgot_derived",
    "memory.rollback",
    "memory.forgot",
    "task.part",
    "tool.called",
    "tool.finished",
    "trail.event",
    "action.observed",
    "browser.connected",
    "browser.disconnected",
    "browser.profiles_changed",
    "memory.proposed",
    "secret.refused",
    "browser.dialog",
    "browser.file_chooser",
    "browser.download",
    "browser.cleanup_failed",
    "browser.connection_refused",
    "disturbance.observed",
    "memory.derived",
    "memory.digest",
    "memory.embedding_unavailable",
    "perception.attention",
    "perception.batch",
    "security.alignment",
    "memory.import.progress",
    "task.step",
    "task.progress",
    "cursor.point",
];

/// The event named `topic`, when a plan can wait for it.
pub fn wait_event(topic: &str) -> Option<&'static WaitEvent> {
    WAIT_EVENTS.iter().find(|e| e.topic == topic)
}

/// The names and fields as the planner reads them: `task.finished (task_id, status: a task ended …); …`.
pub fn listed() -> String {
    WAIT_EVENTS
        .iter()
        .map(|e| format!("{} ({}: {})", e.topic, e.fields.join(", "), e.means))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The event a step waits on, when it names one (`""` counts as none, ORCH-330).
pub fn waited(step: &Step) -> Option<&str> {
    step.wakes_on().filter(|e| !e.is_empty())
}

/// What code finds wrong with a plan's waits, told back to the planner for repair: an event FERB never publishes, or
/// an `event_match` field its payload doesn't carry (an unrelated event could never match it, so it never ends).
pub fn problems(plan: &Plan) -> Vec<String> {
    let mut out = Vec::new();
    for step in &plan.steps {
        let Some(topic) = waited(step) else { continue };
        match wait_event(topic) {
            None => out.push(format!(
                "{}: FERB never publishes an event named {}, so this wait would never end. A wait's event is one of: \
                 {}. To wait for another task, use {TASK_FINISHED} with event_match [{{\"field\": \"task_id\", \
                 \"value\": <its task id>}}]; an earlier result already shown needs no wait (earlier_results).",
                step.id,
                crate::py::repr_str(topic),
                listed()
            )),
            Some(event) => {
                let unknown: Vec<&str> = step
                    .wakes_match()
                    .iter()
                    .map(|m| m.field.as_str())
                    .filter(|f| !event.fields.contains(f))
                    .collect();
                if !unknown.is_empty() {
                    out.push(format!(
                        "{}: {} carries only the fields {}; event_match names {}, so this wait would never end.",
                        step.id,
                        event.topic,
                        event.fields.join(", "),
                        unknown.join(", ")
                    ));
                }
            }
        }
    }
    out
}

/// The task a wait step waits to finish: its `task.finished` event's `task_id` match.
pub fn awaited_task(step: &Step) -> Option<&str> {
    if waited(step) != Some(TASK_FINISHED) {
        return None;
    }
    step.wakes_match()
        .iter()
        .find(|m| m.field == "task_id" && !m.value.is_empty())
        .map(|m| m.value.as_str())
}

/// Whether a finished task's status lets the steps after a wait on it go on: it finished its work.
pub fn finished_well(status: &str) -> bool {
    crate::plans::DONE.contains(&status)
}

/// The wait step's line while it waits on another task (ORCH-644): "Waiting for “Laptop research” to finish".
pub fn waiting_for_task(name: &str) -> String {
    format!("Waiting for \u{201c}{name}\u{201d} to finish")
}

/// What can be done about a step that waits (ORCH-644), beside it in the planner's task status facts.
pub const WAIT_CONTROLS: &str = "control continue on this task makes the step look again now and go on at once if \
     what it waits for has happened (a task's end, its event, its time); control stop ends the task";

/// What a wait on another task ended with, for the steps after it (ORCH-643): the task's end as FERB recorded it and
/// where its answer is read (never its content: the step that uses it reads it through the gateway, origins kept).
pub fn task_end_output(task_id: &str, name: &str, status: &str) -> serde_json::Value {
    if finished_well(status) {
        serde_json::json!({
            "status": "done",
            "summary": format!(
                "\u{201c}{name}\u{201d} finished ({status}). Its answer: chat_history task_id={task_id}."
            ),
            "event": {"task_id": task_id, "status": status},
        })
    } else {
        serde_json::json!({
            "status": "failed",
            "summary": format!(
                "\u{201c}{name}\u{201d}, the task this step waited for, ended {status}, so the steps that needed it \
                 won't run."
            ),
            "event": {"task_id": task_id, "status": status},
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    // decision: ORCH-642
    #[test]
    fn every_bus_topic_is_waitable_or_named_as_not() {
        let bus: BTreeSet<&str> = ferb_core::bus::CRITICAL
            .iter()
            .chain(ferb_core::bus::TELEMETRY)
            .copied()
            .collect();
        let mut here: BTreeSet<&str> = NOT_WAITED.iter().copied().collect();
        for e in WAIT_EVENTS {
            assert!(here.insert(e.topic), "{} is in both lists", e.topic);
        }
        assert_eq!(
            here.len(),
            WAIT_EVENTS.len() + NOT_WAITED.len(),
            "a topic is listed twice"
        );
        assert_eq!(
            bus, here,
            "a bus topic was added or removed: decide whether a plan can wait on it (WAIT_EVENTS or NOT_WAITED)"
        );
    }
}
