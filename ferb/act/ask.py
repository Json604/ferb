"""Worker questions go to the orchestrator first, then to owner chat only when it also needs the answer.
The gateway handles this tool; hidden OTP input uses a separate sealed path."""

from pydantic import BaseModel, Field

from ferb.core.types import ActionSpec, Identity, RiskLevel, ToolContext, ToolResult
from ferb.policy.rules import Kind

MAX_QUESTION, MAX_OPTIONS, MAX_OPTION = 300, 6, 80


class OwnerAsk:
    name = "owner.ask"
    description = ("Resolve a necessary choice, missing input, or an unrequested final outcome after preparing "
                   "the concrete result. The orchestrator answers from context first; only unresolved questions "
                   "reach owner chat. Returns {answer}, {declined}, or an execution error. Then continue the task.")
    scopes: set[str] = set()
    identities = {Identity.OWNER, Identity.FERB}
    risk, reversible = RiskLevel.LOW, True

    class Input(BaseModel):
        question: str = Field(min_length=1, max_length=MAX_QUESTION)
        options: list[str] = Field(default_factory=list, max_length=MAX_OPTIONS,
                                   description="0–6 short choices (the owner picks by number or answers freely)")

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(Kind.READ)

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        return ToolResult({"error": "owner.ask is answered by the gateway"}, is_error=True)
