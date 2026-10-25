"""L36 §7: sealed values are redacted safely. A word-like answer is hidden as a word, never inside longer words; no
sealed value is ever replaced strictly inside one of FERB's generated id bodies, while one that is a whole id-shaped
token is still the secret; a password glued to other text is still hidden; and saving an answer that is an ordinary
word is said once, at the end of the task's answer.

Live 2026-10-24: one sealed answer is an ordinary word, so that word read [redacted] inside FERB's notes about the
owner and the visa task's episode was refused twice; on 10-14, 48 captures failed because a short sealed value matched
inside hex ids."""
import asyncio
import json
from pathlib import Path
from typing import Any

import pytest

from ferb.core.types import Identity, ToolContext
from ferb.memory.learning import Learning
from ferb.orchestrator.supervisor import ORDINARY_WORD_LINE
from ferb.sanitize.dictionary import ordinary_word
from ferb.sanitize.redact import Redactor, holds_sealed, sealed_spans
from tests.conftest import RunningDaemon, chat, install, outcome, sup, task
from tests.support.hidden_input import asked, keychain_helper
from tests.support.learning_fixtures import Clock
from tests.support.memory_items import run

PLACE = "https://accounts.example"


def test_a_word_like_sealed_value_is_hidden_as_a_word_not_inside_longer_words() -> None:
    r = Redactor()
    r.remember_hidden_secret("pen")
    assert r.text("a pen, an open door and penguins") == "a [redacted], an open door and penguins"


def test_a_sealed_value_is_never_replaced_inside_ferbs_ids() -> None:
    r = Redactor()
    r.remember_hidden_secret("8e95")
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
    r.remember_hidden_secret(token)
    assert r.text(f"token {token} end") == "token [redacted] end"
    assert r.text(f"m-{token}") == "m-[redacted]"
    assert r.ident(token) == "[redacted]"
    assert holds_sealed(f"token {token} end", [token])


@pytest.mark.parametrize("text, want", [("?p=hunter2!x", "?p=[redacted]x"), ("userhunter2!", "user[redacted]")])
def test_a_password_glued_to_other_text_is_still_hidden(text: str, want: str) -> None:
    """Review Focus."""
    r = Redactor()
    r.remember_hidden_secret("hunter2!")
    assert r.text(text) == want


def test_a_sealed_value_with_spaces_at_its_edges_keeps_matching() -> None:
    assert sealed_spans("prefix a suffix", " a ") == [(6, 9)]


def test_a_card_whose_ids_hold_a_short_sealed_value_is_stored(tmp_path: Path) -> None:
    from ferb.store.db import Database
    db = Database(tmp_path / "m.db")
    db.migrate()
    r = Redactor()
    r.remember_hidden_secret("8e95")
    r.remember_hidden_secret("pizza")
    store = Learning(db, Clock(), lambda *args: None, r.text, redact_id=r.ident)
    store.secret_values = r.secret_values
    card = store.put("episode", "t1", {"task_id": "t1", "intent": "order food", "status": "done",
                                       "actions": [{"id": "8e95" * 8, "tool": "browser.act", "args": {}}],
                                       "narrative": "I like pizza and pizzazz"},
                     [], origin_label="ferb", made_by="code:evidence-card/4", state="active")
    assert card["body"]["actions"][0]["id"] == "8e95" * 8
    assert card["body"]["narrative"] == "I like [redacted] and pizzazz"
    db.close()


def test_ordinary_words_come_from_the_system_word_list_read_when_asked(tmp_path: Path) -> None:
    words = tmp_path / "words"
    words.write_text("Pizza\nopen\n")
    assert ordinary_word("pizza", words) and ordinary_word(" Open ", words)
    assert not ordinary_word("pizza2", words) and not ordinary_word("new delhi", words)
    assert not ordinary_word("pizza", tmp_path / "missing")
    words.write_text("open\n")          # scan ruling m7: read on demand, never kept in the daemon
    assert not ordinary_word("pizza", words)


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


def _word_list(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    words = tmp_path / "words"
    words.write_text("pizza\n")
    monkeypatch.setattr("ferb.sanitize.dictionary.SYSTEM_WORDS", words)


def test_saving_an_ordinary_word_answer_is_journaled_once_without_the_word(daemon: RunningDaemon,
                                                                         monkeypatch: pytest.MonkeyPatch,
                                                                         tmp_path: Path) -> None:
    _word_list(monkeypatch, tmp_path)
    keychain = keychain_helper(monkeypatch)             # a stand-in Keychain, never the owner's
    _save(daemon, _ctx("t-food", "r-food"), "hunter2!")             # no ordinary word: nothing to say
    assert run(daemon, lambda: daemon.daemon._journal().of_kind("secret.ordinary_word")) == []
    _save(daemon, _ctx("t-food", "r-food"), "pizza")
    assert list(keychain.values()) == ["pizza"]
    [entry] = run(daemon, lambda: daemon.daemon._journal().of_kind("secret.ordinary_word"))
    assert entry.data == {"task_id": "t-food", "place": "accounts.example"}
    assert "pizza" not in json.dumps(entry.data)
    assert run(daemon, lambda: daemon.daemon.supervisor._secret_lines("t-food")) == \
        [ORDINARY_WORD_LINE.format(place="accounts.example")]


def test_a_note_that_cannot_be_journaled_never_costs_the_owners_fill(daemon: RunningDaemon,
                                                                      monkeypatch: pytest.MonkeyPatch,
                                                                      tmp_path: Path) -> None:
    """The line only explains why a word is hidden: when FERB's journal refuses the note, the saved answer still goes
    to its fill (as a memory read's record never costs the read, re-review R4)."""
    _word_list(monkeypatch, tmp_path)
    keychain_helper(monkeypatch)

    def refused(kind: str, data: dict[str, Any]) -> None:
        raise RuntimeError("journal unavailable")
    monkeypatch.setattr(daemon.daemon.gateway, "journal_problem", refused)
    _save(daemon, _ctx("t-food", "r-food"), "pizza")


def test_a_task_that_saves_an_ordinary_word_answer_ends_its_answer_with_the_line_once(
        daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    """Scan ruling I13: the outcome itself. The worker's hidden input saves "pizza" at one place twice; the full answer
    the owner sees ends with code's line for that place, said once."""
    _word_list(monkeypatch, tmp_path)
    keychain_helper(monkeypatch)
    d = daemon.daemon

    def worker(token: str) -> str:
        grant = d.tokens.validate(token)
        request_id = run(daemon, lambda: d.tasks.get(grant.task_id).task.request_id)
        for _ in range(2):
            _save(daemon, _ctx(grant.task_id, request_id, grant.worker_id, grant.scopes, grant.identity), "pizza")
        return json.dumps({"status": "done", "summary": "Saved your answer.", "answer": "Saved your answer.",
                           "artifacts": [], "evidence": []})

    install(daemon, [sup("task", "Saving.", task("save my favorite food answer"))], {"worker": worker})
    final = outcome(chat(daemon, "save my favorite food answer"))
    line = ORDINARY_WORD_LINE.format(place="accounts.example")
    assert final["answer"] == "Saved your answer.\n\n" + line
    assert "pizza" not in json.dumps(final)
