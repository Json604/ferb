"""The live preview of a task's work (kb area app): masked frames of the one surface (a task's browser tab, or the
app window a call targets) its latest call works on, for FERB.app only. Pixels live in memory only: never journaled,
saved, traced or logged.

Before each call the capability declares the surface (`preview_target`, beside `point` and `on_screen`); nothing is
resolved then, so a call never waits for the preview and a declaration can't fail it. The first frame request resolves
the surface. A capture never queues on the worker's resource locks: it takes them only when free (`try_hold`), else the
last frame is returned with the state. One capture at a time per task, within PREVIEW_LIMIT_S (under the app's 10 s
reply wait); a request whose caller left, or that ran out of time, cancels the capture when nobody else waits for it.
Each answer carries the state (APP-404): working here, working elsewhere (the latest call left this surface),
asking, waiting, paused, done; while FERB isn't working here the surface is frozen on its last frame and nothing is
captured. Parallel steps of one task each keep their own surface (by the step's task id); the answer names the step it
shows. Frames and capture time are counted for the heat measurement (APP-405); no frame-rate cap."""
from __future__ import annotations

import asyncio
import time
from collections.abc import Awaitable, Callable
from contextlib import AbstractAsyncContextManager
from dataclasses import dataclass
from typing import Any

from ferb.act.point import PreviewTarget
from ferb.core.errors import Refused, Withheld

Capture = Callable[[], Awaitable[Any]]                 # one frame: bytes, or (bytes, masking note)
Status = Callable[[str, str], "tuple[str, str] | None"]
TryHold = Callable[[list[str], str], AbstractAsyncContextManager[bool]]

PREVIEW_LIMIT_S = 8.0          # a request's time limit: under the notch's 10 s reply wait (server.CLIENT_WAIT_S)
ELSEWHERE = "Working on another part of this task"
NO_PICTURE = "Working in {label}"


@dataclass
class _Surface:
    target: PreviewTarget
    locks: tuple[str, ...]
    capture: Capture | None = None       # resolved on the first frame request after the target was declared
    frame: bytes | None = None           # its last masked frame (memory only)
    note: str = ""                       # how its secret fields were found, when only text recognition masked it
    problem: dict[str, str] | None = None


@dataclass
class _Worker:
    """One step's preview (keyed by its task id): the surface its latest call works on (None: a call on no surface, a
    file or a command), and the last visual surface it worked on, kept with its last frame."""
    current: str | None = None
    label: str = ""
    surface: _Surface | None = None


@dataclass
class _Job:
    surface: _Surface
    task: "asyncio.Task[str]"            # how it ended: "fresh", "busy" (the worker holds the surface) or "failed"
    waiters: int = 0


@dataclass
class Counts:
    """Cumulative counters for resource_samples.work_json (APP-405)."""
    requests: int = 0
    frames: int = 0
    capture_ms: float = 0.0
    cached: int = 0
    busy: int = 0


