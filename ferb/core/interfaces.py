"""Protocols at real boundaries (ARCHITECTURE.md section 5).

Each protocol is added when its first implementation lands, with section 5's signature exactly.
"""

from datetime import datetime
from typing import AsyncIterator, Protocol

from pydantic import BaseModel

from ferb.core.types import (ActionSpec, Identity, RiskLevel, RunHandle, RuntimeEvent, RuntimeFeatures, Task,
                             ToolContext, ToolResult)


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
