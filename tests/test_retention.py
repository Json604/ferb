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


def _old() -> str:
    return "2020-01-01T00:00:00+00:00"


def _insert_span(daemon: RunningDaemon, sid: str, parent: str | None, task_id: str | None, request_id: str | None,
                 at: str) -> None:
    payload = '{"request_id": "%s"}' % request_id if request_id else "{}"
    _do(daemon, "INSERT INTO spans (id, parent_id, task_id, kind, started_at, input_json) VALUES (?,?,?,?,?,?)",
        sid, parent, task_id, "tool.call", at, payload)


def _spans_with_prefix(daemon: RunningDaemon, prefix: str) -> int:
    return _q(daemon, "SELECT count(*) FROM spans WHERE id LIKE ?", prefix + "%")


def test_a_large_nested_tree_is_purged_completely(daemon: RunningDaemon) -> None:
    root = _say_hi(daemon)
    _age(daemon, root, 31)
    _insert_span(daemon, "z-w", None, root + ".1", None, _old())
    for i in range(300):          # ids sort children before parents in one chunk and after in another
        _insert_span(daemon, f"z-t{i:03d}", "z-w", root + ".1", None, _old())
        _insert_span(daemon, f"a-c{i:03d}", f"z-t{i:03d}", root + ".1", None, _old())
    _purge(daemon)
    assert _tree_spans(daemon, root) == 0
    assert _spans_with_prefix(daemon, "z-") == 0 and _spans_with_prefix(daemon, "a-c") == 0


def test_many_old_loose_spans_with_children_are_purged(daemon: RunningDaemon) -> None:
    for i in range(300):
        _insert_span(daemon, f"z-p{i:03d}", None, None, None, _old())
        _insert_span(daemon, f"a-k{i:03d}", f"z-p{i:03d}", None, None, _old())
    _purge(daemon)
    assert _spans_with_prefix(daemon, "z-p") == 0 and _spans_with_prefix(daemon, "a-k") == 0


def test_loose_span_rules(daemon: RunningDaemon) -> None:
    root = _say_hi(daemon)
    request_id = _q(daemon, "SELECT request_id FROM tasks WHERE id = ?", root)
    recent = daemon.daemon.clock.now().isoformat()
    _insert_span(daemon, "l-recent", None, None, None, recent)
    _insert_span(daemon, "l-task", None, None, request_id, _old())
    _insert_span(daemon, "l-nomatch", None, None, "no-such-request", _old())
    _insert_span(daemon, "l-norequest", None, None, None, _old())
    _purge(daemon)
    assert _spans_with_prefix(daemon, "l-recent") == 1
    assert _spans_with_prefix(daemon, "l-task") == 1
    assert _spans_with_prefix(daemon, "l-nomatch") == 0
    assert _spans_with_prefix(daemon, "l-norequest") == 0


def test_task_origins_outlive_the_spans(daemon: RunningDaemon) -> None:
    root = _say_hi(daemon)
    _do(daemon, "UPDATE episodes SET origins = 'external:web:x.com' WHERE task_id = ?", root)
    _age(daemon, root, 31)
    _purge(daemon)
    assert _tree_spans(daemon, root) == 0
    async def go():
        return daemon.daemon.spans.task_origins(root)
    assert "external:web:x.com" in daemon.run(go())


def test_a_root_interrupted_by_a_restart_is_aged_from_then(daemon: RunningDaemon) -> None:
    root = _say_hi(daemon)
    _age(daemon, root, 31)
    _do(daemon, "UPDATE tasks SET status = 'running' WHERE id = ?", root)
    _do(daemon, "DELETE FROM episodes WHERE task_id = ?", root)
    async def go():
        return daemon.daemon.tasks.interrupt_running()
    assert root in daemon.run(go())
    _purge(daemon)
    assert _tree_spans(daemon, root) > 0


def test_one_bad_root_does_not_stop_the_others(daemon: RunningDaemon) -> None:
    bad = _say_hi(daemon)
    good = _say_hi(daemon)
    _age(daemon, bad, 31)
    _age(daemon, good, 31)
    from ferb.orchestrator import episodes
    real = episodes.get
    def flaky(db: Any, task_id: str) -> Any:
        if task_id == bad:
            raise RuntimeError("unreadable")
        return real(db, task_id)
    episodes.get = flaky  # type: ignore
    try:
        _purge(daemon)
    finally:
        episodes.get = real  # type: ignore
    assert _tree_spans(daemon, bad) > 0
    assert _tree_spans(daemon, good) == 0
    assert _q(daemon, "SELECT count(*) FROM journal WHERE kind = 'retention.failed'") == 1
    assert _q(daemon, "SELECT count(*) FROM journal WHERE kind = 'retention.purged'") == 1


def test_a_pass_handles_at_most_200_roots_oldest_first(daemon: RunningDaemon, monkeypatch: Any) -> None:
    from ferb.daemon import main
    monkeypatch.setattr(main, "ROOTS_PER_PURGE", 1)
    first = _say_hi(daemon)
    second = _say_hi(daemon)
    _age(daemon, first, 40)
    _age(daemon, second, 31)
    _do(daemon, "UPDATE tasks SET updated_at = ? WHERE tree_id = ?", "2020-01-01T00:00:00+00:00", first)
    _purge(daemon)
    assert _tree_spans(daemon, first) == 0 and _tree_spans(daemon, second) > 0
    assert _q(daemon, "SELECT json_extract(data_json, '$.roots_left') FROM journal "
                      "WHERE kind = 'retention.purged'") == 1
    _purge(daemon)
    assert _tree_spans(daemon, second) == 0