class TaskPreviews:
    def __init__(self, sanitize: Callable[[bytes], Awaitable[bytes]], try_hold: TryHold,
                 allowed: Callable[[], bool] = lambda: True, limit_s: float = PREVIEW_LIMIT_S) -> None:
        self.sanitize, self.try_hold, self.allowed, self.limit_s = sanitize, try_hold, allowed, limit_s
        # what the task is doing now, by FERB's records (the supervisor's preview_status): None while it works
        self.status: Status = lambda root, worker: None
        self._roots: dict[str, dict[str, _Worker]] = {}     # root task -> step task id -> its preview, latest last
        self._jobs: dict[str, _Job] = {}
        self.counts = Counts()

    # ---------- the gateway's side: what each call works on ----------

    def declare(self, root: str, worker: str, target: PreviewTarget | None, locks: tuple[str, ...] = ()) -> None:
        """Before a call of `worker` (a step's task id) runs: the surface it works on (None: no surface). Nothing is
        resolved or captured here. A new surface replaces the worker's last one only once it has a frame of its own;
        until then the last frame stays, frozen."""
        workers = self._roots.setdefault(root, {})
        w = workers.pop(worker, None) or _Worker()
        workers[worker] = w                                  # most recent last
        w.current, w.label = (target.scope, target.label) if target is not None else (None, "")
        if target is None:
            return
        if w.surface is not None and w.surface.target.scope == target.scope:
            w.surface.target, w.surface.locks, w.surface.capture = target, locks, None   # resolved again, lazily
        elif target.resolve is not None or w.surface is None:
            frozen = w.surface
            w.surface = _Surface(target, locks)
            if frozen is not None:                           # the last frame stays until this surface has its own
                w.surface.frame, w.surface.note = frozen.frame, frozen.note

    def remove(self, roots: set[str]) -> None:
        """A task ended (or was cancelled): its surfaces, frames and captures go."""
        for root in [r for r in self._roots if any(r == t or r.startswith(t + ".") for t in roots)]:
            del self._roots[root]
            job = self._jobs.pop(root, None)
            if job is not None:
                job.task.cancel()

    # ---------- the app's side: one answer per request ----------

    async def frame(self, root: str, worker: str | None = None) -> dict[str, Any]:
        """The preview now: `state`, a short `line`, the frame (`image`, bytes; `fresh` when captured for this request)
        and the step it shows. No frame for a task that ended, or one with no surface yet."""
        self.counts.requests += 1
        workers = self._roots.get(root)
        doing = self.status(root, worker or "")
        if doing is not None and doing[0] == "done":
            return {"state": "done", "line": doing[1]}
        if not workers:
            return {"state": "none", "line": "No window to show yet"}
        key = worker if worker in workers else next(
            (k for k in reversed(workers) if workers[k].current is not None and workers[k].surface is not None),
            next(reversed(workers)))
        w = workers[key]
        doing = self.status(root, key)
        if doing is not None:                         # asking, waiting, paused, or this step isn't running: frozen
            return self._answer(key, w, *doing)
        surface = w.surface
        if w.current is None or surface is None:
            return self._answer(key, w, "working_elsewhere", ELSEWHERE)
        line = NO_PICTURE.format(label=w.label)
        if surface.target.scope != w.current or surface.target.resolve is None:
            return self._answer(key, w, "working_elsewhere" if surface.target.scope != w.current else "working", line)
        job = self._jobs.get(root)
        if job is not None and job.task.done():
            del self._jobs[root]
            job = None
        if job is None:
            job = self._jobs[root] = _Job(surface, asyncio.get_running_loop().create_task(self._capture(root, surface)))
        elif job.surface is not surface:              # one capture at a time per task: another step's is running
            self.counts.busy += 1
            return self._answer(key, w, "working", line, busy=True)
        job.waiters += 1
        try:
            finished, _ = await asyncio.wait({job.task}, timeout=self.limit_s)
            if not finished:
                surface.problem = {"kind": "slow", "text": "The capture took too long"}
        finally:
            job.waiters -= 1
            if job.waiters == 0 and not job.task.done():      # nobody waits for it any more (left, or timed out)
                job.task.cancel()
            if self._jobs.get(root) is job and (job.task.done() or job.waiters == 0):
                del self._jobs[root]
        if root not in self._roots:                   # the task ended during the capture: its pixels are discarded
            return {"state": "done", "line": "Done"}
        ended = job.task.result() if job.task.done() and not job.task.cancelled() else "failed"
        return self._answer(key, w, "working", line, fresh=ended == "fresh", busy=ended == "busy")

    async def _capture(self, root: str, surface: _Surface) -> str:
        """One capture of a surface, under its locks only if they are free (never queued behind the worker), then
        masked again by text recognition off the loop. A failure keeps the last frame and says why."""
        started = time.monotonic()
        try:
            async with self.try_hold(list(surface.locks), "preview:" + root) as held:
                if not held:
                    self.counts.busy += 1
                    return "busy"
                if surface.capture is None:
                    assert surface.target.resolve is not None
                    _, surface.capture = await surface.target.resolve()
                raw = await surface.capture()
            image, note = raw if isinstance(raw, tuple) else (raw, "")
            frame = await self.sanitize(image)
        except asyncio.CancelledError:
            raise
        except Withheld as e:                         # its secret fields couldn't be painted over: no frame
            surface.problem = {"kind": "withheld", "text": str(e)}
            return "failed"
        except Refused as e:                          # the window closed or changed owner, or the tab isn't the task's
            surface.capture, surface.problem = None, {"kind": "gone", "text": str(e)}
            return "failed"
        except Exception as e:
            surface.capture = None
            surface.problem = ({"kind": "failed", "text": type(e).__name__} if self.allowed()
                               else {"kind": "not_allowed", "text": "Screen Recording isn't allowed for FERB"})
            return "failed"
        if root not in self._roots:
            return "failed"
        surface.frame, surface.note, surface.problem = frame, note, None
        self.counts.frames += 1
        self.counts.capture_ms += (time.monotonic() - started) * 1000
        return "fresh"

    def _answer(self, key: str, w: _Worker, state: str, line: str, *, fresh: bool = False,
                busy: bool = False) -> dict[str, Any]:
        surface = w.surface
        if not fresh:
            self.counts.cached += 1
        out: dict[str, Any] = {"state": state, "line": line, "worker": key, "step": _step_of(key)}
        if surface is not None and surface.frame is not None:
            out |= {"image": surface.frame, "fresh": fresh}
        if surface is not None and surface.note:
            out["masking"] = surface.note
        if surface is not None and surface.problem and not fresh:
            out["problem"] = dict(surface.problem)
        if busy:
            out["busy"] = True
        return out

    def snapshot(self) -> dict[str, float]:
        """The counters, cumulative (resource_samples.work_json takes each window's growth)."""
        c = self.counts
        return {"requests": c.requests, "frames": c.frames, "capture_ms": round(c.capture_ms, 1),
                "cached": c.cached, "busy": c.busy}


def _step_of(worker: str) -> str:
    """The plan step a step's task id names ("<tree>.step.<id>"; a single task's one step is "worker")."""
    rest = worker.split(".", 1)[1] if "." in worker else worker
    return rest.removeprefix("step.")
