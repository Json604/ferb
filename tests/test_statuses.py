"""done / done_unverified / failed are distinct (V4); 'unverified' only when a check ran (V5)."""
import io
import json
import shutil
import sqlite3
from pathlib import Path
from typing import Any

import pytest

from ferb.store.db import Database
from tests.conftest import RunningDaemon, chat, cond, done, install, outcome, sup, task


def _run(daemon: RunningDaemon, worker_final: Any, conds: tuple[dict[str, Any], ...]) -> dict[str, Any]:
    install(daemon, [sup("task", "On it.", task("do it", conds=conds))], {"worker": worker_final})
    return outcome(chat(daemon, "do it"))


def _sql(daemon: RunningDaemon, query: str, *args: Any) -> list[Any]:
    async def go() -> list[Any]:
        return [tuple(r) for r in daemon.daemon.db.conn.execute(query, args).fetchall()]
    return daemon.run(go())


def _unconfirmed(evidence: str) -> str:
    return json.dumps({"results": [{"index": 0, "met": None, "evidence": evidence}]})


# ---------- the three statuses ----------

def test_all_conditions_verified_is_done(daemon: RunningDaemon) -> None:
    evs = _run(daemon, done("did it"), (cond("the file exists"),))
    assert evs["status"] == "done" and evs["verified"] is True


def test_finished_but_a_check_could_not_confirm_is_done_unverified(daemon: RunningDaemon) -> None:
    # Conditions are numbered from 0 for the checker (supervisor._verifier); its met=None is "couldn't confirm".
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"),)))],
            {"worker": done("sent it"), "worker.verify": _unconfirmed("could not open the Sent folder")})
    evs = outcome(chat(daemon, "do it"))
    assert evs["status"] == "done_unverified" and evs["verified"] is False
    assert "unverified" in evs["summary"] and "could not open the Sent folder" in evs["summary"]
    assert evs["answer"].startswith("I did it, but couldn't confirm everything:")
    assert "- the email was sent — unverified: could not open the Sent folder" in evs["answer"]
    assert "sent it" not in evs["answer"]                     # a worker's claim is fact only for a verified condition
    assert _sql(daemon, "SELECT status FROM tasks WHERE id = ?", evs["task_id"]) == [("done_unverified",)]
    async def episode() -> Any:
        from ferb.orchestrator import episodes
        return episodes.get(daemon.daemon.db, evs["task_id"])
    ep = daemon.run(episode())
    assert ep["status"] == "done_unverified" and ep["verified"] is False


def test_a_failed_worker_is_failed_not_unverified(daemon: RunningDaemon) -> None:
    evs = _run(daemon, json.dumps({"status": "failed", "summary": "couldn't find it", "artifacts": [], "evidence": []}),
               (cond("the file exists"),))
    assert evs["status"] == "failed" and "unverified" not in evs["summary"]
    assert evs["answer"].startswith("I couldn't finish this task.") and "- the file exists" in evs["answer"]
    assert not any(word in evs["answer"] for word in ("worker", "checker", "task.evidence", "inspection"))


def test_a_failed_worker_shows_the_real_reason(daemon: RunningDaemon) -> None:
    finish = json.dumps({"action": "finish", "worker_type": "default", "instructions": "Report the missing file.",
                         "question": "", "stop_reason": "There is no report.md in your Downloads folder."})
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the file exists"),)))],
            {"worker": json.dumps({"status": "failed", "summary": "couldn't find it", "artifacts": [], "evidence": []}),
             "worker.dispatch": finish})
    evs = outcome(chat(daemon, "do it"))
    assert evs["status"] == "failed" and "unverified" not in evs["answer"]
    assert "Why work stopped: There is no report.md in your Downloads folder." in evs["answer"]
    assert "Report the missing file." not in evs["answer"]


def test_a_crash_before_any_check_says_what_happened(daemon: RunningDaemon) -> None:
    async def crash(tree: Any, span: str) -> Any:
        raise RuntimeError("boom")
    daemon.daemon.supervisor._worker = crash
    evs = _run(daemon, done("never runs"), (cond("the file exists"),))
    assert evs["status"] == "failed" and "unverified" not in evs["answer"]
    assert "Why work stopped: I couldn't continue this task (RuntimeError)." in evs["answer"]


