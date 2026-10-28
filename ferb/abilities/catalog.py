"""Own built-in discovery and contained resource reads, never imports selected by a tool caller."""
from collections.abc import Callable, Collection, Iterable, Iterator
from contextlib import contextmanager
from importlib import resources
import os
import hashlib
from pathlib import Path
from typing import Any, IO

from ferb.abilities.contracts import AbilityDefinition, ProcedureResource
from ferb.core import origin
from ferb.core.errors import Refused


def builtin_definitions() -> tuple[AbilityDefinition, ...]:
    from ferb.abilities.browser_verification.definition import DEFINITION
    from ferb.abilities.guidance.definitions import DEFINITIONS
    return (DEFINITION, *DEFINITIONS)


@contextmanager
def _open_procedure(resource: ProcedureResource) -> Iterator[IO[str]]:
    """Package APIs also work in a wheel; filesystem resources additionally refuse symlink substitution."""
    root = resources.files(resource.package)
    try:
        if isinstance(root, Path):
            directory = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            try:
                fd = os.open(resource.filename, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=directory)
                with os.fdopen(fd, 'r', encoding='utf-8') as stream:
                    yield stream
            finally:
                os.close(directory)
            return
        with root.joinpath(resource.filename).open('r', encoding='utf-8') as stream:
            yield stream
    except (OSError, UnicodeError) as exc:
        raise Refused('The packaged ability procedure is unavailable or is a symlink.') from exc


class AbilityCatalog:
    def __init__(self, definitions: Iterable[AbilityDefinition] | None = None,
                 capabilities: Callable[[], Collection[str]] = lambda: ()) -> None:
        self._definitions: dict[str, AbilityDefinition] = {}
        self._capabilities = capabilities
        for definition in builtin_definitions() if definitions is None else definitions:
            if definition.id in self._definitions:
                raise ValueError(f'ability {definition.id} registered twice')
            self._definitions[definition.id] = definition

    def _metadata(self, definition: AbilityDefinition) -> dict[str, Any]:
        installed = self._capabilities()
        missing = [name for name in definition.required_capabilities if name not in installed]
        reasons = [definition.unavailable_reason] if definition.unavailable_reason else []
        if missing:
            reasons.append('Required capabilities are not registered: ' + ', '.join(missing) + '.')
        try:
            with _open_procedure(definition.procedure_resource):
                pass                                  # only check availability; the body stays on demand
            readable = True
        except Refused as exc:
            readable = False
            reasons.append(str(exc))
        return {'id': definition.id, 'version': definition.version, 'description': definition.description,
                'source_kind': 'builtin', 'origin': origin.FERB,
                'readable': readable,
                'supported_interactions': list(definition.supported_interactions),
                'required_capabilities': list(definition.required_capabilities),
                'availability': {'available': not reasons, 'missing_capabilities': missing,
                                 'reason': ' '.join(reasons) if reasons else None}}

    def catalog(self) -> list[dict[str, Any]]:
        return [self._metadata(definition) for definition in self._definitions.values()]

    def read(self, name: str) -> dict[str, Any]:
        definition = self._definitions.get(name)
        if definition is None:
            raise Refused('Choose an installed ability ID from abilities.list; paths and modules are not accepted.')
        with _open_procedure(definition.procedure_resource) as stream:
            body = stream.read()
        return {**self._metadata(definition), 'sha256': hashlib.sha256(body.encode('utf-8')).hexdigest(),
                'input_schema': definition.input_schema.model_json_schema(), 'body': body}
