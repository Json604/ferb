"""Typed outcomes the supervisor and workers must return (section 7 steps 2-4, section 4 Outcome).

JSON schemas in strict structured-output form (every property required, no extra properties, null where optional),
plus pydantic models that validate what comes back. A plan is one intent (from the owner's own words) and a small
graph of subtasks; readers return typed fields, actors receive only those fields (section 7 step 4).
"""

from typing import Any, Literal

from pydantic import BaseModel, Field

from ferb.policy.intent import IMPLIES
from ferb.policy.rules import SCOPES_BY_IDENTITY, Kind

KINDS = sorted(v for k, v in vars(Kind).items() if not k.startswith("_") and isinstance(v, str))
ALL_SCOPES = sorted(set().union(*SCOPES_BY_IDENTITY.values()))
FIELD_TYPES = ["string", "number", "boolean", "string_list", "email", "url"]
CHECK_KINDS = ["none", "file_exists", "file_contains", "ledger_done", "tab_url", "account"]
DEFAULT_FIELD_CHARS, MAX_FIELD_CHARS = 500, 2000


def _strings(enum: list[str] | None = None) -> dict[str, Any]:
    item: dict[str, Any] = {"type": "string"}
    if enum:
        item["enum"] = enum
    return {"type": "array", "items": item}


def _nullable(t: str) -> dict[str, Any]:
    return {"type": [t, "null"]}


def _obj(props: dict[str, Any]) -> dict[str, Any]:
    return {"type": "object", "additionalProperties": False, "required": list(props), "properties": props}


INTENT_SCHEMA = _obj({
    "verbs": _strings(sorted(IMPLIES)), "extra_kinds": _strings(KINDS),
    "recipients": _strings(), "sites": _strings(), "accounts": _strings(), "paths": _strings(),
    "max_amount": _nullable("number"),
})
CONDITION_SCHEMA = _obj({
    "text": {"type": "string"},
    # a code-checkable form when the condition allows one (verified without a model); kind "none" otherwise
    "check": _obj({"kind": {"type": "string", "enum": CHECK_KINDS}, "path": _nullable("string"),
                   "text": _nullable("string"), "url_contains": _nullable("string"), "site": _nullable("string"),
                   "expected": _nullable("string")}),
})
SUBTASK_SCHEMA = _obj({
    "id": {"type": "string"}, "goal": {"type": "string"},
    "identity": {"type": "string", "enum": ["owner", "ferb"]},
    "role": {"type": "string", "enum": ["reader", "actor"]},
    "scopes": _strings(ALL_SCOPES),
    "depends_on": _strings(),
    "inputs": {"type": "array", "items": _obj({"from": {"type": "string"}, "field": {"type": "string"}})},
    "output_fields": {"type": "array", "items": _obj({
        "name": {"type": "string"}, "type": {"type": "string", "enum": FIELD_TYPES}, "description": {"type": "string"},
        "max_chars": _nullable("integer")})},
    "success_conditions": {"type": "array", "items": CONDITION_SCHEMA},
})
PLAN_SCHEMA: dict[str, Any] = _obj({
    "goal": {"type": "string"}, "intent": INTENT_SCHEMA,
    "subtasks": {"type": "array", "items": SUBTASK_SCHEMA},
})

REMEMBER_SCHEMA = _obj({
    "kind": {"type": "string", "enum": ["fact", "preference", "person", "project", "env"]},
    "subject": {"type": "string"}, "key": {"type": "string"}, "content": {"type": "string"},
    "quote": {"type": "string"},
})
SUPERVISOR_SCHEMA: dict[str, Any] = _obj({
    "kind": {"type": "string", "enum": ["reply", "clarify", "plan"]},
    "text": {"type": "string"},
    "plan": {"anyOf": [{"type": "null"}, PLAN_SCHEMA]},
    "remember": {"type": "array", "items": REMEMBER_SCHEMA},
})

WORKER_SCHEMA: dict[str, Any] = _obj({
    "status": {"type": "string", "enum": ["done", "failed", "needs_user"]},
    "summary": {"type": "string"}, "artifacts": _strings(), "evidence": _strings(),
})

VERIFIER_SCHEMA: dict[str, Any] = _obj({
    "results": {"type": "array", "items": _obj({"index": {"type": "integer"}, "met": {"type": "boolean"},
                                                "evidence": {"type": "string"}})},
})

RECONCILE_SCHEMA: dict[str, Any] = _obj({
    "happened": {"type": "string", "enum": ["yes", "no", "unclear"]}, "evidence": {"type": "string"},
})


class Intent(BaseModel):
    verbs: list[str]
    extra_kinds: list[str]
    recipients: list[str]
    sites: list[str]
    accounts: list[str]
    paths: list[str]
    max_amount: float | None


class Check(BaseModel):
    kind: Literal["none", "file_exists", "file_contains", "ledger_done", "tab_url", "account"]
    path: str | None
    text: str | None
    url_contains: str | None
    site: str | None
    expected: str | None


class Condition(BaseModel):
    text: str
    check: Check


class InputRef(BaseModel):
    from_: str = Field(alias="from")
    field: str


class OutputField(BaseModel):
    name: str
    type: Literal["string", "number", "boolean", "string_list", "email", "url"]
    description: str
    max_chars: int | None

    @property
    def limit(self) -> int:
        return min(self.max_chars or DEFAULT_FIELD_CHARS, MAX_FIELD_CHARS)


class Subtask(BaseModel):
    id: str
    goal: str
    identity: Literal["owner", "ferb"]
    role: Literal["reader", "actor"]
    scopes: list[str]
    depends_on: list[str]
    inputs: list[InputRef]
    output_fields: list[OutputField]
    success_conditions: list[Condition]


class Plan(BaseModel):
    goal: str
    intent: Intent
    subtasks: list[Subtask]


class Remember(BaseModel):
    """Something the owner stated about themselves or asked FERB to remember, with the exact words it rests on."""
    kind: Literal["fact", "preference", "person", "project", "env"]
    subject: str
    key: str
    content: str
    quote: str


class SupervisorOutcome(BaseModel):
    kind: Literal["reply", "clarify", "plan"]
    text: str
    plan: Plan | None
    remember: list[Remember] = []


class WorkerOutcome(BaseModel):
    status: Literal["done", "failed", "needs_user"]
    summary: str
    artifacts: list[str]
    evidence: list[str]


class VerifierResult(BaseModel):
    index: int
    met: bool
    evidence: str


class VerifierOutcome(BaseModel):
    results: list[VerifierResult]


class ReconcileOutcome(BaseModel):
    happened: Literal["yes", "no", "unclear"]
    evidence: str


def reader_schema(fields: list[OutputField]) -> dict[str, Any]:
    """A reader's output: its status plus exactly the typed fields the plan asked for (strict, capped)."""
    props: dict[str, Any] = {}
    for f in fields:
        if f.type in ("string", "email", "url"):
            props[f.name] = {"type": ["string", "null"], "maxLength": f.limit}
        elif f.type == "number":
            props[f.name] = _nullable("number")
        elif f.type == "boolean":
            props[f.name] = _nullable("boolean")
        else:
            props[f.name] = {"type": ["array", "null"], "items": {"type": "string", "maxLength": f.limit},
                             "maxItems": 50}
    return _obj({"status": {"type": "string", "enum": ["done", "failed", "needs_user"]},
                 "summary": {"type": "string", "maxLength": 1000}, "fields": _obj(props)})
