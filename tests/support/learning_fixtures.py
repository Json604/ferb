"""Shared learning test fixtures (L29): a store on a fresh database, cards, intent answers."""
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


def label_card(store: Learning, node: dict[str, Any], name: str = "a test intent") -> Any:
    """`node`'s intent label (L29) as the label pass stores it: an intent without slots. A test about consolidation
    labels its cards first, so it sees only consolidation calls."""
    from ferb.memory.intents import INTENT_KIND, intent_id
    return store.put(INTENT_KIND, node["id"], {"intent_id": intent_id(name), "name": name, "description": name,
                                               "slots": {}, "values": {}},
                     [node["id"]], made_by="model:test", state="active")
