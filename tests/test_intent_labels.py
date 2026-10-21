"""L29: each card gets a model-assigned intent label; code checks its shape and types, never sameness."""
import asyncio
import json
import re
from typing import Any

import pytest

from ferb.core.types import ModelLimits
from ferb.memory import intents
from ferb.memory.consolidate import PROPOSAL_SCHEMA, Consolidator
from ferb.memory.learning import Learning, LearningRefused
from ferb.sanitize.redact import Redactor
from ferb.store.db import Database
from tests.support.learning_fixtures import EMPTY, Clock, NoMemory, card, new_intent, new_store
from tests.support.learning_runtime import SchemaAnswers
from tests.support.strict_schema import strict_problems


@pytest.fixture
def learning(tmp_path):
    db, store = new_store(tmp_path / "memory.db")
    yield store
    db.close()


def run(store: Learning, rt: SchemaAnswers) -> dict[str, Any]:
    return asyncio.run(Consolidator(store, NoMemory(), rt.make).catch_up())


def test_the_label_schema_is_strict_compatible() -> None:
    assert strict_problems(intents.INTENT_SCHEMA) == []


def test_an_unlabelled_card_is_labelled_once(learning) -> None:
    c = card(learning, "t1", "find me the cheapest egg curry with breads")
    rt = SchemaAnswers({id(intents.INTENT_SCHEMA): lambda ctx: new_intent("cheapest food order", "egg curry"),
                        id(PROPOSAL_SCHEMA): lambda ctx: EMPTY})
    run(learning, rt)
    label = intents.labels(learning)[c["id"]]
    assert label["name"] == "cheapest food order" and label["values"] == {"dish": "egg curry"}
    assert label["intent_id"] == intents.intent_id("cheapest food order")
    calls = len(rt.calls)
    run(learning, rt)                                   # labelled: no second label call
    assert len(rt.calls) == calls


def test_a_second_run_is_offered_the_existing_intent_and_may_reuse_it(learning) -> None:
    card(learning, "t1", "find me the cheapest egg curry with breads")
    first = SchemaAnswers({id(intents.INTENT_SCHEMA): lambda ctx: new_intent("cheapest food order", "egg curry"),
                           id(PROPOSAL_SCHEMA): lambda ctx: EMPTY})
    run(learning, first)
    existing = intents.intent_id("cheapest food order")
    c2 = card(learning, "t2", "cheapest paneer with naan please", dish="paneer")
    seen: list[str] = []

    def reuse(ctx: str) -> dict[str, Any]:
        seen.append(ctx)
        return {"intent_id": existing, "name": "", "description": "", "slots": [],
                "values": [{"name": "dish", "value": '"paneer"'}]}
    run(learning, SchemaAnswers({id(intents.INTENT_SCHEMA): reuse, id(PROPOSAL_SCHEMA): lambda ctx: EMPTY}))
    assert existing in seen[0]                          # the intent list was offered
    assert intents.labels(learning)[c2["id"]]["intent_id"] == existing
    assert [i["runs"] for i in intents.intents(learning)] == [2]


def test_a_new_intent_named_like_an_existing_one_is_that_intent(learning) -> None:
    card(learning, "t1", "cheapest egg curry")
    run(learning, SchemaAnswers({id(intents.INTENT_SCHEMA): lambda ctx: new_intent("Cheapest food order", "egg curry"),
                                 id(PROPOSAL_SCHEMA): lambda ctx: EMPTY}))
    c2 = card(learning, "t2", "cheapest paneer", dish="paneer")
    run(learning, SchemaAnswers({id(intents.INTENT_SCHEMA): lambda ctx: new_intent("cheapest  FOOD order", "paneer"),
                                 id(PROPOSAL_SCHEMA): lambda ctx: EMPTY}))
    assert intents.labels(learning)[c2["id"]]["intent_id"] == intents.intent_id("cheapest food order")
    assert len(intents.intents(learning)) == 1


