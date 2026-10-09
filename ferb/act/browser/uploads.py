"""Owns the copies of files FERB gives a browser to upload (rule 17, MP2); must not choose a file or change the original.

A browser that reads a file in a folder macOS protects (Desktop, Documents, Downloads, iCloud Drive) makes macOS ask the
owner in front of their work ("Brave Browser would like to access files in your Desktop folder", task 79bc5bfa). FERB
reads the original itself, copies it into its own workspace — an APFS clone where the volume allows it — and gives the
browser the copy under the original's name, since sites show the name. The original is only read. A copy of outside
content (a file under an untrusted fs root) is kept under `outside/`, itself an untrusted root, so it keeps the
original's origin label; any other copy is the owner's file, labeled private like its original.

Copies belong to the task tree and go when it ends, except a copy shown in a tab that outlives the tree (the owner's
borrowed tab, or one kept open for them): Chromium reads the file again when the form is submitted, so that copy stays
until the browser says the tab closed, or until the next start-up, which removes every copy a restart left behind.

FERB's shell can write the workspace, so each folder here is held by a handle opened without following a link
(O_NOFOLLOW), from `uploads/` down; the copy is made into its folder's handle from the original's handle, never through
a link at its own name (anything already there refuses the copy), and must still be a regular file at the path given to
the browser; every removal goes through the handle of the folder it is in. A link swapped into a path can't redirect,
replace, re-stamp or remove anything outside `uploads/`. A link in place of one of FERB's folders is refused when
staging and removed, never followed, when cleaning; a link in place of `uploads/` itself (or, until the next start-up,
`outside/`) is left alone and reported, with nothing under it touched. What a cleanup can't remove is reported after
the rest is removed; an entry a concurrent cleanup already removed is no failure."""

import errno
import os
import secrets
import shutil
import stat
import threading
from pathlib import Path
from typing import Callable, Iterable, Sequence

from ferb.act.fs import recovery

FOLDER = "uploads"          # under FERB's workspace
OUTSIDE = "outside"         # copies of outside content: an untrusted fs root (downloads.untrusted_roots adds it)
_OPEN_DIR = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
_OPEN_FILE = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
Leaf = tuple[str, ...]      # a copy's folder under `uploads/`: ([outside,] tree, random)


class StagingFailed(Exception):
    """No copy for the browser, in words the worker reads. Nothing was given to the browser."""


class CleanupLeft(OSError):
    """What a cleanup couldn't remove (a locked file a shell command planted, a link in place of FERB's folder); the
    rest went."""

    def __init__(self, left: list[str]) -> None:
        super().__init__("FERB couldn't remove " + "; ".join(left))
        self.left = left


class _NotOurs(OSError):
    """A link or a file where one of FERB's folders should be: nothing under it is touched."""

    def __init__(self, path: Path) -> None:
        super().__init__(errno.ENOTDIR, "not FERB's folder (a link or a file); nothing under it was touched", str(path))


def tab_key(profile: str, chrome_id: int) -> str:
    """The browser tab a copy was shown in: the bridge's `browser/profile` and the browser's tab id."""
    return f"{profile}#{chrome_id}"


def _tree_name(tree: str) -> bool:
    return bool(tree) and Path(tree).name == tree and not tree.startswith(".") and tree != OUTSIDE


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


def _remove(at: int, name: str, shown: Path, left: list[str]) -> int:
    """One entry of a folder FERB holds open, removed through that handle: a folder with all it holds (rmtree on a
    handle checks the entry is no link and never follows one inside), anything else unlinked, a link never followed.
    What can't be removed is added to `left` and the rest still goes. Returns 1 when the entry went."""
    try:
        mode = os.stat(name, dir_fd=at, follow_symlinks=False).st_mode
        if not stat.S_ISDIR(mode):
            os.unlink(name, dir_fd=at)
            return 1
    except FileNotFoundError:
        return 0
    except OSError as e:
        left.append(f"{shown}: {e.strerror or e}")
        return 0
    failed: list[str] = []

    def note(fn: Callable[..., object], path: str, exc: BaseException) -> None:
        if not isinstance(exc, FileNotFoundError):              # gone already (a concurrent cleanup): not a failure
            failed.append(f"{shown.parent / path}: {getattr(exc, 'strerror', exc)}")
    shutil.rmtree(name, dir_fd=at, onexc=note)
    left.extend(failed)
    return 0 if failed else 1


