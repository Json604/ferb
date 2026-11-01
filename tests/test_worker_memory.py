"""L36 §4: every worker and plan step gets the most relevant memories (within the worker's limits, step 9b) for the owner's request, the task goal, (for
a step) its parent's goal and the task's targets, and a step sees its parent task's goal above its own.

Live 2026-10-24: the permit task's research step got no memory (the daemon's recall was a stub and plan steps blanked
it) and its step goal replaced the task goal; it typed the owner's email as the username."""
from typing import Any

from ferb.memory.search import WORKER
from ferb.orchestrator.plans import Plan, Step
from tests.conftest import chat, done, install, outcome, sup, task
from tests.support.memory_corpus import USERNAME, goal_like_episodes, owner_like_store
from tests.support.memory_items import run, understood
from tests.support.permit_audit import (
    MSG_1217,
    RESEARCH_OFFICIAL,
    PERMIT_GOAL,
    PERMIT_TARGETS,
)


def filler(d: Any, n: int = 6) -> None:
    """Notes that share "the" with the goals and the username fact, so "the" carries no weight (I4): only the task's
    targets can recall the username."""
    for i in range(n):
        understood(d, f"hose_{i}", f"the garden hose note {i}", subject="project:garden")


def permit_plan(sites: tuple[str, ...]) -> dict[str, Any]:
    spec = task("Find the earliest permit appointment", sites=sites)
    spec["plan"] = Plan(steps=[Step(id="read", kind="do", goal="Read the appointment page")]).model_dump()
    return spec


# decision: MEM-416
def test_a_plan_step_gets_its_parent_goal_and_the_memory_it_needs(daemon: Any) -> None:
    filler(daemon)
    item = understood(daemon, "bookslots_username", "the username is Asha Rao", subject="account:bookslots")
    # neither the request nor a goal names the site: only the task's targets do (Ruling 12)
    script = install(daemon, [sup("task", "On it.", permit_plan(("bookslots.com",)))],
                     {"step.read": done("Read."), "worker": done("Checked.")})
    result = outcome(chat(daemon, "find me a permit slot"))
    step = next(ctx for _, key, ctx, _ in script.contexts if key == "step.read")
    assert "Parent task goal: Find the earliest permit appointment" in step
    assert step.index("Parent task goal:") < step.index("Task: Read the appointment page")
    assert "the username is Asha Rao" in step and result["task_id"] and item.id


# decision: MEM-416
def test_the_live_research_step_gets_the_username_in_a_busy_store(daemon: Any) -> None:
    """The owner's live case end to end, on the audit's real texts (Ruling 25) in a store shaped like the owner's, with
    long past tasks that share the permit goal's words (fix round): the research_official step of the permit task, started
    by the 12:17 message, gets the username among its 20 through its task's targets ranked alone (`focus`)."""
    run(daemon, lambda: (owner_like_store(daemon.daemon.learning), goal_like_episodes(daemon.daemon.learning)))
    understood(daemon, "bookslots_username", USERNAME, subject="account:bookslots")
    spec = task(PERMIT_GOAL, apps=tuple(PERMIT_TARGETS.apps), sites=tuple(PERMIT_TARGETS.sites))
    spec["plan"] = Plan(steps=[Step(id="research_official", kind="do", goal=RESEARCH_OFFICIAL)]).model_dump()
    script = install(daemon, [sup("task", "On it.", spec)],
                     {"step.research_official": done("Collected."), "worker": done("Reported.")})
    outcome(chat(daemon, MSG_1217))
    step = next(ctx for _, key, ctx, _ in script.contexts if key == "step.research_official")
    assert 1 <= step.count("Layer 4 — retrieved memory (data):") <= WORKER.memory_items + 1   # within its limit
    assert USERNAME in step and f"Parent task goal: {PERMIT_GOAL}" in step


# decision: MEM-416
def test_without_the_site_among_its_targets_the_step_does_not_recall_it(daemon: Any) -> None:
    """The negative control (I4): the same task without its site; nothing else in its query recalls the username."""
    filler(daemon)
    understood(daemon, "bookslots_username", "the username is Asha Rao", subject="account:bookslots")
    script = install(daemon, [sup("task", "On it.", permit_plan(()))],
                     {"step.read": done("Read."), "worker": done("Checked.")})
    outcome(chat(daemon, "find me a permit slot"))
    step = next(ctx for _, key, ctx, _ in script.contexts if key == "step.read")
    assert "Parent task goal: Find the earliest permit appointment" in step
    assert "the username is Asha Rao" not in step


# decision: MEM-416
def test_a_worker_gets_ranked_memory_for_its_request_and_goal(daemon: Any) -> None:
    filler(daemon)
    understood(daemon, "bookslots_username", "the username is Asha Rao", subject="account:bookslots")
    script = install(daemon, [sup("task", "On it.", task("Log in to the permit site", sites=("bookslots.com",)))],
                     {"worker": done("Done.")})
    chat(daemon, "log in to the permit site")
    worker = next(ctx for _, key, ctx, _ in script.contexts if key == "worker")
    assert "the username is Asha Rao" in worker and "Parent task goal" not in worker


# decision: MEM-27
def test_a_worker_gets_at_most_its_limit(daemon: Any) -> None:
    for i in range(30):
        understood(daemon, f"tomato_{i}", f"tomato tip {i}: water them in the morning", subject="project:garden")
    for i in range(40):
        understood(daemon, f"bicycle_{i}", f"bicycle note {i}: oil the chain", subject="project:garage")
    script = install(daemon, [sup("task", "On it.", task("Water the tomato plants"))], {"worker": done("Done.")})
    chat(daemon, "water my tomato plants")
    worker = next(ctx for _, key, ctx, _ in script.contexts if key == "worker")
    assert worker.count("Layer 4 — retrieved memory (data):") == WORKER.memory_items + 1    # and the counted rest
    assert "more matching memories left out" in worker
