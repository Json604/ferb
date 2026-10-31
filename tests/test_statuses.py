"""done / done_unverified / failed are distinct (V4); 'unverified' only when a check ran (V5)."""
import io
import json
import shutil
import sqlite3
from pathlib import Path
from typing import Any

import pytest

from ferb.store.db import Database
from tests.conftest import RunningDaemon, chat, cond, done, install, outcome, sup, task, verdict


def _run(daemon: RunningDaemon, worker_final: Any, conds: tuple[dict[str, Any], ...]) -> dict[str, Any]:
    install(daemon, [sup("task", "On it.", task("do it", conds=conds))], {"worker": worker_final})
    return outcome(chat(daemon, "do it"))


def _sql(daemon: RunningDaemon, query: str, *args: Any) -> list[Any]:
    async def go() -> list[Any]:
        return [tuple(r) for r in daemon.daemon.db.conn.execute(query, args).fetchall()]
    return daemon.run(go())


def _failed(summary: str) -> str:
    return json.dumps({"status": "failed", "summary": summary, "artifacts": [], "evidence": []})


# FERB's internal roles and records never reach the owner's text; an exception's class name does only when the
# failure is a bug in FERB, told as one (owner 2026-10-24)
ROLES = ("worker", "checker", "verifier", "task.evidence", "inspection")
INTERNAL = ROLES + ("Error", "Exception")
BUG = "That's a bug in FERB, not something you asked wrong."


# ---------- the three statuses ----------

def test_all_conditions_verified_is_done(daemon: RunningDaemon) -> None:
    evs = _run(daemon, done("did it"), (cond("the file exists"),))
    assert evs["status"] == "done" and evs["verified"] is True


def test_finished_but_a_check_could_not_confirm_is_done_unverified(daemon: RunningDaemon) -> None:
    # Conditions are numbered from 0 for the checker (supervisor._verifier); its met=None is "couldn't confirm".
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"),)))],
            {"worker": done("sent it"),
             "worker.verify": verdict(None, next="accept_unconfirmed", evidence="could not open the Sent folder",
                                      how="Look in your Sent folder")})
    evs = outcome(chat(daemon, "do it"))
    assert evs["status"] == "done_unverified" and evs["verified"] is False
    # owner decision 9: the answer first, then one line naming what isn't confirmed (never stated as fact)
    assert evs["answer"] == "sent it\n\nCouldn't confirm: the email was sent (Look in your Sent folder)."
    assert _sql(daemon, "SELECT status FROM tasks WHERE id = ?", evs["task_id"]) == [("done_unverified",)]
    async def episode() -> Any:
        from ferb.orchestrator import episodes
        return episodes.get(daemon.daemon.db, evs["task_id"])
    ep = daemon.run(episode())
    assert ep["status"] == "done_unverified" and ep["verified"] is False


def test_a_fail_after_an_unconfirmed_check_is_failed(daemon: RunningDaemon) -> None:
    reason = "The Sent folder shows no message to Bob."
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"),)))],
            {"worker": done("sent it"), "worker.verify": verdict(None, next="fail", reason=reason)})
    evs = outcome(chat(daemon, "do it"))
    assert evs["status"] == "failed" and evs["verified"] is False
    assert evs["answer"] == "I couldn't finish this task. " + reason and "sent it" not in evs["answer"]


def test_unconfirmed_is_not_accepted_when_the_check_found_something_false(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"),)))],
            {"worker": done("sent it"),
             "worker.verify": verdict(False, next="accept_unconfirmed", evidence="the draft is still in Drafts")})
    evs = outcome(chat(daemon, "do it"))
    assert evs["status"] == "failed"
    assert "the email was sent: the draft is still in Drafts" in evs["answer"] and "sent it" not in evs["answer"]


def test_a_failed_worker_is_failed_not_unverified(daemon: RunningDaemon) -> None:
    evs = _run(daemon, _failed("couldn't find it"), (cond("the file exists"),))
    assert evs["status"] == "failed" and "unverified" not in evs["summary"]
    # The checker gave no reason: the attempt's own reason for not finishing is said (V5)
    assert evs["answer"] == "I couldn't finish this task. couldn't find it."
    assert not any(word in evs["answer"] for word in INTERNAL)


def test_a_failed_worker_shows_the_real_reason(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the file exists"),)))],
            {"worker": _failed("couldn't find it"),
             "worker.verify": verdict(None, next="fail", reason="There is no report.md in your Downloads folder.")})
    evs = outcome(chat(daemon, "do it"))
    assert evs["status"] == "failed" and "unverified" not in evs["answer"]
    assert evs["answer"] == "I couldn't finish this task. There is no report.md in your Downloads folder."


