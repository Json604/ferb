"""Owns S1 recovery: before FERB overwrites or edits a file, a copy of it (an APFS clone, else a byte copy) kept for
`retention.recovery_days` (7); a durable record of where a trashed item came from and went, kept while it is still in
the Trash; restore; and local APFS snapshots before bulk or destructive work. This is what makes FERB's file changes
recoverable, so F5 treats them as not destructive.
- Must not lose the original: a clone failure falls back to a byte copy, never silently skips, and when no copy can be
  made the caller must not change the file.
- Must not overwrite on restore anything the owner may have changed since (see `restore`), nor reach outside the
  recovery root, nor take an item out of the Trash that isn't the one FERB put there.
- Every path here is the file the tool really acts on (symlinks followed, `..` after them: files.real_path), never the
  textual spelling (normalize_path), which can name another file.
- Copying and hashing run in a worker thread (the `_async` forms); the database is written on the caller's thread.
Copies live under ~/.ferb/recovery (owner only), never sent anywhere; this is the owner's own Mac."""

import asyncio
import ctypes
import errno
import hashlib
import os
import re
import shutil
import stat
import subprocess
import uuid
from dataclasses import dataclass
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Awaitable, Callable

from pydantic import BaseModel, Field

from ferb.core.types import ActionSpec, Identity, RiskLevel, Target, ToolContext, ToolResult
from ferb.policy.rules import Kind

TRASH_DIRS = (".Trash", ".Trashes")       # macOS Trash folders: the home volume's, and other volumes'
TRASH_RECORD_MAX_DAYS = 90                # a trash record stays while its item is in the Trash, at most this long
RENAME_EXCL = 0x00000004                  # renamex_np: fail rather than replace an existing destination
SNAPSHOT_TIMEOUT_S = 30.0
SNAPSHOT_DELETE_TIMEOUT_S = 120.0         # measured 15.5 s for one delete (2026-10-09)
SNAPSHOT_EVERY_S = 15 * 60                # at most one snapshot per 15 minutes, across all tasks
STARTUP_DISK = "/"
SNAPSHOT_HOW = ("a recovery point of the whole disk. FERB can't open it (that needs admin rights); you can, from macOS "
                "Recovery or Time Machine")
_SNAPSHOT_DATE = re.compile(r"Created local snapshot with date: (\d{4}-\d{2}-\d{2}-\d{6})")
_SNAPSHOT_LOOSE = re.compile(r"Created local snapshot with date: (\S+)")
_SNAPSHOT_NAME = re.compile(r"com\.apple\.TimeMachine\.(\d{4}-\d{2}-\d{2}-\d{6})\.local")
_SUFFIX = re.compile(r"\.[A-Za-z0-9]{1,16}")
_DAY = re.compile(r"\d{4}-\d{2}-\d{2}")
_LOCKED = stat.UF_IMMUTABLE | stat.UF_APPEND
BEFORE_FERB, RESTORED = "before FERB", "restored"
_INSERT = ("INSERT INTO recovery (id, task_id, kind, original, saved_to, reason, at, saved_dev, saved_ino) "
           "VALUES (?,?,?,?,?,?,?,?,?)")

try:
    _libc: Any = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
    _libc.clonefile.argtypes = (ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint32)
    _libc.clonefile.restype = ctypes.c_int
    _libc.renamex_np.argtypes = (ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint)
    _libc.renamex_np.restype = ctypes.c_int
except (OSError, AttributeError):        # not macOS: no clonefile / renamex_np; the byte copy and rename still work
    _libc = None


class NotRestorable(Exception):
    """Why a record can't be restored, in words the owner reads (short paths)."""


class NoFileTarget(Exception):
    """A write-file tool whose file FERB can't tell (no real_path hook, no path argument): S1 is never skipped
    silently, so the call doesn't run."""


