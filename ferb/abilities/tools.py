"""Observation-only adapters for the built-in catalog. Execution belongs to the gateway-backed runner."""
from pydantic import BaseModel, ConfigDict

from ferb.abilities.catalog import AbilityCatalog
from ferb.core.types import ActionSpec, Identity, RiskLevel, ToolContext, ToolResult


class AbilitiesList:
    name = 'abilities.list'
    description = ('List packaged FERB abilities, including browser verification, with their versions and '
                   'execution availability. Read a procedure with abilities.read when relevant.')
    scopes: set[str] = set()
    identities = {Identity.OWNER, Identity.FERB}
    risk, reversible, observation, origin = RiskLevel.LOW, True, True, 'ferb'

    class Input(BaseModel):
        model_config = ConfigDict(extra='forbid')

    def __init__(self, catalog: AbilityCatalog) -> None:
        self.catalog = catalog

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec('read')

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        return ToolResult({'abilities': self.catalog.catalog()})


class AbilitiesRead:
    name = 'abilities.read'
    description = ('Read the packaged procedure for an installed FERB ability, including browser verification. '
                   'Returns metadata and execution availability; this read never starts the ability.')
    scopes: set[str] = set()
    identities = {Identity.OWNER, Identity.FERB}
    risk, reversible, observation, origin = RiskLevel.LOW, True, True, 'ferb'

    class Input(BaseModel):
        model_config = ConfigDict(extra='forbid')
        name: str

    def __init__(self, catalog: AbilityCatalog) -> None:
        self.catalog = catalog

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec('read')

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        a = self.Input.model_validate(args)
        return ToolResult(self.catalog.read(a.name))
