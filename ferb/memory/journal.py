"""Owns append-only redacted events. Must not treat retrieved observations as owner authorization or silently cut stored content.

Minimal append-only journal: what happened, from the first run. Writes pass the redaction seam."""

import json
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any, Callable

from ferb.core import origin as origins_
from ferb.core.interfaces import Clock, IdSource
from ferb.memory.summary import NARRATIVE_KIND, SUMMARY_KIND, digest_origins
from ferb.store.db import Database

Redact = Callable[[dict[str, Any]], dict[str, Any]]


@dataclass(frozen=True)
class JournalEntry:
    id: str
    at: datetime
    kind: str                         # e.g. "daemon.started", later "chat.message", "task.outcome", ...
    data: dict[str, Any] = field(default_factory=dict)
    origin: str = ""                  # comma-joined origins (ferb/core/origin.py)


OWNER_KINDS = {"owner.verdict", "memory.added", "memory.edited", "approval.resolved"}   # the owner's own words/answers


def journal_origin(kind: str, data: dict[str, Any]) -> str:
    """A journal row's origins by kind. A chat message also carries external:paste when it held pasted parts, or
    external:unlabeled when it was recorded before FERB counted them."""
    if kind == "chat.message":
        parts = data.get("untrusted_parts")
        extra = {origins_.external("unlabeled")} if parts is None else (
            {origins_.external("paste")} if parts else set())
        return origins_.join({origins_.OWNER} | extra)
    if kind in OWNER_KINDS:
        return origins_.OWNER
    if kind in (SUMMARY_KIND, NARRATIVE_KIND):         # by what the day's digest or narrative holds
        return origins_.join(digest_origins(kind, data))
    return origins_.FERB


class Journal:
    def __init__(self, db: Database, clock: Clock, ids: IdSource, redact: Redact) -> None:
        self._db, self._clock, self._ids, self._redact = db, clock, ids, redact

    @property
    def db(self) -> Database:
        """The database the journal writes to; records that live beside it (continuity, episodes) share it."""
        return self._db

    def append(self, kind: str, data: dict[str, Any] | None = None,
               origin: str | None = None) -> JournalEntry:
        """Never dropped: if the write fails, the exception propagates to the caller."""
        body = self._redact(data or {})
        entry = JournalEntry(self._ids.new_id(), self._clock.now(), kind, body,
                             origin or journal_origin(kind, body))
        self._db.conn.execute("INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?)",
                              (entry.id, entry.at.isoformat(), entry.kind, json.dumps(entry.data), entry.origin))
        return entry

    def entries(self, since: datetime | None = None) -> list[JournalEntry]:
        rows = self._db.conn.execute("SELECT * FROM journal ORDER BY seq").fetchall()
        return [e for e in map(_entry, rows) if since is None or e.at >= since]

    def entries_of(self, *kinds: str) -> list[JournalEntry]:
        """These kinds' rows, oldest first, read through the journal_kind index (not the whole journal)."""
        rows = self._db.conn.execute(f"SELECT * FROM journal WHERE kind IN ({','.join('?' * len(kinds))}) ORDER BY seq",
                                     kinds)
        return [_entry(r) for r in rows]

    def of_kind(self, kind: str, field: str = "", equals: str = "", missing: str | None = None) -> list[JournalEntry]:
        """One kind's rows, oldest first, read through the journal_kind index; with `field`, only rows whose data holds
        `equals` there (`missing`: what a row without that field counts as, e.g. "owner" for a pre-session chat)."""
        sql = "SELECT * FROM journal WHERE kind = ?"
        params: list[Any] = [kind]
        if field:
            sql += " AND COALESCE(json_extract(data_json, ?), ?) = ?"
            params += ["$." + field, missing, equals]
        return [_entry(r) for r in self._db.conn.execute(sql + " ORDER BY seq", params)]


def _entry(r: Any) -> JournalEntry:
    data = json.loads(r["data_json"])
    return JournalEntry(r["id"], datetime.fromisoformat(r["at"]), r["kind"], data,
                        r["origin"] or journal_origin(r["kind"], data))
