"""Owner live use 2026-10-18 23:42 (task 891b3eaf): asked "are the agents working?", FERB said "Yes — they're working in
parallel across the Scaler tabs right now" while the only worker had already reported blocked. The planner's turn now
carries FERB's own record of each task (state, the last worker report's first line, plan step counts), and its prompt
says to answer progress only from it."""
from pathlib import Path
from typing import Any

import pytest

from ferb.core.types import Identity, Task
from ferb.orchestrator.plans import Plan, Step
from ferb.orchestrator.schemas import TaskSpec
from ferb.orchestrator.supervisor import ChatSession, _Tree
from ferb.orchestrator.tree import CONVERSATION_ROOTS
from tests.conftest import RunningDaemon, chat, install, sup

PROMPTS = Path(__file__).resolve().parents[1] / "ferb" / "orchestrator" / "prompts"


def _tree(tree_id: str, goal: str, reports: list[dict[str, Any]], plan: Plan | None = None) -> _Tree:
    worker = Task(f"{tree_id}.worker", tree_id, goal, "device", Identity.OWNER, set(), "worker", "req-" + tree_id, [],
                  None)
    tree = _Tree(tree_id, TaskSpec(goal=goal, identity="owner", plan=plan), worker)
    tree.reports = reports
    return tree


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
            return supervisor._status_facts(ChatSession("chat-plan"))[0]
        finally:
            supervisor._tree_of.pop(tree.id, None)
    row = daemon.run(facts())[0]
    assert row["state"] == "blocked" and row["plan_steps"] == {"done": 1, "blocked": 1}


def test_finished_tasks_come_from_tasks_recorded_since_the_session_started(daemon: RunningDaemon) -> None:
    """Re-review 2/3 (R4b): each planner turn read every task ever recorded, then every task of the conversation (the
    default "owner" conversation holds them all). The finished ones now come from the tasks recorded since the chat
    session started: a rowid floor captured then, read by the rowid key, each matched to its conversation by key."""
    d = daemon.daemon
    supervisor = d.supervisor
    assert supervisor is not None

    async def query_plan() -> list[str]:
        return [str(tuple(r)[-1]) for r in d.db.conn.execute("EXPLAIN QUERY PLAN " + CONVERSATION_ROOTS, (0, "c"))]
    plan = daemon.run(query_plan())
    assert any("INTEGER PRIMARY KEY (rowid>?)" in step for step in plan), plan
    assert not any(step.startswith("SCAN") for step in plan), plan

    def add(task_id: str, conversation: str) -> None:
        d.db.conn.execute("INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, "
                          "outcome_json, created_at, updated_at) VALUES (?, ?, 'r', ?, 'owner', 'root', '[]', 'done', "
                          "'{\"summary\": \"Done.\"}', ?, ?)", (task_id, task_id, "goal " + task_id,
                                                                  "2026-10-20T09:00:00+05:30", "2026-10-20T09:00:00+05:30"))
        d.db.conn.execute("INSERT OR IGNORE INTO conversations(id) VALUES (?)", (conversation,))
        d.db.conn.execute("INSERT INTO task_context (task_id, conversation_id, spec_json, owner_words) "
                          "VALUES (?, ?, '{}', '')", (task_id, conversation))

    async def facts() -> tuple[list[str], list[str]]:
        add("old-task", "chat-r4")
        session = ChatSession("chat-r4", task_floor=d.tasks.max_rowid())        # the session starts here
        add("other-chat", "chat-elsewhere")
        add("new-task", "chat-r4")
        unstarted = ChatSession("chat-r4")                                        # no floor: running tasks only
        return ([row["task_id"] for row in supervisor._status_facts(session)[0]],
                [row["task_id"] for row in supervisor._status_facts(unstarted)[0]])
    assert daemon.run(facts()) == (["new-task"], [])
