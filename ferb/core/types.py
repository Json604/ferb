"""Core types (ARCHITECTURE.md section 4). Pure data: no I/O, imports nothing from ferb except core."""

from dataclasses import dataclass
from datetime import datetime
from enum import Enum
from typing import Any, Literal


class Identity(Enum):
    FERB = "ferb"      # FERB's own profile + credentials
    OWNER = "owner"    # owner's logged-in browsers/apps; no credentials ever


@dataclass(frozen=True)
class SecretRef:
    key: str           # e.g. "ferb/email/password" (FERB identity only)


@dataclass(frozen=True)
class SecretSpec:
    ref: SecretRef
    allowed_origins: list[str]
    account_hint: str


class RiskLevel(Enum):
    LOW = 1
    MEDIUM = 2
    HIGH = 3


@dataclass(frozen=True)
class Budget:
    """Aggregate limits for a task tree (section 7 step 10). None = no limit on that axis."""
    max_seconds: float | None = None
    max_tokens: int | None = None
    max_cost_usd: float | None = None
    max_steps: int | None = None


@dataclass
class Task:
    id: str
    parent_id: str | None
    goal: str
    device_id: str
    identity: Identity
    scopes: set[str]
    role: Literal["reader", "actor", "mixed"]
    request_id: str
    success_conditions: list[str]
    deadline: datetime | None
    budget: Budget


@dataclass
class Outcome:
    task_id: str
    status: Literal["done", "failed", "unknown", "cancelled", "needs_user"]
    summary: str
    artifacts: list[str]
    evidence: list[str]


@dataclass
class ActionIntent:
    id: str
    task_id: str
    capability: str
    target: dict[str, Any]
    payload_hash: str
    state: Literal["intended", "executing", "done", "failed", "unknown"]


@dataclass
class Span:
    id: str
    parent_id: str | None
    kind: str
    task_id: str | None        # None for daemon-level spans
    device_id: str | None
    started_at: datetime
    ended_at: datetime | None
    input: dict[str, Any]
    output: dict[str, Any] | None
    cost_usd: float | None
    error: str | None


@dataclass
class TrailEvent:
    id: str
    at: datetime
    device_id: str
    actor: Literal["owner", "ferb", "unknown"]
    task_id: str | None
    source: str
    data: dict[str, Any]


@dataclass
class MemoryItem:
    id: str
    kind: Literal["fact", "preference", "person", "project", "env"]
    subject: str
    content: str
    source_ids: list[str]
    observed_at: datetime
    valid_from: datetime
    valid_to: datetime | None
    supersedes: str | None
    confidence: float
    origin: Literal["owner_stated", "observed", "inferred", "third_party"]
    status: Literal["proposed", "active", "retracted"]

