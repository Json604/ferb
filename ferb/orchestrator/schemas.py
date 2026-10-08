"""Owns structured approach outputs and mechanical validation. Must not bypass the gateway or turn model uncertainty into an implicit target choice.

Typed outcomes the supervisor and workers must return.

JSON schemas in strict structured-output form (every property required, no extra properties, null where optional),
plus pydantic models that validate what comes back. A chat turn is a reply, one clarifying question, or a task: the
goal in plain words, the identity, the targets the model understood (informational), and success conditions."""

from typing import Any, Literal

from pydantic import BaseModel, model_validator

from ferb.clock.when import WHEN_SCHEMA, When
from ferb.policy.targets import Targets

CHECK_KINDS = ["none", "file_exists", "file_contains", "ledger_done", "tab_url", "account", "after_change", "after_value"]


def _strings(enum: list[str] | None = None) -> dict[str, Any]:
    item: dict[str, Any] = {"type": "string"}
    if enum:
        item["enum"] = enum
    return {"type": "array", "items": item}


def _nullable(t: str) -> dict[str, Any]:
    return {"type": [t, "null"]}


def _obj(props: dict[str, Any]) -> dict[str, Any]:
    return {"type": "object", "additionalProperties": False, "required": list(props), "properties": props}


TARGETS_SCHEMA = _obj({"apps": _strings(), "sites": _strings(), "paths": _strings(), "recipients": _strings()})
CONDITION_SCHEMA = _obj({
    "text": {"type": "string"},
    "layer": {"type": "string", "enum": ["outcome", "intake", "artifact", "external"]},
    # a code-checkable form when the condition allows one (verified without a model); kind "none" otherwise
    "check": _obj({"kind": {"type": "string", "enum": CHECK_KINDS}, "path": _nullable("string"),
                   "text": _nullable("string"), "url_contains": _nullable("string"), "site": _nullable("string"),
                   "expected": _nullable("string"), "tool": _nullable("string")}),
})
TASK_SCHEMA: dict[str, Any] = _obj({
    "goal": {"type": "string"},
    "identity": {"type": "string", "enum": ["owner", "ferb"]},
    "worker_type": {"type": "string"},
    "targets": TARGETS_SCHEMA,
    "success_conditions": {"type": "array", "items": CONDITION_SCHEMA, "minItems": 1},
})

REMEMBER_SCHEMA = _obj({
    "kind": {"type": "string", "enum": ["fact", "preference", "person", "project", "env"]},
    "subject": {"type": "string"}, "key": {"type": "string"}, "content": {"type": "string"},
    "quote": {"type": "string"},
})
TIMER_SCHEMA = _obj({"when": WHEN_SCHEMA, "message": {"type": "string"}, "quote": {"type": "string"}})
SCHEDULE_SCHEMA = _obj({
    "request": {"type": "string"}, "when": WHEN_SCHEMA,
    "repeat": {"type": "string", "enum": ["none", "daily", "weekdays", "weekly", "every"]},
    "weekday": _nullable("string"), "every_minutes": _nullable("integer"), "quote": {"type": "string"},
    "targets": TARGETS_SCHEMA,
})
SCHEDULE_CHANGE_SCHEMA = _obj({"id": {"type": "string"},
                               "action": {"type": "string", "enum": ["cancel", "pause", "resume"]},
                               "quote": {"type": "string"}})
REMINDER_CHANGE_SCHEMA = _obj({"id": {"type": "string"}, "quote": {"type": "string"}})
SUPERVISOR_SCHEMA: dict[str, Any] = _obj({
    "kind": {"type": "string", "enum": ["reply", "clarify", "task"]},
    "text": {"type": "string"},
    "task": {"anyOf": [{"type": "null"}, TASK_SCHEMA]},
    "control": {"anyOf": [{"type": "null"}, _obj({"task_id": {"type": "string"},
                  "action": {"type": "string", "enum": ["stop", "retry", "continue"]}})]},
    "remember": {"type": "array", "items": REMEMBER_SCHEMA},
    "timers": {"type": "array", "items": TIMER_SCHEMA},
    "schedule": {"anyOf": [{"type": "null"}, SCHEDULE_SCHEMA]},
    "schedule_changes": {"type": "array", "items": SCHEDULE_CHANGE_SCHEMA},
    "reminder_cancels": {"type": "array", "items": REMINDER_CHANGE_SCHEMA},
})

WORKER_SCHEMA: dict[str, Any] = _obj({
    "status": {"type": "string", "enum": ["done", "failed", "needs_user", "blocked"]},
    "summary": {"type": "string"}, "artifacts": _strings(), "evidence": _strings(),
})

DISPATCH_SCHEMA = _obj({
    "action": {"type": "string", "enum": ["retry", "ask", "finish"]},
    "worker_type": {"type": "string"},
    "instructions": {"type": "string"},
    "question": {"type": "string"},
    "stop_reason": {"type": "string"},
    # finish only (null otherwise): the orchestrator's judgment of finished work a check couldn't confirm (V4)
    "result": {"anyOf": [{"type": "null"}, {"type": "string", "enum": ["unconfirmed", "failed"]}]},
})

