"""Known values (FERB's own keys and tokens, GATE-572) are redacted safely: exactly, wherever they appear; never
strictly inside one of FERB's generated id bodies, while one that is a whole id-shaped token is still the secret; a
short one never inside a longer number. A saved secret or a hidden answer is not a known value (owner, 2026-11-10).

Live 2026-10-14: 48 captures failed because a short sealed value matched inside hex ids. Live 2026-11-10: a saved
answer that is an ordinary word read [redacted] in three of FERB's answers."""
import asyncio
import json
from pathlib import Path

import pytest

from ferb.core.types import Identity, ToolContext
from ferb.memory.learning import Learning
from ferb.sanitize.redact import REDACTED, Redactor, holds_sealed, sealed_spans
from tests.conftest import RunningDaemon, chat, install, outcome, sup, task
from tests.support.hidden_input import asked, keychain_helper
from tests.support.learning_fixtures import Clock
from tests.support.memory_items import run

PLACE = "https://accounts.example"


def _redactor(*values: str) -> Redactor:
    r = Redactor()
    for value in values:
        r.add_secret_value(value)
    return r


def test_a_sealed_value_is_never_replaced_inside_ferbs_ids() -> None:
    r = Redactor()
    r.add_secret_value("8e95")
    ident = "m-" + "8e95" * 16
    assert r.text(f"see {ident} done") == f"see {ident} done"
    assert r.ident(ident) == ident
    assert r.text("value 8e95 here") == "value [redacted] here"     # a value with digits: anywhere outside ids


@pytest.mark.parametrize("length", [12, 32, 64])
def test_a_sealed_value_that_is_a_whole_id_shaped_token_is_still_redacted(length: int) -> None:
    """Scan ruling I8: only a match strictly inside an id body is spared (the accepted residual, redact.py's
    INTERNAL_ID_KEYS note). A hex token or password echoed alone, or as the whole body after a prefix, is the secret."""
    token = ("4f3a9c1e7b2d" * 6)[:length]
    r = Redactor()
    r.add_secret_value(token)
    assert r.text(f"token {token} end") == "token [redacted] end"
    assert r.text(f"m-{token}") == "m-[redacted]"
    assert r.ident(token) == "[redacted]"
    assert holds_sealed(f"token {token} end", [token])


@pytest.mark.parametrize("text, want", [("?p=hunter2!x", "?p=[redacted]x"), ("userhunter2!", "user[redacted]")])
def test_a_password_glued_to_other_text_is_still_hidden(text: str, want: str) -> None:
    """Review Focus."""
    r = Redactor()
    r.add_secret_value("hunter2!")
    assert r.text(text) == want


def test_a_sealed_value_with_spaces_at_its_edges_keeps_matching() -> None:
    assert sealed_spans("prefix a suffix", " a ") == [(6, 9)]


def test_a_card_whose_ids_hold_a_short_sealed_value_is_stored(tmp_path: Path) -> None:
    from ferb.store.db import Database
    db = Database(tmp_path / "m.db")
    db.migrate()
    r = Redactor()
    r.add_secret_value("8e95")
    r.add_secret_value("Zq9-pizza")
    store = Learning(db, Clock(), lambda *args: None, r.text, redact_id=r.ident)
    store.secret_values = r.secret_values
    card = store.put("episode", "t1", {"task_id": "t1", "intent": "order food", "status": "done",
                                       "actions": [{"id": "8e95" * 8, "tool": "browser.act", "args": {}}],
                                       "narrative": "I like Zq9-pizza and pizzazz"},
                     [], origin_label="ferb", made_by="code:evidence-card/4", state="active")
    assert card["body"]["actions"][0]["id"] == "8e95" * 8
    assert card["body"]["narrative"] == "I like [redacted] and pizzazz"
    db.close()


def _ctx(task_id: str, request_id: str, worker_id: str = "w1", scopes: frozenset[str] = frozenset({"browser:act"}),
         identity: Identity = Identity.OWNER) -> ToolContext:
    return ToolContext(task_id, worker_id, request_id, identity, scopes, "dev", None, "/tmp", (), ())


