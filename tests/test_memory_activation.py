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

    async def go() -> tuple[list[str], Any]:
        ids = [mem.propose("preference", "owner", "editor", wording, "observed", [("trail", src)]).id
               for (_, wording), src in zip(days, rows)]
        return ids, mem.get(ids[0])
    ids, item = daemon.run(go())
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
        async def start(self, task: Any, system: str, context: str, token: str) -> None:
            seen["context"] = context
            raise RuntimeError("stop after capturing the context")

    distiller = daemon.daemon.distiller
    distiller.make_runtime = lambda *a, **k: Capture()
    try:
        daemon.run(distiller._model("2099-01-04", summary.DayEvidence("2099-01-04", {})))
    except RuntimeError:
        pass
    assert "Patterns seen before, not yet confirmed" in seen["context"]
    assert "editor" in seen["context"]
