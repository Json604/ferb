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
                         "source_plain": "your signed-in mail.google.com page",
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
    assert made.question == "Open www.overleaf.com? The link includes “ceae41”"
    assert made.detail == ("FERB treats “ceae41” as yours because it read it in your signed-in mail.google.com page. "
                           "Opening this link shows it to www.overleaf.com. Yes also lets FERB open www.overleaf.com in "
                           "later tasks.")                                       # what the Yes allows (GATE-205)
    assert made.exact == "https://www.overleaf.com/latex/templates/ceae41"


# decision: GATE-513
def test_several_needs_make_one_question_with_every_exact_part() -> None:
    made = confirm.card([_need("floor:new_recipient", RULES["floor:new_recipient"]),
                         _need("sink:outside_value", {"what": "recipient", "value": "rahul@example.com",
                                                      "origin": "an email"})], typed='"see you at 5"')
    assert made.question == "Send to rahul@example.com for the first time?"
    assert "You never gave FERB this address; it came from an email." in made.detail
    assert 'Text last typed there: ""see you at 5""' in made.detail
    assert made.exact.split("\n") == ["To: rahul@example.com", "rahul@example.com"]


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
        "Send to rahul@example.com for the first time?",
        "You haven't asked FERB to contact this person before; check it's who you meant. Yes also lets FERB send to "
        "this person in later tasks.",
        "To: rahul@example.com")


# decision: GATE-513
# decision: GATE-204
def test_a_payments_code_question_says_what_it_pays() -> None:
    """Owner, 2026-11-07: "otp needed to pay x on y". The place first (re-review R1), then the payment the bank page
    shows; when FERB couldn't read it, the payment the owner approved, and that it couldn't."""
    from datetime import datetime, timezone
    from ferb.policy.payments import Approval, otp_card
    at = datetime(2026, 11, 7, tzinfo=timezone.utc)
    read = Approval(cid="c1", chain="t1", host="orderly.example", amount="₹248.75", merchant="Orderly", at=at,
                    bank_amount="INR 248.75", bank_merchant="ORDERLY")
    question, why, exact = otp_card("secure.bank.example", read)
    assert question == "secure.bank.example: code to pay INR 248.75 to ORDERLY"
    assert exact == "INR 248.75 → ORDERLY · secure.bank.example" and "once" in why
    unread = Approval(cid="c1", chain="t1", host="orderly.example", amount="₹248.75", merchant="Orderly", at=at)
    question, why, exact = otp_card("secure.bank.example", unread)
    assert question == "secure.bank.example: code for the payment you approved (₹248.75 to Orderly)"
    assert why.startswith("FERB couldn't read the amount on this page.")


# decision: GATE-514
# decision: GATE-205
@pytest.mark.parametrize("rule, values, destinations", [
    ("floor:new_recipient", RULES["floor:new_recipient"], (("email", "rahul@example.com"),)),
    ("sink:new_destination", RULES["sink:new_destination"], (("host", "forms.example.com"),)),
    ("sink:private_url", RULES["sink:private_url"], (("host", "www.overleaf.com"),)),
    ("sink:outside_value", RULES["sink:outside_value"], (("phone", "4417 1234 5678"),)),
])
def test_a_yes_that_makes_a_destination_known_for_good_says_so(rule: str, values: dict[str, str],
                                                               destinations: tuple[tuple[str, str], ...]) -> None:
    """The fresh review's main finding: a Yes to these makes the person or site known for good (SQ6, GATE-205,
    Gateway._known_from), so the card must say what the Yes allows, not only what happens now."""
    made = confirm.card([confirm.Need(rule, rule + ":k", values, destinations=destinations)])
    assert any(said in made.detail for said in ("from now on", "in later tasks", "again later", "to it later")), made


