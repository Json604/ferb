"""Owner live use 2026-10-18 23:42 (task 891b3eaf): asked "are the agents working?", FERB said "Yes — they're working in
parallel across the Scaler tabs right now" while the only worker had already reported blocked. The planner's turn now
carries FERB's own record of each task (state, the last worker report's first line, plan steps), once (step 8c), and its
prompt says to answer progress only from it."""
import json
from pathlib import Path
from typing import Any

import pytest

from ferb.core.types import Identity, Task
from ferb.orchestrator.plans import Plan, Step
from ferb.orchestrator.schemas import TaskSpec
from ferb.orchestrator.supervisor import ChatSession, _Tree
from tests.conftest import RunningDaemon, chat, install, sup

PROMPTS = Path(__file__).resolve().parents[1] / "ferb" / "orchestrator" / "prompts"


def _tree(tree_id: str, goal: str, reports: list[dict[str, Any]], plan: Plan | None = None) -> _Tree:
    worker = Task(f"{tree_id}.worker", tree_id, goal, "device", Identity.OWNER, set(), "worker", "req-" + tree_id, [],
                  None)
    tree = _Tree(tree_id, TaskSpec(goal=goal, identity="owner", plan=plan), worker)
    tree.reports = reports
    return tree


def _facts(supervisor: Any, session: ChatSession) -> dict[str, Any]:
    """The task block's facts (`Supervisor._task_state`), as the planner reads them."""
    block = supervisor._task_state(session)[0]
    if block is None:
        return {"running": [], "ended_in_this_chat": []}
    text = block[0].text
    found, _ = json.JSONDecoder().raw_decode(text[text.index("{"):])
    return found


def test_a_progress_question_sees_that_the_task_is_blocked(daemon: RunningDaemon) -> None:
    supervisor = daemon.daemon.supervisor
    assert supervisor is not None
    blocked = _tree("tree-scaler", "Analyze the 15 Scaler assignments",
                    [{"status": "blocked", "summary": "Only one tab can be driven at a time; I couldn't start the "
                      "other agents.\nDetails follow."}])

    async def add() -> None:
        supervisor._tree_of[blocked.id] = blocked
    daemon.run(add())
    script = install(daemon, [sup("reply", "It's blocked: only one tab can be driven at a time.")])
    try:
        chat(daemon, "are the agents working?")
    finally:
        async def remove() -> None:
            supervisor._tree_of.pop(blocked.id, None)
        daemon.run(remove())
    context = [c for _, key, c, _ in script.contexts if key == "supervisor"][-1]
    status = context[context.index("task status"):]
    assert "tree-scaler" in status and '"state": "blocked"' in status
    assert "Only one tab can be driven at a time" in status and "Details follow" not in status   # its first line


def test_the_planner_answers_progress_only_from_the_status_facts() -> None:
    words = " ".join((PROMPTS / "supervisor.md").read_text().split())
    assert ("Answer a question about progress (\"are the agents working?\", \"how far is it?\") only from the task "
            "status facts: say a task is blocked, waiting or finished when they say so, and never claim work is going "
            "on that they don't show.") in words


