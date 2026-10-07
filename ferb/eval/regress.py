"""Run a data-defined regression set against the daemon and record one result row per task.
Must not infer a verdict: the owner's `ferb verdict` is acceptance; this only records what happened."""

import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, TextIO


DEFAULT_SET = Path(__file__).parent / "sets" / "smoke.toml"   # shipped inside the package


@dataclass(frozen=True)
class RegressTask:
    id: str
    prompt: str
    surface: str  # "disposable" | "owner"
    expect: str   # free text shown to the owner when asking for a verdict


def load_set(path: Path) -> list[RegressTask]:
    data = tomllib.loads(path.read_text())
    return [RegressTask(t["id"], t["prompt"], t.get("surface", "disposable"), t.get("expect", ""))
            for t in data.get("task", [])]


def run_set(client: Any, tasks: list[RegressTask], out: TextIO | None,
            record: Callable[[list[dict[str, Any]]], None] | None = None,
            connect: Callable[[], Any] | None = None) -> list[dict[str, Any]]:
    """Submit each task in one fresh conversation. An owner question is declined (and recorded in `asked`)
    so it never blocks the run. `connect` opens a short-lived connection for answers, because the streaming
    connection must not be read while its stream is open; without it `client` is used."""
    rows: list[dict[str, Any]] = []
    session_id = client.call("chat.attach", {"new": True})["session_id"]
    broken = False
    for t in tasks:
        if broken:
            if connect is None:
                rows.append({"id": t.id, "task_id": None, "status": "errored", "asked": [],
                             "outcome_summary": "not run: the stream broke on an earlier task and cannot be reused"})
                continue
            try:
                client.close()
            except Exception:
                pass
            client, broken = connect(), False        # a fresh connection: never read a broken stream's leftovers
        row: dict[str, Any] = {"id": t.id, "task_id": None, "status": "errored", "outcome_summary": "", "asked": []}
        try:
            for ev in client.stream("chat.send", {"session_id": session_id, "text": t.prompt}):
                kind = ev.get("type")
                if ev.get("task_id") and row["task_id"] is None:
                    row["task_id"] = ev["task_id"]
                if kind == "event" and ev.get("topic") == "approval.requested":
                    data = ev.get("data") or {}
                    row["asked"].append(data.get("reason") or str(data))
                    try:
                        _decline(client, connect, data.get("approval_id"))
                    except Exception as e:           # keep draining this stream to its final event
                        row["asked"].append(f"decline failed: {type(e).__name__}: {e}")
                elif kind == "outcome":
                    row["status"] = ev.get("status", "unknown")
                    row["outcome_summary"] = ev.get("summary", "")
                elif kind in ("reply", "clarify"):
                    row["status"], row["outcome_summary"] = kind, ev.get("text", "")
                elif kind == "error":
                    row["status"], row["outcome_summary"] = "errored", ev.get("text", "")
        except Exception as e:
            row["status"], row["outcome_summary"] = "errored", f"{type(e).__name__}: {e}"
            broken = True
        rows.append(row)
        if out is not None:
            print(f"  {t.id}: {row['status']} ({row['task_id']})", file=out, flush=True)
    if record is not None:
        record(rows)
    return rows


def _decline(client: Any, connect: Callable[[], Any] | None, approval_id: str | None) -> None:
    c = connect() if connect else client
    try:
        c.call("chat.answer", {"approval_id": approval_id, "text": ""})
    finally:
        if connect:
            c.close()
