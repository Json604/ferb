"""Task-wide desktop observations; correlation must never become guessed causation."""

import asyncio
import json
from datetime import datetime, timedelta

import pytest

from ferb.daemon.disturbance import DisturbanceSentinel


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _facts(context: str) -> dict:
    return json.loads(context[context.index("{"):])


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


def _app(pid: int, name: str) -> dict:
    return {"pid": pid, "bundle_id": f"com.example.{pid}", "name": name}


def _changes(records: list) -> list[dict]:
    return [d for _, kind, d in records if kind == "disturbance.change"]


@pytest.mark.anyio
async def test_a_front_app_change_with_no_ferb_on_screen_action_names_the_app_and_ferbs_processes():
    state = {"frontmost_app": {"pid": 1, "bundle_id": "com.brave.Browser", "name": "Brave Browser"}, "cursor": [0, 0]}
    records = []
    procs = [{"id": "p-1", "task": "tree.worker", "command": "codex -m gpt-6-astra", "background": False}]
    sentinel = DisturbanceSentinel(lambda: dict(state), lambda *r: records.append(r), interval_s=60,
                                   processes=lambda: procs, now=lambda: datetime(2026, 10, 9, 16, 36, 2))
    await sentinel.start("tree")
    await sentinel.before_action("tree.worker", "wait-1", "shell.wait", {"id": "p-1"}, on_screen=False)
    state["frontmost_app"] = {"pid": 9, "bundle_id": "com.google.Chrome", "name": "Google Chrome"}
    await sentinel.sample()
    context = sentinel.context("tree.worker")
    await sentinel.after_action("wait-1")
    await sentinel.finish("tree")
    [change] = _changes(records)
    assert change["processes"] == procs and change["actions"][0]["on_screen"] is False
    assert change["on_screen_in_task"] is False and change["last_on_screen_ended_s"] is None
    assert ("16:36:02 Google Chrome came to the front — no FERB on-screen action in progress (none yet); "
            "running: codex -m gpt-6-astra (started by this task)") in context
    facts = _facts(context)
    assert facts["front_or_space_changes_with_no_ferb_on_screen_action"] == 1 and len(facts["latest"]) == 1


@pytest.mark.anyio
async def test_a_change_during_or_just_after_an_on_screen_ferb_action_is_near_it():
    state = {"frontmost_app": {"pid": 1, "bundle_id": "a.app", "name": "A"}}
    records = []
    sentinel = DisturbanceSentinel(lambda: dict(state), lambda *r: records.append(r), interval_s=60,
                                   processes=lambda: [])
    await sentinel.start("tree")
    await sentinel.before_action("tree.worker", "click", "mac.act", {}, on_screen=True)
    state["frontmost_app"] = {"pid": 2, "bundle_id": "b.app", "name": "B"}
    await sentinel.after_action("click")                 # sampled while the click was in flight
    state["frontmost_app"] = {"pid": 3, "bundle_id": "c.app", "name": "C"}
    await sentinel.sample()                              # just after it ended: still within two sample intervals
    context = sentinel.context("tree.worker")
    await sentinel.finish("tree")
    changes = _changes(records)
    assert len(changes) == 2 and all(any(a["on_screen"] for a in c["actions"]) for c in changes)
    assert "no FERB on-screen action in progress" not in context
    assert _facts(context)["front_or_space_changes_with_no_ferb_on_screen_action"] == 0