def _save(daemon: RunningDaemon, ctx: ToolContext, answer: str) -> None:
    """One hidden input saved at PLACE through `Gateway.ask_secret`, answered by the owner as the app answers it."""
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(ctx, PLACE, remember="favorite food"),
                                               daemon.loop)
    ap = asked(daemon)
    run(daemon, lambda: daemon.daemon._resolve_approval(ap.id, True, None, answer))
    assert waiting.result(5) == answer


LIVE_ANSWER = "I didn't book a flight; I haven't booked anything yet."     # live 2026-11-10, with "booked" saved


# decision: GATE-572
def test_a_saved_answer_is_not_hidden_where_the_same_word_appears(daemon: RunningDaemon,
                                                                  monkeypatch: pytest.MonkeyPatch) -> None:
    """Owner 2026-11-10: a value saved in the Keychain is not searched for in other text, when it is saved and when a
    later fill reads it back by its name; only FERB's own keys are known by value."""
    keychain = keychain_helper(monkeypatch)             # a stand-in Keychain, never the owner's
    ctx = _ctx("t-book", "r-book")
    _save(daemon, ctx, "booked")
    assert list(keychain.values()) == ["booked"]
    redact = daemon.daemon.redact
    assert redact.text(LIVE_ANSWER) == LIVE_ANSWER
    revealed = asyncio.run_coroutine_threadsafe(
        daemon.daemon.gateway.ask_secret(ctx, PLACE, handle="favorite food"), daemon.loop).result(5)
    assert revealed == "booked"
    assert redact.text(LIVE_ANSWER) == LIVE_ANSWER and redact({"answer": LIVE_ANSWER}) == {"answer": LIVE_ANSWER}
    assert redact.secret_values() == frozenset()


# decision: GATE-572
def test_a_task_that_saves_an_answer_shows_the_same_word_in_its_answer(daemon: RunningDaemon,
                                                                         monkeypatch: pytest.MonkeyPatch) -> None:
    """The live case end to end: the worker's hidden input saves "booked"; the answer the owner sees keeps the word."""
    keychain_helper(monkeypatch)
    d = daemon.daemon

    def worker(token: str) -> str:
        grant = d.tokens.validate(token)
        request_id = run(daemon, lambda: d.tasks.get(grant.task_id).task.request_id)
        _save(daemon, _ctx(grant.task_id, request_id, grant.worker_id, grant.scopes, grant.identity), "booked")
        return json.dumps({"status": "done", "summary": LIVE_ANSWER, "answer": LIVE_ANSWER, "artifacts": [],
                           "evidence": []})

    install(daemon, [sup("task", "Checking.", task("check my bookings"))], {"worker": worker})
    final = outcome(chat(daemon, "check my bookings"))
    assert final["answer"] == LIVE_ANSWER and REDACTED not in json.dumps(final)


# decision: GATE-573
def test_secret_fields_are_still_never_captured() -> None:
    """Ruling 2: a password, secure or concealed field's value is masked at capture by the field's facts, never by a
    known value."""
    r = Redactor()
    assert r.field("booked", role="AXSecureTextField") == REDACTED
    assert r.field("booked", secure_input=True) == REDACTED and r.field("booked", concealed=True) == REDACTED
    assert r.field("booked", kind="textbox", label="Password") == REDACTED
    assert r.field("booked", kind="textbox", label="Destination") == "booked"


def test_a_short_known_value_never_matches_inside_a_longer_number() -> None:
    """Step 6a (replaces ruling 6's digit-run case): a short known value (an OTP, 6 characters or fewer) is no hit
    inside a longer number, so a phone number, a UPI or Aadhaar number and a JSON number keep their digits; alone, or
    beside a label, it is still hidden."""
    r = _redactor("482913")
    assert r.text("call 914829137700") == "call 914829137700"
    assert not holds_sealed("call 914829137700", r.secret_values())
    assert r.text('{"n": 14829137, "m": 482913}') == '{"n": 14829137, "m": [redacted]}'
    assert r.text("call 482913 now") == "call [redacted] now" and holds_sealed("x 482913", r.secret_values())
    assert "482913" not in r.text("pin=482913482913")                # the label hides the whole value


def test_an_id_under_an_id_key_keeps_its_form_without_hex_letters() -> None:
    """Ruling 6: a generated id FERB reads under its own id keys keeps its form, digits only or not."""
    r = _redactor("4567")
    assert r.ident("123456789012") == "123456789012"
    assert r.ident("chat-123456789012") == "chat-123456789012"
