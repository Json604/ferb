"""One permanent record per task, composed by code from its trace (L1, D21, MP1)."""
import json
import sqlite3
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.client import Client
from ferb.memory.journal import Journal
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


def _failed_turn(kind: str) -> dict[str, Any]:
    """One trace whose `kind` model span ended with an error, though its recorded text parses as a decision."""
    b = TraceBuilder()
    if kind == "model.turn":
        n = b.planner(3, {"kind": "task", "text": "Deleting it.", "task": {"goal": b.goal}})
    elif kind == "model.dispatch":
        b.worker(5, "failed", "could not delete")
        n = b.dispatch(2, "retry", "try again")
    elif kind == "model.question":
        b.worker(5, "done", "deleted")
        n = b.question(2, {"ask_owner": False, "answer": "the hf one", "question": ""})
    else:
        b.worker(5, "done", "deleted")
        n = b.reconcile(2, "done", "All resources released.")
    n["error"] = "ValidationError: 1 validation error for the decision"
    return b.finish()


@pytest.mark.parametrize("kind", ["model.turn", "model.dispatch", "model.question", "model.resources"])
def test_a_failed_model_turn_reads_as_a_failure_not_a_decision(kind: str) -> None:
    c = compose.compose(_failed_turn(kind))
    step = next(s for s in c["steps"] if s["role"] == "Planner")
    assert step["mark"] == "✗" and step["text"] == "failed: ValidationError: 1 validation error for the decision"
    failed = 2 if kind == "model.dispatch" else 1               # the dispatch case's worker also reported failed
    assert c["problems"] == [f"✗ {failed} step{'s' if failed > 1 else ''} failed"]   # a failed retry is no retry


def test_the_reconciler_is_a_planner_step_naming_what_it_decided() -> None:
    b = TraceBuilder()
    b.worker(5, "done", "Opened the page and saved it.")
    b.checker(3, [True, True], ["saved file exists", "nothing else changed"])
    b.reconcile(4, "failed", "The tab closed during cleanup, so the page is no longer open.")
    c = compose.compose(b.finish(status="failed", verified=False))
    assert [s["role"] for s in c["steps"]] == ["Worker", "Checker", "Planner"]
    assert c["steps"][2] == {"at": c["steps"][2]["at"], "role": "Planner", "seconds": 4.0, "mark": "",
                             "text": "after cleanup: failed — The tab closed during cleanup, so the page is no "
                                     "longer open."}
    assert c["usage"]["model_calls"] == 3


def test_spans_still_open_read_as_not_finished() -> None:
    b = TraceBuilder()
    p = b.planner(3, {"kind": "task", "text": "Deleting it.", "task": {"goal": b.goal}})
    w = b.worker(5, "done", "deleted")
    c_ = b.checker(3, [True, True], ["gone", "only one deletion"])
    trace = b.finish()
    for n in (p, w):
        n["ended_at"], n["output"] = None, None
    b.root["ended_at"] = None
    c = compose.compose(trace)
    assert [(s["role"], s["mark"], s["text"], s["seconds"]) for s in c["steps"][:2]] == [
        ("Planner", "⏹", "did not finish (interrupted)", None),
        ("Worker", "⏹", "did not finish (interrupted)", None)]
    assert c["steps"][2]["mark"] == "✓" and c_["ended_at"]           # an open worker is not "verified in step 3"
    assert c["problems"] == ["⏹ 2 steps did not finish"]
    assert c["ended_at"] is None and c["seconds"] is None            # the last timestamp is a lower bound, not the end


def test_unpriced_usage_is_named_and_never_counted_as_free() -> None:
    b = TraceBuilder()
    w = b.worker(5, "done", "deleted")
    b.usage(w, 1000, 0, 10, None, model="new-model-x")
    u = compose.compose(b.finish())["usage"]
    assert u["unpriced_models"] == ["new-model-x"] and u["cost_usd"] is None
    b2 = TraceBuilder()
    w2 = b2.worker(5, "done", "deleted")
    b2.usage(w2, 1000, 0, 10, 0.25)
    b2.usage(w2, 1000, 0, 10, None, model="new-model-x")
    u2 = compose.compose(b2.finish())["usage"]
    assert u2["cost_usd"] == 0.25 and u2["unpriced_models"] == ["new-model-x"] and u2["usage_records"] == 2


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


def test_a_read_recorded_without_an_origin_counts_as_outside_content(tmp_path: Path) -> None:
    """I1 (S3, MP2): every span from before F3 has no origin. A reading tool call or a context source that holds
    output but no origin is unlabeled outside content, the same rule task.evidence labels it by; an action's
    acknowledgement without one adds nothing."""
    b = TraceBuilder()
    w = b.worker(5, "done", "read the page")
    b.tool(w, 1, 0.5, "browser.read", {"url": "https://x.example"}, "text", origin=None, kind="read")
    c = compose.compose(b.finish())
    assert c["origins"] == ["external:unlabeled"]
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    episodes.write(db, c | {"made_by": episodes.MADE_BY, "built_at": T0.isoformat()})
    got = episodes.get(db, c["task_id"])
    assert got is not None and got["external_read"] is True
    b2 = TraceBuilder()
    w2 = b2.worker(5, "done", "clicked")
    b2.tool(w2, 1, 0.5, "browser.act", {"element": "e1"}, "clicked", origin=None, kind="click")
    b2.source("recalled memory", "a note", None)
    assert compose.compose(b2.finish())["origins"] == ["external:unlabeled"]      # the context source, not the click
    b3 = TraceBuilder()
    b3.tool(b3.worker(5, "done", "clicked"), 1, 0.5, "browser.act", {"element": "e1"}, "clicked", origin=None,
            kind="click")
    assert compose.compose(b3.finish())["origins"] == []


