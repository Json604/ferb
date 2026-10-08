"""Owns the permanent per-task record (L1, D21): composed by code from the task's trace when it ends, kept forever,
rebuildable from spans while they exist (MP1). Must not call a model or rewrite what the records say."""

import json
from typing import Any

from ferb.core import origin
from ferb.trace import compose

MADE_BY = "code:episode/1"
JSON_FIELDS = ("conditions", "steps", "problems", "tools", "usage", "desktop")


def build(spans: Any, task_id: str, now: str) -> dict[str, Any]:
    trace = {"task_id": task_id, "spans": spans.tree_json(task_id), "outcome": spans.outcome(task_id)}
    return compose.compose(trace) | {"made_by": MADE_BY, "built_at": now}


def write(db: Any, ep: dict[str, Any]) -> None:
    db.conn.execute(
        "INSERT OR REPLACE INTO episodes (task_id, request_id, request_text, goal, status, verified, started_at, "
        "ended_at, seconds, answer, conditions_json, steps_json, problems_json, tools_json, origins, external_read, "
        "usage_json, desktop_json, made_by, built_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        (ep["task_id"], ep["request_id"], ep["request_text"], ep["goal"], ep["status"],
         None if ep["verified"] is None else int(bool(ep["verified"])), ep["started_at"], ep["ended_at"],
         ep["seconds"], ep["answer"], *(json.dumps(ep[k], ensure_ascii=False) for k in JSON_FIELDS[:4]),
         origin.join(ep["origins"]), int(any(origin.is_external(o) for o in ep["origins"])),
         json.dumps(ep["usage"]), json.dumps(ep["desktop"], ensure_ascii=False), ep["made_by"], ep["built_at"]))


def get(db: Any, task_id: str) -> dict[str, Any] | None:
    row = db.conn.execute("SELECT * FROM episodes WHERE task_id = ?", (task_id,)).fetchone()
    if row is None:
        return None
    ep = dict(row)
    for key in JSON_FIELDS:
        ep[key] = json.loads(ep.pop(key + "_json"))
    ep["origins"] = sorted(origin.split(ep["origins"]))
    ep["external_read"] = bool(ep["external_read"])
    ep["verified"] = None if ep["verified"] is None else bool(ep["verified"])
    return ep
