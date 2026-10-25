"""L36 §2: a safety net that saves the owner's words, and every guess FERB makes asked at once (Keep / Edit / Drop).

Live 2026-10-24 (memory audit): of the owner's durable statements in 14 days, 14 were missed and 9 only proposed; the
nightly distill made 27 inferred proposals that nobody was ever asked about."""
import json
from pathlib import Path
from typing import Any

import pytest

from ferb.core.errors import FerbError
from ferb.memory.items import MemoryRefused, capture
from ferb.sanitize.untrusted import mark, typed_lines_of
from tests.conftest import chat, install
from tests.support.memory_items import (c_handle, distill_day, distill_item, journal, owner_fact, pending, run,
                                        said)
from tests.support.schedules import own_daemon

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
    Keep / Edit / Drop question in chat (nor a typed "Retire" an out-of-date item's Retire / Keep, L36 §6)."""
    words = " ".join(PLANNER_PROMPT.read_text().split())
    assert ('A question with ferb_check true is FERB\'s own check: route only a plain answer meant for it, exactly one '
            'of its options ("Yes" or "No"; "Keep", "Edit" or "Drop"; "Retire" or "Keep").') in words


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


def guess(daemon: Any, message: str = "my dentist is Dr. Rao", content: str = "Your dentist is Dr. Rao",
          **fields: Any) -> tuple[Any, Any]:
    """One of FERB's guesses from the owner's chat that day, and its Keep / Edit / Drop question."""
    jid = said(daemon, message)
    distill_day(daemon, distill_item(c_handle(daemon, jid), content, **fields))
    [q] = pending(daemon)
    [it] = run(daemon, lambda: daemon.daemon.memory.items("proposed"))
    return q, it


# Fix round 1: a guess whose words name passwords made FERB's question about it look like a password question
PASSWORD_GUESS = {"message": "opened 1Password again", "content": "You keep your passwords in 1Password",
                  "key": "password_app", "kind": "preference"}


def test_a_guess_is_asked_about_once(daemon: Any) -> None:
    _, it = guess(daemon)
    run(daemon, lambda: daemon.daemon._ask_keep(it, "again"))
    assert len(pending(daemon)) == 1


def test_only_a_waiting_guess_is_asked_about(daemon: Any) -> None:
    """Fix round 1 (minor 3): the planner capturing a paraphrase the owner already kept found it active and asked
    again; `_ask_keep` asks only about an inferred item still proposed."""
    q, it = guess(daemon)
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, 1))
    kept = run(daemon, lambda: daemon.daemon.memory.get(it.id))
    mine = owner_fact(daemon, "dentist_city", "My dentist is in Pune")
    run(daemon, lambda: (daemon.daemon._ask_keep(kept, "again"), daemon.daemon._ask_keep(mine, "again")))
    assert pending(daemon) == []


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


def test_drop_while_a_forget_of_another_version_waits_leaves_the_guess_to_that_forget(daemon: Any) -> None:
    """Fix round 1 (Important 1): the owner's "Forget for good?" about their own version of the fact lists every
    version, the guess and the chat message it came from included. Dropping the guess first cut that link, so the Yes
    left the message the question named. While such a question waits, Drop retracts the guess."""
    added = journal(daemon, "memory.added", {"content": "My dentist is Dr. Mehta"})
    mine = run(daemon, lambda: daemon.daemon.memory.propose("person", "owner", "dentist", "My dentist is Dr. Mehta",
                                                            "owner_stated", [("journal", added)], 1.0))
    q, it = guess(daemon)
    message = it.source_ids[0].split(":", 1)[1]
    told = run(daemon, lambda: daemon.daemon._chat_forget(mine.id, "r-forget"))
    assert "Your dentist is Dr. Rao" in told and "chat.message" in told
    [forget] = pending(daemon, "memory.forget")
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, 3))
    assert run(daemon, lambda: daemon.daemon.memory.get(it.id)).status == "retracted"
    assert [p.id for p in pending(daemon, "memory.forget")] == [forget.id]
    run(daemon, lambda: daemon.daemon._resolve_approval(forget.id, True, 1))
    daemon.run(daemon.daemon._settle_forgets())
    assert run(daemon, lambda: [e.id for e in daemon.daemon._journal().entries_of("chat.message")]) == []
    assert run(daemon, lambda: [daemon.daemon.memory.get(i) for i in (it.id, mine.id, message)]) == [None] * 3


