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
  a worker's attempt ends with the reason (ORCH-307 decides what comes next)."""

from ferb.core.errors import FerbError

# RUN-445 (owner 2026-11-08: "ye swe can use thsi cause lcadue official docs say that acc ot you."): the follow-up after
# an empty answer, sent once per run as a new user message after the model's (empty) turn
FOLLOW_UP = "You ended without an answer. Give your final answer now."
# RUN-446 (owner 2026-11-08: "Yes, at 10 (Recommended)"): wrong calls per model run, every role
WRONG_CALLS_CAP = 10


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
