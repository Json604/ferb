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
