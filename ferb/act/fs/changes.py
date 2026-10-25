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

import difflib
import errno
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

SIMILAR = 0.5               # a made file is shown against a file at least this alike (git's default copy/rename bar)
TEXT_MAX = 2_000_000        # display only: characters of extracted text compared per side; a longer text is not
                            # diffed here (the record says so and the checker reads the file); it decides nothing
XML_MAX = 50_000_000        # a DOCX part declaring more than this uncompressed is not inflated (a zip bomb)
CONTEXT_LINES = 2           # unchanged lines shown around each change in a diff
INDEX = "index.json"
_W = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"
_PROPS = {"{http://purl.org/dc/elements/1.1/}title": "title", "{http://purl.org/dc/elements/1.1/}subject": "subject",
          "{http://purl.org/dc/elements/1.1/}creator": "creator", "{http://purl.org/dc/elements/1.1/}description":
          "description", "{http://schemas.openxmlformats.org/package/2006/metadata/core-properties}lastModifiedBy":
          "last modified by", "{http://schemas.openxmlformats.org/package/2006/metadata/core-properties}keywords":
          "keywords"}
_PARTS = re.compile(r"word/(header|footer|footnotes|endnotes)\d*\.xml")

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
    n: int = 0


@dataclass(frozen=True)
class Change:
    """One file the task changed, as facts: `what` is modified, created or removed; `before` the copy FERB kept from
    before (None: none was kept, `note` says why); `source` for a made file, the file it was compared with."""
    path: str
    what: str
    before: str | None = None
    source: str | None = None
    note: str = ""


def _kind(st: os.stat_result) -> int:
    return 1 if stat.S_ISREG(st.st_mode) else 2 if stat.S_ISDIR(st.st_mode) else 3


