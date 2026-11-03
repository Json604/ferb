"""Step 8a end-to-end runs (TEST-500): the real daemon, gateway and scripted model through the one execution
path. A single task is a one-step plan; one status, finish and stop; "needs the owner" pauses in place and asks; pause
and resume (ORCH-308); the check-in after 5 attempts (ORCH-307); a loop survives a restart; a scheduled
run declines questions and stops at 5."""
import json
import threading
import time
from datetime import timedelta
from typing import Any

import pytest

from ferb.orchestrator.attempts import CHECK_IN_AFTER
from ferb.orchestrator.plans import Plan, Step
from ferb.policy.targets import Targets
from tests.conftest import (STALL, RunningDaemon, answer_question, chat, done, install, outcome, report, sup, task,
                            verdict, writes)
from tests.support.schedules import make_schedule, own_daemon, run


def _report(status: str, summary: str) -> str:
    """A report that isn't done: its reason, and as the question too when it needs the owner."""
    return json.dumps(report("not_done", reason=summary, question=summary if status == "needs_user" else None))


RETRY = verdict(None, next="retry", missing="The table isn't booked.", instructions="Try another way.")


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


# decision: ORCH-322, ORCH-323
# decision: TEST-502
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


# decision: ORCH-323
# decision: TEST-502
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


# decision: ORCH-308
# decision: TEST-502
def test_pause_keeps_everything_in_place_and_the_owners_reply_resumes_it(daemon: RunningDaemon) -> None:
    """ORCH-308: pause ends the current attempt and marks the task paused, not cancelled: nothing is released
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


# decision: ORCH-323
def test_a_plan_step_that_needs_the_owner_asks_once_and_carries_on_with_the_answer(daemon: RunningDaemon) -> None:
    """The endless relaunch (kb area orchestration): a plan step that needed the owner was relaunched forever and the
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
                     {"worker": _report("failed", "The site errored."), "worker.verify": RETRY})
    turn = Turn(daemon, "book the table")
    for choice in answers:
        asked = answer_question(daemon, choice=choice)
        assert asked.options == ("Yes", "No") and "Keep going?" in asked.reason
    return script, turn


# decision: ORCH-307
def test_after_five_failed_attempts_ferb_asks_and_no_stops_with_what_it_found(daemon: RunningDaemon) -> None:
    script, turn = _failing(daemon, ["No"])
    result = turn.result()
    assert len(script.calls("worker")) == CHECK_IN_AFTER == 5
    assert result["status"] == "failed" and "You asked me to stop after 5 attempts." in result["answer"]


# decision: ORCH-307
def test_yes_at_the_check_in_allows_five_more_attempts(daemon: RunningDaemon) -> None:
    script, turn = _failing(daemon, ["Yes", "No"])
    turn.result()
    assert len(script.calls("worker")) == 10


def _pending(d: RunningDaemon, capability: str = "plan.ask") -> Any:
    """FERB's open question of this capability, once it is asked (left unanswered)."""
    import asyncio

    async def go() -> Any:
        while True:
            for ap in d.daemon.state.approvals.pending():
                if ap.capability == capability:
                    return ap
            await asyncio.sleep(0.02)
    return d.run(go(), timeout=20)


def _routed(approval_id: str, reading: str) -> str:
    return json.dumps({"kind": "reply", "text": "", "task": None,
                       "answer_question": {"approval_id": approval_id, "answer": reading}})


# decision: ORCH-328, ORCH-301
def test_free_text_in_chat_answers_the_check_in_only_as_one_of_its_options(daemon: RunningDaemon) -> None:
    """A chat answer to an option question counted as "carry on" whatever it said ("stop it" kept going). Now words
    that aren't an option count only through the planner's reading when that is exactly an option; otherwise FERB asks
    which option it is, and the check-in stays open."""
    script, turn = _failing(daemon, [])
    asked = _pending(daemon)
    script.sup.append(_routed(asked.id, "they want it to stop"))       # not an option: FERB asks which
    told = chat(daemon, "hmm, stop it I guess")
    assert [e["type"] for e in told] == ["clarify"]
    assert run(daemon, lambda: daemon.daemon.state.approvals.get(asked.id)).state == "pending"
    script.sup.append(_routed(asked.id, "No"))                          # the planner's reading is an option
    chat(daemon, "stop it")
    result = turn.result()
    assert result["status"] == "failed" and len(script.calls("worker")) == CHECK_IN_AFTER
    ap = run(daemon, lambda: daemon.daemon.state.approvals.get(asked.id))
    assert ap is None or (ap.choice == "No" and ap.answer == "stop it")


