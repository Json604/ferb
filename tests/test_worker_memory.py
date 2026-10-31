"""L36 §4: every worker and plan step gets the most relevant memories (within the worker's limits, step 9b) for the owner's request, the task goal, (for
a step) its parent's goal and the task's targets, and a step sees its parent task's goal above its own.

Live 2026-10-24: the visa task's research step got no memory (the daemon's recall was a stub and plan steps blanked
it) and its step goal replaced the task goal; it typed the owner's email as the username."""
import json
from typing import Any

from ferb.memory.search import WORKER
from ferb.orchestrator.plans import Plan, Step
from tests.conftest import chat, done, install, outcome, sup, task
from tests.support.memory_corpus import USERNAME, goal_like_episodes, owner_like_store
from tests.support.memory_items import run, understood
from tests.support.visa_audit import (
    MSG_1217,
    RESEARCH_OFFICIAL,
    VISA_GOAL,
    VISA_TARGETS,
)


def filler(d: Any, n: int = 6) -> None:
    """Notes that share "the" with the goals and the username fact, so "the" carries no weight (I4): only the task's
    targets can recall the username."""
    for i in range(n):
        understood(d, f"hose_{i}", f"the garden hose note {i}", subject="project:garden")


def visa_plan(sites: tuple[str, ...]) -> dict[str, Any]:
    spec = task("Find the earliest B1/B2 appointment", sites=sites)
    spec["plan"] = Plan(steps=[Step(id="read", kind="do", goal="Read the appointment page")]).model_dump()
    return spec


def test_a_plan_step_gets_its_parent_goal_and_the_memory_it_needs(daemon: Any) -> None:
    filler(daemon)
    item = understood(daemon, "usvisascheduling_username", "the username is Asha Rao", subject="account:usvisascheduling")
    # neither the request nor a goal names the site: only the task's targets do (Ruling 12)
    script = install(daemon, [sup("task", "On it.", visa_plan(("usvisascheduling.com",)))],
                     {"step.read": done("Read."), "worker": done("Checked.")})
    result = outcome(chat(daemon, "find me a visa slot"))
    step = next(ctx for _, key, ctx, _ in script.contexts if key == "step.read")
    assert "Parent task goal: Find the earliest B1/B2 appointment" in step
    assert step.index("Parent task goal:") < step.index("Task: Read the appointment page")
    assert "the username is Asha Rao" in step and result["task_id"] and item.id


def test_the_live_research_step_gets_the_username_in_a_busy_store(daemon: Any) -> None:
    """The owner's live case end to end, on the audit's real texts (Ruling 25) in a store shaped like the owner's, with
    long past tasks that share the visa goal's words (fix round): the research_official step of the visa task, started
    by the 12:17 message, gets the username among its 20 through its task's targets ranked alone (`focus`)."""
    run(daemon, lambda: (owner_like_store(daemon.daemon.learning), goal_like_episodes(daemon.daemon.learning)))
    understood(daemon, "usvisascheduling_username", USERNAME, subject="account:usvisascheduling")
    spec = task(VISA_GOAL, apps=tuple(VISA_TARGETS.apps), sites=tuple(VISA_TARGETS.sites))
    spec["plan"] = Plan(steps=[Step(id="research_official", kind="do", goal=RESEARCH_OFFICIAL)]).model_dump()
    script = install(daemon, [sup("task", "On it.", spec)],
                     {"step.research_official": done("Collected."), "worker": done("Reported.")})
    outcome(chat(daemon, MSG_1217))
    step = next(ctx for _, key, ctx, _ in script.contexts if key == "step.research_official")
    assert 1 <= step.count("Layer 4 — retrieved memory (data):") <= WORKER.memory_items + 1   # within its limit
    assert USERNAME in step and f"Parent task goal: {VISA_GOAL}" in step


def test_without_the_site_among_its_targets_the_step_does_not_recall_it(daemon: Any) -> None:
    """The negative control (I4): the same task without its site; nothing else in its query recalls the username."""
    filler(daemon)
    understood(daemon, "usvisascheduling_username", "the username is Asha Rao", subject="account:usvisascheduling")
    script = install(daemon, [sup("task", "On it.", visa_plan(()))],
                     {"step.read": done("Read."), "worker": done("Checked.")})
    outcome(chat(daemon, "find me a visa slot"))
    step = next(ctx for _, key, ctx, _ in script.contexts if key == "step.read")
    assert "Parent task goal: Find the earliest B1/B2 appointment" in step
    assert "the username is Asha Rao" not in step


