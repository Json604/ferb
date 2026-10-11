"""NSWorkspace lifecycle notifications for every application. No application is launched by this source."""
import asyncio
import threading
from typing import Any, Callable

from ferb.sense.trail import Observation
from ferb.sense.watchers.base import Emit, run_loop_slice


class WorkspaceWatcher:
    source = "workspace"

    def __init__(self, redact: Callable[[Any], Any], excluded: Callable[[str, str], bool]) -> None:
        self.redact, self.excluded = redact, excluded

    async def run(self, emit: Emit, stop: asyncio.Event) -> None:
        loop, done = asyncio.get_running_loop(), threading.Event()
        failure: list[Exception] = []

        def native_body() -> None:
            import AppKit
            center = AppKit.NSWorkspace.sharedWorkspace().notificationCenter()
            tokens = []

            def observed(note: Any) -> None:
                app = (note.userInfo() or {}).get(AppKit.NSWorkspaceApplicationKey)
                data: dict[str, Any] = {"event": str(note.name())}
                if app is not None:
                    bundle, name = str(app.bundleIdentifier() or ""), str(app.localizedName() or "")
                    if self.excluded(bundle, name):
                        return
                    data |= {"bundle_id": bundle, "app": name, "pid": int(app.processIdentifier())}
                if not done.is_set():
                    loop.call_soon_threadsafe(emit, Observation(self.source, self.redact(data), {"actor": "unknown"}))
            try:
                for name in ("NSWorkspaceDidLaunchApplicationNotification", "NSWorkspaceDidTerminateApplicationNotification",
                             "NSWorkspaceDidActivateApplicationNotification", "NSWorkspaceDidDeactivateApplicationNotification",
                             "NSWorkspaceWillSleepNotification", "NSWorkspaceDidWakeNotification",
                             "NSWorkspaceDidMountNotification", "NSWorkspaceDidUnmountNotification"):
                    tokens.append(center.addObserverForName_object_queue_usingBlock_(getattr(AppKit, name), None, None, observed))
                while not done.is_set():
                    run_loop_slice(done, 0.5)
            except Exception as error:
                failure.append(error)
            finally:
                for token in tokens:
                    center.removeObserver_(token)
        def native() -> None:
            try:
                native_body()
            except Exception as error:
                failure.append(error)

        thread = threading.Thread(target=native, name="ferb-workspace", daemon=True)
        thread.start()
        try:
            while not stop.is_set():
                await asyncio.sleep(0.1)
                if failure:
                    raise RuntimeError(f"Workspace source failed: {type(failure[0]).__name__}")
        finally:
            done.set()
            await asyncio.to_thread(thread.join, 2)
