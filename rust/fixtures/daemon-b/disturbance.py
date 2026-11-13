"""Fixtures for the Rust port of ferb/daemon/disturbance.py (ENG-3): the DisturbanceSentinel driven by scripted steps,
with every seam faked (the desktop read, the owner-input facts, FERB's processes, the clock, the monotonic time, the
pause), and the pure helpers `screen_changes` and `told`.

    cd <repo> && uv run python rust/fixtures/daemon-b/disturbance.py

Writes disturbance.json beside this file: `pure` (inputs and outputs of `screen_changes`, `told`, `_elapsed`) and
`scenarios`, each a list of `steps` and what the sentinel did: `records` (every `record(task, kind, data)` call, in
order), `probe` (the desktop probe's start/pump/close calls and reads), and per step its result (`after_action`,
`sample`, `context`, `facts`). The cases are tests/test_disturbance.py's, tests/test_tab_sound.py's and the HAND-213 /
SENSE-406 ones of tests/test_mac_focus.py and tests/test_owner_input_tap.py, plus edges found reading the code. The
monotonic time moves 0.25 s at every reading (and by a `mono` step), so the Rust port must read it in the same order.
The wall clock (`now`) is UTC and moves only by a `now` step. Invented data only; the same bytes on every run.
"""

import asyncio
import copy
import json
import os
import sys
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from types import SimpleNamespace
from typing import Any

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
os.environ["TZ"] = "UTC"
time.tzset()
sys.path.insert(0, str(REPO))

import ferb.daemon.disturbance as dist  # noqa: E402

START = datetime(2026, 10, 9, 16, 35, 55, tzinfo=timezone.utc)


class Fakes:
    def __init__(self, sc: dict[str, Any]) -> None:
        self.state: dict[str, Any] = copy.deepcopy(sc.get("state", {}))
        self.fail: list[str] = []                      # the next reads raise this
        self.owner: Any = sc.get("owner_input", {})
        self.procs: Any = sc.get("processes", [])
        self.now = START
        self.mono = 1000.0
        self.paused = bool(sc.get("paused", False))
        self.arm_pause = False
        self.probe: list[Any] = []
        self.records: list[Any] = []

    def monotonic(self) -> float:
        self.mono += 0.25
        return self.mono


def _raise(spec: str) -> None:
    kind, _, text = spec.partition(": ")
    raise {"RuntimeError": RuntimeError, "ValueError": ValueError, "KeyError": KeyError}[kind](text)


class Probe:
    def __init__(self, f: Fakes, hooks: bool) -> None:
        self.f = f
        if hooks:
            self.start = lambda: f.probe.append("start")
            self.close = lambda: f.probe.append("close")
            self.pump = lambda: f.probe.append("pump")

    def __call__(self) -> dict[str, Any]:
        f = self.f
        f.probe.append("read")
        if f.arm_pause:
            f.paused = True
        if f.fail:
            _raise(f.fail.pop(0))
        return copy.deepcopy(f.state)


