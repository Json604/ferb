"""Read changed general-pasteboard text passively; private/transient markers are checked before text access.

macOS 15.4+ pasteboard privacy (I3; AppKit NSPasteboard.h `accessBehavior`): a program that reads the general
pasteboard's contents makes macOS ask the owner unless the app is set to Allow in System Settings → Privacy & Security
→ Paste from Other Apps. An app that never asked reports `default` and isn't listed there until it first does. Reading
`changeCount` and `types` never asks. This listen-only watcher reads contents only when macOS allows it without asking,
so it never raises a surprise alert; otherwise it reports the access state once (journaled, and `ferb doctor` says how
to allow it). The one deliberate request is `request_access`, which only `ferb setup` triggers, with the owner there."""
import asyncio
from typing import Any, Callable

from ferb.sense.capture import clipboard_observation, clipboard_write
from ferb.sense.watchers.base import Emit, focused_app, poll

# NSPasteboardAccessBehavior (macOS 15.4+), by value
BEHAVIOR = {0: "default", 1: "ask", 2: "allow", 3: "deny"}
READABLE = frozenset({"allow", "unrestricted"})      # read without macOS asking the owner


def access(board: Any) -> str:
    """How macOS treats this process reading the general pasteboard: default, ask, allow or deny; `unrestricted`
    before macOS 15.4 (no pasteboard privacy)."""
    behavior = getattr(board, "accessBehavior", None)
    if not callable(behavior):
        return "unrestricted"
    return BEHAVIOR.get(int(behavior()), "unknown")


def general_board() -> Any:
    from AppKit import NSPasteboard
    return NSPasteboard.generalPasteboard()


def request_access(board: Any = None) -> str:
    """`ferb setup` only, with the owner there: one read of the general pasteboard by this process, so macOS shows
    its "paste from other apps" alert now (and lists FERB in Paste from Other Apps) rather than as a surprise later.
    Nothing read is kept. Returns the access state afterwards."""
    from AppKit import NSPasteboardTypeString
    board = board if board is not None else general_board()
    if access(board) in ("default", "ask"):
        board.stringForType_(NSPasteboardTypeString)
    return access(board)


class ClipboardWatcher:
    source = "clipboard"

    def __init__(self, redact: Callable[[Any], Any], excluded: Callable[[str, str], bool], interval_s: float = 0.5,
                 report: Callable[[str], None] = lambda state: None, board: Any = None) -> None:
        self.redact, self.excluded, self.interval_s, self.report = redact, excluded, interval_s, report
        self.board = board
        # do not recapture the old clipboard at startup/resume: the count when the board is first known
        self.last: int | None = int(board.changeCount()) if board is not None else None
        self.state: str | None = None                 # the access state last reported

    def step(self, emit: Callable[[Any], None], report: Callable[[str], None] | None = None) -> None:
        from AppKit import NSPasteboardTypeString
        board = self.board if self.board is not None else general_board()
        count = int(board.changeCount())
        if self.last is None:
            self.last = count
        if count == self.last:
            return
        self.last = count
        state = access(board)
        if state != self.state:
            self.state = state
            (report or self.report)(state)
        if state not in READABLE:                     # reading would make macOS ask, or is denied: never read
            return
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
            emit(event)

    async def run(self, emit: Emit, stop: asyncio.Event) -> None:
        loop = asyncio.get_running_loop()
        board = self.board if self.board is not None else general_board()
        self.board, self.last = board, int(board.changeCount())
        await poll(self.interval_s, stop, lambda: self.step(
            lambda event: loop.call_soon_threadsafe(emit, event),
            lambda state: loop.call_soon_threadsafe(self.report, state)))
