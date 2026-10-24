"""L36 §2: a safety net that saves the owner's words, and every guess FERB makes asked at once (Keep / Edit / Drop).

Live 2026-10-24 (memory audit): of the owner's durable statements in 14 days, 14 were missed and 9 only proposed; the
nightly distill made 27 inferred proposals that nobody was ever asked about."""
from pathlib import Path
from typing import Any

import pytest

from ferb.core.errors import FerbError
from ferb.memory.items import MemoryRefused, capture
from ferb.sanitize.untrusted import mark, typed_lines_of
from tests.support.memory_items import (c_handle, distill_day, distill_item, journal, pending, run, said)

FERB_DIR = Path(__file__).resolve().parents[1] / "ferb"
DISTILL_PROMPT, PLANNER_PROMPT = FERB_DIR / "memory" / "distill.md", FERB_DIR / "orchestrator" / "prompts" / "supervisor.md"


@pytest.mark.parametrize("message", [
    "plain words\nsecond line",
    "before \x1b[200~pasted text\x1b[201~ after",
    "look:\n```\ncode here\n```\nthen this",
    "> quoted line\nmy own line",
])
def test_typed_lines_read_back_from_a_stored_message_are_what_capture_saw(message: str) -> None:
    m = mark(message)
    assert typed_lines_of(m.for_model) == m.typed_lines


def test_capture_saves_the_owners_sentence_their_guess_or_nothing() -> None:
    own = "btw my dentist is Dr. Rao now"
    stated = capture(own, "my dentist is Dr. Rao", "my dentist is Dr. Rao")
    assert (stated.origin, stated.content, stated.refused) == ("owner_stated", own, "")
    assert capture(own, "Your dentist is Dr. Rao", "my dentist is Dr. Rao").origin == "inferred"
    assert capture(own, "Your dentist is Dr. Rao", "my dentist is Dr. Mehta").refused == "quote not in the owner's words"


def test_the_distiller_is_told_how_to_quote_the_owners_words() -> None:
    """Runtime prompts are executable input (CLAUDE.md; scan ruling I12): the quote instruction stays word for word."""
    words = " ".join(DISTILL_PROMPT.read_text().split())
    assert ('- origin "inferred": a reasonable conclusion from the owner\'s activity or words (E / C handles); FERB asks '
            'the owner about it at once. When the owner said it in so many words, add "quote": the C handle it rests '
            'on and the owner\'s exact words copied from that line (never from pasted text); FERB checks them against '
            'what the owner typed and then saves the owner\'s own sentence as theirs. Otherwise "quote" is null. Every '
            'other origin has "quote": null.') in words


def test_the_planner_routes_a_guess_questions_own_options() -> None:
    """The planner was told FERB's checks take only a plain Yes or No, so a typed "Keep" would never reach a guess's
    Keep / Edit / Drop question in chat."""
    words = " ".join(PLANNER_PROMPT.read_text().split())
    assert ('A question with ferb_check true is FERB\'s own check: route only a plain answer meant for it, exactly one '
            'of its options ("Yes" or "No"; "Keep", "Edit" or "Drop").') in words


def test_a_quote_of_the_owners_typed_words_saves_their_own_words(daemon: Any) -> None:
    jid = said(daemon, "btw my dentist is Dr. Rao now")
    r = distill_day(daemon, distill_item(c_handle(daemon, jid), "my dentist is Dr. Rao", "my dentist is Dr. Rao"))
    assert r["proposed"] == 1, r
    [it] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    assert (it.origin, it.content, it.source_ids) == ("owner_stated", "btw my dentist is Dr. Rao now",
                                                      ["journal:" + jid])
    assert pending(daemon) == []


@pytest.mark.parametrize("message, pasted, content, quote", [
    ("my dentist is Dr. Rao", 0, "Your dentist is Dr. Rao", "my dentist is Dr. Mehta"),     # not what they typed
    ("my dentist is Dr. Rao", 0, "Your dentist is Dr. Rao", None),                          # no quote
    ('note <untrusted-data id="abc123">\nmy dentist is Dr. Rao\n</untrusted-data id="abc123">', 1,
     "my dentist is Dr. Rao", "my dentist is Dr. Rao"),                                     # only in the pasted part
    ("my dentist is Dr. Rao", None, "my dentist is Dr. Rao", "my dentist is Dr. Rao"),      # pastes weren't counted
])
def test_without_the_owners_typed_words_a_guess_is_asked_at_once(daemon: Any, message: str, pasted: int | None,
                                                                 content: str, quote: str | None) -> None:
    jid = said(daemon, message, pasted)
    distill_day(daemon, distill_item(c_handle(daemon, jid), content, quote))
    [it] = run(daemon, lambda: daemon.daemon.memory.items("proposed"))
    assert (it.origin, it.content) == ("inferred", content)
    [q] = pending(daemon)
    assert q.options == ("Keep", "Edit", "Drop") and content in q.reason