@dataclass(frozen=True)
class Recovery:
    """Where recovery copies and records live: ~/.ferb/recovery and the daemon's database."""
    root: Path
    db: Any
    clock: Any
    ids: Any


def short(path: Path | str) -> str:
    """A path as the owner writes it: the home folder as ~ (P27)."""
    p, home = Path(path), Path.home()
    return "~/" + str(p.relative_to(home)) if p.is_relative_to(home) and p != home else str(p)


# ---------- copies ----------

def _clone(src: Path, dst: Path) -> None:
    """APFS clonefile(2): a copy-on-write copy, instant and free on the same volume. Raises OSError otherwise (another
    volume, a disk that isn't APFS). Never replaces dst."""
    if _libc is None:
        raise OSError(errno.ENOTSUP, "clonefile is unavailable")
    if _libc.clonefile(os.fsencode(src), os.fsencode(dst), 0) != 0:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code))


def _byte_copy(src: Path, dst: Path) -> None:
    """A plain copy into a new file (never replacing one), with the original's mode and times. A partial copy goes."""
    with open(src, "rb") as s:
        d = open(dst, "xb")
        try:
            with d:
                shutil.copyfileobj(s, d)
            shutil.copystat(src, dst)
        except BaseException:
            _unlock(dst)
            dst.unlink(missing_ok=True)
            raise


def _unlock(path: Path) -> None:
    """Clear a locked / append-only flag a copy inherited from its original (clone and copystat both carry it), so
    the purge can always delete the copy. Best effort."""
    try:
        flags = os.lstat(path).st_flags
        if flags & _LOCKED:
            os.chflags(path, flags & ~_LOCKED, follow_symlinks=False)
    except (OSError, AttributeError, NotImplementedError):
        pass


def _copy_new(src: Path, dst: Path) -> None:
    """Copy src to a new path: a clone when the volume allows it, else a byte copy. Raises when neither works, or when
    dst already exists (it is never replaced)."""
    try:
        _clone(src, dst)
        return
    except OSError as e:
        if e.errno == errno.EEXIST:
            raise
    _byte_copy(src, dst)


def _make_copy(src: Path, dst: Path) -> None:
    _copy_new(src, dst)
    _unlock(dst)


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def _inside(root: Path, rel: str | None) -> Path | None:
    """A clone's file, only when it resolves inside the recovery root (symlinks followed); else None."""
    if not rel:
        return None
    path = (root / rel).resolve()
    return path if path.is_relative_to(root.resolve()) else None


@dataclass(frozen=True)
class _Copy:
    eid: str
    src: Path
    rel: str
    dst: Path
    at: datetime


def _plan_copy(path: Path, root: Path, clock: Any, ids: Any) -> _Copy | None:
    if not path.is_file():
        return None
    src = path.resolve()
    eid = str(ids.new_id())
    at = clock.now()
    day = at.astimezone().date().isoformat()
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    (root / day).mkdir(mode=0o700, exist_ok=True)
    rel = f"{day}/{eid}{src.suffix if _SUFFIX.fullmatch(src.suffix) else ''}"
    dst = root / rel
    if _inside(root, rel) != dst.resolve():                  # an id can't place a copy outside the root
        raise ValueError(f"recovery id {eid!r} is not a plain file name")
    return _Copy(eid, src, rel, dst, at)


def _record_copy(c: _Copy, reason: str, task_id: str | None, db: Any) -> str:
    try:
        db.conn.execute(_INSERT, (c.eid, task_id, "clone", str(c.src), c.rel, reason, c.at.isoformat(), None, None))
    except BaseException:
        _unlock(c.dst)
        c.dst.unlink(missing_ok=True)                        # never a copy without its record
        raise
    return c.eid


