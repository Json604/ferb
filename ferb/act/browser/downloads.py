"""Owns taking a browser download into FERB's workspace as facts (D35); must not judge the file or pick a source.

The extension catches a download only in a tab FERB drives for a task and holds it there; nothing reaches the browser's
own download flow, so no Save dialog. This side takes it only while that tab is a live task tab: otherwise the response
goes back to the browser untouched (the owner's own download, never cancelled or read). It pulls the bytes in parts
(`download_part`), one transfer per tab, within a per-file, per-task and time budget; hashes what it received; writes
`~/.ferb/workspace/downloads/<id>-<safe name>` (never elsewhere) and records facts, never content. The file is FERB's
working copy, outside content (MP2: `external:download`, the folder is always an untrusted fs root). It stays until the
owner deletes it; only when the folder passes `browser.download_total_mb` do the oldest move to the Trash (owner,
2026-10-09: no time-based clearing). A download that would leave less than `browser.download_min_free_gb` free on the
disk is refused: by its declared size before any byte moves, and before each part is written while it streams. A
download the extension couldn't intercept, or one it could not take, is a fact for the worker."""

import asyncio
import base64
import hashlib
import os
import secrets
import shutil
import stat
import unicodedata
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Callable, Iterable, NamedTuple

from ferb.act.browser import uploads
from ferb.core import origin

ORIGIN = origin.external("download")
FOLDER = "downloads"                    # under FERB's workspace
NAME_MAX_BYTES = 150                    # the safe name; the id prefix adds 16 (APFS allows 255 bytes)
PART_TIMEOUT_S = 30.0
TRANSFER_DEADLINE_S = 600.0             # a slow drip under the per-part timeout still ends
TAB_WAIT_S = 20.0                       # a second download in a tab waits this long for the first (the extension's
                                        # hold lasts 30 s), then is refused
SETTLE_S = 60.0                         # a navigation/open waits this long for a download its tab started
# [browser] limits, the owner's decision of 2026-10-09 (F4 live fixes): one download, per task tree, FERB's folder, and
# the free disk a download must leave. No time-based clearing (retention.download_days is gone).
DEFAULTS = {"download_max_mb": 512, "download_task_max_files": 50, "download_task_max_mb": 4096,
            "download_total_mb": 20480, "download_min_free_gb": 10}
OUTSIDE = "skipped: not in FERB's downloads folder, so retention leaves it where it is"
DROPPED = ("the page started more saves at once than FERB holds (20 in a tab, 100 in all): this one, and any others "
           "it started within the minute, were dropped, neither taken nor given to the browser; not taken")
_DROP = {"Cc", "Cf", "Cs", "Zl", "Zp"}  # controls, format (bidi overrides, zero-width), surrogates, line separators
_MB = 1024 * 1024
_GB = 1024 * _MB

Publish = Callable[[str, dict[str, Any]], None]
Setting = Callable[[str, str, Any], Any]


@dataclass(frozen=True)
class TaskTab:
    """Whose tab a download came from: the live task that drives it, its alias, and the profile's identity."""
    task: str
    tab: str
    identity: str                       # owner | ferb


class Limits(NamedTuple):
    file_bytes: int
    task_files: int
    task_bytes: int
    total_bytes: int
    min_free_bytes: int


class _Refused(Exception):
    """A budget refused the download: it is failed in the browser (a task tab: no dialog) and recorded as refused."""

    def __init__(self, reason: str, size: int | None) -> None:
        super().__init__(reason)
        self.reason, self.size = reason, size


def safe_name(raw: str) -> str:
    """A file name that can only name a file inside the downloads folder: no path separators (/, \\, :), no control or
    invisible format characters (a right-to-left override can disguise an extension), no leading dots (hidden or
    `..`), at most NAME_MAX_BYTES of UTF-8 with the extension kept. Empty becomes "download"."""
    name = unicodedata.normalize("NFC", str(raw or ""))
    name = "".join("_" if ch in "/\\:" else ch for ch in name if unicodedata.category(ch) not in _DROP)
    name = " ".join(name.split()).lstrip(". ").rstrip(". ")
    if len(name.encode()) > NAME_MAX_BYTES:
        stem, dot, ext = name.rpartition(".")
        keep = f".{ext}" if dot and stem and 0 < len(ext.encode()) <= 16 else ""
        budget = NAME_MAX_BYTES - len(keep.encode())
        cut = name[:len(name) - len(keep)] if keep else name
        while len(cut.encode()) > budget:
            cut = cut[:-1]
        name = cut.rstrip(". ") + keep
    return name or "download"