def test_a_task_that_read_only_private_data_is_not_marked(tmp_path: Path) -> None:
    c = compose.compose(hf_cache_retry())
    c["usage"]["unpriced_models"] = ["modèle-ü"]
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    episodes.write(db, c | {"made_by": episodes.MADE_BY, "built_at": T0.isoformat()})
    got = episodes.get(db, c["task_id"])
    assert got is not None and got["external_read"] is False and got["origins"] == ["private"]
    raw = db.conn.execute("SELECT usage_json FROM episodes WHERE task_id = ?", (c["task_id"],)).fetchone()[0]
    assert "modèle-ü" in raw                                # stored as written, like the other JSON columns


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


def _seed_ghost(home: Path) -> None:
    """A daemon that stops while task 'ghost' runs: its root, worker and checker spans are still open."""
    d = RunningDaemon(home)
    try:
        async def seed():
            now = d.daemon.clock.now().isoformat()
            d.daemon.db.conn.execute(
                "INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, created_at, "
                "updated_at) VALUES ('ghost', 'ghost', 'r', 'tidy the desktop', 'owner', 'worker', '[]', 'running', ?, ?)",
                (now, now))
            root = d.daemon.spans.start("task", task_id="ghost", input={"request": "tidy the desktop"})
            d.daemon.spans.start("worker", task_id="ghost.worker", parent_id=root.id,
                                 input={"goal": "tidy the desktop", "role": "worker", "scopes": []})
            d.daemon.spans.start("worker", task_id="ghost.worker.verify", parent_id=root.id,
                                 input={"goal": "verify", "role": "verifier", "scopes": []})
            d.daemon.db.conn.commit()
        d.run(seed())
    finally:
        d.stop()


def test_a_task_interrupted_by_a_restart_gets_an_episode(home: Path) -> None:
    _seed_ghost(home)
    d2 = RunningDaemon(home)
    try:
        ep = _episode(d2, "ghost")
        assert ep is not None and ep["status"] == "interrupted" and ep["request_text"] == "tidy the desktop"
        steps = json.loads(ep["steps_json"])
        assert [(s["role"], s["mark"], s["text"], s["seconds"]) for s in steps] == [
            ("Worker", "⏹", "did not finish (interrupted)", None),
            ("Checker", "⏹", "did not finish (interrupted)", None)]
        assert json.loads(ep["problems_json"]) == ["⏹ 2 steps did not finish"]
        assert ep["ended_at"] is None and ep["seconds"] is None
    finally:
        d2.stop()


def test_a_failing_episode_fallback_never_stops_start_up(home: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _seed_ghost(home)
    real_append = Journal.append

    def append(self: Journal, kind: str, data: Any = None, origin: Any = None) -> Any:
        if kind == "episode.failed":
            raise sqlite3.OperationalError("disk I/O error")
        return real_append(self, kind, data, origin)
    monkeypatch.setattr(episodes, "build", _broken)
    monkeypatch.setattr(Journal, "append", append)
    d2 = RunningDaemon(home)                                # start-up goes on: no exception here
    try:
        assert _episode(d2, "ghost") is None and Client(d2.sock).call("ping", {}) is not None
    finally:
        d2.stop()


def _broken(*a: Any, **k: Any) -> dict[str, Any]:
    raise RuntimeError("composer broke")


def test_an_episode_that_cannot_be_built_never_fails_the_task(daemon: RunningDaemon,
                                                              monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(episodes, "build", _broken)
    install(daemon, [sup("task", "Doing it.", task("say hi"))], {"worker": done("said hi")})
    evs = chat(daemon, "say hi")
    assert [e for e in evs if e["type"] == "outcome"][-1]["status"] == "done"
    root = _last_root(daemon)

    async def failed():
        rows = daemon.daemon.db.conn.execute("SELECT data_json FROM journal WHERE kind = 'episode.failed'").fetchall()
        return [json.loads(r["data_json"]) for r in rows]
    assert daemon.run(failed()) == [{"task_id": root, "error": "RuntimeError: composer broke"}]
    assert _episode(daemon, root) is None


def test_a_failing_episode_fallback_never_skips_the_tasks_cleanup(daemon: RunningDaemon,
                                                                  monkeypatch: pytest.MonkeyPatch) -> None:
    journal = daemon.daemon.supervisor.journal
    real_append = journal.append

    def append(kind: str, data: Any = None, origin: Any = None) -> Any:
        if kind == "episode.failed":
            raise sqlite3.OperationalError("disk I/O error")
        return real_append(kind, data, origin)
    monkeypatch.setattr(episodes, "build", _broken)
    monkeypatch.setattr(journal, "append", append)
    install(daemon, [sup("task", "Doing it.", task("say hi"))], {"worker": done("said hi")})
    finished: list[dict[str, Any]] = []
    publish = daemon.daemon.supervisor.publish
    monkeypatch.setattr(daemon.daemon.supervisor, "publish",
                        lambda kind, data: (finished.append(data) if kind == "task.finished" else None,
                                            publish(kind, data)))
    evs = chat(daemon, "say hi")
    root = _last_root(daemon)
    assert [e for e in evs if e["type"] == "outcome"][-1]["status"] == "done"
    assert {"task_id": root, "status": "done"} in finished and daemon.daemon.supervisor._tree_of == {}


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
