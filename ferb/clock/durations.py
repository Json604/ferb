"""Measured task durations (section 13, build step 14): how long finished worker runs of the same kind took, from the
spans. The kind is the subtask's route (from its scopes: browser / mac / shell / mixed) and role (reader / actor). A
worker's time header gets one line, once there are MIN_SAMPLES."""

import json
import statistics
from datetime import datetime
from typing import Any

from ferb.store.db import Database

MIN_SAMPLES = 5
WINDOW = 500                        # most recent finished worker runs considered


def route_of(scopes: set[str]) -> str:
    kinds = {("browser" if s.startswith(("browser:", "identity:")) else "shell" if s == "shell" else
              "mac" if s.startswith(("mac:", "applescript", "screen")) else "") for s in scopes} - {""}
    return kinds.pop() if len(kinds) == 1 else "mixed"


def samples(db: Database, route: str, role: str) -> list[float]:
    out = []
    for r in db.conn.execute("SELECT input_json, started_at, ended_at FROM spans WHERE kind = 'worker' AND ended_at IS "
                             "NOT NULL AND error IS NULL ORDER BY rowid DESC LIMIT ?", (WINDOW,)):
        try:
            inp: dict[str, Any] = json.loads(r["input_json"])
        except ValueError:
            continue
        if inp.get("role") == role and route_of(set(inp.get("scopes", []))) == route:
            out.append((datetime.fromisoformat(r["ended_at"]) - datetime.fromisoformat(r["started_at"])).total_seconds())
    return out


def typical(db: Database, scopes: set[str], role: str) -> str | None:
    route = route_of(scopes)
    xs = samples(db, route, role)
    if len(xs) < MIN_SAMPLES:
        return None
    xs.sort()
    p90 = xs[min(len(xs) - 1, int(round(0.9 * (len(xs) - 1))))]
    return (f"similar tasks ({route}, {role}) took median {_m(statistics.median(xs))}, p90 {_m(p90)} "
            f"(n={len(xs)})")


def _m(seconds: float) -> str:
    return f"{seconds:.0f}s" if seconds < 90 else f"{seconds / 60:.0f}m"