def preserve(path: Path, *, reason: str, task_id: str | None, db: Any, clock: Any, ids: Any, root: Path) -> str | None:
    """Before FERB changes the file at `path` (the real path the tool writes): copy it to root/<day>/<id><suffix> and
    record it; returns the record id, or None when there is nothing to keep (no such file yet, or a folder). Raises when
    no copy could be made: the caller must then leave the file alone."""
    c = _plan_copy(path, root, clock, ids)
    if c is None:
        return None
    _make_copy(c.src, c.dst)
    return _record_copy(c, reason, task_id, db)


async def preserve_async(path: Path, *, reason: str, task_id: str | None, db: Any, clock: Any, ids: Any,
                         root: Path) -> str | None:
    """`preserve` on the daemon's loop: the copy (a byte copy can be long) runs in a worker thread."""
    c = _plan_copy(path, root, clock, ids)
    if c is None:
        return None
    await asyncio.to_thread(_make_copy, c.src, c.dst)
    return _record_copy(c, reason, task_id, db)


def mark_written(record_id: str, path: Path, db: Any) -> None:
    """After the change: the file as FERB left it, so a restore can tell whether anyone changed it since."""
    db.conn.execute("UPDATE recovery SET written_sha256 = ? WHERE id = ?", (_sha256(path.resolve()), record_id))


async def mark_written_async(record_id: str, path: Path, db: Any) -> None:
    digest = await asyncio.to_thread(lambda: _sha256(path.resolve()))
    db.conn.execute("UPDATE recovery SET written_sha256 = ? WHERE id = ?", (digest, record_id))


def _same_then_drop(path: Path, copy: Path) -> bool:
    try:
        if _sha256(path.resolve()) != _sha256(copy):
            return False
    except OSError:
        return False
    _unlock(copy)
    copy.unlink(missing_ok=True)
    return True


async def discard_if_unchanged(record_id: str, path: Path, db: Any, root: Path) -> bool:
    """The call reported an error: its copy and record go only when the file is still byte-identical to the copy
    (nothing changed). Otherwise both stay, so whatever happened can be undone."""
    r = db.conn.execute("SELECT saved_to FROM recovery WHERE id = ? AND kind = 'clone'", (record_id,)).fetchone()
    copy = _inside(root, r["saved_to"]) if r is not None else None
    if copy is None or not await asyncio.to_thread(_same_then_drop, path, copy):
        return False
    db.conn.execute("DELETE FROM recovery WHERE id = ?", (record_id,))
    return True


def _trash_place(saved_to: str | None) -> Path | None:
    """Where an item sits in the Trash: absolute, no `..`, its folder resolved, inside a real .Trash / .Trashes
    folder; else None."""
    if not saved_to:
        return None
    p = Path(saved_to)
    if not p.is_absolute() or ".." in p.parts:
        return None
    try:
        real = p.parent.resolve() / p.name                   # the item itself may be a link: never followed
    except (OSError, RuntimeError):
        return None
    if any(a.name in TRASH_DIRS and a.is_dir() for a in real.parents):
        return real
    return None


def record_trash(original: Path, trashed_to: str | None, *, task_id: str | None, db: Any, clock: Any, ids: Any) -> str:
    """A durable record of a trashed item: where it was (the real path the tool trashed) and where it went in the
    Trash, with the item's device and inode, so `restore` puts back exactly that item without depending on Finder's
    Put Back or on the span that recorded the call (it ages out)."""
    place = _trash_place(trashed_to)
    dev = ino = None
    if place is not None:
        try:
            st = os.lstat(place)
            dev, ino = st.st_dev, st.st_ino
        except OSError:
            pass
    eid = str(ids.new_id())
    db.conn.execute(_INSERT, (eid, task_id, "trash", str(original), str(place) if place else trashed_to, "trash",
                              clock.now().isoformat(), dev, ino))
    return eid


def _same_trash_item(r: Any) -> bool:
    place = _trash_place(r["saved_to"])
    if place is None or r["saved_dev"] is None:
        return False
    try:
        st = os.lstat(place)
    except OSError:
        return False
    return (st.st_dev, st.st_ino) == (r["saved_dev"], r["saved_ino"])