def test_drop_deletes_the_guess_s_index_node(daemon: Any) -> None:
    """A pending guess is indexed as a `proposed` fact node (the dashboard confirms it there); Drop deletes that too."""
    q, it = guess(daemon)
    run(daemon, lambda: daemon.daemon.learning.sync_facts(daemon.daemon.memory))

    def nodes() -> list[str]:
        return [r[0] for r in daemon.daemon.db.conn.execute(
            "SELECT node_id FROM learning_sources WHERE source_ref = ?", ("memory:" + it.id,))
            if daemon.daemon.learning.get(r[0]) is not None]
    assert len(run(daemon, nodes)) == 1
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, 3))
    assert run(daemon, nodes) == []


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
    with pytest.raises(MemoryRefused, match="forget"):          # fix round 1 (minor 4): never asked about again
        run(daemon, lambda: daemon.daemon.memory.propose(it.kind, it.subject, it.key, it.content, "inferred",
                                                         [("journal", it.source_ids[0].split(":", 1)[1])], 0.6))


def test_an_edit_waits_across_a_restart(home: Any) -> None:
    """Rule 9: the Edit question, like every memory question, comes back after a restart with its words."""
    d = own_daemon(home, "id")
    try:
        q, it = guess(d)
        run(d, lambda: d.daemon._resolve_approval(q.id, True, 2))
        [edit] = pending(d, "memory.edit")
    finally:
        d.stop()
    again = own_daemon(home, "again")
    try:
        [back] = pending(again, "memory.edit")
        assert (back.id, back.reason, back.options) == (edit.id, edit.reason, ())
        run(again, lambda: again.daemon._resolve_approval(back.id, True, None, "My dentist is Dr. Rao in Pune"))
        assert run(again, lambda: [a.content for a in again.daemon.memory.items("active")]) == [
            "My dentist is Dr. Rao in Pune"]
    finally:
        again.stop()


def test_a_guess_that_names_passwords_never_hides_the_owners_edit(daemon: Any) -> None:
    """Fix round 1 (Important 2): FERB's memory questions never ask for a secret, but "passwords" in a guess made the
    answer field hide a one-word answer, or this one, whole, and "[redacted]" was saved as the owner's words."""
    q, it = guess(daemon, **PASSWORD_GUESS)
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, 2))
    [edit] = pending(daemon, "memory.edit")
    run(daemon, lambda: daemon.daemon._resolve_approval(edit.id, True, None, "I use Bitwarden now"))
    assert run(daemon, lambda: [(a.origin, a.content) for a in daemon.daemon.memory.items("active")]) == [
        ("owner_stated", "I use Bitwarden now")]


def test_a_guess_that_names_passwords_never_hides_the_owners_chat(daemon: Any) -> None:
    """Fix round 1 (Important 2): while that guess's question waited, every one-word chat message ("stop") was hidden
    whole before the planner saw it."""
    guess(daemon, **PASSWORD_GUESS)
    assert run(daemon, lambda: [daemon.daemon._hide_code_reply(w) for w in ("stop", "continue", "thanks")]) == [
        "stop", "continue", "thanks"]


@pytest.mark.parametrize("typed, routed_words", [("hunter2-Zq9", "[redacted]"),
                                                  ("hunter2-Zq9  hunter2-Zq9", "[redacted] [redacted]")])
def test_an_edit_that_comes_back_hidden_is_never_saved(daemon: Any, typed: str, routed_words: str) -> None:
    """Secret classes are still hidden (rule 14), so an Edit answer can come back as nothing but "[redacted]" marks; it
    is refused and asked again, never saved as the owner's words (fix round 2: several marks too). In the answer field
    the question stays open; from chat (the planner routed already-hidden words) FERB asks again and says why."""
    daemon.daemon.redact.add_secret_value("hunter2-Zq9")
    q, it = guess(daemon)
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, 2))
    [edit] = pending(daemon, "memory.edit")
    with pytest.raises(FerbError, match="secret"):
        run(daemon, lambda: daemon.daemon._resolve_approval(edit.id, True, None, typed))
    assert [p.id for p in pending(daemon, "memory.edit")] == [edit.id]
    routed = run(daemon, lambda: daemon.daemon.state.approvals.resolve(edit.id, True, text=routed_words))
    line = run(daemon, lambda: daemon.daemon._question_answered(routed))
    [again] = pending(daemon, "memory.edit")
    assert "secret" in line and again.reason == edit.reason
    assert run(daemon, lambda: (daemon.daemon.memory.get(it.id).status, daemon.daemon.memory.items("active"))) == (
        "proposed", [])