def test_a_label_with_wrong_slots_or_types_is_refused_and_waits(learning) -> None:
    offered = {"i-x": {"id": "i-x", "name": "n", "description": "d", "slots": {"dish": "string"}}}
    with pytest.raises(LearningRefused):
        intents.check({"intent_id": "i-x", "name": "", "description": "", "slots": [],
                       "values": [{"name": "size", "value": '"large"'}]}, offered)
    with pytest.raises(LearningRefused):
        intents.check({"intent_id": "i-x", "name": "", "description": "", "slots": [],
                       "values": [{"name": "dish", "value": "3"}]}, offered)
    with pytest.raises(LearningRefused):
        intents.check({"intent_id": "i-unknown", "name": "", "description": "", "slots": [], "values": []}, offered)


def test_a_failed_label_is_recorded_and_tried_again_only_when_forced(learning) -> None:
    card(learning, "t1", "cheapest egg curry")
    bad = SchemaAnswers({id(intents.INTENT_SCHEMA): lambda ctx: {"intent_id": "i-nope", "name": "", "description": "",
                                                                 "slots": [], "values": []},
                         id(PROPOSAL_SCHEMA): lambda ctx: EMPTY})
    run(learning, bad)
    row = learning.db.conn.execute("SELECT status, error FROM learning_jobs WHERE source_ref LIKE 'label:%'").fetchone()
    assert row[0] == "failed" and row[1]
    calls = len(bad.calls)
    run(learning, bad)
    assert len(bad.calls) == calls                      # waits
    asyncio.run(Consolidator(learning, NoMemory(), bad.make).catch_up(force=True))
    assert len(bad.calls) > calls                       # `ferb memory consolidate` tries again


def test_a_failed_label_is_tried_again_when_its_card_changes(learning) -> None:
    card(learning, "t1", "cheapest egg curry")
    run(learning, SchemaAnswers({id(intents.INTENT_SCHEMA): lambda ctx: {"intent_id": "i-nope", "name": "",
                                                                         "description": "", "slots": [], "values": []},
                                 id(PROPOSAL_SCHEMA): lambda ctx: EMPTY}))
    changed = card(learning, "t1", "cheapest egg curry, with naan")        # the card is read again: a new node
    run(learning, SchemaAnswers({id(intents.INTENT_SCHEMA): lambda ctx: new_intent("cheapest food order", "egg curry"),
                                 id(PROPOSAL_SCHEMA): lambda ctx: EMPTY}))
    assert intents.labels(learning)[changed["id"]]["name"] == "cheapest food order"


def test_the_intent_list_is_fitted_to_a_small_window(learning) -> None:
    for n in range(40):                                  # forty labelled cards, forty intents
        c = card(learning, f"old{n}", f"task number {n}")
        learning.put(intents.INTENT_KIND, c["id"], {"intent_id": f"i-{n:02}", "name": f"intent {n} " + "x" * 2000,
                                                    "description": "d", "slots": {}, "values": {}},
                     [c["id"]], made_by="model:test", state="active")
    card(learning, "new", "something new")
    small = ModelLimits("fake-model", 4_000, 500, "test double")
    rt = SchemaAnswers({id(intents.INTENT_SCHEMA): lambda ctx: new_intent("something", "x"),
                        id(PROPOSAL_SCHEMA): lambda ctx: EMPTY}, limits=small)
    run(learning, rt)
    label_calls = [ctx for schema, ctx in rt.calls if schema == id(intents.INTENT_SCHEMA)]
    assert label_calls and "i-39" in label_calls[0] and "i-00" not in label_calls[0]   # newest intents offered first


def test_a_card_too_large_for_the_window_is_never_sent_and_its_job_says_why(learning) -> None:
    big = card(learning, "big", "cheapest egg curry " + "word " * 5_000)
    rt = SchemaAnswers({id(intents.INTENT_SCHEMA): lambda ctx: new_intent("cheapest food order", "egg curry"),
                        id(PROPOSAL_SCHEMA): lambda ctx: EMPTY}, limits=ModelLimits("small", 4_000, 500, "test double"))
    run(learning, rt)
    assert not [ctx for schema, ctx in rt.calls if schema == id(intents.INTENT_SCHEMA)]
    error = learning.db.conn.execute("SELECT error FROM learning_jobs WHERE source_ref = ?",
                                     ("label:" + big["id"],)).fetchone()[0]
    assert "window" in error


