from pathlib import Path

import pytest

from ferb.cli.client import Client, RpcError
from ferb.store.db import Database
from tests.conftest import RunningDaemon

NOW = "2099-01-01T00:00:00+00:00"
TASK_INSERT = ("INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, created_at, "
               "updated_at) VALUES (?,?,'r','g','owner','mixed','[]','done',?,?)")


def test_owner_verdicts_table_stores_pass_fail(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    db.conn.execute(TASK_INSERT, ("task-1", "task-1", NOW, NOW))
    db.conn.execute("INSERT INTO owner_verdicts (id, task_id, verdict, note, at) VALUES ('v1','task-1','pass','clean',?)",
                    ("2099-01-01T00:01:00+00:00",))
    db.conn.commit()
    row = db.conn.execute("SELECT verdict, note FROM owner_verdicts WHERE task_id='task-1'").fetchone()
    assert row["verdict"] == "pass" and row["note"] == "clean"
    with pytest.raises(Exception):
        db.conn.execute("INSERT INTO owner_verdicts (id, task_id, verdict, at) VALUES ('v2','task-1','maybe','x')")


def test_verdict_rpc_stores_and_rejects_unknown_task(daemon: RunningDaemon) -> None:
    daemon.run(_add_task(daemon, "abc123"))
    assert daemon.run(_outcome(daemon, "abc123"))["owner_verdict"] is None
    c = Client(daemon.sock)
    try:
        out = c.call("verdict.set", {"task_id": "abc123", "verdict": "pass", "note": "clean"})
        assert out == {"task_id": "abc123", "verdict": "pass"}
        with pytest.raises(RpcError):
            c.call("verdict.set", {"task_id": "nope", "verdict": "pass"})
        with pytest.raises(RpcError):
            c.call("verdict.set", {"task_id": "abc123", "verdict": "maybe"})
        c.call("verdict.set", {"task_id": "abc123", "verdict": "fail", "note": "key sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789"})
    finally:
        c.close()
    got = daemon.run(_outcome(daemon, "abc123"))["owner_verdict"]
    assert got["verdict"] == "fail" and got["at"]
    assert "abcdefghijklmnopqrstuvwxyz0123456789" not in got["note"]


async def _outcome(d: RunningDaemon, task_id: str) -> dict:
    return d.daemon.spans.outcome(task_id)


async def _add_task(d: RunningDaemon, task_id: str) -> None:
    d.daemon.db.conn.execute(TASK_INSERT, (task_id, task_id, NOW, NOW))
    d.daemon.db.conn.commit()


async def _add_child(d: RunningDaemon, child_id: str, root_id: str) -> None:
    d.daemon.db.conn.execute(TASK_INSERT, (child_id, root_id, NOW, NOW))
    d.daemon.db.conn.execute("UPDATE tasks SET parent_id = ? WHERE id = ?", (root_id, child_id))


def test_verdict_rejects_child_task_and_names_root(daemon: RunningDaemon) -> None:
    daemon.run(_add_task(daemon, "rootaaaa"))
    daemon.run(_add_child(daemon, "rootaaaa.1", "rootaaaa"))
    c = Client(daemon.sock)
    try:
        with pytest.raises(RpcError, match="rootaaaa"):
            c.call("verdict.set", {"task_id": "rootaaaa.1", "verdict": "pass"})
    finally:
        c.close()
    assert daemon.run(_outcome(daemon, "rootaaaa"))["owner_verdict"] is None


def test_verdict_resolves_prefix_and_rejects_ambiguous_and_last(daemon: RunningDaemon) -> None:
    daemon.run(_add_task(daemon, "abcdef111111"))
    daemon.run(_add_task(daemon, "abcdef222222"))
    daemon.run(_add_task(daemon, "uniq99887766"))
    c = Client(daemon.sock)
    try:
        assert c.call("verdict.set", {"task_id": "uniq99", "verdict": "pass"})["task_id"] == "uniq99887766"
        with pytest.raises(RpcError, match="ambiguous"):
            c.call("verdict.set", {"task_id": "abcdef", "verdict": "pass"})
        with pytest.raises(RpcError):
            c.call("verdict.set", {"task_id": "last", "verdict": "pass"})
    finally:
        c.close()
    assert daemon.run(_outcome(daemon, "uniq99887766"))["owner_verdict"]["verdict"] == "pass"