def test_after_ferbs_which_is_it_about_a_password_guess_the_owners_keep_still_answers_it(daemon: Any) -> None:
    """Fix round 2: FERB's own clarification ("Is that Keep, Edit or Drop for this? Keep this? “You keep your passwords
    in 1Password” …") was read as a password question, so the owner's next reply was hidden whole: "Keep" reached the
    planner and the journal as "[redacted]", and the same clarification came back."""
    q, it = guess(daemon, **PASSWORD_GUESS)
    keep = json.dumps({"kind": "reply", "text": "", "answer_question": {"approval_id": q.id, "answer": "Keep"}})
    install(daemon, [keep])
    assert "Keep, Edit or Drop" in " ".join(str(e.get("text")) for e in chat(daemon, "yes"))
    assert run(daemon, lambda: daemon.daemon._hide_code_reply("thanks", "s1")) == "thanks"    # FERB asked about memory
    install(daemon, [keep])
    chat(daemon, "Keep")
    said_ = run(daemon, lambda: [e.data["text"] for e in daemon.daemon._journal().entries_of("chat.message")])
    assert said_[-1] == "Keep"
    assert run(daemon, lambda: daemon.daemon.memory.get(it.id)).status == "active"


def test_a_which_one_listing_the_planners_password_question_still_hides_a_password(daemon: Any) -> None:
    """Fix round 3 (rule 14): when "Keep" could answer both the guess and the planner's own pending question, FERB's
    "which one is it for?" lists both; that is no question about memory only, so a password typed next is hidden."""
    q, it = guess(daemon, **PASSWORD_GUESS)
    install(daemon, [json.dumps({"kind": "clarify", "text": "What's the Wi-Fi password for the router?"})])
    chat(daemon, "connect the TV to the wifi")
    install(daemon, [json.dumps({"kind": "reply", "text": "", "answer_question": {"approval_id": q.id,
                                                                                    "answer": "Keep"}})])
    asked = " ".join(str(e.get("text")) for e in chat(daemon, "Keep"))
    assert "could answer either" in asked and "Wi-Fi password" in asked
    assert run(daemon, lambda: daemon.daemon._hide_code_reply("hunter2!", "s1")) == "[redacted]"


def _send_check(daemon: Any) -> Any:
    """One of FERB's own Yes / No checks (D33), waiting outside any task."""
    from ferb.core.types import Identity
    from ferb.daemon.gateway import CONFIRM_ASK
    from ferb.policy.rules import ActionRequest
    action = ActionRequest(CONFIRM_ASK, "owner_question", Identity.OWNER, frozenset(), "", "", "req-x", frozenset())
    return daemon.daemon.state.approvals.question(action, "Send the draft to bob@example.com?", ["Yes", "No"])


@pytest.mark.parametrize("which", ["dentist guess", "password guess", "send check"])
def test_a_password_for_the_planners_question_ferb_displaced_is_still_hidden(daemon: Any, which: str) -> None:
    """Follow-up (rule 14): the planner asked "What's the Wi-Fi password for the router?"; the owner's "sure" went to a
    waiting check, and FERB's own "Is that … for this?" replaced the planner's question as the chat's pending one, so
    a password typed next for the planner's question reached the planner unhidden. An option is still a choice."""
    if which == "send check":
        q, option = run(daemon, lambda: _send_check(daemon)), "No"
    else:
        q, _ = guess(daemon, **(PASSWORD_GUESS if which == "password guess" else {}))
        option = "Drop"
    install(daemon, [json.dumps({"kind": "clarify", "text": "What's the Wi-Fi password for the router?"})])
    chat(daemon, "connect the TV to the wifi")
    install(daemon, [json.dumps({"kind": "reply", "text": "", "answer_question": {"approval_id": q.id,
                                                                                    "answer": q.options[0]}})])
    assert "Is that" in " ".join(str(e.get("text")) for e in chat(daemon, "sure"))          # FERB's own question
    assert run(daemon, lambda: [daemon.daemon._hide_code_reply(w, "s1") for w in ("hunter2!", option, option + ".")]) == [
        "[redacted]", option, option + "."]                 # a closing full stop is still the choice, as `_bare` reads it


