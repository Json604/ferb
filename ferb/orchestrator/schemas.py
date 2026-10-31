"""Owns structured approach outputs and mechanical validation. Must not bypass the gateway or turn model uncertainty into an implicit target choice.

Typed outcomes the supervisor and workers must return.

JSON schemas in strict structured-output form (every property required, no extra properties, null where optional),
plus pydantic models that validate what comes back. A chat turn is a reply, one clarifying question, or a task: the
goal in plain words, the identity, the targets the model understood (informational), and success conditions."""

from typing import Any, Literal

from pydantic import BaseModel, model_validator

from ferb.clock.when import DAY_WORDS, WHEN_SCHEMA, When
from ferb.policy.targets import Targets
from ferb.orchestrator.plans import STEP_KINDS, Part, Plan, SendScope
from ferb.orchestrator.probes import PROBE_SCHEMA, Probe

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
EVENT_MATCH_SCHEMA = {"type": "array", "items": _obj({"field": {"type": "string"}, "value": {"type": "string"}})}
# A step: do (optional input list; optional repeat rule = a loop, L31), wait, ask or converse (plans.Step)
STEP_SCHEMA = _obj({
    "id": {"type": "string"}, "kind": {"type": "string", "enum": list(STEP_KINDS)},
    "goal": {"type": "string"}, "depends_on": _strings(), "inputs": _strings(), "worker_type": {"type": "string"},
    "conditions": _strings(), "until": _nullable("string"), "event": _nullable("string"),
    "event_match": EVENT_MATCH_SCHEMA,
    "question": {"type": "string"}, "options": _strings(),
    "conversation": {"anyOf": [{"type": "null"}, CONVERSATION_SCHEMA]},
    "repeat": {"anyOf": [{"type": "null"}, _obj({"end_when": {"type": "string"}, "event": _nullable("string"),
                                                 "event_match": EVENT_MATCH_SCHEMA})]},
})
SEND_SCOPE_SCHEMA = _obj({
    "capability": {"type": "string"}, "identity": {"type": "string", "enum": ["owner", "ferb"]},
    "account": _nullable("string"), "site": _nullable("string"), "recipients": _strings(),
    "content": _nullable("string")})
# step 10b (patch table row 24): how a campaign's scope is written, shown where the scope is (not in the prompt)
SEND_SCOPE_SCHEMA["description"] = ("One exact send scope. Posting from the owner's browser (a campaign): capability "
                                    "browser.act, site the page's host (x.com), recipients [] for public posts, "
                                    "content null unless the owner gave the exact text, account the account the posts "
                                    "go out from.")
PLAN_SCHEMA = _obj({"steps": {"type": "array", "items": STEP_SCHEMA}, "input_paths": _strings(),
                    "big_job": {"type": "boolean"}, "time_estimate": {"type": "string"},
                    "cost_estimate": {"type": "string"},
                    "standing_sends": {"type": "array", "items": SEND_SCOPE_SCHEMA}})
# Decision 19 (answers in steps): one of the owner's asks, in their order; exactly one of direct / answer_step
PART_SCHEMA = _obj({"id": {"type": "string"}, "quote": {"type": "string"}, "label": {"type": "string"},
                    "direct": _nullable("string"), "answer_step": _nullable("string"),
                    "conditions": {"type": "array", "items": {"type": "integer"}}})
TASK_SCHEMA: dict[str, Any] = _obj({
    "goal": {"type": "string"},
    "identity": {"type": "string", "enum": ["owner", "ferb"]},
    "worker_type": {"type": "string"},
    "targets": TARGETS_SCHEMA,
    "success_conditions": {"type": "array", "items": CONDITION_SCHEMA, "minItems": 1},
    "plan": {"anyOf": [{"type": "null"}, PLAN_SCHEMA]},
    # hands decision 4: the folders the owner meant, resolved to paths (a typo resolved, "~/Downloads"); the plan line
    # states them, and they are the only visible folders the task's terminal commands may change
    "folders": _strings(),
    # hands decision 3: the owner asked to keep the task's tabs open (at its end they leave the group and stay open)
    "keep_tabs_open": {"type": "boolean"},
    # decision 19: the owner's separate asks (empty for one ask): each posts once, in its own row, when checked
    "parts": {"type": "array", "items": PART_SCHEMA},
})

# L36 §6: a remembered item's last day in the owner's own date words; code works out the date
UNTIL_SCHEMA = _obj({"quote": {"type": "string"},
                     "day": {"anyOf": [{"type": "string", "enum": list(DAY_WORDS)}, {"type": "null"}]},
                     "date": _nullable("string"), "in_days": _nullable("integer")})
