"""A secret pasted into the chat (setup intro task 11, kb area gateway): the message is masked before any model sees
it (as before), and FERB now says so. The ack names each kind found (`hidden`), the journal records the kinds
(`chat.secret_hidden`), and the planner gets one fact line, so it never asks for the value again. Names only, never a
value (rule 7); the kinds are formats, never a judgement of intent (rule 4). The tokens are invented."""
import asyncio
import json
import time
from typing import Any

from ferb.cli.client import Client
from ferb.core.types import Identity
from ferb.policy.rules import ActionRequest
from ferb.sanitize.redact import REDACTED, Redactor
from tests.conftest import (RunningDaemon, done, gateway_request, gateway_setup, install, outcome, report, sup, task,
                            turn_events)

FAKE_GITHUB = "ghp_" + "Fake0Token0For0Tests0Only0abcdefgh12"     # invented: GitHub's format, no real token
FAKE_STRIPE = "sk_" + "test_" + "Invented0Key0For0Tests"         # invented: Stripe's test-key format
FAKE_ANTHROPIC = "sk-ant-" + "invented-key-for-tests-0123"       # invented: Anthropic's format (OpenAI's prefix too)
SAVED = "plum-velvet-7781"                                       # an invented saved secret, no format of its own
SAVED_SECRET = "saved secret"                                    # the kind the app words as one of the owner's own
TEST_CARD = "4111 1111 1111 1111"                                # the well-known test card number (Luhn-valid)
FAKE_TWILIO = "AC" + "0123456789abcdef" * 2 + ":" + "fedcba9876543210" * 2   # invented: account id and auth token


def _send(d: RunningDaemon, text: str) -> list[dict[str, Any]]:
    c = Client(d.sock, timeout=30)
    try:
        return list(c.stream("chat.send", {"session_id": "s1", "text": text}))
    finally:
        c.close()


def _planner_context(script: Any) -> str:
    return [r for _, key, r in script.requests if key == "supervisor"][-1].context


def _until(check: Any, timeout: float = 20) -> None:
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if check():
            return
        time.sleep(0.02)
    raise AssertionError("the condition never held")


def _journal(d: RunningDaemon) -> list[tuple[str, dict[str, Any]]]:
    async def go() -> list[tuple[str, dict[str, Any]]]:
        return [(e.kind, e.data) for e in d.daemon.journal.entries()]
    return d.run(go())


# decision: GATE-501, GATE-502
def test_a_pasted_token_is_hidden_and_named(daemon: RunningDaemon) -> None:
    script = install(daemon, [sup("reply", "Noted.")])
    events = _send(daemon, f"here is my token {FAKE_GITHUB} for the deploy")
    ack = events[0]
    assert ack["type"] == "ack" and ack["hidden"] == ["GitHub token"]
    assert FAKE_GITHUB not in ack["message"] and REDACTED in ack["message"]
    context = _planner_context(script)
    assert REDACTED in context and FAKE_GITHUB not in context
    assert "FERB's note: the owner's message held a GitHub token; FERB hid it." in context
    rows = _journal(daemon)
    assert [data for kind, data in rows if kind == "chat.secret_hidden"] == [
        {"session_id": "s1", "request_id": ack["request_id"], "kinds": ["GitHub token"]}]
    assert not any(FAKE_GITHUB in repr(data) for _, data in rows)          # the raw value is never journaled
    assert FAKE_GITHUB not in repr(events)                                   # nor anywhere in the stream
    assert events[-1]["type"] == "reply"                                     # nothing refused: the message still ran


# decision: GATE-501, GATE-502
def test_a_saved_secret_is_named_as_saved(daemon: RunningDaemon) -> None:
    daemon.call_soon(daemon.daemon._add_secret_value, SAVED)
    script = install(daemon, [sup("reply", "Noted.")])
    ack = _send(daemon, f"the door code note says {SAVED}, file it")[0]
    assert ack["hidden"] == [SAVED_SECRET]
    context = _planner_context(script)
    assert SAVED not in context and "FERB's note: the owner's message held a saved secret; FERB hid it." in context


# decision: GATE-502
def test_a_plain_message_has_no_notice(daemon: RunningDaemon) -> None:
    script = install(daemon, [sup("reply", "Done.")])
    ack = _send(daemon, "rename the Q3 report to final")[0]
    assert "hidden" not in ack
    assert "FERB hid it" not in _planner_context(script)
    assert not [data for kind, data in _journal(daemon) if kind == "chat.secret_hidden"]


# decision: GATE-501
def test_each_kind_is_named_once_in_order() -> None:
    r = Redactor()
    text = f"{FAKE_STRIPE} then {FAKE_ANTHROPIC} and again {FAKE_STRIPE}"
    assert r.kinds(text) == ["Stripe key", "Anthropic key"]        # one name per kind; a key isn't also OpenAI's
    assert r.kinds("Authorization: Bearer abcdef0123456789xyz") == ["bearer token"]
    assert r.kinds("call me at 5pm about the report") == []


# decision: GATE-501
def test_a_cards_parts_and_a_twilio_pair_are_named_as_one() -> None:
    """A card with its expiry and CVV is one payment card; a Twilio account id with its auth token is one Twilio key.
    Everything is still masked as before: only the names fold."""
    r = Redactor()
    card = f"card {TEST_CARD} exp 12/27 cvv 123"
    assert r.kinds(card) == ["payment card"]
    assert "1111" not in r.text(card) and "12/27" not in r.text(card) and "123" not in r.text(card)
    assert r.kinds(f"twilio {FAKE_TWILIO}") == ["Twilio key"]


