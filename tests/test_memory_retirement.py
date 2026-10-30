"""L36 §6: out-of-date facts retire. A dated fact leaves search after its day, a later statement supersedes an earlier
one, and distillation retires an item only on the owner's own words (otherwise it asks).

Live 2026-10-24: the 10-11 wedding flight, "DevOps left" and the finished RSNA competition still sat in every planner
prompt; memory item 54 still said "Chennai, notify first" after the owner said "book anywhere"."""
import threading
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any
from zoneinfo import ZoneInfo

import pytest

from ferb.clock.when import When, WhenError, end_of_day, local_zone
from ferb.orchestrator.schemas import Remember, Until
from tests.conftest import chat, install, sup, task
from tests.support.live_steer import StalledWorker, control_turn
from tests.support.memory_items import c_handle, distill_day, distill_item, journal, owner_fact, pending, run, said

DISTILL_PROMPT = Path(__file__).resolve().parents[1] / "ferb" / "memory" / "distill.md"
CHENNAI = "visa: Chennai for both centers, any date, notify me before booking"
ANYWHERE = "book the visa anywhere, don't wait for Chennai, book then tell me"


def test_end_of_day_is_the_next_local_midnight() -> None:
    zone = ZoneInfo("Asia/Kolkata")
    now = datetime(2026, 10, 24, 10, 0, tzinfo=zone)
    assert end_of_day(When(date="10-25"), now, zone) == datetime(2026, 10, 26, tzinfo=zone).astimezone(timezone.utc)
    assert end_of_day(When(day="today"), now, zone) == datetime(2026, 10, 25, tzinfo=zone).astimezone(timezone.utc)
    with pytest.raises(WhenError):
        end_of_day(When(date="2026-10-23"), now, zone)                      # already over
    with pytest.raises(WhenError):
        end_of_day(When(day="today", time="18:00"), now, zone)              # a day, not a time


@pytest.mark.parametrize("today, said, last", [
    ((2026, 12, 31), "01-02", (2027, 1, 2)),          # across the new year: next year's, two days on
    ((2027, 1, 2), "12-30", None),                    # three days ago, last year's: over (not next December)
    ((2026, 10, 24), "10-23", None),                  # yesterday: over (scan m13), never a year away
    ((2026, 10, 24), "10-24", (2026, 10, 24)),        # today counts
    ((2027, 3, 1), "02-29", (2028, 2, 29)),           # only next year's exists
], ids=["new-year", "last-december", "yesterday", "today", "leap-day"])
def test_a_month_and_day_is_the_occurrence_nearest_today(today: tuple[int, int, int], said: str,
                                                        last: tuple[int, int, int] | None) -> None:
    """Fix round 1 (review): `_day` turns a past "MM-DD" into next year's (right for a reminder). An item's last day the
    owner gave without a year is the occurrence nearest today (last year's, this year's or next year's; ties toward
    the future), and one behind today is over."""
    zone = ZoneInfo("Asia/Kolkata")
    now = datetime(*today, 10, 0, tzinfo=zone)
    if last is None:
        with pytest.raises(WhenError, match="already over"):
            end_of_day(When(date=said), now, zone)
    else:
        assert end_of_day(When(date=said), now, zone) == (datetime(*last, tzinfo=zone) + timedelta(days=1)).astimezone(
            timezone.utc)


@pytest.mark.parametrize("today, said, shown", [((2027, 1, 2), "12-30", "(Wed 30 Dec 2026)"),
                                                ((2026, 10, 24), "10-23", "(Fri 23 Oct)")],
                         ids=["last-year", "this-year"])
def test_a_day_already_over_names_its_year_when_it_is_another(today: tuple[int, int, int], said: str,
                                                             shown: str) -> None:
    """Re-review minor: the refusal names the year of a day in another year, so the owner sees which one was meant."""
    zone = ZoneInfo("Asia/Kolkata")
    with pytest.raises(WhenError) as refused:
        end_of_day(When(date=said), datetime(*today, 10, 0, tzinfo=zone), zone)
    assert str(refused.value) == f"that day is already over {shown}"