REMEMBER_SCHEMA = _obj({
    "kind": {"type": "string", "enum": ["fact", "preference", "person", "project", "env"]},
    "subject": {"type": "string"}, "key": {"type": "string"}, "content": {"type": "string"},
    "quote": {"type": "string"},
    # L36 §5: this entry's content is the owner's username for this https:// site; null otherwise
    "login_origin": _nullable("string"),
    "until": {"anyOf": [{"type": "null"}, UNTIL_SCHEMA]},
    # step 9a: the memory ids of current facts this one replaces (code checks each holds); [] for none
    "replaces": _strings(),
    # step 9a: a period fact without an end date in the owner's words: days until FERB checks it still holds; 0 = none
    "review_in_days": {"type": "integer"},
})
# Decision 22: a schedule's end, the remembered item's `until` with a time of day or a span from now as well ("until 6
# pm", "for the next 2 hours"); code works out the time (`when.end_at`). Unused fields are "" / 0, not null: the
# planner's schema is at Anthropic's union-type limit (ferb/runtimes/schema_shape.py), and an object that is null when
# absent costs none
SCHEDULE_UNTIL_SCHEMA = _obj({"quote": {"type": "string"}, "day": {"type": "string", "enum": ["", *DAY_WORDS]},
                              "date": {"type": "string"}, "in_days": {"type": "integer"}, "time": {"type": "string"},
                              "in_minutes": {"type": "number"}})
# Decision 17 ("when you're back at your Mac"): true when the owner wants it when they're at their Mac; code holds each
# run or reminder until they're active there. A plain boolean: the planner's schema is at the union-type limit
WHEN_ACTIVE_SCHEMA = {"type": "boolean", "description": (
    "true when the owner's words want it when they're at their Mac (\"whenever I'm on my Mac\", \"when I'm back at my "
    "Mac\"): it comes due as `when` and `repeat` say and FERB holds it until they're active at the Mac. `when` may then "
    "leave the time out (all null: from today; a day alone: from that day). false otherwise.")}
TIMER_SCHEMA = _obj({"when": WHEN_SCHEMA, "message": {"type": "string"}, "quote": {"type": "string"},
                     "when_active": WHEN_ACTIVE_SCHEMA})
SCHEDULE_SCHEMA = _obj({
    "request": {"type": "string"}, "when": WHEN_SCHEMA,
    "repeat": {"type": "string", "enum": ["none", "daily", "weekdays", "weekly", "every"]},
    "weekday": _nullable("string"), "every_minutes": _nullable("integer"), "quote": {"type": "string"},
    "targets": TARGETS_SCHEMA,
    # D30: the scopes the runs may send within without asking, stated when the schedule is made (an owner turn)
    "standing_sends": {"type": "array", "items": SEND_SCOPE_SCHEMA},
    # "tell me when …": until_met ends itself once a confirmed run finds the condition met; standing keeps watching
    "watch": {"type": "string", "enum": ["none", "until_met", "standing"]},
    # decision 22: the end the owner's words give it ("until Nov 3", "for 2 weeks"); null = runs until stopped
    "until": {"anyOf": [{"type": "null"}, SCHEDULE_UNTIL_SCHEMA]},
    "when_active": WHEN_ACTIVE_SCHEMA,          # decision 17: each run waits until the owner is at the Mac
})
SCHEDULE_CHANGE_SCHEMA = _obj({"id": {"type": "string"},
                               "action": {"type": "string", "enum": ["cancel", "pause", "resume", "set_end"]},
                               "quote": {"type": "string"},
                               # set_end only: the new end, in the owner's words (else null)
                               "until": {"anyOf": [{"type": "null"}, SCHEDULE_UNTIL_SCHEMA]}})
