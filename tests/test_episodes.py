"""One permanent record per task, composed by code from its trace (L1, D21, MP1)."""
import json
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.client import Client
from ferb.orchestrator import episodes
from ferb.store.db import Database
from ferb.trace import compose
from tests.conftest import STALL, RunningDaemon, chat, done, install, sup, task
from tests.support.trace_builder import T0, TraceBuilder, hf_cache_retry


def test_compose_turns_the_trace_into_steps_with_claims_and_marks() -> None:
    c = compose.compose(hf_cache_retry())
    assert [s["role"] for s in c["steps"]] == ["Planner", "Worker", "Checker", "Planner", "Worker", "Checker"]
    assert c["steps"][1]["text"].startswith("2 tool calls · worker reported done:")
    assert c["steps"][2]["mark"] == "⚠" and "checked nothing — 2 conditions unknown" in c["steps"][2]["text"]
    assert c["steps"][3]["text"] == "retry — verify only, delete nothing more"
    assert c["steps"][4]["text"].endswith(" — verified in step 6") and c["steps"][5]["mark"] == "✓"
    assert c["problems"] == ["⚠ 1 retry (1m17s spent after it)", "⚠ 1 check left conditions unknown"]
    assert c["status"] == "done" and c["verified"] is True and c["answer"] == "Deleted ~/.cache/huggingface."
    assert c["request_text"] == "delete the hf cache completely" and c["seconds"] == 143
    assert [x["met"] for x in c["conditions"]] == [True, True]
    assert c["origins"] == ["private"] and c["tools"] == {"shell.run": {"calls": 2, "errors": 0}}


def test_usage_is_summed_exactly() -> None:
    u = compose.compose(hf_cache_retry())["usage"]
    assert (u["input_tokens"], u["cached_input_tokens"], u["output_tokens"]) == (700000, 580000, 7000)
    assert u["model_calls"] == 6 and u["usage_records"] == 6 and round(u["cost_usd"], 2) == 0.84
    assert u["unpriced_models"] == [] and u["billing"] == ["subscription"]


def test_desktop_details_only_near_screen_actions() -> None:
    b = TraceBuilder()
    b.worker(5, "done", "clicked")
    b.desktop(4, near=[("mac.act", ["frontmost_app"]), ("shell.run", ["cursor"])])
    d = compose.compose(b.finish())["desktop"]
    assert d["changes"] == 4 and [n["tools"] for n in d["near_ferb"]] == [["mac.act"]]


def test_pasted_parts_show_as_a_marker_in_the_request() -> None:
    b = TraceBuilder(request='summarize <untrusted-data id="ab12">\nsecret plan\n</untrusted-data id="ab12">')
    assert compose.compose(b.finish())["request_text"] == "summarize [pasted text]"


def test_a_task_whose_request_or_context_held_outside_content_is_marked(tmp_path: Path) -> None:
    """S3: the request's origins (root task span) and what the context held (context.source) count, not only tool
    results."""
    b = TraceBuilder()
    b.root["origin"] = "external:paste,owner"
    b.source("recalled memory", "a note from an email", "external:email:someone")
    b.worker(5, "done", "summarized")
    c = compose.compose(b.finish())
    assert {"external:paste", "owner", "external:email:someone"} <= set(c["origins"])
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    ep = c | {"made_by": episodes.MADE_BY, "built_at": T0.isoformat()}
    episodes.write(db, ep)
    episodes.write(db, ep)                                  # idempotent: rebuilding replaces the row
    row = db.conn.execute("SELECT origins, external_read FROM episodes WHERE task_id = ?", (c["task_id"],)).fetchone()
    assert row["external_read"] == 1 and "external:paste" in row["origins"].split(",")
    got = episodes.get(db, c["task_id"])
    assert got is not None and got["external_read"] is True and got["steps"] == c["steps"]
    assert got["verified"] is True and got["usage"] == c["usage"]


def test_a_task_that_read_only_private_data_is_not_marked(tmp_path: Path) -> None:
    c = compose.compose(hf_cache_retry())
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    episodes.write(db, c | {"made_by": episodes.MADE_BY, "built_at": T0.isoformat()})
    got = episodes.get(db, c["task_id"])
    assert got is not None and got["external_read"] is False and got["origins"] == ["private"]