def chain(d: Any) -> None:
    for i in range(4):
        owner_fact(d, f"chain_{i}", f"bicycle chain note {i}")


def pune(d: Any) -> list[str]:
    return [c["text"] for c in d.run(d.daemon._learning_candidates("where is Pune")) if "Pune" in c["text"]]


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


def test_a_later_statement_replaces_the_earlier_one(daemon: Any) -> None:
    """Regression (scan m14): supersession by a reused key, here through `memory add` (`owner_fact`). The steer the spec
    names is the next test."""
    chain(daemon)
    first = owner_fact(daemon, "appointment_preferences", CHENNAI)
    owner_fact(daemon, "appointment_preferences", ANYWHERE)
    found = [c["text"] for c in daemon.run(daemon.daemon._learning_candidates("visa booking Chennai"))]
    assert any("book the visa anywhere" in t for t in found)
    assert not any("notify me before booking" in t for t in found)
    assert run(daemon, lambda: daemon.daemon.memory.get(first.id)).valid_to is not None


def test_a_superseding_steer_replaces_the_chennai_preference(daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """Spec Testing: "a superseding steer replaces item 54's Chennai preference". The owner corrects the running visa
    task; the planner's remember entry reuses the item's key, so the new words supersede the old ones on this path too
    (1c22dcb)."""
    chain(daemon)
    first = owner_fact(daemon, "appointment_preferences", CHENNAI)
    worker = StalledWorker(monkeypatch)
    script = install(daemon, [sup("task", "Booking.", task("Book the visa appointment",
                                                           sites=("usvisascheduling.com",)))])
    thread = threading.Thread(target=lambda: chat(daemon, "book my visa appointment on usvisascheduling.com",
                                                  session="visa"))
    thread.start()
    try:
        assert worker.started.wait(5)
        tree_id = worker.handles[0].task_id.rsplit(".", 1)[0]
        script.sup.append(control_turn("Book the visa appointment at any center", tree_id, "continue", remember=[{
            "kind": "fact", "subject": "owner", "key": "appointment_preferences", "content": ANYWHERE,
            "quote": "book the visa anywhere"}]))
        events = chat(daemon, ANYWHERE, session="visa", timeout=5)
    finally:
        worker.release.set()
        thread.join(5)
    assert [e for e in events if e["type"] == "reply"][-1]["text"].endswith("Remembered: " + ANYWHERE)
    assert run(daemon, lambda: daemon.daemon.memory.get(first.id)).valid_to is not None
    [now] = [i for i in run(daemon, lambda: daemon.daemon.memory.items("active")) if i.key == "appointment_preferences"]
    assert (now.content, now.origin, now.supersedes) == (ANYWHERE, "owner_stated", first.id)
    found = [c["text"] for c in daemon.run(daemon.daemon._learning_candidates("visa booking Chennai"))]
    assert any("book the visa anywhere" in t for t in found)
    assert not any("notify me before booking" in t for t in found)


def test_the_distiller_is_told_how_to_retire() -> None:
    """Runtime prompts are executable input (CLAUDE.md; scan ruling I12): the retire instruction stays word for word."""
    words = " ".join(DISTILL_PROMPT.read_text().split())
    assert ('- "retire": true when the owner\'s words say something under "What FERB already remembers" is no longer '
            'true ("the RSNA competition is over"): give that item\'s kind, subject and key, origin "inferred", '
            'content: the owner\'s words that say so, copied from that line, and "quote": the whole clause it\'s in, '
            'as above; FERB retires it only when the quote is theirs, and otherwise asks the owner. Every other item '
            'has "retire": false.') in words


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


def rsna_over(d: Any, message: str = "the RSNA competition is over", pasted: int = 0,
              quote: str | None = "the RSNA competition is over",
              content: str = "the RSNA competition is over") -> dict[str, Any]:
    """The owner's message, then a day's distillation that retires the RSNA item quoting it."""
    handle = c_handle(d, said(d, message, pasted))
    return distill_day(d, distill_item(handle, content, quote, kind="fact", key="rsna", retire=True))


def test_distill_retires_an_item_on_the_owners_own_words(daemon: Any) -> None:
    chain(daemon)
    rsna = owner_fact(daemon, "rsna", "I'm in the RSNA knee competition")
    later(daemon)
    r = rsna_over(daemon)
    assert run(daemon, lambda: daemon.daemon.memory.get(rsna.id)).valid_to is not None
    assert pending(daemon, "memory.retire") == []
    assert [i for i in run(daemon, lambda: daemon.daemon.memory.items("active")) if i.key == "rsna"] == []
    assert (r["retired"], r["retire_unconfirmed"]) == ([rsna.id], [])


def test_a_retire_without_asking_is_told_once(daemon: Any) -> None:
    """Re-review ruling (c): whenever FERB retires a fact without asking, it tells the owner in one notice line with the
    item and their words; a reply undoes it through the planner's usual steer, forget or edit."""
    chain(daemon)
    owner_fact(daemon, "rsna", "I'm in the RSNA knee competition")
    later(daemon)
    jid = said(daemon, "the RSNA competition is over")
    distill_day(daemon, distill_item(c_handle(daemon, jid), "the RSNA competition is over",
                                     "the RSNA competition is over", kind="fact", key="rsna", retire=True))
    assert notices(daemon) == [f"I retired “I'm in the RSNA knee competition” because you said “the RSNA competition "
                               f"is over” ({said_on(daemon, jid)}). Reply to undo."]


@pytest.mark.parametrize("message, pasted, quote, content, why", [
    ("the RSNA competition is over", 0, "the", "the RSNA competition is over",                 # a word, no clause
     "the quote isn't a whole clause of the owner's words"),
    ('kaggle news <untrusted-data id="abc123">\nthe RSNA competition is over\n</untrusted-data id="abc123">', 1,
     "the RSNA competition is over", "the RSNA competition is over",                            # only in the paste
     "no quote of the owner's own words"),
    ("kaggle results came out today", 0, "the RSNA competition ended", "the RSNA competition ended",   # not typed
     "no quote of the owner's own words"),
    ("kaggle results came out today", 0, "kaggle results came out today", "kaggle results came out today",
     "the quote shares no telling word with what FERB remembers"),                              # about something else
], ids=["one-word", "pasted", "not-typed", "unrelated-clause"])
def test_a_retire_whose_quote_is_not_a_clause_of_the_owners_words_about_the_item_is_asked(
        daemon: Any, message: str, pasted: int, quote: str, content: str, why: str) -> None:
    """Review fix round 1: ending something FERB remembers on the owner's words needs a whole clause they typed (a chat
    forget's rule, `whole_clauses`; never the pasted part) sharing a telling word with what the item says. Otherwise
    the owner is asked, and the item stays current."""
    chain(daemon)
    rsna = owner_fact(daemon, "rsna", "I'm in the RSNA knee competition")
    later(daemon)
    r = rsna_over(daemon, message, pasted, quote, content)
    assert run(daemon, lambda: daemon.daemon.memory.get(rsna.id)).valid_to is None
    [q] = pending(daemon, "memory.retire")
    assert "RSNA knee competition" in q.reason
    assert (r["retired"], r["retire_unconfirmed"]) == ([], [{"item": rsna.id, "why": why}])
    assert notices(daemon) == []                                       # asked, so nothing to tell


def test_older_words_never_retire_a_newer_version(daemon: Any) -> None:
    """Review fix round 1: at 10:00 the owner said the competition was over; at 15:00 they said they're in the 2027 one.
    That night's distillation quotes the 10:00 words: they are older than what FERB remembers, so it asks."""
    chain(daemon)
    owner_fact(daemon, "rsna", "I'm in the RSNA knee competition")
    later(daemon)
    handle = c_handle(daemon, said(daemon, "the RSNA competition is over"))                  # 10:00
    later(daemon, 5 * 60)
    now = owner_fact(daemon, "rsna", "I'm in RSNA 2027")                                   # 15:00
    r = distill_day(daemon, distill_item(handle, "the RSNA competition is over", "the RSNA competition is over",
                                         kind="fact", key="rsna", retire=True))
    assert run(daemon, lambda: daemon.daemon.memory.get(now.id)).valid_to is None
    [q] = pending(daemon, "memory.retire")
    assert "RSNA 2027" in q.reason
    assert r["retire_unconfirmed"] == [{"item": now.id, "why": "the owner's words are older than what FERB remembers"}]


@pytest.mark.parametrize("quote, replaced", [("my dentist is Dr. Mehta now", True), ("my", False)],
                         ids=["whole-clause", "one-word"])
def test_a_distilled_statement_replaces_the_owners_own_only_on_a_whole_clause(daemon: Any, quote: str,
                                                                               replaced: bool) -> None:
    """Review fix round 1: the owner's quoted words supersede an item they stated only as a whole clause sharing a
    telling word with it; otherwise the new words are FERB's guess, asked with Keep / Edit / Drop, and the owner's own
    item stays."""
    chain(daemon)
    rao = owner_fact(daemon, "dentist", "my dentist is Dr. Rao", kind="person")
    later(daemon)
    jid = said(daemon, "my dentist is Dr. Mehta now")
    r = distill_day(daemon, distill_item(c_handle(daemon, jid), "my dentist is Dr. Mehta now", quote))
    assert (run(daemon, lambda: daemon.daemon.memory.get(rao.id)).valid_to is not None) == replaced
    assert notices(daemon) == ([f"I replaced “my dentist is Dr. Rao” because you said “my dentist is Dr. Mehta now” "
                                f"({said_on(daemon, jid)}). Reply to undo."] if replaced else [])
    [new] = [i for i in run(daemon, lambda: daemon.daemon.memory.items(None)) if "Mehta" in i.content]
    assert (new.origin, new.status) == (("owner_stated", "active") if replaced else ("inferred", "proposed"))
    assert len(pending(daemon)) == (0 if replaced else 1)
    assert r["kept_as_guess"] == ([] if replaced else [
        {"item": new.id, "why": "the quote isn't a whole clause of the owner's words"}])


def test_older_words_never_replace_a_newer_version(daemon: Any) -> None:
    """Review fix round 1: the owner said Dr. Rao in the morning and told FERB Dr. Mehta later; the night's
    distillation of the morning's words is a guess, never a new version over the later one."""
    chain(daemon)
    handle = c_handle(daemon, said(daemon, "my dentist is Dr. Rao"))
    later(daemon)
    mehta = owner_fact(daemon, "dentist", "my dentist is Dr. Mehta", kind="person")
    r = distill_day(daemon, distill_item(handle, "my dentist is Dr. Rao", "my dentist is Dr. Rao"))
    assert run(daemon, lambda: daemon.daemon.memory.get(mehta.id)).valid_to is None
    [guess] = run(daemon, lambda: daemon.daemon.memory.items("proposed"))
    assert (guess.origin, guess.content) == ("inferred", "my dentist is Dr. Rao")
    assert r["kept_as_guess"] == [{"item": guess.id, "why": "the owner's words are older than what FERB remembers"}]


def test_a_retire_without_the_owners_words_is_asked(daemon: Any) -> None:
    chain(daemon)
    rsna = owner_fact(daemon, "rsna", "I'm in the RSNA knee competition")
    later(daemon)
    handle = c_handle(daemon, said(daemon, "kaggle results came out today"))
    distill_day(daemon, distill_item(handle, "The RSNA competition is over", kind="fact", key="rsna", retire=True))
    assert run(daemon, lambda: daemon.daemon.memory.get(rsna.id)).valid_to is None
    [q] = pending(daemon, "memory.retire")
    assert q.options == ("Retire", "Keep") and "RSNA" in q.reason
    assert q.reason == ("Is this no longer true? “I'm in the RSNA knee competition” FERB thinks something you said "
                        "changed it.")
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, 1))
    assert run(daemon, lambda: daemon.daemon.memory.get(rsna.id)).valid_to is not None


