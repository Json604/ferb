"""App install / removal (section 8): FSEvents on the app folders; each change rescans that app only (Info.plist),
updates the device profile, publishes `device.changed`, and is recorded in the trail (actor unknown: an installer
can't be attributed)."""

import asyncio
from pathlib import Path
from typing import Any, Callable

from ferb.device.scan import read_app
from ferb.sense.trail import Observation
from ferb.sense.watchers.base import Emit, FsStream
from ferb.sense.watchers.files import change_of

APP_FOLDERS = [Path("/Applications"), Path("/Applications/Utilities"), Path.home() / "Applications"]


class AppInstallsWatcher:
    source = "app_install"

    def __init__(self, on_change: Callable[[str, Any, str], None], folders: list[Path] | None = None,
                 stream: Callable[..., Any] = FsStream) -> None:
        self.on_change = on_change                      # (change, AppInfo | None, path): device profile + bus
        self.folders = [f for f in (folders or APP_FOLDERS) if f.is_dir()]
        self.stream = stream

    def observe(self, path: str, flags: int) -> Observation | None:
        p = Path(path)
        if p.suffix != ".app" or p.parent not in self.folders:
            return None                                  # only top-level app bundles, not files inside them
        change = change_of(path, flags)
        if change in ("created", "renamed_in"):
            info = read_app(p)
            if info is None:
                return None
            return Observation(self.source, {"change": "installed", "app": info.name, "bundle_id": info.bundle_id,
                                             "path": path}, {"actor": "unknown", "app_info": info})
        if change in ("removed", "renamed_out"):
            return Observation(self.source, {"change": "removed", "app": p.stem, "path": path}, {"actor": "unknown"})
        return None

    def deliver(self, emit: Emit, o: Observation) -> None:
        """On the event loop: the device profile + `device.changed` first, then the trail (which may drop it)."""
        self.on_change(str(o.data["change"]), o.hints.get("app_info"), str(o.data["path"]))
        emit(o)

    async def run(self, emit: Emit, stop: asyncio.Event) -> None:
        if not self.folders:
            await stop.wait()
            return
        loop = asyncio.get_running_loop()

        def on_event(path: str, flags: int) -> None:
            o = self.observe(path, flags)
            if o is not None:
                loop.call_soon_threadsafe(self.deliver, emit, o)
        s = self.stream(self.folders, on_event)
        if not s.start():
            await stop.wait()
            return
        try:
            await stop.wait()
        finally:
            s.stop()
