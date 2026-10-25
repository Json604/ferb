"""L36 §6: out-of-date facts retire. A dated fact leaves search after its day, a later statement supersedes an earlier
one, and distillation retires an item only on the owner's own words (otherwise it asks).

Live 2026-10-24: the 10-11 wedding flight, "DevOps left" and the finished RSNA competition still sat in every planner
prompt; memory item 54 still said "Chennai, notify first" after the owner said "book anywhere"."""
import threading
from datetime import datetime, timezone
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


def test_a_past_month_and_day_is_over_not_next_year() -> None:
    """Scan m13: `_day` turns a past "MM-DD" into next year's (right for a reminder); an item's last day the owner gave
    without a year that is already behind is over, never a year away."""
    zone = ZoneInfo("Asia/Kolkata")
    with pytest.raises(WhenError, match="already over"):
        end_of_day(When(date="10-23"), datetime(2026, 10, 24, 10, 0, tzinfo=zone), zone)


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
        script.sup.append(control_turn("Book the visa appointment at any center", tree_id, "steer", remember=[{
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
            'true ("the RSNA competition is over"): give that item\'s kind, subject and key, content saying what '
            'changed, and the quote of the owner\'s words; FERB retires it only when the quote is theirs, and '
            'otherwise asks the owner. Every other item has "retire": false.') in words


def test_distill_retires_an_item_on_the_owners_own_words(daemon: Any) -> None:
    chain(daemon)
    rsna = owner_fact(daemon, "rsna", "I'm in the RSNA knee competition")
    handle = c_handle(daemon, said(daemon, "the RSNA competition is over"))
    distill_day(daemon, distill_item(handle, "the RSNA competition is over", "the RSNA competition is over",
                                     kind="fact", key="rsna", retire=True))
    assert run(daemon, lambda: daemon.daemon.memory.get(rsna.id)).valid_to is not None
    assert pending(daemon, "memory.retire") == []
    assert [i for i in run(daemon, lambda: daemon.daemon.memory.items("active")) if i.key == "rsna"] == []


def test_a_retire_without_the_owners_words_is_asked(daemon: Any) -> None:
    chain(daemon)
    rsna = owner_fact(daemon, "rsna", "I'm in the RSNA knee competition")
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


def test_ferb_chat_shows_the_retire_question_as_ferbs_check() -> None:
    """`ferb chat`: Retire or Keep is FERB's own check (answered only by an option), and skipping it keeps the item."""
    from ferb.cli.chat import question
    from ferb.cli.live_chat import LiveChat
    ask = {"approval_id": "ap-retire", "kind": "owner_question", "capability": "memory.retire",
           "reason": "Is this no longer true? “x”", "options": ["Retire", "Keep"]}
    said: list[str] = []
    chat_ = LiveChat.__new__(LiveChat)
    chat_.pending, chat_._lock = {}, threading.RLock()
    chat_.say = said.append                                            # type: ignore[method-assign]
    chat_.answer_approval(ask)
    assert said[-1].strip() == "Reply Retire or Keep."
    assert question(ask).endswith("1) Retire\n     2) Keep\n   number or your answer (Enter = Keep): ")