def test_keep_leaves_the_item_and_is_not_asked_again(daemon: Any) -> None:
    """The owner's Keep carries (rule 9): a later distillation's retire of the same item without their words asks
    nothing; their own words still retire it."""
    chain(daemon)
    rsna = owner_fact(daemon, "rsna", "I'm in the RSNA knee competition")
    later(daemon)
    handle = c_handle(daemon, said(daemon, "kaggle results came out today"))
    guess = distill_item(handle, "The RSNA competition is over", kind="fact", key="rsna", retire=True)
    distill_day(daemon, guess)
    [q] = pending(daemon, "memory.retire")
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, 2))
    assert run(daemon, lambda: daemon.daemon.memory.get(rsna.id)).valid_to is None
    run(daemon, lambda: daemon.daemon.db.conn.execute("DELETE FROM distill_runs"))
    distill_day(daemon, guess)
    assert pending(daemon, "memory.retire") == []
    assert run(daemon, lambda: daemon.daemon.memory.get(rsna.id)).valid_to is None


def test_a_waiting_retire_question_closes_when_the_item_is_superseded(daemon: Any) -> None:
    """The item stopped holding another way (the owner's later words replaced it): FERB's question about it goes, and a
    restart doesn't ask it again."""
    chain(daemon)
    rsna = owner_fact(daemon, "rsna", "I'm in the RSNA knee competition")
    later(daemon)
    handle = c_handle(daemon, said(daemon, "kaggle results came out today"))
    distill_day(daemon, distill_item(handle, "The RSNA competition is over", kind="fact", key="rsna", retire=True))
    [q] = pending(daemon, "memory.retire")
    owner_fact(daemon, "rsna", "I'm in the RSNA 2027 knee competition")
    assert pending(daemon, "memory.retire") == []
    assert run(daemon, lambda: daemon.daemon.db.conn.execute(
        "SELECT COUNT(*) FROM memory_questions WHERE kind = 'retire'").fetchone()[0]) == 0
    assert run(daemon, lambda: daemon.daemon.state.approvals.get(q.id)).state == "superseded"


