"""Owns append-only redacted events. Must not treat retrieved observations as owner authorization or silently cut stored content.

Minimal append-only journal: what happened, from the first run. Writes pass the redaction seam."""

import json
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any, Callable

from ferb.core.interfaces import Clock, IdSource
from ferb.store.db import Database

Redact = Callable[[dict[str, Any]], dict[str, Any]]


@dataclass(frozen=True)
class JournalEntry:
    id: str
    at: datetime
    kind: str                         # e.g. "daemon.started", later "chat.message", "task.outcome", ...
    data: dict[str, Any] = field(default_factory=dict)


class Journal:
    def __init__(self, db: Database, clock: Clock, ids: IdSource, redact: Redact) -> None:
        self._db, self._clock, self._ids, self._redact = db, clock, ids, redact

    def append(self, kind: str, data: dict[str, Any] | None = None) -> JournalEntry:
        """Never dropped: if the write fails, the exception propagates to the caller."""
        entry = JournalEntry(self._ids.new_id(), self._clock.now(), kind, self._redact(data or {}))
        self._db.conn.execute("INSERT INTO journal (id, at, kind, data_json) VALUES (?, ?, ?, ?)",
                              (entry.id, entry.at.isoformat(), entry.kind, json.dumps(entry.data)))
        return entry

    def entries(self, since: datetime | None = None) -> list[JournalEntry]:
        rows = self._db.conn.execute("SELECT * FROM journal ORDER BY seq").fetchall()
        out = [JournalEntry(r["id"], datetime.fromisoformat(r["at"]), r["kind"], json.loads(r["data_json"]))
               for r in rows]
        return [e for e in out if since is None or e.at >= since]
