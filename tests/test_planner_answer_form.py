"""The planner's answer form (ORCH-630): its side entries in one `side_entries` object, null on a turn with none, and a
condition's check null when no code check fits. The strict check takes either; code reads the group back as the fields
`SupervisorOutcome` always had, and an answer written before the group still reads (STORE-403)."""
import json
from typing import Any

import pytest

from ferb.core.structured import check_final
from ferb.orchestrator.reply_stream import ReplyDraft
from ferb.orchestrator.schemas import SIDE_ENTRIES, SUPERVISOR_SCHEMA, SupervisorOutcome, grouped
from ferb.runtimes.schema_shape import for_anthropic
from ferb.sanitize.redact import Redactor

WHEN = {"in_minutes": 20, "in_days": None, "day": None, "date": None, "time": None}
TIMER = {"when": WHEN, "message": "Tea", "quote": "timer for 20 minutes", "when_active": False, "name": "Tea timer",
         "kind": "timer"}
EMPTY = {"answer_question": None, "remember": [], "forget": [], "timers": [], "schedules": [], "schedule_changes": [],
         "show_schedules": False, "reminder_cancels": [], "conversation_changes": []}
CHECK = {"kind": "file_exists", "path": "~/Desktop/a.txt", "text": None, "url_contains": None, "site": None,
         "expected": None, "tool": None}


def task(check: dict[str, Any] | None) -> dict[str, Any]:
    return {"goal": "Write a.txt", "name": "a.txt", "identity": "owner", "worker_type": "default",
            "targets": {"apps": [], "sites": [], "paths": [], "recipients": []},
            "success_conditions": [{"text": "a.txt exists", "owner_text": "a.txt exists", "layer": "artifact",
                                    "check": check}],
            "plan": None, "folders": [], "keep_tabs_open": False, "parts": [], "earlier_results": []}


def answer(kind: str = "reply", side: dict[str, Any] | None = None, the_task: dict[str, Any] | None = None) -> dict:
    return {"kind": kind, "answers_question": False, "task": the_task, "control": None, "side_entries": side,
            "text": "Here is what I found."}


# decision: ORCH-630
def test_the_strict_check_takes_a_null_group_or_a_whole_one_and_nothing_between() -> None:
    for good in (answer(), answer(side=EMPTY | {"timers": [TIMER]}), answer("task", None, task(None)),
                 answer("task", None, task(CHECK))):
        check_final(json.dumps(good), SUPERVISOR_SCHEMA)
    flat = {**answer(), **EMPTY}                     # the fields outside the group: the form before it
    partial = answer(side={"timers": [TIMER]})       # a group must hold every entry
    loose = answer("task", None, task({"kind": "file_exists", "path": "~/a"}))
    for bad in (flat, partial, loose):
        with pytest.raises(ValueError):
            check_final(json.dumps(bad), SUPERVISOR_SCHEMA)


# decision: ORCH-630
def test_the_group_reads_back_as_the_outcomes_own_fields() -> None:
    out = SupervisorOutcome.model_validate(answer(side=EMPTY | {"timers": [TIMER], "show_schedules": True}))
    assert [t.message for t in out.timers] == ["Tea"] and out.show_schedules and out.remember == []
    # ORCH-582 reads the grouped entries as before: kind task with no task, a timer in the group, is a reply
    assert SupervisorOutcome.model_validate(answer("task", EMPTY | {"timers": [TIMER]})).kind == "reply"
    spec = SupervisorOutcome.model_validate(answer("task", None, task(None))).task
    assert spec is not None and spec.success_conditions[0].check.kind == "none"
    spec = SupervisorOutcome.model_validate(answer("task", None, task(CHECK))).task
    assert spec is not None and spec.success_conditions[0].check.path == "~/Desktop/a.txt"


# decision: ORCH-630
def test_an_answer_written_before_the_group_reads_the_same() -> None:
    old_check = CHECK | {"kind": "none", "path": None}
    older = {k: v for k, v in answer("task", None, task(old_check)).items() if k != "side_entries"} | EMPTY | {
        "timers": [TIMER]}
    newer = grouped(older)
    assert newer["side_entries"]["timers"] == [TIMER] and newer["task"]["success_conditions"][0]["check"] is None
    check_final(json.dumps(newer), SUPERVISOR_SCHEMA)
    assert SupervisorOutcome.model_validate(older) == SupervisorOutcome.model_validate(newer)
    assert set(SIDE_ENTRIES) == set(EMPTY)


# decision: ORCH-630
def test_anthropics_list_form_of_the_group_maps_back() -> None:
    shape = for_anthropic(SUPERVISOR_SCHEMA)
    for side, timers in (([], []), ([EMPTY | {"timers": [TIMER]}], ["Tea"])):
        wire = answer() | {"side_entries": side, "task": [], "control": []}
        out = SupervisorOutcome.model_validate_json(shape.answer_text(json.dumps(wire)))
        assert [t.message for t in out.timers] == timers


# decision: ORCH-630
@pytest.mark.parametrize("side, streams", [(None, True), (EMPTY, True), (EMPTY | {"timers": [TIMER]}, False),
                                           (EMPTY | {"show_schedules": True}, False)])
def test_a_reply_streams_only_with_no_side_entry(side: dict[str, Any] | None, streams: bool) -> None:
    seen: list[dict[str, Any]] = []
    draft = ReplyDraft(Redactor(), seen.append)
    raw = json.dumps(answer(side=side))
    for i in range(0, len(raw), 5):
        draft.feed(raw[i:i + 5])
    assert any(e["type"] == "reply_delta" for e in seen) is streams