REMINDER_CHANGE_SCHEMA = _obj({"id": {"type": "string"}, "quote": {"type": "string"}})
SUPERVISOR_SCHEMA: dict[str, Any] = _obj({
    "kind": {"type": "string", "enum": ["reply", "clarify", "task"]},
    "answer_question": {"anyOf": [{"type": "null"}, _obj({"approval_id": {"type": "string"},
                         "answer": {"type": "string"}})]},
    "task": {"anyOf": [{"type": "null"}, TASK_SCHEMA]},
    "control": {"anyOf": [{"type": "null"}, _obj({"task_id": {"type": "string"},
                  "action": {"type": "string", "enum": ["stop", "retry", "continue", "pause"]},
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
    # the owner's answer, shown in full and streamed: last, so a pure reply streams after every action field
    "text": {"type": "string"},
})

# The worker's report (step 8b, notes/orchestration.md "Attempt loop"): done or not done, the reason and an optional
# question for the owner when not done, the owner's answer, the artifacts and a `how` note for memory (notes/memory.md
# Tier 3). A role's own fields are added only to that role's schema (`worker_schema`).
_REPORT = {
    "status": {"type": "string", "enum": ["done", "not_done"]},
    "reason": {"type": "string"},              # not_done: why, in plain words; "" when done
    "question": _nullable("string"),           # not_done: one what/who/which question for the owner, else null
    "answer": {"type": "string"},              # the owner's answer, answer first
    "artifacts": _strings(),                   # paths of deliverables created or changed
    "how": {"type": "string"},                 # a few lines on the steps that worked (memory), "" when nothing did
}
_INPUT_NOTES = {"input_notes": {"type": "array", "items": _obj({"input": {"type": "string"},
                                "note": {"type": "string"}, "evidence": _strings()})}}
_PROPOSALS = {"conversation_proposals": {"type": "array", "items": CONVERSATION_SCHEMA}}
_CONVERSATION = {"conversation_action": {"type": "string", "enum": ["wait", "handover", "finished", "blocked"]}}
_WATCH = {"watch_met": {"type": ["boolean", "null"]}}       # whether the watched condition holds now (null: unseen)
# decision 17: a scheduled run (not a watch) that found nothing to tell the owner and did nothing posts nothing
_SCHEDULED = {"nothing_to_tell": {"type": "boolean", "description": (
    "true only when this scheduled run found nothing its request asks to tell the owner and did nothing for them: "
    "FERB records it without posting it. false whenever the answer has anything for the owner.")}}
_LOOP = {"loop_action": {"type": "string", "enum": ["continue", "finished", "blocked"]},
         "probe": {"anyOf": [{"type": "null"}, PROBE_SCHEMA]}}
# decision 19: a step of a multi-part task that finds an answer already posted to the owner is wrong or out of date
_PARTS = {"part_updates": {"type": "array", "items": _obj({"part": {"type": "string"}, "text": {"type": "string"},
                                                           "why": {"type": "string"}})}}


def worker_schema(*, inputs: bool = False, proposals: bool = False, conversation: bool = False, watch: bool = False,
                  loop: bool = False, parts: bool = False, scheduled: bool = False) -> dict[str, Any]:
    """The report schema for one worker run: the shared report, plus input notes (a do step with an input list),
    conversation proposals (a step a converse step takes its scopes from), a conversation turn's action, a watch's
    watch_met, a loop round's action and probe, a multi-part task's step's corrections to answers already posted, a
    scheduled run's nothing_to_tell."""
    props = dict(_REPORT)
    for wanted, extra in ((inputs, _INPUT_NOTES), (proposals, _PROPOSALS), (conversation, _CONVERSATION),
                          (watch, _WATCH), (loop, _LOOP), (parts, _PARTS), (scheduled, _SCHEDULED)):
        if wanted:
            props |= extra
    return _obj(props)


WORKER_SCHEMA: dict[str, Any] = worker_schema()
# Each role's schema as a constant too, so the provider-shape test covers every one
WORKER_STEP_SCHEMA: dict[str, Any] = worker_schema(inputs=True, proposals=True, parts=True)
WORKER_CONVERSATION_SCHEMA: dict[str, Any] = worker_schema(conversation=True)
WORKER_WATCH_SCHEMA: dict[str, Any] = worker_schema(watch=True)
WORKER_LOOP_SCHEMA: dict[str, Any] = worker_schema(loop=True)
WORKER_ALL_SCHEMA: dict[str, Any] = worker_schema(inputs=True, proposals=True, conversation=True, watch=True, loop=True,
                                                  parts=True)

# The merged checker (owner decision 3, prompts decision 1): it judges each condition and chooses what's next. Code
# checks the choice against the checker's own verdicts (`Supervisor._next`); it never judges content.
NEXT = ["accept", "fix_answer", "retry", "ask", "accept_unconfirmed", "fail"]
CHECKER_SCHEMA: dict[str, Any] = _obj({
    "results": {"type": "array", "items": _obj({"index": {"type": "integer"}, "met": {"type": ["boolean", "null"]},
                                                "evidence": {"type": "string"},
                                                "how": {"type": ["string", "null"]}})},
    "next": {"type": "string", "enum": NEXT},
    # accept / fix_answer / accept_unconfirmed: exact edits to the worker's answer (each `find` copied from it)
    "answer_edits": {"type": "array", "items": _obj({"find": {"type": "string"}, "replace": {"type": "string"}})},
    "missing": {"type": "string"},             # retry: what is missing
    "instructions": {"type": "string"},        # retry: what the next attempt does differently
    "worker_type": _nullable("string"),        # retry: another worker type from the supplied list, else null
    "question": {"type": "string"},            # ask: one what/who/which question for the owner
    "reason": {"type": "string"},              # fail / accept_unconfirmed: what happened, in the owner's terms
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
    folders: list[str] = []          # resolved folders (hands decision 4); a spec saved before step 8c has none
    keep_tabs_open: bool = False     # the owner asked to keep its tabs open (hands decision 3)
    parts: list[Part] = []           # the owner's separate asks (decision 19); a spec saved before step 8d has none


class Until(BaseModel):
    """A remembered item's last day in the owner's own date words (L36 §6); code works out the date."""
    quote: str
    day: str | None = None
    date: str | None = None
    in_days: int | None = None


class ScheduleUntil(Until):
    """A schedule's end in the owner's words (decision 22): a day as for `Until`, optionally a time on it, or a span from
    now; code works out the time (`when.end_at`)."""
    time: str | None = None
    in_minutes: float | None = None

    @model_validator(mode="before")
    @classmethod
    def _unused(cls, data: Any) -> Any:
        """The schema's "" / 0 for an unused field is None here."""
        if isinstance(data, dict):
            return {k: (None if k != "quote" and v in ("", 0) else v) for k, v in data.items()}
        return data

    def when(self) -> When:
        return When(in_minutes=self.in_minutes, in_days=self.in_days, day=self.day, date=self.date, time=self.time)


class Remember(BaseModel):
    """Something the owner stated about themselves or asked FERB to remember, with the exact words it rests on."""
    kind: Literal["fact", "preference", "person", "project", "env"]
    subject: str
    key: str
    content: str
    quote: str
    login_origin: str | None = None   # L36 §5: this entry's content is the username for this https:// site
    until: Until | None = None        # L36 §6: the item's last day, in the owner's words
    replaces: list[str] = []          # step 9a: memory ids of the current facts this one replaces
    review_in_days: int = 0           # step 9a: when FERB checks a period fact still holds (0: no check)


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
    # pause (owner decision 8): a running task stops where it is, losing nothing; the owner's next correction (continue)
    # resumes it in place
    action: Literal["stop", "retry", "continue", "pause"]
    # continue on an ended task: the planner's judgment that the follow-up asks for a different result (a new goal),
    # so FERB plans it afresh inside the same task; false resumes the saved goal and plan (L11, ef9e5d75)
    new_goal: bool = False
    standing_sends: Literal["unchanged"] | list[SendScope]
    supersedes: list[str]


class SupervisorOutcome(BaseModel):
    control: TaskControl | None = None
    kind: Literal["reply", "clarify", "task"]
    text: str
    answer_question: AnswerQuestion | None = None
    task: TaskSpec | None = None
    remember: list[Remember] = []
    forget: list["ForgetRequest"] = []
    timers: list["TimerRequest"] = []
    schedule: "ScheduleRequest | None" = None
    schedule_changes: list["ScheduleChange"] = []
    reminder_cancels: list["ReminderCancel"] = []
    conversation_changes: list["ConversationChange"] = []

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
    until: ScheduleUntil | None = None     # decision 22: its end in the owner's words; None runs until stopped
    when_active: bool = False              # decision 17: each run fires when the owner is at the Mac


class ScheduleChange(BaseModel):
    id: str
    action: Literal["cancel", "pause", "resume", "set_end"]
    quote: str
    until: ScheduleUntil | None = None     # set_end: the new end in the owner's words


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
    when_active: bool = False    # decision 17: it fires when the owner is at the Mac


class ConversationChange(BaseModel):
    id: str
    action: Literal['pause', 'resume', 'takeover', 'close']
    quote: str


SupervisorOutcome.model_rebuild()


class WorkerReport(BaseModel):
    """A worker run's report (WORKER_SCHEMA and its role variants). A role's own fields keep their empty default in
    every other role's report."""
    status: Literal["done", "not_done"]
    reason: str = ''
    question: str | None = None
    answer: str = ''
    artifacts: list[str] = []
    how: str = ''
    input_notes: list[dict[str, Any]] = []
    conversation_action: Literal['wait', 'handover', 'finished', 'blocked'] | None = None
    conversation_proposals: list[dict[str, Any]] = []
    watch_met: bool | None = None
    loop_action: Literal['continue', 'finished', 'blocked'] | None = None
    probe: Probe | None = None
    part_updates: list[dict[str, Any]] = []
    nothing_to_tell: bool = False


class QuestionOutcome(BaseModel):
    ask_owner: bool
    answer: str
    question: str
    display_text: str = ""


class CheckerResult(BaseModel):
    index: int
    met: bool | None
    evidence: str
    how: str | None = None     # unconfirmed only: one plain sentence on how the owner could check it themselves


class AnswerEdit(BaseModel):
    find: str
    replace: str


class CheckerOutcome(BaseModel):
    results: list[CheckerResult]
    next: Literal["accept", "fix_answer", "retry", "ask", "accept_unconfirmed", "fail"]
    answer_edits: list[AnswerEdit] = []
    missing: str = ''
    instructions: str = ''
    worker_type: str | None = None
    question: str = ''
    reason: str = ''
