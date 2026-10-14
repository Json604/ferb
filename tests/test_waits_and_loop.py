"""L21-L23: time waits fire on wall-clock time after Mac sleep; the plan inventory walk leaves the daemon's loop
free; a Codex account check that fails in any way still gives its request a chat reply naming the failure."""
import asyncio
import json
import threading
from typing import Any
from datetime import datetime, timezone
from pathlib import Path

import pytest

from ferb.cli.client import Client
from ferb.orchestrator.plan_executor import PlanExecutor
from ferb.orchestrator.plan_store import PlanStore
from ferb.orchestrator.plans import Plan, Step
from ferb.runtimes.codex_exec import MODEL, CodexRuntime
from ferb.store.db import Database
from tests.conftest import chat, done, install, outcome, sup, task
from tests.support.runtime_fakes import Process


# ---------- L22: waits after sleep ----------

def test_time_wait_fires_at_wall_time_when_the_monotonic_clock_paused(tmp_path, monkeypatch):
    """A wait set at 23:00 for 09:00 on a Mac that sleeps overnight: asyncio's monotonic clock barely moves while the
    wall clock jumps ten hours; the wait must still end as soon as the wall clock reaches 09:00."""
    import ferb.clock.timers as durable
    import ferb.orchestrator.plan_executor as executor
    monkeypatch.setattr(durable, 'POLL_S', 0.01)        # test speed only: production re-checks every POLL_S
    wall = {'now': datetime(2026, 10, 13, 23, 0, tzinfo=timezone.utc)}

    class WallClock(datetime):
        @classmethod
        def now(cls, tz=None):                          # type: ignore[override]
            return wall['now'].astimezone(tz) if tz else wall['now']
    monkeypatch.setattr(executor, 'datetime', WallClock)
    db = Database(tmp_path / 'state.db')
    db.migrate()
    store = PlanStore(db, lambda value: value)
    store.create('tree', Plan(steps=[Step(id='board', kind='wait', goal='Wait until 09:00',
                                          until='2026-10-14T09:00:00+00:00')]), {}, 'chat')

    async def worker(step, inputs, prior):
        raise AssertionError('a wait step never runs a worker')

    async def run():
        job = asyncio.create_task(PlanExecutor(store, worker, lambda: False).run('tree'))
        while store.steps('tree')['board']['status'] != 'waiting':
            await asyncio.sleep(0)
        await asyncio.sleep(0.05)
        assert not job.done()                           # ten hours early: still waiting
        wall['now'] = datetime(2026, 10, 14, 9, 0, 1, tzinfo=timezone.utc)   # the Mac woke after 09:00
        return await asyncio.wait_for(job, 2)
    result = asyncio.run(run())
    assert result['status'] == 'done'
    assert store.steps('tree')['board']['output']['status'] == 'done'
    db.close()


def test_time_wait_ends_cancelled_when_stopped(tmp_path):
    db = Database(tmp_path / 'state.db')
    db.migrate()
    store = PlanStore(db, lambda value: value)
    store.create('tree', Plan(steps=[Step(id='board', kind='wait', goal='Wait until tomorrow',
                                          until='2999-01-01T09:00:00+00:00')]), {}, 'chat')
    stopped = {'on': False}

    async def worker(step, inputs, prior):
        raise AssertionError('a wait step never runs a worker')

    async def run():
        engine = PlanExecutor(store, worker, lambda: stopped['on'])
        job = asyncio.create_task(engine.run('tree'))
        while store.steps('tree')['board']['status'] != 'waiting':
            await asyncio.sleep(0)
        stopped['on'] = True
        engine.wake()
        return await asyncio.wait_for(job, 2)
    result = asyncio.run(run())
    assert result['status'] == 'cancelled'
    assert store.steps('tree')['board']['output']['status'] == 'cancelled'
    db.close()


# ---------- L23: the inventory walk runs off the event loop ----------

