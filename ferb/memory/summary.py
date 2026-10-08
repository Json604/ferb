"""Owns durable daily evidence summaries. Must not treat retrieved observations as owner authorization or silently cut stored content.

The permanent daily summary: written by code from the day's trail and journal, with no model, so
it always exists before the raw rows may be purged.

The digest keeps the journey: per hour, the owner's main apps and sites; for the day, time per app, sites, folders
whose files changed, window titles, idle time, the owner's chats and the tasks FERB ran. What FERB itself did is a
separate section, never mixed into the owner's.

The same pass hands distillation its evidence as handles, each naming its sources:
- E#: the owner's own trail rows (observed items may cite only these);
- C#: the owner's own words in chat (pasted parts removed);
- P#: text the owner pasted into chat (third-party);
- F#: FERB's own activity (never evidence for the owner's habits or preferences)."""

import json
import re
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any

from ferb.core import origin as origins_
from ferb.store.db import Database

SUMMARY_KIND, NARRATIVE_KIND = "trail.day_summary", "trail.day_narrative"
SUMMARY_MADE_BY = "code:summary/1"                # MP1: derived rows say how they were made
UNTRUSTED = re.compile(r'<untrusted-data id="([0-9a-f]+)">\n(.*?)\n</untrusted-data id="\1">', re.S)
Source = tuple[str, str]


def digest_origins(kind: str, data: dict[str, Any]) -> set[str]:
    """Every origin a day summary or narrative holds (MP2), decided by what it contains: window titles and site hosts
    are outside text; app and folder data is the owner's own; FERB's task summaries are model-written; a narrative is
    model-written from that evidence. An empty digest is the owner's own (code-built)."""
    web = origins_.external("web", "trail")
    if kind == NARRATIVE_KIND:
        return {origins_.INFERRED, web}
    out: set[str] = set()
    if data.get("windows") or data.get("sites"):
        out.add(web)
    if data.get("apps_min") or data.get("folders"):
        out.add(origins_.PRIVATE)
    ferb = data.get("ferb")
    if isinstance(ferb, dict) and ferb.get("tasks"):
        out.add(origins_.INFERRED)
    return out or {origins_.PRIVATE}


@dataclass
class DayEvidence:
    day: str
    digest: dict[str, Any]
    lines: list[str] = field(default_factory=list)                 # what distillation reads, one handle per line
    handles: dict[str, list[Source]] = field(default_factory=dict)

    def add(self, prefix: str, text: str, sources: list[Source]) -> None:
        h = f"{prefix}{sum(1 for k in self.handles if k.startswith(prefix)) + 1}"
        self.handles[h] = sources
        self.lines.append(f"{h}: {text}")


def _rows(db: Database, day: str) -> list[Any]:
    rows = db.conn.execute("SELECT * FROM trail WHERE day = ? ORDER BY seq", (day,)).fetchall()
    return sorted(rows, key=lambda r: datetime.fromisoformat(r["at"]))     # by time (offsets may differ)


def _journal(db: Database, day: str) -> list[Any]:
    out = []
    for r in db.conn.execute("SELECT * FROM journal WHERE kind IN ('chat.message', 'task.outcome') ORDER BY seq"):
        if datetime.fromisoformat(r["at"]).astimezone().date().isoformat() == day:
            out.append(r)
    return out


