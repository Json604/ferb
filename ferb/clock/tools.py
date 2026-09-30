"""time.* capabilities (section 13, build step 14): reminders for the owner and a worker's durable sleep.

A reminder only reminds: it never starts work (scheduled tasks are build step 15). A sleep counts as waiting (not a
stall) and ends with the task; the task's 4-hour limit still applies.
"""

from datetime import datetime, timedelta
from typing import Any, Callable

from pydantic import BaseModel, Field

from ferb.clock.when import WHEN_HELP, When, WhenError, resolve
from ferb.core.types import ActionSpec, Identity, RiskLevel, Target, ToolContext, ToolResult
from ferb.policy.rules import Kind


def _err(msg: str) -> ToolResult:
    return ToolResult({"error": msg}, is_error=True)


class _Base:
    scopes = {"time"}
    identities = {Identity.OWNER, Identity.FERB}
    risk, reversible = RiskLevel.LOW, True

    def __init__(self, timers: Callable[[], Any], now: Callable[[], datetime]) -> None:
        self.timers, self.now = timers, now

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(Kind.TIMER, Target(), content=getattr(args, "message", None))

    def _add(self, at: datetime, message: str, ctx: ToolContext) -> ToolResult:
        t = self.timers()
        if t is None:
            return _err("timers aren't available")
        try:
            tm = t.add("reminder", at, message, task_id=ctx.task_id, request_id=ctx.request_id)
        except ValueError as e:
            return _err(str(e))
        return ToolResult({"timer": tm.id, "at": tm.fire_at.astimezone().isoformat(timespec="minutes"),
                           "note": "the owner gets it as a notification and in ferb chat"})


class TimeRemindAt(_Base):
    name = "time.remind_at"
    description = "Remind the owner at a time. It only reminds. " + WHEN_HELP

    class Input(When):
        message: str = Field(max_length=300)

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        try:
            at = resolve(When.model_validate(args.model_dump(include=set(When.model_fields))), self.now())
        except WhenError as e:
            return _err(str(e))
        return self._add(at, str(getattr(args, "message")), ctx)


class TimeSetTimer(_Base):
    name = "time.set_timer"
    description = "Set a timer for the owner that goes off after some minutes. It only notifies."

    class Input(BaseModel):
        minutes: float = Field(gt=0, le=60 * 24 * 366)
        message: str = Field(max_length=300)

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        return self._add(self.now() + timedelta(minutes=float(getattr(args, "minutes"))), str(getattr(args, "message")),
                         ctx)


class TimeSleepUntil(_Base):
    name = "time.sleep_until"
    description = ("Pause this task until a time, e.g. to wait for something to be ready. Within the task's time "
                   "limit; waiting doesn't count as being stuck. " + WHEN_HELP)

    class Input(When):
        pass

    def __init__(self, timers: Callable[[], Any], now: Callable[[], datetime],
                 sleep: Callable[[], Any], limit: Callable[[str], datetime | None]) -> None:
        super().__init__(timers, now)
        self.sleep, self.limit = sleep, limit

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(Kind.READ, Target())

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        try:
            at = resolve(When.model_validate(args.model_dump(include=set(When.model_fields))), self.now())
        except WhenError as e:
            return _err(str(e))
        limit = self.limit(ctx.task_id)
        if limit is not None and at >= limit:
            return _err(f"that's past this task's time limit ({limit.astimezone():%H:%M})")
        sleep = self.sleep()
        if sleep is None:
            return _err("timers aren't available")
        await sleep(ctx.worker_id, ctx.task_id, at)
        return ToolResult({"woke_at": self.now().astimezone().isoformat(timespec="seconds")})
