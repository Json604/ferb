"""Shared learning test fixtures (L29): a store on a fresh database, cards, intent answers, an intent's procedure."""
import json
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

from ferb.memory.learning import Learning
from ferb.store.db import Database

EMPTY = {"procedures": [], "lessons": [], "identity": [], "episodes": []}


class Clock:
    def now(self) -> datetime:
        return datetime(2026, 10, 21, tzinfo=timezone.utc)


class TickingClock:
    """A clock one second later at every reading: each node a store writes is newer than the one before."""

    def __init__(self) -> None:
        self.at = datetime(2026, 10, 21, tzinfo=timezone.utc)

    def now(self) -> datetime:
        self.at += timedelta(seconds=1)
        return self.at


class NoMemory:
    def items(self, **_: Any) -> list[Any]:
        return []

    def get(self, _ident: str) -> None:
        return None


def new_store(path: Path, clock: Any = None) -> tuple[Database, Learning]:
    db = Database(path)
    db.migrate()
    return db, Learning(db, clock or Clock(), lambda *args: None, lambda s: s)


def card(store: Learning, ident: str, request: str, dish: str = "egg curry") -> Any:
    return store.put("episode", ident, {"task_id": ident, "verified": True, "request": request, "apps": {},
                                        "slots": {}, "status": "done",
                                        "actions": [{"id": ident + "-a", "tool": "browser.type",
                                                     "args": {"text": dish}, "outside_action": False}]},
                     [], origin_label="ferb", made_by="code:evidence-card/2", state="active")


def new_intent(name: str, dish: str) -> dict[str, Any]:
    return {"intent_id": "", "name": name, "description": "find the cheapest dish with breads",
            "slots": [{"name": "dish", "value": "string"}], "values": [{"name": "dish", "value": json.dumps(dish)}]}


def on_loop(daemon: Any, fn: Any, *args: Any, **kwargs: Any) -> Any:
    """`fn(*args, **kwargs)` on a test daemon's loop thread (its SQLite connection belongs to that thread)."""
    async def go() -> Any:
        return fn(*args, **kwargs)
    return daemon.run(go())


FOOD_DISHES = ("egg curry", "paneer", "dal")


def store_intent_procedure(store: Learning, *, name: str, slots: dict[str, str]) -> str:
    """Three verified cards of one intent (food0..food2, one dish each), each with its label, and the procedure they
    teach, keyed by the intent id (L29). Its id."""
    from ferb.memory import intents
    iid = intents.intent_id(name)
    cards = []
    for n, dish in enumerate(FOOD_DISHES):
        c = store.put("episode", f"food{n}", {"task_id": f"food{n}", "verified": True, "request": f"cheapest {dish}",
                                              "apps": {}, "slots": {}, "status": "done",
                                              "actions": [{"id": f"food{n}-a", "tool": "browser.type",
                                                           "args": {"text": dish}, "outside_action": False}]},
                      [], origin_label="ferb", made_by="code:evidence-card/2", state="active")
        store.put(intents.INTENT_KIND, c["id"], {"intent_id": iid, "name": name, "description": "the cheapest dish",
                                                 "slots": slots, "values": {"dish": dish}},
                  [c["id"]], made_by="model:test", state="active")
        cards.append(c)
    p = store.propose_procedure({"intent": name, "slots": slots, "preconditions": {"apps": {}},
                                 "steps": [{"tool": "browser.type", "args": {"text": "{dish}"}, "expect": "typed",
                                            "optional": False,
                                            "evidence": [f"food{n}-a" for n in range(len(FOOD_DISHES))]}],
                                 "ui_hints": "", "examples": [{"episode": c["id"], "slots": {"dish": d}}
                                                              for c, d in zip(cards, FOOD_DISHES)]},
                                "b", "model:test|prompt:1", intent_id=iid)
    return str(p["id"])


def intent_procedure(daemon: Any, *, name: str, slots: dict[str, str]) -> str:
    """`store_intent_procedure` in a test daemon's learning store, on its loop."""
    return str(on_loop(daemon, store_intent_procedure, daemon.daemon.learning, name=name, slots=slots))


def label_card(store: Learning, node: dict[str, Any], name: str = "a test intent") -> Any:
    """`node`'s intent label (L29) as the label pass stores it: an intent without slots. A test about consolidation
    labels its cards first, so it sees only consolidation calls."""
    from ferb.memory.intents import INTENT_KIND, intent_id
    return store.put(INTENT_KIND, node["id"], {"intent_id": intent_id(name), "name": name, "description": name,
                                               "slots": {}, "values": {}},
                     [node["id"]], made_by="model:test", state="active")