def build(db: Database, day: str) -> DayEvidence:
    rows = _rows(db, day)
    ev = DayEvidence(day, {})
    owner = [r for r in rows if r["actor"] == "owner"]
    data = {r["id"]: json.loads(r["data_json"]) for r in rows}

    # time per app (owner focus until the next focus change or going idle; a gap is capped at an hour)
    app_s: dict[str, float] = defaultdict(float)
    app_ids: dict[str, list[Source]] = defaultdict(list)
    hourly: dict[int, Counter[str]] = defaultdict(Counter)
    current: tuple[str, datetime] | None = None
    for r in rows:
        at = datetime.fromisoformat(r["at"]).astimezone()
        d = data[r["id"]]
        if current is not None and (r["source"] == "app_focus" or (r["source"] == "idle" and d.get("state") == "idle")):
            app_s[current[0]] += min((at - current[1]).total_seconds(), 3600)
            current = None
        if r["source"] == "app_focus":
            name = str(d.get("app") or d.get("bundle_id"))
            if r["actor"] == "owner":
                current = (name, at)
                app_ids[name].append(("trail", r["id"]))
                hourly[at.hour][name] += 1
    sites: dict[str, list[Any]] = defaultdict(list)
    folders: dict[str, Counter[str]] = defaultdict(Counter)
    folder_ids: dict[str, list[Source]] = defaultdict(list)
    titles: Counter[str] = Counter()
    title_ids: dict[str, list[Source]] = defaultdict(list)
    idle_s, idle_from = 0.0, None
    for r in owner:
        d, at = data[r["id"]], datetime.fromisoformat(r["at"]).astimezone()
        if r["source"] == "browser_tab" and d.get("host"):
            sites[str(d["host"])].append(r)
            hourly[at.hour][str(d["host"])] += 1
        elif r["source"] == "file":
            folder = str(Path(str(d.get("path", ""))).parent)
            folders[folder][str(d.get("change"))] += 1
            folder_ids[folder].append(("trail", r["id"]))
        elif r["source"] == "window" and d.get("title"):
            key = f"{d.get('app')}: {d['title']}"
            titles[key] += 1
            title_ids[key].append(("trail", r["id"]))
        elif r["source"] == "idle":
            if d.get("state") == "idle":
                idle_from = at
            elif idle_from is not None:
                idle_s += (at - idle_from).total_seconds()
                idle_from = None
    top_apps = sorted(app_s.items(), key=lambda kv: -kv[1])
    for name, secs in top_apps:
        ev.add("E", f"the owner used {name} for about {round(secs / 60)} min", app_ids[name])
    for host, rs in sorted(sites.items(), key=lambda kv: -len(kv[1])):
        profiles = sorted({f"{json.loads(r['data_json']).get('browser')}/{json.loads(r['data_json']).get('profile')}"
                           for r in rs})
        ev.add("E", f"the owner visited {host} {len(rs)} time(s) ({', '.join(profiles)})", [("trail", r["id"]) for r in rs])
    for folder, c in sorted(folders.items(), key=lambda kv: -sum(kv[1].values())):
        ev.add("E", f"files changed in {folder}: {dict(c)}", folder_ids[folder])
    for t, n in titles.most_common():
        ev.add("E", f"window '{t}' ({n}x)", title_ids[t])
    ferb_counts = Counter(r["source"] for r in rows if r["actor"] == "ferb")
    if ferb_counts:
        ev.add("F", f"FERB's own activity: {dict(ferb_counts)}",
               [("trail", r["id"]) for r in rows if r["actor"] == "ferb"])
    chats, tasks = 0, []
    for j in _journal(db, day):
        d = json.loads(j["data_json"])
        if j["kind"] == "chat.message":
            chats += 1
            text = str(d.get("text", ""))
            own = UNTRUSTED.sub("[pasted]", text).strip()
            if own:
                ev.add("C", f"the owner said: {own}", [("journal", j["id"])])
            for m in UNTRUSTED.finditer(text):
                ev.add("P", f"text the owner pasted (third-party): {m.group(2)}", [("journal", j["id"])])
        else:
            tasks.append({"status": d.get("status"), "summary": str(d.get("summary", ""))})
            ev.add("F", f"FERB ran a task ({d.get('status')}): {str(d.get('summary', ''))}", [("journal", j["id"])])
    ev.digest = {
        "day": day,
        "hours": {f"{h:02d}": [k for k, _ in c.most_common()] for h, c in sorted(hourly.items())},
        "apps_min": {k: round(v / 60) for k, v in top_apps},
        "sites": {h: len(rs) for h, rs in sorted(sites.items(), key=lambda kv: -len(kv[1]))},
        "folders": {f: dict(c) for f, c in sorted(folders.items(), key=lambda kv: -sum(kv[1].values()))},
        "windows": [t for t, _ in titles.most_common()],
        "duration_estimate": {"method": "focus-event intervals", "maximum_gap_seconds": 3600},
        "idle_min": round(idle_s / 60),
        "events": len(owner),
        "chats": chats,
        "ferb": {"events": dict(ferb_counts), "tasks": tasks},
    }
    return ev


def summarized(db: Database, day: str) -> bool:
    return db.conn.execute("SELECT 1 FROM journal WHERE kind = ? AND json_extract(data_json, '$.day') = ?",
                           (SUMMARY_KIND, day)).fetchone() is not None


def rebuild(db: Database, journal: Any, day: str) -> bool:
    """After a forget: the day's summary again from the rows that remain (its narrative, written by a model from the
    old rows, is deleted and not regenerated). False if no raw rows are left."""
    if db.conn.execute("SELECT 1 FROM trail WHERE day = ? LIMIT 1", (day,)).fetchone() is None:
        return False
    db.conn.execute("DELETE FROM journal WHERE kind IN (?, ?) AND json_extract(data_json, '$.day') = ?",
                    (SUMMARY_KIND, NARRATIVE_KIND, day))
    journal.append(SUMMARY_KIND, build(db, day).digest | {"rebuilt": True, "made_by": SUMMARY_MADE_BY})
    return True
