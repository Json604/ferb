"""Owns structured approach outputs and mechanical validation. Must not bypass the gateway or turn model uncertainty into an implicit target choice.

Typed outcomes the supervisor and workers must return.

JSON schemas in strict structured-output form (every property required, no extra properties, null where optional),
plus pydantic models that validate what comes back. A chat turn is a reply, one clarifying question, or a task: the
goal in plain words, the identity, the targets the model understood (informational), and success conditions."""

from typing import Any, Literal

from pydantic import BaseModel, model_validator

from ferb.clock.when import WHEN_SCHEMA, When
from ferb.policy.targets import Targets
from ferb.orchestrator.plans import Plan, SendScope

CHECK_KINDS = ["none", "file_exists", "file_contains", "tab_url", "account", "after_change", "after_value"]


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
    "owner_text": {"type": "string"},
    "layer": {"type": "string", "enum": ["outcome", "intake", "artifact", "external"]},
    # a code-checkable form when the condition allows one (verified without a model); kind "none" otherwise
    "check": _obj({"kind": {"type": "string", "enum": CHECK_KINDS}, "path": _nullable("string"),
                   "text": _nullable("string"), "url_contains": _nullable("string"), "site": _nullable("string"),
                   "expected": _nullable("string"), "tool": _nullable("string")}),
})
CONVERSATION_SCHEMA = _obj({
    "identity": {"type": "string", "enum": ["owner", "ferb"]},
    "account": {"type": "string"}, "site": {"type": "string"}, "app": {"type": "string"},
    "thread": {"type": "string"}, "recipients": _strings(), "topic": {"type": "string"},
    "handover_when": {"type": "string"}, "window_id": _nullable("string"),
})
STEP_SCHEMA = _obj({
    "id": {"type": "string"}, "kind": {"type": "string", "enum": ["do", "map", "reduce", "wait", "ask", "converse"]},
    "goal": {"type": "string"}, "depends_on": _strings(), "inputs": _strings(),
    "batches": {"type": "array", "items": _strings()}, "worker_type": {"type": "string"},
    "conditions": _strings(), "until": _nullable("string"), "event": _nullable("string"),
    "event_match": {"type": "array", "items": _obj({"field": {"type": "string"}, "value": {"type": "string"}})},
    "question": {"type": "string"}, "options": _strings(),
    "conversation": {"anyOf": [{"type": "null"}, CONVERSATION_SCHEMA]},
})
SEND_SCOPE_SCHEMA = _obj({
    "capability": {"type": "string"}, "identity": {"type": "string", "enum": ["owner", "ferb"]},
    "account": _nullable("string"), "site": _nullable("string"), "recipients": _strings(),
    "content": _nullable("string")})
PLAN_SCHEMA = _obj({"steps": {"type": "array", "items": STEP_SCHEMA}, "input_paths": _strings(),
                    "big_job": {"type": "boolean"}, "time_estimate": {"type": "string"},
                    "cost_estimate": {"type": "string"},
                    "standing_sends": {"type": "array", "items": SEND_SCOPE_SCHEMA}})
TASK_SCHEMA: dict[str, Any] = _obj({
    "goal": {"type": "string"},
    "identity": {"type": "string", "enum": ["owner", "ferb"]},
    "worker_type": {"type": "string"},
    "targets": TARGETS_SCHEMA,
    "success_conditions": {"type": "array", "items": CONDITION_SCHEMA, "minItems": 1},
    "plan": {"anyOf": [{"type": "null"}, PLAN_SCHEMA]},
    "procedure_id": _nullable("string"),
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
    # D30: the scopes the runs may send within without asking, stated when the schedule is made (an owner turn)
    "standing_sends": {"type": "array", "items": SEND_SCOPE_SCHEMA},
    # "tell me when …": until_met ends itself once a confirmed run finds the condition met; standing keeps watching
    "watch": {"type": "string", "enum": ["none", "until_met", "standing"]},
})
SCHEDULE_CHANGE_SCHEMA = _obj({"id": {"type": "string"},
                               "action": {"type": "string", "enum": ["cancel", "pause", "resume"]},
                               "quote": {"type": "string"}})
