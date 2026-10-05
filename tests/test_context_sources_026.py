"""Complete durable source records and explicit ambiguity at harness boundaries."""

import asyncio
from datetime import timedelta

import pytest

from ferb.clock.clock import FakeClock
from ferb.clock.timers import Timers
from ferb.clock.tools import TimeRemindAt
from ferb.clock.when import When, WhenError, resolve
from ferb.core.ids import SequentialIds
from ferb.core.types import Identity
from ferb.daemon.questions import Approvals
from ferb.decide.decider import JevUnavailable, ShadowDecider
from ferb.memory import summary
from ferb.memory.items import slug
from ferb.policy.rules import ActionRequest
from ferb.store.db import Database


def test_reminder_preserves_complete_message_beyond_one_year(tmp_path):
    db = Database(tmp_path / "timers.db")
    db.migrate()
    clock = FakeClock()
    text = "Detailed requested reminder. " * 200 + "Do not lose this last step."
    args = TimeRemindAt.Input(in_minutes=60 * 24 * 800, message=text)
    due = resolve(When(in_minutes=args.in_minutes), clock.now())
    timer = Timers(db, clock, SequentialIds()).add("reminder", due, args.message)
    assert timer.message == text
    db.close()


def test_calendar_day_without_time_is_missing_input_not_implicit_nine_am():
    with pytest.raises(WhenError, match="time"):
        resolve(When(day="tomorrow"), FakeClock().now())


def test_unknown_timezone_and_conflicting_calendar_fields_are_facts(monkeypatch):
    from ferb.clock.when import local_zone
    monkeypatch.setenv("TZ", "not-a-real-time-zone")
    with pytest.raises(WhenError, match="time zone"):
        local_zone()
    from zoneinfo import ZoneInfo
    with pytest.raises(WhenError, match="one"):
        resolve(When(day="tomorrow", date="2030-01-01", time="12:00"), FakeClock().now(), ZoneInfo("UTC"))


def test_repeating_interval_is_explicit_positive_not_arbitrary_fifteen_minutes():
    from ferb.orchestrator.schedules import Rule
    now = FakeClock().now()
    rule = Rule.build("every", now, every_minutes=1)
    assert rule.next_after(now, now) == now + timedelta(minutes=1)


def test_long_memory_keys_do_not_collapse_to_same_fact():
    prefix = "project_requirement_" * 5
    assert slug(prefix + "alpha") != slug(prefix + "beta")


def test_duplicate_question_label_does_not_choose_first_candidate():
    q = Approvals(FakeClock())
    action = ActionRequest("owner.ask", "owner_question", Identity.OWNER, frozenset(), "t", "w", "r", frozenset())
    pending = q.question(action, "Which candidate?", ["Report.pdf", "report.pdf"])
    q.resolve(pending.id, True, text="report.pdf")
    assert pending.choice is None
    assert pending.answer == "report.pdf"


def test_day_summary_keeps_full_owner_message_and_all_source_handles(daemon):
    text = "Detailed source material. " * 100 + "The distinguishing final requirement."

    async def scenario():
        daemon.daemon.journal.append("chat.message", {"text": text})
        day = daemon.daemon.clock.now().astimezone().date().isoformat()
        ev = summary.build(daemon.daemon.db, day)
        ev.add("E", "all sources", [("trail", str(n)) for n in range(75)])
        return ev

    evidence = daemon.run(scenario())
    assert text in "\n".join(evidence.lines)
    assert len(evidence.handles["E1"]) == 75


def test_distillation_persists_full_narrative_and_all_proposals(daemon):
    narrative = "Narrative detail. " * 300 + "Final detail."

    async def scenario():
        d = daemon.daemon
        d.journal.append("chat.message", {"text": "A project has many detailed requirements."})
        day = d.clock.now().astimezone().date().isoformat()
        d.distiller.jev = None

        async def model(day, evidence):
            return {"narrative": narrative, "items": [
                {"kind": "fact", "subject": "project:test", "key": f"requirement_{n}", "content": f"Requirement {n}",
                 "origin": "inferred", "handles": ["C1"]} for n in range(45)]}

        d.distiller._model = model
        result = await d.distiller.run_day(day)
        entry = next(e for e in d.journal.entries(d.clock.now() - timedelta(days=1))
                     if e.kind == summary.NARRATIVE_KIND)
        return result, entry

    result, entry = daemon.run(scenario())
    assert entry.data["text"] == narrative
    assert result["proposed"] == 45


def test_jev_oversized_state_is_explicit_unavailability_never_partial_json():
    decider = ShadowDecider(None, FakeClock(), SequentialIds(), None, lambda value: value)
    with pytest.raises(JevUnavailable, match="state"):
        decider._state({"complete_evidence": "x" * 13000})