@pytest.mark.anyio
async def test_a_change_after_the_window_says_how_long_before_the_last_on_screen_action_ended():
    """Controller ruling (2026-10-09): a FERB effect can land after the 0.5 s window (an app finishing its launch), so
    the statement gives the time since the latest on-screen FERB call of any task ended: tenths under 10 s, whole
    seconds from 10 s."""
    state = {"frontmost_app": {"pid": 1, "bundle_id": "a.app", "name": "A"}}
    clock = {"now": datetime(2026, 10, 9, 16, 35, 55)}
    records = []
    sentinel = DisturbanceSentinel(lambda: dict(state), lambda *r: records.append(r), interval_s=0.01,
                                   processes=lambda: [], now=lambda: clock["now"])
    await sentinel.start("tree")
    await sentinel.before_action("tree.worker", "open", "mac.open", {}, on_screen=True)
    await sentinel.after_action("open")                  # ended at 16:35:55
    await asyncio.sleep(0.1)                             # well past two sample intervals
    clock["now"] += timedelta(seconds=7, milliseconds=680)
    state["frontmost_app"] = {"pid": 2, "bundle_id": "com.google.Chrome", "name": "Google Chrome"}
    await sentinel.sample()
    clock["now"] += timedelta(seconds=5)
    state["frontmost_app"] = {"pid": 3, "bundle_id": "com.apple.Safari", "name": "Safari"}
    await sentinel.sample()
    context = sentinel.context("tree.worker")
    await sentinel.finish("tree")
    assert [(c["on_screen_in_task"], c["last_on_screen_ended_s"]) for c in _changes(records)] == [(True, 7.6),
                                                                                                  (True, 12)]
    assert ("16:36:02 Google Chrome came to the front — no FERB on-screen action in progress (the last ended 7.6 s "
            "earlier); running: nothing FERB started") in context
    assert "16:36:07 Safari came to the front — no FERB on-screen action in progress (the last ended 12 s " in context


@pytest.mark.anyio
async def test_another_tasks_on_screen_call_is_named_as_such_and_its_commands_stay_out():
    """MP2: another task tree's command text was never this tree's to read; its processes show by id only."""
    state = {"frontmost_app": _app(1, "A")}
    clock = {"now": datetime(2026, 10, 9, 9, 0, 0)}
    records = []
    procs = [{"id": "p-1", "task": "tree.worker", "command": "npm run dev", "background": True},
             {"id": "p-2", "task": "other.worker", "command": "curl https://example.com/secret-page", "background": False}]
    sentinel = DisturbanceSentinel(lambda: dict(state), lambda *r: records.append(r), interval_s=0.01,
                                   processes=lambda: procs, now=lambda: clock["now"])
    await sentinel.start("tree")
    await sentinel.start("other")
    await sentinel.before_action("other.worker", "click", "mac.act", {}, on_screen=True)
    await sentinel.after_action("click")
    await asyncio.sleep(0.1)
    clock["now"] += timedelta(milliseconds=700)
    state["frontmost_app"] = _app(2, "Google Chrome")
    await sentinel.sample()
    context = sentinel.context("tree.worker")
    await sentinel.finish("tree")
    await sentinel.finish("other")
    mine = [d for task, kind, d in records if kind == "disturbance.change" and task == "tree"]
    assert [p.get("command") for p in mine[0]["processes"]] == ["npm run dev", None]
    assert ("09:00:00 Google Chrome came to the front — no FERB on-screen action in progress (none in this task; the "
            "last of any FERB task ended 0.7 s earlier); running: npm run dev (started by this task); p-2 (started by "
            "another FERB task)") in context
    assert "curl" not in context and "secret-page" not in json.dumps(mine)


@pytest.mark.anyio
async def test_an_unreadable_front_app_states_nothing_and_the_same_app_after_it_states_nothing():
    state: dict = {"frontmost_app": _app(1, "Brave Browser")}
    records = []
    sentinel = DisturbanceSentinel(lambda: dict(state), lambda *r: records.append(r), interval_s=60,
                                   processes=lambda: [], now=lambda: datetime(2026, 10, 9, 9, 0, 0))
    await sentinel.start("tree")
    del state["frontmost_app"]                           # AX couldn't read it
    state["unavailable"] = {"frontmost_app": "AX error -25204"}
    await sentinel.sample()
    state.pop("unavailable")
    state["frontmost_app"] = _app(1, "Brave Browser")    # the same app, readable again
    await sentinel.sample()
    quiet = _facts(sentinel.context("tree"))
    state["frontmost_app"] = _app(2, "Google Chrome")    # a real switch
    await sentinel.sample()
    facts = _facts(sentinel.context("tree"))
    await sentinel.finish("tree")
    changes = _changes(records)
    assert len(changes) == 3 and changes[0].get("front_app_unreadable") is True
    assert "processes" not in changes[0] and "processes" not in changes[1]
    assert quiet["front_or_space_changes_with_no_ferb_on_screen_action"] == 0 and quiet["latest"] == []
    assert facts["latest"] == ["09:00:00 Google Chrome came to the front — no FERB on-screen action in progress "
                               "(none yet); running: nothing FERB started"]


