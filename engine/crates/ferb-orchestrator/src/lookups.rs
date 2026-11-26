//! The planner's read-only lookups (supervisor.py `PLANNER_TOOLS`, `PLANNER_SCOPES`): one list for the supervisor,
//! which offers each one that is installed, and for the runtime check, which offers them all (sorted) in its planner
//! call. Here because both crates already depend on this one and neither may depend on the other.
//!
//! decision: ORCH-641, DMN-595, ORCH-663, SENSE-422

/// Python's six, then `about.diagnostics` (DMN-595: FERB's own diagnostics, a Rust addition), in Python's order, then
/// `task.trace` (ORCH-663: a task's trace, the planner's alone), then `device.facts` (SENSE-422: this Mac's facts; the
/// lookup is observe-only, so its exact location is refused there and asked in a task).
pub const PLANNER_TOOLS: [&str; 9] = [
    "memory.read",
    "chat.history",
    "task.evidence",
    "trail.query",
    "trail.days",
    "about.read",
    "about.diagnostics",
    "task.trace",
    "device.facts",
];
pub const PLANNER_SCOPES: [&str; 2] = ["task:read", "trail:read"];

/// The front door's own tools (kb/plans/front-door.md, step 5): its hand-off, control, question and side-entry calls,
/// each taking the matching part of the planner's answer form and checked by the supervisor's own checks
/// (`Inner::front_call`). Offered beside the lookups only to the front door's grant; sorted.
///
/// decision: ORCH-735, ORCH-736
pub const FRONT_TOOLS: [&str; 6] = [
    "chat.ask",
    "chat.control_task",
    "chat.dispatch_task",
    "chat.remember",
    "chat.schedule",
    "chat.set_reminder",
];
/// The front door's write scope, beside [`PLANNER_SCOPES`]: its grant may call [`FRONT_TOOLS`].
pub const FRONT_SCOPES: [&str; 1] = ["chat:act"];
