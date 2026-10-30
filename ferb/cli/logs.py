"""Render persisted daemon log events for inspection. Must not change their recorded meaning.

`ferb logs [--tasks --trail --tools --browsers]`: live stream from the daemon's bus."""

import json
from datetime import datetime
from typing import Any, TextIO

from ferb.cli.client import Client
from ferb.trace import lines


def format_event(ev: dict[str, Any]) -> str:
    if ev["topic"] == "tool.finished":
        d = ev["data"]
        return lines.step_line(ev["at"], d.get("tool", "?"), d.get("args") or {}, d.get("why") or "",
                               d.get("summary") or "", d.get("seconds"), d.get("error"))
    at = datetime.fromisoformat(ev["at"]).strftime("%H:%M:%S")
    marker = "!" if ev["class"] == "critical" else ("*" if ev["class"] == "notice" else " ")
    data = json.dumps(ev["data"], separators=(",", ":"), ensure_ascii=False)
    return f"{at} {marker} {ev['topic']:<24} {data}"


def run(client: Client, filters: list[str], out: TextIO, limit: int | None = None) -> int:
    n = 0
    for ev in client.stream("logs.subscribe", {"filters": filters}):
        print(format_event(ev), file=out, flush=True)
        n += 1
        if limit is not None and n >= limit:
            break
    return 0