# ---------- reading and restoring ----------

def recent(db: Any, root: Path, limit: int = 100) -> tuple[list[dict[str, Any]], int]:
    """The records, newest first (at most `limit`), with whether each can still be put back; and how many there are."""
    total = int(db.conn.execute("SELECT count(*) FROM recovery").fetchone()[0])
    rows = db.conn.execute("SELECT * FROM recovery ORDER BY at DESC, rowid DESC LIMIT ?", (limit,)).fetchall()
    out = []
    for r in rows:
        if r["kind"] == "clone":
            copy = _inside(root, r["saved_to"])
            available: bool | None = copy is not None and copy.is_file()
        elif r["kind"] == "trash":
            available = _same_trash_item(r)
        else:
            available = None                                 # a snapshot: macOS may have thinned it
        out.append({"id": r["id"], "task_id": r["task_id"], "kind": r["kind"], "original": r["original"],
                    "saved_to": r["saved_to"] if r["kind"] == "snapshot" else None, "reason": r["reason"],
                    "at": r["at"], "available": available})
    return out, total


def find(db: Any, ref: str) -> Any:
    """A record by its id or the start of it (the owner types a few characters)."""
    ref = ref.strip()
    if not ref:
        raise NotRestorable("say which record: ferb restore <id>")
    exact = db.conn.execute("SELECT * FROM recovery WHERE id = ?", (ref,)).fetchone()
    if exact is not None:
        return exact
    rows = db.conn.execute("SELECT * FROM recovery WHERE substr(id, 1, ?) = ? LIMIT 2", (len(ref), ref)).fetchall()
    if len(rows) > 1:
        raise NotRestorable(f"more than one record starts with {ref}; give more of its id")
    if not rows:
        raise NotRestorable(f"no recovery record {ref}; `ferb restore` lists the ones FERB still keeps")
    return rows[0]


def restore(record_id: str, db: Any, root: Path) -> Path:
    """Put a record back and return where it went. Never overwrites what the owner may have changed:
    - a clone goes back in place when the file is gone, or is still exactly what FERB left (written_sha256); if the
      file changed since (or FERB never learned what it left), the copy goes beside it as "<name> (before FERB)<ext>"
      and the current file stays as it is;
    - a trashed item moves back from the Trash to where it was, or beside it as "<name> (restored)<ext>" when something
      is there now; only the very item FERB trashed (same device and inode), and its record then goes (it is back);
    - a snapshot can't be opened by FERB: the error says how the owner can use it.
    `record_id` may be the start of an id. Raises NotRestorable with the reason in the owner's words. On the daemon's
    loop use `restore_async` (the file work runs in a thread); hold the file's lock around either."""
    r = dict(find(db, record_id))
    back = _put_back(r, root)
    if r["kind"] == "trash":
        db.conn.execute("DELETE FROM recovery WHERE id = ?", (r["id"],))
    return back


async def restore_async(record_id: str, db: Any, root: Path) -> Path:
    r = dict(find(db, record_id))
    back = await asyncio.to_thread(_put_back, r, root)
    if r["kind"] == "trash":
        db.conn.execute("DELETE FROM recovery WHERE id = ?", (r["id"],))
    return back


def _put_back(r: dict[str, Any], root: Path) -> Path:
    """The file work of a restore (no database): safe in a worker thread."""
    original = Path(r["original"])
    if r["kind"] == "snapshot":
        raise NotRestorable(f"{r['saved_to']} is {SNAPSHOT_HOW}")
    dest = original.resolve()                                 # the file itself, through any symlink
    if r["kind"] == "clone":
        return _restore_copy(r, original, dest, root)
    if r["kind"] == "trash":
        return _restore_trashed(r, original, dest)
    raise NotRestorable(f"unknown record kind {r['kind']!r}")