@pytest.mark.anyio
async def test_processes_that_cannot_be_listed_are_recorded_as_not_recorded():
    state = {"frontmost_app": {"pid": 1, "bundle_id": "a.app", "name": "A"}}
    records = []

    def broken() -> list:
        raise RuntimeError("shell manager gone")

    sentinel = DisturbanceSentinel(lambda: dict(state), lambda *r: records.append(r), interval_s=60,
                                   processes=broken, now=lambda: datetime(2026, 10, 9, 9, 0, 0))
    await sentinel.start("tree")
    state["frontmost_app"] = {"pid": 2, "bundle_id": "b.app", "name": "B"}
    await sentinel.sample()
    context = sentinel.context("tree")
    await sentinel.finish("tree")
    [change] = _changes(records)
    assert "processes" in change and change["processes"] is None
    assert "09:00:00 B came to the front — no FERB on-screen action in progress (none yet); running: not recorded" \
        in context


@pytest.mark.anyio
async def test_only_the_newest_statements_reach_the_agents_context():
    state = {"frontmost_app": {"pid": 100, "bundle_id": "app0", "name": "App 0"}}
    records = []
    sentinel = DisturbanceSentinel(lambda: dict(state), lambda *r: records.append(r), interval_s=60,
                                   processes=lambda: [], now=lambda: datetime(2026, 10, 9, 9, 0, 0))
    await sentinel.start("tree")
    for i in range(1, 13):
        state["frontmost_app"] = {"pid": i, "bundle_id": f"app{i}", "name": f"App {i}"}
        await sentinel.sample()
    facts = _facts(sentinel.context("tree"))
    await sentinel.finish("tree")
    assert facts["front_or_space_changes_with_no_ferb_on_screen_action"] == 12 and len(facts["latest"]) == 10
    assert facts["latest"][0].startswith("09:00:00 App 3 came to the front")
    assert len(_changes(records)) == 12


@pytest.mark.anyio
async def test_a_task_run_while_paused_records_nothing_about_the_desktop_and_resuming_records_again():
    """`ferb pause` promises nothing is recorded until `ferb resume`: no reads, samples, app names, processes or
    statements, and nothing in the agents' context. FERB's own calls still pass through (they are its actions)."""
    state = {"frontmost_app": _app(1, "A"), "cursor": [0, 0]}
    reads = []
    listed = []
    records = []
    paused = {"on": True}

    def read() -> dict:
        reads.append(1)
        return dict(state)

    def processes() -> list:
        listed.append(1)
        return []

    sentinel = DisturbanceSentinel(read, lambda *r: records.append(r), interval_s=60, processes=processes,
                                   now=lambda: datetime(2026, 10, 9, 9, 0, 0), paused=lambda: paused["on"])
    await sentinel.start("tree")
    await sentinel.before_action("tree.worker", "wait", "shell.wait", {}, on_screen=False)
    state["frontmost_app"] = _app(2, "Google Chrome")
    await sentinel.after_action("wait")
    await sentinel.sample()
    assert sentinel.context("tree") == ""
    assert reads == [] and listed == [] and [kind for _, kind, _ in records] == ["disturbance.paused"]
    paused["on"] = False                                  # ferb resume, mid-task
    await sentinel.sample()                               # a fresh baseline: what changed meanwhile wasn't observed
    state["frontmost_app"] = _app(3, "Safari")
    await sentinel.sample()
    context = sentinel.context("tree")
    await sentinel.finish("tree")
    assert [kind for _, kind, _ in records] == ["disturbance.paused", "disturbance.resumed", "disturbance.baseline",
                                                "disturbance.change", "disturbance.summary"]
    assert "Safari came to the front" in context and "Google Chrome" not in context