def untrusted_roots(configured: Iterable[str], workspace: Path) -> tuple[str, ...]:
    """The fs roots whose files are outside content: the owner's configured ones, plus FERB's downloads folder and the
    folder of upload copies made from outside content (`uploads/outside`), which code adds so config can't remove
    them (MP2)."""
    out = [str(p) for p in configured]
    for folder in (str(workspace / FOLDER), str(workspace / uploads.FOLDER / uploads.OUTSIDE)):
        if folder not in out:
            out.append(folder)
    return tuple(out)


def _mb(n: int) -> str:
    return f"{n / _MB:g} MB"


def _gb(n: int) -> str:
    return f"{n / _GB:g} GB"


def _number(value: Any, default: float) -> float:
    try:
        return max(float(value), 0.0)
    except (TypeError, ValueError):
        return default


class Downloads:
    """One per daemon, shared by the owner's and FERB's browser drivers."""

    def __init__(self, root: Path, db: Any, now: Callable[[], datetime], setting: Setting,
                 redact_text: Callable[[str], str], publish: Publish) -> None:
        self.root, self.db, self.now, self.setting, self.redact_text, self.publish = (
            root, db, now, setting, redact_text, publish)
        self.task_tab: Callable[[Any, int], TaskTab | None] = lambda br, chrome_id: None   # live task tabs only
        self.task_root: Callable[[str], str] = lambda task: task   # the task tree a worker step belongs to (budgets)
        self._active: dict[tuple[str, int], set[asyncio.Task[None]]] = {}
        self._locks: dict[tuple[str, int], asyncio.Lock] = {}
        self._inflight: dict[str, int] = {}                 # tree -> bytes received by its transfers still running
        self._running: dict[str, int] = {}                  # tree -> transfers still running
        self._unreported: dict[tuple[str, int], list[dict[str, Any]]] = {}
        self._tab_task: dict[tuple[str, int], str] = {}     # tab -> the task whose downloads it holds (pruning)
        self.free_bytes: Callable[[], int] = self._disk_free   # tests replace it

    def limits(self) -> Limits:
        b = {k: _number(self.setting("browser", k, v), v) for k, v in DEFAULTS.items()}
        return Limits(int(b["download_max_mb"] * _MB), int(b["download_task_max_files"]),
                      int(b["download_task_max_mb"] * _MB), int(b["download_total_mb"] * _MB),
                      int(b["download_min_free_gb"] * _GB))

    def _disk_free(self) -> int:
        """Free bytes on the disk that holds FERB's downloads folder (its nearest existing folder)."""
        where = self.root
        while not where.exists() and where != where.parent:
            where = where.parent
        return shutil.disk_usage(where).free

    # ---------- the extension's download event ----------

    def receive(self, br: Any, ev: Any) -> None:
        """A download the extension caught (held) or saw the browser start itself. Runs as its own task: its parts
        come back over the same connection the event arrived on. It never raises out of the task."""
        key = (br.name, int(ev.tab_id))
        job = asyncio.get_running_loop().create_task(self._guarded(br, ev))
        self._active.setdefault(key, set()).add(job)

        def done(t: asyncio.Task[None]) -> None:
            jobs = self._active.get(key)
            if jobs is not None:
                jobs.discard(t)
                if not jobs:
                    self._active.pop(key, None)
        job.add_done_callback(done)

    async def _guarded(self, br: Any, ev: Any) -> None:
        try:
            await self._receive(br, ev)
        except asyncio.CancelledError:
            raise
        except Exception as e:                          # never an unretrieved task exception; the event says why
            self.publish("browser.download", {"profile": br.name, "tab_id": ev.tab_id, "state": "error",
                                               "reason": self.redact_text(f"{type(e).__name__}: {e}")})

    async def _receive(self, br: Any, ev: Any) -> None:
        owner = self.task_tab(br, int(ev.tab_id))
        if owner is None:                               # not a live task tab: back to the browser, untouched
            if not ev.uncaptured and ev.download_id:
                await _send(br, ev.download_id, release=True)
            self.publish("browser.download", {"profile": br.name, "tab_id": ev.tab_id, "state": "released",
                                               "reason": "not a live task tab: left to the browser"})
            return
        if ev.uncaptured and ev.detail == "dropped":    # over the extension's hold caps: neither FERB's nor the browser's
            self._insert(br, owner, ev, "refused", reason=DROPPED, final=True)
            return
        if ev.uncaptured:
            self._insert(br, owner, ev, "not_taken", reason=_not_taken(ev.detail), final=True)
            return
        if not ev.download_id:
            return
        key = (br.name, int(ev.tab_id))
        self._tab_task[key] = owner.task
        lock = self._locks.setdefault(key, asyncio.Lock())
        try:
            await asyncio.wait_for(lock.acquire(), TAB_WAIT_S)
        except asyncio.TimeoutError:
            await _send(br, ev.download_id, abort=True)
            self._insert(br, owner, ev, "refused", size=ev.size, final=True, reason=(
                "another download from this tab was still being taken (one at a time per tab); not taken"))
            return
        try:
            await self._admit_and_take(br, owner, ev)
        finally:
            lock.release()

    async def _admit_and_take(self, br: Any, owner: TaskTab, ev: Any) -> None:
        lim = self.limits()
        tree = self.task_root(owner.task)
        files, taken = self._tree_usage(tree)
        used = taken + self._inflight.get(tree, 0)
        size = ev.size
        try:
            if files >= lim.task_files:
                raise _Refused(f"this task already took {files} downloads, its limit "
                               "(browser.download_task_max_files, per request); not taken", size)
            if size is not None and size > lim.file_bytes:
                raise _Refused(f"larger than the {_mb(lim.file_bytes)} limit (browser.download_max_mb): {size} bytes; "
                               "not taken", size)
            if size is not None and used + size > lim.task_bytes:
                raise _Refused(f"would pass this task's {_mb(lim.task_bytes)} download budget "
                               f"(browser.download_task_max_mb, per request; {used} bytes taken or arriving): {size} "
                               "bytes; not taken", size)
            if size is not None and self.free_bytes() - size < lim.min_free_bytes:
                raise _Refused(f"would leave less than {_gb(lim.min_free_bytes)} free on this disk "
                               f"(browser.download_min_free_gb): {size} bytes; not taken", size)
        except _Refused as r:
            await _send(br, ev.download_id, abort=True)
            self._insert(br, owner, ev, "refused", size=r.size, reason=r.reason, final=True)
            return
        except OSError as e:                            # the floor can't be checked: refused, never left to the browser
            await _send(br, ev.download_id, abort=True)
            self._insert(br, owner, ev, "refused", size=size, final=True, reason=self.redact_text(
                f"couldn't read the free disk space for browser.download_min_free_gb ({e}); not taken"))
            return
        await self._take(br, owner, ev, lim, tree)

    async def _take(self, br: Any, owner: TaskTab, ev: Any, lim: Limits, tree: str) -> None:
        name = safe_name(self.redact_text(ev.name or ""))
        fid = ""
        loop = asyncio.get_running_loop()
        started, received, digest = loop.time(), 0, hashlib.sha256()
        folder = fd = -1
        moved = False
        self._running[tree] = self._running.get(tree, 0) + 1     # the tree's entries live while any transfer runs
        self._inflight[tree] = self._inflight.get(tree, 0)
        try:
            fid = self._insert(br, owner, ev, "in_progress")
            final, temp = f"{fid}-{name}", f".{fid}.part"
            folder = self._open_folder()
            fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=folder)
            while True:
                left = TRANSFER_DEADLINE_S - (loop.time() - started)
                if left <= 0:
                    raise _Refused(f"still arriving after {TRANSFER_DEADLINE_S / 60:g} minutes ({received} bytes); "
                                   "not taken", received)
                part = await br.call("download_part", {"download_id": ev.download_id, "offset": received},
                                     timeout=min(PART_TIMEOUT_S, max(left, 1.0)))
                if not isinstance(part, dict) or part.get("offset") != received:
                    raise ValueError("the browser sent download parts out of order")
                data = base64.b64decode(str(part.get("data") or ""), validate=True)
                received += len(data)
                self._inflight[tree] += len(data)
                if received > lim.file_bytes:
                    raise _Refused(f"larger than the {_mb(lim.file_bytes)} limit (browser.download_max_mb): stopped "
                                   f"after {received} bytes; not taken", received)
                # what the tree took (other transfers' bytes committed since this one started included) + what arrives
                if self._tree_usage(tree)[1] + self._inflight[tree] > lim.task_bytes:
                    raise _Refused(f"passed this task's {_mb(lim.task_bytes)} download budget "
                                   f"(browser.download_task_max_mb, per request) after {received} bytes; not taken",
                                   received)
                if self.free_bytes() - len(data) < lim.min_free_bytes:
                    raise _Refused(f"would leave less than {_gb(lim.min_free_bytes)} free on this disk "
                                   f"(browser.download_min_free_gb): stopped after {received - len(data)} bytes; "
                                   "not taken", received - len(data))
                view = memoryview(data)
                while view:
                    view = view[os.write(fd, view):]
                digest.update(data)
                if part.get("eof"):
                    break
            os.close(fd)
            fd = -1
            try:
                os.stat(final, dir_fd=folder, follow_symlinks=False)
                raise FileExistsError(final)            # a random id: never expected; never overwrite
            except FileNotFoundError:
                pass
            os.rename(temp, final, src_dir_fd=folder, dst_dir_fd=folder)
            moved = True
            try:
                self._finish(br, ev, fid, "taken", path=str(self.root / final), name=name, size=received,
                             sha256=digest.hexdigest())
            except Exception:                           # no record: no file (never a file FERB can't account for)
                os.unlink(final, dir_fd=folder)
                raise
            self._evict()                               # the folder's total budget, oldest first, to the Trash
        except _Refused as r:
            await _send(br, ev.download_id, abort=True)
            self._finish_quietly(br, ev, fid, "refused", size=r.size, reason=r.reason)
        except asyncio.CancelledError:
            self._finish_quietly(br, ev, fid, "failed", size=received or None,
                                 reason="FERB stopped before the transfer finished; nothing was kept")
            raise
        except Exception as e:
            await _send(br, ev.download_id, abort=True)
            self._finish_quietly(br, ev, fid, "failed", size=received or None, reason=self.redact_text(
                f"the transfer didn't finish ({e}); nothing was kept"))
        finally:
            self._inflight[tree] = max(self._inflight.get(tree, 0) - received, 0)
            self._running[tree] = self._running.get(tree, 1) - 1
            if self._running[tree] <= 0:                 # only the last transfer of the tree clears its entries
                self._running.pop(tree, None)
                self._inflight.pop(tree, None)
            if fd >= 0:
                os.close(fd)
            if folder >= 0:
                if not moved and fid:
                    try:
                        os.unlink(f".{fid}.part", dir_fd=folder)
                    except FileNotFoundError:
                        pass
                os.close(folder)

    def _open_folder(self) -> int:
        """The downloads folder, opened without following a symlink: a link planted there (FERB's shell can write the
        workspace) can't send a file anywhere else."""
        self.root.mkdir(parents=True, exist_ok=True)
        fd = os.open(self.root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        if not stat.S_ISDIR(os.fstat(fd).st_mode):
            os.close(fd)
            raise NotADirectoryError(str(self.root))
        return fd

    # ---------- records ----------

    def _tree_usage(self, tree: str) -> tuple[int, int]:
        """Downloads the task tree (the request's root task and its steps, `root.*`) took or is taking, and the bytes
        it took; running transfers' bytes are counted separately (`_inflight`)."""
        r = self.db.conn.execute(
            "SELECT count(*) AS n, COALESCE(sum(CASE WHEN state = 'taken' THEN bytes END), 0) AS b FROM downloads "
            "WHERE (task_id = ? OR (task_id >= ? || '.' AND task_id < ? || '/')) AND state IN ('taken', 'in_progress')",
            (tree, tree, tree)).fetchone()
        return int(r["n"]), int(r["b"])

    def forget_task(self, task: str) -> None:
        """Task end: its tabs' locks (when idle) and unreported facts go (the records stay)."""
        for key in [k for k, t in self._tab_task.items() if t == task]:
            lock = self._locks.get(key)
            if lock is not None and not lock.locked():
                self._locks.pop(key, None)
            if key not in self._active:
                self._unreported.pop(key, None)
                self._tab_task.pop(key, None)

    def finalize_stale(self) -> int:
        """At startup: a transfer a restart cut off can't still be running."""
        return int(self.db.conn.execute(
            "UPDATE downloads SET state = 'failed', reason = 'FERB restarted before the transfer finished; nothing was "
            "kept' WHERE state = 'in_progress'").rowcount or 0)

    def _insert(self, br: Any, owner: TaskTab, ev: Any, state: str, *, size: int | None = None,
                reason: str | None = None, final: bool = False) -> str:
        fid = "dl-" + secrets.token_hex(6)
        row = {"id": fid, "task_id": owner.task, "profile": br.name, "identity": owner.identity, "tab": owner.tab,
               "state": state, "path": None, "name": safe_name(self.redact_text(ev.name)) if ev.name else None,
               "bytes": size, "sha256": None, "mime": (ev.mime or None),
               "source_url": self.redact_text(ev.source_url or "") or None, "reason": reason, "origin": ORIGIN,
               "at": self.now().isoformat()}
        self.db.conn.execute(f"INSERT INTO downloads ({', '.join(row)}) VALUES ({', '.join('?' * len(row))})",
                             tuple(row.values()))
        if final:
            self._report(br, ev, row)
        return fid

    def _finish(self, br: Any, ev: Any, fid: str, state: str, *, path: str | None = None, name: str | None = None,
                size: int | None = None, sha256: str | None = None, reason: str | None = None) -> None:
        self.db.conn.execute("UPDATE downloads SET state = ?, path = ?, name = COALESCE(?, name), bytes = ?, "
                             "sha256 = ?, reason = ? WHERE id = ?", (state, path, name, size, sha256, reason, fid))
        row = self.db.conn.execute("SELECT * FROM downloads WHERE id = ?", (fid,)).fetchone()
        self._report(br, ev, dict(row))

    def _finish_quietly(self, br: Any, ev: Any, fid: str, state: str, **fields: Any) -> None:
        if not fid:
            return                                      # the in_progress row was never written
        try:
            self._finish(br, ev, fid, state, **fields)
        except Exception:
            pass                                        # the in_progress row stays: nothing was kept either way

    def _report(self, br: Any, ev: Any, row: dict[str, Any]) -> None:
        fact = _fact(row)
        self._unreported.setdefault((br.name, int(ev.tab_id)), []).append(fact)
        self.publish("browser.download", {"profile": br.name, "task_id": row["task_id"], **fact})

    def unreported(self, br: Any, chrome_id: int) -> list[dict[str, Any]]:
        """Downloads from this tab the worker hasn't seen in a result yet (each final state once per tab)."""
        return self._unreported.pop((br.name, int(chrome_id)), [])

    async def settle(self, br: Any, chrome_id: int, timeout: float = SETTLE_S) -> None:
        """Wait (bounded) for the downloads this tab started to finish taking."""
        jobs = list(self._active.get((br.name, int(chrome_id)), ()))
        if jobs:
            await asyncio.wait(jobs, timeout=timeout)

    def facts_for(self, task_ids: set[str], identity: str | None = None) -> list[dict[str, Any]]:
        if not task_ids:
            return []
        marks = ", ".join("?" * len(task_ids))
        sql = f"SELECT * FROM downloads WHERE task_id IN ({marks})"
        params: list[Any] = sorted(task_ids)
        if identity is not None:
            sql += " AND identity = ?"
            params.append(identity)
        return [_fact(dict(r)) for r in self.db.conn.execute(sql + " ORDER BY at, rowid", params)]

    # ---------- the folder's total: the oldest working copies go to the Trash, never unlinked ----------

    def purge(self) -> int:
        """While the folder is over browser.download_total_mb, the oldest files move to the Trash (owner, 2026-10-09: no
        time-based clearing; a file stays until the owner deletes it). Their rows stay, with the path cleared and where
        the Trash put them. A file already gone is recorded so; one that won't move is skipped and recorded (the next
        pass tries again) and stops counting toward the total, so it never makes a newer file go in its place. A row
        whose file isn't in the downloads folder is left as it is (path and file), recorded once as skipped, and never
        counted."""
        lim = self.limits()
        rows = [dict(r) for r in self.db.conn.execute(
            "SELECT id, path, bytes, at, kept FROM downloads WHERE path IS NOT NULL ORDER BY at, rowid")]
        for r in [r for r in rows if Path(str(r["path"])).parent != self.root]:
            rows.remove(r)
            if r["kept"] != OUTSIDE:
                self.db.conn.execute("UPDATE downloads SET kept = ? WHERE id = ?", (OUTSIDE, r["id"]))
        total, moved = sum(int(r["bytes"] or 0) for r in rows), 0
        try:
            folder = self._open_folder()
        except OSError:
            return 0
        try:
            for r in rows:
                if total <= lim.total_bytes:
                    break
                result = self._trash(folder, r)
                total -= int(r["bytes"] or 0)            # a stuck file stops counting: no newer file goes in its place
                if result != "stuck":
                    moved += 1
        finally:
            os.close(folder)
        return moved

    def _evict(self) -> None:
        try:
            self.purge()
        except Exception:
            pass                                        # the next retention pass tries again

    def _trash(self, folder: int, row: dict[str, Any]) -> str:
        """One working copy in the downloads folder to the Trash: "trashed", "gone" (looked: no such file in the
        folder any more) or "stuck"."""
        from ferb.act.fs import files                   # Task 8's seam (tests replace move_to_trash)
        p = Path(str(row["path"]))
        try:
            st = os.stat(p.name, dir_fd=folder, follow_symlinks=False)
        except FileNotFoundError:
            st = None
        if st is None or not stat.S_ISREG(st.st_mode):
            self.db.conn.execute("UPDATE downloads SET path = NULL, kept = 'gone' WHERE id = ?", (row["id"],))
            return "gone"
        try:
            where = files.move_to_trash(self.root / p.name)
        except Exception as e:
            self.db.conn.execute("UPDATE downloads SET kept = ? WHERE id = ?",
                                 (self.redact_text(f"stuck: couldn't move it to the Trash ({e})")[:300], row["id"]))
            return "stuck"
        self.db.conn.execute("UPDATE downloads SET path = NULL, trashed_to = ?, kept = 'trash' WHERE id = ?",
                             (where, row["id"]))
        return "trashed"

    def sweep(self) -> int:
        """At startup: partial files of transfers that never finished (FERB's own `.dl-*.part`) are removed."""
        if not self.root.is_dir() or self.root.is_symlink():
            return 0
        gone = 0
        for p in self.root.iterdir():
            if p.name.startswith(".dl-") and p.name.endswith(".part") and stat.S_ISREG(os.lstat(p).st_mode):
                p.unlink()
                gone += 1
        return gone


def _not_taken(detail: str | None) -> str:
    if detail == "canceled":
        return ("the browser started this download itself and it ended canceled: a Save dialog may have been shown and "
                "dismissed; FERB couldn't intercept it")
    if detail == "new_tab":
        return ("a new tab this tab opened started a download before FERB could attach to it; the browser handled it "
                "(it may have shown its own Save dialog)")
    return ("the browser started this download itself (a link FERB couldn't intercept, e.g. a script-made blob or data "
            "link); FERB couldn't take or stop it, so the browser may have shown its own Save dialog and saved it where "
            "it saves downloads")


def _fact(row: dict[str, Any]) -> dict[str, Any]:
    """What the worker sees: facts about the file, labeled outside content."""
    keys = {"id": "download", "state": "state", "tab": "tab", "path": "path", "name": "name", "sha256": "sha256",
            "mime": "mime", "source_url": "source_url", "reason": "reason", "trashed_to": "trashed_to",
            "origin": "origin"}
    out = {k: row[src] for src, k in keys.items() if row.get(src) not in (None, "")}
    if row.get("bytes") is not None:
        out["bytes" if row["state"] == "taken" else "size"] = row["bytes"]
    kept = row.get("kept")
    if row["state"] == "taken" and not row.get("path"):
        out["kept"] = "already gone when retention ran" if kept == "gone" else "moved to the Trash (retention)"
    elif kept and str(kept).startswith(("stuck", "skipped")):
        out["kept"] = str(kept)
    return out


async def _send(br: Any, download_id: str | None, **flags: Any) -> None:
    """Tell the extension what becomes of a held download: abort (failed in the browser: no dialog, for a refused
    download in a task tab) or release (back to the browser untouched). Best effort: a dropped connection or a
    transfer the extension already let go of ends it anyway."""
    if not download_id:
        return
    try:
        await br.call("download_part", {"download_id": download_id, **flags}, timeout=10)
    except Exception:
        pass