def test_episode_is_written_when_a_task_finishes(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "Doing it.", task("say hi"))], {"worker": done("said hi")})
    chat(daemon, "say hi")
    ep = _episode(daemon, _last_root(daemon))
    assert ep is not None and ep["request_text"] == "say hi" and ep["status"] == "done"
    assert ep["made_by"] == episodes.MADE_BY and json.loads(ep["steps_json"])
    usage = json.loads(ep["usage_json"])
    assert usage["usage_records"] >= 3                      # the intake planner's usage counts too


def test_the_intake_planners_usage_is_in_the_tree(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "Doing it.", task("say hi"))], {"worker": done("said hi")})
    chat(daemon, "say hi")
    root = _last_root(daemon)

    async def go():
        tree = daemon.daemon.spans.tree_json(root)
        planner = next(n for n in tree if n["kind"] == "model.turn")
        return [c["kind"] for c in planner["children"]]
    assert "model.usage" in daemon.run(go())


def test_a_stopped_task_gets_an_episode(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "Working.", task("long job"))], {"worker": STALL})
    c = Client(daemon.sock, timeout=30)
    try:
        for ev in c.stream("chat.send", {"session_id": "s", "text": "long job"}):
            if ev["type"] == "plan":
                c2 = Client(daemon.sock)
                c2.call("task.stop", {"task_ids": [ev["task_id"]]})
                c2.close()
            if ev["type"] in ("outcome", "reply", "clarify"):
                break
    finally:
        c.close()
    root = _last_root(daemon)
    ep = _episode(daemon, root)

    async def status():
        return daemon.daemon.db.conn.execute("SELECT status FROM tasks WHERE id = ?", (root,)).fetchone()["status"]
    assert ep is not None and ep["status"] == daemon.run(status()) and ep["status"] != "done"
    # The worker span records its own status, so a stopped worker reads as cancelled, not as a guess from its error.
    assert any(s["role"] == "Worker" and "worker reported cancelled" in s["text"] for s in json.loads(ep["steps_json"]))


def test_a_task_interrupted_by_a_restart_gets_an_episode(home: Path) -> None:
    d = RunningDaemon(home)
    try:
        async def seed():
            now = d.daemon.clock.now().isoformat()
            d.daemon.db.conn.execute(
                "INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, created_at, "
                "updated_at) VALUES ('ghost', 'ghost', 'r', 'tidy the desktop', 'owner', 'worker', '[]', 'running', ?, ?)",
                (now, now))
            d.daemon.spans.start("task", task_id="ghost", input={"request": "tidy the desktop"})
            d.daemon.db.conn.commit()
        d.run(seed())
    finally:
        d.stop()
    d2 = RunningDaemon(home)
    try:
        ep = _episode(d2, "ghost")
        assert ep is not None and ep["status"] == "interrupted" and ep["request_text"] == "tidy the desktop"
    finally:
        d2.stop()


def test_an_episode_that_cannot_be_built_never_fails_the_task(daemon: RunningDaemon,
                                                              monkeypatch: pytest.MonkeyPatch) -> None:
    def broken(*a: Any, **k: Any) -> dict[str, Any]:
        raise RuntimeError("composer broke")
    monkeypatch.setattr(episodes, "build", broken)
    install(daemon, [sup("task", "Doing it.", task("say hi"))], {"worker": done("said hi")})
    evs = chat(daemon, "say hi")
    assert [e for e in evs if e["type"] == "outcome"][-1]["status"] == "done"
    root = _last_root(daemon)

    async def failed():
        rows = daemon.daemon.db.conn.execute("SELECT data_json FROM journal WHERE kind = 'episode.failed'").fetchall()
        return [json.loads(r["data_json"]) for r in rows]
    assert daemon.run(failed()) == [{"task_id": root, "error": "RuntimeError: composer broke"}]
    assert _episode(daemon, root) is None


def _last_root(daemon: RunningDaemon) -> str:
    async def go():
        return daemon.daemon.db.conn.execute(
            "SELECT id FROM tasks WHERE parent_id IS NULL ORDER BY created_at DESC, rowid DESC").fetchone()["id"]
    return daemon.run(go())


def _episode(daemon: RunningDaemon, task_id: str) -> dict | None:
    async def go():
        row = daemon.daemon.db.conn.execute("SELECT * FROM episodes WHERE task_id = ?", (task_id,)).fetchone()
        return dict(row) if row else None
    return daemon.run(go())