def _names(copy: Path, leaf_fd: int, name: str) -> bool:
    """`copy`, resolved as the browser will, still names the regular file made in the held folder (no folder swapped
    meanwhile, no link put in the copy's place)."""
    try:
        made = os.stat(name, dir_fd=leaf_fd, follow_symlinks=False)
        return stat.S_ISREG(made.st_mode) and os.path.samestat(os.stat(copy, follow_symlinks=False), made)
    except OSError:
        return False


class Uploads:
    def __init__(self, root: Path, untrusted: Callable[[], Iterable[str]]) -> None:
        self.root, self.untrusted = root, untrusted
        self._lock = threading.Lock()                   # staging and cleanup run in worker threads
        self._shown: dict[str, set[Leaf]] = {}          # tab -> copies it may still show (re-read at submit)
        self._ended: set[str] = set()                   # trees that ended while a tab still showed a copy of theirs

    def _outside(self, source: Path) -> bool:
        for u in self.untrusted():
            ur = Path(u).expanduser().resolve()
            if source == ur or ur in source.parents:
                return True
        return False

    def stage(self, tree: str, source: str, sha256: str, tab: str | None = None) -> str:
        """`stage_from`, the source (a real path) opened here, never through a link at its last step."""
        try:
            fd = os.open(source, _OPEN_FILE)
        except OSError as e:
            raise StagingFailed(f"FERB can't read {source} ({e.strerror or e})") from None
        try:
            return self.stage_from(tree, fd, source, sha256, tab)
        finally:
            os.close(fd)

    def stage_all(self, tree: str, sources: Sequence[tuple[int, str, str]], tab: str | None = None) -> list[str]:
        """Each (open file, its real path, its SHA-256) staged; when one fails, the copies already made go at once."""
        made: list[str] = []
        try:
            for fd, source, sha256 in sources:
                made.append(self.stage_from(tree, fd, source, sha256, tab))
            return made
        except BaseException:
            self._discard(made)
            raise

    def stage_from(self, tree: str, src_fd: int, source: str, sha256: str, tab: str | None = None) -> str:
        """Copy the open file src_fd (`source`: its real path, whose identity was just read through this handle) to
        <root>[/outside]/<tree>/<random>/<its name> and return the copy's path, which must still name the copy, a
        regular file. Anything already at that name (a planted link) refuses the copy. The copy must hash to `sha256`;
        otherwise it is removed and refused. `tab` (`tab_key`): the tab shown the copy,
        which keeps it past its tree while that tab is open. Blocking: callers on the event loop run it in a thread."""
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
                leaf_fd = _open(leaf, at, shown / leaf)
                held.append(leaf_fd)
                try:
                    digest = recovery.copy_into(src_fd, leaf_fd, src.name)
                except FileExistsError:
                    raise StagingFailed(f"{copy} was already taken (a link or a file put there); nothing was "
                                        "copied") from None
                if digest != sha256:
                    raise StagingFailed("the file changed while FERB copied it; inspect it again")
                if not _names(copy, leaf_fd, src.name):
                    raise StagingFailed(f"{copy} no longer names FERB's copy (one of its folders was replaced)")
            except BaseException:
                shutil.rmtree(leaf, dir_fd=at, ignore_errors=True)   # only the leaf just made, through its folder
                raise
        except _NotOurs as e:
            raise StagingFailed(f"{e.filename} is not FERB's folder (a link or a file); nothing was copied") from None
        finally:
            for fd in held:
                os.close(fd)
        if tab:
            with self._lock:
                self._shown.setdefault(tab, set()).add((*parts, leaf))
        return str(copy)

    def closed(self, tab: str) -> bool:
        """That tab is gone (FERB closed it, or the browser said it closed): the copies it showed need not stay. Those
        of a tree still running go when it ends. True when it showed a copy of a tree that already ended: the caller
        runs `collect` (in a thread)."""
        with self._lock:
            leaves = self._shown.pop(tab, set())
            return any(leaf[-2] in self._ended for leaf in leaves)

    def forget_tree(self, tree: str) -> int:
        """The task tree ended: its copies go, except those a tab still open shows (they go when it closes, or at the
        next start-up). A planted link is removed, never followed. Returns entries removed; raises CleanupLeft naming
        what couldn't be removed, after removing the rest."""
        if not _tree_name(tree):
            return 0
        with self._lock:
            self._ended.add(tree)
        left: list[str] = []
        gone = self._clean(tree, left)
        if left:
            raise CleanupLeft(left)
        return gone

    def collect(self) -> int:
        """Copies of trees that already ended that no open tab shows any more go. Raises CleanupLeft as forget_tree."""
        with self._lock:
            trees = sorted(self._ended)
        left: list[str] = []
        gone = sum(self._clean(tree, left) for tree in trees)
        if left:
            raise CleanupLeft(left)
        return gone

    def sweep(self) -> int:
        """At start-up no task is running and no tab shows a copy FERB knows of: every copy left here goes, a planted
        link included (removed, never followed). Raises OSError when `uploads/` is not FERB's folder, and CleanupLeft
        naming what couldn't be removed, after removing the rest."""
        with self._lock:
            self._shown.clear()
            self._ended.clear()
        root = _existing(self.root, None, self.root)
        if root is None:
            return 0
        gone, left = 0, list[str]()
        try:
            for name in os.listdir(root):
                if name == OUTSIDE and stat.S_ISDIR(os.stat(name, dir_fd=root, follow_symlinks=False).st_mode):
                    outside = _existing(OUTSIDE, root, self.root / OUTSIDE)
                    if outside is None:
                        continue
                    try:
                        for entry in os.listdir(outside):
                            gone += _remove(outside, entry, self.root / OUTSIDE / entry, left)
                    finally:
                        os.close(outside)
                else:
                    gone += _remove(root, name, self.root / name, left)
        finally:
            os.close(root)
        if left:
            raise CleanupLeft(left)
        return gone

    def _clean(self, tree: str, left: list[str]) -> int:
        """An ended tree's copies that no tab shows go, in both folders; the tree is done once no tab shows any."""
        with self._lock:
            kept = {leaf for leaves in self._shown.values() for leaf in leaves if leaf[-2] == tree}
        gone, keeping = 0, False
        try:
            root = _existing(self.root, None, self.root)
            if root is not None:
                try:
                    g, k = self._clean_in(root, (), tree, kept, left)
                    gone, keeping = gone + g, keeping or k
                    outside = _existing(OUTSIDE, root, self.root / OUTSIDE)
                    if outside is not None:
                        try:
                            g, k = self._clean_in(outside, (OUTSIDE,), tree, kept, left)
                            gone, keeping = gone + g, keeping or k
                        finally:
                            os.close(outside)
                finally:
                    os.close(root)
        except OSError as e:                                         # _NotOurs: a link in place of FERB's folder
            left.append(str(e))
        if not keeping:
            with self._lock:
                self._ended.discard(tree)
        return gone

    def _clean_in(self, at: int, base: tuple[str, ...], tree: str, kept: set[Leaf],
                  left: list[str]) -> tuple[int, bool]:
        """`tree`'s folder in the open folder `at` (`base` names it): copies no tab shows removed through it, and the
        folder too once empty. Returns (entries removed, any copy kept)."""
        shown = self.root.joinpath(*base, tree)
        try:
            if not stat.S_ISDIR(os.stat(tree, dir_fd=at, follow_symlinks=False).st_mode):
                return _remove(at, tree, shown, left), False         # a planted link or file: removed, never followed
            fd = _open(tree, at, shown)
        except FileNotFoundError:                                    # none, or a concurrent cleanup just removed it
            return 0, False
        gone, keep, before = 0, 0, len(left)
        try:
            for name in os.listdir(fd):
                if (*base, tree, name) in kept:
                    keep += 1
                else:
                    gone += _remove(fd, name, shown / name, left)
        finally:
            os.close(fd)
        if not keep and len(left) == before:
            try:
                os.rmdir(tree, dir_fd=at)
            except OSError:
                pass                                                 # a copy staged meanwhile: its tree end removes it
        return gone, keep > 0

    def _discard(self, copies: Iterable[str]) -> None:
        """Copies just made for an upload that failed go at once, through their folders (best effort: what stays goes
        with the tree)."""
        for copy in copies:
            leaf = Path(copy).relative_to(self.root).parts[:-1]
            with self._lock:
                for leaves in self._shown.values():
                    leaves.discard(leaf)
            held: list[int] = []
            try:
                at = _open(self.root, None, self.root)
                held.append(at)
                for name in leaf[:-1]:
                    at = _open(name, at, self.root)
                    held.append(at)
                _remove(at, leaf[-1], Path(copy).parent, [])
            except OSError:
                pass
            finally:
                for fd in held:
                    os.close(fd)