def test_a_worker_gets_ranked_memory_for_its_request_and_goal(daemon: Any) -> None:
    filler(daemon)
    understood(daemon, "usvisascheduling_username", "the username is Asha Rao", subject="account:usvisascheduling")
    script = install(daemon, [sup("task", "On it.", task("Log in to the visa site", sites=("usvisascheduling.com",)))],
                     {"worker": done("Done.")})
    chat(daemon, "log in to the visa site")
    worker = next(ctx for _, key, ctx, _ in script.contexts if key == "worker")
    assert "the username is Asha Rao" in worker and "Parent task goal" not in worker


def test_the_worker_query_is_the_request_goal_and_targets_embedded_off_the_event_loop(daemon: Any) -> None:
    """Ruling 12: the push's query is the owner's request, the goal and the task's targets, and its embedding runs in a
    worker thread, never on the event loop (L28)."""
    import threading
    queries: list[tuple[str, str]] = []

    class Embedder:
        name = "probe-model"

        def encode(self, text: str, *, query: bool = False) -> list[int]:
            if query:
                queries.append((text, threading.current_thread().name))
            return [1, 2, 3]
    understood(daemon, "usvisascheduling_username", "the username is Asha Rao", subject="account:usvisascheduling")
    daemon.daemon.learning.embedder = Embedder()
    install(daemon, [sup("task", "On it.", task("Log in to the visa site", sites=("usvisascheduling.com",)))],
            {"worker": done("Done.")})
    chat(daemon, "log in now")
    worker = [(text, name) for text, name in queries if "usvisascheduling" in text]
    assert [text for text, _ in worker] == ["log in now\nLog in to the visa site\nusvisascheduling com"]
    assert daemon.thread.name not in {name for _, name in queries}


def test_the_worker_search_carries_its_targets_and_attention_and_memory_read_carry_none(daemon: Any) -> None:
    """Fix round: the worker's and plan step's focus is the task's targets; attention and the memory tools search a
    query alone."""
    calls: list[tuple[str, str]] = []
    learning = daemon.daemon.learning
    original = learning.search

    def spy(query: str, **kw: Any) -> Any:
        calls.append((query, kw.get("focus", "")))
        return original(query, **kw)
    learning.search = spy
    install(daemon, [sup("task", "On it.", task("Log in to the visa site", sites=("usvisascheduling.com",)))],
            {"worker": done("Done.")})
    chat(daemon, "log in now")
    assert ("log in now\nLog in to the visa site\nusvisascheduling com", "usvisascheduling com") in calls
    calls.clear()
    install(daemon, [json.dumps({"tell": False, "text": "", "source_refs": [], "quotes": []})])
    daemon.run(daemon.daemon.supervisor.attention(["trail:x"], [{"data": {"title": "usvisascheduling.com login"}}], []))
    daemon.run(daemon.daemon._memory_item_search("usvisascheduling username"))
    assert calls and all(focus == "" for _, focus in calls), calls      # memory.read searches memory_items itself


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


def test_only_the_worker_gets_memory_not_its_checker(daemon: Any) -> None:
    """I10: the checker judges fresh observations; memory goes to worker and step runs alone."""
    filler(daemon)
    understood(daemon, "usvisascheduling_username", "the username is Asha Rao", subject="account:usvisascheduling")
    script = install(daemon, [sup("task", "On it.", task("Log in to the visa site", sites=("usvisascheduling.com",)))],
                     {"worker": done("Done.")})
    chat(daemon, "log in to the visa site")
    checker = next(ctx for _, key, ctx, _ in script.contexts if key == "worker.verify")
    assert "retrieved memory" not in checker and "the username is Asha Rao" not in checker


def test_a_loop_round_sees_its_parent_task_goal_above_its_own(daemon: Any) -> None:
    """m15: a loop step's rounds get the parent goal too (do/reduce, converse and loop members alike)."""
    spec = task("Keep my lofi playlist playing in the music tab")
    spec["plan"] = Plan(steps=[Step(id="keep", kind="loop", goal="Keep the tab playing",
                                    end_when="the tab plays")]).model_dump()
    finished = json.dumps({"status": "done", "summary": "It plays.", "artifacts": [], "evidence": ["media time moved"],
                           "answer": "It plays.", "loop_action": "finished"})
    script = install(daemon, [sup("task", "Keeping it playing.", spec)],
                     {"loop.keep": finished, "worker": done("It plays.")})
    outcome(chat(daemon, "keep my lofi playlist playing"))
    loop = next(ctx for _, key, ctx, _ in script.contexts if key == "loop.keep")
    assert "Parent task goal: Keep my lofi playlist playing in the music tab" in loop
    assert loop.index("Parent task goal:") < loop.index("Task: Keep the tab playing")