# decision: GATE-514
def test_a_question_that_types_into_a_terminal_leads_with_the_command() -> None:
    """Live 2026-11-07: one question covered deleting for good, Terminal coming forward and typing `rm -f …`; the card
    must lead with the command the Yes runs, not with the delete."""
    made = confirm.card([_need("floor:delete_permanent", {"path": "the four Swiggy PDFs", "place": "Terminal"}),
                         _need(confirm.FOREGROUND, {"app": "Terminal"}),
                         _need(confirm.TERMINAL_INPUT, {"app": "Terminal", "text": '"rm -f -- a.pdf"', "enter": "yes",
                                                        "surface": "a terminal app"})])
    assert made.question == "Run this command in Terminal?"
    assert made.exact.split("\n")[0] == "rm -f -- a.pdf"
    assert "permanently deletes" in made.detail and "Terminal comes to the front" in made.detail


# decision: GATE-514
def test_a_card_names_a_running_app_by_its_name_never_its_bundle_id(monkeypatch: pytest.MonkeyPatch) -> None:
    """Live 2026-11-07: "Let FERB use your screen in com.google.Chrome for this task?"."""
    from ferb.act.mac import apps
    chrome = apps.RunningApp("com.google.Chrome", "Google Chrome", 42, False)
    monkeypatch.setattr(apps, "find", lambda name: chrome if name in ("com.google.Chrome", "pid:42") else None)
    made = confirm.card([_need(confirm.FOREGROUND, {"app": "com.google.Chrome"})])
    assert made.question == "Let FERB use your screen in Google Chrome for this task?"
    assert "com.google" not in made.detail
    assert confirm.app_label("pid:42") == "Google Chrome" and confirm.app_label("Terminal") == "Terminal"
    assert confirm.app_label("docs.google.com") == "docs.google.com"               # a site stays as written


# decision: GATE-564, GATE-561
def test_a_saved_ordinary_word_never_blanks_ferbs_own_words(daemon: RunningDaemon) -> None:
    """Past bug (an application task, 2026-11-09): a saved security answer that is an ordinary word ("anything" then,
    "plant" here) is hidden wherever it appears, and the whole card was scanned, so FERB's own sentence read "not in
    [redacted] you said or wrote", which also showed which word is sealed. Values are redacted each by itself; FERB's words are not scanned,
    in the card, its record and what clients get."""
    from types import SimpleNamespace
    gateway = daemon.daemon.gateway
    gateway.redact.add_secret_value("plant")
    need = confirm.Need("sink:outside_value", "k", {"what": "typed", "value": "plant",
                                                  "origin": "the web page on mysite.example"})
    ap = SimpleNamespace(summary="", detail="", exact="")
    gateway._set_card(ap, [need], "", False)
    assert "plant" in ap.detail and "[redacted]" not in ap.detail     # FERB's own words stay whole
    assert ap.exact == "[redacted]"                                     # the secret value itself is still hidden

    async def stored() -> str:
        span = daemon.daemon.spans.start("tool.call", input={"tool": "owner.ask", "args": {"question": ap.detail}})
        row = daemon.daemon.db.conn.execute("SELECT input_json FROM spans WHERE id = ?", (span.id,)).fetchone()
        return str(row["input_json"])
    assert "[redacted]" not in daemon.run(stored())                      # the record keeps them whole too
    assert gateway.built(f"a secret for {gateway.value('https://site.example')}") == \
        "a secret for https://site.example"                             # GATE-546's record of a secret question


# decision: GATE-566, GATE-512
def test_action_word_buttons_still_answer_yes_or_no(daemon: RunningDaemon) -> None:
    """The reviewed cards' buttons say the action ("Send", "Don't send"), but the answer recorded is still the option:
    the app answers by number, and a typed answer picks by the option's words or its button's words."""
    from ferb.daemon.questions import Approval, card_parts
    ap = _ask(daemon, [_need("floor:new_recipient", RULES["floor:new_recipient"])])   # the app's second button
    assert len(ap.labels) == len(ap.options) == 3 and ap.options[:2] == ("Yes", "No")
    assert ap.choice == "No"
    typed = Approval("ap-1", "ferb.confirm", "r", "req", options=ap.options, labels=ap.labels)
    for said, picked in ((ap.labels[0], 1), ("yes", 1), (ap.labels[1], 2), ("No", 2), (ap.labels[2], 3)):
        assert typed.option_for(said) == picked, said
    assert typed.option_for("1") is None                          # FERB's checks take words, never a number
    assert card_parts(typed)["labels"] == list(ap.labels) and card_parts(typed)["title"]
