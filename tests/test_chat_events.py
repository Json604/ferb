"""L7, L19 (TQ3, P25, P29): the daemon acknowledges every message on receipt, before routing; every chat event names
its request and when it was sent; the CLI never answers a pending question from ordinary typed text."""
import json
import queue
import re
import threading
import time
from datetime import datetime
from pathlib import Path
from typing import Any

from ferb.cli.client import Client
from ferb.core.types import Identity
from ferb.policy.rules import ActionRequest
from tests.conftest import done, install, sup, task

ROOT = Path(__file__).resolve().parents[1]


def _stream(sock: Path, session: str, text: str) -> "queue.Queue[dict[str, Any] | None]":
    """One chat.send stream read on its own thread; its events land on a queue, None when it ends."""
    events: queue.Queue[dict[str, Any] | None] = queue.Queue()
    client = Client(sock, timeout=30)

    def read() -> None:
        try:
            for event in client.stream('chat.send', {'session_id': session, 'text': text}):
                events.put(event)
        finally:
            events.put(None)
            client.close()
    threading.Thread(target=read, daemon=True).start()
    return events


def _rest(events: "queue.Queue[dict[str, Any] | None]") -> list[dict[str, Any]]:
    out = []
    while (event := events.get(timeout=30)) is not None:
        out.append(event)
    return out


def _journal(daemon: Any, kind: str) -> list[tuple[int, dict[str, Any]]]:
    async def rows() -> list[tuple[int, dict[str, Any]]]:
        return [(r['seq'], json.loads(r['data_json'])) for r in daemon.daemon.db.conn.execute(
            'SELECT seq, data_json FROM journal WHERE kind = ? ORDER BY seq', (kind,))]
    return daemon.run(rows())  # type: ignore[no-any-return]


def _stamped(event: dict[str, Any], request_id: str) -> bool:
    return event.get('request_id') == request_id and datetime.fromisoformat(event['at']).tzinfo is not None


def test_ack_arrives_before_the_planner_is_called(daemon):
    called, release = threading.Event(), threading.Event()

    def planner(_token: str) -> str:
        called.set()
        assert release.wait(20), 'the test never released the planner'
        return sup('reply', 'Hello again.')
    install(daemon, [planner])
    events = _stream(daemon.sock, 's1', 'Hi there')
    try:
        first = events.get(timeout=5)              # the planner is still blocked: only the ack can have been sent
        assert not release.is_set()
    finally:
        release.set()
    rest = _rest(events)
    assert called.is_set()
    assert first is not None and first['type'] == 'ack'
    from ferb.orchestrator import acks
    assert first['text'] in acks.PHRASES
    assert _stamped(first, first['request_id'])
    assert [e['type'] for e in rest if e['type'] in ('ack', 'reply')] == ['reply']
    # Journaled into chat history before the message is routed (the planner runs after chat.message).
    [(ack_seq, ack_row)] = _journal(daemon, 'chat.ack')
    [(message_seq, message_row)] = _journal(daemon, 'chat.message')
    assert ack_row == {'session_id': 's1', 'request_id': first['request_id'], 'text': first['text'],
                       'received_at': ack_row['received_at']}             # when the message arrived (L8 baseline)
    assert datetime.fromisoformat(ack_row['received_at']) <= datetime.fromisoformat(first['at'])
    assert message_row['request_id'] == first['request_id'] and ack_seq < message_seq


def test_two_sends_on_one_session_each_get_their_own_ack_and_request_id(daemon):
    release, called = threading.Event(), threading.Event()

    def planner_a(_token: str) -> str:
        called.set()
        assert release.wait(20), 'the test never released the first planner'
        return sup('task', 'Writing the note.', task('write the note'))
    install(daemon, [planner_a, sup('reply', 'The note task is running.')], {'worker': done('Wrote the note.')})
    published: list[tuple[str, dict[str, Any]]] = []
    publish = daemon.daemon._publish
    daemon.daemon._publish = lambda topic, data: (published.append((topic, data)), publish(topic, data))[1]
    a = _stream(daemon.sock, 'shared', 'Write the note')
    try:
        ack_a = a.get(timeout=5)
        assert called.wait(5)                      # the first message's planner is thinking
        b = _stream(daemon.sock, 'shared', 'What are you doing?')
        ack_b = b.get(timeout=5)                   # acknowledged while the first message is still planning
        assert not release.is_set()
    finally:
        release.set()
    events_a, events_b = [ack_a, *_rest(a)], [ack_b, *_rest(b)]
    assert ack_a is not None and ack_b is not None
    assert ack_a['type'] == ack_b['type'] == 'ack' and ack_a['request_id'] != ack_b['request_id']
    for events, ack in ((events_a, ack_a), (events_b, ack_b)):
        assert [e['type'] for e in events].count('ack') == 1
        assert all(_stamped(e, ack['request_id']) for e in events), events
    # Decision 18: the second message came before FERB's first reply, so the turn restarted and read both: the second
    # stream ends merged into the first, whose turn replies once (its first planner answer was never acted on)
    assert [e['type'] for e in events_b[1:]] == ['merged'] and events_b[1]['into'] == ack_a['request_id']
    assert [e['type'] for e in events_a if e['type'] in ('plan', 'reply')] == ['reply']
    assert not [data for topic, data in published if topic == 'task.created']
    # P29: still journaled in arrival order; each chat.message carries the id its ack announced, the second read with
    # the first
    rows = [row for _, row in _journal(daemon, 'chat.message')]
    assert [row['request_id'] for row in rows] == [ack_a['request_id'], ack_b['request_id']]
    assert rows[1]['read_with'] == ack_a['request_id'] and 'read_with' not in rows[0]
    assert [row['request_id'] for _, row in _journal(daemon, 'chat.ack')] == [ack_a['request_id'], ack_b['request_id']]


