"""Owns taking a browser download into FERB's workspace as facts (D35); must not judge the file or pick a source.

The extension catches a download only in a tab FERB drives for a task and holds it there; nothing reaches the browser's
own download flow, so no Save dialog. This side takes it only while that tab is a live task tab: otherwise the response
goes back to the browser untouched (the owner's own download, never cancelled or read). It pulls the bytes in parts
(`download_part`), one transfer per tab, within the task tree's budget (hands decision 7, owner 2026-10-31: no size cap
per file; 300 files and 32 GB per request), failed only when it stops moving (no bytes for STALL_S), never for how long
it takes; hashes what it received; writes `~/.ferb/workspace/downloads/<id>-<safe name>` (never elsewhere) and records
facts, never content. The file is FERB's working copy, outside content (MP2: `external:download`, the folder is always
an untrusted fs root). It stays until the owner deletes it; when the folder passes `browser.download_total_gb` the
daemon asks the owner whether to delete the oldest for good (hands decision 8, owner 2026-10-31, replacing the move to
the Trash, which freed no space); no time-based clearing. A download that would leave less than
`browser.download_min_free_gb` free on the disk is refused: by its declared size before any byte moves, and before each
part is written while it streams. A download the extension couldn't intercept, or one it could not take, is a fact for
the worker.

The task tree's byte budget is one counter shared with FERB's shell (one path): what a tree's shell commands received
through the egress proxy (`shell_received`, kept in `shell_downloads` so a restart doesn't reset it) counts beside the
files its tabs took, and `shell_stop` says when a command receiving bytes must stop (past the budget, or the disk under
the floor). Shell downloads have no file count (a package manager fetches many parts per install). All sizes are
decimal (1 GB = 1000^3 bytes), as Finder and the owner count them."""

import asyncio
import base64
import hashlib
import os
import secrets
import shutil
import stat
import time
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
# A transfer fails only when it stops moving: no byte for this long (a part that doesn't answer within it, or parts that
# keep coming back empty). Never a total time (ruling, step 12e: the old 10-minute ceiling capped a download at a few GB).
STALL_S = 60.0
TAB_WAIT_S = 20.0                       # a second download in a tab waits this long for the first (the extension's
                                        # hold lasts 30 s), then is refused
SETTLE_S = 60.0                         # a navigation/open waits this long for a download its tab started
# [browser] limits: per task tree (hands decision 7, owner 2026-10-31: 300 files and 32 GB, replacing 2026-10-09's 50
# files and 4 GB; no per-file cap, replacing 512 MB), FERB's folder (20 GB, 2026-10-09) and the free disk a download must
# leave (10 GB, 2026-10-09). No time-based clearing (retention.download_days is gone). Decimal GB throughout.
DEFAULTS = {"download_task_max_files": 300, "download_task_max_gb": 32, "download_total_gb": 20,
            "download_min_free_gb": 10}
OUTSIDE = "skipped: not in FERB's downloads folder, so retention leaves it where it is"
DROPPED = ("the page started more saves at once than FERB holds (20 in a tab, 100 in all): {n}, neither taken nor "
           "given to the browser; not taken")
_DROP = {"Cc", "Cf", "Cs", "Zl", "Zp"}  # controls, format (bidi overrides, zero-width), surrogates, line separators
_GB = 1000 ** 3                         # decimal GB, as Finder and macOS show it (every limit here)

Publish = Callable[[str, dict[str, Any]], None]
Setting = Callable[[str, str, Any], Any]


@dataclass(frozen=True)
class TaskTab:
    """Whose tab a download came from: the live task that drives it, its alias, and the profile's identity."""
    task: str
    tab: str
    identity: str                       # owner | ferb


class OverCap(NamedTuple):
    """The downloads folder over its cap: the oldest files that would go (download ids, never names), their bytes, the
    folder's total and the cap."""
    ids: tuple[str, ...]
    bytes: int
    total: int
    cap: int


class Limits(NamedTuple):
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


def _gb(n: int) -> str:
    if 0 < n < _GB // 10:                               # under 0.1 GB: in MB, so a small budget never reads "0 GB"
        return f"{n / 1000 ** 2:.2f}".rstrip("0").rstrip(".") + " MB"
    return f"{n / _GB:.2f}".rstrip("0").rstrip(".") + " GB"


