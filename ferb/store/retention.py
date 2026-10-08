"""Owns age-based retention for traces and Jev decisions (D14, D21). Must not delete what nothing else preserves: a
task's spans go only after its permanent episode exists; the task list, episodes, the journal (chat, day summaries,
verdicts) and memory are never aged out here. Raw activity (trail) has its own purge (ferb/sense/trail.py)."""

from contextlib import contextmanager
from typing import Any, Iterator

from ferb.store.spans import tree_where


@contextmanager
def _transaction(db: Any) -> Iterator[None]:
    """All or nothing: a failure leaves no half-deleted trace."""
    db.conn.execute("BEGIN")
    try:
        yield
        db.conn.execute("COMMIT")
    except BaseException:
        db.conn.execute("ROLLBACK")
        raise


def _delete_with_descendants(db: Any, seed_where: str, params: tuple[Any, ...]) -> int:
    """One statement, so a parent never goes before its child (spans.parent_id is a foreign key)."""
    return int(db.conn.execute(
        f"WITH RECURSIVE sub(id) AS (SELECT id FROM spans WHERE {seed_where} "
        "UNION SELECT s.id FROM spans s JOIN sub ON s.parent_id = sub.id) "
        "DELETE FROM spans WHERE id IN (SELECT id FROM sub)", params).rowcount or 0)


def finished_roots_before(db: Any, cutoff: str) -> list[str]:
    """Root tasks that ended before the cutoff and still have spans, oldest first (never a running or planned
    one)."""
    return [r["id"] for r in db.conn.execute(
        "SELECT id FROM tasks t WHERE parent_id IS NULL AND status NOT IN ('running', 'planned') AND updated_at < ? "
        "AND EXISTS (SELECT 1 FROM spans WHERE task_id = t.id OR (task_id >= t.id || '.' AND task_id < t.id || '/')) "
        "ORDER BY updated_at, rowid",
        (cutoff,))]


def purge_task_spans(db: Any, root_id: str) -> int:
    """Delete a task tree's spans, and its intake planner's (they carry the request id, not a task id)."""
    row = db.conn.execute("SELECT request_id FROM tasks WHERE id = ?", (root_id,)).fetchone()
    where, params = tree_where(root_id)
    with _transaction(db):
        return _delete_with_descendants(
            db, f"{where} OR (task_id IS NULL AND json_extract(input_json, '$.request_id') = ?)",
            (*params, row["request_id"] if row else None))


def purge_loose_spans(db: Any, cutoff: str) -> int:
    """Old spans tied to no task: no task id, and no request id or one that never became a task (chat replies,
    distillation usage, runtime checks)."""
    with _transaction(db):
        return _delete_with_descendants(
            db, "task_id IS NULL AND parent_id IS NULL AND started_at < ? AND "
                "(json_extract(input_json, '$.request_id') IS NULL OR "
                " json_extract(input_json, '$.request_id') NOT IN (SELECT request_id FROM tasks))", (cutoff,))


def purge_decisions(db: Any, cutoff: str) -> int:
    return int(db.conn.execute("DELETE FROM decisions WHERE at < ?", (cutoff,)).rowcount or 0)