def run(sc: dict[str, Any]) -> dict[str, Any]:
    f = Fakes(sc)
    dist.time = SimpleNamespace(monotonic=f.monotonic)      # type: ignore[attr-defined]

    def owner() -> dict[str, Any]:
        if isinstance(f.owner, str):
            _raise(f.owner)
        return copy.deepcopy(f.owner)  # type: ignore[no-any-return]

    def procs() -> list[dict[str, Any]]:
        if isinstance(f.procs, str):
            _raise(f.procs)
        return copy.deepcopy(f.procs)  # type: ignore[no-any-return]

    def record(task: str, kind: str, data: dict[str, Any]) -> None:
        f.records.append([task, kind, json.loads(json.dumps(data))])

    sentinel = dist.DisturbanceSentinel(Probe(f, sc.get("probe_hooks", True)), record, owner_input=owner,
                                        processes=procs, now=lambda: f.now, paused=lambda: f.paused)
    results: list[Any] = []

    async def go() -> None:
        for st in sc["steps"]:
            do = st["do"]
            out: Any = None
            if do == "set":
                f.state = copy.deepcopy(st["state"])
            elif do == "patch":
                f.state.update(copy.deepcopy(st["state"]))
                for k in st.get("drop", []):
                    f.state.pop(k, None)
            elif do == "read_fails":
                f.fail.append(st["error"])
            elif do == "owner_input":
                f.owner = st["value"]
            elif do == "processes":
                f.procs = st["value"]
            elif do == "now":
                f.now += timedelta(seconds=st["advance_s"])
            elif do == "mono":
                f.mono += st["advance_s"]
            elif do == "paused":
                f.paused = st["on"]
            elif do == "arm_pause":
                f.arm_pause = st["on"]
            elif do == "start":
                await sentinel.start(st["task"])
            elif do == "finish":
                await sentinel.finish(st["task"])
            elif do == "shutdown":
                await sentinel.shutdown()
            elif do == "finish_and_start":
                await asyncio.gather(sentinel.finish(st["finish"]), sentinel.start(st["start"]))
            elif do == "finish_and_shutdown":
                await asyncio.gather(sentinel.finish(st["task"]), sentinel.shutdown())
            elif do == "before":
                await sentinel.before_action(st["task"], st["span"], st["tool"], st.get("args", {}),
                                             on_screen=st.get("on_screen", False))
            elif do == "after":
                out = await sentinel.after_action(st["span"])
            elif do == "call_started":
                sentinel.call_started(st["task"], st["span"], st["tool"])
            elif do == "call_ended":
                sentinel.call_ended(st["span"])
            elif do == "sound":
                sentinel.sound_changed(st["tab"], st["change"])
            elif do == "sample":
                out = await sentinel.sample()
            elif do == "context":
                out = sentinel.context(st["task"])
            elif do == "facts":
                out = sentinel.facts(st["task"])
            elif do == "active":
                out = list(sentinel.active)
            else:
                raise ValueError(do)
            results.append(json.loads(json.dumps(out)))
    asyncio.run(go())
    return {"name": sc["name"], "what": sc["what"], "state": sc.get("state", {}),
            "owner_input": sc.get("owner_input", {}), "processes": sc.get("processes", []),
            "paused": sc.get("paused", False), "probe_hooks": sc.get("probe_hooks", True),
            "steps": sc["steps"], "results": results, "records": f.records, "probe": f.probe}


def app(pid: int, name: str) -> dict[str, Any]:
    return {"pid": pid, "bundle_id": f"com.example.{pid}", "name": name}


