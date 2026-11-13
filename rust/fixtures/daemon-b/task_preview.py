"""Fixtures for the Rust port of ferb/daemon/task_preview.py (ENG-3): TaskPreviews driven by scripted steps, with the
real ResourceLocks (ferb/act/locks.py) and scripted captures, a fake monotonic clock (ferb.daemon.task_preview.time
and ferb.act.point.time: it moves only by a `clock` step or a capture's `cost_s`), the status, sanitizer, Screen
Recording check and journal note faked.

    cd <repo> && uv run python rust/fixtures/daemon-b/task_preview.py

Writes task_preview.json beside this file. Each scenario: `limit_s`, `during_reads` (how the daemon's try_hold is
called), `allowed`, the `surfaces` (each a list of capture behaviors, used in turn, the last one repeated: `frame`
with an optional masking `note`, `raise` an error class, `slow` (runs past the limit), `gate` (waits for a `release`
step), `cost_s` (the clock moves while it captures)), the `steps`, and what came out: each step's `result` (a frame's
answer with the image's bytes as text, a snapshot, whether locks were free), and `notes` (preview.slow /
preview.failed). The cases are tests/test_task_preview.py's and tests/test_hangs_and_previews.py's, plus edges found
reading the code. A `slow` capture takes `limit_s` (0.2 s) of real time here; nothing else waits. Invented data only;
the same bytes on every run.
"""

import asyncio
import json
import os
import sys
import time
from pathlib import Path
from types import SimpleNamespace
from typing import Any

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
os.environ["TZ"] = "UTC"
time.tzset()
sys.path.insert(0, str(REPO))

import ferb.act.point as point  # noqa: E402
import ferb.daemon.task_preview as tp  # noqa: E402
from ferb.act.locks import ResourceLocks  # noqa: E402
from ferb.act.mac.screenshot import CaptureBusy, ScreenOff  # noqa: E402
from ferb.act.point import PreviewTarget  # noqa: E402
from ferb.core.errors import NoTarget, Refused, StaleRef, Withheld  # noqa: E402


class CaptureError(RuntimeError):
    def __init__(self, message: str, domain: str, code: int) -> None:
        super().__init__(message)
        self.domain, self.code = domain, code


ERRORS = {"Withheld": Withheld, "Refused": Refused, "StaleRef": StaleRef, "NoTarget": NoTarget,
          "ScreenOff": ScreenOff, "CaptureBusy": CaptureBusy, "RuntimeError": RuntimeError,
          "ValueError": ValueError}


def _error(b: dict[str, Any]) -> BaseException:
    if b["raise"] == "CaptureError":
        return CaptureError(b.get("text", ""), b["domain"], b["code"])
    return ERRORS[b["raise"]](b.get("text", ""))


class Clock:
    def __init__(self) -> None:
        self.t = 500.0

    def monotonic(self) -> float:
        return self.t


