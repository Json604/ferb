"""Packaged owner acceptance definitions. Listing a check never executes it or implies a pass."""
import tomllib
from pathlib import Path
from typing import Any


CATALOGUE = Path(__file__).parent / 'sets' / 'owner_acceptance.toml'


def checklist() -> dict[str, Any]:
    result = tomllib.loads(CATALOGUE.read_text())
    result['security_definitions'] = tomllib.loads((CATALOGUE.parent / 'security_full.toml').read_text())
    return result
