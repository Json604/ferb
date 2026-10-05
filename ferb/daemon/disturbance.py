"""Observe desktop changes throughout tasks; never activate, restore, block or infer authorization.

The harness records action timing and owner input beside changes. Temporal correlation cannot
identify a unique cause when the owner and several tasks act concurrently. No window titles,
content, pixels or keystrokes are collected. Public Space notifications report transitions, not IDs.
"""

import asyncio
import copy
import json
import time
from dataclasses import dataclass, field
from typing import Any, Callable


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
        import Quartz as Q
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
            result["frontmost_app"] = {"pid": int(pid), "bundle_id": str(running.bundleIdentifier() or "")
                                       if running else None}
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
        try:
            location = Q.CGEventGetLocation(Q.CGEventCreate(None))
            result["cursor"] = [float(location.x), float(location.y)]
        except Exception as exc:
            unavailable["cursor"] = str(exc)
        try:
            windows = Q.CGWindowListCopyWindowInfo(Q.kCGWindowListOptionOnScreenOnly, Q.kCGNullWindowID)
            if windows is None:
                raise RuntimeError("Window server unavailable")
            result["visible_window_order"] = [
                {"id": int(w[Q.kCGWindowNumber]), "pid": int(w[Q.kCGWindowOwnerPID]),
                 "layer": int(w[Q.kCGWindowLayer])}
                for w in windows if float(w.get(Q.kCGWindowAlpha, 1)) > 0]
        except Exception as exc:
            unavailable["visible_window_order"] = str(exc)
        return result


@dataclass
class _Observation:
    last: dict[str, Any] | None = None
    samples: int = 0
    changes: int = 0
    errors: int = 0
    unavailable: dict[str, str] = field(default_factory=dict)
    max_sample_ms: float = 0


class DisturbanceSentinel:
    """One sampler shared by all active tasks, plus samples immediately around each tool action."""

    def __init__(self, read: Callable[[], dict[str, Any]], record: Callable[[str, str, dict[str, Any]], None], *,
                 interval_s: float = 0.25, owner_input: Callable[[], dict[str, Any]] = dict) -> None:
        if interval_s <= 0:
            raise ValueError("sample interval must be positive")
        self.read, self.record, self.interval_s, self.owner_input = read, record, interval_s, owner_input
        self.active: dict[str, _Observation] = {}
        self._actions: dict[str, dict[str, Any]] = {}
        self._recent: dict[str, dict[str, Any]] = {}
        self._sampler: asyncio.Task[None] | None = None
        self._lock = asyncio.Lock()

    async def start(self, task_id: str) -> None:
        if task_id in self.active:
            return
        if not self.active and hasattr(self.read, "start"):
            self.read.start()
        self.active[task_id] = _Observation()
        await self.sample()
        if self._sampler is None:
            self._sampler = asyncio.create_task(self._run())

    async def finish(self, task_id: str) -> None:
        if task_id not in self.active:
            return
        await self.sample()
        observed = self.active.pop(task_id)
        self.record(task_id, "disturbance.summary", {
            "samples": observed.samples, "changes": observed.changes, "errors": observed.errors,
            "unavailable": observed.unavailable, "sample_interval_s": self.interval_s,
            "max_sample_ms": observed.max_sample_ms,
            "coverage": "sampled; short changes between samples may be missed; Space transitions use notifications",
            "permission_assessment": "not_inferred", "attribution": "temporal_correlation_only"})
        if not self.active:
            if self._sampler is not None:
                self._sampler.cancel()
                try:
                    await self._sampler
                except asyncio.CancelledError:
                    pass
                self._sampler = None
            if hasattr(self.read, "close"):
                self.read.close()
            self._recent.clear()

    async def shutdown(self) -> None:
        for task_id in list(self.active):
            await self.finish(task_id)

    async def before_action(self, task_id: str, span_id: str, tool: str, args: dict[str, Any]) -> None:
        # The first sample separates changes before this action from changes while it runs.
        await self.sample()
        self._actions[span_id] = {"task_id": task_id, "span_id": span_id, "tool": tool,
                                  "foreground_argument": args.get("allow_foreground"),
                                  "started_monotonic": time.monotonic(), "state": "in_flight"}

    async def after_action(self, span_id: str) -> None:
        try:
            await self.sample()
        finally:
            action = self._actions.pop(span_id, None)
            if action:
                self._recent[span_id] = dict(action, state="recently_ended", ended_monotonic=time.monotonic())

    async def _run(self) -> None:
        while self.active:
            await asyncio.sleep(self.interval_s)
            await self.sample()

    async def sample(self) -> None:
        if not self.active:
            return
        async with self._lock:
            started = time.monotonic()
            try:
                if hasattr(self.read, "pump"):
                    self.read.pump()
                snapshot = copy.deepcopy(await asyncio.to_thread(self.read))
                sample_ms = (time.monotonic() - started) * 1000
                owner = self.owner_input()
            except Exception as exc:
                for task_id, observed in self.active.items():
                    observed.errors += 1
                    self.record(task_id, "disturbance.error", {"error": f"{type(exc).__name__}: {exc}"})
                return
            self._recent = {key: action for key, action in self._recent.items()
                            if started - action["ended_monotonic"] <= self.interval_s * 2}
            actions = list(self._actions.values()) + list(self._recent.values())
            for task_id, observed in self.active.items():
                observed.samples += 1
                observed.max_sample_ms = max(observed.max_sample_ms, sample_ms)
                observed.unavailable.update(snapshot.get("unavailable", {}))
                if observed.last is None:
                    self.record(task_id, "disturbance.baseline", {"snapshot": snapshot,
                                "sample_interval_s": self.interval_s, "space_identity": "transition_epoch_only"})
                else:
                    changes = {key: {"before": observed.last.get(key), "after": snapshot.get(key)}
                               for key in observed.last.keys() | snapshot.keys()
                               if observed.last.get(key) != snapshot.get(key)}
                    if changes:
                        observed.changes += 1
                        self.record(task_id, "disturbance.change", {
                            "changes": changes, "actions": actions, "owner_input": owner,
                            "sample_duration_ms": sample_ms, "permission_assessment": "not_inferred",
                            "attribution": "temporal_correlation_only"})
                observed.last = snapshot

    def context(self, task_id: str) -> str:
        observed = self.active.get(task_id.split(".", 1)[0])
        if observed is None:
            return ""
        return "\nDesktop observations (facts, not inferred permission or causation): " + json.dumps({
            "samples": observed.samples, "changes": observed.changes, "errors": observed.errors,
            "unavailable": observed.unavailable,
            "read_full": "task.evidence query=disturbance on this task root"})