def _restore_copy(r: dict[str, Any], original: Path, dest: Path, root: Path) -> Path:
    copy = _inside(root, r["saved_to"])
    if copy is None or not copy.is_file():
        raise NotRestorable(f"the copy FERB kept of {short(original)} is gone")
    if not os.path.lexists(dest):
        dest.parent.mkdir(parents=True, exist_ok=True)
        _copy_new(copy, dest)
        return dest
    if dest.is_file():
        now = _sha256(dest)
        if now == _sha256(copy):                              # already what it was
            return dest
        if r["written_sha256"] and now == r["written_sha256"]:   # still exactly what FERB left: back in place
            tmp = dest.parent / f".{dest.name}.{uuid.uuid4().hex[:8]}.ferb-restore"
            _copy_new(copy, tmp)
            try:
                os.replace(tmp, dest)
            finally:
                tmp.unlink(missing_ok=True)
            return dest
    return _beside(dest, BEFORE_FERB, lambda target: _copy_new(copy, target))


def _restore_trashed(r: dict[str, Any], original: Path, dest: Path) -> Path:
    place = _trash_place(r["saved_to"])
    if place is None or r["saved_dev"] is None:
        raise NotRestorable(f"FERB doesn't know where {short(original)} went in the Trash")
    if not os.path.lexists(place):
        raise NotRestorable(f"{short(original)} is no longer in the Trash, so it can't be put back")
    if not _same_trash_item(r):
        raise NotRestorable(f"what is in the Trash under that name now isn't the {short(original)} FERB moved there, "
                            "so FERB left it alone")
    dest.parent.mkdir(parents=True, exist_ok=True)
    try:
        _move_new(place, dest)
        return dest
    except FileExistsError:
        return _beside(dest, RESTORED, lambda target: _move_new(place, target))


def _beside(dest: Path, label: str, put: Callable[[Path], None]) -> Path:
    """The first free "<stem> (<label>[ n])<suffix>" next to dest; `put` creates it exclusively (a race just moves on
    to the next name)."""
    for n in range(1, 1000):
        target = dest.with_name(f"{dest.stem} ({label}{'' if n == 1 else f' {n}'}){dest.suffix}")
        if os.path.lexists(target):
            continue
        try:
            put(target)
            return target
        except FileExistsError:
            continue
    raise NotRestorable(f"no free name next to {short(dest)}")


def _move_new(src: Path, dst: Path) -> None:
    """Move src to dst, never replacing what is at dst (renamex_np with RENAME_EXCL). Raises FileExistsError then."""
    if _libc is not None:
        if _libc.renamex_np(os.fsencode(src), os.fsencode(dst), RENAME_EXCL) == 0:
            return
        code = ctypes.get_errno()
        if code == errno.EEXIST:
            raise FileExistsError(code, os.strerror(code), str(dst))
        if code != errno.EXDEV:
            raise OSError(code, os.strerror(code), str(src))
    if os.path.lexists(dst):                                  # another volume (or no renamex_np): check, then move
        raise FileExistsError(errno.EEXIST, os.strerror(errno.EEXIST), str(dst))
    shutil.move(str(src), str(dst))


# ---------- retention ----------

def purge_before(cutoff_iso: str, db: Any, root: Path, *, trash_backstop_iso: str | None = None) -> int:
    """Records older than the cutoff go, a clone's file first: a file that can't be deleted keeps its record for the
    next pass, and a path outside the recovery root is never deleted.
    - A trash record stays while its very item is still in the Trash, up to the backstop (TRASH_RECORD_MAX_DAYS); the
      item itself always stays in the owner's Trash.
    - Snapshot records are left to `purge_snapshots` (deleting a snapshot is slow).
    - Copies with no record in day folders older than the cutoff's day (a copy whose record was never written) go,
      and so do empty day folders."""
    rows = db.conn.execute("SELECT * FROM recovery WHERE at < ? AND kind != 'snapshot'", (cutoff_iso,)).fetchall()
    gone: list[str] = []
    for r in rows:
        if r["kind"] == "trash" and _same_trash_item(r) and (trash_backstop_iso is None or
                                                             r["at"] >= trash_backstop_iso):
            continue
        copy = _inside(root, r["saved_to"]) if r["kind"] == "clone" else None
        if copy is not None:
            try:
                _unlock(copy)
                copy.unlink(missing_ok=True)
            except OSError:
                continue
        gone.append(r["id"])
    for i in range(0, len(gone), 500):
        chunk = gone[i:i + 500]
        db.conn.execute(f"DELETE FROM recovery WHERE id IN ({','.join('?' * len(chunk))})", chunk)
    _sweep(cutoff_iso, db, root)
    return len(gone)