def test_an_option_of_a_waiting_question_is_never_hidden_as_a_password(daemon: Any) -> None:
    """Fix round 2: a reply that is exactly an option of a waiting question is a choice, even after the planner's own
    question asked about a password; any other one-word reply to that question is still hidden (rule 14)."""
    guess(daemon, **PASSWORD_GUESS)
    continuity = daemon.daemon.supervisor.continuity
    run(daemon, lambda: (continuity.ensure("s1"), continuity.save_pending("s1", "log me in",
                                                                         "What's your Zomato password?")))
    assert run(daemon, lambda: [daemon.daemon._hide_code_reply(w, "s1") for w in ("Keep", "drop", "Bitwarden")]) == [
        "Keep", "drop", "[redacted]"]


def test_ferbs_which_is_it_names_a_checks_own_options() -> None:
    from types import SimpleNamespace

    from ferb.orchestrator.supervisor import _plain_choice
    assert _plain_choice(SimpleNamespace(options=("Keep", "Edit", "Drop"), reason="Keep this? “x”")) == (
        "Is that Keep, Edit or Drop for this? Keep this? “x”")
    assert _plain_choice(SimpleNamespace(options=("Yes", "No"), reason="Forget for good?")) == (
        "Is that a Yes or a No to this? Forget for good?")


def test_the_terminal_names_a_guess_questions_options() -> None:
    """`ferb chat`: the reply hint and the prompt name Keep, Edit or Drop; Enter (skipping) is Drop (ruling 5)."""
    import threading

    from ferb.cli.chat import question
    from ferb.cli.live_chat import LiveChat
    ask = {"approval_id": "ap-guess", "kind": "owner_question", "capability": "memory.confirm",
           "reason": "Keep this? “x”", "options": ["Keep", "Edit", "Drop"]}
    said: list[str] = []
    chat_ = LiveChat.__new__(LiveChat)
    chat_.pending, chat_._lock = {}, threading.RLock()
    chat_.say = said.append                                            # type: ignore[method-assign]
    chat_.answer_approval(ask)
    assert said[-1].strip() == "Reply Keep, Edit or Drop."
    assert question(ask).endswith("number or your answer (Enter = Drop): ")


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


def test_the_older_guesses_are_offered_once_as_one_list(daemon: Any) -> None:
    src = said(daemon, "x")
    old = [run(daemon, lambda c=c: daemon.daemon.memory.propose("fact", "owner", f"k{c}", f"guess number {c}",
                                                                "inferred", [("journal", src)], 0.6)) for c in range(3)]
    run(daemon, lambda: daemon.daemon._ask_keep(old[2], "Keep this?"))        # asked already: not in the list
    first = daemon.run(daemon.daemon._memory_learning(None, {"op": "backlog"}))["backlog"]
    assert first["text"] == "Review 2 older guesses"
    assert [p["id"] for p in first["proposals"]] == [old[0].id, old[1].id]
    assert daemon.run(daemon.daemon._memory_learning(None, {"op": "backlog"}))["backlog"] is None
    kept = daemon.run(daemon.daemon._memory_learning(None, {"op": "guess", "id": old[0].id, "action": "keep"}))
    assert kept["line"] == "Kept." and run(daemon, lambda: daemon.daemon.memory.get(old[0].id)).status == "active"
    with pytest.raises(FerbError):
        daemon.run(daemon.daemon._memory_learning(None, {"op": "guess", "id": old[1].id, "action": "edit", "text": " "}))
    edited = daemon.run(daemon.daemon._memory_learning(None, {"op": "guess", "id": old[1].id, "action": "edit",
                                                              "text": "my own words"}))
    assert edited["line"] == "Saved in your words: my own words"
