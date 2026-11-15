//! The rules every runtime and role shares for a model run that doesn't hand over an answer (answer_rules.py;
//! RUN-445, RUN-446, RUN-447). Runtime-neutral: no provider code; never judges what an answer says.

/// RUN-445 (owner 2026-11-08): the follow-up after an empty answer, sent once per run.
pub const FOLLOW_UP: &str = "You ended without an answer. Give your final answer now.";
/// RUN-446 (owner 2026-11-08): wrong calls per model run, every role.
pub const WRONG_CALLS_CAP: u32 = 10;
/// RUN-447: the answer tool's name and description (PRM-337).
pub const ANSWER_TOOL: &str = "ferb_answer";
pub const ANSWER_TOOL_DESC: &str = "Give FERB your final answer for this message: call this once, when you have decided, with the whole answer as its input (one object matching this input schema). Include every property; use null where the schema allows it for a value that doesn't apply. FERB acts on this answer; nothing you call in the same reply runs.";
/// PRM-336 / PRM-350: the planner's answer form where its typed format isn't used.
pub const PLANNER_ANSWER: &str = "FERB's answer format (required): your lookup tools are for you, not for a task. When the owner asks about something from before (a detail of an earlier answer, what an earlier task found or did, something they told you, memory), call the lookups yourself first and answer from what they return: a short-form result's full text is one chat.history call with its entry_id, an earlier task's records are task.evidence with its task_id. Never make a task just to read FERB's own records; when a task needs something from an earlier result (a file it made, the values to send), name that result's Result id in the task's earlier_results instead of reading it into the goal: FERB gives the worker its full text. Then give your final answer by calling the ferb_answer tool with the whole answer (its input schema is the answer format), or reply with only that JSON object, nothing before or after it.";
/// RUN-447: the result other calls get when they came in the same reply as the answer.
pub const NOT_RUN_WITH_ANSWER: &str = "Not run: your answer came in the same reply.";
pub const ANSWER_NOT_TAKEN: &str =
    "Not taken: the owner updated the request; answer again using the new instructions.";

/// answer_rules.is_empty: no tool call and no answer text (whitespace is no text).
pub fn is_empty(calls: usize, final_text: Option<&str>) -> bool {
    calls == 0 && ferb_py::value::strip(final_text.unwrap_or_default()).is_empty()
}

/// answer_rules.ToolLoop's words: the run reached WRONG_CALLS_CAP wrong calls and FERB stopped it.
pub fn tool_loop_message(runtime: &str, count: u32) -> String {
    format!(
        "the AI service ({runtime}) kept calling tools it wasn't given, or lookups after its limit ({count} such calls), so FERB stopped this call"
    )
}
