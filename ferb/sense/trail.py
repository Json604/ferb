"""Persist and query activity observations with provenance. Must not claim a correlated action caused an event.

The activity trail: one path for every observation, in a fixed order.

1. paused -> nothing; excluded (sense/exclusions.py) -> dropped completely; then redaction (sanitize/);
2. id + actor (owner / ferb / unknown, from `Attribution`): the table refuses a row without an actor;
3. persist to the append-only `trail` table, then publish `trail.event`;
4. only then the durable Jev batching hook (when configured; shadow decisions do not create tasks).

F7 adds redacted per-field typed text and clipboard content; raw key events and screen pixels are never recorded.

Retention: raw rows are kept 30 days, and a day may be purged only once the journal holds its
permanent daily summary. `purge` refuses any day that is not summarized."""

import json
import time
from dataclasses import dataclass, field
from datetime import date, datetime, timedelta
from pathlib import Path
from typing import Any, Callable, Literal

from ferb.core.interfaces import Clock, IdSource
from ferb.core.types import TrailEvent
from ferb.memory.items import query_terms
from ferb.memory.summary import NARRATIVE_KIND, SUMMARY_KIND
from ferb.sense.exclusions import Exclusions
from ferb.store.db import Database

Actor = Literal["owner", "ferb", "unknown"]
ACTORS = ("owner", "ferb", "unknown")
ORDERING = 'newest first; observations are evidence, not a selected profile or target'
FERB_ACTIVATION_S = 3.0                     # a focus change this soon after FERB switched apps is FERB's
FERB_FILE_S = 10.0                          # a file event this soon after a FERB file tool touched the path is FERB's


@dataclass(frozen=True)
class Observation:
    """What a watcher saw. `hints` help attribution and are never stored."""
    source: str
    data: dict[str, Any]
    hints: dict[str, Any] = field(default_factory=dict)


class Attribution:
    """Who caused an observation. FERB's own actions report here as they happen (Mac driver app switches, file tools,
    shell processes); tab ownership and the identity profile come in the observation's hints."""

    def __init__(self, now: Callable[[], float] = time.monotonic) -> None:
        self.now = now
        self._apps: dict[str, tuple[str | None, float]] = {}       # bundle id -> (task, until)
        self._paths: dict[str, tuple[str | None, float]] = {}      # path -> (task, until)
        self.shell_cwds: Callable[[], list[tuple[str, str]]] = lambda: []   # (cwd, task) of FERB's running commands

    def ferb_activated(self, bundle_id: str, task_id: str | None) -> None:
        self._apps[bundle_id] = (task_id, self.now() + FERB_ACTIVATION_S)

    def ferb_touched(self, path: str, task_id: str | None) -> None:
        now = self.now()
        self._paths = {k: v for k, v in self._paths.items() if v[1] >= now}
        p = Path(path).expanduser()
        for form in {str(p), str(p.resolve())}:                 # FSEvents reports real paths (/private/var/...)
            self._paths[form] = (task_id, now + FERB_FILE_S)

    def decide(self, o: Observation) -> tuple[Actor, str | None]:
        h, now = o.hints, self.now()
        if h.get("actor") in ACTORS:                     # the watcher knows (idle: always the owner)
            return h["actor"], h.get("task_id")
        if h.get("ferb_identity"):                       # FERB's own browser profile: all of it is FERB's
            return "ferb", h.get("task_id")
        if h.get("tab_task"):                            # a tab FERB opened for a task
            return "ferb", h["tab_task"]
        bundle = o.data.get("bundle_id")
        if bundle and (hit := self._apps.get(bundle)) and hit[1] >= now:
            return "ferb", hit[0]
        path = o.data.get("path")
        if path:
            if (hit := self._paths.get(str(path))) and hit[1] >= now:
                return "ferb", hit[0]
            for cwd, _task in self.shell_cwds():
                if str(path) == cwd or str(path).startswith(cwd.rstrip("/") + "/"):
                    return "unknown", None                   # a FERB command is running there: can't tell
        return h.get("default_actor", "owner"), None