def test_a_result_names_its_request_even_after_the_stream_dropped_before_the_plan(daemon):
    """chat.status and the task.result bus event let a client place an answer whose chat.send stream ended early."""
    release = threading.Event()

    def planner(_token: str) -> str:
        assert release.wait(20), 'the test never released the planner'
        return sup('task', 'Writing the note.', task('write the note'))
    install(daemon, [planner], {'worker': done('Wrote the note.')})
    published: list[tuple[str, dict[str, Any]]] = []
    publish = daemon.daemon._publish
    daemon.daemon._publish = lambda topic, data: (published.append((topic, data)), publish(topic, data))[1]
    client = Client(daemon.sock, timeout=30)
    try:
        ack = next(client.stream('chat.send', {'session_id': 'dropped', 'text': 'Write the note'}))
    finally:
        client.close()                             # the stream drops before the plan; the turn keeps going
    release.set()
    status: dict[str, Any] = {}
    reader = Client(daemon.sock)
    try:
        for _ in range(300):
            status = reader.call('chat.status', {'peek': True})
            if status['results']:
                break
            time.sleep(0.05)
    finally:
        reader.close()
    [result] = status['results']
    assert ack['type'] == 'ack' and result['request_id'] == ack['request_id']
    assert datetime.fromisoformat(result['at']).tzinfo is not None
    [bus] = [data for topic, data in published if topic == 'task.result']
    assert bus['request_id'] == ack['request_id'] and bus['task_id'] == result['task_id'] and bus['at'] == result['at']
    # A result reported without a chat stream (a resumed plan) names the request its task ran for, too.
    daemon.run(daemon.daemon._task_result({'task_id': result['task_id'], 'status': 'done', 'answer': 'Again.'},
                                          shown=True))
    again = [data for topic, data in published if topic == 'task.result'][-1]
    assert again['request_id'] == ack['request_id'] and datetime.fromisoformat(again['at']).tzinfo is not None


def test_pick_never_repeats_one_of_the_last_eight():
    from ferb.orchestrator import acks
    said: list[str] = []
    for _ in range(200):
        phrase = acks.pick(said)
        assert phrase in acks.PHRASES and phrase not in said[-8:]
        said.append(phrase)
    assert len(set(said)) > 8


def test_daemon_acks_never_repeat_one_of_the_last_eight_said(daemon):
    """The daemon's window is what it journaled, across messages and chats (the CLI no longer keeps its own)."""
    async def acknowledge() -> list[str]:
        return [daemon.daemon._chat_ack(f's{n % 3}', f'r{n}')['text'] for n in range(40)]
    said = daemon.run(acknowledge())
    assert all(phrase not in said[max(0, n - 8):n] for n, phrase in enumerate(said))
    assert [row['text'] for _, row in _journal(daemon, 'chat.ack')] == said


def test_ack_phrases_are_the_plans_p25_list_without_the_starred_one():
    from ferb.orchestrator import acks
    plan = (ROOT / 'docs' / '0.27_PLAN.md').read_text()
    draft = re.search(r'- P25\. .*?Draft:\s*(.*?)\s*Sources:', plan, re.S)
    assert draft is not None
    listed = [p.strip() for p in re.sub(r'\(\*[^)]*\)', '', draft.group(1)).split('·')]
    assert acks.PHRASES == tuple(p for p in listed if p and not p.endswith('*'))
    assert 'On it, one sec.' not in acks.PHRASES and len(acks.PHRASES) == 29


def test_approval_requested_carries_its_request_id_only_when_it_belongs_to_a_task(daemon):
    approvals = daemon.daemon.state.approvals
    task_question = approvals.question(ActionRequest('owner.ask', 'owner_question', Identity.FERB, frozenset(),
                                                     'tree-1.worker', 'tree-1.worker', 'req-1', frozenset()),
                                       'Which file?', ['a.md', 'b.md'])
    setup_question = approvals.question(ActionRequest('setup.persona', 'owner_question', Identity.OWNER, frozenset(),
                                                      '', '', 'setup-persona', frozenset()), 'Save it?', ['Skip'])
    seen: list[tuple[str, dict[str, Any]]] = []
    sub: list[Any] = []

    async def collect() -> None:
        async for ev in sub[0]:
            seen.append((ev.topic, ev.data))
            if len(seen) == 2:
                return
    daemon.call_soon(lambda: sub.append(daemon.daemon.bus.subscribe({'approval.requested'})))
    daemon.call_soon(daemon.daemon._publish, 'approval.requested',
                     {'approval_id': task_question.id, 'reason': 'Which file?', 'task_id': 'tree-1.worker'})
    daemon.call_soon(daemon.daemon._publish, 'approval.requested',
                     {'approval_id': setup_question.id, 'reason': 'Save it?', 'task_id': None})
    daemon.run(collect())
    assert seen[0][1]['request_id'] == 'req-1'
    assert 'request_id' not in seen[1][1]
