"""Observe desktop changes FERB could have caused; never activate, restore, block or infer authorization.

Daemon decision 5 (owner 2026-10-30): the desktop is sampled only right before and right after a tool call that can
change the owner's screen (its capability declares `on_screen`: Mac input, foreground, opening an app), never around
read-only tools and never on a timer, plus one baseline when a task starts. Only what FERB could have changed is
compared and recorded: the front app, the focused window, fullscreen and the Space (FERB_KEYS); the owner's cursor and
window order are not read. What changed during one call is returned to the gateway as that call's facts (the merged
checker reads them, notes/orchestration.md decision 9); a change found when the next on-screen call starts, with no
FERB on-screen call in flight, is recorded as that fact, with when the last one ended and the processes FERB started
that were running (P26). Who did it stays the reader's conclusion (rule 6). No window titles, content, pixels or
keystrokes are collected. Public Space notifications report transitions, not IDs.
While the owner has paused sensing (`ferb pause`), nothing about the desktop is recorded; a read already under way
when the pause begins is dropped. Space switches may still be counted in memory meanwhile; the baseline taken after
`ferb resume` discards them.
"""

import asyncio
import copy
import json
import math
import time
from collections import deque
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any, Callable

from ferb.trace import lines

NO_ON_SCREEN_KEPT = 10   # the newest plain statements an agent's context carries; every change stays in the records
# What FERB could change on the owner's screen, and all that is compared (daemon decision 5): never the cursor
FERB_KEYS = ("frontmost_app", "key_window", "fullscreen", "active_space_epoch")
PAUSED = "sensing paused by the owner (Pause FERB): nothing about the desktop is read or recorded until Resume"


class DesktopProbe:
    """Read public macOS desktop facts; unavailable is distinct from unchanged or false."""

    def __init__(self) -> None:
        self.space_epoch = 0
        self._center: Any = None
        self._observer: Any = None
        self._space_error: str | None = None

    def start(self) -> None:
        try:
            from AppKit import NSWorkspace, NSWorkspaceActiveSpaceDidChangeNotification
            self._center = NSWorkspace.sharedWorkspace().notificationCenter()
            self._observer = self._center.addObserverForName_object_queue_usingBlock_(
                NSWorkspaceActiveSpaceDidChangeNotification, None, None, self._space_changed)
        except Exception as exc:
            self._space_error = f"{type(exc).__name__}: {exc}"

    def _space_changed(self, notification: Any) -> None:
        self.space_epoch += 1

    def pump(self) -> None:
        # NSWorkspace notifications require a serviced run loop on the registration thread.
        try:
            import CoreFoundation as CF
            CF.CFRunLoopRunInMode(CF.kCFRunLoopDefaultMode, 0, True)
        except Exception as exc:
            self._space_error = f"{type(exc).__name__}: {exc}"

    def close(self) -> None:
        if self._center is not None and self._observer is not None:
            self._center.removeObserver_(self._observer)
        self._center = self._observer = None

    def __call__(self) -> dict[str, Any]:
        import ApplicationServices as AX
        import CoreFoundation as CF
        from AppKit import NSRunningApplication

        unavailable: dict[str, str] = {}
        result: dict[str, Any] = {"unavailable": unavailable}
        if self._observer is not None and self._space_error is None:
            result["active_space_epoch"] = self.space_epoch
        else:
            unavailable["active_space_epoch"] = self._space_error or "Space observer not registered"

        def attr(element: Any, name: str) -> Any:
            error, value = AX.AXUIElementCopyAttributeValue(element, name, None)
            if error:
                raise RuntimeError(f"{name}: AX error {error}")
            return value

        try:
            app = attr(AX.AXUIElementCreateSystemWide(), "AXFocusedApplication")
            error, pid = AX.AXUIElementGetPid(app, None)
            if error:
                raise RuntimeError(f"AX pid error {error}")
            running = NSRunningApplication.runningApplicationWithProcessIdentifier_(pid)
            result["frontmost_app"] = {"pid": int(pid),
                                       "bundle_id": str(running.bundleIdentifier() or "") if running else None,
                                       "name": str(running.localizedName() or "") if running else None}
            try:
                window = attr(app, "AXFocusedWindow")
                result["key_window"] = {"pid": int(pid), "ax_identity": int(CF.CFHash(window))} if window else None
                try:
                    result["fullscreen"] = bool(attr(window, "AXFullScreen")) if window else None
                except Exception as exc:
                    unavailable["fullscreen"] = str(exc)
            except Exception as exc:
                unavailable["key_window"] = str(exc)
                unavailable["fullscreen"] = "No readable focused window"
        except Exception as exc:
            unavailable.update({field: str(exc) for field in ("frontmost_app", "key_window", "fullscreen")})
        return result