def test_a_crash_before_any_check_says_what_happened(daemon: RunningDaemon) -> None:
    async def crash(tree: Any, span: str) -> Any:
        raise RuntimeError("boom")
    daemon.daemon.supervisor._worker = crash
    evs = _run(daemon, done("never runs"), (cond("the file exists"),))
    assert evs["status"] == "failed" and "unverified" not in evs["answer"]
    assert evs["answer"] == ("I couldn't finish this task. I couldn't continue this task: something broke inside "
                             "FERB (RuntimeError: boom). " + BUG)
    assert not any(word in evs["answer"] for word in ROLES)


def test_a_crash_after_an_earlier_unconfirmed_check_is_not_reported_as_checked(daemon: RunningDaemon) -> None:
    supervisor = daemon.daemon.supervisor
    real, calls = supervisor.run_worker, []

    async def second_attempt_crashes(tree: Any, task_: Any, *a: Any, **k: Any) -> Any:
        if task_.id.endswith(".worker"):
            calls.append(task_.id)
            if len(calls) == 2:
                raise RuntimeError("boom")
        return await real(tree, task_, *a, **k)
    supervisor.run_worker = second_attempt_crashes
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"),)))],
            {"worker": done("sent it"),
             "worker.verify": verdict(None, next="retry", instructions="Open Sent and check.",
                                      evidence="the Sent folder did not load")})
    evs = outcome(chat(daemon, "do it"))
    assert len(calls) == 2 and evs["status"] == "failed"
    assert "the Sent folder did not load" not in evs["answer"] and "sent it" not in evs["answer"]
    assert "something broke inside FERB (RuntimeError: boom)" in evs["answer"]


@pytest.mark.parametrize("bad", ["not json", "{}"])
def test_a_crashed_check_leaves_finished_work_unconfirmed_and_says_why(daemon: RunningDaemon, bad: str) -> None:
    # "not json": the checker's run produced no usable verdict; "{}": its verdict failed validation (verify raises).
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"),)))],
            {"worker": done("sent it"), "worker.verify": bad})
    evs = outcome(chat(daemon, "do it"))
    assert evs["status"] == "done_unverified" and not evs["verified"]
    assert evs["answer"].startswith("sent it\n\nCouldn't confirm: the check couldn't run")
    # why, in plain words (owner 2026-10-24): the checker's answer didn't fit FERB's form
    assert "the AI's answer didn't fit the form FERB requires" in evs["answer"]
    assert "unverified" not in evs["answer"] and not any(word in evs["answer"] for word in ROLES)


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
    assert evs["status"] == "done_unverified" and evs["verified"] is False       # never a bare "done ✓"
    assert evs["answer"] == "done\n\nCouldn't confirm: there was nothing to check it against."


def test_a_condition_found_false_is_failed_and_says_what_the_check_found(daemon: RunningDaemon) -> None:
    v = json.dumps({"results": [{"index": 0, "met": False, "evidence": "the draft is still in Drafts"},
                                {"index": 1, "met": None, "evidence": "the Sent folder did not load"}]})
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"), cond("Bob got it"))))],
            {"worker": done("sent it"), "worker.verify": v})
    evs = outcome(chat(daemon, "do it"))
    assert evs["status"] == "failed" and evs["verified"] is False
    assert "the email was sent: the draft is still in Drafts" in evs["answer"]
    assert "Bob got it: the Sent folder did not load" in evs["answer"]


