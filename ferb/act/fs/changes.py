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
from typing import Any, Callable, Iterable

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
    """One file that changed during the task, as facts: `what` is modified, created or removed; `before` the copy FERB
    kept from before (None: none was kept, `note` says why); `source` for a new file, the file it was compared with.
    `content` False: a fact only (a file the step's calls didn't name, a shell history file, a link): its name, size
    and time, never what it says."""
    path: str
    what: str
    before: str | None = None
    source: str | None = None
    note: str = ""
    content: bool = True
    size: int | None = None
    mtime_ns: int | None = None


def _kind(st: os.stat_result) -> int:
    return 1 if stat.S_ISREG(st.st_mode) else 2 if stat.S_ISDIR(st.st_mode) else 3


def _what_now(st: os.stat_result, path: Path) -> str:
    """A path that is no longer a plain file, in words (no content is read)."""
    if stat.S_ISLNK(st.st_mode):
        try:
            return f"replaced by a link to {os.readlink(path)}"
        except OSError:
            return "replaced by a link"
    return "now a folder" if stat.S_ISDIR(st.st_mode) else "now a special file (not a plain file)"


class Changes:
    """The before-and-after record. `closed(path)`: whether FERB's shell sandbox denies reading the path (FERB's own
    state, the never-read stores: `Protection.read_denied`); the record never reads, copies or lists such a path,
    whatever the call named, and checks the path a file really opens to before reading it. `home`: the owner's home,
    for the shell history files whose text is never shown (rule 14)."""

    def __init__(self, root: Path, closed: Callable[[Path], bool] | None = None, home: Path | None = None) -> None:
        self.root, self.closed = root, closed or (lambda path: False)
        home = home or Path.home()
        self.history = (home / ".zsh_history", home / ".bash_history")
        self.sessions = home / ".zsh_sessions"
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

    def _history_file(self, path: Path) -> bool:
        """A shell history file (the HISTFILE defaults): what the owner typed into a terminal, never shown (rule 14)."""
        return path in self.history or path == self.sessions or self.sessions in path.parents

    # before a call
    def before(self, task_id: str, tree_id: str, paths: Iterable[Path]) -> dict[str, int]:
        """Before a call that may change these paths (each named exactly by the call): each existing plain file not yet
        recorded for the task is copied and its folder listed; a missing one is remembered as named and its nearest
        existing folder listed (a file made there, or under new folders there, shows as new); a named folder is
        listed. A path the shell's sandbox denies to reads is never touched (counted as protected). Returns counts
        for the call's record. Never raises for one path."""
        with self._lock(task_id):
            rec = self._record(task_id, tree_id)
            assert rec is not None
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
                    if os.access(path, os.W_OK):
                        self._list(rec, path, counts)
                elif stat.S_ISREG(st.st_mode):
                    if os.access(path.parent, os.W_OK):
                        self._list(rec, path.parent, counts)
                    self._file(rec, folder, path, counts)
            self._save(rec)
            return counts

    def _list(self, rec: _Record, folder: Path, counts: dict[str, int]) -> bool:
        """The folder's own entries, once per task (its listing from before the task first touched it)."""
        key = str(folder)
        if key in rec.folders:
            return True
        if self._skip(folder):
            return False
        entries: dict[str, list[int]] = {}
        try:
            with os.scandir(folder) as it:
                for e in it:
                    try:
                        st = e.stat(follow_symlinks=False)
                    except OSError:
                        continue
                    entries[e.name] = [_kind(st), st.st_size, st.st_mtime_ns, st.st_ino]
        except OSError:
            return False
        rec.folders[key] = entries
        counts["folders"] += 1
        return True

    def _file(self, rec: _Record, folder: Path, path: Path, counts: dict[str, int]) -> None:
        key = str(path)
        if key in rec.files or self._skip(path):
            return
        try:
            st = path.lstat()
        except OSError:
            return
        if not stat.S_ISREG(st.st_mode) or not (os.access(path, os.W_OK) or os.access(path.parent, os.W_OK)):
            return                                     # a file FERB couldn't change
        rec.n += 1
        name = f"c{rec.n}" + (path.suffix if _SUFFIX.fullmatch(path.suffix) else "")   # the type tells how to read it
        entry = _File(True, st.st_size, st.st_mtime_ns, st.st_ino)
        if not self._history_file(path):                # a history file's text is never kept, so never diffed
            try:
                recovery._clone(path, folder / name)
                recovery._unlock(folder / name)         # a locked original's clone could never be removed
                entry.copy = name
                counts["copied"] += 1
            except OSError as e:
                entry.no_copy = ("this volume can't make an instant copy" if e.errno in (errno.EXDEV, errno.ENOTSUP)
                                 else f"FERB couldn't copy it ({e.strerror or e})")
                counts["not_copied"] += 1
        rec.files[key] = entry

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
        (content) or, with no copy, its size, time and identity; new, changed and removed files in each listed folder
        (and plain files under folders made there). A file the calls named exactly carries content; anything else
        that changed in a listed folder is a fact only, as is a shell history file or a path that is no longer a plain
        file. Nothing the shell's sandbox denies to reads appears."""
        with self._lock(task_id):
            rec = self._record(task_id, tree_id)
            if rec is None:
                return []
            folder = self._folder(rec.tree_id, task_id)
            out: list[Change] = []
            seen: set[str] = set()
            for key, f in rec.files.items():
                seen.add(key)
                if (c := self._compare(Path(key), f, folder)) is not None:
                    out.append(c)
            for where, entries in rec.folders.items():
                try:
                    now = {e.name: e for e in os.scandir(where)}
                except OSError:
                    now = {}
                for name in sorted(set(entries) | set(now)):
                    path = Path(where) / name
                    key = str(path)
                    if key in seen or self._skip(path):
                        continue
                    seen.add(key)
                    old, new = entries.get(name), now.get(name)
                    if new is None:
                        if old is not None and old[0] == 1:
                            out.append(self._fact(path, "removed", None))
                        continue
                    try:
                        st = new.stat(follow_symlinks=False)
                    except OSError:
                        continue
                    if stat.S_ISDIR(st.st_mode) and old is None:     # a folder made here: the plain files in it
                        out += self._made_under(rec, path, seen)
                    elif stat.S_ISREG(st.st_mode) and old is None:
                        out.append(Change(key, "created", size=st.st_size, mtime_ns=st.st_mtime_ns)
                                   if self._shows(rec, path) else self._fact(path, "created", st))
                    elif stat.S_ISREG(st.st_mode) and [1, st.st_size, st.st_mtime_ns, st.st_ino] != old:
                        out.append(self._fact(path, "modified", st))
            return [self._paired(c, out, folder, rec) if c.what == "created" and c.content else c for c in out]

    def _shows(self, rec: _Record, path: Path) -> bool:
        """Whether a new file's content is shown: a call named it exactly, and it isn't a shell history file."""
        return str(path) in rec.named and not self._history_file(path)

    @staticmethod
    def _fact(path: Path, what: str, st: os.stat_result | None) -> Change:
        return Change(str(path), what, content=False, size=st.st_size if st is not None else None,
                      mtime_ns=st.st_mtime_ns if st is not None else None)

    def _made_under(self, rec: _Record, top: Path, seen: set[str]) -> list[Change]:
        """The plain files under a folder made during the step (never following links, never into a skipped path)."""
        out: list[Change] = []
        for here, dirs, names in os.walk(top, followlinks=False):
            dirs[:] = [d for d in dirs if not self._skip(Path(here) / d)]
            for name in sorted(names):
                path = Path(here) / name
                if str(path) in seen or self._skip(path):
                    continue
                seen.add(str(path))
                try:
                    st = path.lstat()
                except OSError:
                    continue
                if stat.S_ISREG(st.st_mode):
                    out.append(Change(str(path), "created", size=st.st_size, mtime_ns=st.st_mtime_ns)
                               if self._shows(rec, path) else self._fact(path, "created", st))
        return out

    def _compare(self, path: Path, f: _File, folder: Path) -> Change | None:
        try:
            st = path.lstat()
        except FileNotFoundError:
            st = None
        except OSError:
            return None
        copy = str(folder / f.copy) if f.copy else None
        history = self._history_file(path)
        if st is not None and not stat.S_ISREG(st.st_mode):           # never read through what replaced it
            if not f.existed:
                return None
            return Change(str(path), "modified", content=False, note=_what_now(st, path))
        if not f.existed:
            return None if st is None else Change(str(path), "created", content=not history, size=st.st_size,
                                                  mtime_ns=st.st_mtime_ns)
        if st is None:
            return Change(str(path), "removed", copy, note=f.no_copy, content=not history)
        if (st.st_size, st.st_mtime_ns, st.st_ino) == (f.size, f.mtime_ns, f.ino):
            return None
        if copy is not None and _same(path, Path(copy)):
            return None                                     # touched, same bytes
        if history:
            return self._fact(path, "modified", st)
        return Change(str(path), "modified", copy, note=f.no_copy, size=st.st_size, mtime_ns=st.st_mtime_ns)

    def _paired(self, made: Change, all_changes: list[Change], folder: Path, rec: _Record) -> Change:
        """A new file with the most similar original of its kind (SIMILAR at least): first the copies FERB kept of the
        files the step named, then at most PAIR_READS plain files beside it whose names are like its name and whose
        sizes are near its size (filtered before any text is read)."""
        path = Path(made.path)
        new = self.text_of(path)
        if new is None or not new[0]:
            return made
        suffix = path.suffix.lower()
        candidates: list[tuple[str, Path, bool]] = [
            (key, folder / f.copy, False) for key, f in rec.files.items()
            if f.copy and Path(key).suffix.lower() == suffix and key != made.path]
        changed = {c.path for c in all_changes}
        try:
            size = path.lstat().st_size
            near: list[tuple[float, Path]] = []
            with os.scandir(path.parent) as it:
                for e in it:
                    sibling = Path(e.path)
                    if (sibling.suffix.lower() != suffix or e.path in changed or sibling == path
                            or e.path in rec.files or not e.is_file(follow_symlinks=False)):
                        continue
                    alike = difflib.SequenceMatcher(None, sibling.stem.lower(), path.stem.lower()).ratio()
                    sz = e.stat(follow_symlinks=False).st_size
                    if alike >= SIMILAR and size / PAIR_SIZE <= sz <= size * PAIR_SIZE and not self._skip(sibling) \
                            and not self._history_file(sibling):
                        near.append((alike, sibling))
            candidates += [(str(p), p, True) for _, p in sorted(near, reverse=True)[:PAIR_READS]]
        except OSError:
            pass
        best: tuple[float, str, Path] | None = None
        lines = new[0].splitlines()
        for original, source, current in candidates:
            old = self.text_of(source) if current else _text(source, None)
            if old is None or not old[0]:
                continue
            m = difflib.SequenceMatcher(None, old[0].splitlines(), lines, autojunk=False)
            if m.real_quick_ratio() < SIMILAR or m.quick_ratio() < SIMILAR:
                continue
            ratio = m.ratio()
            if ratio >= SIMILAR and (best is None or ratio > best[0]):
                best = (ratio, original, source)
        if best is None:
            return made
        return dataclasses.replace(made, before=str(best[2]), source=best[1])

    # reading
    def text_of(self, path: Path) -> tuple[str, list[Annotation]] | None:
        """What a current file says (`text_of`), opened without following a link and only when the file it really
        opens isn't closed to the shell's sandbox."""
        return _text(path, lambda opened: self._skip(opened))

    def describe(self, change: Change) -> str:
        """One change as the checker reads it (module `describe`), its current file read through `text_of`."""
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
    """A plain file's bytes, opened without following a link (O_NOFOLLOW); with `closed`, only when the file it really
    opens to isn't closed. None when it is a link, not a plain file, closed, larger than FILE_MAX, or unreadable."""
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
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
    """Whether a current plain file (never through a link) has the same bytes as FERB's copy."""
    try:
        fa = os.open(a, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    except OSError:
        return False
    try:
        fb = os.open(b, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    except OSError:
        os.close(fa)
        return False
    try:
        if not stat.S_ISREG(os.fstat(fa).st_mode) or os.fstat(fa).st_size != os.fstat(fb).st_size:
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
    if suffix in (".pdf", ".docx", ".docm", ".dotx"):
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
    if not change.content:
        size = f", {change.size:,} bytes" if change.size is not None else ""
        how = change.note or ("not shown: no call of this step named it; it may be the owner's or another task's"
                              if change.what != "modified" or change.before is None else "not shown")
        verb = {"created": "new", "removed": "removed", "modified": "changed"}.get(change.what, change.what)
        return f"{change.path}: {verb} during this step{size}{_when(change.mtime_ns)} ({how})"
    if change.what == "removed":
        head = f"{change.path}: removed (no longer at this path)"
        return head + (f"; {change.note}" if change.note else "")
    if change.what == "created" and change.source is None:
        new = read(path)
        if new is None:
            return f"{change.path}: new, named by this step (no text FERB can read)"
        body = new[0] if len(new[0]) <= TEXT_MAX else new[0][:TEXT_MAX] + "\n[… cut here; read the file for the rest]"
        notes = f"\nPDF annotations: {_annotations([], new[1])}" if new[1] else ""
        return f"{change.path}: new, named by this step. Its text:\n{body}{notes}"
    if change.before is None:
        return f"{change.path}: changed; no diff: {change.note or 'FERB kept no copy from before'}"
    old, new = _text(Path(change.before), None), read(path)        # the copy is FERB's own; the file is read safely
    if change.what == "created":
        head = f"{change.path}: new, named by this step; compared with {change.source} as it was before the step"
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
