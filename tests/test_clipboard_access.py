"""I3: macOS 15.4+ pasteboard privacy. A program reading the general pasteboard's contents makes macOS ask the owner
unless the app is set to Allow (AppKit NSPasteboard.h: `accessBehavior`; reading `changeCount` and `types` never asks).
The background clipboard watcher reads contents only when macOS allows it without asking, so FERB never raises a
surprise alert; otherwise it records the access state once and doctor says how to allow it. No real pasteboard is
read: the board is a fake."""
import asyncio
from types import SimpleNamespace
from typing import Any

from ferb.sense.watchers import clipboard as cb


class Board:
    def __init__(self, behavior: int | None) -> None:
        self.behavior, self.count, self.reads = behavior, 1, 0
        if behavior is None:                       # before macOS 15.4: no accessBehavior at all
            self.__dict__["accessBehavior"] = None

    def accessBehavior(self) -> int:
        assert self.behavior is not None
        return self.behavior

    def changeCount(self) -> int:
        return self.count

    def types(self) -> list[str]:
        return ["public.utf8-plain-text"]

    def stringForType_(self, kind: Any) -> str:
        self.reads += 1
        return "copied words"


def _run(board: Board, changes: int, monkeypatch: Any) -> tuple[list[Any], list[str]]:
    monkeypatch.setattr(cb, "focused_app", lambda: SimpleNamespace(bundle_id="com.apple.Safari", name="Safari", pid=9))
    events: list[Any] = []
    reported: list[str] = []
    watcher = cb.ClipboardWatcher(lambda o: o, lambda bundle, name: False, report=reported.append, board=board)
    for _ in range(changes):
        board.count += 1
        watcher.step(events.append)
    return events, reported


# decision: SENSE-404
def test_a_board_that_would_ask_is_never_read(monkeypatch) -> None:
    for behavior, state in ((0, "default"), (1, "ask"), (3, "deny")):
        board = Board(behavior)
        events, reported = _run(board, 3, monkeypatch)
        assert board.reads == 0 and events == [], state             # no surprise alert, nothing recorded
        assert reported == [state]                                   # said once, not on every copy


# decision: SENSE-404
def test_a_board_set_to_allow_is_read(monkeypatch) -> None:
    board = Board(2)
    events, reported = _run(board, 2, monkeypatch)
    assert board.reads == 2 and len(events) == 2 and reported == ["allow"]


# decision: SENSE-404
def test_before_macos_15_4_the_board_is_read_as_before(monkeypatch) -> None:
    board = Board(None)
    events, reported = _run(board, 1, monkeypatch)
    assert board.reads == 1 and reported == ["unrestricted"]


# decision: SENSE-404
def test_access_names_every_documented_behavior() -> None:
    assert [cb.access(Board(n)) for n in (0, 1, 2, 3)] == ["default", "ask", "allow", "deny"]
    assert cb.access(Board(None)) == "unrestricted"


# decision: SENSE-404
def test_the_one_requested_read_happens_only_when_asked(monkeypatch) -> None:
    board = Board(0)
    assert cb.request_access(board) == "default"                    # a fake board never changes; real macOS asks here
    assert board.reads == 1
    allowed = Board(2)
    assert cb.request_access(allowed) == "allow" and allowed.reads == 0     # nothing to ask
    assert asyncio                                                   # (imported for parity with other watcher tests)