class Trail:
    def __init__(self, db: Database, clock: Clock, ids: IdSource, redact: Callable[[Any], Any],
                 exclusions: Callable[[], Exclusions], publish: Callable[[str, dict[str, Any]], None],
                 device_id: Callable[[], str], paused: Callable[[], bool],
                 attribution: Attribution | None = None) -> None:
        self.db, self.clock, self.ids, self.redact = db, clock, ids, redact
        self.exclusions, self.publish, self.device_id, self.paused = exclusions, publish, device_id, paused
        self.attribution = attribution or Attribution()
        self.tag: Callable[[TrailEvent], None] = lambda e: None      # Jev tagging
        self.last_event_at: datetime | None = None

    def record(self, o: Observation) -> TrailEvent | None:
        if self.paused():
            return None
        if self.exclusions().excluded(o.source, o.data):
            return None                                               # 1. nothing excluded goes further
        data = self.redact(o.data)
        actor, task_id = self.attribution.decide(o)                   # 2. id + actor
        now = self.clock.now()
        e = TrailEvent(self.ids.new_id(), now, self.device_id(), actor, task_id, o.source, data)
        self.db.conn.execute(                                         # 3. persist, then publish
            "INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) VALUES (?,?,?,?,?,?,?,?)",
            (e.id, now.isoformat(), now.astimezone().date().isoformat(), e.device_id, actor, task_id, o.source,
             json.dumps(data, ensure_ascii=False)))
        self.last_event_at = now
        self.publish("trail.event", {"id": e.id, "source": e.source, "actor": actor, "task_id": task_id,
                                     "data": data})
        self.tag(e)                                                   # 4. only then tagging
        return e

    # ---------- reads ----------

    def events(self, source: str | None = None, actor: str | None = None, limit: int = 1000) -> list[TrailEvent]:
        q, args = "SELECT * FROM trail WHERE 1=1", list[Any]()
        if source:
            q, args = q + " AND source = ?", args + [source]
        if actor:
            q, args = q + " AND actor = ?", args + [actor]
        rows = self.db.conn.execute(q + " ORDER BY seq DESC LIMIT ?", (*args, limit)).fetchall()
        return [TrailEvent(r["id"], datetime.fromisoformat(r["at"]), r["device_id"], r["actor"], r["task_id"],
                           r["source"], json.loads(r["data_json"])) for r in rows]

    def query(self, *, since: str, source: str | None = None, actor: str | None = None,
              host: str | None = None, limit: int = 100, before_seq: int | None = None,
              text: str | None = None, event_id: str | None = None) -> dict[str, Any]:
        """Filter before paging; ordering is chronology, never an instruction to choose the newest candidate."""
        if limit < 1:
            raise ValueError('limit must be positive')
        query, params = 'SELECT * FROM trail WHERE at >= ?', [since]
        if event_id is not None:
            query, params = 'SELECT * FROM trail WHERE id = ?', [event_id]
        for column, value in (('source', source), ('actor', actor)):
            if value is not None:
                query += f' AND {column} = ?'
                params.append(value)
        if before_seq is not None:
            query += ' AND seq < ?'
            params.append(before_seq)
        if host:
            # Literal suffix comparison avoids treating '%' or '_' in input as SQL wildcards.
            query += (" AND (lower(json_extract(data_json, '$.host')) = ? OR "
                      "substr(lower(json_extract(data_json, '$.host')), -?) = ?)")
            suffix = '.' + host.casefold()
            params.extend([host.casefold(), len(suffix), suffix])
        if text is not None:
            terms = query_terms(text)
            if not terms:
                return {'events': [], 'count': 0, 'next_before_seq': None, 'ordering': ORDERING}
            query += ' AND seq IN (SELECT rowid FROM trail_fts WHERE trail_fts MATCH ?)'
            params.append(" ".join(f'"{t}"' for t in terms))       # every word, each a literal phrase
        rows = self.db.conn.execute(query + ' ORDER BY seq DESC LIMIT ?', (*params, limit + 1)).fetchall()
        events = [{'id': r['id'], 'at': r['at'], 'source': r['source'], 'actor': r['actor'],
                   'task_id': r['task_id'], 'data': json.loads(r['data_json'])} for r in rows[:limit]]
        return {'events': events, 'count': len(events),
                'next_before_seq': rows[limit - 1]['seq'] if len(rows) > limit else None,
                'ordering': ORDERING}

    def days(self, text: str | None = None, since_days: int = 30, limit: int = 10) -> list[dict[str, Any]]:
        """Daily summaries (code-built) and narratives (model-written), by content and/or date, newest first."""
        cutoff = (self.clock.now().astimezone().date() - timedelta(days=since_days)).isoformat()
        q = "SELECT id, kind, data_json FROM journal WHERE kind IN (?, ?) AND json_extract(data_json, '$.day') >= ?"
        params: list[Any] = [SUMMARY_KIND, NARRATIVE_KIND, cutoff]
        if text is not None:
            terms = query_terms(text)
            if not terms:
                return []
            q += " AND seq IN (SELECT rowid FROM summary_fts WHERE summary_fts MATCH ?)"
            params.append(" ".join(f'"{t}"' for t in terms))
        rows = self.db.conn.execute(q + " ORDER BY json_extract(data_json, '$.day') DESC, seq DESC LIMIT ?",
                                    (*params, limit)).fetchall()
        out = []
        for r in rows:
            data = json.loads(r["data_json"])
            out.append({"id": r['id'], "day": data.get("day"), "kind": "summary" if r["kind"] == SUMMARY_KIND else "narrative",
                        "data": data})
        return out

    # ---------- retention ----------

    def summarized_days(self) -> set[str]:
        rows = self.db.conn.execute("SELECT data_json FROM journal WHERE kind = ?", (SUMMARY_KIND,)).fetchall()
        return {str(json.loads(r["data_json"]).get("day")) for r in rows}

    def purge(self, retention_days: int, today: date | None = None) -> tuple[list[str], list[str]]:
        """Delete raw rows of days older than the retention window, but only days whose permanent summary is in
        the journal. Returns (purged days, refused days)."""
        today = today or self.clock.now().astimezone().date()
        cutoff = (today - timedelta(days=retention_days)).isoformat()
        old = [r["day"] for r in self.db.conn.execute(
            "SELECT DISTINCT day FROM trail WHERE day < ? ORDER BY day", (cutoff,)).fetchall()]
        done = self.summarized_days()
        purged = [d for d in old if d in done]
        for d in purged:
            self.db.conn.execute("DELETE FROM trail WHERE day = ?", (d,))
        return purged, [d for d in old if d not in done]