def test_no_conditions_is_unverified_only_because_nothing_could_be_checked(daemon: RunningDaemon) -> None:
    # A planned task always carries conditions (SupervisorOutcome refuses one without), so this is a direct run.
    from ferb.orchestrator.schemas import TaskSpec

    async def go() -> dict[str, Any]:
        supervisor = daemon.daemon.supervisor
        install(daemon, [], {"worker": done("done")})
        events = [e async for e in supervisor.run_tree(TaskSpec.model_validate(task("do it", conds=())),
                                                       "On it.", "no-conditions", "do it")]
        return outcome(events)
    evs = daemon.run(go())
    assert evs["status"] in ("done_unverified", "failed")    # never a bare "done ✓"
    assert evs["status"] == "done_unverified" and evs["verified"] is False
    assert evs["answer"] == "Completion is unverified (no checkable conditions)."


def test_a_condition_found_false_is_failed_and_unconfirmed_ones_say_so(daemon: RunningDaemon) -> None:
    verdict = json.dumps({"results": [{"index": 0, "met": False, "evidence": "the draft is still in Drafts"},
                                      {"index": 1, "met": None, "evidence": "the Sent folder did not load"}]})
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"), cond("Bob got it"))))],
            {"worker": done("sent it"), "worker.verify": verdict})
    evs = outcome(chat(daemon, "do it"))
    assert evs["status"] == "failed" and evs["verified"] is False
    assert "- the email was sent — failed: the draft is still in Drafts" in evs["answer"]
    assert "- Bob got it — unverified: the Sent folder did not load" in evs["answer"]   # a check ran for it


def test_a_question_before_any_check_never_says_unverified(daemon: RunningDaemon) -> None:
    ask = json.dumps({"status": "needs_user", "summary": "Which account should I use?", "artifacts": [],
                      "evidence": []})
    evs = _run(daemon, ask, (cond("signed in"),))
    assert evs["status"] == "needs_user" and "unverified" not in evs["answer"]
    assert "Question: Which account should I use?" in evs["answer"]


def test_a_question_after_an_unconfirmed_check_keeps_the_check_result(daemon: RunningDaemon) -> None:
    ask = json.dumps({"action": "ask", "worker_type": "default", "instructions": "",
                      "question": "Can you open the Sent folder for me?"})
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"),)))],
            {"worker": done("sent it"), "worker.verify": _unconfirmed("the Sent folder did not load"),
             "worker.dispatch": ask})
    evs = outcome(chat(daemon, "do it"))
    assert evs["status"] == "needs_user"
    assert "- the email was sent — unverified: the Sent folder did not load" in evs["answer"]
    assert "Question: Can you open the Sent folder for me?" in evs["answer"]


def test_a_retry_after_an_unconfirmed_check_that_then_fails_is_failed(daemon: RunningDaemon) -> None:
    retry = json.dumps({"action": "retry", "worker_type": "default", "instructions": "Open Sent and check.",
                        "question": ""})
    finish = json.dumps({"action": "finish", "worker_type": "default", "instructions": "", "question": "",
                         "stop_reason": "Gmail is signed out."})
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"),)))],
            {"worker": [done("sent it"), json.dumps({"status": "failed", "summary": "Gmail is signed out.",
                                                     "artifacts": [], "evidence": []})],
             "worker.verify": _unconfirmed("the Sent folder did not load"), "worker.dispatch": [retry, finish]})
    evs = outcome(chat(daemon, "do it"))
    assert evs["status"] == "failed" and "unverified" not in evs["answer"]   # the last attempt was never checked
    assert "Gmail is signed out." in evs["answer"]


def test_a_planning_failure_after_an_unconfirmed_finish_stays_done_unverified(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"),)))],
            {"worker": done("sent it"), "worker.verify": _unconfirmed("the Sent folder did not load"),
             "worker.dispatch": "not json"})
    evs = outcome(chat(daemon, "do it"))
    assert evs["status"] == "done_unverified"
    assert "- the email was sent — unverified: the Sent folder did not load" in evs["answer"]
    assert "I couldn't plan another attempt" in evs["answer"]


