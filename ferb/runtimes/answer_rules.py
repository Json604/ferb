"""Owns the rules every runtime and role shares for a model run that doesn't hand over an answer (RUN-445, RUN-446).
Runtime-neutral: no provider code; must not judge what an answer says.

- Empty answer: a model call that returns no answer text and no tool call (end_turn/STOP/completed with nothing in it,
  Anthropic's tool_use stop with no tool block, thinking only). An adapter that can continue its run asks once, in the
  same conversation, with FOLLOW_UP (Anthropic's documented fix for empty end_turn responses, measured 12/12 on the
  owner's failing planner turns); a second empty answer ends the run with an error marked `empty`, which the shared run
  (ferb/orchestrator/worker.py `_run_once`) raises as EmptyAnswer.
- Wrong call: a call to a tool the run wasn't offered (adapters report it as a `tool_call` event with `offered` False)
  or one FERB's gateway refused because the grant's calls were spent (the planner's lookups, ORCH-516). Lookups and
  tools that ran never count. The shared run stops a run at WRONG_CALLS_CAP of them with ToolLoop.
- The planner turns EmptyAnswer and ToolLoop into one last call with no tools and what its lookups found (ORCH-557);
  a worker's attempt ends with the reason (ORCH-307 decides what comes next).
- The answer tool (RUN-447): where the planner's typed format isn't used (the provider refused it, or it is over the
  counted limits), the direct-API loop offers ANSWER_TOOL, a non-strict tool whose input schema is the core answer
  schema, and states PLANNER_ANSWER in place of the schema. A call to it is the answer; it is never a wrong call and
  never reaches FERB's gateway. A text answer is still accepted."""

from ferb.core.errors import FerbError

# RUN-445 (owner 2026-11-08: "ye swe can use thsi cause lcadue official docs say that acc ot you."): the follow-up after
# an empty answer, sent once per run as a new user message after the model's (empty) turn
FOLLOW_UP = "You ended without an answer. Give your final answer now."
# RUN-446 (owner 2026-11-08: "Yes, at 10 (Recommended)"): wrong calls per model run, every role
WRONG_CALLS_CAP = 10
# RUN-447: the answer tool's name and description (PRM-337, as measured on 2026-11-08: 0 of 135 first calls empty)
ANSWER_TOOL = "ferb_answer"
ANSWER_TOOL_DESC = ("Give FERB your final answer for this message: call this once, when you have decided, with the whole "
                    "answer as its input (one object matching this input schema). Include every property; use null "
                    "where the schema allows it for a value that doesn't apply. FERB acts on this answer; nothing you "
                    "call in the same reply runs.")
# PRM-336 (owner 2026-11-09: "Yes, build with this wording (Recommended)"): the planner's answer form where its typed
# format isn't used, in place of the stated schema, beside ANSWER_TOOL. PRM-350 (owner ORCH-618, 2026-11-11: "i like
# the id part, why isnt the planner depending on taht id then???"): a task names an earlier result by its id instead
# of the planner reading it into the goal
PLANNER_ANSWER = ("FERB's answer format (required): your lookup tools are for you, not for a task. When the owner asks "
                  "about something from before (a detail of an earlier answer, what an earlier task found or did, "
                  "something they told you, memory), call the lookups yourself first and answer from what they "
                  "return: a short-form result's full text is one chat.history call with its entry_id, an earlier "
                  "task's records are task.evidence with its task_id. Never make a task just to read FERB's own "
                  "records; when a task needs something from an earlier result (a file it made, the values to send), "
                  "name that result's Result id in the task's earlier_results instead of reading it into the goal: "
                  "FERB gives the worker its full text. Then give your final answer by calling the ferb_answer tool "
                  "with the whole answer (its input schema is the answer format), or reply with only that JSON "
                  "object, nothing before or after it.")
# RUN-447: the result other calls get when they came in the same reply as the answer, and the answer's when an owner
# correction arrived before it was taken
NOT_RUN_WITH_ANSWER = "Not run: your answer came in the same reply."
ANSWER_NOT_TAKEN = "Not taken: the owner updated the request; answer again using the new instructions."


def is_empty(calls: object, final: str | None) -> bool:
    """A model call that handed over nothing: no tool call and no answer text (whitespace is no text)."""
    return not calls and not (final or "").strip()


class AnswerMissing(FerbError):
    """A model run that ended without an answer by one of the rules above; `runtime` is the runtime's name."""

    def __init__(self, message: str, runtime: str = "") -> None:
        super().__init__(message)
        self.runtime = runtime


class EmptyAnswer(AnswerMissing):
    """The model ended without an answer twice in one run (once where its adapter can't continue the run)."""


class ToolLoop(AnswerMissing):
    """The run reached WRONG_CALLS_CAP wrong calls and FERB stopped it. `count`: how many it made."""

    def __init__(self, runtime: str, count: int) -> None:
        super().__init__(f"the AI service ({runtime}) kept calling tools it wasn't given, or lookups after its limit "
                         f"({count} such calls), so FERB stopped this call", runtime)
        self.count = count
