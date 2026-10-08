"""Owns taking a browser download into FERB's workspace as facts (D35); must not judge the file or pick a source.

The extension catches a download only in a tab FERB drives for a task and holds it there; nothing reaches the browser's
own download flow, so no Save dialog. This side pulls the bytes in parts (`download_part`), hashes what it received,
enforces `browser.download_max_mb`, writes `~/.ferb/workspace/downloads/<id>-<safe name>` (never elsewhere) and keeps a
record of facts, never content. The file is outside content (MP2): `external:download`, and the folder is always an
untrusted fs root. A download the extension couldn't intercept, or one it could not take, is a fact for the worker."""

import asyncio
import base64
import binascii
import hashlib
import os
import secrets
import stat
import unicodedata
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Iterable

from ferb.core import origin
from ferb.core.errors import DriverError

ORIGIN = origin.external("download")
FOLDER = "downloads"                    # under FERB's workspace
NAME_MAX_BYTES = 150                    # the safe name; the id prefix adds 15 (APFS allows 255 bytes)
DEFAULT_MAX_MB = 100                    # browser.download_max_mb
PART_TIMEOUT_S = 30.0
SETTLE_S = 60.0                         # a navigation/open waits this long for a download its tab started
_DROP = {"Cc", "Cf", "Cs", "Zl", "Zp"}  # controls, format (bidi overrides, zero-width), surrogates, line separators

Publish = Callable[[str, dict[str, Any]], None]


@dataclass(frozen=True)
class TaskTab:
    """Whose tab a download came from: the task that drives it, its alias, and the profile's identity."""
    task: str
    tab: str
    identity: str                       # owner | ferb


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
    """The fs roots whose files are outside content: the owner's configured ones plus FERB's downloads folder, which
    code adds so config can't remove it (MP2)."""
    out = [str(p) for p in configured]
    folder = str(workspace / FOLDER)
    return tuple(out if folder in out else [*out, folder])


def _mb(n: int) -> str:
    return f"{n / (1024 * 1024):g} MB"


