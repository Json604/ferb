"""Protocols at real boundaries (ARCHITECTURE.md section 5).

Each protocol is added when its first implementation lands, with section 5's signature exactly.
"""

from datetime import datetime
from typing import AsyncIterator, Protocol

from pydantic import BaseModel

from ferb.core.types import (ActionResult, ActionSpec, BrowserAction, DriverFeatures, ElementRef, Identity,
                             PageSnapshot, RiskLevel, RunHandle, RuntimeEvent, RuntimeFeatures, SecretRef, SecretSpec,
                             TabRef, Task, ToolContext, ToolResult, WaitCondition)


class Clock(Protocol):
    def now(self) -> datetime: ...              # timezone-aware wall time: timestamps, schedules
    def monotonic(self) -> float: ...           # elapsed time + timeouts; unaffected by clock changes
    async def sleep_until(self, t: datetime) -> None: ...


class IdSource(Protocol):
    """Injected ID generation (rule 7) so any run can be replayed with deterministic ids."""
    def new_id(self) -> str: ...


class AgentRuntime(Protocol):
    name: str
    features: RuntimeFeatures                   # native_tools_contained must be True to be registered
    async def start(self, task: Task, system: str, context: str, run_token: str) -> RunHandle: ...
    def events(self, run: RunHandle) -> AsyncIterator[RuntimeEvent]: ...
    async def continue_(self, run: RunHandle, message: str) -> None: ...   # raises Unsupported if no continuation
    async def cancel(self, run: RunHandle) -> None: ...


class Capability(Protocol):
    name: str
    description: str
    scopes: set[str]
    identities: set[Identity]                   # which identities may call it
    risk: RiskLevel
    reversible: bool
    Input: type[BaseModel]
    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult: ...
    def describe(self, args: BaseModel) -> ActionSpec: ...   # kind, target, content, data labels for policy


class SecretStore(Protocol):                    # FERB identity only
    def has(self, ref: SecretRef) -> bool: ...
    def spec(self, ref: SecretRef) -> SecretSpec: ...
    # No reveal method for callers. Filling happens in the extension of the FERB profile: the daemon checks the
    # target frame's origin (as the extension reports it) against spec.allowed_origins, then the extension re-checks
    # location.origin in the same call that sets the value on that exact element, and forgets the value.


class BrowserDriver(Protocol):
    name: str                                   # "ext:chrome" | "ext:brave" | "ax:<browser>"
    identity: Identity
    features: DriverFeatures
    async def tabs(self) -> list[TabRef]: ...
    async def open(self, url: str, background: bool = True) -> TabRef: ...
    async def snapshot(self, tab: TabRef) -> PageSnapshot: ...            # structured, sanitized, has a generation
    async def act(self, el: ElementRef, action: BrowserAction, intent_id: str) -> ActionResult: ...
    async def wait(self, tab: TabRef, until: WaitCondition, timeout: float) -> bool: ...
    async def screenshot(self, tab: TabRef) -> bytes: ...
    async def close(self, tab: TabRef) -> None: ...
