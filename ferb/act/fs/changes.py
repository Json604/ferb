"""Owns the before-and-after record of the files a task changes (L39): before a call that can change files runs, a
copy of each existing file it names and a listing of each folder it names or writes into; at the step's check, what
changed, as an old-vs-new text diff the checker reads (plain text directly, DOCX through its text, PDF through its
text layer with its annotations counted, so a box drawn over text shows as "annotations added, text layer
unchanged"). Must not judge whether a change was meant: code records and compares facts; the checker judges (rules
5, 6).

- What a call names (`named`): a file tool's declared files; for a shell command or an AppleScript, every absolute, ~
  and folder-relative path written in its text, quoted or not, after the text's own literal `NAME=value` assignments
  and `cd` are applied. These are facts of the text, never a guess at what the command does. A named folder's own
  files are copied (not its subfolders'), and the folder listed; a named file's folder is listed too, so a file made,
  removed or changed beside it shows (a corrected copy saved next to the original).
- Copies are APFS clones (instant; no space until the original changes). Where the volume can't clone, the file is
  listed with no earlier copy: its change is still reported, without a diff. Nothing is copied or listed from FERB's
  own state, a store closed to FERB's tools, a Trash or /dev, nor a file FERB couldn't change (neither it nor its
  folder writable).
- A file made in the step is compared with the most similar file of its kind that the step changed or removed, or
  that sits unchanged in the same folder, as git finds a copy (at least SIMILAR alike): a corrected copy shows as its
  changes from the original.
- Copies live under ~/.ferb/changes (owner only) for the task tree's life: `forget` when the tree ends, and FERB's
  start sweeps what a stopped daemon left. Each step task has its own record; a loop round starts its own (`reset`).
Residual (ruling, L39): a write to a path the command's text never names (a script file's own paths, a computed name,
a glob outside a named folder) is not seen. The sandbox's kernel write reports would see it (measured 2026-10-25:
every write, by the command's tag, in a live log stream; a later `log show` lost a quarter of 3,000), but they need a
system log stream running and give no earlier copy."""

import dataclasses
import difflib
import errno
import functools
import io
import hashlib
import json
import os
import re
import shutil
import stat
import threading
import xml.etree.ElementTree as ET
import zipfile
from collections import Counter
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Callable, Iterable, Iterator

from ferb.act.fs import recovery
from ferb.act.fs.files import opened_path

SIMILAR = 0.5               # a made file is shown against a file at least this alike (git's default copy/rename bar)
TEXT_MAX = 2_000_000        # display only: characters of extracted text compared per side; a longer text is not
                            # diffed here (the record says so and the checker reads the file); it decides nothing
XML_MAX = 50_000_000        # a DOCX part declaring more than this uncompressed is not inflated (a zip bomb)
CONTEXT_LINES = 2           # unchanged lines shown around each change in a diff
FILE_MAX = 100_000_000      # display only: a file larger than this is not read for a diff (the record says so)
PAIR_READS = 3              # a new file is compared with at most this many like-named files beside it (never a whole
PAIR_SIZE = 4               # folder), each within this factor of its size; both filter before any text is read
FOLDER_FACTS = 50           # display only: files no call named listed by name per check, across all folders; the
                            # rest counted per listed folder
WALK_MAX = 5_000            # display only: entries looked at per check in folders the step made (and files changed in
WALK_DEPTH = 32             # place past the page); folders nested deeper aren't walked; a fact says where it stopped
PREVIEW_CHARS = 12_000      # display only: the first part of a new named text file's text in its record (fs_read reads
                            # the rest; a new named DOCX or PDF's record holds all its text, which fs_read can't extract)
DOCUMENTS = (".pdf", ".docx", ".docm", ".dotx")
UNREADABLE = "FERB can't read it"
UNCHANGED = "content unchanged (the same bytes as before the step)"
HISTORY_NOTE = "a shell history file: what is typed in a terminal is never shown (rule 14)"
INDEX = "index.json"
_W = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"
_PROPS = {"{http://purl.org/dc/elements/1.1/}title": "title", "{http://purl.org/dc/elements/1.1/}subject": "subject",
          "{http://purl.org/dc/elements/1.1/}creator": "creator", "{http://purl.org/dc/elements/1.1/}description":
          "description", "{http://schemas.openxmlformats.org/package/2006/metadata/core-properties}lastModifiedBy":
          "last modified by", "{http://schemas.openxmlformats.org/package/2006/metadata/core-properties}keywords":
          "keywords"}
_PARTS = re.compile(r"word/(header|footer|footnotes|endnotes)\d*\.xml")
Annotation = tuple[int, str, str]          # a PDF annotation: (page, type, its text)

# ---------- what a command's text names ----------

_ASSIGN = re.compile(r"(?:^|(?<=[\s;&|(]))(?:export\s+|local\s+|readonly\s+|declare\s+)?([A-Za-z_][A-Za-z0-9_]*)="
                     r"(\"(?:[^\"\\\n]|\\.)*\"|'[^'\n]*'|[^\s;&|()<>'\"`]*)")
_CD = re.compile(r"(?:^|(?<=[\s;&|(]))cd\s+(\"(?:[^\"\\\n]|\\.)*\"|'[^'\n]*'|[^\s;&|()<>'\"`]+)")
_TOKENS = re.compile(r"\"((?:[^\"\\\n]|\\.)*)\"|'([^'\n]*)'|([^\s'\"`;|&<>(){}]+)")
_INNER = re.compile(r"(?<=['\"])(~?/[^'\"\n]+)(?=['\"])")      # a path quoted inside a quoted string (code)
_FILE_NAME = re.compile(r"[^/\s]+\.[A-Za-z0-9]{1,8}")
_SUFFIX = re.compile(r"\.[A-Za-z0-9]{1,16}")
_VAR = re.compile(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)")
_EMBEDDED = re.compile(r"(?:^|[=:,(])((?:~|/)[^,;:)]*)$")
_GLOB = re.compile(r"[*?\[]")


def _unquote(word: str) -> str:
    if len(word) >= 2 and word[0] == word[-1] == "'":
        return word[1:-1]
    if len(word) >= 2 and word[0] == word[-1] == '"':
        return re.sub(r"\\(.)", r"\1", word[1:-1])
    return word


def _expand(text: str, values: dict[str, str], home: Path) -> str | None:
    """`text` with the command's own literal variables put in, ~ expanded; None when a variable stays unknown or a
    command substitution is in it (its value isn't in the text)."""
    if "$(" in text or "`" in text:
        return None
    for _ in range(4):                                   # a value may name an earlier variable
        done = _VAR.sub(lambda m: values.get(m.group(1) or m.group(2), "\0"), text)
        if done == text:
            break
        text = done
    if "\0" in text or "$" in text:
        return None
    if text == "~" or text.startswith("~/"):
        text = str(home) + text[1:]
    return text


