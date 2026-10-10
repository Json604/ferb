"""Packaged owner acceptance definitions. Listing a check never executes it or implies a pass."""
import tomllib
from pathlib import Path
from typing import Any


CATALOGUE = Path(__file__).parent / 'sets' / 'owner_acceptance.toml'


def checklist() -> dict[str, Any]:
    return tomllib.loads(CATALOGUE.read_text())