@pytest.mark.parametrize("review,status", [("done", "done_unverified"), ("failed", "failed")])
def test_cleanup_after_an_unconfirmed_finish_keeps_it_unverified_unless_cleanup_broke_it(
        daemon: RunningDaemon, review: str, status: str) -> None:
    async def cleanup(task_id: str) -> list[dict[str, Any]]:
        return [{"tab": "temporary", "closed": True}]
    daemon.daemon.supervisor.on_task_end = cleanup
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"),)))],
            {"worker": done("sent it"), "worker.verify": _unconfirmed("the Sent folder did not load"),
             "worker.resources": json.dumps({"status": review, "summary": "Closed the temporary tab.",
                                             "artifacts": [], "evidence": []})})
    evs = outcome(chat(daemon, "do it"))
    assert evs["status"] == status and not evs["verified"]
    assert "- the email was sent — unverified: the Sent folder did not load" in evs["answer"]   # a check ran


# ---------- the readers tell them apart ----------

def test_chat_verbose_shows_three_distinct_outcomes() -> None:
    from ferb.cli.chat import render
    shown = {s: render({"type": "outcome", "status": s, "summary": "S", "answer": "A", "verified": s == "done"},
                       verbose=True) for s in ("done", "done_unverified", "failed")}
    assert shown["done"].startswith("ferb: done ✓ — S")
    assert shown["done_unverified"].startswith("ferb: done — unverified — S")
    assert shown["failed"].startswith("ferb: failed ✗ — S")


def _seed(tmp_path: Path, statuses: tuple[str, ...]) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    for i, st in enumerate(statuses):
        db.conn.execute("INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, "
                        "outcome_json, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                        (f"t{i}", f"t{i}", "r", "g", "owner", "mixed", "[]", st,
                         json.dumps({"verified": st == "done"}), "2099-01-01T00:00:00+00:00",
                         "2099-01-01T00:01:00+00:00"))
    db.close()


def test_stats_counts_unverified_apart_from_done_and_failed(tmp_path: Path) -> None:
    from ferb.cli.admin import stats
    _seed(tmp_path, ("done", "done_unverified", "failed", "failed"))
    out = io.StringIO()
    assert stats(tmp_path, out, days=36500) == 0
    text = out.getvalue()
    assert "done 1 (25%)" in text and "unverified 1 (25%)" in text and "failed 2 (50%)" in text
    assert "  done_unverified 1" in text


def test_baseline_counts_unverified_apart_from_done_and_failed(tmp_path: Path) -> None:
    from ferb.cli.admin import baseline
    _seed(tmp_path, ("done", "done_unverified", "failed", "failed"))
    assert baseline(tmp_path, io.StringIO()) == 0
    data = json.loads(next((tmp_path / "baselines").glob("*.json")).read_text())
    assert (data["done"], data["done_pct"]) == (1, 25)
    assert (data["unverified"], data["unverified_pct"]) == (1, 25)
    assert (data["failed"], data["failed_pct"]) == (2, 50)
    assert data["avg_task_s"] == 60.0                       # done_unverified is a finished task


def test_trace_story_names_each_status() -> None:
    from ferb.trace import compose, render
    from tests.support.trace_builder import TraceBuilder

    def first_line(status: str, verified: bool | None) -> str:
        b = TraceBuilder()
        b.worker(5, "done", "sent it")
        ep = compose.compose(b.finish(status=status, verified=verified, answer="A"))
        assert ep["status"] == status and ep["verified"] is verified
        return render.story(ep | {"made_by": "x", "built_at": "2026-10-07T11:07:00+05:30"})[0]
    assert " · done ✓ verified · " in first_line("done", True)
    assert " · done (unverified) · " in first_line("done_unverified", False)
    assert " · failed ✗ · " in first_line("failed", False)
    assert " · done · " in first_line("done", None)          # an older record that never said it was checked


# ---------- scheduled runs ----------

def test_an_unverified_scheduled_run_neither_clears_nor_adds_a_failure(daemon: RunningDaemon) -> None:
    from datetime import timedelta
    from ferb.orchestrator.schedules import Rule
    from tests.test_schedules import fire, results, run

    first = daemon.daemon.clock.now() + timedelta(minutes=1)

    def make() -> Any:
        d = daemon.daemon
        s = d.schedules.add("send the report", "every day send the report", {}, Rule.build("daily", first), first)
        t = d.timers.add("schedule", first, s.id)
        d.schedules.set_timer(s.id, t.id, first)
        d.schedules.ran(s.id, False)
        d.schedules.ran(s.id, False)
        return s
    s = run(daemon, make)
    install(daemon, [sup("task", "On it.", task("send the report", conds=(cond("the report was sent"),)))],
            {"worker": done("sent it"), "worker.verify": _unconfirmed("the Sent folder did not load")})
    fire(daemon, 61)
    r = results(daemon)
    assert r and r[0]["status"] == "done_unverified"
    assert run(daemon, lambda: daemon.daemon.schedules.get(s.id)).failures == 2
    assert run(daemon, lambda: daemon.daemon.schedules.ran(s.id, True)) == 0


