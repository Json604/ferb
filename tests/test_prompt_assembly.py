"""Step 10b (notes/prompts.md target design 2-4): how code assembles each role's instructions. Role blocks reach a
worker only when its run needs them; no role gets a second copy of the tool catalog in its context; the tool-less
planner gets no catalogs; the effect/destination contract is in the worker's instructions once, not on every tool."""
import json
from typing import Any

from ferb.core.prompts import load_prompt
from ferb.daemon.gateway import EFFECT_HELP
from ferb.orchestrator.plans import Plan, Step
from tests.conftest import chat, done, install, outcome, sup, task

STEP_BLOCK = load_prompt("orchestrator/prompts/roles/step.md")
LOOP_BLOCK = load_prompt("orchestrator/prompts/roles/loop.md")


def _systems(script: Any, key: str) -> list[str]:
    return [req.system for _, k, req in script.requests if k == key]


def test_a_plain_task_gets_no_role_block_and_a_plan_step_gets_its_own(daemon: Any) -> None:
    script = install(daemon, [sup("task", "On it.", task("Say hello"))], {"worker": done("Said hello.")})
    assert outcome(chat(daemon, "say hello"))["status"] == "done"
    [plain] = _systems(script, "worker")
    assert STEP_BLOCK not in plain and LOOP_BLOCK not in plain

    spec = task("Sort the downloads then report")
    spec["plan"] = Plan(steps=[Step(id="sort", kind="do", goal="Sort the downloads"),
                               Step(id="report", kind="do", goal="Report", depends_on=["sort"])]).model_dump()
    script = install(daemon, [sup("task", "On it.", spec)],
                     {"step.sort": done("Sorted."), "step.report": done("Reported.")})
    assert outcome(chat(daemon, "sort my downloads and tell me"))["status"] == "done"
    for key in ("step.sort", "step.report"):
        [system] = _systems(script, key)
        assert STEP_BLOCK in system and LOOP_BLOCK not in system


def test_no_role_gets_the_tool_catalog_in_its_context_and_the_planner_no_catalogs(daemon: Any) -> None:
    d = daemon.daemon
    script = install(daemon, [sup("task", "On it.", task("Say hello"))], {"worker": done("Said hello.")})
    assert outcome(chat(daemon, "say hello"))["status"] == "done"
    descriptions = [cap.description for cap in d.gateway.caps.values() if len(cap.description) > 80]
    assert descriptions
    for _, key, req in script.requests:
        assert not any(text in req.context for text in descriptions), key
    [planner] = _systems(script, "supervisor")
    abilities = daemon.run(_value(lambda: json.dumps(d.supervisor.abilities_catalog())))
    assert abilities not in planner and abilities in _systems(script, "worker")[0]


def test_the_effect_contract_is_in_the_worker_instructions_once_not_on_each_tool(daemon: Any) -> None:
    d = daemon.daemon
    script = install(daemon, [sup("task", "On it.", task("Say hello", identity="owner"))],
                     {"worker": done("Said hello.")})
    assert outcome(chat(daemon, "say hello"))["status"] == "done"
    [system] = _systems(script, "worker")
    assert system.count(EFFECT_HELP) == 1

    def tools() -> list[dict[str, Any]]:
        sv = d.supervisor
        scopes = frozenset(set().union(*(c.scopes for c in d.gateway.caps.values())))
        from ferb.core.types import Identity
        return d.gateway.tools_for(sv.tokens.validate(sv.tokens.issue("t-x", "w-x", scopes, Identity.OWNER,
                                                                       ttl_s=60)))
    offered = daemon.run(_value(tools))
    acting = [t for t in offered if "effect" in t["input_schema"]["properties"]]
    assert acting and EFFECT_HELP not in json.dumps(offered)


async def _value(fn: Any) -> Any:
    return fn()
