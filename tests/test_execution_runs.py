"""Step 8a end-to-end runs (tests decision 1): the real daemon, gateway and scripted model through the one execution
path. A single task is a one-step plan; one status, finish and stop; "needs the owner" pauses in place and asks; pause
and resume (owner decision 8); the check-in after 5 attempts (owner decision 7); a loop survives a restart; a scheduled
run declines questions and stops at 5."""
import json
import threading
import time
from datetime import timedelta
from typing import Any

from ferb.orchestrator.attempts import CHECK_IN_AFTER
from ferb.orchestrator.plans import Plan, Step
from ferb.policy.targets import Targets
from tests.conftest import (STALL, RunningDaemon, answer_question, chat, done, install, outcome, sup, task, writes)
from tests.support.schedules import make_schedule, own_daemon, run


def _report(status: str, summary: str) -> str:
    return json.dumps({"status": status, "summary": summary, "artifacts": [], "evidence": []})


RETRY = json.dumps({"action": "retry", "instructions": "Try another way.", "worker_type": "default", "question": "",
                    "stop_reason": "", "result": None})


class Turn:
    """One chat turn streamed in the background, so the test can act while the task runs."""

    def __init__(self, d: RunningDaemon, text: str, session: str = "s1") -> None:
        self.events: list[dict[str, Any]] = []
        self.error: BaseException | None = None
        self.thread = threading.Thread(target=self._run, args=(d, text, session), daemon=True)
        self.thread.start()

    def _run(self, d: RunningDaemon, text: str, session: str) -> None:
        try:
            self.events = chat(d, text, session, timeout=60)
        except BaseException as e:            # surfaced by result()
            self.error = e

    def result(self, timeout: float = 30) -> dict[str, Any]:
        self.thread.join(timeout)
        assert not self.thread.is_alive(), "the turn didn't end"
        if self.error is not None:
            raise self.error
        return outcome(self.events)


def _until(d: RunningDaemon, check: Any, timeout: float = 20) -> None:
    """Wait until `check()` holds; it reads the daemon through `run` itself (never nested inside one)."""
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if check():
            return
        time.sleep(0.02)
    raise AssertionError("the condition never held")


def _row(d: RunningDaemon, task_id: str) -> str | None:
    found = run(d, lambda: d.daemon.db.conn.execute("SELECT status FROM tasks WHERE id = ?", (task_id,)).fetchone())
    return found[0] if found else None


def _tree(d: RunningDaemon) -> str:
    _until(d, lambda: bool(run(d, d.daemon.supervisor.running_trees)))
    return run(d, d.daemon.supervisor.running_trees)[0][0]


def _published(d: RunningDaemon) -> list[tuple[str, dict[str, Any]]]:
    seen: list[tuple[str, dict[str, Any]]] = []
    sv = d.daemon.supervisor
    original = sv.publish
    sv.publish = lambda topic, data: (seen.append((topic, data)), original(topic, data))[1]
    return seen


def test_a_plain_task_is_a_one_step_plan_and_finishes_once(daemon: RunningDaemon) -> None:
    published = _published(daemon)
    script = install(daemon, [sup("task", "On it.", task("Say hello"))], {"worker": done("Said hello.")})
    result = outcome(chat(daemon, "say hello"))
    tree = result["task_id"]
    assert result["status"] == "done" and result["verified"]
    steps = run(daemon, lambda: daemon.daemon.supervisor.plans.steps(tree))
    assert list(steps) == ["worker"] and steps["worker"]["status"] == "done"
    assert run(daemon, lambda: daemon.daemon.supervisor.plans.load(tree)["status"]) == "done"
    assert [d["task_id"] for t, d in published if t == "task.finished"] == [tree]     # once per tree
    assert _row(daemon, tree) == "done" and _row(daemon, tree + ".worker") == "done"
    assert script.calls("worker") == ["worker"] and script.calls("checker") == ["worker.verify"]


def test_stop_mid_task_ends_every_row_as_stopped(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "On it.", task("Wait for the build"))], {"worker": STALL})
    turn = Turn(daemon, "wait for the build")
    tree = _tree(daemon)
    _until(daemon, lambda: _row(daemon, tree + ".worker") == "running")
    run(daemon, lambda: daemon.daemon.supervisor.stop_tree(tree))
    result = turn.result()
    assert result["status"] == "cancelled" and result["answer"].startswith("Stopped, as you asked.")
    assert _row(daemon, tree) == "cancelled" and _row(daemon, tree + ".worker") == "cancelled"
    assert run(daemon, lambda: daemon.daemon.supervisor.plans.steps(tree))["worker"]["status"] == "cancelled"


