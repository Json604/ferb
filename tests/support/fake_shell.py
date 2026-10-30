"""A stand-in for `shell.run` with the real tool's arguments and kind, recording the escape grant the gateway hands it
(ctx.escape) instead of running anything: the escape hatch's questions are tested through the gateway's one
authorization step without starting a sandboxed process."""
from typing import Any

from pydantic import BaseModel

from ferb.core.types import ActionSpec, Identity, RiskLevel, Target, ToolContext, ToolResult
from ferb.policy.rules import Kind


class FakeShell:
    name, description, scopes = "shell.run", "fake shell.run", {"shell"}
    identities, risk, reversible = {Identity.OWNER, Identity.FERB}, RiskLevel.MEDIUM, False
    target_args = ("cwd",)
    method_args = ("escape",)

    class Input(BaseModel):
        command: str
        cwd: str | None = None
        escape: str | None = None

    def __init__(self) -> None:
        self.grants: list[Any] = []

    def describe(self, args: Any) -> ActionSpec:
        return ActionSpec(Kind.RUN_SHELL, Target(path=args.cwd or "."), content=args.command)

    async def run(self, args: Any, ctx: ToolContext) -> ToolResult:
        self.grants.append(ctx.escape)
        return ToolResult({"ok": True, "exit_code": 0})
