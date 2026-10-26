"""Definition contracts for explicitly registered abilities. No executable path comes from model input."""
from dataclasses import dataclass
import re
from types import ModuleType

from pydantic import BaseModel


@dataclass(frozen=True)
class ProcedureResource:
    """A resource in an already imported, reviewed package, never a module name supplied by a caller."""
    package: ModuleType
    filename: str = 'SKILL.md'

    def __post_init__(self) -> None:
        if not re.fullmatch(r'[A-Za-z0-9_-]+\.[A-Za-z0-9]+', self.filename):
            raise ValueError('A procedure resource must be a single file in its package.')


@dataclass(frozen=True)
class AbilityDefinition:
    id: str
    version: str
    description: str
    procedure_resource: ProcedureResource
    input_schema: type[BaseModel]
    supported_interactions: tuple[str, ...]
    required_capabilities: tuple[str, ...]
    unavailable_reason: str | None = None

    def __post_init__(self) -> None:
        if not re.fullmatch(r'[a-z][a-z0-9_]*', self.id):
            raise ValueError('An ability ID must be a lowercase name, not a path or module.')
