"""Forget tests' shared helpers (memory review I2, fix wave 6a): the daemon's own records as tests read them, and a
task or chat turn held running across a forget, as the daemon tracks one."""
import json
from typing import Any

from tests.conftest import RunningDaemon, chat, install


QUILL = "my dentist is Dr. Quillfeather"


def _on_loop(d: RunningDaemon, fn: Any) -> Any:
    async def go() -> Any:
        return fn()
    return d.run(go())


def _reply(text: str, **more: Any) -> str:
    return json.dumps({"kind": "reply", "text": text, "display_text": text, **more})


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


def _answers(d: RunningDaemon, kind: str, key: str, value: str) -> list[str]:
    return _on_loop(d, lambda: [str(json.loads(r[0]).get("text")) for r in d.daemon.db.conn.execute(
        f"SELECT data_json FROM journal WHERE kind = ? AND json_extract(data_json, '$.{key}') = ? ORDER BY seq",
        (kind, value))])


def _task_records(d: RunningDaemon, task_id: str) -> tuple[list[Any], list[str]]:
    """A task's spans (input, output) and its "while you were away" answer."""
    db = d.daemon.db
    return _on_loop(d, lambda: (
        [(json.loads(r[0]), json.loads(r[1] or "null")) for r in db.conn.execute(
            "SELECT input_json, output_json FROM spans WHERE task_id = ? ORDER BY rowid", (task_id,))],
        [r[0] for r in db.conn.execute("SELECT answer FROM task_results WHERE task_id = ?", (task_id,))]))


def _result(d: RunningDaemon, task_id: str, answer: str) -> None:
    d.daemon.db.conn.execute("INSERT OR REPLACE INTO task_results (task_id, at, status, goal, answer, shown) VALUES "
                             "(?, ?, 'done', 'book it', ?, 1)", (task_id, d.daemon.clock.now().isoformat(), answer))


def _running_across_the_forget(d: RunningDaemon, writes: Any, request: str | None = None,
                               task_id: str | None = None) -> tuple[Any, Any]:
    """A chat turn (for `request`, running its task `task_id`) still running when the owner forgets, as the daemon
    tracks one: once let go, it records what `writes` writes, then ends. Returns (gate, job)."""
    import asyncio
    gate = asyncio.Event()

    async def start() -> Any:
        if task_id is not None:
            now = d.daemon.clock.now().isoformat()
            d.daemon.db.conn.execute(
                "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
                "outcome_json, created_at, updated_at) VALUES (?, ?, NULL, ?, 'book it', 'worker', 'worker', '[]', "
                "'running', '{}', ?, ?)", (task_id, task_id, request, now, now))

        async def job() -> None:
            await gate.wait()
            writes()
        task = asyncio.get_running_loop().create_task(job())
        d.daemon._turns.add(task)
        task.add_done_callback(d.daemon._turns.discard)
        if request is not None:
            d.daemon._turn_requests[task] = request
            task.add_done_callback(lambda done: d.daemon._turn_requests.pop(done, None))
        return task
    return gate, d.run(start())


def _let_go(d: RunningDaemon, gate: Any, job: Any) -> None:
    """The job records what it records after the forget's scrub and ends; then every scrub queued has finished."""
    async def ended() -> None:
        await job
    d.call_soon(gate.set)
    d.run(ended())
    d.run(d.daemon._settle_forgets())
