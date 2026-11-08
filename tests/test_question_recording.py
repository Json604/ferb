"""Test mode's question recorder (GATE-546; the run itself: GATE-549): with test mode's questions set to "record", every
question FERB would ask is recorded with its card and rule and answered from a fixed table in code, never by a model
and never by the owner: the URL sink's Yes for this task tree only (never a known destination), No for everything
else (a payment here), a task's own question declined with the fixed line, a P19 retry No."""
import json
from typing import Any

from ferb.daemon.gateway import TEST_TASK_ANSWER
from ferb.policy import confirm
from tests.conftest import Answers, RunningDaemon, gateway_request, gateway_setup

URL_NEED = confirm.Need("sink:private_url", "sink:private_url:www.overleaf.com",
                        {"destination": "www.overleaf.com", "value": "ceae41", "matched": "ceae41",
                         "source": "a page on mail.google.com", "read": "mail.google.com",
                         "url": "https://www.overleaf.com/latex/templates/ceae41"},
                        destinations=(("host", "www.overleaf.com"),))
PAY_NEED = confirm.Need("floor:pay", "floor:pay:swiggy.com", {"amount": "₹1,299", "merchant": "SWIGGY",
                                                              "site": "swiggy.com", "account": "", "recipients": ""})


def _record_mode(daemon: RunningDaemon) -> None:
    async def on() -> dict[str, Any]:
        return await daemon.daemon._test_mode(None, {"allow_send": ["me@example.com"], "questions": "record"})
    assert daemon.run(on())["questions"] == "record"


def _asked(daemon: RunningDaemon) -> list[dict[str, Any]]:
    async def rows() -> list[dict[str, Any]]:
        return [{"input": json.loads(r[0]), "output": json.loads(r[1])} for r in daemon.daemon.db.conn.execute(
            "SELECT input_json, output_json FROM spans WHERE kind = 'tool.call' AND "
            "json_extract(input_json, '$.tool') = 'owner.ask' ORDER BY rowid")]
    return daemon.run(rows())


# decision: GATE-546
def test_ferbs_questions_are_recorded_with_their_card_and_answered_from_the_table(daemon: RunningDaemon) -> None:
    _record_mode(daemon)
    grant, _ = gateway_setup(daemon, {"web"}, gateway_request("find the overleaf template and pay for lunch"), [])
    rec = daemon.daemon.tasks.get(grant.task_id)
    owner = Answers(daemon, granted=True, choice=1)             # the owner would say Yes: never asked
    try:
        url = daemon.run(daemon.daemon.gateway.ask_confirmation(grant, rec, [URL_NEED], None, "browser.open", ""))
        pay = daemon.run(daemon.daemon.gateway.ask_confirmation(grant, rec, [PAY_NEED], None, "browser.act",
                                                                  "checkout"))
    finally:
        owner.stop()
    assert owner.asked == []                                       # no card reached the owner
    assert url.granted and not pay.granted
    first, second = _asked(daemon)
    assert first["output"]["answer"] == "Yes" and first["output"]["test_mode"] is True
    assert first["output"]["needs"][0]["rule"] == "sink:private_url"
    assert first["output"]["card"]["question"] and "overleaf" in json.dumps(first["output"]["card"])
    assert second["output"]["answer"] == "No" and second["output"]["needs"][0]["rule"] == "floor:pay"
    # the Yes is this task tree's only: the address never becomes a known destination
    async def known() -> bool:
        return bool(daemon.daemon.destinations.approved("host", "www.overleaf.com"))
    assert not daemon.run(known())


# decision: GATE-546
def test_a_tasks_own_question_is_declined_with_the_fixed_line(daemon: RunningDaemon) -> None:
    _record_mode(daemon)
    grant, _ = gateway_setup(daemon, {"web"}, gateway_request("pick a template"), [])
    rec = daemon.daemon.tasks.get(grant.task_id)
    owner = Answers(daemon, granted=True, choice=1)

    class Args:
        question, options, retry_action = "Which template do you want?", ["A", "B"], ""
    try:
        daemon.daemon.gateway.route_question = None              # straight to the owner's side
        got = daemon.run(daemon.daemon.gateway._ask_owner(grant, rec, Args()))
    finally:
        owner.stop()
    assert owner.asked == [] and got == {"declined": True, "reason": TEST_TASK_ANSWER}
    [asked] = _asked(daemon)
    assert asked["output"]["test_mode"] is True and asked["input"]["args"]["question"]