def _scheduled(daemon: RunningDaemon, workers: dict[str, Any]) -> dict[str, Any]:
    first = daemon.daemon.clock.now() + timedelta(minutes=5)
    sched = make_schedule(daemon, "check the permit page", "check the permit page every day", Targets.of(), first)
    install(daemon, [sup("task", "Checking.", task("Check the permit page"))], workers)
    return daemon.run(daemon.daemon.supervisor.run_scheduled(sched, late=False), timeout=60)


# decision: ORCH-323
def test_a_scheduled_run_declines_a_question_at_once(daemon: RunningDaemon) -> None:
    """Nobody attends a scheduled run: needing the owner ends it at once, reported, never a wait."""
    final = _scheduled(daemon, {"worker": _report("needs_user", "Which account?")})
    assert final["status"] == "needs_user" and "Which account?" in final["answer"]
    assert not daemon.daemon.state.approvals.pending()


# decision: ORCH-307
def test_a_scheduled_run_stops_at_five_failed_attempts_and_reports(daemon: RunningDaemon) -> None:
    final = _scheduled(daemon, {"worker": _report("failed", "The page didn't load."), "worker.verify": RETRY})
    assert final["status"] == "failed" and "nobody was there to ask" in final["answer"]
    assert not daemon.daemon.state.approvals.pending()


# decision: ORCH-505, ORCH-322
# decision: TEST-502
def test_a_loop_survives_a_restart_and_reruns_the_round_cut_short(home: Any, monkeypatch: Any) -> None:
    """L31 with ORCH-308: FERB's shutdown suspends a loop mid-round; after the restart the tree resumes by
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


# decision: ORCH-307
def test_a_check_in_asked_before_a_restart_is_asked_again_and_no_stops(home: Any, monkeypatch: Any) -> None:
    """A check-in waiting when FERB restarted came back as an owner pause, whose "No" resumed the work. Now the same
    check-in is asked again, and No stops with what it found; no further attempt runs."""
    monkeypatch.setattr("ferb.sanitize.images.recognize_text", lambda _image: [])
    first = own_daemon(home, "a")
    try:
        install(first, [sup("task", "On it.", task("Book the table"))],
                {"worker": _report("failed", "The site errored."), "worker.verify": RETRY})
        turn = Turn(first, "book the table")
        asked = _pending(first)
        assert asked.options == ("Yes", "No") and "Keep going?" in asked.reason
        tree = asked.task_id.split(".", 1)[0]
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
        script = install(second, [], {"worker": done("Booked."), "worker.verify": verdict(True)})
        second.loop.call_soon_threadsafe(lambda: second.loop.create_task(sv.resume_plans()))
        again = answer_question(second, choice="No")
        assert again.options == ("Yes", "No") and "Keep going?" in again.reason
        _until(second, lambda: _row(second, tree) == "failed")
        assert script.calls("worker") == []
        assert run(second, lambda: second.daemon.journal.of_kind("task.check_in", "task_id", tree))[-1].data[
            "keep_going"] is False
    finally:
        second.stop()


def test_closing_a_tab_the_task_holds_asks_whether_to_continue(daemon: RunningDaemon) -> None:
    """HAND-201: the owner closing a FERB tab or the task's group mid-task stops that part, and FERB asks whether
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


# decision: ORCH-510
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