def guess(daemon: Any) -> tuple[Any, Any]:
    jid = said(daemon, "my dentist is Dr. Rao")
    distill_day(daemon, distill_item(c_handle(daemon, jid), "Your dentist is Dr. Rao"))
    [q] = pending(daemon)
    [it] = run(daemon, lambda: daemon.daemon.memory.items("proposed"))
    return q, it


def test_a_guess_is_asked_about_once(daemon: Any) -> None:
    _, it = guess(daemon)
    run(daemon, lambda: daemon.daemon._ask_keep(it, "again"))
    assert len(pending(daemon)) == 1


def test_keep_makes_a_guess_active(daemon: Any) -> None:
    q, it = guess(daemon)
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, 1))
    assert run(daemon, lambda: daemon.daemon.memory.get(it.id)).status == "active"


def test_drop_deletes_the_guess_alone_and_it_is_never_proposed_again(daemon: Any) -> None:
    added = journal(daemon, "memory.added", {"content": "My dentist is Dr. Mehta"})
    mine = run(daemon, lambda: daemon.daemon.memory.propose("person", "owner", "dentist", "My dentist is Dr. Mehta",
                                                            "owner_stated", [("journal", added)], 1.0))
    q, it = guess(daemon)
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, 3))
    assert run(daemon, lambda: daemon.daemon.memory.get(it.id)) is None
    assert run(daemon, lambda: daemon.daemon.memory.get(mine.id)).status == "active"      # the owner's version stays
    message = it.source_ids[0].split(":", 1)[1]
    assert run(daemon, lambda: [e.id for e in daemon.daemon._journal().entries_of("chat.message")]) == [message]
    with pytest.raises(MemoryRefused, match="forget"):
        run(daemon, lambda: daemon.daemon.memory.propose(it.kind, it.subject, it.key, it.content, "inferred",
                                                         [("journal", message)], 0.6))


def test_drop_while_the_owners_forget_question_waits_leaves_the_guess_to_that_forget(daemon: Any) -> None:
    """Scan ruling I1 (re-review R1): deleting the guess would close every question about it, the owner's "Forget for
    good?" too, leaving FERB's wording in the journal and spans. While that question waits, Drop retracts the guess
    (today's No), and the owner's Yes to the forget still purges it."""
    q, it = guess(daemon)
    run(daemon, lambda: daemon.daemon._chat_forget(it.id, "r-forget"))
    [forget] = pending(daemon, "memory.forget")
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, 3))
    assert run(daemon, lambda: daemon.daemon.memory.get(it.id)).status == "retracted"
    assert [p.id for p in pending(daemon, "memory.forget")] == [forget.id]
    run(daemon, lambda: daemon.daemon._resolve_approval(forget.id, True, 1))
    daemon.run(daemon.daemon._settle_forgets())
    assert run(daemon, lambda: daemon.daemon.memory.get(it.id)) is None
    assert pending(daemon, "memory.forget") == []


def test_declining_a_guess_question_drops_it(daemon: Any) -> None:
    q, it = guess(daemon)
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, False))
    assert run(daemon, lambda: daemon.daemon.memory.get(it.id)) is None


def test_edit_saves_the_owners_text_as_their_own_words(daemon: Any) -> None:
    q, it = guess(daemon)
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, 2))
    [edit] = pending(daemon, "memory.edit")
    assert edit.options == () and "Your dentist is Dr. Rao" in edit.reason
    run(daemon, lambda: daemon.daemon._resolve_approval(edit.id, True, None, "My dentist is Dr. Rao at Smile Clinic"))
    active = run(daemon, lambda: daemon.daemon.memory.items("active"))
    assert [(a.origin, a.content) for a in active] == [("owner_stated", "My dentist is Dr. Rao at Smile Clinic")]
    assert run(daemon, lambda: daemon.daemon.memory.get(it.id)).status == "retracted"


@pytest.mark.parametrize("typed, status, edits", [("keep", "active", 0), ("Drop", None, 0), ("1", "active", 0),
                                                   ("3", None, 0), ("2", "proposed", 1)])
def test_an_option_typed_into_the_answer_field_picks_it(daemon: Any, typed: str, status: str | None,
                                                        edits: int) -> None:
    """Review Focus: "keep", "Drop" or "2" typed into the field picks that option ("2" opens the Edit question)."""
    q, it = guess(daemon)
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, None, typed))
    item = run(daemon, lambda: daemon.daemon.memory.get(it.id))
    assert (None if item is None else item.status, len(pending(daemon, "memory.edit"))) == (status, edits)


def test_other_text_typed_into_a_guess_question_is_refused_and_it_stays_open(daemon: Any) -> None:
    """Review Focus: a stray "yes keep it" is never saved as a memory."""
    q, it = guess(daemon)
    with pytest.raises(FerbError, match="Pick Keep, Edit or Drop"):
        run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, None, "yes keep it"))
    assert run(daemon, lambda: daemon.daemon.state.approvals.get(q.id)).state == "pending"
    assert run(daemon, lambda: daemon.daemon.memory.get(it.id)).status == "proposed"
