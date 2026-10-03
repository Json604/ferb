"""File events (section 11) in the folders the owner lists in `[sense] watch_folders` only (empty by default): path and
kind of change, never contents. A folder must exist, be a directory under the owner's home, and not be (or be
inside) an excluded path; others are skipped and reported. Excluded paths inside a watched folder are dropped by the
pipeline. `.DS_Store` and `.git` internals are noise and ignored. Repeats for one path within a second coalesce.
"""

import asyncio
import os
import time
from pathlib import Path
from typing import Any, Callable

from ferb.sense.exclusions import Exclusions
from ferb.sense.trail import Observation
from ferb.sense.watchers.base import CREATED, IS_DIR, MODIFIED, REMOVED, RENAMED, Emit, FsStream

COALESCE_S = 1.0


def valid_folders(folders: list[str], exclusions: Exclusions, home: Path | None = None) -> tuple[list[Path], list[str]]:
    """(folders to watch, problems with the others)."""
    home = (home or Path.home()).resolve()
    ok, problems = [], []
    for f in folders:
        p = Path(str(f)).expanduser()
        try:
            p = p.resolve(strict=True)
        except OSError:
            problems.append(f"{f}: doesn't exist")
            continue
        if not p.is_dir():
            problems.append(f"{f}: not a folder")
        elif p != home and home not in p.parents:
            problems.append(f"{f}: not inside your home folder")
        elif exclusions.path(p):
            problems.append(f"{f}: excluded ([privacy] exclude_paths)")
        else:
            ok.append(p)
    return ok, problems


def change_of(path: str, flags: int) -> str | None:
    if flags & REMOVED and not os.path.lexists(path):
        return "removed"
    if flags & RENAMED:
        return "renamed_in" if os.path.lexists(path) else "renamed_out"
    if flags & CREATED:
        return "created"
    if flags & MODIFIED:
        return "modified"
    return None


def noise(path: str) -> bool:
    parts = Path(path).parts
    return parts[-1] == ".DS_Store" or ".git" in parts[:-1] or parts[-1] == ".git"


class FilesWatcher:
    source = "file"

    def __init__(self, folders: Callable[[], list[Path]], stream: Callable[..., Any] = FsStream,
                 now: Callable[[], float] = time.monotonic) -> None:
        self.folders, self.stream, self.now = folders, stream, now
        self._recent: dict[tuple[str, str], float] = {}
        self._known: set[str] = set()                   # paths seen existing (bounded below)

    def observe(self, path: str, flags: int) -> Observation | None:
        if noise(path):
            return None
        change = change_of(path, flags)
        if change is None:
            return None
        if change == "created" and path in self._known:
            change = "modified"                         # FSEvents keeps a file's flags cumulative: it existed already
        if change in ("removed", "renamed_out"):
            self._known.discard(path)
        else:
            self._known.add(path)
        key, t = (path, change), self.now()
        if t - self._recent.get(key, -COALESCE_S * 2) < COALESCE_S:
            return None
        self._recent[key] = t
        if len(self._recent) > 5000:
            self._recent = {k: v for k, v in self._recent.items() if t - v < COALESCE_S}
        if len(self._known) > 20000:
            self._known.clear()
        return Observation(self.source, {"path": path, "change": change, "is_dir": bool(flags & IS_DIR)})

    async def run(self, emit: Emit, stop: asyncio.Event) -> None:
        folders = self.folders()
        if not folders:
            await stop.wait()
            return
        loop = asyncio.get_running_loop()

        def on_event(path: str, flags: int) -> None:
            o = self.observe(path, flags)
            if o is not None:
                loop.call_soon_threadsafe(emit, o)
        s = self.stream(folders, on_event)
        try:
            if not s.start():
                raise RuntimeError('file event stream failed to start')
            await stop.wait()
        finally:
            s.stop()
