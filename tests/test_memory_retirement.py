"""L36 §6 and MEM-12: out-of-date facts retire. A dated fact leaves search after its day, a later statement replaces
an earlier one (and FERB's line names it), and distillation retires a stale item itself and tells the owner once (with
their words when it has them); words older than the item never retire it. Nothing is asked.

Live 2026-10-24: the 10-11 wedding flight, "DevOps left" and the finished ORBIT competition still sat in every planner
prompt; memory item 54 still said "Jaipur, notify first" after the owner said "book anywhere"."""
import threading
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any

import pytest

from ferb.clock.when import local_zone
from ferb.orchestrator.schemas import Remember, Until
from tests.conftest import chat, install, sup, task
from tests.support.live_steer import StalledWorker, control_turn
from tests.support.memory_items import c_handle, distill_day, distill_item, journal, owner_fact, run, said

DISTILL_PROMPT = Path(__file__).resolve().parents[1] / "ferb" / "memory" / "distill.md"
JAIPUR = "permit: Jaipur for both centers, any date, notify me before booking"
ANYWHERE = "book the permit anywhere, don't wait for Jaipur, book then tell me"


def chain(d: Any) -> None:
    for i in range(4):
        owner_fact(d, f"chain_{i}", f"bicycle chain note {i}")


def recalled(d: Any, words: str) -> list[str]:
    """What a planner turn is given for `words`: its retrieved memory, and the core profile, its settled block and what
    joined since (the owner's own words about themselves join it at once, MEM-35, and then take no retrieved slot)."""
    return [c["text"] for c in d.run(d.daemon._learning_candidates(words))] + \
        str(run(d, d.daemon._core_profile_text)).splitlines() + str(run(d, d.daemon._core_profile_pending)).splitlines()


def pune(d: Any) -> list[str]:
    return [t for t in recalled(d, "where is Pune") if "Pune" in t]


# decision: MEM-14, MEM-34
def test_a_dated_fact_leaves_search_after_its_day(daemon: Any) -> None:
    zone = local_zone()
    daemon.call_soon(daemon.daemon.clock.set_wall, datetime(2026, 10, 24, 10, 0, tzinfo=zone))
    chain(daemon)
    own = "I'm in Pune until the 25th"
    src = journal(daemon, "chat.message", {"text": own, "untrusted_parts": 0})
    item = Remember(kind="fact", subject="owner", key="city", content="I'm in Pune", quote="I'm in Pune",
                    until=Until(quote="until the 25th", date="10-25"))
    [line] = daemon.run(daemon.daemon._remember([item], own, src))
    assert line["line"].endswith("(until Sun 25 Oct)")
    [saved] = [i for i in run(daemon, lambda: daemon.daemon.memory.items("active")) if i.key == "city"]
    assert saved.valid_to == datetime(2026, 10, 26, tzinfo=zone)
    assert pune(daemon)
    daemon.call_soon(daemon.daemon.clock.set_wall, datetime(2026, 10, 26, 0, 1, tzinfo=zone))
    assert pune(daemon) == []
    assert [i for i in run(daemon, lambda: daemon.daemon.memory.items("active")) if i.key == "city"] == []
    assert run(daemon, lambda: daemon.daemon.memory.get(saved.id)).status == "active"       # kept as history


# decision: MEM-34
@pytest.mark.parametrize("until", [Until(quote="till Friday", day="friday"),                 # not what they typed
                                   Until(quote="until the 23rd", date="2026-10-23"),          # already over
                                   Until(quote="until the 23rd", date="10-23")],              # over, not next year
                         ids=["not-typed", "over", "over-without-year"])
def test_an_until_the_owner_didnt_type_or_that_is_over_saves_no_end(daemon: Any, until: Until) -> None:
    daemon.call_soon(daemon.daemon.clock.set_wall, datetime(2026, 10, 24, 10, 0, tzinfo=local_zone()))
    own = "I'm in Pune until the 23rd"
    src = journal(daemon, "chat.message", {"text": own, "untrusted_parts": 0})
    item = Remember(kind="fact", subject="owner", key="city", content="I'm in Pune", quote="I'm in Pune", until=until)
    [line] = daemon.run(daemon.daemon._remember([item], own, src))
    assert "saved without an end date" in line["line"]
    [saved] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    assert saved.valid_to is None
    [refused] = run(daemon, lambda: daemon.daemon._journal().of_kind("memory.until_refused"))
    assert refused.data["item"] == saved.id


