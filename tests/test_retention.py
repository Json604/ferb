"""Traces and Jev decisions age out after 30 days (D21), evidence after 7 (V1, D20); a task's spans go only once its
permanent episode exists; the task list, episodes, journal and memory are kept."""
import asyncio
import json
from datetime import timedelta
from pathlib import Path
from typing import Any

from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
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
    assert _q(daemon, "SELECT count(*) FROM journal WHERE kind = 'retention.purged'") == 0   # nothing purged: no row


def _scenario(daemon: RunningDaemon) -> list[str]:
    """Two finished roots past the cut, one recent; an old and a new Jev decision; old loose spans; an old and a new
    piece of evidence."""
    roots = [_say_hi(daemon) for _ in range(3)]
    for root in roots[:2]:
        _age(daemon, root, 31)
    _do(daemon, "UPDATE spans SET started_at = ?, ended_at = ? WHERE task_id = ? OR task_id LIKE ?",
        daemon.daemon.clock.now().isoformat(), daemon.daemon.clock.now().isoformat(), roots[2], roots[2] + ".%")
    now = daemon.daemon.clock.now()
    for i, at in enumerate((now - timedelta(days=31), now)):
        _do(daemon, "INSERT INTO decisions (id, at, question, baseline, mode) VALUES (?, ?, 'q', 'b', 'shadow')",
            f"d{i}", at.isoformat())
    for i in range(3):
        _insert_span(daemon, f"z-p{i}", None, None, None, _old())
    for i, at in enumerate((now - timedelta(days=8), now)):
        async def put() -> None:
            eid = daemon.daemon.evidence.put(roots[0], None, "snapshot", data={"n": i}, origin="ferb")
            daemon.daemon.db.conn.execute("UPDATE evidence SET captured_at = ? WHERE id = ?", (at.isoformat(), eid))
        daemon.run(put())
    return roots


def _purged(daemon: RunningDaemon, roots: list[str], before: int, result: dict[str, int]) -> dict[str, Any]:
    """What a pass did, in terms that don't depend on how many spans each task happened to record."""
    after = _q(daemon, "SELECT count(*) FROM spans")
    row = json.loads(_q(daemon, "SELECT data_json FROM journal WHERE kind = 'retention.purged'"))
    return {"true_count": result["spans"] == before - after > 3, "decisions": result["decisions"],
            "evidence": result["evidence"], "evidence_left": _q(daemon, "SELECT count(*) FROM evidence"),
            "journal": row == {"trace_days": 30, "spans": result["spans"], "decision_days": 30,
                               "decisions": result["decisions"], "evidence_days": 7, "evidence": result["evidence"],
                               "recovery_days": 7, "recovery": result["recovery"], "roots_left": 0},
            "roots_emptied": [_tree_spans(daemon, r) == 0 for r in roots],
            "loose_left": _spans_with_prefix(daemon, "z-p"),
            "decisions_left": _q(daemon, "SELECT count(*) FROM decisions"),
            "episodes": [_q(daemon, "SELECT count(*) FROM episodes WHERE task_id = ?", r) for r in roots]}