def test_pause_keeps_everything_in_place_and_the_owners_reply_resumes_it(daemon: RunningDaemon) -> None:
    """Owner decision 8: pause ends the current attempt and marks the task paused, not cancelled: nothing is released
    (the browser keeps the task's tab group: `task_paused`), no answer is given, and the owner's reply (the planner's
    continue) resumes it in place."""
    released: list[str] = []
    sv = daemon.daemon.supervisor
    original_end = sv.on_task_end

    async def end(task_id: str) -> Any:
        released.append(task_id)
        return await original_end(task_id)
    sv.on_task_end = end
    published = _published(daemon)
    turn_script = install(daemon, [sup("task", "On it.", task("Tidy the notes", paths=("notes.md",)))],
                          {"worker": [STALL, writes(daemon.sock, "notes.md", "# Notes")]})   # resumed: its tools work
    turn = Turn(daemon, "tidy my notes")
    tree = _tree(daemon)
    _until(daemon, lambda: _row(daemon, tree + ".worker") == "running")
    pause = json.dumps({"kind": "reply", "text": "Paused; tell me how you'd like it.", "task": None,
                        "control": {"task_id": tree, "action": "pause", "standing_sends": "unchanged",
                                    "supersedes": []}})
    turn_script.sup.append(pause)
    said = [e for e in chat(daemon, "wait, hold on") if e["type"] == "reply"]
    assert said and "Paused" in said[0]["text"]
    _until(daemon, lambda: _row(daemon, tree) == "paused" and _row(daemon, tree + ".worker") == "paused")
    assert run(daemon, lambda: sv.task_paused(tree + ".worker")) and turn.thread.is_alive()
    assert released == [] and not [t for t, _ in published if t == "task.finished"]
    assert ("task.paused", {"task_id": tree}) in published
    resume = json.dumps({"kind": "reply", "text": "Carrying on.", "task": None,
                         "control": {"task_id": tree, "action": "continue", "standing_sends": "unchanged",
                                     "supersedes": []}})
    turn_script.sup.append(resume)
    chat(daemon, "ok, carry on but keep the headings")
    _until(daemon, lambda: ("task.resumed", {"task_id": tree}) in published)
    result = turn.result()
    assert result["status"] == "done" and result["verified"]
    assert turn_script.calls("worker") == ["worker", "worker"]          # the paused attempt, then the resumed one
    resumed = [c for _, k, c, _ in turn_script.contexts if k == "worker"][1]
    assert "keep the headings" in resumed


def test_a_plan_step_that_needs_the_owner_asks_once_and_carries_on_with_the_answer(daemon: RunningDaemon) -> None:
    """The endless relaunch (notes/orchestration.md): a plan step that needed the owner was relaunched forever and the
    owner never asked. Now it pauses in place, asks once, and the answer carries it on."""
    spec = task("Sort the downloads then report")
    spec["plan"] = Plan(steps=[Step(id="sort", kind="do", goal="Sort the downloads"),
                               Step(id="report", kind="do", goal="Report", depends_on=["sort"])]).model_dump()
    script = install(daemon, [sup("task", "On it.", spec)],
                     {"step.sort": [_report("needs_user", "Which folder should I sort?"), done("Sorted Downloads.")],
                      "step.report": done("Reported."), "worker": done("All done.")})
    turn = Turn(daemon, "sort my downloads and tell me")
    asked = answer_question(daemon, text="the Downloads folder")
    tree = asked.task_id.split(".", 1)[0]
    assert "Which folder should I sort?" in asked.reason
    result = turn.result()
    assert result["status"] == "done"
    assert script.calls("worker").count("step.sort") == 2
    second = [c for _, k, c, _ in script.contexts if k == "step.sort"][1]
    assert "the Downloads folder" in second
    asked_twice = run(daemon, lambda: [e for e in daemon.daemon.journal.entries_of("plan.question")])
    assert len(asked_twice) == 1 and asked_twice[0].data["task_id"] == tree


def _failing(daemon: RunningDaemon, answers: list[str | None]) -> tuple[Any, Turn]:
    script = install(daemon, [sup("task", "On it.", task("Book the table"))],
                     {"worker": _report("failed", "The site errored."), "worker.dispatch": RETRY})
    turn = Turn(daemon, "book the table")
    for choice in answers:
        asked = answer_question(daemon, choice=choice)
        assert asked.options == ("Yes", "No") and "Keep going?" in asked.reason
    return script, turn


