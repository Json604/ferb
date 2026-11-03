"""Capture without Keep questions (MEM-12): the owner's words are saved as theirs, anything else
FERB works out from what they said is saved at once as FERB's understanding, with a quiet "Memory: noted …" line and
correction by talking; durable facts said in passing are captured (MEM-8), from the planner's remember and the
daily distill's chat pass.

Live 2026-10-24 (memory audit): of the owner's durable statements in 14 days, 14 were missed and 9 only proposed;
live 2026-10-30: "what are some easy piano pieces? I'm a beginner" stored nothing."""
import json
from typing import Any

import pytest

from ferb.memory.items import capture
from tests.conftest import chat, install
from tests.support.memory_items import c_handle, distill_day, distill_item, owner_fact, run, said


# decision: MEM-12
def test_capture_saves_the_owners_sentence_their_guess_or_nothing() -> None:
    own = "btw my dentist is Dr. Rao now"
    stated = capture(own, "my dentist is Dr. Rao", "my dentist is Dr. Rao")
    assert (stated.origin, stated.content, stated.refused) == ("owner_stated", own, "")
    assert capture(own, "Your dentist is Dr. Rao", "my dentist is Dr. Rao").origin == "inferred"
    assert capture(own, "Your dentist is Dr. Rao", "my dentist is Dr. Mehta").refused == "quote not in the owner's words"


# decision: MEM-12
def test_a_quote_of_the_owners_typed_words_saves_their_own_words(daemon: Any) -> None:
    jid = said(daemon, "btw my dentist is Dr. Rao now")
    r = distill_day(daemon, distill_item(c_handle(daemon, jid), "my dentist is Dr. Rao", "my dentist is Dr. Rao"))
    assert r["proposed"] == 1, r
    [it] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    assert (it.origin, it.content, it.source_ids) == ("owner_stated", "btw my dentist is Dr. Rao now",
                                                      ["journal:" + jid])
    assert run(daemon, lambda: daemon.daemon.state.approvals.pending()) == []


# decision: MEM-8, MEM-12
@pytest.mark.parametrize("message, pasted, content, quote", [
    ("my dentist is Dr. Rao", 0, "Your dentist is Dr. Rao", "my dentist is Dr. Mehta"),     # not what they typed
    ("my dentist is Dr. Rao", 0, "Your dentist is Dr. Rao", None),                          # no quote
    ('note <untrusted-data id="abc123">\nmy dentist is Dr. Rao\n</untrusted-data id="abc123">', 1,
     "my dentist is Dr. Rao", "my dentist is Dr. Rao"),                                     # only in the pasted part
    ("my dentist is Dr. Rao", None, "my dentist is Dr. Rao", "my dentist is Dr. Rao"),      # pastes weren't counted
])
def test_without_the_owners_typed_words_it_is_saved_as_ferbs_understanding(daemon: Any, message: str,
                                                                           pasted: int | None, content: str,
                                                                           quote: str | None) -> None:
    jid = said(daemon, message, pasted)
    distill_day(daemon, distill_item(c_handle(daemon, jid), content, quote))
    [it] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    assert (it.origin, it.content) == ("inferred", content)
    assert run(daemon, lambda: daemon.daemon.state.approvals.pending()) == []          # no Keep question


# MEM-8's real chats (kb area memory): durable facts said in passing, through the real chat path. The planner
# here is scripted; the pipeline under test is what code does with its remember entries.
PASSING = [
    ("what are some easy piano pieces? I'm a beginner", "fact", "piano_skill_level", "I'm a beginner",
     "I'm a beginner"),
    ("make a short summary list, I can't read this much jargon", "preference", "summary_style",
     "The owner wants short summary lists without jargon", "I can't read this much jargon"),
    ("sounds too linkedinish, rewrite it", "preference", "writing_tone",
     "The owner doesn't want writing that sounds too linkedinish", "sounds too linkedinish"),
    ("do contest on my brave contest personal account", "preference", "contest_account",
     "do contest on my brave contest personal account", "on my brave contest personal account"),
]