# ---------- the migration ----------

def _db_at(tmp_path: Path, version: int, monkeypatch: pytest.MonkeyPatch) -> Database:
    from ferb.store import db as dbmod
    real = dbmod.MIGRATIONS
    older = tmp_path / "migrations"
    older.mkdir()
    for f in real.glob("[0-9][0-9][0-9][0-9]_*.sql"):
        if int(f.name[:4]) <= version:
            shutil.copy(f, older / f.name)
    monkeypatch.setattr(dbmod, "MIGRATIONS", older)
    d = Database(tmp_path / "ferb.db")
    d.migrate()
    monkeypatch.setattr(dbmod, "MIGRATIONS", real)
    return d


def test_migration_keeps_tasks_context_and_order_and_accepts_done_unverified(tmp_path: Path,
                                                                            monkeypatch: pytest.MonkeyPatch) -> None:
    d = _db_at(tmp_path, 20, monkeypatch)
    c = d.conn
    now = "2026-10-08T00:00:00+00:00"
    for tid, parent, st in (("zz", None, "done"), ("gone", None, "failed"), ("aa", None, "failed"),
                            ("aa.worker", "aa", "done"), ("mm", None, "needs_user")):
        c.execute("INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
                  "outcome_json, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                  (tid, tid.split(".")[0], parent, "r-" + tid, "g " + tid, "owner", "mixed", "[]", st,
                   json.dumps({"summary": tid}), now, now))
    c.execute("DELETE FROM tasks WHERE id = 'gone'")           # a gap: row ids are kept, not renumbered
    c.execute("INSERT INTO conversations (id) VALUES ('c1')")
    for tid, prior in (("aa", "zz"), ("mm", "aa")):
        c.execute("INSERT INTO task_context (task_id, conversation_id, spec_json, owner_words, prior_task, "
                  "request_payload) VALUES (?, 'c1', '{}', ?, ?, ?)", (tid, "words " + tid, prior, "payload " + tid))
    tasks_before = c.execute("SELECT rowid, * FROM tasks ORDER BY rowid").fetchall()
    assert [r[0] for r in tasks_before] == [1, 3, 4, 5]
    context_before = c.execute("SELECT rowid, * FROM task_context ORDER BY rowid").fetchall()
    columns_before = c.execute("PRAGMA table_info(tasks)").fetchall()
    with pytest.raises(sqlite3.IntegrityError):
        c.execute("UPDATE tasks SET status = 'done_unverified' WHERE id = 'zz'")

    assert d.migrate() == [21]
    assert [tuple(r) for r in c.execute("SELECT rowid, * FROM tasks ORDER BY rowid")] == \
        [tuple(r) for r in tasks_before]
    assert [tuple(r) for r in c.execute("SELECT rowid, * FROM task_context ORDER BY rowid")] == \
        [tuple(r) for r in context_before]
    assert [tuple(r) for r in c.execute("PRAGMA table_info(tasks)")] == [tuple(r) for r in columns_before]
    assert {r["name"] for r in c.execute("PRAGMA index_list(tasks)") if r["origin"] == "c"} == {"tasks_tree",
                                                                                                "tasks_status"}
    assert not {r[0] for r in c.execute("SELECT name FROM sqlite_master")} & {"tasks_new", "task_context_hold"}
    assert c.execute("PRAGMA foreign_key_check").fetchall() == []
    assert c.execute("PRAGMA foreign_keys").fetchone()[0] == 1
    c.execute("UPDATE tasks SET status = 'done_unverified' WHERE id = 'zz'")
    with pytest.raises(sqlite3.IntegrityError):
        c.execute("UPDATE tasks SET status = 'verified-ish' WHERE id = 'zz'")
    with pytest.raises(sqlite3.IntegrityError):                # task_context still references tasks
        c.execute("INSERT INTO task_context (task_id, conversation_id, spec_json, owner_words) "
                  "VALUES ('nope', 'c1', '{}', 'w')")
    with pytest.raises(sqlite3.IntegrityError):
        c.execute("DELETE FROM tasks WHERE id = 'aa'")