def test_a_blocked_plan_step_shows_in_the_tasks_state(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    """Re-review 2 (R4a): in a plan run the worker reports are per step; a blocked step made the task "running" with
    the block visible only in the step counts."""
    supervisor = daemon.daemon.supervisor
    assert supervisor is not None
    plan = Plan(steps=[Step(id="open", kind="do", goal="Open the tabs"), Step(id="read", kind="do", goal="Read them")])
    tree = _tree("tree-plan", "Analyze the 15 Scaler assignments", [{"status": "done", "summary": "Opened 15 tabs"}],
                 plan)
    monkeypatch.setattr(supervisor.plans, "steps", lambda key: {"open": {"status": "done", "output": {}},
                                                                "read": {"status": "blocked", "output": {}}})

    async def facts() -> list[dict[str, Any]]:
        supervisor._tree_of[tree.id] = tree
        try:
            return _facts(supervisor, ChatSession("chat-plan"))["running"]
        finally:
            supervisor._tree_of.pop(tree.id, None)
    row = daemon.run(facts())[0]
    assert row["state"] == "blocked"
    assert row["plan_steps"] == [{"id": "open", "goal": "Open the tabs", "status": "done"},
                                 {"id": "read", "goal": "Read them", "status": "blocked"}]


def test_ended_tasks_are_this_chats_newest_first(daemon: RunningDaemon) -> None:
    """Step 8c: one task block. This chat's ended tasks come newest first; another chat's ended tasks are not this
    chat's to show (its running ones would be, with the chat they belong to)."""
    d = daemon.daemon
    supervisor = d.supervisor
    assert supervisor is not None

    def add(task_id: str, conversation: str) -> None:
        d.db.conn.execute("INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, "
                          "outcome_json, created_at, updated_at) VALUES (?, ?, 'r', ?, 'owner', 'root', '[]', 'done', "
                          "'{\"summary\": \"Done.\"}', ?, ?)", (task_id, task_id, "goal " + task_id,
                                                                  "2026-10-20T09:00:00+05:30", "2026-10-20T09:00:00+05:30"))
        d.db.conn.execute("INSERT OR IGNORE INTO conversations(id) VALUES (?)", (conversation,))
        d.db.conn.execute("INSERT INTO task_context (task_id, conversation_id, spec_json, owner_words) "
                          "VALUES (?, ?, '{}', '')", (task_id, conversation))

    async def facts() -> list[str]:
        add("old-task", "chat-r4")
        add("other-chat", "chat-elsewhere")
        add("new-task", "chat-r4")
        return [row["task_id"] for row in _facts(supervisor, ChatSession("chat-r4"))["ended_in_this_chat"]]
    assert daemon.run(facts()) == ["new-task", "old-task"]


def test_a_running_loops_state_and_its_pause_question_are_in_the_tasks_row(daemon: RunningDaemon,
                                                                           monkeypatch: pytest.MonkeyPatch) -> None:
    """L31: a loop's rounds are not the tree's worker reports, so its row said "running" while the loop waited for the
    owner. The row now names each loop's state, and a paused loop's question (the round's own, as journaled), so the
    owner's reply to it routes as a steer of that task."""
    d = daemon.daemon
    supervisor = d.supervisor
    assert supervisor is not None
    plan = Plan(steps=[Step(id="keep", kind="loop", goal="Keep the inbox at zero"),
                       Step(id="tab", kind="loop", goal="Keep the tab playing")])
    tree = _tree("tree-loop", "Keep the inbox at zero and the tab playing", [], plan)
    monkeypatch.setattr(supervisor.plans, "steps", lambda key: {"keep": {"status": "running", "output": {}},
                                                                "tab": {"status": "running", "output": {}}})

    def status(step: str, goal: str, state: str) -> dict[str, Any]:
        return {"task_id": tree.id, "step": step, "goal": goal, "state": state, "rounds": 2, "last_round_end": 1.0,
                "gap_seconds": 120.0 if state == "backing_off" else 0.0}

    async def facts() -> list[dict[str, Any]]:
        supervisor._tree_of[tree.id] = tree
        d.journal.append("loop.round", {"task_id": tree.id, "step": "keep", "number": 1, "status": "done",
                                        "summary": "Archived 3 mails.", "question": ""})
        d.journal.append("loop.round", {"task_id": tree.id, "step": "keep", "number": 2, "status": "needs_user",
                                        "summary": "Two inboxes are signed in.",
                                        "question": "Which account should I keep at zero?"})
        supervisor._loop_status[(tree.id, "keep")] = status("keep", "Keep the inbox at zero", "paused")
        supervisor._loop_status[(tree.id, "tab")] = status("tab", "Keep the tab playing", "backing_off")
        try:
            return _facts(supervisor, ChatSession("chat-loop"))["running"]
        finally:
            supervisor._tree_of.pop(tree.id, None)
            supervisor._loop_status.clear()
    row = daemon.run(facts())[0]
    assert row["state"] == "paused"
    assert row["loops"] == [
        {"goal": "Keep the inbox at zero", "state": "paused", "rounds": 2,
         "question": "Which account should I keep at zero?"},
        {"goal": "Keep the tab playing", "state": "backing off", "rounds": 2}]


def test_the_planner_routes_a_reply_to_a_paused_loop_as_a_steer() -> None:
    words = " ".join((PROMPTS / "supervisor.md").read_text().split())
    assert ("The owner's reply to a paused loop's question (its task's row in the task status facts) is a correction "
            "of that task (action continue).") in words
