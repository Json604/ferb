"""Action ledger (section 7 step 9, section 19). Safety test 21.11: lost acknowledgement after Send -> unknown ->
reconciliation, no duplicate send. Ported from spike 13's ledger semantics."""

from pathlib import Path

import pytest

from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.orchestrator.ledger import BadTransition, DuplicateUnresolved, Ledger
from ferb.store.db import Database

SEND = dict(task_id="t1", capability="gmail.send", target={"to": ["alice@x.com"]}, payload_hash="h-send-1")


@pytest.fixture
def db(tmp_path: Path) -> Database:
    d = Database(tmp_path / "ferb.db")
    d.migrate()
    return d


def test_happy_path(db: Database) -> None:
    led = Ledger(db, FakeClock(), SequentialIds("a"))
    i = led.intend(**SEND)
    assert i.state == "intended" and i.id == "a-000001"
    led.mark(i.id, "executing")
    led.mark(i.id, "done", ["span-9"])
    row = led.get(i.id)
    assert (row.intent.state, row.reconciled, row.evidence) == ("done", False, ["span-9"])


def test_lost_ack_becomes_unknown_blocks_a_duplicate_until_reconciled(db: Database) -> None:
    led = Ledger(db, FakeClock(), SequentialIds("a"))
    i = led.intend(**SEND)
    led.mark(i.id, "executing")
    led.mark(i.id, "unknown", ["connection lost after Send was clicked"])
    with pytest.raises(DuplicateUnresolved):
        led.intend(**SEND)                                   # a blind retry is refused
    with pytest.raises(BadTransition):
        led.mark(i.id, "done")                               # unknown settles only through reconcile
    with pytest.raises(ValueError):
        led.reconcile(i.id, "done", [])                      # evidence required
    led.reconcile(i.id, "done", ["snapshot: message found in Sent"])
    row = led.get(i.id)
    assert (row.intent.state, row.reconciled) == ("done", True)
    assert "snapshot: message found in Sent" in row.evidence
    # reconciled as done: nothing re-sent; a genuinely new send (different payload) is still possible
    led.intend(**{**SEND, "payload_hash": "h-send-2"})


def test_reconciled_as_failed_allows_a_deliberate_retry(db: Database) -> None:
    led = Ledger(db, FakeClock(), SequentialIds("a"))
    i = led.intend(**SEND)
    led.mark(i.id, "executing")
    led.mark(i.id, "unknown")
    led.reconcile(i.id, "failed", ["snapshot: not in Sent, draft still open"])
    assert led.intend(**SEND).state == "intended"


def test_restart_marks_in_flight_actions_unknown(tmp_path: Path) -> None:
    d = Database(tmp_path / "ferb.db")
    d.migrate()
    led = Ledger(d, FakeClock(), SequentialIds("a"))
    a = led.intend(**SEND)
    b = led.intend(**{**SEND, "payload_hash": "h2"})
    led.mark(b.id, "executing")
    c = led.intend(**{**SEND, "payload_hash": "h3"})
    led.mark(c.id, "executing")
    led.mark(c.id, "done")
    d.close()                                                # "crash"
    d2 = Database(tmp_path / "ferb.db")
    led2 = Ledger(d2, FakeClock(), SequentialIds("b"))
    assert sorted(led2.mark_unknown_after_restart()) == sorted([a.id, b.id])
    assert {r.intent.id for r in led2.unknown()} == {a.id, b.id}
    assert led2.get(c.id).intent.state == "done"


def test_illegal_transitions(db: Database) -> None:
    led = Ledger(db, FakeClock(), SequentialIds("a"))
    i = led.intend(**SEND)
    with pytest.raises(BadTransition):
        led.mark(i.id, "done")                               # never done without executing
    led.mark(i.id, "failed")
    with pytest.raises(BadTransition):
        led.mark(i.id, "executing")                          # terminal
