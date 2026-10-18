"""Owner live use 2026-10-18 23:42 (task 891b3eaf): asked "are the agents working?", FERB said "Yes — they're working in
parallel across the Scaler tabs right now" while the only worker had already reported blocked. The planner's turn now
carries FERB's own record of each task (state, the last worker report's first line, plan step counts), and its prompt
says to answer progress only from it."""
from pathlib import Path
from typing import Any

from ferb.core.types import Identity, Task
from ferb.orchestrator.schemas import TaskSpec
from ferb.orchestrator.supervisor import _Tree
from tests.conftest import RunningDaemon, chat, install, sup

PROMPTS = Path(__file__).resolve().parents[1] / "ferb" / "orchestrator" / "prompts"


def _tree(tree_id: str, goal: str, reports: list[dict[str, Any]]) -> _Tree:
    worker = Task(f"{tree_id}.worker", tree_id, goal, "device", Identity.OWNER, set(), "worker", "req-" + tree_id, [],
                  None)
    tree = _Tree(tree_id, TaskSpec(goal=goal, identity="owner"), worker)
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