# decision: ORCH-323
def test_a_resumed_scheduled_run_stays_unattended_and_reports_to_its_schedule(daemon: RunningDaemon) -> None:
    """`approvals.unattended` lives in memory: a scheduled run resumed after a restart waited for the owner forever, and
    its result skipped the schedule's bookkeeping (its failure count, retry and watch)."""
    sv = daemon.daemon.supervisor
    first = daemon.daemon.clock.now() + timedelta(minutes=5)
    sched = make_schedule(daemon, "check the permit page", "check the permit page every day", Targets.of(), first)
    install(daemon, [sup("task", "Checking.", task("Check the permit page"))],
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


# ---------- ORCH-312: pause details ----------

def _pause_through_chat(daemon: RunningDaemon, script: Any, tree: str, words: str = "wait, hold on") -> None:
    script.sup.append(json.dumps({"kind": "reply", "text": "Paused.", "task": None,
                                  "control": {"task_id": tree, "action": "pause", "standing_sends": "unchanged",
                                              "supersedes": []}}))
    chat(daemon, words)
    _until(daemon, lambda: _row(daemon, tree) == "paused")


def _owner_back(daemon: RunningDaemon, after_s: float = 0) -> None:
    """The Mac comes back into use `after_s` seconds from now (locked, then unlocked; ORCH-317)."""
    from tests.support.schedules import mac_away, mac_back
    if after_s:
        daemon.call_soon(daemon.daemon.clock.advance, after_s)
    mac_away(daemon)
    mac_back(daemon)


def _mentions(daemon: RunningDaemon) -> list[Any]:
    return [ap for ap in run(daemon, daemon.daemon.state.approvals.pending) if "Resume" in ap.options]


# decision: ORCH-312, ORCH-317
def test_a_paused_task_is_mentioned_once_a_day_when_the_owner_is_back_and_resume_resumes_it(
        daemon: RunningDaemon) -> None:
    """ORCH-312: FERB mentions a paused task once a day, from its record, in its chat, with a Resume that goes the
    pause's own way; only when the owner is back at the Mac (never while away), never twice a day, and yesterday's
    unanswered mention is replaced, not stacked."""
    script = install(daemon, [sup("task", "On it.", task("Check the permit page"))],
                     {"worker": [STALL, done("Checked: no new slots.")]})
    turn = Turn(daemon, "check the permit page")
    tree = _tree(daemon)
    _until(daemon, lambda: _row(daemon, tree + ".worker") == "running")
    _pause_through_chat(daemon, script, tree)
    try:
        _owner_back(daemon)                                 # back the same day it was paused: nothing yet
        time.sleep(0.3)
        assert _mentions(daemon) == []
        _owner_back(daemon, 24 * 3600)                      # the next day, back at the Mac
        _until(daemon, lambda: len(_mentions(daemon)) == 1)
        [first] = _mentions(daemon)
        assert first.task_id.split(".", 1)[0] == tree and set(first.options) == {"Resume", "Stop"}
        _owner_back(daemon, 600)                            # back again later that day: once a day
        time.sleep(0.3)
        assert [ap.id for ap in _mentions(daemon)] == [first.id]
        _owner_back(daemon, 24 * 3600)                      # the day after, still unanswered: replaced
        _until(daemon, lambda: [ap.id for ap in _mentions(daemon)] not in ([], [first.id]))
        assert len(_mentions(daemon)) == 1 and _row(daemon, tree) == "paused"
        assert run(daemon, lambda: daemon.daemon.state.approvals.get(first.id)).state == "superseded"
        answer_question(daemon, choice="Resume")
        result = turn.result()
        assert result["status"] == "done" and script.calls("worker") == ["worker", "worker"]
        marks = run(daemon, lambda: daemon.daemon.journal.of_kind("task.pause_mentioned", "task_id", tree))
        assert len(marks) == 2
    finally:
        daemon.daemon.presence.stop()


# decision: ORCH-312
def test_pause_stops_the_tasks_listening_and_running_commands_and_keeps_the_rest(daemon: RunningDaemon,
                                                                                 tmp_path: Any) -> None:
    """ORCH-312: a paused task keeps only its tabs and progress: its listening and its commands still running stop;
    a long-lived command (made to outlive the task) and another task's are left alone."""
    import asyncio
    from ferb.act.shell.manager import ShellManager
    from tests.support.hearing import wait_started
    from tests.test_audio_listen import _hearing
    hearing, _, capture = _hearing(tmp_path)
    daemon.daemon.hearing = hearing
    shells = daemon.daemon.shells = ShellManager(tmp_path / "logs", lambda text: text)

    async def start() -> dict[str, str]:
        asyncio.get_running_loop().create_task(hearing.listen("x.worker", "com.apple.Music", 77, 3600, None))
        out = {}
        for name, owner, long_lived in (("mine", "x.worker", False), ("server", "x.s1", True),
                                        ("other", "y.worker", False)):
            out[name] = (await shells.start("sleep 30", tmp_path, owner, long_lived)).id
        return out
    procs = daemon.run(start())
    wait_started(capture)
    daemon.call_soon(daemon.daemon._publish, "task.paused", {"task_id": "x"})

    def running(name: str) -> bool:
        return next(p for p in run(daemon, shells.list) if p.id == procs[name]).running
    _until(daemon, lambda: not running("mine") and capture.handles[0].stopped)
    assert running("server") and running("other") and not hearing.listening
    stopped = run(daemon, lambda: daemon.daemon.journal.of_kind("task.pause_stopped", "task_id", "x"))
    assert stopped and {r["resource"] for r in stopped[0].data["resources"]} == {"shell", "listen"}
    daemon.run(shells.kill_all())


# decision: ORCH-312, PRM-309
@pytest.mark.parametrize("go_on_while_the_attempt_ends", [False, True])
def test_pausing_a_scheduled_run_pauses_its_schedule_and_resuming_it_resumes_the_schedule(
        daemon: RunningDaemon, go_on_while_the_attempt_ends: bool) -> None:
    """ORCH-312, ORCH-308: the owner's pause of a scheduled run pauses its schedule and their "go on" resumes both.
    Past bug (T-1005): a "go on" that came while the paused attempt's run was still ending (a real runtime's cancel
    takes a moment) was recorded but never resumed it, so the task and its schedule stayed paused for good. The
    worker's run here ends only when the test lets it, so each order is the same every time."""
    sv = daemon.daemon.supervisor
    first = daemon.daemon.clock.now() + timedelta(minutes=5)
    sched = make_schedule(daemon, "check the permit page", "check the permit page every day", Targets.of(), first)
    ending = threading.Event()

    def attempt(_token: str) -> str:            # runs on until the test lets its cancelled run end
        assert ending.wait(30)
        return _report("failed", "Interrupted.")
    script = install(daemon, [sup("task", "Checking.", task("Check the permit page"))],
                     {"worker": [attempt, done("Checked.")]})
    daemon.loop.call_soon_threadsafe(lambda: daemon.loop.create_task(sv.run_scheduled(sched, late=False)))
    tree = _tree(daemon)

    def step() -> str:
        return str(run(daemon, lambda: sv.plans.steps(tree))["worker"]["status"])
    try:
        _until(daemon, lambda: _row(daemon, tree + ".worker") == "running")
        _pause_through_chat(daemon, script, tree, "hold on, pause the permit check")
        _until(daemon, lambda: run(daemon, lambda: daemon.daemon.schedules.get(sched.id)).state == "paused")
        if not go_on_while_the_attempt_ends:
            ending.set()
            _until(daemon, lambda: step() == "paused")          # the step waits at the pause
        script.sup.append(json.dumps({"kind": "reply", "text": "Carrying on.", "task": None,
                                      "control": {"task_id": tree, "action": "continue",
                                                  "standing_sends": "unchanged", "supersedes": []}}))
        chat(daemon, "ok go on")
        if go_on_while_the_attempt_ends:
            assert step() == "running"                          # the attempt hasn't ended: no step waits yet
            ending.set()
        _until(daemon, lambda: run(daemon, lambda: daemon.daemon.schedules.get(sched.id)).state == "active")
        resumed = run(daemon, lambda: daemon.daemon.schedules.get(sched.id))
        assert resumed.next_run is not None and resumed.timer_id is not None
        _until(daemon, lambda: _row(daemon, tree) == "done")
        assert script.calls("worker") == ["worker", "worker"]
    finally:
        ending.set()
