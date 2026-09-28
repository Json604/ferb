"""Typed outcomes the supervisor and workers must return (section 7 step 2, section 4 Outcome).

JSON schemas in strict structured-output form (every property required, no extra properties), plus pydantic
models that validate what comes back.
"""

from typing import Any, Literal

from pydantic import BaseModel

from ferb.policy.intent import IMPLIES
from ferb.policy.rules import SCOPES_BY_IDENTITY, Kind

KINDS = sorted(v for k, v in vars(Kind).items() if not k.startswith("_") and isinstance(v, str))
ALL_SCOPES = sorted(set().union(*SCOPES_BY_IDENTITY.values()))


def _strings(enum: list[str] | None = None) -> dict[str, Any]:
    item: dict[str, Any] = {"type": "string"}
    if enum:
        item["enum"] = enum
    return {"type": "array", "items": item}


PLAN_SCHEMA: dict[str, Any] = {
    "type": "object", "additionalProperties": False,
    "required": ["goal", "identity", "role", "verbs", "extra_kinds", "recipients", "sites", "accounts", "paths",
                 "max_amount", "scopes", "success_conditions", "deadline_minutes"],
    "properties": {
        "goal": {"type": "string"},
        "identity": {"type": "string", "enum": ["owner", "ferb"]},
        "role": {"type": "string", "enum": ["reader", "actor", "mixed"]},
        "verbs": _strings(sorted(IMPLIES)),
        "extra_kinds": _strings(KINDS),
        "recipients": _strings(), "sites": _strings(), "accounts": _strings(), "paths": _strings(),
        "max_amount": {"type": ["number", "null"]},
        "scopes": _strings(ALL_SCOPES),
        "success_conditions": _strings(),
        "deadline_minutes": {"type": "integer"},
    },
}

SUPERVISOR_SCHEMA: dict[str, Any] = {
    "type": "object", "additionalProperties": False, "required": ["kind", "text", "plan"],
    "properties": {
        "kind": {"type": "string", "enum": ["reply", "clarify", "plan"]},
        "text": {"type": "string"},
        "plan": {"anyOf": [{"type": "null"}, PLAN_SCHEMA]},
    },
}

WORKER_SCHEMA: dict[str, Any] = {
    "type": "object", "additionalProperties": False, "required": ["status", "summary", "artifacts", "evidence"],
    "properties": {
        "status": {"type": "string", "enum": ["done", "failed", "needs_user"]},
        "summary": {"type": "string"},
        "artifacts": _strings(),
        "evidence": _strings(),
    },
}


class Plan(BaseModel):
    goal: str
    identity: Literal["owner", "ferb"]
    role: Literal["reader", "actor", "mixed"]
    verbs: list[str]
    extra_kinds: list[str]
    recipients: list[str]
    sites: list[str]
    accounts: list[str]
    paths: list[str]
    max_amount: float | None
    scopes: list[str]
    success_conditions: list[str]
    deadline_minutes: int


class SupervisorOutcome(BaseModel):
    kind: Literal["reply", "clarify", "plan"]
    text: str
    plan: Plan | None


class WorkerOutcome(BaseModel):
    status: Literal["done", "failed", "needs_user"]
    summary: str
    artifacts: list[str]
    evidence: list[str]