def _sweep(cutoff_iso: str, db: Any, root: Path) -> None:
    """Tidying, never fails the pass: unrecorded copies in day folders before the cutoff's day, then empty folders."""
    try:
        before = datetime.fromisoformat(cutoff_iso).astimezone().date().isoformat()
        kept = {r["saved_to"] for r in db.conn.execute("SELECT saved_to FROM recovery WHERE kind = 'clone'")}
        for day in root.iterdir():
            if not day.is_dir() or day.is_symlink() or not _DAY.fullmatch(day.name):
                continue
            if day.name < before:
                for f in day.iterdir():
                    if f"{day.name}/{f.name}" not in kept and (f.is_file() or f.is_symlink()):
                        _unlock(f)
                        f.unlink(missing_ok=True)
            if not any(day.iterdir()):
                day.rmdir()
    except (OSError, ValueError):
        pass


async def purge_snapshots(cutoff_iso: str, db: Any, delete: Callable[[str], Awaitable[bool]] | None,
                          on_gone: Callable[[str, bool], None] | None = None) -> int:
    """FERB's own snapshots older than the cutoff: deleted (`delete`), then their records. Only a name of the strict
    Time Machine form is ever deleted; one that couldn't be deleted keeps its record for the next pass. Any other name
    (tmutil reported something unexpected), or no `delete`: macOS's own thinning removes the snapshot and only the
    record goes. `on_gone(name, deleted)` runs right after each record goes, so a pass cancelled midway (shutdown;
    a delete takes seconds) has journaled what it did."""
    rows = db.conn.execute("SELECT id, saved_to FROM recovery WHERE kind = 'snapshot' AND at < ?",
                           (cutoff_iso,)).fetchall()
    done = 0
    for r in rows:
        name = str(r["saved_to"] or "")
        deleted = False
        if delete is not None and _SNAPSHOT_NAME.fullmatch(name):
            try:
                if not await delete(name):
                    continue
            except asyncio.CancelledError:
                raise
            except Exception:
                continue
            deleted = True
        db.conn.execute("DELETE FROM recovery WHERE id = ?", (r["id"],))
        done += 1
        if on_gone is not None:
            on_gone(name, deleted)
    return done


# ---------- snapshots before bulk or destructive work ----------

def local_snapshot(timeout_s: float = SNAPSHOT_TIMEOUT_S) -> str | None:
    """A local APFS snapshot of the startup disk (`tmutil localsnapshot`); its name (com.apple.TimeMachine.<date>.local;
    if tmutil reports success in other words, those words, so it is still recorded and rate-limited), or None when
    none was taken.
    Measured 2026-10-09 (macOS 26.6.2, uid 501, no Time Machine destination): taking one needs no admin rights and no
    prompt (0.4 s); reading one back (mount_apfs -s) is refused without admin rights, so it is a recovery point for
    the owner (Recovery, Time Machine, or sudo), while FERB itself restores from per-file copies and the Trash.
    macOS treats local snapshots as purgeable (deleted(8)): they go under disk pressure. Never waits on input."""
    try:
        done = subprocess.run(["/usr/bin/tmutil", "localsnapshot"], stdin=subprocess.DEVNULL, capture_output=True,
                              text=True, timeout=timeout_s)
    except (OSError, subprocess.SubprocessError):
        return None
    if done.returncode != 0:
        return None
    text = f"{done.stdout}\n{done.stderr}"
    strict, loose = _SNAPSHOT_DATE.search(text), _SNAPSHOT_LOOSE.search(text)
    if strict:
        return f"com.apple.TimeMachine.{strict.group(1)}.local"
    lines = [line.strip() for line in text.splitlines() if line.strip()]  # tmutil said it made one: keep its words
    return loose.group(1) if loose else lines[-1] if lines else "a snapshot tmutil reported without a name"


