"""Expose owned shell process operations through typed tools. Must not choose commands, retries or foreground methods.

Task-owned shell processes: start, wait, read, send and kill without detached launcher guesses."""

from pydantic import BaseModel, Field

from ferb.act.shell import guard
from ferb.act.shell.manager import ProcHandle, ShellManager
from ferb.core.errors import Refused
from ferb.core.types import ActionSpec, Identity, RiskLevel, Target, ToolContext, ToolResult
from ferb.policy.rules import Kind

TAIL = 8000
# What a command read can't be known by code: fetches_network misses `gh`, `npm view` and scripts, and a local read
# may be a downloaded file. So command output is unlabeled outside content (S3); detected network use reports
# external:web:shell instead (_untrusted). F5's proxy (real network use) and sandbox path lists (real file reads) will
# provide facts that can narrow this label.
OUTPUT_ORIGIN = "external:unlabeled"


def _untrusted(network: bool, command: str) -> tuple[tuple[str, str], ...]:
    """Network-fetching commands bring in untrusted content: annotate its source for the agent."""
    return (("web", f"shell: {command}"),) if network else ()


def _err(msg: str) -> ToolResult:
    return ToolResult({"error": msg}, is_error=True)


class _Base:
    identities = {Identity.OWNER}
    scopes = {"shell"}
    risk, reversible = RiskLevel.MEDIUM, False
    # True: it can change what is on the owner's screen (bring an app or window forward, switch the Space, move
    # the cursor, open a dialog); desktop changes near it may be FERB's (P26)
    on_screen = False

    def __init__(self, shells: ShellManager) -> None:
        self.shells = shells

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        try:
            return await self._run(args, ctx)
        except Refused as e:
            return _err(str(e))

    async def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        raise NotImplementedError


class ShellStart(_Base):
    name = "shell.start"
    on_screen = True                  # a command FERB runs can open an app or bring a window forward
    description = ("Start a tracked shell command immediately and return its id. Use for coding agents and other "
                   "long commands, then shell.wait/read for actual progress and exit status. Run the command "
                   "directly, without nohup or trailing &: the id tracks this command, not detached children. "
                   "background=true keeps it alive after task completion (servers); it is not needed to work "
                   "asynchronously during a task. Starting a process does not prove it is ready or successful.")
    target_args = ("cwd",)                             # Target.path, normalized by the gateway (P19 hash)
    method_args = ("background", "timeout_s")          # how it runs, not what it does (P19 hash)
    ledger_key = "shell.run"                           # shell.start and shell.run run the same command: one action

    class Input(BaseModel):
        command: str
        cwd: str | None = Field(default=None, description="Working folder (default: FERB's workspace)")
        background: bool = False

    def describe(self, args: BaseModel) -> ActionSpec:
        cmd = str(getattr(args, "command"))
        # No cwd is the workspace (guard.working_directory); "." normalizes to it, so both spellings are one target
        return ActionSpec(Kind.RUN_SHELL, Target(path=getattr(args, "cwd") or "."), content=cmd)

    async def _start(self, args: BaseModel, ctx: ToolContext) -> ProcHandle:
        cmd = str(getattr(args, "command"))
        cwd = guard.working_directory(getattr(args, "cwd"), ctx)
        network = guard.fetches_network(cmd)
        return await self.shells.start(["/bin/sh", "-c", cmd], cwd, ctx.task_id,
                                       bool(getattr(args, "background")), network)

    async def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        h = await self._start(args, ctx)
        return ToolResult({"id": h.id, "running": self.shells.info(h).running, "exit_code": None})


async def _output(shells: ShellManager, h: ProcHandle, *, since: int = 0) -> ToolResult:
    text, offset, total = await shells.read(h, since, TAIL)
    info = shells.info(h)
    return ToolResult({"id": h.id, "output": text, "offset": offset, "total": total, "more": offset < total,
                       "running": info.running, "exit_code": info.exit_code, "capture_error": info.capture_error,
                       "network_annotation": {"inferred": info.network, "certainty": "heuristic_only"},
                       "retrieval": "shell.read with since=offset until more=false" if offset < total else None},
                      is_error=info.exit_code not in (None, 0) or bool(info.capture_error),
                      read_sources=_untrusted(info.network, info.command))