# decision: MEM-8, MEM-10, MEM-12
@pytest.mark.parametrize("message, kind, key, content, quote", PASSING)
def test_a_durable_fact_said_in_passing_is_saved_with_a_quiet_line(daemon: Any, message: str, kind: str, key: str,
                                                                   content: str, quote: str) -> None:
    install(daemon, [json.dumps({"kind": "reply", "text": "Here you go.", "remember": [
        {"kind": kind, "subject": "owner", "key": key, "content": content, "quote": quote}]})])
    events = chat(daemon, message)
    [it] = run(daemon, lambda: [i for i in daemon.daemon.memory.items("active") if i.key == key])
    assert it.origin == ("owner_stated" if content in message else "inferred")
    lines = [e["text"] for e in events if e.get("type") == "reply"]
    assert any("Memory: noted" in line for line in lines), events
    assert run(daemon, lambda: daemon.daemon.state.approvals.pending()) == []
    assert it.about                                       # MEM-10: what the words are about, stored beside them


# decision: MEM-8
def test_the_daily_distill_captures_a_fact_said_in_passing(daemon: Any) -> None:
    jid = said(daemon, "what are some easy piano pieces? I'm a beginner")
    r = distill_day(daemon, distill_item(c_handle(daemon, jid), "The owner is a beginner at piano", None,
                                         kind="fact", key="piano_skill_level"))
    assert r["proposed"] == 1, r
    [it] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    assert (it.origin, it.key) == ("inferred", "piano_skill_level")


# decision: MEM-22
def test_the_distill_never_proposes_a_stored_fact_again(daemon: Any) -> None:
    """Fix list: the live store's four duplicate pairs came from the distill re-proposing keys already stored."""
    owner_fact(daemon, "dentist", "my dentist is Dr. Rao", kind="person")
    jid = said(daemon, "booked Dr. Rao again")
    r = distill_day(daemon, distill_item(c_handle(daemon, jid), "Your dentist is Dr. Rao"))
    assert r["proposed"] == 0 and any("already remembered" in why for why in r["refused"]), r
    assert len(run(daemon, lambda: daemon.daemon.memory.items("active"))) == 1


# decision: MEM-12, MEM-21
def test_correction_by_talking_replaces_the_wrong_fact(daemon: Any) -> None:
    """MEM-12: "that's wrong, …" — the planner remembers the corrected fact naming the wrong one; code checks the
    id, ends it, and FERB's line names what it replaced. Live 2026-11-04: the whole correction was saved as the fact;
    only the clause that states it is (MEM-419)."""
    wrong = owner_fact(daemon, "work_browser", "I use Brave for work", kind="preference")
    install(daemon, [json.dumps({"kind": "reply", "text": "Got it.", "remember": [
        {"kind": "preference", "subject": "owner", "key": "browser_for_work", "content": "I use Arc for work now",
         "quote": "I use Arc for work now", "replaces": [wrong.id]}]})])
    events = chat(daemon, "that's wrong, I use Arc for work now")
    assert run(daemon, lambda: daemon.daemon.memory.get(wrong.id)).valid_to is not None
    [now] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    assert now.content == "I use Arc for work now" and now.origin == "owner_stated"
    lines = " ".join(e["text"] for e in events if e.get("type") == "reply")
    assert "replaces" in lines and "I use Brave for work" in lines


# decision: MEM-21
def test_a_replaced_id_that_holds_nothing_replaces_nothing(daemon: Any) -> None:
    install(daemon, [json.dumps({"kind": "reply", "text": "Got it.", "remember": [
        {"kind": "preference", "subject": "owner", "key": "tea", "content": "I like green tea",
         "quote": "I like green tea", "replaces": ["no-such-id"]}]})])
    chat(daemon, "I like green tea")
    assert [i.content for i in run(daemon, lambda: daemon.daemon.memory.items("active"))] == ["I like green tea"]
    assert run(daemon, lambda: daemon.daemon.journal.of_kind("memory.replace_refused"))