def test_a_retire_question_restored_after_a_restart_is_dropped_when_its_item_is_over(daemon: Any) -> None:
    chain(daemon)
    rsna = owner_fact(daemon, "rsna", "I'm in the RSNA knee competition")
    later(daemon)
    handle = c_handle(daemon, said(daemon, "kaggle results came out today"))
    distill_day(daemon, distill_item(handle, "The RSNA competition is over", kind="fact", key="rsna", retire=True))
    [q] = pending(daemon, "memory.retire")
    d = daemon.daemon

    def restart_after_retire() -> None:
        d.db.conn.execute("UPDATE memory_items SET valid_to = ? WHERE id = ?", (d.clock.now().isoformat(), rsna.id))
        d.state.approvals._items.pop(q.id)
        d._owner_followups.clear()
        d._restore_memory_questions()
    run(daemon, restart_after_retire)
    assert pending(daemon, "memory.retire") == []
    assert run(daemon, lambda: d.db.conn.execute("SELECT COUNT(*) FROM memory_questions").fetchone()[0]) == 0


def test_ferb_memory_list_shows_a_dated_items_last_day() -> None:
    """Review fix round 1: an item that ends at the next midnight after its last day is listed "until" that day."""
    from ferb.cli.admin import _item_line
    item = {"id": "m1", "kind": "fact", "subject": "owner", "key": "city", "content": "I'm in Pune",
            "origin": "owner_stated", "status": "active", "evidence_days": 0}
    assert "(you said, active (until 2026-10-25))" in _item_line(item | {"valid_to": "2026-10-26T00:00:00+05:30"})
    assert "(until 2026-10-24)" in _item_line(item | {"valid_to": "2026-10-24T15:00:00+05:30"})   # replaced that day