class Downloads:
    """One per daemon, shared by the owner's and FERB's browser drivers."""

    def __init__(self, root: Path, db: Any, now: Callable[[], Any], max_mb: Callable[[], Any],
                 redact_url: Callable[[str], str], publish: Publish) -> None:
        self.root, self.db, self.now, self.max_mb, self.redact_url, self.publish = (
            root, db, now, max_mb, redact_url, publish)
        self.task_tab: Callable[[Any, int], TaskTab | None] = lambda br, chrome_id: None   # the drivers answer
        self._active: dict[tuple[str, int], set[asyncio.Task[None]]] = {}
        self._unreported: dict[tuple[str, int], list[dict[str, Any]]] = {}

    def max_bytes(self) -> int:
        try:
            mb = float(self.max_mb())
        except (TypeError, ValueError):
            mb = DEFAULT_MAX_MB
        return int(max(mb, 0) * 1024 * 1024)

    # ---------- the extension's download event ----------

    def receive(self, br: Any, ev: Any) -> None:
        """A download the extension caught (held) or saw the browser start itself. Runs as its own task: its parts
        come back over the same connection the event arrived on."""
        key = (br.name, int(ev.tab_id))
        job = asyncio.get_running_loop().create_task(self._receive(br, ev))
        self._active.setdefault(key, set()).add(job)

        def done(t: asyncio.Task[None]) -> None:
            jobs = self._active.get(key)
            if jobs is not None:
                jobs.discard(t)
                if not jobs:
                    self._active.pop(key, None)
        job.add_done_callback(done)

    async def _receive(self, br: Any, ev: Any) -> None:
        owner = self.task_tab(br, int(ev.tab_id))
        if owner is None:                               # not a tab FERB drives for a task: let it go, keep nothing
            if not ev.uncaptured and ev.download_id:
                await _abort(br, ev.download_id)
            self.publish("browser.download", {"profile": br.name, "tab_id": ev.tab_id, "state": "ignored",
                                               "reason": "not a task tab"})
            return
        if ev.uncaptured:
            self._record(br, owner, ev, "not_taken", reason=(
                "the browser started this download itself (a link FERB couldn't intercept, e.g. a script-made blob "
                "or data link); FERB couldn't take or stop it, so the browser may have shown its own Save dialog "
                "and saved it where it saves downloads"))
            return
        if not ev.download_id:
            return
        limit, size = self.max_bytes(), ev.size
        if size is not None and size > limit:
            await _abort(br, ev.download_id)
            self._record(br, owner, ev, "refused", size=size, reason=(
                f"larger than the {_mb(limit)} limit (browser.download_max_mb): {size} bytes; not taken"))
            return
        await self._take(br, owner, ev, limit)

    async def _take(self, br: Any, owner: TaskTab, ev: Any, limit: int) -> None:
        fid = "dl-" + secrets.token_hex(6)
        name = safe_name(ev.name or "")
        final, temp = f"{fid}-{name}", f".{fid}.part"
        try:
            folder = self._open_folder()
        except OSError as e:
            await _abort(br, ev.download_id)
            self._record(br, owner, ev, "failed", fid=fid, reason=f"FERB's downloads folder is unusable ({e})")
            return
        received, digest, moved = 0, hashlib.sha256(), False
        fd = -1
        try:
            fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=folder)
            while True:
                part = await br.call("download_part", {"download_id": ev.download_id, "offset": received},
                                     timeout=PART_TIMEOUT_S)
                if not isinstance(part, dict) or part.get("offset") != received:
                    raise ValueError("the browser sent download parts out of order")
                data = base64.b64decode(str(part.get("data") or ""), validate=True)
                received += len(data)
                if received > limit:
                    await _abort(br, ev.download_id)
                    self._record(br, owner, ev, "refused", fid=fid, size=received, reason=(
                        f"larger than the {_mb(limit)} limit (browser.download_max_mb): stopped after {received} "
                        "bytes; not taken"))
                    return
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
            self._record(br, owner, ev, "taken", fid=fid, name=name, path=str(self.root / final), size=received,
                         sha256=digest.hexdigest())
        except (DriverError, ValueError, binascii.Error, OSError) as e:
            await _abort(br, ev.download_id)
            self._record(br, owner, ev, "failed", fid=fid, size=received or None,
                         reason=f"the transfer didn't finish ({e}); nothing was kept")
        finally:
            if fd >= 0:
                os.close(fd)
            if not moved:
                try:
                    os.unlink(temp, dir_fd=folder)
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

    def _record(self, br: Any, owner: TaskTab, ev: Any, state: str, *, fid: str | None = None, name: str | None = None,
                path: str | None = None, size: int | None = None, sha256: str | None = None,
                reason: str | None = None) -> None:
        row = {"id": fid or "dl-" + secrets.token_hex(6), "task_id": owner.task, "profile": br.name,
               "identity": owner.identity, "tab": owner.tab, "state": state, "path": path,
               "name": name if name is not None else (safe_name(ev.name) if ev.name else None), "bytes": size,
               "sha256": sha256, "mime": (ev.mime or None), "source_url": self.redact_url(ev.source_url or "") or None,
               "reason": reason, "origin": ORIGIN, "at": self.now().isoformat()}
        self.db.conn.execute(f"INSERT INTO downloads ({', '.join(row)}) VALUES ({', '.join('?' * len(row))})",
                             tuple(row.values()))
        fact = _fact(row)
        self._unreported.setdefault((br.name, int(ev.tab_id)), []).append(fact)
        self.publish("browser.download", {"profile": br.name, "task_id": owner.task, **fact})

    def unreported(self, br: Any, chrome_id: int) -> list[dict[str, Any]]:
        """Downloads from this tab the worker hasn't seen in a result yet (each is reported once per tab)."""
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


def _fact(row: dict[str, Any]) -> dict[str, Any]:
    """What the worker sees: facts about the file, labeled outside content."""
    keys = {"id": "download", "state": "state", "tab": "tab", "path": "path", "name": "name", "sha256": "sha256",
            "mime": "mime", "source_url": "source_url", "reason": "reason", "origin": "origin"}
    out = {k: row[src] for src, k in keys.items() if row.get(src) not in (None, "")}
    if row.get("bytes") is not None:
        out["bytes" if row["state"] == "taken" else "size"] = row["bytes"]
    return out


async def _abort(br: Any, download_id: str | None) -> None:
    """Let the extension fail the held response; best effort (a dropped connection already ended it)."""
    if not download_id:
        return
    try:
        await br.call("download_part", {"download_id": download_id, "abort": True}, timeout=10)
    except DriverError:
        pass
