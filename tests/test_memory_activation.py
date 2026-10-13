"""Observed patterns accumulate evidence by key across days and activate on the third day."""
from typing import Any

from tests.conftest import RunningDaemon, trail_row
from ferb.memory import summary


def test_same_key_different_wording_on_three_days_activates_once(daemon: RunningDaemon) -> None:
    mem = daemon.daemon.memory

    days = (("2099-01-01", "Codes in VS Code most evenings"),
            ("2099-01-02", "Uses VS Code for coding in the evening"),
            ("2099-01-03", "Evening coding happens in VS Code"))
    rows = [trail_row(daemon, day) for day, _ in days]

    async def go() -> tuple[list[str], list[str], Any]:
        ids, statuses = [], []
        for (_, wording), src in zip(days, rows):
            it = mem.propose("preference", "owner", "editor", wording, "observed", [("trail", src)])
            ids.append(it.id)
            statuses.append(it.status)
        return ids, statuses, mem.get(ids[0])
    ids, statuses, item = daemon.run(go())
    assert statuses == ["proposed", "proposed", "active"]
    assert len(set(ids)) == 1
    assert item.status == "active"


def test_distiller_sees_pending_observed_proposals(daemon: RunningDaemon) -> None:
    mem = daemon.daemon.memory

    row = trail_row(daemon, "2099-01-01")

    async def propose() -> None:
        mem.propose("preference", "owner", "editor", "Codes in VS Code", "observed", [("trail", row)])
    daemon.run(propose())
    seen: dict[str, str] = {}

    class Capture:
        from ferb.core.types import ModelLimits
        limits = ModelLimits("fake-model", 1_000_000, 100_000, "test double")

        async def start(self, task: Any, request: Any, token: str) -> None:
            seen["context"] = request.context
            raise RuntimeError("stop after capturing the context")

    distiller = daemon.daemon.distiller
    distiller.make_runtime = lambda *a, **k: Capture()
    try:
        daemon.run(distiller._model("2099-01-04", summary.DayEvidence("2099-01-04", {})))
    except RuntimeError:
        pass
    assert "Patterns seen before, not yet confirmed" in seen["context"]
    assert "editor" in seen["context"]


def test_changed_pattern_is_a_new_proposal_not_merged_into_the_active_item(daemon: RunningDaemon) -> None:
    mem = daemon.daemon.memory
    rows = [trail_row(daemon, f"2099-02-0{n}") for n in (1, 2, 3, 4)]

    async def go() -> tuple[Any, Any, Any]:
        first = None
        for src in rows[:3]:
            first = mem.propose("preference", "owner", "editor", "Codes in VS Code", "observed", [("trail", src)])
        assert first is not None and first.status == "active"
        new = mem.propose("preference", "owner", "editor", "Codes in Cursor", "observed", [("trail", rows[3])])
        return first, new, mem.get(first.id)
    first, new, old = daemon.run(go())
    assert new.id != first.id and new.status == "proposed"
    assert old.content == "Codes in VS Code" and old.status == "active"


def test_non_observed_origin_with_other_wording_stays_a_separate_item(daemon: RunningDaemon) -> None:
    mem = daemon.daemon.memory
    rows = [trail_row(daemon, "2099-03-01"), trail_row(daemon, "2099-03-02")]

    async def go() -> list[str]:
        return [mem.propose("preference", "owner", "editor", wording, "inferred", [("trail", src)]).id
                for wording, src in (("Codes in VS Code", rows[0]), ("Uses VS Code to code", rows[1]))]
    ids = daemon.run(go())
    assert len(set(ids)) == 2


def test_observation_identical_to_the_active_item_is_not_credited_to_a_pending_reword(daemon: RunningDaemon) -> None:
    mem = daemon.daemon.memory
    rows = [trail_row(daemon, f"2099-05-{n:02d}") for n in range(1, 7)]

    async def go() -> dict[str, Any]:
        active = None
        for src in rows[:3]:
            active = mem.propose("preference", "owner", "editor", "Codes in VS Code", "observed", [("trail", src)])
        assert active is not None and active.status == "active"
        pending = mem.propose("preference", "owner", "editor", "Codes in Cursor", "observed", [("trail", rows[3])])
        again = [mem.propose("preference", "owner", "editor", "Codes in VS Code", "observed", [("trail", src)]).id
                 for src in rows[4:]]
        return {"active": active, "pending": mem.get(pending.id), "again": again,
                "active_items": [i.content for i in mem.items("active")]}
    out = daemon.run(go())
    assert out["again"] == [out["active"].id] * 2
    assert out["pending"].status == "proposed"
    assert out["active_items"] == ["Codes in VS Code"]