def test_the_async_pass_purges_the_same_as_the_sync_one_and_yields_between_roots(home: Path) -> None:
    """I3 (D21): the hourly pass runs on the daemon loop one root at a time, yielding between roots; it purges and
    journals exactly what the synchronous pass does."""
    seen: dict[str, dict[str, Any]] = {}
    order: list[str] = []
    for mode in ("sync", "async"):
        d = RunningDaemon(home.parent / mode, clock=FakeClock(), ids=SequentialIds("id"), watch_owner_input=False)
        try:
            sup_ = d.daemon.supervisor
            assert sup_ is not None
            sup_.availability = sup_._default_availability
            roots = _scenario(d)
            before = _q(d, "SELECT count(*) FROM spans")
            if mode == "sync":
                result: dict[str, int] = {}
                d.call_soon(lambda: result.update(d.daemon._purge_records()))
            else:
                real = d.daemon._purge_root
                d.daemon._purge_root = lambda root, now: (order.append("root"), real(root, now))[1]  # type: ignore

                async def ticking() -> dict[str, int]:
                    async def tick() -> None:
                        while True:
                            order.append("tick")
                            await asyncio.sleep(0)
                    ticker = asyncio.ensure_future(tick())
                    try:
                        return await d.daemon._purge_pass()
                    finally:
                        ticker.cancel()
                result = d.run(ticking())
            seen[mode] = _purged(d, roots, before, result)
        finally:
            d.stop()
    assert seen["async"] == seen["sync"] == {
        "true_count": True, "decisions": 1, "evidence": 1, "evidence_left": 1, "journal": True,
        "roots_emptied": [True, True, False], "loose_left": 0, "decisions_left": 1, "episodes": [1, 1, 1]}
    first, second = [i for i, what in enumerate(order) if what == "root"]
    assert "tick" in order[first + 1:second]                     # the loop ran between the two roots


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
    before = _q(daemon, "SELECT count(*) FROM spans")
    assert before > 601
    from ferb.store import retention
    async def go():
        return retention.purge_task_spans(daemon.daemon.db, root)
    deleted = daemon.run(go())
    assert deleted == before - _q(daemon, "SELECT count(*) FROM spans") > 601                                           # the true count, not -1
    assert _tree_spans(daemon, root) == 0
    assert _spans_with_prefix(daemon, "z-") == 0 and _spans_with_prefix(daemon, "a-c") == 0


def test_many_old_loose_spans_with_children_are_purged(daemon: RunningDaemon) -> None:
    for i in range(300):
        _insert_span(daemon, f"z-p{i:03d}", None, None, None, _old())
        _insert_span(daemon, f"a-k{i:03d}", f"z-p{i:03d}", None, None, _old())
    _purge(daemon)
    assert _spans_with_prefix(daemon, "z-p") == 0 and _spans_with_prefix(daemon, "a-k") == 0
    assert _q(daemon, "SELECT json_extract(data_json, '$.spans') FROM journal WHERE kind = 'retention.purged'") == 600


def test_purge_functions_return_true_counts(daemon: RunningDaemon) -> None:
    from ferb.store import retention
    _insert_span(daemon, "c-p", None, None, None, _old())
    _insert_span(daemon, "c-k", "c-p", None, None, _old())
    async def go():
        return retention.purge_loose_spans(daemon.daemon.db, "2021-01-01T00:00:00+00:00")
    assert daemon.run(go()) == 2


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


def test_a_pass_cancelled_between_roots_journals_what_it_purged(daemon: RunningDaemon) -> None:
    """Residual 3 (D21): shutdown can cancel the hourly pass between roots; what it already purged is journaled."""
    first, second = _say_hi(daemon), _say_hi(daemon)
    for root in (first, second):
        _age(daemon, root, 31)
    _do(daemon, "UPDATE tasks SET updated_at = ? WHERE tree_id = ?", "2020-01-01T00:00:00+00:00", first)

    async def cancel_after_the_first_root() -> None:
        pass_ = asyncio.ensure_future(daemon.daemon._purge_pass())
        real = daemon.daemon._purge_root

        def purge_one(root: str, now: Any) -> int:
            pass_.cancel()                               # shutdown arrives while this root is purged
            return real(root, now)
        daemon.daemon._purge_root = purge_one  # type: ignore[method-assign]
        try:
            await pass_
        except asyncio.CancelledError:
            pass
        finally:
            del daemon.daemon._purge_root
    daemon.run(cancel_after_the_first_root())
    assert _tree_spans(daemon, first) == 0 and _tree_spans(daemon, second) > 0
    row = json.loads(_q(daemon, "SELECT data_json FROM journal WHERE kind = 'retention.purged'"))
    assert row["spans"] > 0 and row["decisions"] == 0 and row["roots_left"] == 1 and row["stopped"] is True


