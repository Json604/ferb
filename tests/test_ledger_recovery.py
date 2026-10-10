"""The ledger records what may have happened for recovery, not proof of success (V7)."""
from pathlib import Path

from ferb.act.mac.tools import MacAppleScript
from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.orchestrator.ledger import Ledger
from ferb.policy.rules import Kind
from ferb.store.db import Database
from tests.conftest import FakeCap, RunningDaemon, gateway_request, gateway_setup, gateway_tool_call


def test_unresolved_unknown_rows_are_listed_for_recovery(daemon: RunningDaemon) -> None:
    boom = FakeCap("probe.send", "send", {"probe"}, boom=True)   # raises after "sending": outcome unknown
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("send it to x@y.com"), [boom])
    r = gateway_tool_call(daemon, grant, "probe.send", to="x@y.com")
    assert r["is_error"] and "outcome unknown" in r["content"]["error"]
    async def read() -> list[dict]:
        return daemon.daemon.ledger.unresolved(grant.task_id.split(".", 1)[0])
    rows = daemon.run(read())
    assert len(rows) == 1 and rows[0]["state"] == "unknown"


def test_unresolved_is_scoped_to_the_tree_and_excludes_settled_rows(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    ledger = Ledger(db, FakeClock(), SequentialIds("a"))
    a = ledger.intend("t1.worker", "x.send", {}, "h1")
    b = ledger.intend("t1.worker.sub", "x.send", {}, "h2")
    c = ledger.intend("t10.worker", "x.send", {}, "h3")        # a different tree whose id shares a prefix
    d = ledger.intend("t1.worker", "x.send", {}, "h4")
    for i in (a, b, c, d):
        ledger.mark(i.id, "executing")
    ledger.mark(d.id, "done")
    ids = {r["id"] for r in ledger.unresolved("t1")}
    assert ids == {a.id, b.id}
    assert {r["state"] for r in ledger.unresolved("t1")} == {"executing"}


def test_applescript_stays_consequential() -> None:
    """Code cannot prove a script read-only without judging intent (rule 6); no declared read-only flag exists,
    so every AppleScript is ledgered. V7's 'read-only AppleScripts create no row' is deferred."""
    from types import SimpleNamespace
    tool = MacAppleScript.__new__(MacAppleScript)
    tool._bundle = lambda args: "com.apple.Notes"  # type: ignore[method-assign]
    spec = tool.describe(SimpleNamespace(script="get name of every note"))  # type: ignore[arg-type]
    assert spec.kind is Kind.APPLESCRIPT
