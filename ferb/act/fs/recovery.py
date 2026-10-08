"""Owns S1 recovery: before FERB overwrites or edits a file, a copy of it (an APFS clone, else a byte copy) kept for
`retention.recovery_days` (7); a durable record of where a trashed file came from and went; restore; and a best-effort
local APFS snapshot before shell or delete work. This is what makes FERB's file changes recoverable, so F5 treats
them as not destructive. Must not lose the original: a clone failure falls back to a byte copy, never silently skips,
and when no copy can be made the caller must not change the file. Must not overwrite anything on restore that the
owner may have changed since (see `restore`). Copies live under ~/.ferb/recovery (owner only), never sent anywhere;
this is the owner's own Mac. The gateway calls preserve / mark_written / discard / record_trash (it holds the stores;
ToolContext has none)."""

import ctypes
import errno
import hashlib
import os
import re
import shutil
import subprocess
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any

TRASH_DIRS = (".Trash", ".Trashes")       # macOS Trash folders: the home volume's, and other volumes'
RENAME_EXCL = 0x00000004                  # renamex_np: fail rather than replace an existing destination
SNAPSHOT_TIMEOUT_S = 30.0
_SNAPSHOT_DATE = re.compile(r"Created local snapshot with date: (\S+)")
_SUFFIX = re.compile(r"\.[A-Za-z0-9]{1,16}")
BEFORE_FERB, RESTORED = "before FERB", "restored"
_INSERT = "INSERT INTO recovery (id, task_id, kind, original, saved_to, reason, at) VALUES (?,?,?,?,?,?,?)"

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
            dst.unlink(missing_ok=True)
            raise


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


def preserve(path: Path, *, reason: str, task_id: str | None, db: Any, clock: Any, ids: Any, root: Path) -> str | None:
    """Before FERB changes `path`: copy it to root/<day>/<id><suffix> and record it; returns the record id, or None when
    there is nothing to keep (no such file yet, or a folder). Raises when no copy could be made: the caller must then
    leave the file alone. The record keeps the path as given (`original`); the copy is of the file itself, through any
    symlink."""
    if not path.is_file():
        return None
    src = path.resolve()
    eid = str(ids.new_id())
    at = clock.now()
    day = at.astimezone().date().isoformat()
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    (root / day).mkdir(mode=0o700, exist_ok=True)
    suffix = src.suffix if _SUFFIX.fullmatch(src.suffix) else ""
    rel = f"{day}/{eid}{suffix}"
    dst = root / rel
    if _inside(root, rel) != dst.resolve():                  # an id can't place a copy outside the root
        raise ValueError(f"recovery id {eid!r} is not a plain file name")
    _copy_new(src, dst)
    try:
        db.conn.execute(_INSERT, (eid, task_id, "clone", str(path), rel, reason, at.isoformat()))
    except BaseException:
        dst.unlink(missing_ok=True)                          # never a copy without its record
        raise
    return eid


def mark_written(record_id: str, path: Path, db: Any) -> None:
    """After the change: the file as FERB left it, so a restore can tell whether anyone changed it since."""
    db.conn.execute("UPDATE recovery SET written_sha256 = ? WHERE id = ?", (_sha256(path.resolve()), record_id))


def discard(record_id: str, db: Any, root: Path) -> None:
    """The call refused before changing the file: its copy and record go."""
    r = db.conn.execute("SELECT saved_to FROM recovery WHERE id = ? AND kind = 'clone'", (record_id,)).fetchone()
    if r is None:
        return
    copy = _inside(root, r["saved_to"])
    if copy is not None:
        copy.unlink(missing_ok=True)
    db.conn.execute("DELETE FROM recovery WHERE id = ?", (record_id,))


def record_trash(original: Path, trashed_to: str | None, *, task_id: str | None, db: Any, clock: Any, ids: Any) -> str:
    """A durable record of a trashed item: where it was and where it went in the Trash, so `restore` can put it back
    without depending on Finder's Put Back or on the span that recorded the call (it ages out)."""
    eid = str(ids.new_id())
    db.conn.execute(_INSERT, (eid, task_id, "trash", str(original), trashed_to, "trash", clock.now().isoformat()))
    return eid


# ---------- reading and restoring ----------