SCENARIOS: list[dict[str, Any]] = [
    {"name": "links_changes_to_action", "what": "test_samples_task_and_links_changes_to_action_without_claiming_causation",
     "state": {"frontmost_app": {"pid": 1}, "cursor": [10, 20], "active_space_epoch": 0, "key_window": "window-a",
               "fullscreen": False, "visible_window_order": [1, 2]},
     "owner_input": {"recent": True},
     "steps": [{"do": "start", "task": "task"},
               {"do": "before", "task": "task.worker", "span": "action-1", "tool": "mac.open",
                "args": {"app": "Notes"}, "on_screen": True},
               {"do": "patch", "state": {"frontmost_app": {"pid": 2}, "active_space_epoch": 1, "cursor": [99, 99],
                                         "visible_window_order": [2, 1]}},
               {"do": "after", "span": "action-1"},
               {"do": "facts", "task": "task.worker"},
               {"do": "context", "task": "task"},
               {"do": "finish", "task": "task"}]},
    {"name": "no_samples_around_reads", "what": "test_no_timer_and_no_samples_around_read_only_calls",
     "state": {"frontmost_app": {"pid": 1}}, "probe_hooks": False,
     "steps": [{"do": "start", "task": "task"},
               {"do": "before", "task": "task.worker", "span": "w", "tool": "shell.wait", "on_screen": False},
               {"do": "after", "span": "w"},
               {"do": "before", "task": "task.worker", "span": "c", "tool": "mac.click", "on_screen": True},
               {"do": "after", "span": "c"},
               {"do": "after", "span": "c"},
               {"do": "finish", "task": "task"},
               {"do": "sample"}]},
    {"name": "sample_error", "what": "test_a_sample_error_is_recorded_and_the_task_goes_on",
     "state": {"frontmost_app": {"pid": 1}, "unavailable": {"key_window": "AX not allowed"}},
     "steps": [{"do": "start", "task": "task"},
               {"do": "read_fails", "error": "RuntimeError: probe unavailable"},
               {"do": "sample"},
               {"do": "patch", "state": {"frontmost_app": {"pid": 3}}},
               {"do": "sample"},
               {"do": "owner_input", "value": "KeyError: owner"},
               {"do": "sample"},
               {"do": "context", "task": "task"},
               {"do": "finish", "task": "task"},
               {"do": "active"}]},
    {"name": "cursor_alone", "what": "test_a_cursor_move_alone_records_nothing",
     "state": {"frontmost_app": {"pid": 1}, "cursor": [0, 0]},
     "steps": [{"do": "start", "task": "task"}, {"do": "patch", "state": {"cursor": [300, 400]}},
               {"do": "sample"}, {"do": "finish", "task": "task"}]},
    {"name": "finish_while_start", "what": "test_a_tree_finishing_while_another_starts_keeps_the_probe_open",
     "state": {"frontmost_app": {"pid": 1}},
     "steps": [{"do": "start", "task": "a"}, {"do": "finish_and_start", "finish": "a", "start": "b"},
               {"do": "active"}, {"do": "finish", "task": "b"}, {"do": "start", "task": "b"},
               {"do": "start", "task": "b"}, {"do": "finish", "task": "b"}, {"do": "finish", "task": "b"}]},
    {"name": "overlapping_tasks", "what": "test_overlapping_tasks_do_not_invent_unique_action_attribution",
     "state": {"frontmost_app": {"pid": 1}},
     "steps": [{"do": "start", "task": "a"}, {"do": "start", "task": "b"},
               {"do": "before", "task": "a.worker", "span": "one", "tool": "mac.click",
                "args": {"allow_foreground": True}, "on_screen": True},
               {"do": "before", "task": "b.worker", "span": "two", "tool": "mac.click", "on_screen": True},
               {"do": "patch", "state": {"frontmost_app": {"pid": 2}}},
               {"do": "after", "span": "one"}, {"do": "after", "span": "two"},
               {"do": "facts", "task": "a"}, {"do": "facts", "task": "b.step.x"},
               {"do": "finish", "task": "a"}, {"do": "finish", "task": "b"}]},
    {"name": "front_app_no_on_screen_action",
     "what": "test_a_front_app_change_with_no_ferb_on_screen_action_names_the_app_and_ferbs_processes",
     "state": {"frontmost_app": {"pid": 1, "bundle_id": "com.example.browser", "name": "Example Browser"},
               "cursor": [0, 0]},
     "processes": [{"id": "p-1", "task": "tree.worker", "command": "codex -m model-x", "background": False}],
     "steps": [{"do": "now", "advance_s": 40.5},
               {"do": "start", "task": "tree"},
               {"do": "before", "task": "tree.worker", "span": "wait-1", "tool": "shell.wait", "args": {"id": "p-1"}},
               {"do": "patch", "state": {"frontmost_app": {"pid": 9, "bundle_id": "com.example.chrome",
                                                           "name": "Example Chrome"}}},
               {"do": "sample"},
               {"do": "context", "task": "tree.worker"},
               {"do": "after", "span": "wait-1"},
               {"do": "finish", "task": "tree"},
               {"do": "context", "task": "tree"}]},
    {"name": "change_during_and_after", "what": "test_a_change_during_or_just_after_an_on_screen_ferb_action_is_near_it",
     "state": {"frontmost_app": {"pid": 1, "bundle_id": "a.app", "name": "A"}},
     "steps": [{"do": "start", "task": "tree"},
               {"do": "before", "task": "tree.worker", "span": "click", "tool": "mac.act", "on_screen": True},
               {"do": "patch", "state": {"frontmost_app": {"pid": 2, "bundle_id": "b.app", "name": "B"}}},
               {"do": "after", "span": "click"},
               {"do": "patch", "state": {"frontmost_app": {"pid": 3, "bundle_id": "c.app", "name": "C"}}},
               {"do": "sample"},
               {"do": "context", "task": "tree.worker"},
               {"do": "finish", "task": "tree"}]},
    {"name": "how_long_since", "what": "test_a_change_after_the_window_says_how_long_before_the_last_on_screen_action_ended",
     "state": {"frontmost_app": {"pid": 1, "bundle_id": "a.app", "name": "A"}},
     "steps": [{"do": "start", "task": "tree"},
               {"do": "before", "task": "tree.worker", "span": "open", "tool": "mac.open", "on_screen": True},
               {"do": "after", "span": "open"},
               {"do": "now", "advance_s": 7.68},
               {"do": "patch", "state": {"frontmost_app": {"pid": 2, "bundle_id": "com.example.chrome",
                                                           "name": "Example Chrome"}}},
               {"do": "sample"},
               {"do": "now", "advance_s": 5},
               {"do": "patch", "state": {"frontmost_app": {"pid": 3, "bundle_id": "com.example.safari",
                                                           "name": "Example Safari"}}},
               {"do": "sample"},
               {"do": "now", "advance_s": 0.04},
               {"do": "patch", "state": {"active_space_epoch": 4}},
               {"do": "sample"},
               {"do": "patch", "state": {"active_space_epoch": 5,
                                         "frontmost_app": {"pid": 4, "bundle_id": "d.app", "name": ""}}},
               {"do": "sample"},
               {"do": "context", "task": "tree.worker"},
               {"do": "finish", "task": "tree"}]},
    {"name": "another_tasks_call", "what": "test_another_tasks_on_screen_call_is_named_as_such_and_its_commands_stay_out",
     "state": {"frontmost_app": app(1, "A")},
     "processes": [{"id": "p-1", "task": "tree.worker", "command": "npm run dev", "background": True},
                   {"id": "p-2", "task": "other.worker", "command": "curl https://example.com/private-page",
                    "background": False},
                   {"id": "p-3", "background": False}],
     "steps": [{"do": "start", "task": "tree"}, {"do": "start", "task": "other"},
               {"do": "before", "task": "other.worker", "span": "click", "tool": "mac.act", "on_screen": True},
               {"do": "after", "span": "click"},
               {"do": "now", "advance_s": 0.7},
               {"do": "patch", "state": {"frontmost_app": app(2, "Example Chrome")}},
               {"do": "sample"},
               {"do": "context", "task": "tree.worker"},
               {"do": "context", "task": "other"},
               {"do": "finish", "task": "tree"}, {"do": "finish", "task": "other"}]},
    {"name": "unreadable_front_app",
     "what": "test_an_unreadable_front_app_states_nothing_and_the_same_app_after_it_states_nothing",
     "state": {"frontmost_app": app(1, "Example Browser")},
     "steps": [{"do": "start", "task": "tree"},
               {"do": "set", "state": {"unavailable": {"frontmost_app": "AX error -25204"}}},
               {"do": "sample"},
               {"do": "set", "state": {"frontmost_app": app(1, "Example Browser")}},
               {"do": "sample"},
               {"do": "context", "task": "tree"},
               {"do": "patch", "state": {"frontmost_app": app(2, "Example Chrome")}},
               {"do": "sample"},
               {"do": "patch", "state": {"frontmost_app": {"pid": "x", "name": "Odd"}}},
               {"do": "sample"},
               {"do": "context", "task": "tree"},
               {"do": "finish", "task": "tree"}]},
    {"name": "processes_unlisted", "what": "test_processes_that_cannot_be_listed_are_recorded_as_not_recorded",
     "state": {"frontmost_app": {"pid": 1, "bundle_id": "a.app", "name": "A"}},
     "processes": "RuntimeError: shell manager gone",
     "steps": [{"do": "start", "task": "tree"},
               {"do": "patch", "state": {"frontmost_app": {"pid": 2, "bundle_id": "b.app", "name": "B"}}},
               {"do": "sample"}, {"do": "context", "task": "tree"}, {"do": "finish", "task": "tree"}]},
    {"name": "newest_statements", "what": "test_only_the_newest_statements_reach_the_agents_context",
     "state": {"frontmost_app": {"pid": 100, "bundle_id": "app0", "name": "App 0"}},
     "steps": [{"do": "start", "task": "tree"}]
     + [s for i in range(1, 13) for s in (
         {"do": "patch", "state": {"frontmost_app": {"pid": i, "bundle_id": f"app{i}", "name": f"App {i}"}}},
         {"do": "now", "advance_s": 1}, {"do": "sample"})]
     + [{"do": "context", "task": "tree"}, {"do": "finish", "task": "tree"}]},
    {"name": "paused_then_resumed",
     "what": "test_a_task_run_while_paused_records_nothing_about_the_desktop_and_resuming_records_again",
     "state": {"frontmost_app": app(1, "A"), "cursor": [0, 0]}, "paused": True,
     "steps": [{"do": "start", "task": "tree"},
               {"do": "before", "task": "tree.worker", "span": "wait", "tool": "shell.wait"},
               {"do": "patch", "state": {"frontmost_app": app(2, "Example Chrome")}},
               {"do": "after", "span": "wait"},
               {"do": "before", "task": "tree.worker", "span": "act", "tool": "mac.act", "on_screen": True},
               {"do": "after", "span": "act"},
               {"do": "sample"},
               {"do": "context", "task": "tree"},
               {"do": "paused", "on": False},
               {"do": "sample"},
               {"do": "patch", "state": {"frontmost_app": app(3, "Example Safari")}},
               {"do": "sample"},
               {"do": "context", "task": "tree"},
               {"do": "finish", "task": "tree"}]},
    {"name": "pause_during_read", "what": "test_a_pause_that_begins_during_a_read_drops_what_was_read",
     "state": {"frontmost_app": app(1, "A")},
     "steps": [{"do": "start", "task": "tree"},
               {"do": "arm_pause", "on": True},
               {"do": "patch", "state": {"frontmost_app": app(2, "Example Chrome")}},
               {"do": "sample"},
               {"do": "context", "task": "tree"},
               {"do": "arm_pause", "on": False}, {"do": "paused", "on": False},
               {"do": "start", "task": "second"},
               {"do": "arm_pause", "on": True}, {"do": "read_fails", "error": "RuntimeError: gone"},
               {"do": "sample"},
               {"do": "arm_pause", "on": False}, {"do": "paused", "on": False},
               {"do": "finish", "task": "tree"}, {"do": "finish", "task": "second"}]},
    {"name": "finished_twice_at_once", "what": "test_a_task_finished_twice_at_once_is_summarized_once",
     "state": {"frontmost_app": {"pid": 1}},
     "steps": [{"do": "start", "task": "tree"}, {"do": "start", "task": "other"},
               {"do": "finish_and_shutdown", "task": "tree"}, {"do": "active"}]},
    {"name": "unavailable_keys", "what": "a key unreadable in either sample is not a change of the call; others are kept out",
     "state": {"frontmost_app": app(1, "A"), "key_window": {"pid": 1, "ax_identity": 77}, "fullscreen": False,
               "active_space_epoch": 2, "extra": "never kept",
               "unavailable": {"cursor": "not read", "fullscreen": "No readable focused window"}},
     "steps": [{"do": "start", "task": "tree"},
               {"do": "before", "task": "tree.worker", "span": "k", "tool": "mac.key", "on_screen": True},
               {"do": "set", "state": {"frontmost_app": app(1, "A"), "key_window": {"pid": 1, "ax_identity": 78},
                                       "fullscreen": True, "active_space_epoch": 2}},
               {"do": "after", "span": "k"},
               {"do": "before", "task": "tree.worker", "span": "f", "tool": "mac.key", "on_screen": True},
               {"do": "patch", "state": {"fullscreen": False, "active_space_epoch": 3}},
               {"do": "after", "span": "f"},
               {"do": "context", "task": "tree"},
               {"do": "facts", "task": "tree"},
               {"do": "facts", "task": "nobody"},
               {"do": "finish", "task": "tree"}]},
    {"name": "sound_near_calls", "what": "test_sound_stopping_in_the_owners_tab_near_a_ferb_call_is_a_disturbance_fact",
     "state": {"frontmost_app": {"pid": 1}},
     "owner_input": {"monitored": True, "recent": False, "last_event_wall": None},
     "steps": [{"do": "sound", "tab": {"browser": "chrome", "tab_id": 41}, "change": {"audible": {"before": True,
                                                                                               "after": False}}},
               {"do": "start", "task": "x"},
               {"do": "call_started", "task": "x.worker", "span": "span-1", "tool": "browser.use_tab"},
               {"do": "sound", "tab": {"browser": "chrome", "tab_id": 41}, "change": {}},
               {"do": "sound", "tab": {"browser": "chrome", "tab_id": 41},
                "change": {"audible": {"before": True, "after": False}}},
               {"do": "call_ended", "span": "span-1"},
               {"do": "call_ended", "span": "span-unknown"},
               {"do": "mono", "advance_s": 4.0},
               {"do": "sound", "tab": {"browser": "chrome", "tab_id": 41},
                "change": {"muted": {"before": False, "after": True}}},
               {"do": "mono", "advance_s": 2.0},
               {"do": "sound", "tab": {"browser": "chrome", "tab_id": 41},
                "change": {"audible": {"before": False, "after": True}}},
               {"do": "start", "task": "y"},
               {"do": "call_started", "task": "y.step.a", "span": "s-y", "tool": "browser.click"},
               {"do": "call_started", "task": "x.step.b", "span": "s-x", "tool": "browser.type"},
               {"do": "call_started", "task": "z.worker", "span": "s-z", "tool": "browser.read"},
               {"do": "call_started", "task": "y.step.a", "span": "s-y", "tool": "browser.scroll"},
               {"do": "sound", "tab": {"browser": "chrome", "tab_id": 42, "task": "z"},
                "change": {"audible": {"before": False, "after": True}}},
               {"do": "facts", "task": "x"},
               {"do": "paused", "on": True},
               {"do": "sound", "tab": {"browser": "chrome", "tab_id": 43},
                "change": {"audible": {"before": False, "after": True}}},
               {"do": "paused", "on": False},
               {"do": "finish", "task": "x"}, {"do": "finish", "task": "y"}]},
    {"name": "sound_owner_input", "what": "HAND-213 / SENSE-406: owner input near, the tap down, a reader that fails",
     "state": {"frontmost_app": {"pid": 1}},
     "owner_input": {"monitored": True, "recent": False, "last_event_wall": None},
     "steps": [{"do": "start", "task": "x"},
               {"do": "call_started", "task": "x.worker", "span": "span-1", "tool": "browser.use_tab"},
               {"do": "owner_input", "value": {"monitored": True, "recent": False,
                                                "last_event_wall": START.timestamp() - 1.0}},
               {"do": "sound", "tab": {"tab_id": 41}, "change": {"audible": {"before": True, "after": False}}},
               {"do": "owner_input", "value": {"monitored": True, "recent": False,
                                                "last_event_wall": START.timestamp() - 3.0}},
               {"do": "sound", "tab": {"tab_id": 41}, "change": {"audible": {"before": True, "after": False}}},
               {"do": "owner_input", "value": {"monitored": True, "recent": True, "last_event_wall": None}},
               {"do": "sound", "tab": {"tab_id": 41}, "change": {"audible": {"before": True, "after": False}}},
               {"do": "owner_input", "value": {"monitored": True, "recent": False,
                                                "last_event_wall": START.timestamp() - 60.0}},
               {"do": "sound", "tab": {"tab_id": 41}, "change": {"audible": {"before": False, "after": True}}},
               {"do": "owner_input", "value": {"monitored": False, "recent": False, "last_event_wall": None}},
               {"do": "sound", "tab": {"tab_id": 41}, "change": {"muted": {"before": False, "after": True}}},
               {"do": "owner_input", "value": {"recent": 0, "last_event_wall": "not a number"}},
               {"do": "sound", "tab": {"tab_id": 41}, "change": {"muted": {"before": True, "after": False}}},
               {"do": "owner_input", "value": {"recent": False, "last_event_wall": str(START.timestamp() - 2.0)}},
               {"do": "sound", "tab": {"tab_id": 41}, "change": {"muted": {"before": False, "after": True}}},
               {"do": "owner_input", "value": "RuntimeError: tap gone"},
               {"do": "sound", "tab": {"tab_id": 41}, "change": {"muted": {"before": True, "after": False}}},
               {"do": "finish", "task": "x"}]},
]