def _low_disk(free: int, floor: int, what: str) -> str:
    return (f"only {_gb(free)} free now; FERB keeps at least {_gb(floor)} free (browser.download_min_free_gb): "
            f"{what}; not taken")


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
        self._shell: dict[str, int] = {}                    # tree -> bytes its shell commands received (cache of the
        #                                                     shell_downloads row; the row survives a restart)
        self.free_bytes: Callable[[], int] = self._disk_free   # tests replace it
        self.monotonic: Callable[[], float] = time.monotonic   # the stall clock (tests replace it)
        self.on_over_cap: Callable[[], None] = lambda: None   # the daemon's cap check (hands decision 8)

    def limits(self) -> Limits:
        """The owner's limits; an old config's per-file key (`download_max_mb`) or MB keys are not read."""
        b = {k: _number(self.setting("browser", k, v), v) for k, v in DEFAULTS.items()}
        return Limits(int(b["download_task_max_files"]), int(b["download_task_max_gb"] * _GB),
                      int(b["download_total_gb"] * _GB), int(b["download_min_free_gb"] * _GB))

    def _disk_free(self) -> int:
        """Free bytes on the disk that holds FERB's downloads folder (its nearest existing folder)."""
        where = self.root
        while not where.exists() and where != where.parent:
            where = where.parent
        return shutil.disk_usage(where).free

    def _free(self, what: str, size: int | None) -> int:
        """Free bytes now, for the floor. Unreadable: the floor can't be checked, so the download is refused."""
        try:
            return self.free_bytes()
        except OSError as e:
            raise _Refused(self.redact_text(f"couldn't read the free disk space to check browser.download_min_free_gb "
                                            f"({e}): {what}; not taken"), size) from e

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
            n = int(getattr(ev, "count", None) or 1)
            self._insert(br, owner, ev, "refused", final=True,
                         reason=DROPPED.format(n="this save was dropped" if n == 1 else f"{n} saves were dropped"))
            return
        if ev.uncaptured:
            self._insert(br, owner, ev, "not_taken", reason=_not_taken(ev.detail), final=True)
            return
        if not ev.download_id:
            return
        # From here the held download is a live task tab's: whatever breaks, it is aborted, never left to the
        # extension's idle hand-back (in a task tab that is a Save dialog or a silent ~/Downloads save).
        key = (br.name, int(ev.tab_id))
        try:
            self._tab_task[key] = owner.task
            lock = self._locks.setdefault(key, asyncio.Lock())
            await asyncio.wait_for(lock.acquire(), TAB_WAIT_S)
        except asyncio.TimeoutError:
            await _send(br, ev.download_id, abort=True)
            self._insert(br, owner, ev, "refused", size=ev.size, final=True, reason=(
                "another download from this tab was still being taken (one at a time per tab); not taken"))
            return
        except Exception as e:
            await self._not_admitted(br, owner, ev, e)
            return
        try:
            admitted = await self._admit(br, owner, ev)
            if admitted is not None:
                await self._take(br, owner, ev, *admitted)
        finally:
            lock.release()

    async def _not_admitted(self, br: Any, owner: TaskTab, ev: Any, e: Exception) -> None:
        """Admission broke (e.g. config.toml left half-edited mid-task): abort first, so the browser never gets it,
        then record the failure for the worker, best effort (no record: the event still says why)."""
        await _send(br, ev.download_id, abort=True)
        reason = self.redact_text(f"FERB couldn't check this download against its limits ({type(e).__name__}: {e}); "
                                  "not taken")
        try:
            self._insert(br, owner, ev, "failed", size=ev.size, reason=reason, final=True)
        except Exception:
            self.publish("browser.download", {"profile": br.name, "tab_id": ev.tab_id, "state": "failed",
                                               "reason": reason})

    async def _admit(self, br: Any, owner: TaskTab, ev: Any) -> tuple[Limits, str] | None:
        """The budgets and the free-disk floor, before any byte moves. Returns the limits and the task tree to take it
        under; None when it was refused or admission broke (aborted in the browser and recorded either way)."""
        size = ev.size
        try:
            lim = self.limits()
            tree = self.task_root(owner.task)
            files, taken = self._tree_usage(tree)
            used = taken + self._inflight.get(tree, 0) + self._shell_bytes(tree)
            if files >= lim.task_files:
                raise _Refused(f"this task already took {files} downloads, its limit "
                               "(browser.download_task_max_files, per request); not taken", size)
            if size is not None and used + size > lim.task_bytes:
                raise _Refused(f"would pass this task's {_gb(lim.task_bytes)} download budget "
                               f"(browser.download_task_max_gb, per request, browser and shell together; {used} bytes "
                               f"taken or arriving): {size} bytes; not taken", size)
            what = f"this download is {size} bytes" if size is not None else "its size isn't declared; nothing was read"
            free = self._free(what, size)
            if free - (size or 0) < lim.min_free_bytes:
                raise _Refused(_low_disk(free, lim.min_free_bytes, what), size)
        except _Refused as r:
            await _send(br, ev.download_id, abort=True)
            self._insert(br, owner, ev, "refused", size=r.size, reason=r.reason, final=True)
            return None
        except Exception as e:
            await self._not_admitted(br, owner, ev, e)
            return None
        return lim, tree

    async def _take(self, br: Any, owner: TaskTab, ev: Any, lim: Limits, tree: str) -> None:
        name = safe_name(self.redact_text(ev.name or ""))
        fid = ""
        moved_at, received, digest = self.monotonic(), 0, hashlib.sha256()
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
                left = STALL_S - (self.monotonic() - moved_at)
                if left <= 0:
                    raise _Refused(f"stalled: no bytes arrived for {STALL_S:g} seconds (stopped after {received} "
                                   "bytes); not taken", received)
                asked = time.monotonic()
                try:
                    part = await br.call("download_part", {"download_id": ev.download_id, "offset": received},
                                         timeout=max(left, 1.0))
                except Exception:
                    if time.monotonic() - asked < max(left, 1.0):
                        raise                            # it broke (not a wait that ran out): "didn't finish"
                    raise _Refused(f"stalled: no bytes arrived for {STALL_S:g} seconds (stopped after {received} "
                                   "bytes); not taken", received) from None
                if not isinstance(part, dict) or part.get("offset") != received:
                    raise ValueError("the browser sent download parts out of order")
                data = base64.b64decode(str(part.get("data") or ""), validate=True)
                if data:
                    moved_at = self.monotonic()          # progress: the stall clock starts again
                received += len(data)
                self._inflight[tree] += len(data)
                # what the tree took (other transfers' bytes committed since this one started included), what arrives,
                # and what its shell commands received
                if self._used(tree) > lim.task_bytes:
                    raise _Refused(f"passed this task's {_gb(lim.task_bytes)} download budget "
                                   "(browser.download_task_max_gb, per request, browser and shell together) after "
                                   f"{received} bytes; not taken", received)
                written = received - len(data)
                free = self._free(f"stopped after {written} bytes", written)
                if free - len(data) < lim.min_free_bytes:
                    raise _Refused(_low_disk(free, lim.min_free_bytes, f"stopped after {written} bytes"), written)
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
            self._evict()                               # the folder's cap: the owner is asked when it is over
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

    def _used(self, tree: str) -> int:
        """Bytes counted against the tree's budget: files its tabs took, transfers still arriving, and what its shell
        commands received through the egress proxy."""
        return self._tree_usage(tree)[1] + self._inflight.get(tree, 0) + self._shell_bytes(tree)

    # ---------- the shell's downloads: the same per-tree budget and free-disk floor (one path) ----------

    def _shell_bytes(self, tree: str) -> int:
        if tree not in self._shell:
            r = self.db.conn.execute("SELECT bytes FROM shell_downloads WHERE tree_id = ?", (tree,)).fetchone()
            self._shell[tree] = int(r["bytes"]) if r is not None else 0
        return self._shell[tree]

    def shell_received(self, task: str, n: int) -> None:
        """A shell command of this task received `n` more bytes through FERB's egress proxy: counted against its tree's
        budget, beside its browser downloads (kept, so a daemon restart doesn't start the tree afresh)."""
        if n <= 0:
            return
        tree = self.task_root(task)
        total = self._shell_bytes(tree) + int(n)
        self._shell[tree] = total
        self.db.conn.execute(
            "INSERT INTO shell_downloads (tree_id, bytes, at) VALUES (?, ?, ?) "
            "ON CONFLICT(tree_id) DO UPDATE SET bytes = excluded.bytes, at = excluded.at",
            (tree, total, self.now().isoformat()))

    def shell_stop(self, task: str) -> str | None:
        """Why a shell command of this task that is receiving bytes must stop now, as a fact for the worker; None: it
        may go on. Past the tree's byte budget (browser and shell together), or the disk under the free-disk floor.
        A floor that can't be checked (free space unreadable) stops it too, as for a browser download."""
        lim = self.limits()
        tree = self.task_root(task)
        used = self._used(tree)
        if used > lim.task_bytes:
            return (f"FERB stopped this command: this task has received {_gb(used)} in downloads, past its "
                    f"{_gb(lim.task_bytes)} budget (browser.download_task_max_gb, per request, browser and shell "
                    "together). Don't start another download in this task; say what was and wasn't fetched.")
        try:
            free = self.free_bytes()
        except OSError as e:
            return self.redact_text(f"FERB stopped this command: it couldn't read the free disk space to check "
                                    f"browser.download_min_free_gb ({e}) while the command was downloading.")
        if free < lim.min_free_bytes:
            return (f"FERB stopped this command: only {_gb(free)} free on the disk now; FERB keeps at least "
                    f"{_gb(lim.min_free_bytes)} free (browser.download_min_free_gb) and the command was downloading. "
                    "Tell the owner the disk is nearly full; don't retry the download.")
        return None

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

    # ---------- the folder's total: over the cap, the owner is asked; their Yes deletes the oldest for good ----------

    def _in_folder(self) -> list[dict[str, Any]]:
        """The rows of files kept in the downloads folder, oldest first. A row whose file isn't in the folder is left
        as it is (path and file), recorded once as skipped, and never counted."""
        rows = [dict(r) for r in self.db.conn.execute(
            "SELECT id, path, bytes, at, kept FROM downloads WHERE path IS NOT NULL ORDER BY at, rowid")]
        for r in [r for r in rows if Path(str(r["path"])).parent != self.root]:
            rows.remove(r)
            if r["kept"] != OUTSIDE:
                self.db.conn.execute("UPDATE downloads SET kept = ? WHERE id = ?", (OUTSIDE, r["id"]))
        return rows

    def over_cap(self) -> OverCap | None:
        """The folder over browser.download_total_gb (hands decision 8, owner 2026-10-31): the oldest files whose
        permanent deletion brings it back under, with their count and bytes; None while it is within. Nothing is
        changed: the owner decides (the daemon asks)."""
        lim = self.limits()
        rows = self._in_folder()
        total = sum(int(r["bytes"] or 0) for r in rows)
        if total <= lim.total_bytes:
            return None
        doomed, left = [], total
        for r in rows:
            if left <= lim.total_bytes:
                break
            doomed.append(r)
            left -= int(r["bytes"] or 0)
        return OverCap(tuple(str(r["id"]) for r in doomed), sum(int(r["bytes"] or 0) for r in doomed), total,
                       lim.total_bytes)

    def delete(self, ids: Iterable[str]) -> tuple[int, int]:
        """The owner's Yes (hands decision 8): these downloads' files deleted for good, through the open downloads
        folder and only there: a regular file in it, never a link (nothing outside the folder is ever reached). Each
        row stays with its path cleared (`kept` deleted, or gone when the file had already gone). (files deleted,
        their bytes)."""
        wanted = list(dict.fromkeys(ids))
        if not wanted:
            return 0, 0
        try:
            folder = self._open_folder()
        except OSError:
            return 0, 0
        count = size = 0
        try:
            for r in self._in_folder():
                if str(r["id"]) not in wanted:
                    continue
                name = Path(str(r["path"])).name
                try:
                    st = os.stat(name, dir_fd=folder, follow_symlinks=False)
                except FileNotFoundError:
                    st = None
                if st is None or not stat.S_ISREG(st.st_mode):
                    self.db.conn.execute("UPDATE downloads SET path = NULL, kept = 'gone' WHERE id = ?", (r["id"],))
                    continue
                try:
                    os.unlink(name, dir_fd=folder)
                except OSError as e:
                    self.db.conn.execute("UPDATE downloads SET kept = ? WHERE id = ?",
                                         (self.redact_text(f"stuck: couldn't delete it ({e})")[:300], r["id"]))
                    continue
                self.db.conn.execute("UPDATE downloads SET path = NULL, kept = 'deleted' WHERE id = ?", (r["id"],))
                count, size = count + 1, size + st.st_size
        finally:
            os.close(folder)
        return count, size

    def _evict(self) -> None:
        """After a file is taken: the daemon checks the folder's cap (and asks the owner when it is over)."""
        try:
            self.on_over_cap()
        except Exception:
            pass                                        # the next retention pass checks again

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
        out["kept"] = ("already gone when retention ran" if kept == "gone" else
                       "deleted on the owner's Yes (the downloads folder was over its cap)" if kept == "deleted" else
                       "moved to the Trash (retention)")
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