async def _tmutil(*args: str, timeout_s: float) -> tuple[int, str] | None:
    """tmutil with no terminal (stdin closed: it never waits on a prompt), in its own session: (exit code, output), or
    None when it couldn't start or took longer than timeout_s. A run that times out, or whose caller is cancelled
    (daemon shutdown), is left to finish on its own; shutdown never waits for it."""
    try:
        proc = await asyncio.create_subprocess_exec("/usr/bin/tmutil", *args, stdin=subprocess.DEVNULL,
                                                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                                    start_new_session=True)
    except OSError:
        return None
    try:
        out, _ = await asyncio.wait_for(proc.communicate(), timeout_s)
    except TimeoutError:
        return None
    return int(proc.returncode or 0), out.decode(errors="replace")


async def delete_local_snapshot(name: str, timeout_s: float = SNAPSHOT_DELETE_TIMEOUT_S) -> bool:
    """Delete one of FERB's own snapshots by the name `local_snapshot` returned; True when it is gone (deleted now, or
    already thinned by macOS). Measured 2026-10-09: `tmutil deletelocalsnapshots <date>` needs no admin rights and no
    prompt, and took 15.5 s. Only the strict com.apple.TimeMachine.<date>.local form is accepted."""
    found = _SNAPSHOT_NAME.fullmatch(name)
    if found is None:
        return False
    listed = await _tmutil("listlocalsnapshots", STARTUP_DISK, timeout_s=SNAPSHOT_TIMEOUT_S)
    if listed is not None and listed[0] == 0 and name not in listed[1].split():
        return True
    done = await _tmutil("deletelocalsnapshots", found.group(1), timeout_s=timeout_s)
    return done is not None and done[0] == 0


