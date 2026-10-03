"""Search the setup-installed action registry without inferring intent or permission."""

from typing import Any, Callable

from pydantic import BaseModel, Field

from ferb.core.types import ActionSpec, Identity, RiskLevel, ToolContext, ToolResult
from ferb.sense.daily import terms


def search_actions(caps: dict[str, Any], query: str, limit: int = 8) -> list[dict[str, Any]]:
    wanted = terms(query)
    ranked = []
    for name, cap in caps.items():
        score = len(wanted & terms(name.replace('.', ' ') + ' ' + cap.description))
        if score or not wanted:
            ranked.append((score, name, cap))
    ranked.sort(key=lambda hit: (-hit[0], hit[1]))
    return [{"name": name, "description": cap.description, "scopes": sorted(cap.scopes),
             "accounts": sorted(i.value for i in cap.identities)} for _, name, cap in ranked[:limit]]


class ActionsSearch:
    name = 'actions.search'
    description = 'Search FERB’s installed action registry by capability or goal. Use this before declaring a capability gap.'
    identities = {Identity.OWNER, Identity.FERB}
    scopes: set[str] = set()
    risk, reversible = RiskLevel.LOW, True

    class Input(BaseModel):
        query: str = ''
        limit: int = Field(12, ge=1, le=100)

    def __init__(self, capabilities: Callable[[], dict[str, Any]]) -> None:
        self.capabilities = capabilities

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec('read')

    async def run(self, args: Any, ctx: ToolContext) -> ToolResult:
        return ToolResult({'actions': search_actions(self.capabilities(), args.query, args.limit)})