@dataclass
class _Observation:
    last: dict[str, Any] | None = None
    samples: int = 0
    changes: int = 0
    errors: int = 0
    unavailable: dict[str, str] = field(default_factory=dict)
    max_sample_ms: float = 0
    on_screen_calls: int = 0                      # this task's FERB calls that can change the screen, so far
    screen_changes: list[dict[str, Any]] = field(default_factory=list)   # what changed during this task's calls
    no_on_screen_action: deque[str] = field(default_factory=lambda: deque(maxlen=NO_ON_SCREEN_KEPT))
    no_on_screen_action_count: int = 0
    forward: lines.CameForward = field(default_factory=lines.CameForward)
    paused: bool = False


def screen_changes(before: dict[str, Any] | None, after: dict[str, Any] | None) -> dict[str, Any]:
    """What FERB could have changed between two samples (FERB_KEYS): {key: {before, after}}; a key unreadable in either
    sample is left out (unknown is not changed)."""
    if not before or not after:
        return {}
    unread = set(before.get("unavailable") or {}) | set(after.get("unavailable") or {})
    return {k: {"before": before.get(k), "after": after.get(k)} for k in FERB_KEYS
            if k not in unread and before.get(k) != after.get(k)}


class DisturbanceSentinel:
    """One sampler shared by all active tasks: a baseline when a task starts, then samples right around each call that
    can change the screen (`before_action` / `after_action` with on_screen), nothing else."""

    def __init__(self, read: Callable[[], dict[str, Any]], record: Callable[[str, str, dict[str, Any]], None], *,
                 owner_input: Callable[[], dict[str, Any]] = dict,
                 processes: Callable[[], list[dict[str, Any]]] = list,
                 now: Callable[[], datetime] = lambda: datetime.now().astimezone(),
                 paused: Callable[[], bool] = lambda: False) -> None:
        self.read, self.record, self.owner_input = read, record, owner_input
        self.processes, self.now, self.paused = processes, now, paused
        self._on_screen_ended: datetime | None = None   # when the latest on-screen FERB call of any task ended
        self.active: dict[str, _Observation] = {}
        self._actions: dict[str, dict[str, Any]] = {}
        self._lock = asyncio.Lock()

    async def start(self, task_id: str) -> None:
        if task_id in self.active:
            return
        first = not self.active
        self.active[task_id] = _Observation()           # registered and the probe started with no await between, so a
        if first and hasattr(self.read, "start"):       # finishing tree can't close the probe under a starting one
            self.read.start()
        await self.sample()                             # the task's baseline

    async def finish(self, task_id: str) -> None:
        observed = self.active.pop(task_id, None)
        if observed is None:                      # finished meanwhile: the tree's own end and shutdown's both ran
            return
        self.record(task_id, "disturbance.summary", {
            "samples": observed.samples, "changes": observed.changes, "errors": observed.errors,
            "unavailable": observed.unavailable, "max_sample_ms": observed.max_sample_ms,
            "on_screen_calls": observed.on_screen_calls, "screen_changes": observed.screen_changes,
            "coverage": "sampled right before and after FERB's calls that can change the screen; front app, focused "
                        "window, fullscreen and Space only; Space transitions use notifications",
            "permission_assessment": "not_inferred", "attribution": "temporal_correlation_only"})
        if not self.active:                       # no await since the pop: nothing started in between
            if hasattr(self.read, "close"):
                self.read.close()

    async def shutdown(self) -> None:
        for task_id in list(self.active):
            await self.finish(task_id)

    async def before_action(self, task_id: str, span_id: str, tool: str, args: dict[str, Any],
                            on_screen: bool = False) -> None:
        """A call is about to run: only one that can change the screen is sampled (the sample also records what
        changed since the last one, with no FERB on-screen call in flight)."""
        if not on_screen:
            return
        before = await self.sample()
        if (observed := self.active.get(task_id.split(".", 1)[0])) is not None:
            observed.on_screen_calls += 1
        self._actions[span_id] = {"task_id": task_id, "span_id": span_id, "tool": tool, "on_screen": True,
                                  "foreground_argument": args.get("allow_foreground"),
                                  "started_monotonic": time.monotonic(), "state": "in_flight", "_before": before}

    async def after_action(self, span_id: str) -> dict[str, Any]:
        """The call ended: what FERB could have changed during it, as the call's facts ({} when nothing, or when it
        wasn't sampled). Idempotent per call."""
        action = self._actions.get(span_id)
        if action is None:
            return {}
        try:
            after = await self.sample()
        finally:
            self._actions.pop(span_id, None)
            self._on_screen_ended = self.now()
        changed = screen_changes(action.get("_before"), after)
        if changed and (observed := self.active.get(str(action["task_id"]).split(".", 1)[0])) is not None:
            observed.screen_changes.append({"tool": action["tool"], "span_id": span_id, "changes": changed})
        return changed

    def facts(self, task_id: str) -> list[dict[str, Any]]:
        """What FERB's calls in this task changed on the screen so far (for the merged checker, step 8)."""
        observed = self.active.get(task_id.split(".", 1)[0])
        return list(observed.screen_changes) if observed is not None else []

    async def sample(self) -> dict[str, Any] | None:
        """One read, compared for every active task; returns it (None when paused, failed or no task is active)."""
        if not self.active:
            return None
        async with self._lock:
            if self._held():
                return None
            started = time.monotonic()
            try:
                if hasattr(self.read, "pump"):
                    self.read.pump()
                snapshot = copy.deepcopy(await asyncio.to_thread(self.read))
                sample_ms = (time.monotonic() - started) * 1000
                owner = self.owner_input()
            except Exception as exc:
                if self._held():                  # paused while reading: the sample never happened
                    return None
                for task_id, observed in self.active.items():
                    observed.errors += 1
                    self.record(task_id, "disturbance.error", {"error": f"{type(exc).__name__}: {exc}"})
                return None
            if self._held():                      # paused while reading: drop what was read, like the trail does
                return None
            snapshot = {k: v for k, v in snapshot.items() if k in FERB_KEYS or k == "unavailable"}
            snapshot["unavailable"] = {k: v for k, v in (snapshot.get("unavailable") or {}).items() if k in FERB_KEYS}
            actions = [{k: v for k, v in a.items() if not k.startswith("_")} for a in self._actions.values()]
            near = [a for a in actions if a.get("on_screen")]   # any task's: another task's call can move the desktop
            procs: list[dict[str, Any]] | None = None
            listed = False
            for task_id, observed in self.active.items():
                observed.samples += 1
                observed.max_sample_ms = max(observed.max_sample_ms, sample_ms)
                observed.unavailable.update(snapshot.get("unavailable", {}))
                if observed.last is None:
                    observed.forward.baseline(snapshot)
                    self.record(task_id, "disturbance.baseline", {"snapshot": snapshot,
                                                                  "space_identity": "transition_epoch_only"})
                else:
                    changes = {key: {"before": observed.last.get(key), "after": snapshot.get(key)}
                               for key in FERB_KEYS if observed.last.get(key) != snapshot.get(key)}
                    if changes:
                        observed.changes += 1
                        data: dict[str, Any] = {
                            "changes": changes, "actions": actions, "owner_input": owner,
                            "sample_duration_ms": sample_ms, "permission_assessment": "not_inferred",
                            "attribution": "temporal_correlation_only"}
                        if "frontmost_app" in changes and lines.front_pid(snapshot.get("frontmost_app")) is None:
                            data["front_app_unreadable"] = True     # raw record only: nothing is stated about it
                        what = observed.forward.change(changes)
                        if what:                  # a readable app came forward, or the Space changed
                            if not listed:
                                procs, listed = self._listed(), True
                            data["processes"] = _for_tree(procs, task_id)
                            if not near:          # no FERB call that can change the screen in flight
                                self._no_on_screen_action(task_id, observed, what, data)
                        self.record(task_id, "disturbance.change", data)
                observed.last = snapshot
            return snapshot

    def _held(self) -> bool:
        """While the owner has paused sensing: each task's record says so once, nothing is read, and the next sample
        after `ferb resume` starts from a fresh baseline (what changed meanwhile was never observed)."""
        if not self.paused():
            for task_id, observed in self.active.items():
                if observed.paused:
                    observed.paused = False
                    self.record(task_id, "disturbance.resumed", {})
            return False
        for task_id, observed in self.active.items():
            if not observed.paused:
                observed.paused, observed.last, observed.forward = True, None, lines.CameForward()
                self.record(task_id, "disturbance.paused", {"reason": PAUSED})
        return True

    def _listed(self) -> list[dict[str, Any]] | None:
        """The processes FERB started that are running now; None when they couldn't be listed."""
        try:
            return list(self.processes())
        except Exception:
            return None

    def _no_on_screen_action(self, task_id: str, observed: _Observation, what: str, data: dict[str, Any]) -> None:
        """The facts of a change no FERB on-screen call was near: whether this task made one, the time since the latest
        one of any task ended (controller ruling 2026-10-09: a FERB effect can land after the call; one decimal under
        10 s), and the plain statement agents read. States facts, never who caused it (rule 6)."""
        now = self.now()
        ended = self._on_screen_ended
        data["on_screen_in_task"] = observed.on_screen_calls > 0
        data["last_on_screen_ended_s"] = None if ended is None else _elapsed((now - ended).total_seconds())
        observed.no_on_screen_action_count += 1
        observed.no_on_screen_action.append(f"{now.astimezone():%H:%M:%S} " + lines.no_on_screen_action(
            what, data["processes"], task_id, in_task=data["on_screen_in_task"],
            last_ended_s=data["last_on_screen_ended_s"]))

    def context(self, task_id: str) -> str:
        observed = self.active.get(task_id.split(".", 1)[0])
        if observed is None or self.paused():         # paused: nothing about the desktop reaches an agent
            return ""
        return ("\nDesktop observations (facts, not inferred permission or causation; FERB's actions that can change "
                "the screen are marked on_screen): " + json.dumps({
                    "samples": observed.samples, "changes": observed.changes, "errors": observed.errors,
                    "unavailable": observed.unavailable,
                    "front_or_space_changes_with_no_ferb_on_screen_action": observed.no_on_screen_action_count,
                    "latest": list(observed.no_on_screen_action),
                    "read_full": "task.evidence query=disturbance on this task root"}, ensure_ascii=False))


def _for_tree(processes: list[dict[str, Any]] | None, tree: str) -> list[dict[str, Any]] | None:
    """FERB-started processes as one task tree's record keeps them: another tree's lose their command text, which was
    never this tree's to read and would reach its models labelled as FERB's (MP2, S3)."""
    if processes is None:
        return None
    return [p if str(p.get("task", "")).split(".", 1)[0] == tree else {k: v for k, v in p.items() if k != "command"}
            for p in processes]


def _elapsed(s: float) -> float:
    """Seconds since an on-screen call ended: tenths under 10 s (rounded down, so a gap never reads longer than it
    was), whole seconds from 10 s."""
    s = max(0.0, s)
    return float(int(s)) if s >= 10 else math.floor(s * 10) / 10
