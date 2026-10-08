"""Owns age-based retention for traces and Jev decisions (D14, D21). Must not delete what nothing else preserves: a
task's spans go only after its permanent episode exists; the task list, episodes, the journal (chat, day summaries,
verdicts) and memory are never aged out here. Raw activity (trail) has its own purge (ferb/sense/trail.py)."""

from typing import Any, Iterable, Iterator

from ferb.store.spans import tree_where

CHUNK = 500


def _chunks(ids: Iterable[str]) -> Iterator[list[str]]:
    items = list(ids)
    for i in range(0, len(items), CHUNK):
        yield items[i:i + CHUNK]


def _with_descendants(db: Any, ids: set[str]) -> set[str]:
    frontier = set(ids)
    while frontier:
        found: set[str] = set()
        for chunk in _chunks(frontier):
            found |= {r["id"] for r in db.conn.execute(
                f"SELECT id FROM spans WHERE parent_id IN ({','.join('?' * len(chunk))})", chunk)}
        frontier = found - ids
        ids |= frontier
    return ids


def _delete(db: Any, ids: set[str]) -> int:
    for chunk in _chunks(ids):
        db.conn.execute(f"DELETE FROM spans WHERE id IN ({','.join('?' * len(chunk))})", chunk)
    return len(ids)


def finished_roots_before(db: Any, cutoff: str) -> list[str]:
    """Root tasks that ended before the cutoff (never a running or planned one)."""
    return [r["id"] for r in db.conn.execute(
        "SELECT id FROM tasks WHERE parent_id IS NULL AND status NOT IN ('running', 'planned') AND updated_at < ?",
        (cutoff,))]


def purge_task_spans(db: Any, root_id: str) -> int:
    """Delete a task tree's spans, and its intake planner's (they carry the request id, not a task id)."""
    row = db.conn.execute("SELECT request_id FROM tasks WHERE id = ?", (root_id,)).fetchone()
    where, params = tree_where(root_id)
    ids = {r["id"] for r in db.conn.execute(
        f"SELECT id FROM spans WHERE {where} "
        "OR (task_id IS NULL AND json_extract(input_json, '$.request_id') = ?)",
        (*params, row["request_id"] if row else None))}
    return _delete(db, _with_descendants(db, ids))


def purge_loose_spans(db: Any, cutoff: str) -> int:
    """Old spans tied to no task: no task id, and no request id or one that never became a task (chat replies,
    distillation usage, runtime checks)."""
    ids = {r["id"] for r in db.conn.execute(
        "SELECT id FROM spans WHERE task_id IS NULL AND parent_id IS NULL AND started_at < ? AND "
        "(json_extract(input_json, '$.request_id') IS NULL OR "
        " json_extract(input_json, '$.request_id') NOT IN (SELECT request_id FROM tasks))", (cutoff,))}
    return _delete(db, _with_descendants(db, ids))


def purge_decisions(db: Any, cutoff: str) -> int:
    return int(db.conn.execute("DELETE FROM decisions WHERE at < ?", (cutoff,)).rowcount or 0)