def test_after_five_failed_attempts_ferb_asks_and_no_stops_with_what_it_found(daemon: RunningDaemon) -> None:
    script, turn = _failing(daemon, ["No"])
    result = turn.result()
    assert len(script.calls("worker")) == CHECK_IN_AFTER == 5
    assert result["status"] == "failed" and "You asked me to stop after 5 attempts." in result["answer"]


def test_yes_at_the_check_in_allows_five_more_attempts(daemon: RunningDaemon) -> None:
    script, turn = _failing(daemon, ["Yes", "No"])
    turn.result()
    assert len(script.calls("worker")) == 10


def _scheduled(daemon: RunningDaemon, workers: dict[str, Any]) -> dict[str, Any]:
    first = daemon.daemon.clock.now() + timedelta(minutes=5)
    sched = make_schedule(daemon, "check the visa page", "check the visa page every day", Targets.of(), first)
    install(daemon, [sup("task", "Checking.", task("Check the visa page"))], workers)
    return daemon.run(daemon.daemon.supervisor.run_scheduled(sched, late=False), timeout=60)


def test_a_scheduled_run_declines_a_question_at_once(daemon: RunningDaemon) -> None:
    """Nobody attends a scheduled run: needing the owner ends it at once, reported, never a wait."""
    final = _scheduled(daemon, {"worker": _report("needs_user", "Which account?")})
    assert final["status"] == "needs_user" and "Which account?" in final["answer"]
    assert not daemon.daemon.state.approvals.pending()


def test_a_scheduled_run_stops_at_five_failed_attempts_and_reports(daemon: RunningDaemon) -> None:
    final = _scheduled(daemon, {"worker": _report("failed", "The page didn't load."), "worker.dispatch": RETRY})
    assert final["status"] == "failed" and "nobody was there to ask" in final["answer"]
    assert not daemon.daemon.state.approvals.pending()


def test_a_loop_survives_a_restart_and_reruns_the_round_cut_short(home: Any, monkeypatch: Any) -> None:
    """L31 with owner decision 8: FERB's shutdown suspends a loop mid-round; after the restart the tree resumes by
    itself (no "pick it up again?" result), the round cut short runs again, and the loop's saved state carries on."""
    monkeypatch.setattr("ferb.sanitize.images.recognize_text", lambda _image: [])
    spec = task("Keep the tab playing")
    spec["plan"] = Plan(steps=[Step(id="keep", kind="do", goal="Keep the tab playing",
                                    repeat={"end_when": ""})]).model_dump()
    first = own_daemon(home, "a")
    try:
        install(first, [sup("task", "Keeping it playing.", spec)], {"loop.keep": STALL})
        turn = Turn(first, "keep this tab playing until I say stop")
        tree = _tree(first)
        sv = first.daemon.supervisor
        _until(first, lambda: run(first, lambda: sv._loop_status.get((tree, "keep"), {})).get("state") == "acting")
    finally:
        first.stop()
    turn.thread.join(10)
    import ferb.orchestrator.supervisor as supervisor_module
    resume = supervisor_module.Supervisor.resume_plans

    async def held(self: Any) -> None:                   # resumed below, once the scripted model is in place
        return None
    monkeypatch.setattr(supervisor_module.Supervisor, "resume_plans", held)
    second = own_daemon(home, "b")
    try:
        monkeypatch.setattr(supervisor_module.Supervisor, "resume_plans", resume)
        sv = second.daemon.supervisor
        assert run(second, lambda: sv.plans.pending()) == [tree]
        assert run(second, lambda: second.daemon.db.conn.execute(
            "SELECT state FROM loop_state WHERE task_id = ?", (tree,)).fetchone()[0]) == "acting"
        assert run(second, lambda: second.daemon.db.conn.execute(
            "SELECT COUNT(*) FROM task_results WHERE task_id = ?", (tree,)).fetchone()[0]) == 0
        round_two = json.dumps({"status": "done", "summary": "Playing.", "artifacts": [], "evidence": ["playing"],
                                "answer": "Playing.", "loop_action": "continue"})
        script = install(second, [], {"loop.keep": [round_two]})
        second.loop.call_soon_threadsafe(lambda: second.loop.create_task(sv.resume_plans()))
        _until(second, lambda: run(second, lambda: sv.loops.digest(tree, "keep"))["rounds"] >= 1)
        assert len(script.calls("worker")) == 1         # the round cut short ran again, once
        assert run(second, lambda: sv.loops.digest(tree, "keep"))["recent"] == ["done: Playing."]
        run(second, lambda: sv.stop_tree(tree))
        _until(second, lambda: run(second, lambda: tree not in sv._tree_of))
    finally:
        second.stop()


