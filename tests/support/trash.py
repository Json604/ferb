"""The Trash in offline tests: a folder the test owns, never the owner's real Trash.

FsTrash asks macOS which folder is the Trash for the item's volume (`files.trash_folder`) and moves the item there
itself, between two open folders (`files._move_excl`). Downloads retention still moves by path (`files.move_to_trash`).
`use_trash` points all three at the test's folder, and lets a test act as an attacker at the two moments that matter,
whichever way the item moves: `before_move` (after FERB checked the item, before it moves) and `after_move` (right after
the item lands in the Trash, before FERB looks at what landed)."""
import os
from pathlib import Path
from typing import Any, Callable

import pytest

Hook = Callable[[], None] | None


def use_trash(monkeypatch: pytest.MonkeyPatch, trash: Path, *, before_move: Hook = None,
              after_move: Hook = None) -> None:
    from ferb.act.fs import files
    real_move = getattr(files, "_move_excl", None)
    moves = {"n": 0}

    def landed() -> None:
        moves["n"] += 1
        if moves["n"] == 1 and after_move is not None:     # only the move into the Trash, never a put-back
            after_move()

    def folder(near: Path) -> Path:
        if before_move is not None:
            before_move()
        return trash

    def move(src_dir: int, src: str, dst_dir: int, dst: str) -> None:
        assert real_move is not None
        real_move(src_dir, src, dst_dir, dst)
        landed()

    def by_path(p: Path) -> str:                           # NSFileManager's move, by path
        if before_move is not None:
            before_move()
        dest = trash / p.name
        os.rename(p, dest)
        landed()
        return str(dest)
    monkeypatch.setattr(files, "trash_folder", folder, raising=False)
    monkeypatch.setattr(files, "_move_excl", move, raising=False)
    monkeypatch.setattr(files, "move_to_trash", by_path)


def no_real_trash(monkeypatch: pytest.MonkeyPatch) -> None:
    """An offline test that reaches FsTrash without `use_trash` fails instead of moving a file into the owner's
    Trash."""
    from ferb.act.fs import files

    def refuse(*a: Any, **k: Any) -> Path:
        raise AssertionError("an offline test reached the owner's real Trash; use tests.support.trash.use_trash")
    monkeypatch.setattr(files, "trash_folder", refuse, raising=False)