class ShellRun(ShellStart):
    name = "shell.run"
    description = ("Run a shell command (/bin/sh), waiting up to timeout_s. A live process returns its id; use "
                   "shell.wait to await its exit or shell.read for output. For immediate asynchronous start use "
                   "shell.start, without nohup or trailing &. background=true survives task completion (servers).")
    origin = OUTPUT_ORIGIN

    class Input(ShellStart.Input):
        timeout_s: float = Field(default=30, ge=1, le=120)

    async def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        h = await self._start(args, ctx)
        code = await self.shells.wait(h, float(getattr(args, "timeout_s")))
        text, offset, total = await self.shells.tail(h, TAIL)
        out = {"id": h.id, "running": code is None, "exit_code": code, "output": text, "offset": offset,
               "total": total}
        if total > TAIL:
            out["output_truncated"] = True              # the end is shown; the whole output pages from the start
            out["next"] = "shell.read with since=0 reads from the beginning, a page at a time"

        info = self.shells.info(h)
        out["capture_error"] = info.capture_error
        return ToolResult(out, is_error=code not in (None, 0) or bool(info.capture_error),
                          read_sources=_untrusted(info.network, info.command))


class ShellWait(_Base):
    name, description = ("shell.wait", "Wait for a tracked command to exit, returning early when it finishes. "
                         "A timeout leaves it running; inspect output or wait again. Returns exit_code and output "
                         "from since, so failures are visible without sleeping blindly. Does not launch a retry.")
    risk, reversible = RiskLevel.LOW, True
    observation = True                                 # only reads: the final checker may use it (V3, W4)
    origin = OUTPUT_ORIGIN

    class Input(BaseModel):
        id: str
        timeout_s: float = Field(default=30, ge=0, le=120)
        since: int = Field(default=0, ge=0)

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(Kind.READ)

    async def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        h = self.shells.handle(str(getattr(args, "id")), ctx.task_id)
        await self.shells.wait(h, float(getattr(args, "timeout_s")))
        return await _output(self.shells, h, since=int(getattr(args, "since")))


class ShellRead(_Base):
    name, description = ("shell.read", "Read a command's output from a byte offset, a page at a time: start at "
                         "since=0 and continue from the returned offset while more is true.")
    risk, reversible = RiskLevel.LOW, True
    observation = True                                 # only reads: the final checker may use it (V3, W4)
    origin = OUTPUT_ORIGIN

    class Input(BaseModel):
        id: str
        since: int = Field(default=0, ge=0)

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(Kind.READ)

    async def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        h = self.shells.handle(str(getattr(args, "id")), ctx.task_id)
        return await _output(self.shells, h, since=int(getattr(args, "since")))


class ShellSend(_Base):
    name, description = "shell.send", "Send text (e.g. an answer plus a newline) to a running command's input."
    on_screen = True                  # the command reading it can act on the screen

    class Input(BaseModel):
        id: str
        text: str

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(Kind.RUN_SHELL, content=str(getattr(args, "text")))

    async def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        await self.shells.send(self.shells.handle(str(getattr(args, "id")), ctx.task_id), str(getattr(args, "text")))
        return ToolResult({"sent": True})


class ShellKill(_Base):
    name, description = "shell.kill", "Stop one of this task's commands (and its children)."
    on_screen = True                  # ending a process can close its windows
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
    observation = True                                 # only reads: the final checker may use it (V3, W4)
    origin = "ferb"                                    # FERB's own process records

    class Input(BaseModel):
        pass

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(Kind.READ)

    async def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        return ToolResult({"processes": [{"id": p.id, "command": p.command, "running": p.running,
                                          "exit_code": p.exit_code, "background": p.long_lived}
                                         for p in self.shells.list() if p.owner_task == ctx.task_id]})