def named(text: str, cwd: Path, home: Path | None = None) -> list[Path]:
    """Every path written in a command's or script's text, as the text spells it: absolute, ~, or relative to the
    folder the text is in at that point (`cwd`, then each literal `cd`), with the text's literal `NAME=value`
    assignments put in. A glob names the folder before its first wildcard. Facts of the text, never of what runs."""
    home = home or Path.home()
    values: dict[str, str] = {}
    for m in _ASSIGN.finditer(text):
        value = _expand(_unquote(m.group(2)), values, home)
        if value is not None:
            values[m.group(1)] = value
    cds = [(m.start(), _unquote(m.group(1))) for m in _CD.finditer(text)]
    words: list[tuple[int, str]] = []
    for m in _TOKENS.finditer(text):
        if m.group(1) is not None:
            words.append((m.start(), re.sub(r"\\(.)", r"\1", m.group(1))))
        else:
            words.append((m.start(), m.group(2) if m.group(2) is not None else m.group(3)))
    words += [(m.start(), m.group(1)) for m in _INNER.finditer(text)]
    found: list[Path] = []
    for start, raw in words:
        base = cwd
        for at, where in cds:                            # the folder the text is in where this path is written
            if at >= start:
                break
            moved = _expand(where, values, home)
            if moved is not None:
                base = base / moved if not os.path.isabs(moved) else Path(moved)
        for piece in {raw, *(e.group(1) for e in [_EMBEDDED.search(raw)] if e)}:
            path = _expand(piece.strip(), values, home)
            if not path or "\n" in path or "\0" in path or len(path) > 1024:
                continue
            if (g := _GLOB.search(path)) is not None:   # a glob names the folder it lists
                head, slash, _ = path[:g.start()].rpartition("/")
                if not slash:
                    found.append(base)
                    continue
                path = head or "/"
            elif "/" not in path and not _FILE_NAME.fullmatch(path):
                continue                                 # a bare word, not a file name
            found.append(Path(path) if path.startswith("/") else base / path)
    return list(dict.fromkeys(Path(os.path.normpath(p)) for p in found))


# ---------- the record ----------

@dataclass
class _File:
    existed: bool
    size: int = 0
    mtime_ns: int = 0
    ino: int = 0
    copy: str | None = None          # the clone's name in the task's folder
    no_copy: str = ""                # why there is none (the volume can't clone, ...)
    real: str = ""                   # the file the clone was taken from, as the opened file really is (F_GETPATH)
    history: bool = False            # a shell history file, by identity: never cloned, never shown
    dev: int = 0                     # with `ino`, the file's identity (0: a record from before this field)


@dataclass
class _Record:
    task_id: str
    tree_id: str
    files: dict[str, _File] = field(default_factory=dict)
    folders: dict[str, dict[str, list[int]]] = field(default_factory=dict)   # path -> name -> [kind, size, mtime, ino]
    named: set[str] = field(default_factory=set)     # missing paths a call named exactly (a file it may make)
    n: int = 0


@dataclass(frozen=True)
class Change:
    """One file that changed during the task, as facts: `what` is modified, created, removed or renamed; `before` the
    copy FERB kept from before (None: none was kept, `note` says why); `source` for a new file, the file it was compared
    with, and for a renamed one, its name before the step.
    `content` False: a fact only (a file no call named, a shell history file, a path that is no longer a plain file):
    its name, size and time, never what it says. `named`: a call of the step named it exactly."""
    path: str
    what: str
    before: str | None = None
    source: str | None = None
    note: str = ""
    content: bool = True
    size: int | None = None
    mtime_ns: int | None = None
    named: bool = False
    before_real: str | None = None   # the file FERB's copy was taken from (checked again before the copy is read)


def _kind(st: os.stat_result) -> int:
    return 1 if stat.S_ISREG(st.st_mode) else 2 if stat.S_ISDIR(st.st_mode) else 3


def _what_now(st: os.stat_result, path: Path) -> str:
    """A path that is no longer a plain file, in words (no content is read)."""
    if stat.S_ISLNK(st.st_mode):
        try:
            return f"now a link to {os.readlink(path)}"
        except OSError:
            return "now a link"
    return "now a folder" if stat.S_ISDIR(st.st_mode) else "now a special file (not a plain file)"


def _history_name(name: str) -> bool:
    """A home-folder dotfile that holds typed history: *_history, *.history, .histfile, .lesshst (any case)."""
    low = name.casefold()
    return low.startswith(".") and (low.endswith(("_history", ".history")) or low in (".histfile", ".lesshst"))


_NOFOLLOW = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC


def _open(path: Path | str, directory: bool = False, dir_fd: int | None = None) -> int | None:
    """An fd for the path itself, never through a link at its last name (O_NOFOLLOW; O_NONBLOCK so a FIFO can't hold
    FERB); a folder with O_DIRECTORY. None when it can't be opened so."""
    try:
        return os.open(path, _NOFOLLOW | (os.O_DIRECTORY if directory else 0), dir_fd=dir_fd)
    except OSError:
        return None


@dataclass(frozen=True)
class _Scan:
    """A listed folder now: the folder it really is (F_GETPATH of the checked fd), its device, its entries (lstat)."""
    where: Path
    dev: int
    entries: dict[str, os.stat_result]


@dataclass
class _Look:
    """One check's walk: the changes found, the paths seen, recorded files whose name is gone (by identity, a new name
    for one is a rename) and those renamed, and the page shared by every folder: files no call named listed so far
    (FOLDER_FACTS in all), the rest counted per listed folder, entries looked at (WALK_MAX in all), where FERB stopped
    and where folders nested too deep weren't walked."""
    rec: _Record
    folder: Path
    seen: set[str]
    out: list[Change] = field(default_factory=list)
    moved: dict[tuple[int, int], str] = field(default_factory=dict)
    renamed: set[str] = field(default_factory=set)
    names: int = 0
    entries: int = 0
    more: Counter[str] = field(default_factory=Counter)
    stopped: str | None = None
    deep: set[str] = field(default_factory=set)


def _known(recorded: set[tuple[int, int]], dev: int, ino: int) -> bool:
    """A listing entry's identity is a recorded file's (a record from before `dev` was kept matches by inode)."""
    return (dev, ino) in recorded or (0, ino) in recorded


def _kind_of(path: Path) -> str:
    """What kind of text a file holds, for pairing: a PDF, a Word document, or text."""
    suffix = path.suffix.lower()
    return "pdf" if suffix == ".pdf" else "docx" if suffix in DOCUMENTS else "text"