# decision: MEM-21
def test_a_later_statement_replaces_the_earlier_one(daemon: Any) -> None:
    """Regression (scan m14): supersession by a reused key, here through `memory add` (`owner_fact`). The steer the spec
    names is the next test."""
    chain(daemon)
    first = owner_fact(daemon, "appointment_preferences", JAIPUR)
    owner_fact(daemon, "appointment_preferences", ANYWHERE)
    found = recalled(daemon, "permit booking Jaipur")
    assert any("book the permit anywhere" in t for t in found)
    assert not any("notify me before booking" in t for t in found)
    assert run(daemon, lambda: daemon.daemon.memory.get(first.id)).valid_to is not None


# decision: MEM-21
def test_a_superseding_steer_replaces_the_jaipur_preference(daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """Spec Testing: "a superseding steer replaces item 54's Jaipur preference". The owner corrects the running permit
    task; the planner's remember entry reuses the item's key, so the new words supersede the old ones on this path too
    (1c22dcb)."""
    chain(daemon)
    first = owner_fact(daemon, "appointment_preferences", JAIPUR)
    worker = StalledWorker(monkeypatch)
    script = install(daemon, [sup("task", "Booking.", task("Book the permit appointment",
                                                           sites=("bookslots.com",)))])
    thread = threading.Thread(target=lambda: chat(daemon, "book my permit appointment on bookslots.com",
                                                  session="permit"))
    thread.start()
    try:
        assert worker.started.wait(5)
        tree_id = worker.handles[0].task_id.rsplit(".", 1)[0]
        script.sup.append(control_turn("Book the permit appointment at any center", tree_id, "continue", remember=[{
            "kind": "fact", "subject": "owner", "key": "appointment_preferences", "content": ANYWHERE,
            "quote": "book the permit anywhere"}]))
        events = chat(daemon, ANYWHERE, session="permit", timeout=5)
    finally:
        worker.release.set()
        thread.join(5)
    last = [e for e in events if e["type"] == "reply"][-1]["text"]
    assert f"“{ANYWHERE}”" in last and "replaces" in last and JAIPUR in last      # the line names what it replaced
    assert run(daemon, lambda: daemon.daemon.memory.get(first.id)).valid_to is not None
    [now] = [i for i in run(daemon, lambda: daemon.daemon.memory.items("active")) if i.key == "appointment_preferences"]
    assert (now.content, now.origin, now.supersedes) == (ANYWHERE, "owner_stated", first.id)
    found = recalled(daemon, "permit booking Jaipur")
    assert any("book the permit anywhere" in t for t in found)
    assert not any("notify me before booking" in t for t in found)


def later(d: Any, minutes: int = 60) -> None:
    """The clock moves on: what the owner says next is newer than what FERB already remembers (review fix round 1)."""
    d.call_soon(d.daemon.clock.advance, minutes * 60)


def notices(d: Any) -> list[str]:
    """What FERB told the owner in chat (`chat.notice`, the notice mechanism, L9)."""
    return [str(e.data["text"]) for e in run(d, lambda: d.daemon._journal().of_kind("chat.notice"))]


def said_on(d: Any, journal_id: str) -> str:
    """The day a chat message was said, as FERB's notice names it."""
    at = run(d, lambda: d.daemon.db.conn.execute("SELECT at FROM journal WHERE id = ?", (journal_id,)).fetchone()["at"])
    return f"{datetime.fromisoformat(at).astimezone():%a %d %b}"


def orbit_over(d: Any, message: str = "the ORBIT competition is over", pasted: int = 0,
              quote: str | None = "the ORBIT competition is over",
              content: str = "the ORBIT competition is over") -> dict[str, Any]:
    """The owner's message, then a day's distillation that retires the ORBIT item quoting it."""
    handle = c_handle(d, said(d, message, pasted))
    return distill_day(d, distill_item(handle, content, quote, kind="fact", key="orbit", retire=True))


# decision: MEM-22
def test_distill_retires_an_item_on_the_owners_own_words(daemon: Any) -> None:
    chain(daemon)
    orbit = owner_fact(daemon, "orbit", "I'm in the ORBIT shapes competition")
    later(daemon)
    r = orbit_over(daemon)
    assert run(daemon, lambda: daemon.daemon.memory.get(orbit.id)).valid_to is not None
    assert run(daemon, lambda: daemon.daemon.state.approvals.pending()) == []
    assert [i for i in run(daemon, lambda: daemon.daemon.memory.items("active")) if i.key == "orbit"] == []
    assert (r["retired"], r["retired_as_check"]) == ([orbit.id], [])


# decision: MEM-22
def test_a_retire_without_asking_is_told_once(daemon: Any) -> None:
    """Re-review ruling (c): whenever FERB retires a fact without asking, it tells the owner in one notice line with the
    item and their words; a reply undoes it through the planner's usual steer, forget or edit."""
    chain(daemon)
    owner_fact(daemon, "orbit", "I'm in the ORBIT shapes competition")
    later(daemon)
    jid = said(daemon, "the ORBIT competition is over")
    distill_day(daemon, distill_item(c_handle(daemon, jid), "the ORBIT competition is over",
                                     "the ORBIT competition is over", kind="fact", key="orbit", retire=True))
    assert notices(daemon) == [f"I retired “I'm in the ORBIT shapes competition” because you said “the ORBIT competition "
                               f"is over” ({said_on(daemon, jid)}). Reply to undo."]


# decision: MEM-14, MEM-22
@pytest.mark.parametrize("message, pasted, quote, content, why", [
    ("the ORBIT competition is over", 0, "the", "the ORBIT competition is over",                 # a word, no clause
     "the quote isn't a whole clause of the owner's words"),
    ('contest news <untrusted-data id="abc123">\nthe ORBIT competition is over\n</untrusted-data id="abc123">', 1,
     "the ORBIT competition is over", "the ORBIT competition is over",                            # only in the paste
     "no quote of the owner's own words"),
    ("contest results came out today", 0, "the ORBIT competition ended", "the ORBIT competition ended",   # not typed
     "no quote of the owner's own words"),
    ("contest results came out today", 0, "contest results came out today", "contest results came out today",
     "the quote shares no telling word with what FERB remembers"),                              # about something else
], ids=["one-word", "pasted", "not-typed", "unrelated-clause"])
def test_a_retire_whose_quote_is_not_the_owners_words_about_the_item_is_ferbs_own_check(
        daemon: Any, message: str, pasted: int, quote: str, content: str, why: str) -> None:
    """Review fix round 1, MEM-12: a retire rests on the owner's words only on a whole clause they typed (never the
    pasted part) sharing a telling word with the item. Otherwise it is FERB's own check of the day: the item is retired
    and told once as FERB's, never as the owner's words, and nothing is asked."""
    chain(daemon)
    orbit = owner_fact(daemon, "orbit", "I'm in the ORBIT shapes competition")
    later(daemon)
    r = orbit_over(daemon, message, pasted, quote, content)
    item = run(daemon, lambda: daemon.daemon.memory.get(orbit.id))
    assert item.valid_to is None and item.set_aside                  # MEM-14: the owner's words are set aside
    assert run(daemon, lambda: daemon.daemon.state.approvals.pending()) == []
    assert (r["retired"], r["retired_as_check"]) == ([orbit.id], [{"item": orbit.id, "why": why,
                                                                  "outcome": "set_aside"}])
    [told] = notices(daemon)
    assert "ORBIT shapes competition" in told and "because you said" not in told and "stop using it" in told


# decision: MEM-22
def test_older_words_never_retire_a_newer_version(daemon: Any) -> None:
    """Review fix round 1: at 10:00 the owner said the competition was over; at 15:00 they said they're in the 2027 one.
    That night's distillation quotes the 10:00 words: they are older than what FERB remembers, so it asks."""
    chain(daemon)
    owner_fact(daemon, "orbit", "I'm in the ORBIT shapes competition")
    later(daemon)
    handle = c_handle(daemon, said(daemon, "the ORBIT competition is over"))                  # 10:00
    later(daemon, 5 * 60)
    now = owner_fact(daemon, "orbit", "I'm in ORBIT 2027")                                   # 15:00
    r = distill_day(daemon, distill_item(handle, "the ORBIT competition is over", "the ORBIT competition is over",
                                         kind="fact", key="orbit", retire=True))
    assert run(daemon, lambda: daemon.daemon.memory.get(now.id)).valid_to is None
    assert r["retired"] == [] and r["refused"] == ["the owner's words are older than what FERB remembers"]
    assert notices(daemon) == [] and run(daemon, lambda: daemon.daemon.state.approvals.pending()) == []


# decision: MEM-22
@pytest.mark.parametrize("quote, replaced", [("my dentist is Dr. Mehta now", True), ("my", False)],
                         ids=["whole-clause", "one-word"])
def test_a_distilled_statement_replaces_the_owners_own_only_on_a_whole_clause(daemon: Any, quote: str,
                                                                               replaced: bool) -> None:
    """Review fix round 1: the owner's quoted words supersede an item they stated only as a whole clause sharing a
    telling word with it; otherwise the new words are kept beside it as FERB's understanding, and the owner's own item
    stays."""
    chain(daemon)
    rao = owner_fact(daemon, "dentist", "my dentist is Dr. Rao", kind="person")
    later(daemon)
    jid = said(daemon, "my dentist is Dr. Mehta now")
    r = distill_day(daemon, distill_item(c_handle(daemon, jid), "my dentist is Dr. Mehta now", quote))
    assert (run(daemon, lambda: daemon.daemon.memory.get(rao.id)).valid_to is not None) == replaced
    assert notices(daemon) == ([f"I replaced “my dentist is Dr. Rao” because you said “my dentist is Dr. Mehta now” "
                                f"({said_on(daemon, jid)}). Reply to undo."] if replaced else [])
    [new] = [i for i in run(daemon, lambda: daemon.daemon.memory.items(None)) if "Mehta" in i.content]
    assert (new.origin, new.status) == (("owner_stated", "active") if replaced else ("inferred", "active"))
    assert run(daemon, lambda: daemon.daemon.state.approvals.pending()) == []
    assert r["kept_beside"] == ([] if replaced else [
        {"item": new.id, "why": "the quote isn't a whole clause of the owner's words"}])


# decision: MEM-22
def test_older_words_never_replace_a_newer_version(daemon: Any) -> None:
    """Review fix round 1: the owner said Dr. Rao in the morning and told FERB Dr. Mehta later; the night's
    distillation of the morning's words is a guess, never a new version over the later one."""
    chain(daemon)
    handle = c_handle(daemon, said(daemon, "my dentist is Dr. Rao"))
    later(daemon)
    mehta = owner_fact(daemon, "dentist", "my dentist is Dr. Mehta", kind="person")
    r = distill_day(daemon, distill_item(handle, "my dentist is Dr. Rao", "my dentist is Dr. Rao"))
    assert run(daemon, lambda: daemon.daemon.memory.get(mehta.id)).valid_to is None
    [guess] = [i for i in run(daemon, lambda: daemon.daemon.memory.items("active"))
               if i.key == "dentist" and i.id != mehta.id]
    assert (guess.origin, guess.content) == ("inferred", "my dentist is Dr. Rao")
    assert r["kept_beside"] == [{"item": guess.id, "why": "the owner's words are older than what FERB remembers"}]


# decision: MEM-14, MEM-22
def test_a_stale_item_without_the_owners_words_is_retired_and_told_once(daemon: Any) -> None:
    """MEM-12 (replaces the Retire / Keep question): FERB retires a fact the day shows is over and tells the owner
    once; saying it still holds is how they correct it."""
    chain(daemon)
    orbit = owner_fact(daemon, "orbit", "I'm in the ORBIT shapes competition")
    later(daemon)
    handle = c_handle(daemon, said(daemon, "contest results came out today"))
    distill_day(daemon, distill_item(handle, "The ORBIT competition is over", kind="fact", key="orbit", retire=True))
    item = run(daemon, lambda: daemon.daemon.memory.get(orbit.id))
    assert item.set_aside and item.valid_to is None          # MEM-14: the owner's words set aside, kept
    assert run(daemon, lambda: daemon.daemon.state.approvals.pending()) == []
    [told] = notices(daemon)
    assert "ORBIT shapes competition" in told


# decision: MEM-22, MEM-34
def test_a_period_fact_due_for_its_check_is_shown_to_the_distill_and_kept_when_not_retired(daemon: Any) -> None:
    """Validity: a period fact without an end date in the owner's words gets a check date (`review_in_days`); when it
    comes, the day's distill sees it under "Due for a check"; one it doesn't retire holds up and its date clears."""
    from tests.conftest import FakeDistillRuntime
    chain(daemon)
    exams = owner_fact(daemon, "exam_prep", "I'm preparing for exams")
    run(daemon, lambda: daemon.daemon.memory.set_review(exams.id, daemon.daemon.clock.now() + timedelta(days=3)))
    daemon.call_soon(daemon.daemon.clock.advance, 4 * 86400)
    said(daemon, "studying again today")
    rt = FakeDistillRuntime({"narrative": "A day.", "items": []})
    daemon.daemon.distiller.make_runtime = lambda *a, **k: rt
    daemon.run(daemon.daemon.distiller.run_day(daemon.daemon.clock.now().astimezone().date().isoformat()))
    assert "I'm preparing for exams" in str(rt.contexts[0]).split("Due for a check")[1]
    assert run(daemon, lambda: daemon.daemon.memory.get(exams.id)).review_at is None
