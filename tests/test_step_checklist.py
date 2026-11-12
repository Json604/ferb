"""APP-894 and ORCH-626 through the real daemon and the scripted model: a single ask whose plan has three or more steps
carries its steps (each with its label, else its goal's first clause, cut) and their states on the plan event, the
outcome and chat.status, so FERB.app draws them as a checklist; a shorter plan carries none; a multi-part card names its
plan's steps too (APP-895: the feeder steps a row waits on). Live 2026-11-11 14:14 (task f67b5efe): one ask, four steps,
one card with no sign of which step ran."""
import json
from typing import Any

from ferb.cli.client import Client
from ferb.orchestrator.plans import Plan, Step
from tests.conftest import RunningDaemon, chat, cond, done, install, outcome, sup, task
from tests.support.schedules import run

GATHER_GOAL = ("Read the GitHub FERB repository and the local FERB repository on the Desktop, inspect relevant history "
               "and documentation, and produce accurate application-ready notes.")


def _deck_plan(labels: bool = True) -> list[Step]:
    return [Step(id="gather", kind="do", goal=GATHER_GOAL),
            Step(id="research", kind="do", label="Research startup decks" if labels else "",
                 goal="Thoroughly research effective startup pitch decks."),
            Step(id="deck", kind="do", label="Build the deck" if labels else "", goal="Build the deck in HTML and PDF.",
                 depends_on=["gather", "research"]),
            Step(id="fill", kind="do", label="Fill the form" if labels else "", goal="Fill the open form.",
                 depends_on=["gather", "deck"])]


def _spec(steps: list[Step]) -> dict[str, Any]:
    spec = task("Buildathon deck and form", conds=(cond("The form is filled"),))
    spec["plan"] = Plan(steps=steps).model_dump()
    return spec


def _workers(steps: list[Step]) -> dict[str, Any]:
    return {f"step.{s.id}": done(f"{s.id} done.") for s in steps}


# decision: APP-894, ORCH-626
def test_a_single_ask_of_four_steps_carries_its_checklist_from_plan_to_end(daemon: RunningDaemon) -> None:
    steps = _deck_plan()
    install(daemon, [sup("task", "Here's the plan.", _spec(steps))], _workers(steps))
    events = chat(daemon, "gather FERB's story, research decks, build a deck and fill the form")
    plan = next(e for e in events if e["type"] == "plan")
    lines = plan["checklist"]["steps"]
    assert [line["label"] for line in lines] == ["Read the GitHub FERB repository and…", "Research startup decks",
                                                 "Build the deck", "Fill the form"]
    assert lines[2]["depends_on"] == ["gather", "research"] and lines[2]["step_task_id"].endswith(".step.deck")
    assert plan["checklist"]["states"]["deck"]["status"] == "waiting"
    assert "card" not in plan
    final = outcome(events)
    assert final["status"] in ("done", "done_unverified")
    assert {k: v["status"] for k, v in final["checklist"]["states"].items()} == dict.fromkeys(
        ("gather", "research", "deck", "fill"), "done")
    # what a restored chat reads (chat.status results come from it): the record of the end
    told = json.loads(run(daemon, lambda: daemon.daemon.tasks.column(final["task_id"], "outcome_json")))
    assert [line["id"] for line in told["checklist"]["steps"]] == ["gather", "research", "deck", "fill"]
    c = Client(daemon.sock, timeout=10)                 # the Tasks page row says the same count as the card
    try:
        row = next(r for r in c.call("tasks.list", {})["tasks"] if r["task_id"] == final["task_id"])
    finally:
        c.close()
    assert row["steps"] == {"done": 4, "total": 4}


# decision: APP-894
def test_a_two_step_plan_carries_no_checklist(daemon: RunningDaemon) -> None:
    steps = [Step(id="read", kind="do", goal="Read the notes"),
             Step(id="sum", kind="do", goal="Sum them up", depends_on=["read"])]
    install(daemon, [sup("task", "Reading.", _spec(steps))], _workers(steps))
    events = chat(daemon, "read my notes and sum them up")
    assert "checklist" not in next(e for e in events if e["type"] == "plan")
    assert "checklist" not in outcome(events)


# decision: APP-895
def test_a_multi_part_card_names_its_plans_steps(daemon: RunningDaemon) -> None:
    """The live 14:41 card (f67b5efe): its one working row waited on steps that answer no part; the card now names
    every step with its label and what it waits on, so the app shows those steps under the row."""
    steps = [Step(id="research", kind="do", label="Research deck design", goal="Research deck design."),
             Step(id="redesign", kind="do", label="Redesign the deck", goal="Redesign it.", depends_on=["research"]),
             Step(id="attach", kind="do", goal="Attach the improved PDF to the form.", depends_on=["redesign"])]
    spec = _spec(steps)
    spec["success_conditions"] = [cond("The deck is improved")]
    spec["parts"] = [{"id": "rules", "quote": "no em dashes", "label": "Writing and design rules", "direct": "Noted.",
                      "answer_step": None, "conditions": []},
                     {"id": "deck", "quote": "improve this ppt", "label": "Improved pitch deck", "direct": None,
                      "answer_step": "attach", "conditions": [0]}]
    install(daemon, [sup("task", "On it.", spec)], _workers(steps))
    events = chat(daemon, "no em dashes. improve this ppt")
    card = next(e for e in events if e["type"] == "plan")["card"]
    assert [(line["id"], line["label"], line["depends_on"]) for line in card["plan"]] == [
        ("research", "Research deck design", []), ("redesign", "Redesign the deck", ["research"]),
        ("attach", "Attach the improved PDF to the form", ["redesign"])]
    assert "checklist" not in next(e for e in events if e["type"] == "plan")