def test_slow_inventory_walk_leaves_rpcs_answering(daemon, monkeypatch, tmp_path):
    """The planner declares a folder as input_paths. While the walk over it is still in progress, an unrelated RPC
    (handled by a coroutine on the daemon's loop) must answer; the walk then finishes and the plan runs."""
    root = tmp_path / 'sessions'
    root.mkdir()
    (root / 'notes.txt').write_text('owner controlled notes')
    walking, release, seen = threading.Event(), threading.Event(), {}
    real_rglob = Path.rglob

    def slow_rglob(self, *args, **kwargs):
        if Path(self).resolve() == root.resolve() and not walking.is_set():
            walking.set()
            seen['released'] = release.wait(5)         # a regression fails here instead of hanging the suite
        return real_rglob(self, *args, **kwargs)
    monkeypatch.setattr(Path, 'rglob', slow_rglob)
    spec = task('Read my session notes')
    spec['plan'] = Plan(steps=[Step(id='read', kind='do', goal='Read the session notes')],
                        input_paths=[str(root)]).model_dump()
    install(daemon, [sup('task', 'Reading your session notes.', spec)],
            {'step.read': done('Session notes read.'), 'worker': done('Session notes checked.')})
    turn: dict = {}
    chatting = threading.Thread(target=lambda: turn.update(events=chat(daemon, 'Read my session notes')))
    chatting.start()
    assert walking.wait(10), 'the planner never walked the declared folder'
    client = Client(daemon.sock, timeout=10)
    try:
        answered = client.call('ping', {})
    finally:
        client.close()
    release.set()
    chatting.join(30)
    assert answered['pid']
    assert seen['released'] is True, 'the RPC only answered after the walk gave up waiting: the walk held the loop'
    assert outcome(turn['events'])['status'] == 'done'


# ---------- L21: Codex account-check failures reach chat ----------

class AppServer(Process):
    """The shared app-server fake, but the account check fails as told. Behind FERB's real Codex adapter, so no
    thread or model turn is ever started."""

    def __init__(self, check):
        super().__init__()
        self.check = check

    @property
    def methods(self):
        return [m['method'] for m in self.requests]

    def write(self, data):
        message = json.loads(data)
        if message['method'] != 'account/read':
            return super().write(data)
        self.requests.append(message)
        if self.check == 'error':                      # the shape 5b10afb named: an RPC error response
            self.stdout.put({'id': message['id'], 'error': {'code': -32000, 'message': 'provider-private detail'}})
        elif self.check == 'exits':                    # the app-server ends before answering
            self.kill()
        # 'silent': never answers


def _codex_behind_stub(monkeypatch, tmp_path, check):
    """FERB's real Codex adapter with FERB's own login file; only the app-server process is the stub."""
    import ferb.runtimes.codex_stream as stream
    (tmp_path / 'codex-home').mkdir()
    (tmp_path / 'codex-home' / 'auth.json').write_text('{"tokens":{"refresh_token":"test-only-fixture"}}')
    # the catalog Codex fetches for FERB's login: the adapter reads FERB's default model's window from it (L28)
    (tmp_path / 'codex-home' / 'models_cache.json').write_text(json.dumps(
        {'models': [{'slug': MODEL, 'context_window': 272000, 'effective_context_window_percent': 95}]}))
    servers: list[AppServer] = []
    real_spawn = asyncio.create_subprocess_exec

    async def spawn(*cmd, **kwargs):
        if cmd and cmd[0] == '/fake/codex':
            servers.append(AppServer(check))
            return servers[-1]
        return await real_spawn(*cmd, **kwargs)
    monkeypatch.setattr(asyncio, 'create_subprocess_exec', spawn)

    async def adapter_start(self, task_, request, run_token):
        return await stream.start(self, task_, request, run_token)
    monkeypatch.setattr(CodexRuntime, 'start', adapter_start)
    return servers


def _journal(daemon, kind):
    async def rows():
        return [e.data for e in daemon.daemon._journal().entries() if e.kind == kind]
    return daemon.run(rows())


