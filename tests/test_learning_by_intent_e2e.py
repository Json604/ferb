"""L29 end to end in storage: three runs of one kind of task are labelled together, consolidated into a procedure keyed
by their intent, offered to the planner by name, and handed to the worker with its filled steps and the runs."""
import asyncio
import json
from typing import Any

import pytest

from ferb.memory import intents
from ferb.memory.consolidate import PROPOSAL_SCHEMA, Consolidator
from ferb.memory.learning import PROCEDURE_LABEL, RUN_LABEL
from tests.support.learning_fixtures import NoMemory, card, new_store
from tests.support.learning_runtime import SchemaAnswers


@pytest.fixture
def learning(tmp_path):
    db, store = new_store(tmp_path / "memory.db")
    yield store
    db.close()


def test_three_runs_become_a_procedure_the_worker_starts_from(learning) -> None:
    dishes = ("egg curry", "paneer", "dal")
    cards = [card(learning, f"r{n}", f"cheapest {d} with breads", dish=d) for n, d in enumerate(dishes)]
    iid = intents.intent_id("cheapest food order")

    def label(ctx: str) -> dict[str, Any]:                  # a new intent each time: code maps the name to one intent
        dish = next(d for d in dishes if f'"text":"{d}"' in ctx)
        return {"intent_id": "", "name": "cheapest food order", "description": "the cheapest dish with breads",
                "slots": [{"name": "dish", "value": "string"}], "values": [{"name": "dish", "value": json.dumps(dish)}]}

    def consolidate(ctx: str) -> dict[str, Any]:
        return {"episodes": [], "lessons": [], "identity": [], "procedures": [{
            "intent": "cheapest food order", "slots": [{"name": "dish", "value": "string"}],
            "preconditions": {"apps": []}, "ui_hints": "",
            "steps": [{"tool": "browser.type", "args": [{"name": "text", "value": '"{dish}"'}], "expect": "typed",
                       "optional": False, "evidence": [f"r{n}-a" for n in range(3)]}],
            "examples": [{"episode": c["id"], "slots": [{"name": "dish", "value": json.dumps(d)}]}
                         for c, d in zip(cards, dishes)]}]}

    rt = SchemaAnswers({id(intents.INTENT_SCHEMA): label, id(PROPOSAL_SCHEMA): consolidate})
    asyncio.run(Consolidator(learning, NoMemory(), rt.make).catch_up())
    # one label call per run, then one consolidation call for the intent, carrying all three runs
    assert [schema for schema, _ in rt.calls] == [id(intents.INTENT_SCHEMA)] * 3 + [id(PROPOSAL_SCHEMA)]
    assert all(c["id"] in rt.calls[-1][1] for c in cards)
    assert {b["intent_id"] for b in intents.labels(learning).values()} == {iid}
    # the planner's list offers it by the intent's name; its slot values are typed as the planner gives them
    procs = learning.learned_procedures()
    assert [p["node"]["key"] for p in procs] == [iid] and procs[0]["name"] == "cheapest food order"
    ident = procs[0]["node"]["id"]
    assert learning.procedure(ident, {})["state"] == "learned"        # three verified runs, three dish values
    filled = learning.slot_values(ident, [{"name": "dish", "value": '"rajma"'}])
    assert filled == {"dish": "rajma"}
    # the worker gets the steps with rajma filled in, then each earlier run's tool calls (optional, fitted)
    parts = learning.procedure_parts(ident, {}, filled)
    assert parts[0].label == PROCEDURE_LABEL and parts[0].protected
    assert '1. browser.type {"text":"rajma"} — expect: "typed"' in parts[0].text     # the step itself, filled
    runs = [p for p in parts if p.label == RUN_LABEL]
    assert len(runs) == 3 and not any(p.protected for p in runs)
    assert {p.refs for p in runs} == {(c["id"],) for c in cards}
    assert all(f'browser.type {{"text":"{d}"}}' in "\n".join(p.text for p in runs) for d in dishes)
