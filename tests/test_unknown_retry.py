"""A consequential action whose outcome is unknown is never retried without the owner (P19).

Code refuses the repeat (CLAUDE.md rules 5 and 9). The only way past is the owner's Yes to a question FERB itself
attaches to that exact action (owner.ask with retry_action); a worker can't settle it on its own word."""
import json
from datetime import timedelta
from pathlib import Path
from typing import Any

import pytest

from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.core.types import Identity, Task
from ferb.orchestrator.ledger import DuplicateUnresolved, Ledger
from ferb.orchestrator.tree import TaskRecord
from ferb.store.db import Database
from tests.conftest import Answers, FakeCap, RunningDaemon, gateway_request, gateway_setup, gateway_tool_call

APPROVED = "owner approved retry after inspection"


def _rows(d: RunningDaemon, capability: str) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        return [dict(r) for r in d.daemon.db.conn.execute(
            "SELECT id, task_id, state, reconciled, evidence_json FROM action_ledger WHERE capability = ? "
            "ORDER BY created_at, rowid", (capability,)).fetchall()]
    return d.run(go())


def _spans(d: RunningDaemon, tool: str) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        return [dict(r) for r in d.daemon.db.conn.execute(
            "SELECT id, error, origin, output_json FROM spans WHERE kind = 'tool.call' "
            "AND json_extract(input_json, '$.tool') = ? ORDER BY rowid", (tool,)).fetchall()]
    return d.run(go())


def _pay_setup(d: RunningDaemon) -> tuple[Any, FakeCap]:
    boom = FakeCap("probe.pay", "pay", {"probe"}, boom=True)     # raises after "paying": outcome unknown
    grant, _ = gateway_setup(d, {"probe"}, gateway_request("pay x@y.com $5"), [boom])
    return grant, boom


def _refused(d: RunningDaemon, grant: Any) -> dict[str, Any]:
    """The first payment's outcome is unknown; the identical retry is refused. Returns the refusal."""
    first = gateway_tool_call(d, grant, "probe.pay", to="x@y.com", text="pay $5")
    assert first["is_error"] and "outcome unknown" in first["content"]["error"]
    second = gateway_tool_call(d, grant, "probe.pay", to="x@y.com", text="pay $5")
    assert second["is_error"] and second["content"].get("retry_action")
    return second


def _ask(d: RunningDaemon, grant: Any, owner: Answers | None, **args: Any) -> dict[str, Any]:
    try:
        return gateway_tool_call(d, grant, "owner.ask", **args)
    finally:
        if owner is not None:
            owner.stop()


def _second_worker(d: RunningDaemon, scopes: set[str]) -> Any:
    """Another worker in the same task tree as gateway_setup's task-1 (a later attempt or a sub-worker)."""
    task = Task("task-1.again", "task-1", "g", "dev", Identity.OWNER, scopes, "worker", "req-1", [],
                d.daemon.clock.now() + timedelta(minutes=10))

    async def go() -> Any:
        root = d.daemon.tasks.get("task-1")
        d.daemon.tasks.add(TaskRecord(task, d.daemon.clock.now(), root.span_id, "task-1"))
        return d.daemon.tokens.validate(d.daemon.tokens.issue(task.id, "w2", scopes, Identity.OWNER, ttl_s=600))
    return d.run(go())


# ---------- the refusal ----------

def test_an_unknown_consequential_action_is_not_retried_without_the_owner(daemon: RunningDaemon) -> None:
    grant, boom = _pay_setup(daemon)
    second = _refused(daemon, grant)
    e = second["content"]["error"].lower()
    assert "unknown" in e and ("owner" in e or "ask" in e or "inspect" in e)
    assert boom.calls == 1                                        # it never ran a second time
    assert second["footer"].endswith("\nOrigin: ferb")
    for internal in ("ledger", "reconcile", "payload", "duplicateunresolved", "intent"):   # P27: owner-safe words
        assert internal not in e
    assert "owner_ask" in second["content"]["next"]               # the agent is told the way past
    assert [r["state"] for r in _rows(daemon, "probe.pay")] == ["unknown"]   # no row for the refused call
    spans = _spans(daemon, "probe.pay")                           # the refusal is recorded
    assert len(spans) == 2 and spans[1]["error"] == second["content"]["error"]
    assert second["content"]["retry_action"] in spans[1]["output_json"]


