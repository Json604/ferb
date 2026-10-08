"""Traces and Jev decisions age out after 30 days (D21); a task's spans go only once its permanent episode exists;
the task list, episodes, journal and memory are kept."""
from datetime import timedelta
from typing import Any

from tests.conftest import RunningDaemon, chat, done, install, sup, task


def _q(daemon: RunningDaemon, sql: str, *params: Any) -> Any:
    async def go():
        return daemon.daemon.db.conn.execute(sql, params).fetchone()[0]
    return daemon.run(go())


def _do(daemon: RunningDaemon, sql: str, *params: Any) -> None:
    async def go():
        daemon.daemon.db.conn.execute(sql, params)
    daemon.run(go())


def _say_hi(daemon: RunningDaemon) -> str:
    install(daemon, [sup("task", "Doing it.", task("say hi"))], {"worker": done("said hi")})
    chat(daemon, "say hi")
    return _q(daemon, "SELECT id FROM tasks WHERE parent_id IS NULL ORDER BY created_at DESC, rowid DESC")


def _age(daemon: RunningDaemon, root: str, days: int) -> None:
    old = (daemon.daemon.clock.now() - timedelta(days=days)).isoformat()
    _do(daemon, "UPDATE tasks SET created_at = ?, updated_at = ? WHERE tree_id = ?", old, old, root)
    _do(daemon, "UPDATE spans SET started_at = ?, ended_at = ?", old, old)


def _purge(daemon: RunningDaemon) -> None:
    daemon.call_soon(daemon.daemon._purge_records)


def _tree_spans(daemon: RunningDaemon, root: str) -> int:
    return _q(daemon, "SELECT count(*) FROM spans WHERE task_id = ? OR substr(task_id, 1, ?) = ?",
              root, len(root) + 1, root + ".")


def test_old_task_spans_are_purged_and_the_record_kept(daemon: RunningDaemon) -> None:
    root = _say_hi(daemon)
    _age(daemon, root, 31)
    _purge(daemon)
    assert _tree_spans(daemon, root) == 0
    assert _q(daemon, "SELECT count(*) FROM spans WHERE json_extract(input_json, '$.request_id') = "
                      "(SELECT request_id FROM tasks WHERE id = ?)", root) == 0  # the intake planner's spans too
    assert _q(daemon, "SELECT count(*) FROM episodes WHERE task_id = ?", root) == 1
    assert _q(daemon, "SELECT count(*) FROM tasks WHERE id = ?", root) == 1      # the task list stays
    assert _q(daemon, "SELECT count(*) FROM journal WHERE kind = 'chat.message'") == 1
    assert _q(daemon, "SELECT count(*) FROM journal WHERE kind = 'retention.purged'") == 1


def test_recent_tasks_keep_their_spans(daemon: RunningDaemon) -> None:
    root = _say_hi(daemon)
    _age(daemon, root, 29)
    _purge(daemon)
    assert _tree_spans(daemon, root) > 0


def test_spans_without_an_episode_are_kept_until_one_is_built(daemon: RunningDaemon) -> None:
    root = _say_hi(daemon)
    _do(daemon, "DELETE FROM episodes WHERE task_id = ?", root)
    _age(daemon, root, 31)
    daemon.daemon.spans.outcome = lambda task_id: (_ for _ in ()).throw(RuntimeError("broken"))  # type: ignore
    _purge(daemon)
    assert _tree_spans(daemon, root) > 0                                       # no episode: nothing deleted
    assert _q(daemon, "SELECT count(*) FROM journal WHERE kind = 'episode.failed'") >= 1
    del daemon.daemon.spans.outcome                                            # composing works again
    _purge(daemon)
    assert _q(daemon, "SELECT count(*) FROM episodes WHERE task_id = ?", root) == 1
    assert _tree_spans(daemon, root) == 0


def test_running_tasks_are_never_purged(daemon: RunningDaemon) -> None:
    root = _say_hi(daemon)
    _age(daemon, root, 31)
    _do(daemon, "UPDATE tasks SET status = 'running' WHERE id = ?", root)
    _purge(daemon)
    assert _tree_spans(daemon, root) > 0


def test_jev_decisions_age_out(daemon: RunningDaemon) -> None:
    now = daemon.daemon.clock.now()
    for i, at in enumerate((now - timedelta(days=31), now)):
        _do(daemon, "INSERT INTO decisions (id, at, question, baseline, mode) VALUES (?, ?, 'q', 'b', 'shadow')",
            f"d{i}", at.isoformat())
    _purge(daemon)
    assert _q(daemon, "SELECT count(*) FROM decisions") == 1
