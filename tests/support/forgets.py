"""Forget tests' shared helpers (memory review I2, fix wave 6a): the daemon's own records as tests read them, and a
task or chat turn held running across a forget, as the daemon tracks one."""
import json
from typing import Any

from tests.conftest import RunningDaemon, chat, install


def _on_loop(d: RunningDaemon, fn: Any) -> Any:
    async def go() -> Any:
        return fn()
    return d.run(go())


def _reply(text: str, **more: Any) -> str:
    return json.dumps({"kind": "reply", "text": text, **more})


def _remember(d: RunningDaemon, said: str, content: str, key: str) -> Any:
    install(d, [_reply("Okay.", remember=[{"kind": "person", "subject": "owner", "key": key, "content": content,
                                           "quote": content}])])
    chat(d, said)
    return _on_loop(d, lambda: next(i for i in d.daemon.memory.items("active") if i.key == key))


def _lesson(d: RunningDaemon, text: str) -> Any:
    return _on_loop(d, lambda: d.daemon.learning.put("lesson", text, {"text": text}, [], made_by="model:t",
                                                     state="active"))


def _identity(d: RunningDaemon, text: str) -> Any:
    return _on_loop(d, lambda: d.daemon.learning.put("identity", "owner", {"text": text}, [], origin_label="owner",
                                                     made_by="owner:identity/1", owner=True, state="active"))


def _held_in_spans(d: RunningDaemon, text: str, where: str = "1", *args: Any) -> int:
    return _on_loop(d, lambda: sum((r[0] or "").count(text) + (r[1] or "").count(text)
                                   for r in d.daemon.db.conn.execute(
                                       f"SELECT input_json, output_json FROM spans WHERE {where}", args)))


def _texts(d: RunningDaemon, kind: str) -> list[str]:
    return _on_loop(d, lambda: [str(json.loads(r[0]).get("text")) for r in d.daemon.db.conn.execute(
        "SELECT data_json FROM journal WHERE kind = ? ORDER BY seq", (kind,))])