@pytest.mark.parametrize('check', ['error', 'exits', 'silent'])
def test_account_check_failure_gets_a_named_reply_for_its_request(daemon, monkeypatch, tmp_path, check):
    servers = _codex_behind_stub(monkeypatch, tmp_path, check)
    daemon.daemon.supervisor.make_runtime = lambda role, *, mode, tool_names, schema: CodexRuntime(
        Path('/fake/codex'), 'supervisor', tmp_path / 'work', tmp_path, output_schema=schema, model=MODEL,
        timeout_s=0.5 if check == 'silent' else None)
    client = Client(daemon.sock, timeout=30)
    try:
        events = list(client.stream('chat.send', {'session_id': 's1',
                                                  'text': 'set a timer for 8:20, the boarding starts at 8.25'}))
    finally:
        client.close()
    ack = events[0]
    replies = [e for e in events if e['type'] == 'reply']
    assert ack['type'] == 'ack' and len(replies) == 1, events
    assert replies[0]['request_id'] == ack['request_id']
    assert 'Codex could not check the account' in replies[0]['text'], replies[0]['text']
    assert 'provider-private detail' not in json.dumps(events)
    assert servers and not {'thread/start', 'turn/start'} & set(servers[0].methods)
    recorded = _journal(daemon, 'chat.failed')
    assert recorded[-1]['request_id'] == ack['request_id'] and recorded[-1]['why'] == 'ServiceError'
    said = [r for r in _journal(daemon, 'chat.reply') if r.get('request_id') == ack['request_id']]
    assert [r['text'] for r in said] == [replies[0]['text']]          # in the conversation record too


def test_next_turn_sees_the_failed_turns_named_reply(daemon, monkeypatch):
    """The owner can say "retry": the planner's next turn reads the failure FERB told them, not a silent gap."""
    from ferb.runtimes.codex_stream import AccountError
    from tests.conftest import FakeRuntime
    script = install(daemon, [sup('reply', 'Trying again now.')])
    fake_start, failed = FakeRuntime.start, []

    async def first_fails(self, task_, request, token):
        if not failed:
            failed.append(task_.id)
            raise AccountError('account_unavailable', -32000)
        return await fake_start(self, task_, request, token)
    monkeypatch.setattr(FakeRuntime, 'start', first_fails)
    first = chat(daemon, 'set a timer for 8:20, the boarding starts at 8.25')
    assert 'Codex could not check the account' in first[0]['text']
    chat(daemon, 'retry')
    planner_context = [c for mode, key, c, _ in script.contexts if key == 'supervisor'][-1]
    assert 'FERB: ' + first[0]['text'] in planner_context


def test_scheduled_run_account_failure_is_named_and_recorded(daemon, monkeypatch):
    """Nobody is at chat for a scheduled run: its answer names the failure (never "say that again"), the cause is
    journaled under the run's request_id, and the published result keeps that request_id."""
    from datetime import timedelta
    from ferb.policy.targets import Targets
    from ferb.runtimes.codex_stream import AccountError
    from tests.conftest import FakeRuntime
    from tests.support.schedules import fire, make_schedule, results
    notes: list[str] = []

    async def note(title, text):
        notes.append(text)
    daemon.daemon.notify = note
    published: list[dict] = []
    publish = daemon.daemon._publish

    def record(topic, data):
        if topic == 'task.result':
            published.append(data)
        return publish(topic, data)
    daemon.daemon._publish = record
    install(daemon, [])

    async def unavailable(self, task_, request, token):
        raise AccountError('account_unavailable', -32000)
    monkeypatch.setattr(FakeRuntime, 'start', unavailable)
    make_schedule(daemon, 'post the launch thread', 'every day at 9 post the launch thread', Targets.of(),
                  daemon.daemon.clock.now() + timedelta(minutes=1))
    fire(daemon, 61)
    run_id = _journal(daemon, 'schedule.run')[-1]['request_id']
    answer = results(daemon)[-1]['answer']
    assert 'Codex could not check the account' in answer and 'say that again' not in answer
    assert notes and 'Codex could not check the account' in notes[-1]
    failed = [r for r in _journal(daemon, 'chat.failed') if r.get('request_id') == run_id]
    assert failed and failed[-1]['why'] == 'ServiceError'
    assert 'Codex could not check the account' in failed[-1]['detail']
    assert published and published[-1]['request_id'] == run_id