QUESTION_SCHEMA = _obj({
    "ask_owner": {"type": "boolean"},
    "answer": {"type": "string"},
    "question": {"type": "string"},
})

VERIFIER_SCHEMA: dict[str, Any] = _obj({
    "results": {"type": "array", "items": _obj({"index": {"type": "integer"}, "met": {"type": ["boolean", "null"]},
                                                "evidence": {"type": "string"}})},
})

class TargetsSpec(BaseModel):
    apps: list[str] = []
    sites: list[str] = []
    paths: list[str] = []
    recipients: list[str] = []

    def targets(self, paths: list[str] | None = None) -> Targets:
        return Targets.of(self.apps, self.sites, self.paths if paths is None else paths, self.recipients)


class Check(BaseModel):
    kind: Literal["none", "file_exists", "file_contains", "ledger_done", "tab_url", "account",
                  "after_change", "after_value"]
    path: str | None
    text: str | None
    url_contains: str | None
    site: str | None
    expected: str | None
    tool: str | None = None        # after_change / after_value: the tool whose latest call is read (default: any acting call)


class Condition(BaseModel):
    text: str
    check: Check
    layer: Literal["outcome", "intake", "artifact", "external"] = "outcome"


class TaskSpec(BaseModel):
    goal: str
    identity: Literal["owner", "ferb"]
    worker_type: str = 'default'
    targets: TargetsSpec = TargetsSpec()
    success_conditions: list[Condition] = []


class Remember(BaseModel):
    """Something the owner stated about themselves or asked FERB to remember, with the exact words it rests on."""
    kind: Literal["fact", "preference", "person", "project", "env"]
    subject: str
    key: str
    content: str
    quote: str


class TaskControl(BaseModel):
    task_id: str
    action: Literal["stop", "retry", "continue"]


class SupervisorOutcome(BaseModel):
    control: TaskControl | None = None
    kind: Literal["reply", "clarify", "task"]
    text: str
    task: TaskSpec | None = None
    remember: list[Remember] = []
    timers: list["TimerRequest"] = []
    schedule: "ScheduleRequest | None" = None
    schedule_changes: list["ScheduleChange"] = []
    reminder_cancels: list["ReminderCancel"] = []

    @model_validator(mode="after")
    def complete_action_plan(self) -> "SupervisorOutcome":
        if self.kind == "task" and (self.task is None or not self.task.success_conditions or
                                    any(not c.text.strip() for c in self.task.success_conditions)):
            raise ValueError("A new task needs nonempty success_conditions describing the requested outcome")
        return self


class ScheduleRequest(BaseModel):
    """A task to run later or repeatedly: in the owner's words, with the targets recorded now."""
    request: str
    when: When                   # the first run, as the owner said it; code computes the time
    repeat: Literal["none", "daily", "weekdays", "weekly", "every"]
    weekday: str | None = None
    every_minutes: int | None = None
    quote: str
    targets: TargetsSpec = TargetsSpec()


class ScheduleChange(BaseModel):
    id: str
    action: Literal["cancel", "pause", "resume"]
    quote: str


class ReminderCancel(BaseModel):
    """Cancel one of the owner's pending reminders (e.g. FERB got the date wrong), with the owner's words."""
    id: str
    quote: str


class TimerRequest(BaseModel):
    """A reminder the owner asked for: when (as the owner said it; code computes the time), what to say, the owner's
    exact words."""
    when: When
    message: str
    quote: str


SupervisorOutcome.model_rebuild()


class WorkerOutcome(BaseModel):
    status: Literal["done", "failed", "needs_user", "blocked"]
    summary: str
    artifacts: list[str]
    evidence: list[str]


class DispatchOutcome(BaseModel):
    stop_reason: str = ""
    action: Literal["retry", "ask", "finish"]
    # finish only. "unconfirmed": the work looks done and only confirmation is missing; code reports done but not
    # confirmed when the worker reported done and the check found nothing false. Anything else, or absent, is failed.
    result: Literal["unconfirmed", "failed"] | None = None
    worker_type: str
    instructions: str
    question: str

    @model_validator(mode="after")
    def require_branch_content(self) -> "DispatchOutcome":
        if self.action == "ask" and not self.question.strip():
            raise ValueError("action=ask requires question")
        if self.action in ("retry", "finish") and not self.instructions.strip():
            raise ValueError(f"action={self.action} requires instructions; question is not its result field")
        return self


class QuestionOutcome(BaseModel):
    ask_owner: bool
    answer: str
    question: str


class VerifierResult(BaseModel):
    index: int
    met: bool | None
    evidence: str


class VerifierOutcome(BaseModel):
    results: list[VerifierResult]