def pure() -> dict[str, Any]:
    a, b = app(1, "A"), app(2, "")
    samples = [
        (None, {"frontmost_app": a}), ({}, {"frontmost_app": a}), ({"frontmost_app": a}, None),
        ({"frontmost_app": a, "cursor": [1, 1]}, {"frontmost_app": b, "cursor": [2, 2]}),
        ({"frontmost_app": a, "key_window": 1, "fullscreen": False, "active_space_epoch": 0},
         {"frontmost_app": a, "key_window": 2, "fullscreen": True, "active_space_epoch": 1}),
        ({"frontmost_app": a, "unavailable": {"frontmost_app": "x"}}, {"frontmost_app": b}),
        ({"frontmost_app": a}, {"frontmost_app": b, "unavailable": {"key_window": "y"}, "key_window": 3}),
        ({"active_space_epoch": 1}, {"active_space_epoch": 1.0}), ({"fullscreen": True}, {"fullscreen": 1}),
        ({"key_window": None}, {}), ({"unavailable": None, "fullscreen": False}, {"fullscreen": None}),
    ]
    told_in = [{}, {"frontmost_app": {"before": a, "after": b}},
               {"frontmost_app": {"before": "x", "after": None}, "key_window": {"before": 1, "after": 2}},
               {"key_window": {"before": 1, "after": 2}, "fullscreen": {"before": False, "after": True}},
               {"active_space_epoch": {"before": 1, "after": 2}},
               {"frontmost_app": {}, "key_window": {"before": 1, "after": 2}},
               {"frontmost_app": {"before": {"bundle_id": "b.id"}, "after": {"name": "", "bundle_id": ""}}},
               {"fullscreen": {}}]
    elapsed = [0.0, -1.0, 0.07, 0.7, 7.68, 9.99, 9.999, 10.0, 12.9, 59.5]
    return {"screen_changes": [[x, y, dist.screen_changes(x, y)] for x, y in samples],
            "told": [[c, dist.told(c)] for c in told_in],
            "elapsed": [[s, dist._elapsed(s)] for s in elapsed]}


def main() -> None:
    out = {"pure": pure(), "scenarios": [run(sc) for sc in SCENARIOS]}
    text = json.dumps(out, ensure_ascii=False, indent=1) + "\n"
    (HERE / "disturbance.json").write_text(text, encoding="utf-8")
    print(f"disturbance.json: {len(text.encode())} bytes, {len(out['scenarios'])} scenarios")


if __name__ == "__main__":
    main()
