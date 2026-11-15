//! The planner's read-only lookups (supervisor.py `PLANNER_TOOLS`, `PLANNER_SCOPES`): one list for the supervisor,
//! which offers each one that is installed, and for the runtime check, which offers them all (sorted) in its planner
//! call. Here because both crates already depend on this one and neither may depend on the other.
//!
//! decision: ORCH-641, DMN-595

/// Python's six, then `about.diagnostics` (DMN-595: FERB's own diagnostics, a Rust addition), in Python's order.
pub const PLANNER_TOOLS: [&str; 7] = [
    "memory.read",
    "chat.history",
    "task.evidence",
    "trail.query",
    "trail.days",
    "about.read",
    "about.diagnostics",
];
pub const PLANNER_SCOPES: [&str; 2] = ["task:read", "trail:read"];