# decision: GATE-502
def test_an_expiry_or_cvv_without_a_card_names_nothing() -> None:
    """By kind, not by reading intent: a card's part with no card in the text gives no notice; it is still masked."""
    r = Redactor()
    for text in ("my offer is valid until 03/27", "the CVV is 123"):
        assert r.kinds(text) == []
        assert r.text(text) != text                        # redacted as today


# decision: GATE-502
def test_an_answer_holding_a_token_is_named_too(daemon: RunningDaemon) -> None:
    """chat.answer: an answer to a worker's question can carry a pasted key; its kinds come back and are journaled."""
    action = ActionRequest("owner.ask", "owner_question", Identity.OWNER, frozenset(), "t", "w", "r", frozenset())
    ap = daemon.daemon.state.approvals.question(action, "Which deploy key should I use?", [])
    result = daemon.run(daemon.daemon._chat_answer(None, {"approval_id": ap.id, "text": f"use {FAKE_GITHUB}"}))
    assert result["hidden"] == ["GitHub token"] and FAKE_GITHUB not in str(ap.answer)
    rows = _journal(daemon)
    assert [data["kinds"] for kind, data in rows if kind == "chat.secret_hidden"] == [["GitHub token"]]
    assert not any(FAKE_GITHUB in repr(data) for _, data in rows)


# decision: GATE-502
def test_a_worker_gets_ferbs_note_beside_an_answer(daemon: RunningDaemon) -> None:
    """owner.ask: the answer the worker reads is masked, and FERB's note sits beside it, not inside it."""
    grant, _ = gateway_setup(daemon, set(), gateway_request("deploy the site"), [])
    daemon.daemon.gateway.route_question = None              # straight to the owner (no orchestrator in this test)
    call = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.call(grant, "owner.ask", {
        "question": "Which deploy key should I use?", "options": []}), daemon.loop)
    _until(lambda: any(ap.capability == "owner.ask" for ap in daemon.daemon.state.approvals.pending()))
    ap = next(ap for ap in daemon.daemon.state.approvals.pending() if ap.capability == "owner.ask")
    daemon.run(daemon.daemon._chat_answer(None, {"approval_id": ap.id, "text": f"use {FAKE_GITHUB}"}))
    result = call.result(timeout=10)
    given = result["content"]
    assert given["answer"] == f"use {REDACTED}"
    assert given["ferb_note"] == "FERB's note: the owner's answer held a GitHub token; FERB hid it."
    assert FAKE_GITHUB not in repr(result)


# decision: GATE-502
def test_the_next_attempt_gets_ferbs_note_beside_a_step_answer(daemon: RunningDaemon) -> None:
    """plan.ask: the attempt after the owner's answer is told the masked answer and FERB's note on its own line."""
    script = install(daemon, [sup("task", "Deploying.", task("Deploy the site"))],
                     {"worker": [json.dumps(report("not_done", reason="No key.", question="Which deploy key?")),
                                 done("Deployed.")]})
    c = Client(daemon.sock, timeout=60)
    events: list[dict[str, Any]] = []
    try:
        stream = c.stream("chat.send", {"session_id": "s1", "text": "deploy the site"})
        while not any(ap.capability == "plan.ask" for ap in daemon.daemon.state.approvals.pending()):
            events.append(next(stream))
        ap = next(ap for ap in daemon.daemon.state.approvals.pending() if ap.capability == "plan.ask")
        a = Client(daemon.sock)
        try:
            assert a.call("chat.answer", {"approval_id": ap.id, "text": f"use {FAKE_GITHUB}"})["hidden"] == \
                ["GitHub token"]
        finally:
            a.close()
        events += list(stream)
    finally:
        c.close()
    assert outcome(turn_events(events))["status"] == "done"
    contexts = [ctx for _, _, ctx, _ in script.contexts]
    told = [ctx for ctx in contexts if "FERB's note: the owner's answer held a GitHub token; FERB hid it." in ctx]
    assert told and all(f"use {REDACTED}\nFERB's note" in ctx for ctx in told)
    assert not any(FAKE_GITHUB in ctx for ctx in contexts)


def test_a_code_reply_to_an_open_question_names_nothing(daemon: RunningDaemon) -> None:
    """A login code typed in chat while a question asks for one is masked as a reply (`_hide_code_reply`), not a
    credential: no notice and no journal row."""
    async def ask() -> None:
        action = ActionRequest("owner.ask", "owner_question", Identity.FERB, frozenset(), "other.worker",
                               "other.worker", "req-other", frozenset())
        daemon.daemon.state.approvals.question(action, "What's the 6-digit login code?", []).from_task = True
    daemon.run(ask())
    install(daemon, [sup("reply", "Thanks.")])
    events = _send(daemon, "482913")
    assert "hidden" not in events[0] and "482913" not in repr(events)
    assert not [data for kind, data in _journal(daemon) if kind == "chat.secret_hidden"]


# decision: APP-712, ORCH-520
def test_the_acks_message_is_the_message_as_recorded_without_paste_markers(daemon: RunningDaemon) -> None:
    """Live 2026-11-04: the composer's paste brackets (ESC[200~ … ESC[201~) came back in ack.message, so the owner's
    bubble showed "[200~ … [201~". The ack now carries the message exactly as chat.message records it."""
    install(daemon, [sup("reply", "Noted.")])
    composer = "\x1b[200~Save a copy next to the original; don\x1b[201~’\x1b[200~t overwrite it\x1b[201~"
    leaked = "[200~Keep a running notes file[201~"
    ack = _send(daemon, composer + "\n\n" + leaked)[0]
    recorded = [data["text"] for kind, data in _journal(daemon) if kind == "chat.message"]
    assert ack["message"] == recorded[-1] == "Save a copy next to the original; don’t overwrite it\n\nKeep a running notes file"
