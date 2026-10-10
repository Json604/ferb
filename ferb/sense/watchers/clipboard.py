"""Read changed general-pasteboard text passively; private/transient markers are checked before text access."""
import asyncio
from typing import Any, Callable

from ferb.sense.capture import clipboard_observation, clipboard_write
from ferb.sense.watchers.base import Emit, focused_app, poll


class ClipboardWatcher:
    source = "clipboard"

    def __init__(self, redact: Callable[[Any], Any], excluded: Callable[[str, str], bool], interval_s: float = 0.5) -> None:
        self.redact, self.excluded, self.interval_s = redact, excluded, interval_s

    async def run(self, emit: Emit, stop: asyncio.Event) -> None:
        from AppKit import NSPasteboard, NSPasteboardTypeString
        loop = asyncio.get_running_loop()
        board = NSPasteboard.generalPasteboard()
        last = int(board.changeCount())        # do not recapture old clipboard at startup/resume

        def step() -> None:
            nonlocal last
            count = int(board.changeCount())
            if count == last:
                return
            last = count
            app = focused_app()
            if app is None or self.excluded(app.bundle_id, app.name):
                return
            types = {str(t) for t in (board.types() or [])}
            marker = clipboard_write()
            event = clipboard_observation(count, types, lambda: board.stringForType_(NSPasteboardTypeString),
                                           self.redact, ferb_count=marker[0] if marker else None,
                                           task_id=marker[1] if marker else None,
                                           context={"bundle_id": app.bundle_id, "app": app.name, "pid": app.pid,
                                                    "app_context": "frontmost; not proof of writer"})
            if event is not None and int(board.changeCount()) == count:
                # Frontmost is context, not proof of who copied it (other apps can write asynchronously).
                loop.call_soon_threadsafe(emit, event)
        await poll(self.interval_s, stop, step)