class Changes:
    """The before-and-after record. `closed(path)`: the protection lists' answer for reading a path (FERB's state, a
    never-read store); such a path is never copied or listed."""

    def __init__(self, root: Path, closed: Callable[[Path], bool] | None = None) -> None:
        self.root, self.closed = root, closed or (lambda path: False)
        self._records: dict[str, _Record] = {}
        self._lock = threading.Lock()

    # where things live
    @staticmethod
    def _key(text: str) -> str:
        return hashlib.sha256(text.encode()).hexdigest()[:24]

    def _folder(self, tree_id: str, task_id: str) -> Path:
        return self.root / self._key(tree_id) / self._key(task_id)

    def _record(self, task_id: str, tree_id: str | None = None) -> _Record | None:
        rec = self._records.get(task_id)
        if rec is not None or tree_id is None:
            return rec
        index = self._folder(tree_id, task_id) / INDEX           # after a restart: the record on disk
        try:
            data = json.loads(index.read_text())
            rec = _Record(data["task_id"], data["tree_id"], {k: _File(**v) for k, v in data["files"].items()},
                          data["folders"], int(data["n"]))
        except (OSError, ValueError, KeyError, TypeError):
            rec = _Record(task_id, tree_id)
        self._records[task_id] = rec
        return rec

    def _save(self, rec: _Record) -> None:
        folder = self._folder(rec.tree_id, rec.task_id)
        tmp = folder / (INDEX + ".tmp")
        tmp.write_text(json.dumps({"task_id": rec.task_id, "tree_id": rec.tree_id, "n": rec.n, "folders": rec.folders,
                                   "files": {k: asdict(v) for k, v in rec.files.items()}}))
        os.replace(tmp, folder / INDEX)

    def _skip(self, path: Path) -> bool:
        parts = path.parts
        return (path == Path("/") or parts[:2] == ("/", "dev") or any(p in (".Trash", ".Trashes") for p in parts)
                or self.closed(path))

    # before a call
    def before(self, task_id: str, tree_id: str, paths: Iterable[Path]) -> dict[str, int]:
        """Before a call that may change these paths: each existing file not yet recorded for the task is copied, each
        missing one noted as missing; each named folder is listed and its own files copied; each file's folder listed.
        Returns counts for the call's record. Never raises for one path: a path FERB can't look at is skipped."""
        with self._lock:
            rec = self._record(task_id, tree_id)
            assert rec is not None
            folder = self._folder(tree_id, task_id)
            folder.mkdir(mode=0o700, parents=True, exist_ok=True)
            counts = {"copied": 0, "not_copied": 0, "folders": 0}
            for raw in paths:
                try:
                    path = real(raw)
                except (OSError, RuntimeError, ValueError):
                    continue
                if self._skip(path):
                    continue
                try:
                    st = path.lstat()
                except FileNotFoundError:
                    if path.parent.is_dir() and os.access(path.parent, os.W_OK):
                        self._list(rec, path.parent, counts)
                        rec.files.setdefault(str(path), _File(False))
                    continue
                except OSError:
                    continue
                if stat.S_ISDIR(st.st_mode):
                    if os.access(path, os.W_OK) and self._list(rec, path, counts):
                        for name, entry in list(rec.folders[str(path)].items()):
                            if entry[0] == 1:
                                self._file(rec, folder, path / name, counts)
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
        try:
            recovery._clone(path, folder / name)
            entry.copy = name
            counts["copied"] += 1
        except OSError as e:
            entry.no_copy = ("this volume can't make an instant copy" if e.errno in (errno.EXDEV, errno.ENOTSUP)
                             else f"FERB couldn't copy it ({e.strerror or e})")
            counts["not_copied"] += 1
        rec.files[key] = entry

    def reset(self, task_id: str) -> None:
        """A loop round starts its own record (its check judges that round only)."""
        with self._lock:
            rec = self._records.pop(task_id, None)
            if rec is not None:
                shutil.rmtree(self._folder(rec.tree_id, task_id), ignore_errors=True)

    # at the check
    def changes(self, task_id: str, tree_id: str | None = None) -> list[Change]:
        """What changed since the task first touched each recorded file or folder, by comparing it with its copy
        (content) or, with no copy, its size, time and identity; created and removed files in each listed folder."""
        with self._lock:
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
                    key = str(Path(where) / name)
                    if key in seen:
                        continue
                    seen.add(key)
                    old, new = entries.get(name), now.get(name)
                    if new is not None:
                        try:
                            st = new.stat(follow_symlinks=False)
                        except OSError:
                            continue
                        if not stat.S_ISREG(st.st_mode):
                            continue
                        if old is None:
                            out.append(Change(key, "created"))
                        elif [1, st.st_size, st.st_mtime_ns, st.st_ino] != old:
                            out.append(Change(key, "modified", note="FERB kept no copy from before (it wasn't named)"))
                    elif old is not None and old[0] == 1:
                        out.append(Change(key, "removed", note="FERB kept no copy from before (it wasn't named)"))
            return [self._paired(c, out, folder) if c.what == "created" else c for c in out]

    def _compare(self, path: Path, f: _File, folder: Path) -> Change | None:
        try:
            st = path.lstat()
        except FileNotFoundError:
            st = None
        except OSError:
            return None
        copy = str(folder / f.copy) if f.copy else None
        if not f.existed:
            return Change(str(path), "created") if st is not None and stat.S_ISREG(st.st_mode) else None
        if st is None:
            return Change(str(path), "removed", copy, note=f.no_copy)
        if (st.st_size, st.st_mtime_ns, st.st_ino) == (f.size, f.mtime_ns, f.ino):
            return None
        if copy is not None and stat.S_ISREG(st.st_mode) and _same(path, Path(copy)):
            return None                                     # touched, same bytes
        return Change(str(path), "modified", copy, note=f.no_copy)

    def _paired(self, made: Change, all_changes: list[Change], folder: Path) -> Change:
        """A made file with the most similar original of its kind (SIMILAR at least): a copy FERB kept of a file the
        step changed or removed, or an unchanged file beside it."""
        path = Path(made.path)
        new = text_of(path)
        if new is None or not new[0]:
            return made
        candidates: list[tuple[str, Path]] = [(c.path, Path(c.before)) for c in all_changes
                                              if c.before and Path(c.path).suffix.lower() == path.suffix.lower()]
        changed = {c.path for c in all_changes}
        try:
            for sibling in path.parent.iterdir():
                if (sibling.suffix.lower() == path.suffix.lower() and str(sibling) not in changed
                        and sibling != path and sibling.is_file()):
                    candidates.append((str(sibling), sibling))
        except OSError:
            pass
        best: tuple[float, str, Path] | None = None
        lines = new[0].splitlines()
        for original, source in candidates:
            old = text_of(source)
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
        return Change(made.path, "created", str(best[2]), best[1])

    def forget(self, tree_id: str) -> None:
        """The tree ended: its copies and records go."""
        with self._lock:
            for task_id in [t for t, r in self._records.items() if r.tree_id == tree_id]:
                self._records.pop(task_id, None)
            shutil.rmtree(self.root / self._key(tree_id), ignore_errors=True)

    def sweep(self) -> None:
        """At FERB's start, before any task: what a stopped daemon left."""
        with self._lock:
            self._records.clear()
            if self.root.is_dir():
                for child in self.root.iterdir():
                    shutil.rmtree(child, ignore_errors=True) if child.is_dir() else child.unlink(missing_ok=True)


