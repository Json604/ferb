import tomllib
from pathlib import Path

from tests.support.decision_ids import plan_decision_ids

PLAN = Path(__file__).resolve().parents[1] / "docs" / "0.27_PLAN.md"
REGISTRY = Path(__file__).resolve().parents[1] / "docs" / "0.27_DECISIONS.toml"


def test_parser_finds_known_ids() -> None:
    ids = plan_decision_ids(PLAN)
    assert {"D1", "D23", "D32", "MP1", "P1", "P31", "S1", "S10", "W1", "W4"} <= ids


def test_every_decided_id_has_a_registry_entry_with_a_milestone() -> None:
    ids = plan_decision_ids(PLAN)
    registry = tomllib.loads(REGISTRY.read_text())
    missing = sorted(i for i in ids if i not in registry)
    assert not missing, f"Decided IDs with no registry entry: {missing}"
    no_milestone = sorted(i for i in ids if not registry.get(i, {}).get("milestone"))
    assert not no_milestone, f"Decided IDs with no milestone: {no_milestone}"


def test_every_registry_test_is_a_real_file() -> None:
    registry = tomllib.loads(REGISTRY.read_text())
    missing = [(ident, test) for ident, row in registry.items() for test in row.get('tests', [])
               if not (REGISTRY.parents[1] / test).is_file()]
    assert not missing, f'Registry points at absent tests: {missing}'