def test_closing_a_tab_the_task_holds_asks_whether_to_continue(daemon: RunningDaemon) -> None:
    """Hands decision 3: the owner closing a FERB tab or the task's group mid-task stops that part, and FERB asks whether
    to continue (Continue / Stop). Continue carries the task on in place."""
    script = install(daemon, [sup("task", "On it.", task("Compare the prices"))], {"worker": [STALL, done("Compared.")]})
    turn = Turn(daemon, "compare the prices")
    tree = _tree(daemon)
    _until(daemon, lambda: _row(daemon, tree + ".worker") == "running")
    run(daemon, lambda: daemon.daemon._publish("browser.owner_stopped", {
        "task_id": tree + ".worker", "tab": "t1", "profile": "Default", "borrowed": False, "part": "group"}))
    asked = answer_question(daemon, choice="Continue")
    assert asked.options == ("Continue", "Stop") and "tab group" in asked.reason
    result = turn.result()
    assert result["status"] == "done" and len(script.calls("worker")) == 2


def test_a_tab_the_owner_took_is_theirs_and_the_task_runs_on(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "On it.", task("Read the article"))], {"worker": STALL})
    turn = Turn(daemon, "read the article")
    tree = _tree(daemon)
    _until(daemon, lambda: _row(daemon, tree + ".worker") == "running")
    run(daemon, lambda: daemon.daemon._publish("browser.owner_took", {
        "task_id": tree + ".worker", "tab": "t1", "profile": "Default"}))
    time.sleep(0.3)
    assert _row(daemon, tree) == "running" and not daemon.daemon.state.approvals.pending()
    run(daemon, lambda: daemon.daemon.supervisor.stop_tree(tree))
    assert turn.result()["status"] == "cancelled"


def _shut_down(d: RunningDaemon) -> None:
    """FERB's clean shutdown as it reaches the supervisor (before it cancels the turns), then the restart's flag."""
    sv = d.daemon.supervisor

    def go() -> None:
        sv.suspend_plans()
        sv.cancel_all()
    d.call_soon(go)


def test_a_clean_shutdown_runs_no_task_end_clean_up_for_a_tree_that_resumes(daemon: RunningDaemon) -> None:
    """The change records startup keeps for a resuming tree were deleted by the task-end clean-up a shutdown ran."""
    sv = daemon.daemon.supervisor
    finished: list[str] = []

    async def finish(tree_id: str) -> None:
        finished.append(tree_id)
    sv.on_task_finish = finish
    install(daemon, [sup("task", "On it.", task("Wait for the build"))], {"worker": STALL})
    turn = Turn(daemon, "wait for the build")
    tree = _tree(daemon)
    _until(daemon, lambda: _row(daemon, tree + ".worker") == "running")
    _shut_down(daemon)
    _until(daemon, lambda: run(daemon, lambda: tree not in sv._tree_of))
    turn.thread.join(10)
    assert finished == [] and run(daemon, sv.plans.pending) == [tree]


def test_a_resumed_scheduled_run_stays_unattended_and_reports_to_its_schedule(daemon: RunningDaemon) -> None:
    """`approvals.unattended` lives in memory: a scheduled run resumed after a restart waited for the owner forever, and
    its result skipped the schedule's bookkeeping (its failure count, retry and watch)."""
    sv = daemon.daemon.supervisor
    first = daemon.daemon.clock.now() + timedelta(minutes=5)
    sched = make_schedule(daemon, "check the visa page", "check the visa page every day", Targets.of(), first)
    install(daemon, [sup("task", "Checking.", task("Check the visa page"))],
            {"worker": [STALL, _report("needs_user", "Which account?")]})
    daemon.loop.call_soon_threadsafe(lambda: daemon.loop.create_task(sv.run_scheduled(sched, late=False)))
    tree = _tree(daemon)
    _until(daemon, lambda: _row(daemon, tree + ".worker") == "running")
    _shut_down(daemon)
    _until(daemon, lambda: run(daemon, lambda: tree not in sv._tree_of))
    saved = run(daemon, lambda: sv.plans.load(tree))["request"]
    assert saved["unattended"] is True and saved["schedule"]["id"] == sched.id
    sv._suspending = False                                           # the restart
    daemon.run(sv.resume_plans(), timeout=30)
    assert not daemon.daemon.state.approvals.pending()               # declined at once, never a wait
    assert run(daemon, lambda: daemon.daemon.schedules.get(sched.id)).failures == 1