def test_no_label_call_once_a_task_has_started(learning) -> None:
    card(learning, "t1", "cheapest egg curry")
    rt = SchemaAnswers({id(intents.INTENT_SCHEMA): lambda ctx: new_intent("cheapest food order", "egg curry"),
                        id(PROPOSAL_SCHEMA): lambda ctx: EMPTY})
    checks = iter([True])                                # idle at the start of the pass, then a task starts
    out = asyncio.run(Consolidator(learning, NoMemory(), rt.make, idle=lambda: next(checks, False)).catch_up())
    assert out["status"] == "busy" and rt.calls == []


def test_an_intent_id_survives_redaction_in_a_label_and_as_a_procedure_key(tmp_path) -> None:
    """Learning.put redacts a body, and the key of a kind not keyed by FERB's own ids (a procedure's): a hex digest
    could rarely look like a card number there. An intent id is letters only."""
    redactor = Redactor()
    ids = [intents.intent_id(f"kind of task {n}") for n in range(5_000)]
    assert all(re.fullmatch(r"i-[a-p]{16}", i) for i in ids) and len(set(ids)) == len(ids)
    assert redactor.text(json.dumps(ids)) == json.dumps(ids)
    db = Database(tmp_path / "memory.db")
    db.migrate()
    try:
        store = Learning(db, Clock(), lambda *args: None, redactor.text, redactor.ident)
        c = card(store, "t1", "find me the cheapest egg curry with breads")
        iid = intents.intent_id("cheapest food order")
        label = store.put(intents.INTENT_KIND, c["id"], {"intent_id": iid, "name": "cheapest food order",
                                                         "description": "find the cheapest dish with breads",
                                                         "slots": {"dish": "string"}, "values": {"dish": "egg curry"}},
                          [c["id"]], made_by="model:test", state="active")
        assert label["key"] == c["id"] and label["body"]["intent_id"] == iid
        assert intents.labels(store)[c["id"]]["intent_id"] == iid
        procedure = store.propose_procedure({
            "intent": "cheapest food order", "slots": {"dish": "string"}, "preconditions": {"apps": {}},
            "steps": [{"tool": "browser.type", "args": {"text": "{dish}"}, "expect": "the dish is typed",
                       "evidence": ["t1-a"]}],
            "ui_hints": "", "examples": [{"episode": c["id"], "slots": {"dish": "egg curry"}}]},
            "b-1", "model:test", intent_id=iid)
        assert procedure["key"] == iid
    finally:
        db.close()


def test_a_label_is_never_offered_to_the_planner_as_learned_memory(daemon) -> None:
    d = daemon.daemon

    async def put() -> list[str]:
        c = card(d.learning, "t1", "find me the cheapest egg curry with breads")
        d.learning.put(intents.INTENT_KIND, c["id"], {"intent_id": intents.intent_id("zorblat food order"),
                                                      "name": "zorblat food order", "description": "zorblat dishes",
                                                      "slots": {}, "values": {}},
                       [c["id"]], made_by="model:test", state="active")
        return [p["kind"] for p in d.learning.search("zorblat food order")]
    assert intents.INTENT_KIND in daemon.run(put())      # the label is in learned memory
    found = daemon.run(d._learning_candidates("zorblat food order"))
    assert not [f for f in found if intents.INTENT_KIND in f["text"]]


def test_forgetting_a_card_removes_its_label_and_its_failed_label_job(learning) -> None:
    labelled, failed = card(learning, "t1", "cheapest egg curry"), card(learning, "t2", "cheapest paneer", "paneer")
    refused = {"intent_id": "", "name": "n", "description": "d", "slots": [],
               "values": [{"name": "paneer", "value": '"x"'}]}             # a value for no slot: refused
    run(learning, SchemaAnswers({id(intents.INTENT_SCHEMA): lambda ctx: refused if "cheapest paneer" in ctx else
                                 new_intent("cheapest food order", "egg curry"),
                                 id(PROPOSAL_SCHEMA): lambda ctx: EMPTY}))
    assert labelled["id"] in intents.labels(learning)
    assert learning.db.conn.execute("SELECT 1 FROM learning_jobs WHERE source_ref = ? AND status = 'failed'",
                                    ("label:" + failed["id"],)).fetchone()
    learning.forget([labelled["id"], failed["id"]])
    assert intents.labels(learning) == {} and learning.items(intents.INTENT_KIND) == []
    assert learning.db.conn.execute("SELECT count(*) FROM learning_jobs WHERE source_ref LIKE 'label:%'"
                                    ).fetchone()[0] == 0
