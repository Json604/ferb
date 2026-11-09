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
shows. Frames and capture time are counted for the heat measurement (APP-405); no frame-rate cap.

DMN-406..408: a capture looks alongside a call that only reads (`try_hold` during_reads); its own waits (a tab's
record_frame, a window's Accessibility walks) get only the time it has left (point.PREVIEW_DEADLINE); an answer
carries its frame's `seq` and `age_ms`, and leaves the image out when the app already has that frame (`have`); "too
long" is said on the answer that ran out of time only; a call that names only an app keeps showing the window this step
last showed of it; a locked screen is said as such. Each surface kind (tab, window) has its own counters, phase times
and peaks, and a capture that runs out of time or fails is journaled (preview.slow / preview.failed, bounded)."""
from __future__ import annotations

import asyncio
import time
from collections.abc import Awaitable, Callable
from contextlib import AbstractAsyncContextManager
from dataclasses import dataclass, field
from typing import Any

from ferb.act.mac.screenshot import CaptureBusy, ScreenOff
from ferb.act.point import PREVIEW_DEADLINE, PREVIEW_PHASES, PreviewTarget, preview_phase
from ferb.core.errors import Refused, Withheld

Capture = Callable[[], Awaitable[Any]]                 # one frame: bytes, or (bytes, masking note)
Status = Callable[[str, str], "tuple[str, str] | None"]
TryHold = Callable[[list[str], str], AbstractAsyncContextManager[bool]]

PREVIEW_LIMIT_S = 8.0          # a request's time limit: under the notch's 10 s reply wait (server.CLIENT_WAIT_S)
ELSEWHERE = "Working on another part of this task"
NO_PICTURE = "Working in {label}"
SLOW = {"kind": "slow", "text": "The capture took too long"}
LOCKED = {"kind": "screen_locked", "text": "The screen is locked or asleep, so macOS lets nobody capture windows"}
KINDS = ("tab", "window")
OUTCOMES = ("frames", "busy", "slow", "failed", "gone", "withheld", "locked", "no_picture")
NOTE_EVERY_S = 600.0           # a journaled preview problem: the first at once, then one row per window with a count


@dataclass
class _Surface:
    target: PreviewTarget
    locks: tuple[str, ...]
    capture: Capture | None = None       # resolved on the first frame request after the target was declared
    frame: bytes | None = None           # its last masked frame (memory only)
    note: str = ""                       # how its secret fields were found, when only text recognition masked it
    problem: dict[str, str] | None = None
    seq: int = 0                         # which frame it is (TaskPreviews-wide), so an unchanged one isn't sent again
    at: float = 0.0                      # when it was captured (time.monotonic())
    asked: float | None = None           # since when the app has been waiting for its next fresh frame

    @property
    def kind(self) -> str:
        return "tab" if self.target.scope.startswith("tab:") else "window"


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
    phase: str = "start"                 # where the capture is (for a journaled "too long")


@dataclass
class Counts:
    """Cumulative counters for resource_samples.work_json (APP-405), and per surface kind (DMN-406): `<kind>_<outcome>`
    counts, `<kind>_capture_ms` and `<kind>_<phase>_ms` (resolve, extension, walk1, capture, walk2, paint, ocr), plus
    peaks since the last read (`max_<kind>_gap_ms`: the longest wait for a fresh frame of one surface while the app
    asked; `max_<kind>_age_ms`: the oldest frame shown while FERB worked there)."""
    requests: int = 0
    frames: int = 0
    capture_ms: float = 0.0
    cached: int = 0
    busy: int = 0
    kinds: dict[str, float] = field(default_factory=dict)
    peaks: dict[str, float] = field(default_factory=dict)

    def add(self, key: str, n: float = 1) -> None:
        self.kinds[key] = self.kinds.get(key, 0) + n

    def peak(self, key: str, value: float) -> None:
        self.peaks[key] = max(self.peaks.get(key, 0.0), value)


class TaskPreviews:
    def __init__(self, sanitize: Callable[[bytes], Awaitable[bytes]], try_hold: TryHold,
                 allowed: Callable[[], bool] = lambda: True, limit_s: float = PREVIEW_LIMIT_S,
                 note: Callable[[str, dict[str, Any]], None] = lambda kind, data: None) -> None:
        self.sanitize, self.try_hold, self.allowed, self.limit_s = sanitize, try_hold, allowed, limit_s
        # what the task is doing now, by FERB's records (the supervisor's preview_status): None while it works
        self.status: Status = lambda root, worker: None
        self._roots: dict[str, dict[str, _Worker]] = {}     # root task -> step task id -> its preview, latest last
        self._jobs: dict[str, _Job] = {}
        self.counts = Counts()
        self._note = note
        self._noted: dict[str, tuple[float, int]] = {}      # journaled problems: key -> (next row due, count since)
        self._seq = 0

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
            if target.resolve is None and w.surface.target.resolve is not None:
                # a call naming only the app (a menu, a key press, opening a file): this step's last window of it
                # stays the picture (DMN-406), resolved again as it was
                target = PreviewTarget(target.scope, target.label, w.surface.target.resolve)
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

    async def frame(self, root: str, worker: str | None = None, have: int | None = None) -> dict[str, Any]:
        """The preview now: `state`, a short `line`, the frame (`image`, bytes; `fresh` when captured for this request;
        `seq` and `age_ms`, and no `image` when it is the frame `have` names: the app has it) and the step it shows.
        No frame for a task that ended, or one with no surface yet."""
        self.counts.requests += 1
        answer = await self._frame(root, worker)
        if have is not None and answer.get("seq") == have:
            answer.pop("image", None)                         # the app already shows this very frame
        return answer

    async def _frame(self, root: str, worker: str | None) -> dict[str, Any]:
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
        self.counts.add(f"{surface.kind}_requests")
        if surface.target.scope != w.current or surface.target.resolve is None:
            if surface.target.scope == w.current:
                self.counts.add(f"{surface.kind}_no_picture")
            return self._answer(key, w, "working_elsewhere" if surface.target.scope != w.current else "working", line)
        if surface.asked is None:
            surface.asked = time.monotonic()
        job = self._jobs.get(root)
        if job is not None and job.task.done():
            del self._jobs[root]
            job = None
        if job is None:
            job = self._jobs[root] = _Job(surface, asyncio.get_running_loop().create_task(self._capture(root, surface)))
        elif job.surface is not surface:              # one capture at a time per task: another step's is running
            self.counts.busy += 1
            self.counts.add(f"{surface.kind}_busy")
            return self._answer(key, w, "working", line, busy=True)
        job.waiters += 1
        slow = False
        try:
            finished, _ = await asyncio.wait({job.task}, timeout=self.limit_s)
            slow = not finished
            if slow:
                self.counts.add(f"{surface.kind}_slow")
                self._journal("preview.slow", surface, {"phase": job.phase, "seconds": self.limit_s})
        finally:
            job.waiters -= 1
            if job.waiters == 0 and not job.task.done():      # nobody waits for it any more (left, or timed out)
                job.task.cancel()
            if self._jobs.get(root) is job and (job.task.done() or job.waiters == 0):
                del self._jobs[root]
        if root not in self._roots:                   # the task ended during the capture: its pixels are discarded
            return {"state": "done", "line": "Done"}
        ended = job.task.result() if job.task.done() and not job.task.cancelled() else "failed"
        return self._answer(key, w, "working", line, fresh=ended == "fresh", busy=ended == "busy", slow=slow)

    async def _capture(self, root: str, surface: _Surface) -> str:
        """One capture of a surface, under its locks only if they are free or held by a call that only reads (never
        queued behind the worker), then masked again by text recognition off the loop. A failure keeps the last frame
        and says why. The capture's own waits get only the time the preview has left (PREVIEW_DEADLINE), and each
        phase's time is counted per surface kind."""
        started = time.monotonic()
        PREVIEW_DEADLINE.set(started + self.limit_s)          # this capture's task only (a context variable)
        phases: dict[str, float] = {}
        PREVIEW_PHASES.set(phases)
        job = self._jobs.get(root)
        kind = surface.kind
        try:
            async with self.try_hold(list(surface.locks), "preview:" + root) as held:
                if not held:
                    self.counts.busy += 1
                    self.counts.add(f"{kind}_busy")
                    return "busy"
                if surface.capture is None:
                    assert surface.target.resolve is not None
                    if job is not None:
                        job.phase = "resolve"
                    with preview_phase("resolve"):
                        _, surface.capture = await surface.target.resolve()
                if job is not None:
                    job.phase = "capture"
                raw = await surface.capture()
            image, note = raw if isinstance(raw, tuple) else (raw, "")
            if job is not None:
                job.phase = "ocr"
            with preview_phase("ocr"):
                frame = await self.sanitize(image)
        except asyncio.CancelledError:
            raise
        except CaptureBusy:                           # the worker's own screenshot is capturing (DMN-406): no wait
            self.counts.busy += 1
            self.counts.add(f"{kind}_busy")
            return "busy"
        except ScreenOff:                             # locked or asleep: nothing can capture a window now
            surface.problem = dict(LOCKED)
            self.counts.add(f"{kind}_locked")
            return "failed"
        except Withheld as e:                         # its secret fields couldn't be painted over: no frame
            surface.problem = {"kind": "withheld", "text": str(e)}
            self.counts.add(f"{kind}_withheld")
            return "failed"
        except Refused as e:                          # the window closed or changed owner, or the tab isn't the task's
            surface.capture, surface.problem = None, {"kind": "gone", "text": str(e)}
            self.counts.add(f"{kind}_gone")
            return "failed"
        except Exception as e:
            surface.capture = None
            surface.problem = ({"kind": "failed", "text": type(e).__name__} if self.allowed()
                               else {"kind": "not_allowed", "text": "Screen Recording isn't allowed for FERB"})
            self.counts.add(f"{kind}_failed")
            self._journal("preview.failed", surface, {"error": _error_name(e)})
            return "failed"
        finally:
            for name, ms in phases.items():
                self.counts.add(f"{kind}_{name}_ms", ms)
        if root not in self._roots:
            return "failed"
        now = time.monotonic()
        self._seq += 1
        surface.frame, surface.note, surface.problem, surface.seq, surface.at = frame, note, None, self._seq, now
        self.counts.frames += 1
        self.counts.capture_ms += (now - started) * 1000
        self.counts.add(f"{kind}_frames")
        self.counts.add(f"{kind}_capture_ms", (now - started) * 1000)
        if surface.asked is not None:
            self.counts.peak(f"max_{kind}_gap_ms", (now - surface.asked) * 1000)
            surface.asked = None
        return "fresh"

    def _answer(self, key: str, w: _Worker, state: str, line: str, *, fresh: bool = False,
                busy: bool = False, slow: bool = False) -> dict[str, Any]:
        surface = w.surface
        if not fresh:
            self.counts.cached += 1
        out: dict[str, Any] = {"state": state, "line": line, "worker": key, "step": _step_of(key)}
        if surface is not None and surface.frame is not None:
            age = max(0, int((time.monotonic() - surface.at) * 1000))
            out |= {"image": surface.frame, "fresh": fresh, "seq": surface.seq, "age_ms": age}
            if state == "working":
                self.counts.peak(f"max_{surface.kind}_age_ms", age)
        if surface is not None and surface.note:
            out["masking"] = surface.note
        # a problem the surface has stays until its next frame; "too long" belongs to the answer that ran out only
        problem = dict(SLOW) if slow else surface.problem if surface is not None else None
        if problem and not fresh:
            out["problem"] = dict(problem)
        if busy:
            out["busy"] = True
        return out

    def _journal(self, kind: str, surface: _Surface, data: dict[str, Any]) -> None:
        """A preview problem in the journal (never a frame, a title or a URL): the first of its kind per surface kind
        at once, then at most one row per NOTE_EVERY_S with how many there were since. Never raises."""
        key = f"{kind}:{surface.kind}"
        now = time.monotonic()
        due, since = self._noted.get(key, (0.0, 0))
        if now < due:
            self._noted[key] = (due, since + 1)
            return
        self._noted[key] = (now + NOTE_EVERY_S, 0)
        try:
            self._note(kind, {"surface": surface.kind, **data, **({"repeats": since} if since else {})})
        except Exception:
            pass

    def snapshot(self) -> dict[str, float]:
        """The counters, cumulative (resource_samples.work_json takes each window's growth), and the peaks since the
        last read (`max_*`: taken as they are, then started afresh)."""
        c = self.counts
        peaks, c.peaks = c.peaks, {}
        return {"requests": c.requests, "frames": c.frames, "capture_ms": round(c.capture_ms, 1),
                "cached": c.cached, "busy": c.busy,
                **{k: (round(v, 1) if isinstance(v, float) else v) for k, v in sorted(c.kinds.items())},
                **{k: round(v, 1) for k, v in sorted(peaks.items())}}


def _error_name(e: BaseException) -> str:
    """By type, and a capture error's domain and code (its text can hold a window's title)."""
    domain, code = getattr(e, "domain", None), getattr(e, "code", None)
    if isinstance(domain, str) and isinstance(code, int):
        return f"{type(e).__name__} ({domain} {code})"
    return type(e).__name__


def _step_of(worker: str) -> str:
    """The plan step a step's task id names ("<tree>.step.<id>"; a single task's one step is "worker")."""
    rest = worker.split(".", 1)[1] if "." in worker else worker
    return rest.removeprefix("step.")
