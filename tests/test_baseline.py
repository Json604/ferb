import io
import json
from pathlib import Path

from ferb.store.db import Database


def _seed(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    now = "2099-01-01T00:00:00+00:00"
    later = "2099-01-01T00:02:00+00:00"
    for tid, st, outcome in (("t1", "done", {"verified": 1}), ("t2", "failed", {"verified": 0})):
        db.conn.execute("INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, "
                        "outcome_json, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                        (tid, tid, "r", "g", "owner", "mixed", "[]", st, json.dumps(outcome), now, later))
    db.conn.execute("INSERT INTO spans (id, kind, task_id, started_at, ended_at, input_json, output_json) "
                    "VALUES ('u1','model.usage','t1',?,?,'{}',?)",
                    (now, now, json.dumps({"input_tokens": 1000, "cached_input_tokens": 900, "output_tokens": 10})))
    for i, (tid, verdict) in enumerate((("t1", "fail"), ("t1", "pass"), ("t2", "fail"))):
        db.conn.execute("INSERT INTO owner_verdicts (id, task_id, verdict, at) VALUES (?,?,?,?)",
                        (f"v{i}", tid, verdict, now))
    for i, err in enumerate((None, "boom")):
        db.conn.execute("INSERT INTO spans (id, kind, task_id, started_at, ended_at, input_json, error) "
                        "VALUES (?,?,?,?,?,?,?)", (f"c{i}", "tool.call", "t1", now, now, "{}", err))
    db.conn.execute("INSERT INTO spans (id, kind, task_id, started_at, ended_at, input_json) "
                    "VALUES ('m1','model.turn','t1','2099-01-01T00:00:00+00:00','2099-01-01T00:00:02+00:00','{}')")
    for i, kind in enumerate(("approval.resolved", "chat.clarify", "chat.reply", "approval.requested")):
        db.conn.execute("INSERT INTO journal (id, at, kind, data_json) VALUES (?,?,?,?)",
                        (f"j{i}", now, kind, "{}"))
    db.conn.commit()


def test_baseline_writes_snapshot_with_metrics(tmp_path: Path) -> None:
    _seed(tmp_path)
    from ferb.cli.admin import baseline
    out = io.StringIO()
    assert baseline(tmp_path, out) == 0
    files = list((tmp_path / "baselines").glob("*.json"))
    assert len(files) == 1
    assert str(files[0]) in out.getvalue()
    data = json.loads(files[0].read_text())
    assert data["requests"] == 2 and data["done"] == 1 and data["done_pct"] == 50
    assert data["verified"] == 1 and data["verified_pct"] == 50
    assert data["owner_verdicts"] == 2 and data["owner_pass"] == 1
    assert data["cache_pct"] == 90
    assert data["avg_task_s"] == 120.0                       # two root tasks, 2 minutes each
    assert data["owner_questions"] == 2                      # one approval.resolved + one chat.clarify
    assert data["tool_calls"] == 2 and data["tool_errors"] == 1
    assert data["model_runs"]["model.turn"] == {"n": 1, "avg_s": 2.0}
    assert data["avg_trace_bytes"] > 0


def test_baseline_on_empty_db_does_not_divide_by_zero(tmp_path: Path) -> None:
    Database(tmp_path / "ferb.db").migrate()
    from ferb.cli.admin import baseline
    assert baseline(tmp_path, io.StringIO()) == 0
    data = json.loads(next((tmp_path / "baselines").glob("*.json")).read_text())
    assert data["requests"] == 0 and data["done_pct"] == 0 and data["cache_pct"] == 0
    assert data["verified_pct"] == 0 and data["avg_trace_bytes"] == 0


def test_baseline_without_database_fails(tmp_path: Path) -> None:
    from ferb.cli.admin import baseline
    assert baseline(tmp_path, io.StringIO()) == 1


def test_baseline_avg_task_s_ignores_running_tasks_and_children(tmp_path: Path) -> None:
    _seed(tmp_path)
    db = Database(tmp_path / "ferb.db")
    now, later = "2099-01-01T00:00:00+00:00", "2099-01-02T00:00:00+00:00"
    for tid, parent, st in (("t3", None, "running"), ("t4", "t1", "done")):
        db.conn.execute("INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, "
                        "status, outcome_json, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                        (tid, tid, parent, "r", "g", "owner", "mixed", "[]", st, "{}", now, later))
    db.conn.commit()
    from ferb.cli.admin import baseline
    baseline(tmp_path, io.StringIO())
    assert json.loads(next((tmp_path / "baselines").glob("*.json")).read_text())["avg_task_s"] == 120.0


def test_baseline_on_pre_f0_database_prints_clean_message(tmp_path: Path) -> None:
    import sqlite3
    Database(tmp_path / "ferb.db").migrate()
    c = sqlite3.connect(tmp_path / "ferb.db")
    c.execute("DROP TABLE owner_verdicts")
    c.commit()
    c.close()
    from ferb.cli.admin import baseline
    out = io.StringIO()
    assert baseline(tmp_path, out) == 1
    assert "older than FERB 0.27 F0" in out.getvalue() and not (tmp_path / "baselines").exists()


def test_baseline_records_message_to_ack_plan_and_answer(tmp_path: Path) -> None:
    """L8: from a message's arrival to its acknowledgement, its plan line and its first answer, plus the context
    building before the planner starts. A run with no chat message (a scheduled run) isn't a message's."""
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    t = "2099-01-01T00:00:{:06.3f}+00:00".format

    def row(kind: str, at: float, **data: object) -> None:
        db.conn.execute("INSERT INTO journal (id, at, kind, data_json) VALUES (?,?,?,?)",
                        (f"j{kind}{at}", t(at), kind, json.dumps(data)))
    row("chat.ack", 0.05, request_id="r1", text="On it.", received_at=t(0))
    row("chat.message", 0.1, request_id="r1", text="Write the note")
    row("chat.plan", 12, request_id="r1", task_id="t1")
    row("task.answer", 50, request_id="r1", task_id="t1", text="Wrote it.")
    row("chat.ack", 1.02, request_id="r2", text="Looking.", received_at=t(1))
    row("chat.message", 1.5, request_id="r2", text="What time is it?")
    row("chat.reply", 10, request_id="r2", text="Nine.")
    row("chat.plan", 20, request_id="sched", task_id="t9")          # a scheduled run: no chat message
    row("task.answer", 30, request_id="sched", task_id="t9", text="Ran.")
    for sid, request_id, started in (("m1", "r1", 1.1), ("m2", "r1", 8.0), ("m3", "r2", 2.5)):
        db.conn.execute("INSERT INTO spans (id, kind, started_at, ended_at, input_json) VALUES (?,?,?,?,?)",
                        (sid, "model.turn", t(started), t(started + 1),
                         json.dumps({"role": "supervisor", "request_id": request_id, "repair": sid == "m2"})))
    db.conn.commit()
    from ferb.cli.admin import baseline
    assert baseline(tmp_path, io.StringIO()) == 0
    latency = json.loads(next((tmp_path / "baselines").glob("*.json")).read_text())["latency_s"]
    assert latency["ack"] == {"n": 2, "avg_s": 0.035, "median_s": 0.035}
    assert latency["plan"] == {"n": 1, "avg_s": 12.0, "median_s": 12.0}
    assert latency["answer"] == {"n": 2, "avg_s": 29.5, "median_s": 29.5}       # 50 s and 9 s
    assert latency["context"] == {"n": 2, "avg_s": 1.0, "median_s": 1.0}       # chat.message to the first planner turn
    from ferb.eval.comparison import compare_baselines
    assert compare_baselines({}, {"latency_s": latency})["latency_s"] == {"before": None, "after": latency}
