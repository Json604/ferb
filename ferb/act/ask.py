"""`owner.ask` (docs/DESIGN_ONE_WORKER.md section 4): a worker's question to the owner, answered in `ferb chat`.

Every identity, no scopes. The gateway answers it itself (it never reaches `run`): it files the question as an
approval, publishes `approval.requested` so chat shows it like any other question, waits for the answer, and
returns {"answer": "<option or free text>"} or {"declined": true}. A scheduled run (the owner isn't there) gets
{"declined": true} at once.
"""

from pydantic import BaseModel, Field

from ferb.core.types import ActionSpec, Identity, RiskLevel, ToolContext, ToolResult
from ferb.policy.rules import Kind

MAX_QUESTION, MAX_OPTIONS, MAX_OPTION = 300, 6, 80


class OwnerAsk:
    name = "owner.ask"
    description = ("Ask the owner one question in chat and wait for their answer — only when the choice is genuinely "
                   "theirs: which account or profile, which of two matches, whether to go on when something "
                   "unexpected is on screen. Give short options when there are some; the owner may also answer in "
                   "their own words. Returns {answer} or {declined}. Then carry on with the task.")
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