def run(sc: dict[str, Any]) -> dict[str, Any]:
    clock = Clock()
    tp.time = SimpleNamespace(monotonic=clock.monotonic)          # type: ignore[attr-defined]
    point.time = SimpleNamespace(monotonic=clock.monotonic)       # type: ignore[attr-defined]
    locks = ResourceLocks()
    notes: list[Any] = []
    during = sc.get("during_reads", True)

    async def sanitize(image: bytes) -> bytes:
        if image == b"bad":
            raise ValueError("unreadable frame")
        return image

    previews = tp.TaskPreviews(sanitize, lambda keys, holder: locks.try_hold(keys, holder, during_reads=during),
                               allowed=lambda: sc.get("allowed", True), limit_s=sc.get("limit_s", 2.0),
                               note=lambda kind, data: notes.append([kind, json.loads(json.dumps(data))]))
    used: dict[str, int] = {}
    gates: dict[str, asyncio.Event] = {}
    entered: dict[str, asyncio.Event] = {}

    def event(table: dict[str, asyncio.Event], name: str) -> asyncio.Event:
        return table.setdefault(name, asyncio.Event())

    def behavior(name: str) -> dict[str, Any]:
        queue = sc["surfaces"][name]
        n = used.get(name, 0)
        used[name] = n + 1
        return queue[min(n, len(queue) - 1)]  # type: ignore[no-any-return]

    def capture_of(name: str) -> Any:
        async def capture() -> Any:
            b = behavior(name)
            if "gate" in b:
                event(entered, b["gate"]).set()
                await event(gates, b["gate"]).wait()
            if b.get("slow"):
                await asyncio.sleep(5)
            clock.t += b.get("cost_s", 0.0)
            if "raise" in b:
                raise _error(b)
            frame = b["frame"].encode()
            return (frame, b["note"]) if "note" in b else frame
        return capture

    def target(spec: Any) -> Any:
        if spec is None:
            return None
        surface = spec.get("surface")
        if surface is None:
            return PreviewTarget(spec["scope"], spec["label"])
        resolve_spec = spec.get("resolve", {})

        async def resolve() -> Any:
            clock.t += resolve_spec.get("cost_s", 0.0)
            if "raise" in resolve_spec:
                raise _error(resolve_spec)
            return {"scope": spec["scope"]}, capture_of(surface)
        return PreviewTarget(spec["scope"], spec["label"], resolve)

    table: dict[str, Any] | None = None

    def status(root: str, worker: str) -> Any:
        if table is None:
            return None
        got = table.get(worker, table.get("*"))
        return tuple(got) if got else None
    previews.status = status

    def shown(answer: dict[str, Any]) -> dict[str, Any]:
        return {k: (v.decode() if isinstance(v, bytes) else v) for k, v in answer.items()}

    results: list[Any] = []
    tasks: dict[str, asyncio.Task[Any]] = {}
    holds: dict[str, Any] = {}

    async def go() -> None:
        nonlocal table
        for st in sc["steps"]:
            do = st["do"]
            out: Any = None
            if do == "declare":
                previews.declare(st["root"], st["worker"], target(st["target"]), tuple(st.get("locks", [])))
            elif do == "frame":
                out = shown(await previews.frame(st["root"], st.get("worker"), st.get("have")))
            elif do == "frame_bg":
                tasks[st["id"]] = asyncio.create_task(previews.frame(st["root"], st.get("worker"), st.get("have")))
                await asyncio.sleep(0)
            elif do == "wait_entered":
                await event(entered, st["gate"]).wait()
            elif do == "await":
                out = shown(await tasks.pop(st["id"]))
            elif do == "cancel":
                t = tasks.pop(st["id"])
                t.cancel()
                try:
                    await t
                    out = "finished"
                except asyncio.CancelledError:
                    out = "cancelled"
                for _ in range(3):
                    await asyncio.sleep(0)
            elif do == "release":
                event(gates, st["gate"]).set()
                for _ in range(3):
                    await asyncio.sleep(0)
            elif do == "remove":
                previews.remove(set(st["roots"]))
                for _ in range(3):
                    await asyncio.sleep(0)
            elif do == "status":
                table = st["table"]
            elif do == "hold":
                cm = locks.hold(st["keys"], "worker", reading=st.get("reading", False))
                await cm.__aenter__()
                holds[st["id"]] = cm
            elif do == "unhold":
                await holds.pop(st["id"]).__aexit__(None, None, None)
            elif do == "free":
                async with locks.try_hold(st["keys"], "probe") as held:
                    out = held
            elif do == "clock":
                clock.t += st["advance_s"]
            elif do == "snapshot":
                out = previews.snapshot()
            else:
                raise ValueError(do)
            results.append(out)
    asyncio.run(asyncio.wait_for(go(), 30))
    return {"name": sc["name"], "what": sc["what"], "limit_s": sc.get("limit_s", 2.0),
            "during_reads": during, "allowed": sc.get("allowed", True), "surfaces": sc["surfaces"],
            "steps": sc["steps"], "results": results, "notes": notes}


def T(scope: str, surface: str | None, label: str = "Notes", **kw: Any) -> dict[str, Any]:
    return {"scope": scope, "label": label, "surface": surface, **kw}