def test_a_pass_cancelled_after_its_last_root_still_journals_it(daemon: RunningDaemon) -> None:
    root = _say_hi(daemon)
    _age(daemon, root, 31)

    async def cancel_during_the_last_root() -> None:
        pass_ = asyncio.ensure_future(daemon.daemon._purge_pass())
        real = daemon.daemon._purge_root
        daemon.daemon._purge_root = lambda r, now: (pass_.cancel(), real(r, now))[1]  # type: ignore[method-assign]
        try:
            await pass_
        except asyncio.CancelledError:
            pass
        finally:
            del daemon.daemon._purge_root
    daemon.run(cancel_during_the_last_root())
    row = json.loads(_q(daemon, "SELECT data_json FROM journal WHERE kind = 'retention.purged'"))
    assert _tree_spans(daemon, root) == 0 and row["spans"] > 0 and row["roots_left"] == 0 and row["stopped"] is True


def test_a_pass_cancelled_before_anything_was_purged_journals_nothing(daemon: RunningDaemon) -> None:
    root = _say_hi(daemon)
    _age(daemon, root, 31)

    async def cancel_at_once() -> None:
        pass_ = asyncio.ensure_future(daemon.daemon._purge_pass())
        daemon.daemon._purge_root = lambda root, now: (pass_.cancel(), 0)[1]  # type: ignore[method-assign]
        try:
            await pass_
        except asyncio.CancelledError:
            pass
        finally:
            del daemon.daemon._purge_root
    daemon.run(cancel_at_once())
    assert _q(daemon, "SELECT count(*) FROM journal WHERE kind = 'retention.purged'") == 0


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
    from ferb.daemon.areas import retention as retention_area
    monkeypatch.setattr(retention_area, "ROOTS_PER_PURGE", 1)
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


def test_every_retention_days_setting_is_at_least_one_day(daemon: RunningDaemon) -> None:
    """A retention of 0 (or less) would purge evidence, recovery copies or traces the moment they exist,
    before the checker, the owner or `ferb restore` could use them: every retention.*_days F4 reads is at least 1."""
    for key in ("trace_days", "decision_days", "evidence_days", "recovery_days"):
        daemon.daemon.config.set("retention", key, 0 if key != "decision_days" else -3)

    async def read() -> Any:
        plan = daemon.daemon._purge_plan()
        listed = await daemon.daemon._recovery_list(None, {})            # type: ignore[arg-type]
        return plan, listed["days"]
    plan, restore_days = daemon.run(read())
    assert (plan.trace_days, plan.decision_days, plan.evidence_days, plan.recovery_days) == (1, 1, 1, 1)
    assert restore_days == 1


def test_the_retention_loop_goes_on_when_even_its_failure_cant_be_journaled(tmp_path: Path, capsys: Any) -> None:
    """D14: a failed pass is journaled `retention.failed`; when that write fails too (a broken database), the error
    goes to stderr through the daemon's redactor and the hourly loop goes on, as the resource loop's does (L30)."""
    import contextlib
    import sqlite3
    from types import SimpleNamespace
    from ferb.daemon.main import Daemon
    from tests.support.virtual_time import run_virtual
    d = Daemon(tmp_path / ".ferb", watchers=False, distill=False, retention=False, resources=False,
               notifications=False, snapshots=False, watch_owner_input=False, preload_secrets=False)
    passes: list[int] = []

    async def failing() -> dict[str, int]:
        passes.append(1)
        raise RuntimeError("purge failed for token hunter2")

    def append(kind: str, data: Any = None, origin: Any = None) -> Any:
        raise sqlite3.OperationalError("disk I/O error")
    d._purge_pass = failing                                              # type: ignore[method-assign]
    d._journal = lambda: SimpleNamespace(append=append)                  # type: ignore[method-assign]
    d.redact = SimpleNamespace(text=lambda text: text.replace("hunter2", "[REDACTED]"))  # type: ignore[assignment]

    async def three_passes() -> None:
        loop = asyncio.create_task(d._retention_loop())
        await asyncio.sleep(60 + 2 * 3600 + 1)                           # a minute after start, then hourly
        assert not loop.done()
        loop.cancel()
        with contextlib.suppress(asyncio.CancelledError):
            await loop
    run_virtual(three_passes())
    err = capsys.readouterr().err
    assert len(passes) == 3
    assert err.count("FERB: a retention pass failed (RuntimeError: purge failed for token [REDACTED]) and couldn't "
                     "be journaled (OperationalError)") == 3, err
    assert "hunter2" not in err