def real(path: Path) -> Path:
    """The file a path really reaches: its folder's links followed, the name kept (a link itself is the file)."""
    p = Path(os.path.expanduser(str(path)))
    return Path(os.path.realpath(p.parent)) / p.name if p.name else Path(os.path.realpath(p))


def _same(a: Path, b: Path) -> bool:
    try:
        if a.stat().st_size != b.stat().st_size:
            return False
        with open(a, "rb") as x, open(b, "rb") as y:
            while True:
                p, q = x.read(1 << 20), y.read(1 << 20)
                if p != q:
                    return False
                if not p:
                    return True
    except OSError:
        return False


# ---------- what a file says ----------

def _docx(path: Path) -> str | None:
    """A DOCX's text: one line per paragraph (each table cell's own), headers and footers after the body, then its
    document properties (title, author, last modified by)."""
    try:
        with zipfile.ZipFile(path) as z:
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


Annotation = tuple[int, str, str]          # (page, type, its text)


def _pdf(path: Path) -> tuple[str, list[Annotation]] | None:
    """A PDF's text layer, page by page, and its annotations (page, type, text), through PDFKit."""
    try:
        import objc
        from Foundation import NSURL
        from Quartz import PDFDocument
    except ImportError:
        return None
    with objc.autorelease_pool():
        doc = PDFDocument.alloc().initWithURL_(NSURL.fileURLWithPath_(str(path)))
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


def text_of(path: Path) -> tuple[str, list[Annotation]] | None:
    """What a file says, for comparing: (text, PDF annotations). None for a file with no text FERB can read (binary)."""
    suffix = path.suffix.lower()
    if suffix == ".pdf":
        return _pdf(path)
    if suffix in (".docx", ".docm", ".dotx"):
        text = _docx(path)
        return (text, []) if text is not None else None
    try:
        with open(path, "rb") as f:
            data = f.read(TEXT_MAX * 4 + 1)
    except OSError:
        return None
    if b"\0" in data[:8192]:
        return None
    try:
        return data.decode("utf-8"), []
    except UnicodeDecodeError:
        return None


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


def describe(change: Change) -> str:
    """One change as the checker reads it: what happened to the file, then the old-vs-new diff of its text (a made
    file against the original it resembles), a PDF's annotations added or removed, or why there is no diff."""
    path = Path(change.path)
    if change.what == "removed":
        head = f"{change.path}: removed (no longer at this path)"
        return head + (f"; {change.note}" if change.note else "")
    if change.what == "created" and change.source is None:
        new = text_of(path)
        if new is None:
            return f"{change.path}: made by this step (no text FERB can read)"
        body = new[0] if len(new[0]) <= TEXT_MAX else new[0][:TEXT_MAX] + "\n[… cut here; read the file for the rest]"
        notes = f"\nPDF annotations: {_annotations([], new[1])}" if new[1] else ""
        return f"{change.path}: made by this step. Its text:\n{body}{notes}"
    if change.before is None:
        return f"{change.path}: changed; no diff: {change.note or 'FERB kept no copy from before'}"
    old, new = text_of(Path(change.before)), text_of(path)
    if change.what == "created":
        head = f"{change.path}: made by this step; compared with {change.source} as it was before the step"
    else:
        head = f"{change.path}: changed by this step; compared with FERB's copy from before the step"
    if old is None or new is None:
        sizes = ""
        try:
            sizes = f" ({Path(change.before).stat().st_size:,} bytes before, {path.stat().st_size:,} now)"
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
