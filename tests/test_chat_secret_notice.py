"""A secret pasted into the chat (setup intro task 11, notes/gateway.md): the message is masked before any model sees
it (as before), and FERB now says so. The ack names each kind found (`hidden`), the journal records the kinds
(`chat.secret_hidden`), and the planner gets one fact line, so it never asks for the value again. Names only, never a
value (rule 7); the kinds are formats, never a judgement of intent (rule 4). The tokens are invented."""
from typing import Any

from ferb.cli.client import Client
from ferb.core.types import Identity
from ferb.policy.rules import ActionRequest
from ferb.sanitize.redact import REDACTED, Redactor
from tests.conftest import RunningDaemon, install, sup

FAKE_GITHUB = "ghp_" + "Fake0Token0For0Tests0Only0abcdefgh12"     # invented: GitHub's format, no real token
FAKE_STRIPE = "sk_" + "test_" + "Invented0Key0For0Tests"         # invented: Stripe's test-key format
FAKE_ANTHROPIC = "sk-ant-" + "invented-key-for-tests-0123"       # invented: Anthropic's format (OpenAI's prefix too)
SAVED = "plum-velvet-7781"                                       # an invented saved secret, no format of its own
SAVED_SECRET = "saved secret"                                    # the kind the app words as one of the owner's own


def _send(d: RunningDaemon, text: str) -> list[dict[str, Any]]:
    c = Client(d.sock, timeout=30)
    try:
        return list(c.stream("chat.send", {"session_id": "s1", "text": text}))
    finally:
        c.close()


def _planner_context(script: Any) -> str:
    return [r for _, key, r in script.requests if key == "supervisor"][-1].context


def _journal(d: RunningDaemon) -> list[tuple[str, dict[str, Any]]]:
    async def go() -> list[tuple[str, dict[str, Any]]]:
        return [(e.kind, e.data) for e in d.daemon.journal.entries()]
    return d.run(go())


def test_a_pasted_token_is_hidden_and_named(daemon: RunningDaemon) -> None:
    script = install(daemon, [sup("reply", "Noted.")])
    events = _send(daemon, f"here is my token {FAKE_GITHUB} for the deploy")
    ack = events[0]
    assert ack["type"] == "ack" and ack["hidden"] == ["GitHub token"]
    assert FAKE_GITHUB not in ack["message"] and REDACTED in ack["message"]
    context = _planner_context(script)
    assert REDACTED in context and FAKE_GITHUB not in context
    assert "The owner's message held a GitHub token; FERB hid it." in context
    rows = _journal(daemon)
    assert [data for kind, data in rows if kind == "chat.secret_hidden"] == [
        {"session_id": "s1", "request_id": ack["request_id"], "kinds": ["GitHub token"]}]
    assert not any(FAKE_GITHUB in repr(data) for _, data in rows)          # the raw value is never journaled
    assert events[-1]["type"] == "reply"                                     # nothing refused: the message still ran


def test_a_saved_secret_is_named_as_saved(daemon: RunningDaemon) -> None:
    daemon.call_soon(daemon.daemon._add_secret_value, SAVED)
    script = install(daemon, [sup("reply", "Noted.")])
    ack = _send(daemon, f"the door code note says {SAVED}, file it")[0]
    assert ack["hidden"] == [SAVED_SECRET]
    context = _planner_context(script)
    assert SAVED not in context and "The owner's message held a saved secret; FERB hid it." in context


def test_a_plain_message_has_no_notice(daemon: RunningDaemon) -> None:
    script = install(daemon, [sup("reply", "Done.")])
    ack = _send(daemon, "rename the Q3 report to final")[0]
    assert "hidden" not in ack
    assert "FERB hid it" not in _planner_context(script)
    assert not [data for kind, data in _journal(daemon) if kind == "chat.secret_hidden"]


def test_each_kind_is_named_once_in_order() -> None:
    r = Redactor()
    text = f"{FAKE_STRIPE} then {FAKE_ANTHROPIC} and again {FAKE_STRIPE}"
    assert r.kinds(text) == ["Stripe key", "Anthropic key"]        # one name per kind; a key isn't also OpenAI's
    assert r.kinds("Authorization: Bearer abcdef0123456789xyz") == ["bearer token"]
    assert r.kinds("call me at 5pm about the report") == []


def test_an_answer_holding_a_token_is_named_too(daemon: RunningDaemon) -> None:
    """chat.answer: an answer to a worker's question can carry a pasted key; its kinds come back and are journaled."""
    action = ActionRequest("owner.ask", "owner_question", Identity.OWNER, frozenset(), "t", "w", "r", frozenset())
    ap = daemon.daemon.state.approvals.question(action, "Which deploy key should I use?", [])
    result = daemon.run(daemon.daemon._chat_answer(None, {"approval_id": ap.id, "text": f"use {FAKE_GITHUB}"}))
    assert result["hidden"] == ["GitHub token"] and FAKE_GITHUB not in str(ap.answer)
    rows = _journal(daemon)
    assert [data["kinds"] for kind, data in rows if kind == "chat.secret_hidden"] == [["GitHub token"]]
    assert not any(FAKE_GITHUB in repr(data) for _, data in rows)