# decision: GATE-546
def test_questions_are_asked_again_once_test_mode_is_off(daemon: RunningDaemon) -> None:
    _record_mode(daemon)

    async def off() -> dict[str, Any]:
        return await daemon.daemon._test_mode(None, {"off": True})
    assert daemon.run(off())["questions"] == "ask" and not daemon.daemon.gateway.test_questions


# decision: GATE-546
def test_outside_test_mode_the_same_yes_makes_the_site_known(daemon: RunningDaemon) -> None:
    """The control for the first test: the owner's own Yes makes the address approved for good (GATE-205)."""
    grant, _ = gateway_setup(daemon, {"web"}, gateway_request("find the overleaf template"), [])
    rec = daemon.daemon.tasks.get(grant.task_id)
    owner = Answers(daemon, granted=True, choice=1)
    try:
        assert daemon.run(daemon.daemon.gateway.ask_confirmation(grant, rec, [URL_NEED], None, "browser.open",
                                                                   "")).granted
    finally:
        owner.stop()

    async def approved() -> bool:
        return bool(daemon.daemon.destinations.approved("host", "www.overleaf.com"))
    assert daemon.run(approved()) and owner.asked


# decision: GATE-546
def test_ferbs_check_in_is_recorded_and_stops_so_a_test_run_never_waits(daemon: RunningDaemon) -> None:
    """The attempt-5 "Keep going?" is asked by the supervisor, not the gateway: in record mode it is recorded and
    answered No at once, so the task stops with what it found instead of waiting for nobody."""
    from ferb.orchestrator.attempts import CHECK_IN_AFTER
    from tests.conftest import chat, install, outcome, report, sup, task, verdict
    _record_mode(daemon)
    failed = json.dumps(report("not_done", reason="The site errored."))
    script = install(daemon, [sup("task", "On it.", task("Book the table"))],
                     {"worker": failed, "worker.verify": verdict(None, next="retry", missing="Not booked.",
                                                                 instructions="Try another way.")})
    result = outcome(chat(daemon, "book the table", timeout=60))
    assert len(script.calls("worker")) == CHECK_IN_AFTER and result["status"] == "failed"
    assert daemon.daemon.state.approvals.pending() == []                     # nothing left for the owner
    [asked] = _asked(daemon)
    assert asked["input"]["question_kind"] == "check_in" and asked["output"] == {
        "test_mode": True, "answer": "No", "declined": False}


# decision: GATE-546
def test_the_browsing_choice_is_recorded_and_the_setting_stays(daemon: RunningDaemon) -> None:
    from ferb.act.browser.profiles import BrowserProfile
    from tests.support.schedules import run
    _record_mode(daemon)
    d = daemon.daemon
    d._browsing_profiles = lambda: [BrowserProfile("chrome", "Profile 1", "Work", "me@work.example")]
    got = run(daemon, lambda: d._browsing(True, "r1"))
    assert "note" in got and d.state.approvals.pending() == []
    assert d.config.get("browsing", "owner_profile", "") == ""
    [asked] = _asked(daemon)
    assert asked["input"]["question_kind"] == "browsing" and asked["output"]["test_mode"] is True


def _tagged(daemon: RunningDaemon, text: str, session: str, connection: str) -> list[dict[str, Any]]:
    """A test run's chat turn, tagged as the run script tags it (GATE-550)."""
    from ferb.cli.client import Client
    from tests.conftest import turn_events
    c = Client(daemon.sock, timeout=60)
    try:
        return turn_events(list(c.stream("chat.send", {"session_id": session, "text": text,
                                                       "test_run": {"run": "r1", "connection": connection}})))
    finally:
        c.close()


