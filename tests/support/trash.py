"""The Trash in offline tests: a folder the test owns, never the owner's real Trash.

Every move to the Trash (fs.trash and downloads retention) asks macOS which folder is the Trash for the item's volume
(`files.trash_folder`) and moves the item there itself, between two open folders (`files.move_excl`). `use_trash`
points both at the test's folder, and lets a test act as an attacker at the two moments that matter: `before_move`
(after FERB checked the item, before it moves) and `after_move` (right after the item lands in the Trash, before FERB
looks at what landed)."""
from pathlib import Path
from typing import Any, Callable

import pytest

Hook = Callable[[], None] | None


def use_trash(monkeypatch: pytest.MonkeyPatch, trash: Path, *, before_move: Hook = None,
              after_move: Hook = None) -> None:
    from ferb.act.fs import files
    real_move = files.move_excl
    moves = {"n": 0}

    def folder(near: Path) -> Path:
        if before_move is not None:
            before_move()
        return trash

    def move(src_dir: int, src: str, dst_dir: int, dst: str) -> None:
        real_move(src_dir, src, dst_dir, dst)
        moves["n"] += 1
        if moves["n"] == 1 and after_move is not None:   # only the first move: the item landing in the Trash
            after_move()
    monkeypatch.setattr(files, "trash_folder", folder)
    monkeypatch.setattr(files, "move_excl", move)


def no_real_trash(monkeypatch: pytest.MonkeyPatch) -> None:
    """An offline test that reaches a move to the Trash without `use_trash` fails instead of moving a file into the
    owner's Trash; nothing reaches NSFileManager's Trash lookup either."""
    from ferb.act.fs import files

    def refuse(*a: Any, **k: Any) -> Any:
        raise AssertionError("an offline test reached the owner's real Trash; use tests.support.trash.use_trash")
    monkeypatch.setattr(files, "_trash_url", refuse)            # trash_folder's only way to the real Trash
