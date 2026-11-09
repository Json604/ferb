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
        """Never dropped: if the write fails, the exception propagates to the caller. FERB's own ids (task_id,
        request_id, session_id, covered, source_refs) are kept whole by key, as in span inputs (`Redactor.ident`)."""
        body = self._redact(data or {})
        entry = JournalEntry(self._ids.new_id(), self._clock.now(), kind, body,
                             origin or journal_origin(kind, body))
        self._db.conn.execute("INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?)",
                              (entry.id, entry.at.isoformat(), entry.kind, json.dumps(entry.data, ensure_ascii=False),
                               entry.origin))
        return entry

    def entries(self, since: datetime | None = None, limit: int | None = None) -> list[JournalEntry]:
        """Rows from `since` on, oldest first; with `limit`, only the newest `limit` of them. Filtered in SQL (its time
        functions read the stored offsets), never by parsing the whole journal."""
        where, params = ("WHERE julianday(at) >= julianday(?)", [since.isoformat()]) if since is not None else ("", [])
        if limit is None:
            rows = self._db.conn.execute(f"SELECT * FROM journal {where} ORDER BY seq", params).fetchall()
        else:
            rows = self._db.conn.execute(f"SELECT * FROM journal {where} ORDER BY seq DESC LIMIT ?",
                                         [*params, limit]).fetchall()[::-1]
        return [_entry(r) for r in rows]

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

    def recent_values(self, kind: str, field: str, limit: int) -> list[Any]:
        """One field of a kind's newest `limit` rows, newest first (e.g. the last start's facts)."""
        return [row[0] for row in self._db.conn.execute(
            "SELECT json_extract(data_json, ?) FROM journal WHERE kind = ? ORDER BY seq DESC LIMIT ?",
            ("$." + field, kind, limit))]

    def by_ids(self, ids: list[str]) -> list[JournalEntry]:
        """These rows, oldest first (ids that aren't there are skipped)."""
        if not ids:
            return []
        return [_entry(r) for r in self._db.conn.execute(
            f"SELECT * FROM journal WHERE id IN ({','.join('?' * len(ids))}) ORDER BY seq", ids)]

    def latest_kind(self, kinds: tuple[str, ...], field: str, equals: str) -> str | None:
        """The kind of the newest row among `kinds` whose data holds `equals` at `field`; None when there is none."""
        row = self._db.conn.execute(
            f"SELECT kind FROM journal WHERE kind IN ({','.join('?' * len(kinds))}) AND json_extract(data_json, ?) = ? "
            "ORDER BY seq DESC LIMIT 1", (*kinds, "$." + field, equals)).fetchone()
        return str(row[0]) if row is not None else None

    def of_running_tasks(self, kind: str) -> list[JournalEntry]:
        """A kind's rows, oldest first, whose `task_id` names a task running now."""
        return [_entry(r) for r in self._db.conn.execute(
            "SELECT * FROM journal WHERE kind = ? AND json_extract(data_json, '$.task_id') IN "
            "(SELECT id FROM tasks WHERE status = 'running') ORDER BY seq", (kind,))]


def _entry(r: Any) -> JournalEntry:
    data = json.loads(r["data_json"])
    return JournalEntry(r["id"], datetime.fromisoformat(r["at"]), r["kind"], data,
                        r["origin"] or journal_origin(r["kind"], data))