# decision: GATE-550, GATE-551
def test_a_second_connections_run_of_the_same_task_sees_nothing_the_first_left(daemon: RunningDaemon) -> None:
    """Owner 2026-11-08: "while runnign smae tasks on all teh provders, there history shouldn collide". The same task
    on a second connection gets no chat, memory, learning or activity the first run left; the first learned nothing."""
    from datetime import datetime
    from ferb.memory.summary import _journal
    from tests.conftest import done, install, sup, task
    from tests.support.schedules import run
    _record_mode(daemon)
    d = daemon.daemon
    words = "find the price of the blue kettle and remember that I like blue kettles"
    planned = json.loads(sup("task", "Looking.", task("Find the blue kettle price"))) | {"remember": [{
        "kind": "fact", "subject": "owner", "key": "kettle_colour", "content": "likes blue kettles",
        "quote": "I like blue kettles"}]}
    script = install(daemon, [json.dumps(planned), json.dumps(planned)],
                     {"worker": done("The blue kettle costs 412 EUR.")})
    _tagged(daemon, words, "qrun-r1-1-codex", "codex")
    _tagged(daemon, words, "qrun-r1-1-anthropic", "anthropic")
    planners = [ctx for _, key, ctx, _ in script.contexts if key == "supervisor"]
    workers = [ctx for _, key, ctx, _ in script.contexts if key == "worker"]
    assert len(planners) == 2 and len(workers) == 2
    assert "412 EUR" not in planners[1] and "412 EUR" not in workers[1]          # nothing of the first run's answer
    assert "likes blue kettles" not in planners[1] + workers[1]                  # nor its memory
    assert not [i for i in run(daemon, lambda: d.memory.items("active")) if i.key == "kettle_colour"]
    assert len(run(daemon, lambda: d.journal.of_kind("test_run.memory_skipped"))) == 2
    trees = run(daemon, lambda: [r[0] for r in d.db.conn.execute("SELECT id FROM tasks WHERE id NOT LIKE '%.%'")])
    assert trees and all(daemon.run(d._capture_card(t, None)) is None for t in trees)       # no learning card
    assert run(daemon, lambda: d.db.conn.execute("SELECT COUNT(*) FROM trail WHERE task_id IS NOT NULL")
               .fetchone()[0]) == 0
    today = datetime.now().astimezone().date().isoformat()
    assert run(daemon, lambda: _journal(d.db, today)) == []                       # the daily distill reads none


# decision: GATE-550
def test_a_test_run_tag_is_refused_outside_record_mode(daemon: RunningDaemon) -> None:
    import pytest
    from ferb.cli.client import RpcError
    with pytest.raises(RpcError, match="test mode"):
        _tagged(daemon, "hello", "qrun-x", "codex")


# decision: GATE-546, RUN-442
def test_no_payment_card_secret_or_code_question_is_ever_answered_yes(daemon: RunningDaemon) -> None:
    """Owner 2026-11-08: "JUST AMEK SURE IT DEOSNT PAY". Every rule a FERB question can come from (the card table's
    list) is answered No by the table except the URL sink's; a secret (a password or card) and a login code are
    declined at once."""
    from ferb.core.types import Identity, ToolContext
    from tests.test_question_cards import RULES
    _record_mode(daemon)
    grant, _ = gateway_setup(daemon, {"web"}, gateway_request("buy the kettle"), [])
    rec = daemon.daemon.tasks.get(grant.task_id)
    owner = Answers(daemon, granted=True, choice=1)
    try:
        answers = {rule: daemon.run(daemon.daemon.gateway.ask_confirmation(
            grant, rec, [confirm.Need(rule, rule + ":k", values)], None, "browser.act", "")).granted
            for rule, values in RULES.items()}
        ctx = ToolContext(grant.task_id, "w1", "req-1", Identity.OWNER, frozenset({"web"}), "dev", None, "/tmp",
                          (), ())
        secret = daemon.run(daemon.daemon.gateway.ask_secret(ctx, "card number on shop.example"))
        code = daemon.run(daemon.daemon.gateway.ask_login_code(ctx, "https://bank.example"))
    finally:
        owner.stop()
    assert owner.asked == []
    assert [rule for rule, yes in answers.items() if yes] == ["sink:private_url"]
    assert answers["floor:pay"] is False and secret is None and code is None
    kinds = [a["input"].get("question_kind") for a in _asked(daemon)]
    assert "secret" in kinds and "login_code" in kinds