class Snapshots:
    """S1 snapshots, before bulk or destructive work: when an agent asks (`recovery.snapshot`; agents judge what is
    bulk, rule 6) and before a permanent delete. At most one attempt every SNAPSHOT_EVERY_S across all tasks, taken or
    failed: a request inside that window reuses the last snapshot and says so, or reports the recent failure. Each
    snapshot is a `recovery` row (kind snapshot) that `ferb restore` lists with how to use it. Best effort: `request`
    returns a failure as a result and never raises one (only its own caller's cancellation), so a snapshot never
    refuses a call; a finished attempt's future is never reused. `take` None: off (tests)."""

    def __init__(self, take: Callable[[], str | None] | None, db: Any, clock: Any, ids: Any,
                 redact: Callable[[Any], Any], every_s: float = SNAPSHOT_EVERY_S) -> None:
        self.take, self.db, self.clock, self.ids, self.every_s = take, db, clock, ids, every_s
        self.redact = redact                                  # the agent's `before` words are stored redacted
        self._running: asyncio.Future[dict[str, Any]] | None = None
        self._failed: tuple[datetime, str] | None = None     # the last failed attempt: when, and why

    async def request(self, task_id: str | None, reason: str) -> dict[str, Any]:
        last = self._recent()
        if last is not None:
            return last | {"reused": True}
        if self.take is None:
            return {"error": "snapshots are turned off for this FERB"}
        failed = self._failed
        if failed is not None and self.clock.now() - failed[0] < timedelta(seconds=self.every_s):
            return {"error": f"no snapshot was taken: the last attempt, under 15 minutes ago, failed ({failed[1]}); "
                             "FERB tries again after that"}
        running = self._running
        mine = running is None or running.done()     # a finished one (taken, failed or cancelled) is never reused
        if running is None or mine:
            running = self._running = asyncio.ensure_future(self._run(task_id, reason))
        try:
            result = await asyncio.shield(running)
        except asyncio.CancelledError:
            current = asyncio.current_task()
            if current is not None and current.cancelling():
                raise                                         # our caller was cancelled; the snapshot goes on
            return {"error": "no snapshot was taken (it was cancelled)"}
        except Exception as e:
            return {"error": f"no snapshot was taken ({type(e).__name__})"}
        return result | ({} if mine or "error" in result else {"reused": True})

    def _recent(self) -> dict[str, Any] | None:
        since = (self.clock.now() - timedelta(seconds=self.every_s)).isoformat()
        r = self.db.conn.execute("SELECT id, saved_to, at FROM recovery WHERE kind = 'snapshot' AND at >= ? "
                                 "ORDER BY at DESC, rowid DESC LIMIT 1", (since,)).fetchone()
        return None if r is None else {"snapshot": r["saved_to"], "record": r["id"], "taken_at": r["at"]}

    async def _run(self, task_id: str | None, reason: str) -> dict[str, Any]:
        take = self.take
        assert take is not None
        try:
            name = await asyncio.wait_for(asyncio.to_thread(take), SNAPSHOT_TIMEOUT_S + 5)
        except asyncio.CancelledError:
            raise
        except Exception as e:
            return self._fail(type(e).__name__)
        if not name:
            return self._fail("tmutil refused or is unavailable")
        self._failed = None
        eid = str(self.ids.new_id())
        # Redacted first, then cut: a secret straddling the cut is still whole when the redactor sees it.
        stored = str(self.redact({"name": name})["name"])[:200]
        before = str(self.redact({"before": reason})["before"])[:200]
        self.db.conn.execute(_INSERT, (eid, task_id, "snapshot", STARTUP_DISK, stored, before,
                                       self.clock.now().isoformat(), None, None))
        return {"snapshot": stored, "record": eid}

    def _fail(self, why: str) -> dict[str, Any]:
        """A failed attempt counts toward the rate limit too: at most one attempt per window."""
        self._failed = (self.clock.now(), why)
        return {"error": f"no snapshot was taken ({why})"}


class RecoverySnapshot:
    """`recovery.snapshot`: an agent asks for a recovery point before bulk or destructive work it judged as such."""
    name = "recovery.snapshot"
    description = ("Before bulk or destructive file work (many files changed, moved or deleted at once, e.g. a shell "
                   "command over a folder), take a local snapshot of the Mac's disk as a recovery point the owner can "
                   "open. At most one every 15 minutes: a recent one is reused. Best effort. Not needed for single "
                   "fs.write / fs.edit / fs.trash calls: FERB keeps those recoverable itself.")
    scopes: set[str] = set()
    identities = {Identity.OWNER, Identity.FERB}
    risk, reversible = RiskLevel.LOW, True

    class Input(BaseModel):
        before: str = Field(min_length=1, max_length=200, description="the bulk or destructive work it is for")

    def __init__(self, snapshots: Snapshots) -> None:
        self.snapshots = snapshots

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(Kind.SNAPSHOT, Target())

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        out = await self.snapshots.request(ctx.task_id, str(getattr(args, "before")))
        if "error" in out:
            return ToolResult(out | {"next": "Decide whether the work is still right without a snapshot; if you go "
                                             "on, say in your report that no snapshot was taken."}, is_error=True)
        how = f"The owner can open it from macOS Recovery or Time Machine (admin rights): {out['snapshot']}."
        reused = "A snapshot from the last 15 minutes is reused. " if out.get("reused") else ""
        return ToolResult(out | {"note": reused + how})
