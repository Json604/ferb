"""Shared learning test fixtures: a store on a fresh database and cards."""
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from ferb.memory.learning import Learning
from ferb.store.db import Database

class Clock:
    def now(self) -> datetime:
        return datetime(2026, 10, 21, tzinfo=timezone.utc)


class NoMemory:
    def items(self, **_: Any) -> list[Any]:
        return []

    def get(self, _ident: str) -> None:
        return None


def new_store(path: Path, clock: Any = None, redact: Any = None) -> tuple[Database, Learning]:
    """A learning store on a fresh database; `redact`: its text redactor (default: none, the identity)."""
    db = Database(path)
    db.migrate()
    return db, Learning(db, clock or Clock(), lambda *args: None, redact or (lambda s: s))


def card(store: Learning, ident: str, request: str, dish: str = "egg curry") -> Any:
    return store.put("episode", ident, {"task_id": ident, "verified": True, "request": request, "apps": {},
                                        "slots": {}, "status": "done",
                                        "actions": [{"id": ident + "-a", "tool": "browser.type",
                                                     "args": {"text": dish}, "outside_action": False}]},
                     [], origin_label="ferb", made_by="code:evidence-card/2", state="active")


def on_loop(daemon: Any, fn: Any, *args: Any, **kwargs: Any) -> Any:
    """`fn(*args, **kwargs)` on a test daemon's loop thread (its SQLite connection belongs to that thread)."""
    async def go() -> Any:
        return fn(*args, **kwargs)
    return daemon.run(go())


FOOD_DISHES = ("egg curry", "paneer", "dal")
