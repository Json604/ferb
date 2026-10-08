"""Own worker question schema. Must not answer unresolved identity questions or create authorization."""

from pydantic import BaseModel, Field

from ferb.core.types import ActionSpec, Identity, RiskLevel, ToolContext, ToolResult
from ferb.policy.rules import Kind

class OwnerAsk:
    name = "owner.ask"
    description = ("Resolve a necessary choice, missing input, or an unrequested final outcome after preparing "
                   "the concrete result. The orchestrator answers from context first; only unresolved questions "
                   "reach owner chat. Returns {answer}, {declined}, or an execution error. Then continue the task.")
    scopes: set[str] = set()
    identities = {Identity.OWNER, Identity.FERB}
    risk, reversible = RiskLevel.LOW, True

    class Input(BaseModel):
        question: str = Field(min_length=1)
        options: list[str] = Field(default_factory=list,
                                   description="Candidate choices with distinguishing evidence; numbered or free-text answer")
        retry_action: str = Field(default="", description=(
            "Only after FERB refused to repeat an action whose earlier outcome is unknown: the retry_action value from "
            "that refusal. Put what you found in question; FERB adds the exact action and Yes/No options, and only the "
            "owner's Yes lets that action run again (returns retry_approved)."))

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(Kind.READ)

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        return ToolResult({"error": "owner.ask is answered by the gateway"}, is_error=True)
