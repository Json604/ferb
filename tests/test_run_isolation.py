"""A developer run's chats are kept apart (GATE-550): chat.send's `test_run` {run, connection} tag, set by
scripts/question_run.py, keeps one run's chats, memory, learning and activity from another's and FERB learns nothing
from them. The tag changes no question and no send: FERB's questions in a tagged chat wait for the owner as anywhere."""
import json
from typing import Any

from tests.conftest import RunningDaemon


def _tagged(daemon: RunningDaemon, text: str, session: str, connection: Any) -> list[dict[str, Any]]:
    """A run's chat turn, tagged as the run script tags it (GATE-550)."""
    from ferb.cli.client import Client
    from tests.conftest import turn_events
    c = Client(daemon.sock, timeout=60)
    try:
        return turn_events(list(c.stream("chat.send", {"session_id": session, "text": text,
                                                       "test_run": {"run": "r1", "connection": connection}})))
    finally:
        c.close()


# decision: GATE-550, GATE-577
def test_a_second_connections_run_of_the_same_task_sees_nothing_the_first_left(daemon: RunningDaemon) -> None:
    """Owner 2026-11-08: "while runnign smae tasks on all teh provders, there history shouldn collide". The same task
    on a second connection gets no chat, memory, learning or activity the first run left; the first learned nothing.
    No test mode is involved: the tag alone does it."""
    from datetime import datetime
    from ferb.memory.summary import _journal
    from tests.conftest import done, install, sup, task
    from tests.support.schedules import run
    d = daemon.daemon
    words = "find the price of the blue kettle and remember that I like blue kettles"
    planned = json.loads(sup("task", "Looking.", task("Find the blue kettle price"))) | {"remember": [{
        "kind": "fact", "subject": "owner", "key": "kettle_colour", "content": "likes blue kettles",
        "quote": "I like blue kettles"}]}
    script = install(daemon, [json.dumps(planned), json.dumps(planned)],
                     {"worker": done("The blue kettle costs 412 EUR.")})
    _tagged(daemon, words, "qrun-r1-1-codex", "codex")
    _tagged(daemon, words, "qrun-r1-1-anthropic", "anthropic")
    planners = [ctx for _, key, ctx, _ in script.contexts if key == "supervisor"]
    workers = [ctx for _, key, ctx, _ in script.contexts if key == "worker"]
    assert len(planners) == 2 and len(workers) == 2
    assert "412 EUR" not in planners[1] and "412 EUR" not in workers[1]          # nothing of the first run's answer
    assert "likes blue kettles" not in planners[1] + workers[1]                  # nor its memory
    assert not [i for i in run(daemon, lambda: d.memory.items("active")) if i.key == "kettle_colour"]
    assert len(run(daemon, lambda: d.journal.of_kind("test_run.memory_skipped"))) == 2
    trees = run(daemon, lambda: [r[0] for r in d.db.conn.execute("SELECT id FROM tasks WHERE id NOT LIKE '%.%'")])
    assert trees and all(daemon.run(d._capture_card(t, None)) is None for t in trees)       # no learning card
    assert run(daemon, lambda: d.db.conn.execute("SELECT COUNT(*) FROM trail WHERE task_id IS NOT NULL")
               .fetchone()[0]) == 0
    today = datetime.now().astimezone().date().isoformat()
    assert run(daemon, lambda: _journal(d.db, today)) == []                       # the daily distill reads none


# decision: GATE-550, GATE-577
def test_a_malformed_run_tag_is_refused(daemon: RunningDaemon) -> None:
    import pytest
    from ferb.cli.client import RpcError
    with pytest.raises(RpcError, match="test_run must be"):
        _tagged(daemon, "hello", "qrun-x", "")