def test_a_reversible_action_is_not_blocked(daemon: RunningDaemon) -> None:
    # A read never gets a ledger row, so it's never blocked by this rule, even after it raised.
    read = FakeCap("probe.read", "read", {"probe"}, boom=True)
    grant, _ = gateway_setup(daemon, {"probe"}, None, [read])
    gateway_tool_call(daemon, grant, "probe.read")
    gateway_tool_call(daemon, grant, "probe.read")
    assert read.calls == 2 and _rows(daemon, "probe.read") == []


def test_a_different_target_is_allowed(daemon: RunningDaemon) -> None:
    grant, boom = _pay_setup(daemon)
    gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")
    # A different recipient is a different payment: not the unknown one, so not blocked by P19 (F5 may still confirm).
    other = gateway_tool_call(daemon, grant, "probe.pay", to="z@y.com", text="pay $5")
    assert "outcome unknown" in other["content"]["error"]         # it ran (and raised), not blocked
    assert boom.calls == 2


def test_a_changed_annotation_is_still_the_same_action(daemon: RunningDaemon) -> None:
    """`effect` and `why` are the agent's notes, never a gate: changing them doesn't make it a new action."""
    grant, boom = _pay_setup(daemon)
    gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")
    again = gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5", effect="pay",
                              why="the first one may not have gone through")
    assert again["is_error"] and again["content"].get("retry_action") and boom.calls == 1


def test_a_retry_from_another_worker_in_the_same_tree_is_refused(daemon: RunningDaemon) -> None:
    grant, boom = _pay_setup(daemon)
    gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")
    later = _second_worker(daemon, {"probe"})
    again = gateway_tool_call(daemon, later, "probe.pay", to="x@y.com", text="pay $5")
    assert again["is_error"] and again["content"].get("retry_action") and boom.calls == 1


def test_an_identical_action_still_running_in_the_tree_is_refused(daemon: RunningDaemon) -> None:
    pay = FakeCap("probe.pay", "pay", {"probe"})
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("pay z@y.com $5"), [pay])
    first = gateway_tool_call(daemon, grant, "probe.pay", to="z@y.com", text="pay $5")     # done: the hash to copy
    row = _rows(daemon, "probe.pay")[0]

    async def running() -> str:          # the same payment, still executing for another worker of the tree
        ledger = daemon.daemon.ledger
        h = ledger.get(row["id"]).intent.payload_hash
        i = ledger.intend("task-1.other", "probe.pay", {"recipients": ["z@y.com"]}, h, reconcile_first=False)
        ledger.mark(i.id, "executing")
        return i.id
    assert not first["is_error"] and row["state"] == "done"
    busy = daemon.run(running())
    owner = Answers(daemon, choice=1)
    try:
        again = gateway_tool_call(daemon, grant, "probe.pay", to="z@y.com", text="pay $5")
        assert again["is_error"] and "already running" in again["content"]["error"]
        assert "retry_action" not in again["content"]
        # an approval can't be offered for an action that hasn't finished
        asked = gateway_tool_call(daemon, grant, "owner.ask", question="Pay again?", retry_action=busy)
    finally:
        owner.stop()
    assert asked["is_error"] and "still running" in asked["content"]["error"] and owner.asked == []
    assert pay.calls == 1


# ---------- the ledger's duplicate check ----------