def test_worker_whose_app_server_never_answers_the_account_check_ends_as_stalled(daemon, monkeypatch, tmp_path):
    """A worker's Codex runtime has no startup timeout; the stall watchdog (stall_s) covers its startup too, so a
    silent account check ends the attempt as a named stall instead of hanging until Stop."""
    import time
    servers = _codex_behind_stub(monkeypatch, tmp_path, 'silent')
    s = daemon.daemon.supervisor
    s.activity.now = time.monotonic                     # real seconds for liveness in this test
    s.stall_s, s.heartbeat_poll_s = 0.3, 0.05            # test speed only: production uses STALL_S
    install(daemon, [sup('task', 'Checking the report.', task('check the report'))])
    scripted = s.make_runtime

    def runtimes(role, *, mode, tool_names, schema):
        if role.startswith('worker:'):
            return CodexRuntime(Path('/fake/codex'), 'worker', tmp_path / 'work', tmp_path, shim_cmd=['/fake/shim'],
                                tool_names=tool_names, output_schema=schema, model=MODEL, timeout_s=None)
        return scripted(role, mode=mode, tool_names=tool_names, schema=schema)
    s.make_runtime = runtimes
    turn: dict = {}
    chatting = threading.Thread(target=lambda: turn.update(events=chat(daemon, 'check the report')), daemon=True)
    chatting.start()
    chatting.join(20)                                   # chat streams have no socket timeout: bound the wait here
    assert 'events' in turn, 'the worker was still starting: its silent account check hung the attempt'
    out = outcome(turn['events'])
    assert out['status'] == 'failed' and 'stopped responding' in out['subtasks']['worker']['summary'], out   # L11 plain words
    assert servers and all(proc.returncode is not None for proc in servers)
    assert not any({'thread/start', 'turn/start'} & set(proc.methods) for proc in servers)


def test_an_account_check_that_fails_while_the_mac_has_no_network_says_its_offline(daemon, monkeypatch):
    """Owner live use: with no network, Codex's account check failed and the reply said "Try again shortly"."""
    import ferb.sense.network as network
    from ferb.runtimes.codex_stream import AccountError
    from tests.conftest import FakeRuntime
    install(daemon, [])

    async def unavailable(self, task_, request, token):
        raise AccountError('account_unavailable', -32000)
    monkeypatch.setattr(FakeRuntime, 'start', unavailable)
    monkeypatch.setattr(network, 'has_route', lambda: False)
    text = chat(daemon, 'what time is it?')[0]['text']
    assert 'seems to be offline' in text and 'Try again shortly' not in text
    monkeypatch.setattr(network, 'has_route', lambda: True)             # online: the service's own words, as before
    assert 'Codex could not check the account' in chat(daemon, 'what time is it?')[0]['text']


def test_a_login_problem_is_never_called_offline(daemon, monkeypatch):
    import ferb.sense.network as network
    from ferb.runtimes.codex_stream import AccountError
    from tests.conftest import FakeRuntime
    install(daemon, [])

    async def signed_out(self, task_, request, token):
        raise AccountError('auth_required')
    monkeypatch.setattr(FakeRuntime, 'start', signed_out)
    monkeypatch.setattr(network, 'has_route', lambda: False)
    assert 'offline' not in chat(daemon, 'what time is it?')[0]['text']


def test_the_route_fact_reads_the_routing_table_without_sending_anything() -> None:
    import errno
    import socket
    from ferb.sense import network

    class NoRoute:
        def __init__(self, family: int, kind: int) -> None:
            pass
        def connect(self, address: Any) -> None:
            raise OSError(errno.ENETUNREACH, "Network is unreachable")
        def close(self) -> None:
            pass
    assert network._route(NoRoute) is False
    class Route(NoRoute):
        def connect(self, address: Any) -> None:
            pass
    assert network._route(Route) is True
    assert socket.SOCK_DGRAM                              # UDP connect: a route lookup, no packet leaves
