"""Read Notification Center through FDA, including records received under DND. Never invoke banner actions.

Records are read incrementally by rowid; the request dictionary uses Notification Center's 4-letter keys (`titl`,
`body`, `thre` for the thread identifier; I4)."""
import asyncio
import hashlib
import plistlib
import sqlite3
from pathlib import Path
from typing import Any, Callable

from ferb.sense.trail import Observation
from ferb.sense.watchers.base import Emit

DATABASE = Path.home() / "Library/Group Containers/group.com.apple.usernoted/db2/db"


class NotificationDatabase:
    def __init__(self, path: Path, redact: Callable[[Any], Any],
                 excluded: Callable[[str, str], bool] = lambda bundle, name: False) -> None:
        self.path, self.redact, self.excluded = path, redact, excluded
        self.after = 0                   # the last record rowid read; the first read after a start reads them all

    def read(self) -> list[Observation]:
        """Records added since the previous read. Rowids reused after Notification Center deleted its newest records
        are read again from the start; the caller's seen-receipts drop what was already recorded."""
        out = []
        # mode=ro permits SQLite's normal WAL reader. immutable=1 would miss notifications in the WAL.
        with sqlite3.connect(self.path.as_uri() + "?mode=ro", uri=True) as db:
            tables = {r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
            if not {"record", "app"} <= tables:
                raise RuntimeError("Notification Center schema is unsupported")
            records = {r[1] for r in db.execute("PRAGMA table_info(record)")}
            apps = {r[1] for r in db.execute("PRAGMA table_info(app)")}
            if not {"uuid", "app_id", "data"} <= records or not {"app_id", "identifier"} <= apps:
                raise RuntimeError("Notification Center schema is unsupported")
            after = self.after
            if int(db.execute("SELECT COALESCE(MAX(rowid), 0) FROM record").fetchone()[0]) < after:
                after = 0
            last = after
            for rowid, uid, bundle, blob in db.execute("SELECT r.rowid, r.uuid, a.identifier, r.data FROM record r JOIN app a "
                                                       "ON r.app_id=a.app_id WHERE r.rowid > ? ORDER BY r.rowid", (after,)):
                last = max(last, int(rowid))
                bundle = str(bundle)
                if self.excluded(bundle, bundle):
                    continue
                external = hashlib.sha256(bytes(uid) if isinstance(uid, bytes) else str(uid).encode()).hexdigest()
                try:
                    content = plistlib.loads(bytes(blob))
                    req = content.get("req", content)
                    if not isinstance(req, dict):
                        raise ValueError("notification request is not a dictionary")
                    title = str(req.get("titl", req.get("title", "")))
                    body = str(req.get("body", ""))
                    # A provided thread identifier (`thre`) is stable; a title fallback is explicitly identified.
                    thread = req.get("thre")
                    data = {"external_id": external, "bundle_id": bundle, "title": title, "body": body,
                            "conversation": str(thread) if thread else title,
                            "conversation_source": "thread" if thread else "title"}
                    out.append(Observation("notification", self.redact(data), {"actor": "unknown"}))
                except (AttributeError, ValueError, TypeError, OverflowError, plistlib.InvalidFileException):
                    out.append(Observation("notification", {"external_id": external, "bundle_id": bundle,
                        "decode_error": "unsupported notification payload"}, {"actor": "unknown"}))
        self.after = last                # only once the whole read succeeded
        return out


class NotificationWatcher:
    source = "notification"

    def __init__(self, reader: NotificationDatabase, seen: Callable[[str], bool],
                 record: Callable[[Observation], None], interval_s: float = 1.0,
                 floor: Callable[[list[Observation]], list[Observation]] = lambda observations: observations) -> None:
        self.reader, self.seen, self.record, self.interval_s = reader, seen, record, interval_s
        self.floor = floor               # I1: what was there at install is marked seen, never recorded

    async def run(self, emit: Emit, stop: asyncio.Event) -> None:
        while not stop.is_set():
            observations = self.floor(await asyncio.to_thread(self.reader.read))
            for observation in observations:
                if stop.is_set():
                    return
                if not self.seen(str(observation.data["external_id"])):
                    self.record(observation)  # event and persistent external ID recorded together on daemon loop
            try:
                await asyncio.wait_for(stop.wait(), self.interval_s)
            except asyncio.TimeoutError:
                pass