def test_the_duplicate_check_covers_the_whole_tree_and_running_rows(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    ledger = Ledger(db, FakeClock(), SequentialIds("a"))
    a = ledger.intend("t1.worker", "x.send", {}, "h1")
    ledger.mark(a.id, "executing")
    with pytest.raises(DuplicateUnresolved) as running:
        ledger.intend("t1.worker.verify", "x.send", {}, "h1")         # still executing, in the same tree
    assert (running.value.intent_id, running.value.state) == (a.id, "executing")
    ledger.mark(a.id, "unknown", ["lost the acknowledgement"])
    for task in ("t1", "t1.worker", "t1.dispatch.again"):
        with pytest.raises(DuplicateUnresolved) as unknown:
            ledger.intend(task, "x.send", {}, "h1")
        assert (unknown.value.intent_id, unknown.value.state) == (a.id, "unknown")
    assert ledger.intend("t10.worker", "x.send", {}, "h1").task_id == "t10.worker"   # another tree, shared prefix
    assert ledger.intend("t1.worker", "x.send", {}, "h2").state == "intended"        # another payload
    assert ledger.intend("t1.worker", "x.post", {}, "h1").state == "intended"        # another capability
    assert ledger.get(a.id).intent.task_id == "t1.worker"                             # the stored key is unchanged
    ledger.reconcile(a.id, "failed", [APPROVED])
    assert ledger.intend("t1.worker", "x.send", {}, "h1").state == "intended"
    ledger.intend("t2.worker", "x.send", {}, "h9")                    # written, not yet marked executing
    with pytest.raises(DuplicateUnresolved) as pending:
        ledger.intend("t2.worker", "x.send", {}, "h9")
    assert pending.value.state == "intended"


# ---------- the owner's approval ----------

def test_the_owners_yes_lets_it_run_again(daemon: RunningDaemon) -> None:
    grant, boom = _pay_setup(daemon)
    ref = _refused(daemon, grant)["content"]["retry_action"]
    owner = Answers(daemon, choice=1)
    answer = _ask(daemon, grant, owner, question="I checked the order page and no payment shows. Pay again?",
                  options=["Sure", "Never"], retry_action=ref)
    assert answer["content"] == {"answer": "Yes", "retry_approved": True}
    assert answer["footer"].endswith("\nOrigin: owner")
    shown = owner.asked[0]
    assert shown.options == ("Yes", "No")                          # FERB's options, not the worker's
    assert shown.reason.startswith("I checked the order page")    # the worker's findings, then FERB's concrete line
    assert "probe.pay to x@y.com" in shown.reason and "twice" in shown.reason
    third = gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")
    assert "outcome unknown" in third["content"]["error"] and boom.calls == 2      # it ran again
    rows = _rows(daemon, "probe.pay")
    assert [(r["state"], r["reconciled"]) for r in rows] == [("failed", 1), ("unknown", 0)]
    ask_span = _spans(daemon, "owner.ask")[0]
    assert ask_span["origin"] == "owner" and APPROVED in rows[0]["evidence_json"]
    assert ask_span["id"] in rows[0]["evidence_json"] and "retry_approved" in ask_span["output_json"]
    # the new attempt's outcome is unknown too: it needs the owner again
    assert gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")["content"].get("retry_action")
    assert boom.calls == 2


def test_a_worker_cannot_approve_its_own_retry(daemon: RunningDaemon) -> None:
    grant, boom = _pay_setup(daemon)
    ref = _refused(daemon, grant)["content"]["retry_action"]
    pay = dict(to="x@y.com", text="pay $5")

    def still_refused() -> None:
        r = gateway_tool_call(daemon, grant, "probe.pay", **pay)
        assert r["is_error"] and r["content"].get("retry_action") == ref and boom.calls == 1

    # An owner's yes to some other question is not an approval of this action.
    yes = _ask(daemon, grant, Answers(daemon, choice=1), question="Pay again?", options=["Yes", "No"])
    assert yes["content"] == {"answer": "Yes"}
    still_refused()
    # The orchestrator can't approve it: a retry question goes to the owner, never answered on the owner's behalf.
    consulted: list[str] = []

    async def orchestrator(task_id: str, question: str, options: list[str]) -> dict[str, Any]:
        consulted.append(question)
        return {"ask_owner": False, "answer": "Yes", "question": ""}
    router = daemon.daemon.gateway.route_question
    daemon.daemon.gateway.route_question = orchestrator
    inferred = _ask(daemon, grant, None, question="Pay again?", options=["Yes", "No"])
    assert inferred["content"] == {"answer": "Yes", "answered_by": "orchestrator"} and consulted
    still_refused()
    consulted.clear()
    owner = Answers(daemon, choice=2)                                 # the owner says No
    no = _ask(daemon, grant, owner, question="Pay again?", retry_action=ref)
    assert no["content"] == {"answer": "No", "retry_approved": False} and consulted == [] and len(owner.asked) == 1
    still_refused()
    daemon.daemon.gateway.route_question = router
    skipped = _ask(daemon, grant, Answers(daemon, choice=None), question="Pay again?", retry_action=ref)  # Enter
    assert skipped["content"] == {"declined": True}
    still_refused()
    daemon.daemon.state.approvals.unattended.add("req-1")             # a scheduled run: nobody at chat
    assert _ask(daemon, grant, None, question="Pay again?", retry_action=ref)["content"]["declined"]
    daemon.daemon.state.approvals.unattended.discard("req-1")
    still_refused()
    # A made-up reference, or another tree's unknown action, is refused before anyone is asked.
    async def elsewhere() -> str:
        ledger = daemon.daemon.ledger
        i = ledger.intend("other-tree.worker", "probe.pay", {}, "h", reconcile_first=False)
        ledger.mark(i.id, "executing")
        ledger.mark(i.id, "unknown")
        return i.id
    foreign = daemon.run(elsewhere())
    owner = Answers(daemon, choice=1)
    try:
        for bad in ("not-a-row", foreign):
            r = gateway_tool_call(daemon, grant, "owner.ask", question="Pay again?", retry_action=bad)
            assert r["is_error"] and "Origin: ferb" in r["footer"]
    finally:
        owner.stop()
    assert owner.asked == []
    assert [r["state"] for r in _rows(daemon, "probe.pay")] == ["unknown", "unknown"]   # nothing settled
    still_refused()


def test_an_attempt_settled_while_the_owner_was_asked_is_reported(daemon: RunningDaemon) -> None:
    """The browser extension can report the earlier attempt after reconnecting while the question is open: the
    owner's Yes then doesn't settle it again, and the worker is told what happened."""
    import asyncio
    grant, boom = _pay_setup(daemon)
    ref = _refused(daemon, grant)["content"]["retry_action"]

    async def owner_after_the_extension() -> None:
        approvals = daemon.daemon.state.approvals
        while not approvals.pending():
            await asyncio.sleep(0.02)
        daemon.daemon.ledger.reconcile(ref, "done", ["the extension reported it done after reconnecting"])
        approvals.resolve(approvals.pending()[0].id, True, 1)
    answering = asyncio.run_coroutine_threadsafe(owner_after_the_extension(), daemon.loop)
    answer = gateway_tool_call(daemon, grant, "owner.ask", question="Pay again?", retry_action=ref)
    answering.result(timeout=5)
    assert answer["content"] == {"answer": "Yes", "retry_approved": False, "earlier_attempt": "done"}
    row = _rows(daemon, "probe.pay")[0]
    assert row["state"] == "done" and APPROVED not in row["evidence_json"]
    again = gateway_tool_call(daemon, grant, "owner.ask", question="Pay again?", retry_action=ref)
    assert again["is_error"] and "no longer unknown" in again["content"]["error"]
    assert boom.calls == 1


# ---------- recovery context ----------

def test_uncertain_prior_actions_cover_the_whole_tree_and_running_rows(daemon: RunningDaemon) -> None:
    from ferb.orchestrator.schemas import TaskSpec
    from ferb.orchestrator.supervisor import _Tree
    from ferb.policy.targets import Request, Targets
    sup = daemon.daemon.supervisor

    async def go() -> str:
        ledger = daemon.daemon.ledger
        for task_id, site, state in (("tr-1.worker", "a.example", "unknown"), ("tr-1.worker.sub", "b.example", "executing"),
                                     ("prior-1.worker", "c.example", "unknown"), ("tr-10.worker", "d.example", "unknown"),
                                     ("tr-1.worker", "e.example", "done")):
            i = ledger.intend(task_id, "probe.send", {"site": site}, site)
            ledger.mark(i.id, "executing")
            if state != "executing":
                ledger.mark(i.id, state)
        daemon.daemon.state.requests.record(Request("req-ctx", "send it", Targets.of()))
        worker = Task("tr-1.worker", "tr-1", "send it", "dev", Identity.OWNER, set(), "worker", "req-ctx", [], None)
        tree = _Tree("tr-1", TaskSpec(goal="send it", identity="owner"), worker, prior_task="prior-1")
        return next(b.text for b in sup._context(tree) if b.text.startswith("Uncertain prior actions"))
    block = daemon.run(go())
    assert "inspect before deciding whether to retry" in block                    # the wording is kept
    listed = json.loads(block.split("\n")[1])
    assert {(r["target"]["site"], r["status"]) for r in listed} == {
        ("a.example", "unknown"), ("b.example", "executing"), ("c.example", "unknown")}
