"""Test mode's question recorder (GATE-545, owner 2026-11-08): with test mode's questions set to "record", every
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


# decision: GATE-545, GATE-546
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


# decision: GATE-545, GATE-546
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


# decision: GATE-545, GATE-546
def test_questions_are_asked_again_once_test_mode_is_off(daemon: RunningDaemon) -> None:
    _record_mode(daemon)

    async def off() -> dict[str, Any]:
        return await daemon.daemon._test_mode(None, {"off": True})
    assert daemon.run(off())["questions"] == "ask" and not daemon.daemon.gateway.test_questions


# decision: GATE-545, GATE-546
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