def _unattended(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    """A run nobody attends (a scheduled one): needing the owner ends it, reported, instead of pausing to ask."""
    monkeypatch.setattr(type(daemon.daemon.supervisor), "_unattended", lambda self, task: True)


def test_a_question_before_any_check_never_says_unverified(daemon: RunningDaemon,
                                                           monkeypatch: pytest.MonkeyPatch) -> None:
    _unattended(daemon, monkeypatch)
    ask = json.dumps({"status": "needs_user", "summary": "Which account should I use?", "artifacts": [],
                      "evidence": []})
    evs = _run(daemon, ask, (cond("signed in"),))
    assert evs["status"] == "needs_user" and "unverified" not in evs["answer"]
    assert evs["answer"] == "I need your answer to finish this task. Which account should I use?"


def test_the_checkers_question_reaches_the_owner(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    _unattended(daemon, monkeypatch)
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"),)))],
            {"worker": done("sent it"),
             "worker.verify": verdict(None, next="ask", question="Which Gmail account did you mean?")})
    evs = outcome(chat(daemon, "do it"))
    assert evs["status"] == "needs_user"
    assert evs["answer"] == "I need your answer to finish this task. Which Gmail account did you mean?"


def test_a_retry_after_an_unconfirmed_check_that_then_fails_is_failed(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"),)))],
            {"worker": [done("sent it"), _failed("Gmail is signed out.")],
             "worker.verify": [verdict(None, next="retry", instructions="Open Sent and check."),
                               verdict(None, next="fail", reason="Gmail is signed out, so nothing could be sent.")]})
    evs = outcome(chat(daemon, "do it"))
    assert evs["status"] == "failed" and "unverified" not in evs["answer"]
    assert evs["answer"] == "I couldn't finish this task. Gmail is signed out, so nothing could be sent."


def test_a_cleanup_error_after_finished_work_is_said_in_a_code_line(daemon: RunningDaemon) -> None:
    """Prompts decision 1: no model reviews the clean-up; code records it and says what it couldn't release."""
    async def cleanup(task_id: str) -> list[dict[str, Any]]:
        return [{"tab": "temporary", "closed": False, "error": "the browser didn't answer"}]
    daemon.daemon.supervisor.on_task_end = cleanup
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"),)))],
            {"worker": done("sent it")})
    evs = outcome(chat(daemon, "do it"))
    assert evs["status"] == "done"
    assert evs["answer"].startswith("sent it\n\nI couldn't tidy up everything after this task: ")
    assert "the browser didn't answer" in evs["answer"]


# ---------- the readers tell them apart ----------

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


def test_stats_with_no_requests_in_the_window(tmp_path: Path) -> None:
    from ferb.cli.admin import stats
    _seed(tmp_path, ())
    out = io.StringIO()
    assert stats(tmp_path, out, days=7) == 0
    assert "Requests in the last 7 day(s): 0\n" in out.getvalue()


def test_stats_counts_unverified_apart_from_done_and_failed(tmp_path: Path) -> None:
    from ferb.cli.admin import stats
    _seed(tmp_path, ("done", "done_unverified", "failed", "failed"))
    out = io.StringIO()
    assert stats(tmp_path, out, days=36500) == 0
    text = out.getvalue()
    assert "done 1 (25%), not confirmed 1 (25%), failed 2 (50%)" in text
    assert "unverified" not in text                          # V5: the label is "not confirmed", in every row
    assert "  not confirmed   1" in text and "  failed          2" in text



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
    assert " · done (not confirmed) · " in first_line("done_unverified", False)
    assert " · failed ✗ · " in first_line("failed", False)
    assert " · done · " in first_line("done", None)          # an older record that never said it was checked


# ---------- scheduled runs ----------

def test_an_unverified_scheduled_run_neither_clears_nor_adds_a_failure(daemon: RunningDaemon) -> None:
    from datetime import timedelta
    from ferb.orchestrator.schedules import Rule
    from tests.support.schedules import fire, results, run

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
            {"worker": done("sent it"),
             "worker.verify": verdict(None, next="accept_unconfirmed", evidence="the Sent folder did not load")})
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
    # migration 0049 drops the dead depends_json
    kept = ("rowid, id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, outcome_json, "
            "created_at, updated_at")
    tasks_before = c.execute(f"SELECT {kept} FROM tasks ORDER BY rowid").fetchall()
    assert [r[0] for r in tasks_before] == [1, 3, 4, 5]
    context_before = c.execute("SELECT rowid, * FROM task_context ORDER BY rowid").fetchall()
    columns_before = c.execute("PRAGMA table_info(tasks)").fetchall()
    with pytest.raises(sqlite3.IntegrityError):
        c.execute("UPDATE tasks SET status = 'done_unverified' WHERE id = 'zz'")

    assert 21 in d.migrate()
    assert [tuple(r) for r in c.execute(f"SELECT {kept} FROM tasks ORDER BY rowid")] == \
        [tuple(r) for r in tasks_before]
    assert [tuple(r) for r in c.execute("SELECT rowid, * FROM task_context ORDER BY rowid")] == \
        [tuple(r) for r in context_before]
    assert [r[1] for r in c.execute("PRAGMA table_info(tasks)")] == \
        [r[1] for r in columns_before if r[1] != "depends_json"]
    c.execute("UPDATE tasks SET status = 'paused' WHERE id = 'mm'")         # 0049: a task paused in place
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