def recent(db: Any, root: Path, limit: int = 100) -> list[dict[str, Any]]:
    """The records, newest first, with whether the copy (or the trashed item) is still there."""
    rows = db.conn.execute("SELECT * FROM recovery ORDER BY at DESC, rowid DESC LIMIT ?", (limit,)).fetchall()
    out = []
    for r in rows:
        saved = _inside(root, r["saved_to"]) if r["kind"] == "clone" else (
            Path(r["saved_to"]) if r["saved_to"] else None)
        out.append({"id": r["id"], "task_id": r["task_id"], "kind": r["kind"], "original": r["original"],
                    "reason": r["reason"], "at": r["at"],
                    "available": saved is not None and os.path.lexists(saved)})
    return out


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
      is there now; its record then goes (it is back).
    `record_id` may be the start of an id. Raises NotRestorable with the reason in the owner's words."""
    r = find(db, record_id)
    original = Path(r["original"])
    dest = original.resolve()                                 # the file itself, through any symlink
    if r["kind"] == "clone":
        return _restore_copy(r, original, dest, root)
    if r["kind"] == "trash":
        return _restore_trashed(r, original, dest, db)
    raise NotRestorable(f"unknown record kind {r['kind']!r}")


def _restore_copy(r: Any, original: Path, dest: Path, root: Path) -> Path:
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


def _restore_trashed(r: Any, original: Path, dest: Path, db: Any) -> Path:
    saved = Path(r["saved_to"]) if r["saved_to"] else None
    if saved is None or not any(part in TRASH_DIRS for part in saved.parts):
        raise NotRestorable(f"FERB doesn't know where {short(original)} went in the Trash")
    if not os.path.lexists(saved):
        raise NotRestorable(f"{short(original)} is no longer in the Trash, so it can't be put back")
    dest.parent.mkdir(parents=True, exist_ok=True)
    try:
        _move_new(saved, dest)
        back = dest
    except FileExistsError:
        back = _beside(dest, RESTORED, lambda target: _move_new(saved, target))
    db.conn.execute("DELETE FROM recovery WHERE id = ?", (r["id"],))
    return back


def _beside(dest: Path, label: str, put: Any) -> Path:
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

def purge_before(cutoff_iso: str, db: Any, root: Path) -> int:
    """Delete records older than the cutoff, a clone's file first: a file that can't be deleted keeps its record for the
    next pass. A path outside the recovery root is never deleted; a trashed item stays in the owner's Trash (only its
    record goes). Empty day folders go too."""
    rows = db.conn.execute("SELECT id, kind, saved_to FROM recovery WHERE at < ?", (cutoff_iso,)).fetchall()
    gone: list[str] = []
    for r in rows:
        copy = _inside(root, r["saved_to"]) if r["kind"] == "clone" else None
        if copy is not None:
            try:
                copy.unlink(missing_ok=True)
            except OSError:
                continue
        gone.append(r["id"])
    for i in range(0, len(gone), 500):
        chunk = gone[i:i + 500]
        db.conn.execute(f"DELETE FROM recovery WHERE id IN ({','.join('?' * len(chunk))})", chunk)
    try:                                                      # tidying only: never fails the pass
        for day in root.iterdir():
            if day.is_dir() and not any(day.iterdir()):
                day.rmdir()
    except OSError:
        pass
    return len(gone)


# ---------- a snapshot before bulk or destructive work ----------

def local_snapshot(timeout_s: float = SNAPSHOT_TIMEOUT_S) -> str | None:
    """A local APFS snapshot of the startup disk (`tmutil localsnapshot`); its name, or None when none was taken.
    Measured 2026-10-09 (macOS 26.6.2, uid 501, no Time Machine destination): taking one needs no admin rights and no
    prompt (0.4 s); reading one back (mount_apfs -s) is refused without admin rights, so it is a recovery point for
    the owner (Recovery, Time Machine, or sudo), while FERB itself restores from per-file copies and the Trash.
    macOS treats local snapshots as purgeable (deleted(8)): they go under disk pressure. Never waits on input."""
    try:
        done = subprocess.run(["/usr/bin/tmutil", "localsnapshot"], stdin=subprocess.DEVNULL, capture_output=True,
                              text=True, timeout=timeout_s)
    except (OSError, subprocess.SubprocessError):
        return None
    found = _SNAPSHOT_DATE.search(f"{done.stdout}\n{done.stderr}")
    return f"com.apple.TimeMachine.{found.group(1)}.local" if done.returncode == 0 and found else None
