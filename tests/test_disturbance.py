"""Task-wide desktop observations; correlation must never become guessed causation."""

import asyncio

import pytest

from ferb.daemon.disturbance import DisturbanceSentinel


@pytest.fixture
def anyio_backend():
    return "asyncio"


@pytest.mark.anyio
async def test_samples_task_and_links_changes_to_action_without_claiming_causation():
    state = {"frontmost_app": {"pid": 1}, "cursor": [10, 20], "active_space_epoch": 0,
             "key_window": "window-a", "fullscreen": False, "visible_window_order": [1, 2]}
    records = []
    sentinel = DisturbanceSentinel(lambda: dict(state), lambda task, kind, data: records.append((task, kind, data)),
                                    interval_s=60, owner_input=lambda: {"recent": True})
    await sentinel.start("task")
    await sentinel.before_action("task.worker", "action-1", "shell.exec", {"command": "example"})
    state["frontmost_app"] = {"pid": 2}
    state["active_space_epoch"] = 1
    await sentinel.after_action("action-1")
    await sentinel.finish("task")
    changes = [r[2] for r in records if r[1] == "disturbance.change"]
    assert len(changes) == 1
    assert set(changes[0]["changes"]) == {"frontmost_app", "active_space_epoch"}
    assert changes[0]["actions"][0]["span_id"] == "action-1"
    assert changes[0]["attribution"] == "temporal_correlation_only"
    assert changes[0]["owner_input"] == {"recent": True}
    assert changes[0]["permission_assessment"] == "not_inferred"
    assert records[-1][1] == "disturbance.summary"


@pytest.mark.anyio
async def test_periodic_samples_survive_errors_and_cancellation_cleanup():
    records = []
    calls = 0

    def read():
        nonlocal calls
        calls += 1
        if calls == 2:
            raise RuntimeError("probe unavailable")
        return {"cursor": [calls, 0], "unavailable": {"key_window": "AX not allowed"}}

    sentinel = DisturbanceSentinel(read, lambda *r: records.append(r), interval_s=0.01)
    await sentinel.start("task")
    await asyncio.sleep(0.06)
    await sentinel.finish("task")
    assert any(kind == "disturbance.error" for _, kind, _ in records)
    assert any(kind == "disturbance.change" for _, kind, _ in records)
    assert records[-1][2]["errors"] >= 1
    assert not sentinel.active


@pytest.mark.anyio
async def test_overlapping_tasks_do_not_invent_unique_action_attribution():
    state = {"cursor": [0, 0]}
    records = []
    sentinel = DisturbanceSentinel(lambda: dict(state), lambda *r: records.append(r), interval_s=60)
    await sentinel.start("a")
    await sentinel.start("b")
    await sentinel.before_action("a.worker", "one", "mac.click", {"allow_foreground": True})
    await sentinel.before_action("b.worker", "two", "mac.click", {})
    state["cursor"] = [2, 3]
    await sentinel.after_action("one")
    await sentinel.after_action("two")
    await sentinel.finish("a")
    await sentinel.finish("b")
    changes = [data for _, kind, data in records if kind == "disturbance.change"]
    assert changes
    assert all({a["span_id"] for a in c["actions"]} == {"one", "two"} for c in changes)
    assert all(c["permission_assessment"] == "not_inferred" for c in changes)


@pytest.mark.anyio
async def test_permission_nonjudgment_is_readable_after_normal_redaction():
    from ferb.sanitize.redact import Redactor
    records = []
    redact = Redactor()
    sentinel = DisturbanceSentinel(lambda: {'cursor': [0, 0]},
                                   lambda task, kind, data: records.append((kind, redact(data))), interval_s=60)
    await sentinel.start('task')
    await sentinel.finish('task')
    assert records[-1][1]['permission_assessment'] == 'not_inferred'