REMINDER_CHANGE_SCHEMA = _obj({"id": {"type": "string"}, "quote": {"type": "string"}})
SUPERVISOR_SCHEMA: dict[str, Any] = _obj({
    "kind": {"type": "string", "enum": ["reply", "clarify", "task"]},
    "answer_question": {"anyOf": [{"type": "null"}, _obj({"approval_id": {"type": "string"},
                         "answer": {"type": "string"}})]},
    "task": {"anyOf": [{"type": "null"}, TASK_SCHEMA]},
    "control": {"anyOf": [{"type": "null"}, _obj({"task_id": {"type": "string"},
                  "action": {"type": "string", "enum": ["stop", "retry", "continue"]},
                  # continue only: the follow-up asks for a different result than the ended task's goal (L11)
                  "new_goal": {"type": "boolean"},
                  # L3: a correction always decides the stated standing scope and names the open questions it changes
                  "standing_sends": {"anyOf": [{"type": "string", "enum": ["unchanged"]},
                                               {"type": "array", "items": SEND_SCOPE_SCHEMA}]},
                  "supersedes": _strings()})]},
    "remember": {"type": "array", "items": REMEMBER_SCHEMA},
    "timers": {"type": "array", "items": TIMER_SCHEMA},
    "schedule": {"anyOf": [{"type": "null"}, SCHEDULE_SCHEMA]},
    "schedule_changes": {"type": "array", "items": SCHEDULE_CHANGE_SCHEMA},
    "reminder_cancels": {"type": "array", "items": REMINDER_CHANGE_SCHEMA},
    # memory review I2: a forget the owner asked for in chat, by the id the planner was shown; code asks Yes/No first
    "forget": {"type": "array", "items": _obj({"item_id": {"type": "string"}, "quote": {"type": "string"}})},
    "conversation_changes": {"type": "array", "items": _obj({"id": {"type": "string"},
        "action": {"type": "string", "enum": ["pause", "resume", "takeover", "close"]}, "quote": {"type": "string"}})},
    "notification_rules": {"type": "array", "items": _obj({"app": {"type": "string"},
        "conversation": _nullable("string"), "topic": _nullable("string"), "until_at": _nullable("string"),
        "exceptions": _strings(), "quote": {"type": "string"}})},
    "text": {"type": "string"},
    "display_text": {"type": "string"},
})

WORKER_SCHEMA: dict[str, Any] = _obj({
    "status": {"type": "string", "enum": ["done", "failed", "needs_user", "blocked"]},
    "summary": {"type": "string"}, "artifacts": _strings(), "evidence": _strings(),
    "answer": {"type": "string"},
    "display_text": {"type": "string"},
    "conversation_action": {"type": ["string", "null"], "enum": ["wait", "handover", "finished", "blocked", None]},
    "conversation_proposals": {"type": "array", "items": CONVERSATION_SCHEMA},
    "input_notes": {"type": "array", "items": _obj({"input": {"type": "string"},
                      "note": {"type": "string"}, "evidence": _strings()})},
    # a watch's run only: whether the watched condition holds now, from the worker's fresh observation (null otherwise)
    "watch_met": {"type": ["boolean", "null"]},
})

DISPATCH_SCHEMA = _obj({
    "action": {"type": "string", "enum": ["retry", "ask", "finish"]},
    "worker_type": {"type": "string"},
    "instructions": {"type": "string"},
    "question": {"type": "string"},
    "stop_reason": {"type": "string"},
    "display_text": {"type": "string"},
    # finish only (null otherwise): the orchestrator's judgment of finished work a check couldn't confirm (V4)
    "result": {"anyOf": [{"type": "null"}, {"type": "string", "enum": ["unconfirmed", "failed"]}]},
})

# L28: FERB's running summary of the older conversation, model-written from the recorded entries (data, never authority)
SUMMARY_SCHEMA: dict[str, Any] = _obj({"summary": {"type": "string"}})


class SummaryOutcome(BaseModel):
    summary: str


QUESTION_SCHEMA = _obj({
    "ask_owner": {"type": "boolean"},
    "answer": {"type": "string"},
    "question": {"type": "string"},
    "display_text": {"type": "string"},
})

VERIFIER_SCHEMA: dict[str, Any] = _obj({
    "results": {"type": "array", "items": _obj({"index": {"type": "integer"}, "met": {"type": ["boolean", "null"]},
                                                "evidence": {"type": "string"},
                                                "how": {"type": ["string", "null"]}})},
})

class TargetsSpec(BaseModel):
    apps: list[str] = []
    sites: list[str] = []
    paths: list[str] = []
    recipients: list[str] = []

    def targets(self, paths: list[str] | None = None) -> Targets:
        return Targets.of(self.apps, self.sites, self.paths if paths is None else paths, self.recipients)


