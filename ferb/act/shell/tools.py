"""shell.* capabilities (OWNER tasks only; lost when a worker is exposed, 10.2): run a command and get its output,
or leave it running and read / send / kill it later. Destructive commands always ask the owner (hard floor)."""

from pydantic import BaseModel, Field

from ferb.act.shell import guard
from ferb.act.shell.manager import ShellManager
from ferb.core.errors import Refused
from ferb.core.types import ActionSpec, Identity, RiskLevel, Target, ToolContext, ToolResult
from ferb.policy.rules import Kind

TAIL = 8000


def _err(msg: str) -> ToolResult:
    return ToolResult({"error": msg}, is_error=True)


class _Base:
    identities = {Identity.OWNER}
    scopes = {"shell"}
    risk, reversible = RiskLevel.MEDIUM, False

    def __init__(self, shells: ShellManager) -> None:
        self.shells = shells

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        try:
            return await self._run(args, ctx)
        except Refused as e:
            return _err(str(e))

    async def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        raise NotImplementedError


class ShellRun(_Base):
    name = "shell.run"
    description = ("Run a shell command (/bin/sh) in a folder FERB may use (default: FERB's workspace). Waits up to "
                   "timeout_s and returns the output; if it's still running, returns its id (read/send/kill it "
                   "later). background=true keeps it running after the task ends (servers). Destructive commands "
                   "ask the owner first.")

    class Input(BaseModel):
        command: str = Field(max_length=4000)
        cwd: str | None = Field(default=None, description="Working folder (default: FERB's workspace)")
        timeout_s: float = Field(default=30, ge=1, le=120)
        background: bool = False

    def describe(self, args: BaseModel) -> ActionSpec:
        cmd = str(getattr(args, "command"))
        return ActionSpec(guard.kind(cmd), Target(path=getattr(args, "cwd")), content=cmd)

    async def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        cmd = str(getattr(args, "command"))
        cwd = guard.check(cmd, getattr(args, "cwd"), ctx)
        h = await self.shells.start(["/bin/sh", "-c", cmd], cwd, ctx.task_id, bool(getattr(args, "background")))
        code = await self.shells.wait(h, float(getattr(args, "timeout_s")))
        text, offset = await self.shells.read(h)
        out = {"id": h.id, "running": code is None, "exit_code": code, "output": text[-TAIL:], "offset": offset}
        if len(text) > TAIL:
            out["output_truncated"] = True
        return ToolResult(out, is_error=code not in (None, 0))


class ShellRead(_Base):
    name, description = "shell.read", "Read a running (or finished) command's output since an offset."
    risk, reversible = RiskLevel.LOW, True

    class Input(BaseModel):
        id: str
        since: int = 0

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(Kind.READ)

    async def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        h = self.shells.handle(str(getattr(args, "id")), ctx.task_id)
        text, offset = await self.shells.read(h, int(getattr(args, "since")))
        return ToolResult({"output": text[-TAIL:], "offset": offset})


class ShellSend(_Base):
    name, description = "shell.send", "Send text (e.g. an answer plus a newline) to a running command's input."

    class Input(BaseModel):
        id: str
        text: str = Field(max_length=4000)

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(Kind.RUN_SHELL, content=str(getattr(args, "text")))

    async def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        await self.shells.send(self.shells.handle(str(getattr(args, "id")), ctx.task_id), str(getattr(args, "text")))
        return ToolResult({"sent": True})


class ShellKill(_Base):
    name, description = "shell.kill", "Stop one of this task's commands (and its children)."
    risk, reversible = RiskLevel.LOW, True

    class Input(BaseModel):
        id: str

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(Kind.OPEN_TAB)                   # ending FERB's own process: reversible bookkeeping

    async def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        await self.shells.kill(self.shells.handle(str(getattr(args, "id")), ctx.task_id))
        return ToolResult({"killed": True})


class ShellList(_Base):
    name, description = "shell.list", "This task's commands: id, command, running, exit code."
    risk, reversible = RiskLevel.LOW, True

    class Input(BaseModel):
        pass

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(Kind.READ)

    async def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        return ToolResult({"processes": [{"id": p.id, "command": p.command, "running": p.running,
                                          "exit_code": p.exit_code, "background": p.long_lived}
                                         for p in self.shells.list() if p.owner_task == ctx.task_id]})