class Changes:
    """The before-and-after record. `closed(path)`: whether FERB's shell sandbox denies reading the path (FERB's own
    state, the never-read stores: `Protection.read_denied`). The record never clones, reads or lists such a path,
    whatever the call named: every file and folder is opened without following a link, the path it really opened
    (F_GETPATH) is checked, and only then is it cloned (from the fd), read or listed (from the fd), so a link swapped in
    between can't redirect it. `home`: the owner's home, for the shell history files, recognized by identity (device
    and inode), whose text is never shown (rule 14)."""

    def __init__(self, root: Path, closed: Callable[[Path], bool] | None = None, home: Path | None = None) -> None:
        self.root, self.closed = root, closed or (lambda path: False)
        self.home = home or Path.home()
        self.sessions = (self.home / ".zsh_sessions", self.home / ".bash_sessions")   # per-session history folders
        self.fish = self.home / ".local" / "share" / "fish" / "fish_history"
        self._hist: set[tuple[int, int]] | None = None   # the history files' identities, for one call
        self._records: dict[str, _Record] = {}
        self._saved: dict[str, str] = {}               # task -> the index last written (written again only on change)
        self._locks: dict[str, threading.Lock] = {}    # one per task: one task's large folder never holds another's
        self._guard = threading.Lock()

    # where things live
    @staticmethod
    def _key(text: str) -> str:
        return hashlib.sha256(text.encode()).hexdigest()[:24]

    def _folder(self, tree_id: str, task_id: str) -> Path:
        return self.root / self._key(tree_id) / self._key(task_id)

    def _lock(self, task_id: str) -> threading.Lock:
        with self._guard:
            return self._locks.setdefault(task_id, threading.Lock())

    def _record(self, task_id: str, tree_id: str | None = None) -> _Record | None:
        with self._guard:
            rec = self._records.get(task_id)
        if rec is not None or tree_id is None:
            return rec
        index = self._folder(tree_id, task_id) / INDEX           # a tree resumed after a restart: its record on disk
        try:
            text = index.read_text()
            data = json.loads(text)
            rec = _Record(data["task_id"], data["tree_id"], {k: _File(**v) for k, v in data["files"].items()},
                          data["folders"], set(data.get("named", [])), int(data["n"]))
            self._saved[task_id] = text
        except (OSError, ValueError, KeyError, TypeError):
            rec = _Record(task_id, tree_id)
        with self._guard:
            return self._records.setdefault(task_id, rec)

    def _save(self, rec: _Record) -> None:
        text = json.dumps({"task_id": rec.task_id, "tree_id": rec.tree_id, "n": rec.n, "folders": rec.folders,
                           "named": sorted(rec.named), "files": {k: asdict(v) for k, v in rec.files.items()}})
        if self._saved.get(rec.task_id) == text:
            return
        folder = self._folder(rec.tree_id, rec.task_id)
        tmp = folder / (INDEX + ".tmp")
        tmp.write_text(text)
        os.replace(tmp, folder / INDEX)
        self._saved[rec.task_id] = text

    def _skip(self, path: Path) -> bool:
        """Never recorded or read: the root, /dev, a Trash, FERB's own change store, and what the shell's sandbox
        denies to reads, as the path is written and as it really resolves (a link into a closed path is closed)."""
        parts = path.parts
        if path == Path("/") or parts[:2] == ("/", "dev") or any(p in (".Trash", ".Trashes") for p in parts):
            return True
        try:
            resolved = Path(os.path.realpath(path))
        except (OSError, ValueError):
            return True
        return any(p == self.root or self.root in p.parents or self.closed(p) for p in {path, resolved})

    def _closed_as_written(self, path: Path) -> bool:
        """`_skip` for a recorded path as it was written, without resolving what stands there now."""
        parts = path.parts
        return (parts[:2] == ("/", "dev") or any(p in (".Trash", ".Trashes") for p in parts) or path == self.root
                or self.root in path.parents or self.closed(path))

    def _opened(self, fd: int) -> Path | None:
        """The path an open fd really is, when it isn't skipped (closed, a Trash, FERB's store); else None."""
        try:
            where = opened_path(fd)
        except OSError:
            return None
        return None if self._skip(where) else where

    # shell and REPL history, by identity
    def _identity(self, path: Path) -> tuple[int, int] | None:
        try:
            st = os.stat(path)
        except OSError:
            return None
        return st.st_dev, st.st_ino

    def _history_ids(self) -> set[tuple[int, int]]:
        """The (device, inode) of every history file now (rule 14's "never shown", a where-boundary by identity, so a
        hard link or another spelling counts): every file under ~/.zsh_sessions (and older macOS bash's
        ~/.bash_sessions), ~/.local/share/fish/fish_history,
        and the home folder's dotfiles named *_history or *.history, .histfile and .lesshst (zsh, bash, ksh, tcsh,
        Python, Node, psql, mysql, sqlite, less). Enumerated afresh by each before() and changes() call."""
        if self._hist is not None:
            return self._hist
        ids: set[tuple[int, int]] = set()

        def add(st: os.stat_result) -> None:
            if stat.S_ISREG(st.st_mode):
                ids.add((st.st_dev, st.st_ino))
        try:
            with os.scandir(self.home) as it:
                for e in it:
                    if _history_name(e.name):
                        try:
                            add(e.stat(follow_symlinks=False))
                        except OSError:
                            continue
        except OSError:
            pass
        for folder in self.sessions:
            try:
                with os.scandir(folder) as it:
                    for e in it:
                        try:
                            add(e.stat(follow_symlinks=False))
                        except OSError:
                            continue
            except OSError:
                pass
        try:
            add(os.stat(self.fish, follow_symlinks=False))
        except OSError:
            pass
        self._hist = ids
        return ids

    def _history_st(self, st: os.stat_result, opened: Path | None = None) -> bool:
        """An opened file is a history file: its (device, inode) is one of `_history_ids` (any spelling, a hard
        link), or it sits in a per-session history folder."""
        if (st.st_dev, st.st_ino) in self._history_ids():
            return True
        sessions = {i for i in map(self._identity, self.sessions) if i is not None}
        return opened is not None and bool(sessions) and any(self._identity(p) in sessions for p in opened.parents)

    def _history(self, path: Path) -> bool:
        """A path names a history file: by identity when it exists (opened without following a link); when it
        doesn't, by its canonical folder (the home folder, the session folders, fish's folder) and a case-insensitive
        name."""
        fd = _open(path)
        if fd is not None:
            try:
                st = os.fstat(fd)
                return stat.S_ISREG(st.st_mode) and self._history_st(st, opened_path(fd))
            except OSError:
                return False
            finally:
                os.close(fd)
        try:                                            # it exists but can't be opened: its identity from lstat
            st = path.lstat()
            if stat.S_ISREG(st.st_mode):
                return self._history_st(st)
        except OSError:
            pass
        folder = self._identity(path.parent)
        if folder is None:
            return False
        if folder in {self._identity(p) for p in self.sessions}:
            return True
        if folder == self._identity(self.fish.parent):
            return path.name.casefold() == self.fish.name
        return folder == self._identity(self.home) and _history_name(path.name)

    # before a call
    def before(self, task_id: str, tree_id: str, paths: Iterable[Path]) -> dict[str, int]:
        """Before a call that may change these paths (each named exactly by the call): each existing plain file not yet
        recorded for the task is cloned (from its fd) and its folder listed; a missing one is remembered as named and
        its nearest existing folder listed (a file made there, or under new folders there, shows as new); a named
        folder is listed. A path the shell's sandbox denies to reads is never touched (counted as protected). Returns
        counts for the call's record. Never raises for one path."""
        with self._lock(task_id):
            rec = self._record(task_id, tree_id)
            assert rec is not None
            self._hist = None
            folder = self._folder(tree_id, task_id)
            folder.mkdir(mode=0o700, parents=True, exist_ok=True)
            counts = {"copied": 0, "not_copied": 0, "folders": 0, "protected": 0}
            for raw in paths:
                try:
                    path = real(raw)
                except (OSError, RuntimeError, ValueError):
                    continue
                if self._skip(path):
                    counts["protected"] += 0 if path == Path("/") else 1
                    continue
                try:
                    st = path.lstat()
                except FileNotFoundError:               # a file it may make: remembered, its nearest folder listed
                    if (near := _nearest(path)) is not None and near != Path("/") and os.access(near, os.W_OK):
                        rec.named.add(str(path))
                        self._list(rec, near, counts)
                    continue
                except OSError:
                    continue
                if stat.S_ISDIR(st.st_mode):
                    self._list(rec, path, counts)
                elif stat.S_ISREG(st.st_mode):
                    self._list(rec, path.parent, counts)
                    self._file(rec, folder, path, counts)
            self._save(rec)
            return counts

    def _scan(self, folder: Path) -> _Scan | None:
        """A folder's own entries (lstat), listed through an fd opened without following a link and only when the
        folder it really is isn't skipped; None otherwise (a link now stands there, or it is closed)."""
        fd = _open(folder, directory=True)
        if fd is None:
            return None
        try:
            where = self._opened(fd)
            if where is None:
                return None
            entries: dict[str, os.stat_result] = {}
            with os.scandir(fd) as it:
                for e in it:
                    try:
                        entries[e.name] = e.stat(follow_symlinks=False)
                    except OSError:
                        continue
            return _Scan(where, os.fstat(fd).st_dev, entries)
        except OSError:
            return None
        finally:
            os.close(fd)

    def _list(self, rec: _Record, folder: Path, counts: dict[str, int]) -> bool:
        """The folder's own entries, once per task (its listing from before the task first touched it); only a
        writable folder FERB may list (`_scan`)."""
        key = str(folder)
        if key in rec.folders:
            return True
        if self._skip(folder) or not os.access(folder, os.W_OK):
            return False
        scan = self._scan(folder)
        if scan is None:
            return False
        rec.folders[key] = {name: [_kind(st), st.st_size, st.st_mtime_ns, st.st_ino]
                            for name, st in scan.entries.items()}
        counts["folders"] += 1
        return True

    def _file(self, rec: _Record, folder: Path, path: Path, counts: dict[str, int]) -> None:
        """Record one named plain file: opened without following a link, checked as the file it really is, cloned
        from that fd (fclonefileat), its facts from that fstat. A path that turns into anything else, or into a closed
        file, between FERB's look and its open is never cloned. One FERB can't open is recorded from lstat, without a
        copy, so its check still names it as the call's."""
        key = str(path)
        if key in rec.files or self._skip(path):
            return
        fd = _open(path)
        if fd is None:
            self._unreadable(rec, path, counts)
            return
        try:
            st = os.fstat(fd)
            opened = self._opened(fd)
            if not stat.S_ISREG(st.st_mode) or opened is None:
                counts["protected"] += 0 if opened is not None else 1
                return
            if not (os.access(opened, os.W_OK) or os.access(opened.parent, os.W_OK)):
                return                                 # a file FERB couldn't change
            rec.n += 1
            name = f"c{rec.n}" + (path.suffix if _SUFFIX.fullmatch(path.suffix) else "")   # the type tells how to read it
            entry = _File(True, st.st_size, st.st_mtime_ns, st.st_ino, real=str(opened),
                          history=self._history_st(st, opened), dev=st.st_dev)
            if not entry.history:                       # a history file's text is never kept, so never diffed
                dir_fd = os.open(folder, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
                try:
                    recovery._fclone(fd, dir_fd, name)
                    recovery._unlock(folder / name)     # a locked original's clone could never be removed
                    entry.copy = name
                    counts["copied"] += 1
                except OSError as e:
                    entry.no_copy = ("this volume can't make an instant copy" if e.errno in (errno.EXDEV, errno.ENOTSUP)
                                     else f"FERB couldn't copy it ({e.strerror or e})")
                    counts["not_copied"] += 1
                finally:
                    os.close(dir_fd)
            rec.files[key] = entry
        except OSError:
            return
        finally:
            os.close(fd)

    def _unreadable(self, rec: _Record, path: Path, counts: dict[str, int]) -> None:
        """B2: a named plain file FERB can't open (a write-only file): its facts from lstat, no copy (UNREADABLE), so
        its check lists it as named by the call, a fact, never as a file no call named."""
        try:
            st = path.lstat()
        except OSError:
            return
        if not stat.S_ISREG(st.st_mode) or not (os.access(path, os.W_OK, follow_symlinks=False)
                                                or os.access(path.parent, os.W_OK)):
            return
        rec.files[str(path)] = _File(True, st.st_size, st.st_mtime_ns, st.st_ino, no_copy=UNREADABLE,
                                     history=self._history_st(st), dev=st.st_dev)
        counts["not_copied"] += 1

    def reset(self, task_id: str) -> list[str]:
        """A loop round starts its own record (its check judges that round only). Returns what couldn't be removed."""
        with self._lock(task_id):
            with self._guard:
                rec = self._records.pop(task_id, None)
            self._saved.pop(task_id, None)
            return _remove(self._folder(rec.tree_id, task_id)) if rec is not None else []

    # at the check
    def changes(self, task_id: str, tree_id: str | None = None) -> list[Change]:
        """What changed since the task first touched each recorded file or folder: a recorded file by its copy
        (content) or, with no copy, its size, time and identity; one now under another name (the same device and
        inode, in a listed folder or a folder made there) as renamed; new, changed and removed files in each listed
        folder, and the plain files under folders made there. A file the calls named exactly carries content; anything
        else is a fact only, as is a history file or a path that is no longer a plain file. Facts for files no call
        named share one page per check (FOLDER_FACTS names, the rest counted per listed folder), and FERB looks at no
        more than WALK_MAX entries in new folders (a fact says where it stopped). Nothing the shell's sandbox denies to
        reads appears, and no folder is listed through a link or walked except through the handle FERB checked."""
        with self._lock(task_id):
            rec = self._record(task_id, tree_id)
            if rec is None:
                return []
            self._hist = None
            folder = self._folder(rec.tree_id, task_id)
            scans = {where: self._scan(Path(where)) for where in rec.folders}
            look = _Look(rec, folder, seen=set(rec.files))
            found: dict[str, Change | None] = {}
            for key, f in rec.files.items():
                found[key] = self._compare(Path(key), f, folder)
                if f.existed and self._gone(rec, Path(key), f, scans):
                    look.moved[(f.dev, f.ino)] = key
            recorded = {(f.dev, f.ino) for f in rec.files.values() if f.existed}
            for where, entries in rec.folders.items():
                place, scan = Path(where), scans[where]
                if scan is None:                         # no longer a folder FERB may list: a fact, never its names
                    try:
                        here = place.lstat()
                        look.out.append(Change(where, "modified", content=False, note=_what_now(here, place)))
                    except FileNotFoundError:
                        look.out.append(Change(where, "removed", content=False, note="the folder is gone"))
                    except OSError:
                        pass
                    continue
                for name in sorted(set(entries) | set(scan.entries)):
                    path = place / name
                    key = str(path)
                    if key in look.seen:
                        continue
                    look.seen.add(key)
                    old, st = entries.get(name), scan.entries.get(name)      # each change is made right here
                    if old is not None and _known(recorded, scan.dev, old[3]) and (st is None or st.st_ino == old[3]):
                        continue                         # a recorded file under another spelling: its record says it
                    if st is None:
                        if old is not None and old[0] == 1:
                            self._unnamed(look, where, scan.where / name, lambda: self._fact(path, "removed", None),
                                          shown=True)
                    elif stat.S_ISDIR(st.st_mode) and old is None:     # a folder made here: the plain files in it
                        self._made_under(look, path, where)
                    elif stat.S_ISREG(st.st_mode) and old is None:
                        now = st
                        self._made(look, path, scan.where / name, where, now.st_dev, now.st_ino, lambda: now)
                    elif stat.S_ISREG(st.st_mode) and [1, st.st_size, st.st_mtime_ns, st.st_ino] != old:
                        changed = st
                        self._unnamed(look, where, scan.where / name, lambda: self._fact(path, "modified", changed),
                                      shown=False)
            for key in sorted(rec.named - look.seen):    # a named file the walk didn't reach (a limit, a deep folder)
                self._named_now(look, Path(key))
            for where in rec.folders:
                parts = ([f"and {look.more[where]:,} more new or changed files here and in folders made here, not "
                          "listed"] if look.more[where] else []) + \
                        ([f"more than {WALK_MAX:,} entries in folders made during the step (and files changed in "
                          "place): FERB stopped looking there"] if look.stopped == where else []) + \
                        ([f"folders nested more than {WALK_DEPTH} deep were not walked"] if where in look.deep else [])
                if parts:
                    look.out.append(Change(where, "modified", content=False, note="; ".join(parts)))
            out = [c for key, c in found.items() if c is not None and key not in look.renamed] + look.out
            return [self._paired(c, out, folder, rec) if c.what == "created" and c.content else c for c in out]

    @staticmethod
    def _gone(rec: _Record, path: Path, f: _File, scans: dict[str, _Scan | None]) -> bool:
        """A recorded file's own name no longer holds it (removed, renamed, renamed in letter case only): a new entry
        that is the same file (device and inode) is then its new name. Its own name is the one its folder's listing
        held for it before the step (the spelling on disk, which a call may have typed in another case)."""
        old, scan = rec.folders.get(str(path.parent)), scans.get(str(path.parent))
        if old is not None and scan is not None:
            names = [n for n, e in old.items() if e[0] == 1 and e[3] == f.ino]
            name = path.name if path.name in names or not names else names[0]
            st = scan.entries.get(name)
            return st is None or st.st_ino != f.ino
        try:
            return os.lstat(path).st_ino != f.ino
        except FileNotFoundError:
            return True
        except OSError:
            return False

    def _unnamed(self, look: _Look, where: str, real_path: Path, change: Callable[[], Change], shown: bool) -> None:
        """A file no call named that changed in a listed folder: listed (a fact) while the check's page has room, then
        counted for its folder. `shown`: a name that appeared or went, which the shell's own listing of that open
        folder shows too, is counted by name alone; a file changed in place is counted only after the closed check,
        within WALK_MAX."""
        if look.names < FOLDER_FACTS:
            if not self._closed_as_written(real_path):
                look.names += 1
                look.out.append(change())
            return
        if not shown:
            if look.entries >= WALK_MAX:
                look.stopped = look.stopped or where
                return
            look.entries += 1
            if self._closed_as_written(real_path):
                return
        look.more[where] += 1

    def _made(self, look: _Look, path: Path, real_path: Path, where: str, dev: int, ino: int,
              stat_of: Callable[[], os.stat_result]) -> None:
        """A plain file that appeared (in a listed folder or a folder made there): the renamed file a call named, a
        new file a call named (content), or else a file no call named (a fact on the page, then counted)."""
        was = look.moved.get((dev, ino)) or look.moved.get((0, ino))
        if was is None and str(path) not in look.rec.named:
            if look.names >= FOLDER_FACTS:
                look.more[where] += 1                    # its name, which the open folder's listing shows: counted
                return
            if self._closed_as_written(real_path):
                return
            try:
                st = stat_of()
            except OSError:
                return
            if stat.S_ISREG(st.st_mode):
                look.names += 1
                look.out.append(self._fact(path, "created", st))
            return
        if self._skip(path) or self._closed_as_written(real_path):
            return
        try:
            st = stat_of()
        except OSError:
            return
        if not stat.S_ISREG(st.st_mode):
            return
        if was is not None:
            look.moved.pop((dev, ino), None) or look.moved.pop((0, ino), None)
            look.renamed.add(was)
            look.out.append(self._renamed(path, st, was, look.rec.files[was], look.folder))
        else:
            look.out.append(self._new(look.rec, path, st))

    def _named_now(self, look: _Look, path: Path) -> None:
        """A missing file a call named that no listing or walk reached: opened without following a link and checked
        as the file it really is, then judged as any file that appeared."""
        if self._skip(path):
            return
        fd = _open(path)
        if fd is None:
            try:
                st = path.lstat()
            except OSError:
                return
            if stat.S_ISREG(st.st_mode):
                look.out.append(Change(str(path), "created", content=False, named=True, size=st.st_size,
                                       mtime_ns=st.st_mtime_ns, note=UNREADABLE))
            return
        try:
            st = os.fstat(fd)
            opened = self._opened(fd)
            if opened is None or not stat.S_ISREG(st.st_mode):
                return
            look.seen.add(str(path))
            self._made(look, path, opened, str(path.parent), st.st_dev, st.st_ino, lambda: st)
        except OSError:
            return
        finally:
            os.close(fd)

    def _new(self, rec: _Record, path: Path, st: os.stat_result) -> Change:
        """A new plain file: content when a call named it exactly, it isn't a history file and FERB can open it; else a
        fact."""
        if str(path) not in rec.named:
            return self._fact(path, "created", st)
        made = Change(str(path), "created", size=st.st_size, mtime_ns=st.st_mtime_ns, named=True)
        if self._history(path):
            return dataclasses.replace(made, content=False, note=HISTORY_NOTE)
        fd = _open(path)
        if fd is None:
            return dataclasses.replace(made, content=False, note=UNREADABLE)
        os.close(fd)
        return made

    def _renamed(self, path: Path, st: os.stat_result, was: str, f: _File, folder: Path) -> Change:
        """A1: the file a call named, now under another name (the same device and inode): renamed, its content compared
        with FERB's copy from before the step; a fact when its content is unchanged, it is a history file, FERB can't
        read it, or its copy came from a path now closed."""
        made = Change(str(path), "renamed", source=was, named=True, size=st.st_size, mtime_ns=st.st_mtime_ns)
        if f.history or self._history(path):
            return dataclasses.replace(made, content=False, note=HISTORY_NOTE)
        if f.no_copy == UNREADABLE:
            return dataclasses.replace(made, content=False, note=UNREADABLE)
        if f.real and self._closed_as_written(Path(f.real)):
            return dataclasses.replace(made, content=False, note="closed to FERB's shell now")
        copy = str(folder / f.copy) if f.copy else None
        if (st.st_size, st.st_mtime_ns) == (f.size, f.mtime_ns) or (copy is not None and _same(path, Path(copy))):
            return dataclasses.replace(made, content=False, note=UNCHANGED)
        return dataclasses.replace(made, before=copy, note=f.no_copy, before_real=f.real or None)

    @staticmethod
    def _fact(path: Path, what: str, st: os.stat_result | None) -> Change:
        return Change(str(path), what, content=False, size=st.st_size if st is not None else None,
                      mtime_ns=st.st_mtime_ns if st is not None else None)

    def _made_under(self, look: _Look, top: Path, where: str) -> None:
        """The plain files under a folder made during the step. It is opened without following a link and checked as
        the folder it really is; every folder under it is opened relative to its parent's fd (O_NOFOLLOW|O_DIRECTORY,
        dir_fd) and checked again before it is listed (os.scandir on the fd), so no folder is ever re-opened by path.
        Files a call named carry content; others go on the check's page, then are counted by name. FERB stops after
        WALK_MAX entries in all and doesn't go deeper than WALK_DEPTH folders (facts on `where` say so)."""
        if look.entries >= WALK_MAX:
            look.stopped = look.stopped or where
            return
        fd = _open(top, directory=True)
        if fd is None:
            return
        opened = self._opened(fd)
        if opened is None:
            os.close(fd)
            return
        stack: list[tuple[Path, Path, int, int, Iterator[tuple[str, bool, bool, int]]]] = []

        def enter(path: Path, real_path: Path, dir_fd: int) -> None:
            try:
                dev = os.fstat(dir_fd).st_dev
                with os.scandir(dir_fd) as it:
                    items = sorted((e.name, e.is_dir(follow_symlinks=False), e.is_file(follow_symlinks=False),
                                    e.inode()) for e in it)
            except OSError:
                os.close(dir_fd)
                return
            stack.append((path, real_path, dir_fd, dev, iter(items)))
        enter(top, opened, fd)
        try:
            while stack:
                path, real_path, dir_fd, dev, items = stack[-1]
                item = next(items, None)
                if item is None:
                    stack.pop()
                    os.close(dir_fd)
                    continue
                if look.entries >= WALK_MAX:
                    look.stopped = look.stopped or where
                    return
                look.entries += 1
                name, is_dir, is_file, ino = item
                if is_dir:
                    if len(stack) >= WALK_DEPTH:
                        look.deep.add(where)
                        continue
                    sub = _open(name, directory=True, dir_fd=dir_fd)
                    if sub is None:
                        continue
                    sub_real = self._opened(sub)
                    if sub_real is None:
                        os.close(sub)
                        continue
                    enter(path / name, sub_real, sub)
                elif is_file:
                    key = str(path / name)
                    if key in look.seen:
                        continue
                    look.seen.add(key)
                    self._made(look, path / name, real_path / name, where, dev, ino,
                               functools.partial(os.stat, name, dir_fd=dir_fd, follow_symlinks=False))
        finally:
            for *_, dir_fd, _, _ in stack:
                os.close(dir_fd)

    def _compare(self, path: Path, f: _File, folder: Path) -> Change | None:
        try:
            st = path.lstat()
        except FileNotFoundError:
            st = None
        except OSError:
            return None
        copy = str(folder / f.copy) if f.copy else None
        if st is not None and not stat.S_ISREG(st.st_mode):           # never read through what replaced it
            if not f.existed:
                return None
            return Change(str(path), "modified", content=False, named=True, note=_what_now(st, path))
        if f.real and self._closed_as_written(Path(f.real)):   # its copy came from a path now closed: never read
            return Change(str(path), "modified", content=False, named=True, note="closed to FERB's shell now")
        if not f.existed:
            return None if st is None else Change(str(path), "created", content=not f.history, size=st.st_size,
                                                  mtime_ns=st.st_mtime_ns, named=True,
                                                  note=HISTORY_NOTE if f.history else "")
        history = f.history or (st is not None and self._history(path))
        if history or f.no_copy == UNREADABLE:          # facts only: a history file, or one FERB can't read (B2)
            note = HISTORY_NOTE if history else UNREADABLE
            if st is None:
                return Change(str(path), "removed", content=False, named=True, note=note)
            if (st.st_size, st.st_mtime_ns, st.st_ino) == (f.size, f.mtime_ns, f.ino):
                return None
            return Change(str(path), "modified", content=False, named=True, size=st.st_size,
                          mtime_ns=st.st_mtime_ns, note=note)
        if st is None:
            return Change(str(path), "removed", copy, note=f.no_copy, named=True, before_real=f.real or None)
        if (st.st_size, st.st_mtime_ns, st.st_ino) == (f.size, f.mtime_ns, f.ino):
            return None
        if copy is not None and _same(path, Path(copy)):
            return None                                     # touched, same bytes
        return Change(str(path), "modified", copy, note=f.no_copy, size=st.st_size, mtime_ns=st.st_mtime_ns,
                      named=True, before_real=f.real or None)

    def _paired(self, made: Change, all_changes: list[Change], folder: Path, rec: _Record) -> Change:
        """A new file with the most similar original of its kind (SIMILAR at least), among at most PAIR_READS
        candidates chosen before any text is read, each within PAIR_SIZE of its size. First the step's own copies (the
        files its calls named, whose sources aren't closed now), whatever their names: alike names (SIMILAR) the most
        alike first, then the rest the closest in size first. Then plain files beside it with alike names, the most
        alike first."""
        path = Path(made.path)
        kind, suffix = _kind_of(path), path.suffix.lower()
        try:
            size = path.lstat().st_size
        except OSError:
            return made

        def likeness(other: Path) -> float:
            return difflib.SequenceMatcher(None, other.stem.lower(), path.stem.lower()).ratio()
        own: list[tuple[tuple[int, float], str, Path, bool, str | None]] = []   # (rank, original, readable, current, real)
        for key, f in rec.files.items():
            if f.copy and key != made.path and _kind_of(Path(key)) == kind and size / PAIR_SIZE <= f.size \
                    <= size * PAIR_SIZE and not (f.real and self._closed_as_written(Path(f.real))):
                alike = likeness(Path(key))
                rank = (0, -alike) if alike >= SIMILAR else (1, float(abs(f.size - size)))
                own.append((rank, key, folder / f.copy, False, f.real or None))
        picks = [c[1:] for c in sorted(own, key=lambda c: (c[0], c[1]))[:PAIR_READS]]
        changed = {c.path for c in all_changes}
        siblings: list[tuple[float, str, Path, bool, str | None]] = []
        try:
            with os.scandir(path.parent) as it:
                for e in it:
                    sibling = Path(e.path)
                    if (sibling.suffix.lower() != suffix or e.path in changed or sibling == path
                            or e.path in rec.files or not e.is_file(follow_symlinks=False)):
                        continue
                    if size / PAIR_SIZE <= e.stat(follow_symlinks=False).st_size <= size * PAIR_SIZE \
                            and (alike := likeness(sibling)) >= SIMILAR:
                        siblings.append((alike, str(sibling), sibling, True, None))
        except OSError:
            pass
        picks += [c[1:] for c in sorted(siblings, key=lambda c: (-c[0], c[1]))][:PAIR_READS - len(picks)]
        if not picks:
            return made
        new = self.text_of(path)
        if new is None or not new[0]:
            return made
        best: tuple[float, str, Path, str | None] | None = None
        lines = new[0].splitlines()
        for original, source, current, real_from in picks:
            if current and (self._skip(source) or self._history(source)):
                continue
            old = self.text_of(source) if current else _text(source, None)
            if old is None or not old[0]:
                continue
            m = difflib.SequenceMatcher(None, old[0].splitlines(), lines, autojunk=False)
            if m.real_quick_ratio() < SIMILAR or m.quick_ratio() < SIMILAR:
                continue
            ratio = m.ratio()
            if ratio >= SIMILAR and (best is None or ratio > best[0]):
                best = (ratio, original, source, real_from)
        if best is None:
            return made
        return dataclasses.replace(made, before=str(best[2]), source=best[1], before_real=best[3])

    # reading
    def text_of(self, path: Path) -> tuple[str, list[Annotation]] | None:
        """What a current file says (`text_of`), opened without following a link and only when the file it really
        opens isn't closed to the shell's sandbox."""
        return _text(path, lambda opened: self._skip(opened))

    def describe(self, change: Change) -> str:
        """One change as the checker reads it (module `describe`), its current file read through `text_of`, and FERB's
        copy read only while the file it was taken from is still open to the shell."""
        if change.before_real and self._closed_as_written(Path(change.before_real)):
            change = dataclasses.replace(change, before=None, note="its copy came from a path closed to FERB's shell")
        return describe(change, self.text_of)

    def forget(self, tree_id: str) -> list[str]:
        """The tree ended: its copies and records go. Returns what couldn't be removed (for FERB's journal)."""
        with self._guard:
            for task_id in [t for t, r in self._records.items() if r.tree_id == tree_id]:
                self._records.pop(task_id, None)
                self._saved.pop(task_id, None)
        return _remove(self.root / self._key(tree_id))

    def sweep(self, keep: Callable[[str], bool] = lambda tree_id: False) -> list[str]:
        """At FERB's start, before any task: what a stopped daemon left, except the records of trees that will resume
        (`keep(tree_id)`), so their checks still see the changes made before the restart. Returns what couldn't be
        removed."""
        failed: list[str] = []
        with self._guard:
            self._records.clear()
            self._saved.clear()
        if not self.root.is_dir():
            return failed
        for child in self.root.iterdir():
            if child.is_dir() and not child.is_symlink():
                if any(t and keep(t) for t in _trees_in(child)):
                    continue
                failed += _remove(child)
            else:
                try:
                    child.unlink(missing_ok=True)
                except OSError:
                    failed.append(str(child))
        return failed


def _trees_in(folder: Path) -> set[str]:
    """The task trees a tree folder's records belong to (read from each task's index)."""
    trees = set()
    for index in folder.glob(f"*/{INDEX}"):
        try:
            trees.add(str(json.loads(index.read_text()).get("tree_id", "")))
        except (OSError, ValueError, AttributeError):
            continue
    return trees


def _nearest(path: Path) -> Path | None:
    """The nearest folder above a missing path that exists (a file may be made under new folders there)."""
    for parent in path.parents:
        if parent.is_dir() and not parent.is_symlink():
            return parent
    return None


def _remove(folder: Path) -> list[str]:
    """Remove a folder of FERB's copies; a locked copy is unlocked first. Returns the paths that stayed."""
    failed: list[str] = []

    def retry(fn: Callable[..., Any], where: str, exc: BaseException) -> None:
        recovery._unlock(Path(where))
        try:
            fn(where)
        except OSError:
            failed.append(where)
    if folder.exists():
        shutil.rmtree(folder, onexc=retry)
    return failed


def real(path: Path) -> Path:
    """The file a path really reaches: its folder's links followed, the name kept (a link itself is the file)."""
    p = Path(os.path.expanduser(str(path)))
    return Path(os.path.realpath(p.parent)) / p.name if p.name else Path(os.path.realpath(p))


def _read(path: Path, closed: Callable[[Path], bool] | None, limit: int | None = None) -> bytes | None:
    """A plain file's bytes, opened without following a link (O_NOFOLLOW) and non-blocking (a FIFO or device never
    holds the check), checked as a plain file before any read; with `closed`, only when the file it really opens to
    isn't closed. None when it is a link, not a plain file, closed, larger than FILE_MAX, or unreadable."""
    try:
        fd = os.open(path, _NOFOLLOW)
    except OSError:
        return None
    try:
        st = os.fstat(fd)
        if not stat.S_ISREG(st.st_mode) or st.st_size > FILE_MAX:
            return None
        if closed is not None and closed(opened_path(fd)):
            return None
        chunks, left = [], limit if limit is not None else st.st_size + 1
        while left > 0 and (block := os.read(fd, min(left, 1 << 20))):
            chunks.append(block)
            left -= len(block)
        return b"".join(chunks)
    except OSError:
        return None
    finally:
        os.close(fd)


def _same(a: Path, b: Path) -> bool:
    """Whether a current plain file (never through a link) has the same bytes as FERB's copy. Both are opened
    non-blocking and checked as plain files before any read (a FIFO or device never holds the check)."""
    try:
        fa = os.open(a, _NOFOLLOW)
    except OSError:
        return False
    try:
        fb = os.open(b, _NOFOLLOW)
    except OSError:
        os.close(fa)
        return False
    try:
        sa, sb = os.fstat(fa), os.fstat(fb)
        if not (stat.S_ISREG(sa.st_mode) and stat.S_ISREG(sb.st_mode)) or sa.st_size != sb.st_size:
            return False
        while True:
            x, y = os.read(fa, 1 << 20), os.read(fb, 1 << 20)
            if x != y:
                return False
            if not x:
                return True
    except OSError:
        return False
    finally:
        os.close(fa)
        os.close(fb)


# ---------- what a file says ----------

def _docx(data: bytes) -> str | None:
    """A DOCX's text: one line per paragraph (each table cell's own), headers and footers after the body, then its
    document properties (title, author, last modified by)."""
    try:
        with zipfile.ZipFile(io.BytesIO(data)) as z:
            names = z.namelist()
            if "word/document.xml" not in names:
                return None
            lines: list[str] = []
            for name in ["word/document.xml"] + sorted(n for n in names if _PARTS.fullmatch(n)):
                if z.getinfo(name).file_size > XML_MAX:
                    lines.append(f"[{name}: too large to read]")
                    continue
                if name != "word/document.xml":
                    lines.append(f"[{name.rpartition('/')[2].removesuffix('.xml')}]")
                lines += _paragraphs(ET.fromstring(z.read(name)))
            if "docProps/core.xml" in names and z.getinfo("docProps/core.xml").file_size <= XML_MAX:
                for node in ET.fromstring(z.read("docProps/core.xml")):
                    if node.tag in _PROPS and (value := (node.text or "").strip()):
                        lines.append(f"[document property] {_PROPS[node.tag]}: {value}")
            return "\n".join(lines)
    except (OSError, zipfile.BadZipFile, ET.ParseError, KeyError, ValueError):
        return None


def _paragraphs(root: ET.Element) -> list[str]:
    lines: list[str] = []

    def walk(node: ET.Element, buf: list[str]) -> None:
        for child in node:
            if child.tag == _W + "p":
                inner: list[str] = []
                walk(child, inner)
                lines.append("".join(inner))
            elif child.tag == _W + "t":
                buf.append(child.text or "")
            elif child.tag == _W + "tab":
                buf.append("\t")
            elif child.tag in (_W + "br", _W + "cr"):
                buf.append("\n")
            elif child.tag != _W + "delText":          # tracked deletions are not the text
                walk(child, buf)
    walk(root, [])
    return lines



def _pdf(data: bytes) -> tuple[str, list[Annotation]] | None:
    """A PDF's text layer, page by page, and its annotations (page, type, text), through PDFKit from the bytes FERB
    read (PDFKit never opens a path itself, so it can't follow a link)."""
    try:
        import objc
        from Foundation import NSData
        from Quartz import PDFDocument
    except ImportError:
        return None
    with objc.autorelease_pool():
        doc = PDFDocument.alloc().initWithData_(NSData.dataWithBytes_length_(data, len(data)))
        if doc is None:
            return None
        pages: list[str] = []
        notes: list[Annotation] = []
        for i in range(int(doc.pageCount())):
            page = doc.pageAtIndex_(i)
            if page is None:
                continue
            pages.append(f"[page {i + 1}]\n{page.string() or ''}")
            for a in page.annotations() or ():
                notes.append((i + 1, str(a.type() or "annotation"), " ".join(str(a.contents() or "").split())))
        return "\n".join(pages), notes


def _text(path: Path, closed: Callable[[Path], bool] | None) -> tuple[str, list[Annotation]] | None:
    """What a plain file says, for comparing: (text, PDF annotations), read through `_read` (no link followed; with
    `closed`, nothing a closed path holds). None for a file with no text FERB can read."""
    suffix = path.suffix.lower()
    if suffix in DOCUMENTS:
        data = _read(path, closed)
        if data is None:
            return None
        if suffix == ".pdf":
            return _pdf(data)
        text = _docx(data)
        return (text, []) if text is not None else None
    data = _read(path, closed, TEXT_MAX * 4 + 1)
    if data is None or b"\0" in data[:8192]:
        return None
    try:
        return data.decode("utf-8"), []
    except UnicodeDecodeError:
        return None


def text_of(path: Path) -> tuple[str, list[Annotation]] | None:
    """What a file says, for comparing (no link followed). `Changes.text_of` also refuses closed paths."""
    return _text(path, None)


def _annotations(old: list[Annotation], new: list[Annotation]) -> str:
    added = list((Counter(new) - Counter(old)).elements())
    removed = list((Counter(old) - Counter(new)).elements())

    def shown(items: list[Annotation]) -> str:
        return "; ".join(f"page {p}: {t}" + (f" {json.dumps(x, ensure_ascii=False)}" if x else "")
                         for p, t, x in sorted(items))
    parts = ([f"{len(added)} annotation{'s' if len(added) != 1 else ''} added ({shown(added)})"] if added else []) + \
            ([f"{len(removed)} annotation{'s' if len(removed) != 1 else ''} removed ({shown(removed)})"]
             if removed else [])
    return ", ".join(parts)


def _when(ns: int | None) -> str:
    if ns is None:
        return ""
    from datetime import datetime
    return ", modified " + datetime.fromtimestamp(ns / 1e9).astimezone().isoformat(timespec="seconds")


def describe(change: Change, read: Callable[[Path], tuple[str, list[Annotation]] | None] = text_of) -> str:
    """One change as the checker reads it. A fact-only change: what happened, its size and time, and why no content
    is shown. Otherwise the old-vs-new diff of its text (a new file against the original it resembles), a PDF's
    annotations added or removed, or why there is no diff. `read` reads the current file (`Changes.text_of`)."""
    path = Path(change.path)
    verb = {"created": "new", "removed": "removed", "modified": "changed",
            "renamed": f"renamed from {change.source}"}.get(change.what, change.what)
    if not change.content and change.named:          # named by the step, but nothing FERB may read now
        size = f", {change.size:,} bytes" if change.size is not None else ""
        return (f"{change.path}: {verb}, named by the call{size}{_when(change.mtime_ns)}; "
                f"{change.note or 'not a plain file now'}" + ("" if change.note == UNCHANGED else "; not read"))
    if not change.content:
        size = f", {change.size:,} bytes" if change.size is not None else ""
        how = change.note or ("not shown: no call of this step named it; it may be the owner's or another task's"
                              if change.what != "modified" or change.before is None else "not shown")
        return f"{change.path}: {verb} during this step{size}{_when(change.mtime_ns)} ({how})"
    if change.what == "removed":
        head = f"{change.path}: removed (no longer at this path)"
        return head + (f"; {change.note}" if change.note else "")
    if change.what == "created" and change.source is None:
        new = read(path)
        if new is None:
            return f"{change.path}: new, named by this step (no text FERB can read)"
        # N6: a text file's first part (fs_read reads the rest); a DOCX's or PDF's whole text, which only FERB extracts
        cut = path.suffix.lower() not in DOCUMENTS and len(new[0]) > PREVIEW_CHARS
        body = new[0][:PREVIEW_CHARS] + "\n[… the file holds the rest: read it with fs_read]" if cut else new[0]
        notes = f"\nPDF annotations: {_annotations([], new[1])}" if new[1] else ""
        return f"{change.path}: new, named by this step. Its text{' (the first part)' if cut else ''}:\n{body}{notes}"
    if change.before is None:
        return f"{change.path}: {verb}; no diff: {change.note or 'FERB kept no copy from before'}"
    old, new = _text(Path(change.before), None), read(path)        # the copy is FERB's own; the file is read safely
    if change.what == "created":
        head = f"{change.path}: new, named by this step; compared with {change.source} as it was before the step"
    elif change.what == "renamed":
        head = f"{change.path}: {verb}, named by this step; compared with FERB's copy from before the step"
    else:
        head = f"{change.path}: changed, named by this step; compared with FERB's copy from before the step"
    if old is None or new is None:
        sizes = ""
        try:
            sizes = f" ({Path(change.before).stat().st_size:,} bytes before, {path.lstat().st_size:,} now)"
        except OSError:
            pass
        return f"{head}\nNo text FERB can read in it{sizes}; its bytes changed."
    if len(old[0]) > TEXT_MAX or len(new[0]) > TEXT_MAX:
        return f"{head}\nIts text is too long to compare here; read the file itself."
    lines = list(difflib.unified_diff(old[0].splitlines(), new[0].splitlines(), "before", "now", lineterm="",
                                      n=CONTEXT_LINES))
    notes = _annotations(old[1], new[1])
    if path.suffix.lower() == ".pdf":
        layer = "text layer unchanged" if not lines else "text layer changed"
        summary = f"PDF: {notes + ', ' if notes else 'no annotations added or removed, '}{layer}"
        return head + "\n" + summary + ("\n" + "\n".join(lines) if lines else "")
    if not lines:
        return f"{head}\nIts text is unchanged (only its formatting or other bytes changed)."
    return head + "\n" + "\n".join(lines)


def shown(path: str) -> str:
    """A path as the owner writes it (~ for the home folder), for an owner-facing line."""
    return recovery.short(path)