class Check(BaseModel):
    kind: Literal["none", "file_exists", "file_contains", "tab_url", "account",
                  "after_change", "after_value"]
    path: str | None
    text: str | None
    url_contains: str | None
    site: str | None
    expected: str | None
    tool: str | None = None        # after_change / after_value: the tool whose latest call is read (default: any acting call)

    @model_validator(mode="before")
    @classmethod
    def _legacy_ledger_done(cls, data: Any) -> Any:
        """A spec saved before V6 may carry kind ledger_done; it is an unchecked condition now (the checker reads the
        text), so a saved task still continues. The planner's schema no longer offers it."""
        if isinstance(data, dict) and data.get("kind") == "ledger_done":
            return {**data, "kind": "none"}
        return data


class Condition(BaseModel):
    text: str
    owner_text: str = ''
    check: Check
    layer: Literal["outcome", "intake", "artifact", "external"] = "outcome"


class TaskSpec(BaseModel):
    goal: str
    identity: Literal["owner", "ferb"]
    worker_type: str = 'default'
    targets: TargetsSpec = TargetsSpec()
    success_conditions: list[Condition] = []
    plan: Plan | None = None
    procedure_id: str | None = None


class Remember(BaseModel):
    """Something the owner stated about themselves or asked FERB to remember, with the exact words it rests on."""
    kind: Literal["fact", "preference", "person", "project", "env"]
    subject: str
    key: str
    content: str
    quote: str


class ForgetRequest(BaseModel):
    """A memory item the owner asked FERB to forget, by the id the planner was shown, with the owner's exact words."""
    item_id: str
    quote: str


class AnswerQuestion(BaseModel):
    """The planner's routing of the owner's message to one open question (L3): its id, from the planner's context,
    and the planner's reading of the answer. The answer recorded is always the owner's own words; for FERB's floor
    checks the reading must agree with the owner's plain Yes or No."""
    approval_id: str
    answer: str


class TaskControl(BaseModel):
    """Which earlier task the message is about and what to do with it. For a correction the planner always decides
    (no default, rule 6) the task's stated standing scope, "unchanged" or the revised scopes (D30), and names the open
    question ids the correction changes, which alone are set aside (L3)."""
    task_id: str
    action: Literal["stop", "retry", "continue", "steer"]
    # continue on an ended task: the planner's judgment that the follow-up asks for a different result (a new goal),
    # so FERB plans it afresh inside the same task; false resumes the saved goal and plan (L11, ef9e5d75)
    new_goal: bool = False
    standing_sends: Literal["unchanged"] | list[SendScope]
    supersedes: list[str]


class SupervisorOutcome(BaseModel):
    control: TaskControl | None = None
    kind: Literal["reply", "clarify", "task"]
    text: str
    display_text: str = ""
    answer_question: AnswerQuestion | None = None
    task: TaskSpec | None = None
    remember: list[Remember] = []
    forget: list["ForgetRequest"] = []
    timers: list["TimerRequest"] = []
    schedule: "ScheduleRequest | None" = None
    schedule_changes: list["ScheduleChange"] = []
    reminder_cancels: list["ReminderCancel"] = []
    conversation_changes: list["ConversationChange"] = []
    notification_rules: list["NotificationRule"] = []

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
    standing_sends: list[SendScope] = []   # stated now, with the owner here; a run can't add to them (D30)
    watch: Literal["none", "until_met", "standing"] = "none"      # a spec written before watches is ordinary work


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


class ConversationChange(BaseModel):
    id: str
    action: Literal['pause', 'resume', 'takeover', 'close']
    quote: str


class NotificationRule(BaseModel):
    app: str
    conversation: str | None = None
    topic: str | None = None
    until_at: str | None = None
    exceptions: list[str] = []
    quote: str


SupervisorOutcome.model_rebuild()


class WorkerOutcome(BaseModel):
    status: Literal["done", "failed", "needs_user", "blocked"]
    summary: str
    artifacts: list[str]
    evidence: list[str]
    answer: str = ''
    display_text: str = ''
    input_notes: list[dict[str, Any]] = []
    conversation_action: Literal['wait', 'handover', 'finished', 'blocked'] | None = None
    conversation_proposals: list[dict[str, Any]] = []
    watch_met: bool | None = None


class DispatchOutcome(BaseModel):
    stop_reason: str = ""
    display_text: str = ""
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
    display_text: str = ""


class VerifierResult(BaseModel):
    index: int
    met: bool | None
    evidence: str
    how: str | None = None     # unconfirmed only: one plain sentence on how the owner could check it themselves


class VerifierOutcome(BaseModel):
    results: list[VerifierResult]
