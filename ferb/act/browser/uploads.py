"""Owns the copies of files FERB gives a browser to upload (rule 17, MP2); must not choose a file or change the original.

A browser that reads a file in a folder macOS protects (Desktop, Documents, Downloads, iCloud Drive) makes macOS ask the
owner in front of their work ("Brave Browser would like to access files in your Desktop folder", task 79bc5bfa). FERB
reads the original itself, copies it into its own workspace — an APFS clone where the volume allows it — and gives the
browser the copy under the original's name, since sites show the name. The original is only read. A copy of outside
content (a file under an untrusted fs root) is kept under `outside/`, itself an untrusted root, so it keeps the
original's origin label; any other copy is the owner's file, labeled private like its original. Copies belong to the
task tree and go when it ends; at start-up, copies a restart left behind go too.

FERB's shell can write the workspace, so each folder here is opened without following a link (O_NOFOLLOW) and every
removal goes through the handle of the folder it is in: nothing outside `uploads/` is ever removed. A link in place of
one of FERB's folders is refused when staging and removed, never followed, when cleaning; a link in place of `uploads/`
itself (or, until the next start-up, `outside/`) is left alone and reported, with nothing under it touched."""

import errno
import os
import secrets
import shutil
import stat
from pathlib import Path
from typing import Callable, Iterable

from ferb.act.fs import recovery

FOLDER = "uploads"          # under FERB's workspace
OUTSIDE = "outside"         # copies of outside content: an untrusted fs root (downloads.untrusted_roots adds it)
_OPEN_DIR = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC


class StagingFailed(Exception):
    """No copy for the browser, in words the worker reads. Nothing was given to the browser."""


class _NotOurs(OSError):
    """A link or a file where one of FERB's folders should be: nothing under it is touched."""

    def __init__(self, path: Path) -> None:
        super().__init__(errno.ENOTDIR, "not FERB's folder (a link or a file); nothing under it was touched", str(path))


def _tree_name(tree: str) -> bool:
    return bool(tree) and Path(tree).name == tree and not tree.startswith(".")


def _open(name: str | Path, at: int | None, shown: Path) -> int:
    """A handle on the folder `name` (relative to the folder handle `at`, if given), never through a link at its last
    step. `shown` is its full path, for the error."""
    try:
        return os.open(name, _OPEN_DIR, dir_fd=at)
    except OSError as e:
        if e.errno in (errno.ENOTDIR, errno.ELOOP):     # macOS: ENOTDIR for a link under O_NOFOLLOW | O_DIRECTORY
            raise _NotOurs(shown) from None
        raise


def _made(name: str | Path, at: int | None, shown: Path) -> int:
    """`_open`, the folder made owner-only first when missing."""
    try:
        os.mkdir(name, 0o700, dir_fd=at)
    except FileExistsError:
        pass
    return _open(name, at, shown)


def _existing(name: str | Path, at: int | None, shown: Path) -> int | None:
    """`_open`, or None when the folder is missing."""
    try:
        return _open(name, at, shown)
    except FileNotFoundError:
        return None


def _remove(at: int, name: str) -> int:
    """One entry of a folder FERB holds open, removed through that handle: a folder with all it holds (rmtree on a
    handle checks the entry is no link and never follows one inside), anything else unlinked, a link never followed.
    Returns 1 when something was removed."""
    try:
        mode = os.stat(name, dir_fd=at, follow_symlinks=False).st_mode
    except FileNotFoundError:
        return 0
    if stat.S_ISDIR(mode):
        shutil.rmtree(name, dir_fd=at)
    else:
        os.unlink(name, dir_fd=at)
    return 1


class Uploads:
    def __init__(self, root: Path, untrusted: Callable[[], Iterable[str]]) -> None:
        self.root, self.untrusted = root, untrusted

    def _outside(self, source: Path) -> bool:
        for u in self.untrusted():
            ur = Path(u).expanduser().resolve()
            if source == ur or ur in source.parents:
                return True
        return False

    def stage(self, tree: str, source: str, sha256: str) -> str:
        """Copy `source` (a real path whose identity was just read) to <root>[/outside]/<tree>/<random>/<its name> and
        return the copy's path. The copy must hash to `sha256`, the original's identity; otherwise it is removed and
        refused. A link in place of one of FERB's folders is refused, never followed. Blocking: callers on the event
        loop run it in a thread."""
        if not _tree_name(tree):
            raise StagingFailed("no task owns this tab, so FERB has nowhere to keep its copy")
        src = Path(source)
        parts = [OUTSIDE, tree] if self._outside(src) else [tree]
        self.root.parent.mkdir(parents=True, exist_ok=True)         # FERB's workspace
        held: list[int] = []
        try:
            shown = self.root
            at = _made(self.root, None, shown)
            held.append(at)
            for name in parts:
                shown = shown / name
                at = _made(name, at, shown)
                held.append(at)
            leaf = secrets.token_hex(6)
            os.mkdir(leaf, 0o700, dir_fd=at)                         # a new random leaf: never an existing folder
            copy = shown / leaf / src.name
            try:
                if recovery.copy_file(src, copy) != sha256:
                    raise StagingFailed("the file changed while FERB copied it; inspect it again")
            except BaseException:
                shutil.rmtree(leaf, dir_fd=at, ignore_errors=True)   # only the leaf just made, through its folder
                raise
            return str(copy)
        except _NotOurs as e:
            raise StagingFailed(f"{e.filename} is not FERB's folder (a link or a file); nothing was copied") from None
        finally:
            for fd in held:
                os.close(fd)

    def forget_tree(self, tree: str) -> int:
        """The task tree ended: its copies go (a planted link is removed, never followed). Returns entries removed.
        Raises OSError when `uploads/` or `outside/` is not FERB's folder; nothing under it is touched."""
        if not _tree_name(tree):
            return 0
        root = _existing(self.root, None, self.root)
        if root is None:
            return 0
        try:
            gone = _remove(root, tree)
            outside = _existing(OUTSIDE, root, self.root / OUTSIDE)
            if outside is not None:
                try:
                    gone += _remove(outside, tree)
                finally:
                    os.close(outside)
            return gone
        finally:
            os.close(root)

    def sweep(self) -> int:
        """At start-up no task is running, so every copy left here belongs to none: all go, a planted link included
        (removed, never followed). Returns entries removed. Raises OSError when `uploads/` is not FERB's folder."""
        root = _existing(self.root, None, self.root)
        if root is None:
            return 0
        gone = 0
        try:
            for name in os.listdir(root):
                if name == OUTSIDE and stat.S_ISDIR(os.stat(name, dir_fd=root, follow_symlinks=False).st_mode):
                    outside = _existing(OUTSIDE, root, self.root / OUTSIDE)
                    if outside is None:
                        continue
                    try:
                        for entry in os.listdir(outside):
                            gone += _remove(outside, entry)
                    finally:
                        os.close(outside)
                else:
                    gone += _remove(root, name)
            return gone
        finally:
            os.close(root)
