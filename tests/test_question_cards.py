"""FERB's question cards (GATE-513, refining GATE-225): each of FERB's own questions is the question in plain words,
why FERB asks, and the exact thing approved in its own box. Every rule a question can come from gets words, with its
facts and with none; a card that can't be built leaves the question its old line, never blank and never unasked; the
app gets the parts in `approval.requested` and `chat.status`."""

from typing import Any

import pytest

from ferb.policy import confirm
from tests.conftest import Answers, RunningDaemon, gateway_request, gateway_setup

# Every rule a question can come from (confirm.Need's rules across FERB), with the facts its code fills
RULES: dict[str, dict[str, str]] = {
    confirm.TLS_OFF: {"command": "curl -k https://x.example", "hosts": "x.example", "how": "-k"},
    confirm.TERMINAL_INPUT: {"app": "Terminal", "surface": "a terminal app", "text": '"ls -la"', "enter": "yes"},
    confirm.FOREGROUND: {"app": "Safari"},
    confirm.BROWSER_PAGE: {"app": "Chrome", "page": "chrome://extensions"},
    confirm.PAIR: {"browser": "Chrome", "name": "Work", "email": "me@work.example"},
    "floor:pay": {"amount": "₹1,299", "merchant": "SWIGGY", "site": "swiggy.com", "account": "HDFC ••••12",
                  "recipients": ""},
    "floor:account_security": {"site": "bank.example", "account": "me@bank.example"},
    "floor:delete_permanent": {"path": "/Users/k/Downloads/old.zip"},
    "floor:new_recipient": {"recipients": "rahul@example.com", "verb": "sends to"},
    "sink:new_destination": {"destination": "forms.example.com", "read": "a page on mail.google.com"},
    confirm.SINK_DESTINATION: {"text": "reaching forms.example.com"},
    "sink:private_url": {"destination": "www.overleaf.com", "value": "ceae41", "matched": "ceae41",
                         "source": "the result of browser.snapshot on mail.google.com", "read": "mail.google.com",
                         "url": "https://www.overleaf.com/latex/templates/ceae41"},
    "sink:outside_value": {"what": "typed", "value": "4417 1234 5678", "origin": "a page on example.com"},
    confirm.BULK: {"what": "send", "count": "3", "bytes": "2 KB", "sample": '"a", "b", "c"'},
    "escape:read": {"text": "let FERB read ~/Library/Keychains for this task", "lift": "read",
                    "path": "~/Library/Keychains"},
}


def _need(rule: str, values: dict[str, str]) -> confirm.Need:
    return confirm.Need(rule, rule + ":k", values)


# decision: GATE-513
@pytest.mark.parametrize("rule", sorted(RULES))
def test_every_rule_has_a_plain_question_with_its_facts_and_with_none(rule: str) -> None:
    made = confirm.card([_need(rule, RULES[rule])])
    assert made.question.strip() and "browser.snapshot" not in made.question + made.detail + made.exact
    bare = confirm.card([_need(rule, {})])                  # every fact missing: still words, never an error
    assert bare.question.strip()


# decision: GATE-513
def test_the_overleaf_question_reads_plainly_with_the_address_in_its_box() -> None:
    made = confirm.card([_need("sink:private_url", RULES["sink:private_url"])])
    assert made.question == ("Open this page on www.overleaf.com? Its address includes something FERB read on "
                             "mail.google.com.")
    assert made.detail == 'FERB found "ceae41" there, and opening the page sends it to www.overleaf.com.'
    assert made.exact == "https://www.overleaf.com/latex/templates/ceae41"


# decision: GATE-513
def test_several_needs_make_one_question_with_every_exact_part() -> None:
    made = confirm.card([_need("floor:new_recipient", RULES["floor:new_recipient"]),
                         _need("sink:outside_value", {"what": "recipient", "value": "rahul@example.com",
                                                      "origin": "an email"})], typed='"see you at 5"')
    assert made.question == "Send to rahul@example.com?"
    assert "Send to this recipient?" in made.detail and 'Text last typed there: ""see you at 5""' in made.detail
    assert made.exact.split("\n") == ["To: rahul@example.com", "rahul@example.com · found in an email"]


def _ask(daemon: RunningDaemon, needs: list[confirm.Need]) -> Any:
    grant, _ = gateway_setup(daemon, {"mail:send"}, gateway_request("Tidy up and send the report."), [])
    rec = daemon.daemon.tasks.get(grant.task_id)
    answers = Answers(daemon, choice=2)
    try:
        daemon.run(daemon.daemon.gateway.ask_confirmation(grant, rec, needs, None, "mail.send", "it is fine"))
    finally:
        answers.stop()
    [ap] = answers.asked
    return ap


# decision: GATE-513
def test_a_card_that_cant_be_built_still_asks_with_todays_line(daemon: RunningDaemon,
                                                               monkeypatch: pytest.MonkeyPatch) -> None:
    def broken(*_: Any, **__: Any) -> Any:
        raise KeyError("recipients")
    monkeypatch.setattr(confirm, "card", broken)
    need = _need("floor:new_recipient", RULES["floor:new_recipient"])
    ap = _ask(daemon, [need])
    assert ap.summary == confirm.summary([need]) and ap.detail == "" and ap.exact == ""
    async def read() -> list[Any]:
        return list(daemon.daemon.journal.of_kind("confirm.card_failed"))
    problems = daemon.run(read())
    assert problems and problems[-1].data["rules"] == ["floor:new_recipient"]


# decision: GATE-513
def test_the_app_gets_the_why_line_and_the_exact_part(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    published: list[dict[str, Any]] = []
    gw = daemon.daemon.gateway
    real = gw.publish
    monkeypatch.setattr(gw, "publish", lambda topic, data: (published.append(data) if topic == "approval.requested"
                                                            else None, real(topic, data))[1])
    ap = _ask(daemon, [_need("floor:new_recipient", RULES["floor:new_recipient"])])
    event = next(e for e in published if e["approval_id"] == ap.id)
    assert (event["summary"], event["detail"], event["exact"]) == (
        "Send to rahul@example.com?", "You haven't named this person to FERB before.", "To: rahul@example.com")
