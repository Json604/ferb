"""L21-L23: time waits fire on wall-clock time after Mac sleep; the plan inventory walk leaves the daemon's loop
free; a Codex account check that fails in any way still gives its request a chat reply naming the failure."""
import asyncio
import json
import threading
from datetime import datetime, timezone
from pathlib import Path

import pytest

from ferb.cli.client import Client
from ferb.orchestrator.plan_executor import PlanExecutor
from ferb.orchestrator.plan_store import PlanStore
from ferb.orchestrator.plans import Plan, Step
from ferb.runtimes.codex_exec import CodexRuntime
from ferb.store.db import Database
from tests.conftest import chat, done, install, outcome, sup, task


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

class _Lines:
    def __init__(self):
        self.queue: asyncio.Queue = asyncio.Queue()

    async def readline(self):
        return await self.queue.get()

    def __aiter__(self):
        return self

    async def __anext__(self):
        line = await self.queue.get()
        if not line:
            raise StopAsyncIteration
        return line


class AppServer:
    """A Codex app-server stub behind FERB's real Codex adapter: it answers initialize, then the account check
    fails as told. No thread or model turn is ever started."""

    def __init__(self, check):
        self.check = check
        self.stdout, self.stderr = _Lines(), _Lines()
        self.stdin, self.returncode, self.methods = self, None, []
        self.exited = asyncio.Event()

    def write(self, data):
        message = json.loads(data)
        self.methods.append(message['method'])
        if message['method'] == 'initialize':
            self._say({'id': message['id'], 'result': {}})
        elif message['method'] == 'account/read':
            if self.check == 'error':                  # the shape 5b10afb named: an RPC error response
                self._say({'id': message['id'], 'error': {'code': -32000, 'message': 'provider-private detail'}})
            elif self.check == 'exits':                # the app-server ends before answering
                self.kill()
            # 'silent': never answers; the adapter's own startup timeout ends the check

    async def drain(self):
        pass

    def _say(self, obj):
        self.stdout.queue.put_nowait(json.dumps(obj).encode() + b'\n')

    def kill(self):
        if self.returncode is None:
            self.returncode = -9
            self.stdout.queue.put_nowait(b'')
            self.stderr.queue.put_nowait(b'')
            self.exited.set()

    async def wait(self):
        await self.exited.wait()
        return self.returncode


@pytest.mark.parametrize('check', ['error', 'exits', 'silent'])
def test_account_check_failure_gets_a_named_reply_for_its_request(daemon, monkeypatch, tmp_path, check):
    import ferb.runtimes.codex_stream as stream
    (tmp_path / 'codex-home').mkdir()
    (tmp_path / 'codex-home' / 'auth.json').write_text('{"tokens":{"refresh_token":"test-only-fixture"}}')
    servers: list[AppServer] = []
    real_spawn = asyncio.create_subprocess_exec

    async def spawn(*cmd, **kwargs):
        if cmd and cmd[0] == '/fake/codex':
            servers.append(AppServer(check))
            return servers[-1]
        return await real_spawn(*cmd, **kwargs)
    monkeypatch.setattr(asyncio, 'create_subprocess_exec', spawn)

    async def adapter_start(self, task_, request, run_token):   # the real adapter; only the process is a stub
        return await stream.start(self, task_, request, run_token)
    monkeypatch.setattr(CodexRuntime, 'start', adapter_start)
    daemon.daemon.supervisor.make_runtime = lambda role, *, mode, tool_names, schema: CodexRuntime(
        Path('/fake/codex'), 'supervisor', tmp_path / 'work', tmp_path, output_schema=schema, model='test-model',
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

    async def failures():
        return [e.data for e in daemon.daemon._journal().entries() if e.kind == 'chat.failed']
    recorded = daemon.run(failures())
    assert recorded[-1]['request_id'] == ack['request_id'] and recorded[-1]['why'] == 'ServiceError'