NOTES = ["app:com.apple.Notes"]
SCENARIOS: list[dict[str, Any]] = [
    {"name": "never_queues_and_abandoned_frees", "what": "test_a_request_never_queues_behind_the_worker_and_an_abandoned_"
     "one_frees_the_surface", "surfaces": {"first": [{"frame": "first"}], "stuck": [{"gate": "g", "frame": "late"}]},
     "steps": [{"do": "declare", "root": "t", "worker": "t.worker", "target": T("app:Notes", "first"), "locks": NOTES},
               {"do": "frame", "root": "t"},
               {"do": "hold", "id": "w", "keys": NOTES},
               {"do": "frame", "root": "t"},
               {"do": "unhold", "id": "w"},
               {"do": "declare", "root": "t", "worker": "t.worker", "target": T("app:Notes", "stuck"), "locks": NOTES},
               {"do": "frame_bg", "id": "r", "root": "t"},
               {"do": "wait_entered", "gate": "g"},
               {"do": "free", "keys": NOTES},
               {"do": "cancel", "id": "r"},
               {"do": "free", "keys": NOTES},
               {"do": "snapshot"}]},
    {"name": "slow_capture", "what": "test_a_slow_capture_ends_within_the_daemon_time_limit_with_the_last_frame",
     "limit_s": 0.2, "surfaces": {"ok": [{"frame": "kept"}], "slow": [{"slow": True}]},
     "steps": [{"do": "declare", "root": "t", "worker": "t.worker", "target": T("app:Notes", "ok")},
               {"do": "frame", "root": "t"},
               {"do": "clock", "advance_s": 1.5},
               {"do": "declare", "root": "t", "worker": "t.worker", "target": T("app:Notes", "slow")},
               {"do": "frame", "root": "t"},
               {"do": "frame", "root": "t"},
               {"do": "clock", "advance_s": 700},
               {"do": "frame", "root": "t"},
               {"do": "snapshot"}]},
    {"name": "no_frame_after_end", "what": "test_no_frame_after_the_task_ends_even_from_a_capture_already_running",
     "surfaces": {"cap": [{"gate": "g", "frame": "captured"}]},
     "steps": [{"do": "declare", "root": "t", "worker": "t.step.a", "target": T("tab:browser/7", "cap")},
               {"do": "frame_bg", "id": "p", "root": "t"},
               {"do": "wait_entered", "gate": "g"},
               {"do": "remove", "roots": ["t"]},
               {"do": "release", "gate": "g"},
               {"do": "await", "id": "p"},
               {"do": "status", "table": {"*": ["done", "Done"]}},
               {"do": "frame", "root": "t"},
               {"do": "status", "table": None},
               {"do": "frame", "root": "t"},
               {"do": "snapshot"}]},
    {"name": "gone_tab", "what": "test_no_frame_for_a_tab_the_task_does_not_own (the resolve refuses)",
     "surfaces": {"never": [{"frame": "x"}]},
     "steps": [{"do": "declare", "root": "t", "worker": "t.step.a",
                "target": T("tab:browser/tab-9", "never", "the browser",
                            resolve={"raise": "Refused", "text": "that tab isn't this task's"})},
               {"do": "frame", "root": "t"}, {"do": "frame", "root": "t"}, {"do": "snapshot"}]},
    {"name": "freezes_with_state", "what": "test_the_preview_freezes_with_a_state_when_ferb_leaves_the_surface_or_asks",
     "surfaces": {"wa": [{"frame": "one"}, {"frame": "two"}, {"frame": "three"}]},
     "steps": [{"do": "declare", "root": "t", "worker": "t.worker", "target": T("app:WhatsApp", "wa", "WhatsApp")},
               {"do": "frame", "root": "t"},
               {"do": "declare", "root": "t", "worker": "t.worker", "target": None},
               {"do": "clock", "advance_s": 0.25},
               {"do": "frame", "root": "t"},
               {"do": "declare", "root": "t", "worker": "t.worker", "target": T("app:WhatsApp", "wa", "WhatsApp")},
               {"do": "status", "table": {"*": ["asking", "Asking you: Send it?"]}},
               {"do": "frame", "root": "t"},
               {"do": "status", "table": None},
               {"do": "frame", "root": "t"},
               {"do": "snapshot"}]},
    {"name": "parallel_steps", "what": "test_parallel_steps_each_keep_their_surface_and_the_answer_names_the_step",
     "surfaces": {"a": [{"frame": "tab-a"}], "b": [{"frame": "tab-b"}]},
     "steps": [{"do": "declare", "root": "t", "worker": "t.step.permit", "target": T("tab:browser/1", "a")},
               {"do": "declare", "root": "t", "worker": "t.step.term", "target": T("tab:browser/2", "b")},
               {"do": "frame", "root": "t"},
               {"do": "frame", "root": "t", "worker": "t.step.permit"},
               {"do": "frame", "root": "t", "worker": "t.step.nobody"},
               {"do": "declare", "root": "t", "worker": "t.step.term", "target": None},
               {"do": "frame", "root": "t"},
               {"do": "declare", "root": "t", "worker": "t.step.permit", "target": None},
               {"do": "frame", "root": "t"},
               {"do": "status", "table": {"t.step.term": ["waiting", "Waiting for 2 minutes"]}},
               {"do": "frame", "root": "t", "worker": "t.step.term"},
               {"do": "frame", "root": "other"},
               {"do": "snapshot"}]},
    {"name": "rows_take_turns", "what": "test_parallel_rows_take_the_tasks_one_capture_in_turn",
     "surfaces": {"a": [{"gate": "g", "frame": "tab-a"}, {"frame": "tab-a2"}], "b": [{"frame": "tab-b"}]},
     "steps": [{"do": "declare", "root": "t", "worker": "t.step.book", "target": T("tab:browser/1", "a")},
               {"do": "declare", "root": "t", "worker": "t.step.compare", "target": T("tab:browser/2", "b")},
               {"do": "frame_bg", "id": "first", "root": "t", "worker": "t.step.book"},
               {"do": "wait_entered", "gate": "g"},
               {"do": "frame", "root": "t", "worker": "t.step.compare"},
               {"do": "frame", "root": "t", "worker": "t.step.compare"},
               {"do": "release", "gate": "g"},
               {"do": "await", "id": "first"},
               {"do": "frame", "root": "t", "worker": "t.step.book"},
               {"do": "frame", "root": "t", "worker": "t.step.compare"},
               {"do": "frame", "root": "t", "worker": "t.step.book"},
               {"do": "frame", "root": "t", "worker": "t.step.compare"},
               {"do": "snapshot"}]},
    {"name": "turn_expires", "what": "DMN-584: a row that stopped asking holds nobody up after TURN_S",
     "surfaces": {"a": [{"gate": "g", "frame": "tab-a"}, {"frame": "tab-a2"}], "b": [{"frame": "tab-b"}]},
     "steps": [{"do": "declare", "root": "t", "worker": "t.step.book", "target": T("tab:browser/1", "a")},
               {"do": "declare", "root": "t", "worker": "t.step.compare", "target": T("tab:browser/2", "b")},
               {"do": "frame_bg", "id": "first", "root": "t", "worker": "t.step.book"},
               {"do": "wait_entered", "gate": "g"},
               {"do": "frame", "root": "t", "worker": "t.step.compare"},
               {"do": "release", "gate": "g"},
               {"do": "await", "id": "first"},
               {"do": "clock", "advance_s": 1.5},
               {"do": "frame", "root": "t", "worker": "t.step.book"},
               {"do": "snapshot"}]},
    {"name": "two_waiters_share", "what": "requests for the surface being captured share the one capture",
     "surfaces": {"a": [{"gate": "g", "frame": "shared", "cost_s": 0.125}]},
     "steps": [{"do": "declare", "root": "t", "worker": "t.worker", "target": T("tab:browser/1", "a")},
               {"do": "frame_bg", "id": "one", "root": "t"},
               {"do": "wait_entered", "gate": "g"},
               {"do": "frame_bg", "id": "two", "root": "t"},
               {"do": "clock", "advance_s": 0.5},
               {"do": "cancel", "id": "one"},
               {"do": "release", "gate": "g"},
               {"do": "await", "id": "two"},
               {"do": "snapshot"}]},
    {"name": "withheld_and_masking", "what": "test_a_withheld_frame_and_the_masking_note_are_said",
     "surfaces": {"masked": [{"frame": "page", "note": "masked by text recognition only"}],
                  "withheld": [{"raise": "Withheld", "text": "its password field couldn't be painted over"},
                               {"frame": "page2"}]},
     "steps": [{"do": "declare", "root": "t", "worker": "t.worker", "target": T("tab:browser/1", "masked")},
               {"do": "frame", "root": "t"},
               {"do": "declare", "root": "t", "worker": "t.worker", "target": T("tab:browser/1", "withheld")},
               {"do": "frame", "root": "t"},
               {"do": "frame", "root": "t"},
               {"do": "snapshot"}]},
    {"name": "unchanged_frame_and_too_long_once", "what": "test_an_unchanged_frame_isnt_sent_again_and_too_long_is_said"
     "_once", "limit_s": 0.2,
     "surfaces": {"a": [{"frame": "frame", "cost_s": 0.04}, {"slow": True}, {"frame": "frame2"}]},
     "steps": [{"do": "declare", "root": "t", "worker": "t.worker", "target": T("tab:browser/1", "a"),
                "locks": ["tab:browser/1"]},
               {"do": "clock", "advance_s": 0.01},
               {"do": "frame", "root": "t"},
               {"do": "clock", "advance_s": 0.3},
               {"do": "hold", "id": "w", "keys": ["tab:browser/1"]},
               {"do": "frame", "root": "t", "have": 1},
               {"do": "unhold", "id": "w"},
               {"do": "frame", "root": "t", "have": 1},
               {"do": "hold", "id": "w", "keys": ["tab:browser/1"]},
               {"do": "frame", "root": "t", "have": 1},
               {"do": "unhold", "id": "w"},
               {"do": "snapshot"},
               {"do": "snapshot"},
               {"do": "hold", "id": "r", "keys": ["tab:browser/1"], "reading": True},
               {"do": "frame", "root": "t", "have": 1},
               {"do": "unhold", "id": "r"},
               {"do": "snapshot"}]},
    {"name": "app_only_and_locked", "what": "test_a_call_naming_only_the_app_keeps_this_steps_window_of_it_and_a_locked_"
     "screen_is_said",
     "surfaces": {"pp": [{"frame": "slide"}, {"frame": "slide2"}, {"raise": "ScreenOff", "text": "locked"},
                         {"raise": "CaptureBusy", "text": "the worker's screenshot"}, {"frame": "slide3"}]},
     "steps": [{"do": "declare", "root": "t", "worker": "t.worker", "target": T("app:PowerPoint", "pp", "PowerPoint")},
               {"do": "frame", "root": "t"},
               {"do": "declare", "root": "t", "worker": "t.worker", "target": T("app:PowerPoint", None, "PowerPoint")},
               {"do": "frame", "root": "t"},
               {"do": "frame", "root": "t"},
               {"do": "frame", "root": "t"},
               {"do": "frame", "root": "t"},
               {"do": "declare", "root": "t", "worker": "t.worker", "target": T("app:Keynote", None, "Keynote")},
               {"do": "frame", "root": "t"},
               {"do": "snapshot"}]},
    {"name": "no_picture_first", "what": "a surface with no window yet: no_picture; a new surface keeps the old frame",
     "surfaces": {"n": [{"frame": "notes"}], "m": [{"frame": "mail", "cost_s": 0.002}]},
     "steps": [{"do": "frame", "root": "t"},
               {"do": "declare", "root": "t", "worker": "t.worker", "target": T("app:Notes", None)},
               {"do": "frame", "root": "t"},
               {"do": "declare", "root": "t", "worker": "t.worker", "target": T("app:Notes", "n")},
               {"do": "frame", "root": "t"},
               {"do": "declare", "root": "t", "worker": "t.worker", "target": T("app:Mail", None, "Mail")},
               {"do": "frame", "root": "t"},
               {"do": "declare", "root": "t", "worker": "t.worker", "target": T("app:Mail", "m", "Mail")},
               {"do": "status", "table": {"t.worker": ["paused", "Paused"]}},
               {"do": "frame", "root": "t"},
               {"do": "status", "table": None},
               {"do": "frame", "root": "t", "have": 1},
               {"do": "frame", "root": "t", "have": 2},
               {"do": "remove", "roots": ["t.worker"]},
               {"do": "frame", "root": "t"},
               {"do": "remove", "roots": ["t"]},
               {"do": "frame", "root": "t"},
               {"do": "snapshot"}]},
    {"name": "failures_journaled", "what": "DMN-408: a failed capture journaled once per window with a count; gone, "
     "not_allowed and the error's domain and code", "allowed": False,
     "surfaces": {"f": [{"raise": "CaptureError", "text": "a window title here", "domain": "SCStreamErrorDomain",
                         "code": -3801}, {"raise": "RuntimeError", "text": "x"}, {"frame": "bad"},
                        {"raise": "StaleRef", "text": "window closed"}, {"raise": "NoTarget", "text": "no window"},
                        {"frame": "ok"}]},
     "steps": [{"do": "declare", "root": "t", "worker": "t.worker", "target": T("app:Notes", "f")},
               {"do": "frame", "root": "t"},
               {"do": "frame", "root": "t"},
               {"do": "clock", "advance_s": 601},
               {"do": "frame", "root": "t"},
               {"do": "frame", "root": "t"},
               {"do": "frame", "root": "t"},
               {"do": "frame", "root": "t"},
               {"do": "snapshot"}]},
    {"name": "plain_try_hold", "what": "without during_reads a read holding the surface makes the preview busy",
     "during_reads": False, "surfaces": {"a": [{"frame": "a1"}]},
     "steps": [{"do": "declare", "root": "t", "worker": "t.worker", "target": T("tab:browser/1", "a"),
                "locks": ["tab:browser/1"]},
               {"do": "hold", "id": "r", "keys": ["tab:browser/1"], "reading": True},
               {"do": "frame", "root": "t"},
               {"do": "unhold", "id": "r"},
               {"do": "declare", "root": "t", "worker": "t.worker", "target": T("tab:browser/1", "a"),
                "locks": ["bogus:1"]},
               {"do": "frame", "root": "t"},
               {"do": "snapshot"}]},
]


def main() -> None:
    out = {"scenarios": [run(sc) for sc in SCENARIOS]}
    text = json.dumps(out, ensure_ascii=False, indent=1) + "\n"
    (HERE / "task_preview.json").write_text(text, encoding="utf-8")
    print(f"task_preview.json: {len(text.encode())} bytes, {len(out['scenarios'])} scenarios")


if __name__ == "__main__":
    main()
