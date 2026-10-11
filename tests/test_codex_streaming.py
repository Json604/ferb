"""Pinned app-server protocol fixtures; no Codex/model process is started."""
import asyncio
import json
from pathlib import Path

import pytest

from ferb.core.types import Identity, ModelRequest, Task
from ferb.runtimes.codex_exec import CodexRuntime

CODEX_START = CodexRuntime.start


class Pipe:
    def __init__(self):
        self.queue = asyncio.Queue()

    def put(self, obj):
        self.queue.put_nowait(json.dumps(obj).encode() + b'\n')

    async def readline(self):
        return await self.queue.get()

    def __aiter__(self):
        return self

    async def __anext__(self):
        value = await self.readline()
        if not value:
            raise StopAsyncIteration
        return value


class Process:
    def __init__(self, account='chatgpt', tools=None, phase='final_answer', fail=False, catalog=True):
        self.stdout, self.stderr = Pipe(), Pipe()
        self.stdin = self
        self.returncode = None
        self.requests = []
        self.account, self.phase, self.fail = account, phase, fail
        self.tools = tools if tools is not None else [{'type': 'function', 'name': 'apply_patch'}]
        self.finished = asyncio.Event()
        self.catalog = catalog

    def write(self, data):
        msg = json.loads(data)
        self.requests.append(msg)
        method, ident = msg['method'], msg.get('id')
        if method == 'initialize':
            self.stdout.put({'id': ident, 'result': {}})
        elif method == 'account/read':
            self.stdout.put({'id': ident, 'result': {'account': {'type': self.account}}})
        elif method == 'thread/start':
            self.stdout.put({'id': ident, 'result': {'thread': {'id': 'th', 'ephemeral': True},
                'model': 'test-model', 'modelProvider': 'openai', 'approvalPolicy': 'never',
                'sandbox': {'type': 'readOnly', 'networkAccess': False}, 'instructionSources': []}})
        elif method == 'turn/start':
            if self.catalog:
                self.stderr.queue.put_nowait(('TRACE {"tools":' + json.dumps(self.tools, separators=(',', ':')) + '}\n').encode())
            self.stdout.put({'id': ident, 'result': {'turn': {'id': 'tu', 'status': 'inProgress'}}})
            self.notify('item/started', {'item': {'type': 'agentMessage', 'id': 'a', 'phase': self.phase, 'text': ''}})
            self.notify('item/agentMessage/delta', {'itemId': 'a', 'delta': '{"text":"Hello '})
            self.notify('item/agentMessage/delta', {'itemId': 'a', 'delta': 'there"}'})

    def complete(self):
        self.notify('item/completed', {'item': {'type': 'agentMessage', 'id': 'a', 'phase': self.phase,
                                               'text': '{"text":"Hello there"}'}})
        self.notify('thread/tokenUsage/updated', {'tokenUsage': {'last': {'inputTokens': 8,
                    'cachedInputTokens': 2, 'outputTokens': 4, 'reasoningOutputTokens': 1}}})
        self.stdout.put({'method': 'turn/completed', 'params': {'threadId': 'th', 'turn': {
            'id': 'tu', 'status': 'failed' if self.fail else 'completed', 'error': {'message': 'interrupted'} if self.fail else None}}})

    def notify(self, method, params):
        self.stdout.put({'method': method, 'params': {'threadId': 'th', 'turnId': 'tu', **params}})

    async def drain(self):
        pass

    def kill(self):
        if self.returncode is None:
            self.returncode = -9
            self.stdout.queue.put_nowait(b'')
            self.stderr.queue.put_nowait(b'')
            self.finished.set()

    async def wait(self):
        await self.finished.wait()
        return self.returncode


def runtime(tmp_path):
    home = tmp_path / 'codex-home'
    home.mkdir()
    (home / 'auth.json').write_text('{"tokens":{"refresh_token":"test-only-fixture"}}')
    return CodexRuntime(Path('/fake/codex'), 'supervisor', tmp_path / 'work', tmp_path,
                        output_schema={'type': 'object', 'properties': {'text': {'type': 'string'}}},
                        model='test-model', timeout_s=None)


async def begin(tmp_path, monkeypatch, proc):
    import ferb.runtimes.codex_stream as stream
    seen = {}
    async def spawn(*cmd, **kw):
        seen.update(cmd=cmd, **kw)
        return proc
    monkeypatch.setattr(asyncio, 'create_subprocess_exec', spawn)
    rt = runtime(tmp_path)
    task = Task('t', None, 'reply', 'd', Identity.OWNER, set(), 'mixed', 'r', [], None)
    run = await stream.start(rt, task, ModelRequest('FERB instructions', ('context',)))
    return stream, rt, run, seen


@pytest.mark.anyio
async def test_final_answer_deltas_arrive_before_turn_completion(tmp_path, monkeypatch):
    proc = Process()
    stream, rt, run, seen = await begin(tmp_path, monkeypatch, proc)
    events = stream.events(rt, run)
    first = await anext(events)
    assert first.kind == 'text_delta' and first.data == {'text': '{"text":"Hello '}
    second = await anext(events)
    assert second.kind == 'text_delta' and second.data == {'text': 'there"}'}
    proc.complete()
    rest = [ev async for ev in events]
    assert rest[-1].kind == 'final' and rest[-1].data['text'] == '{"text":"Hello there"}'
    assert rest[-1].data['tools_seen'] == ['apply_patch']
    assert any(ev.kind == 'usage' and ev.data['input_tokens'] == 8 for ev in rest)
    assert not Path(seen['env']['CODEX_HOME']).exists()
    assert seen['env']['HOME'] == str(tmp_path / 'codex-user')
    assert '--strict-config' in seen['cmd']
    thread = next(m['params'] for m in proc.requests if m['method'] == 'thread/start')
    assert thread['ephemeral'] and thread['dynamicTools'] == [] and thread['environments'] == []
    assert thread['baseInstructions'] == 'FERB instructions'


@pytest.mark.anyio
@pytest.mark.parametrize('phase', ['commentary', None, 'unexpected_phase'])
async def test_commentary_and_unknown_phase_never_become_deltas(tmp_path, monkeypatch, phase):
    proc = Process(phase=phase)
    stream, rt, run, _ = await begin(tmp_path, monkeypatch, proc)
    proc.complete()
    got = [ev async for ev in stream.events(rt, run)]
    assert not any(ev.kind == 'text_delta' for ev in got)
    assert got[-1].kind == ('final' if phase is None else 'error')


@pytest.mark.anyio
async def test_unexpected_actual_tool_catalog_blocks_text_and_final(tmp_path, monkeypatch):
    proc = Process(tools=[{'type': 'function', 'name': 'shell'}])
    stream, rt, run, _ = await begin(tmp_path, monkeypatch, proc)
    proc.complete()
    got = [ev async for ev in stream.events(rt, run)]
    assert any(ev.kind == 'violation' for ev in got)
    assert not any(ev.kind in {'text_delta', 'final'} for ev in got)


@pytest.mark.anyio
async def test_non_subscription_auth_is_rejected_before_turn(tmp_path, monkeypatch):
    proc = Process(account='apiKey')
    with pytest.raises(ValueError, match='subscription'):
        await begin(tmp_path, monkeypatch, proc)
    assert proc.returncode is not None
    assert not any(m['method'] == 'turn/start' for m in proc.requests)


@pytest.mark.anyio
async def test_failed_turn_does_not_emit_final(tmp_path, monkeypatch):
    proc = Process(fail=True)
    stream, rt, run, _ = await begin(tmp_path, monkeypatch, proc)
    proc.complete()
    got = [ev async for ev in stream.events(rt, run)]
    assert got[-1].kind == 'error'
    assert not any(ev.kind == 'final' for ev in got)


@pytest.mark.anyio
@pytest.mark.parametrize('concurrent', [False, True])
async def test_cancel_cleans_up_and_keeps_refreshed_auth_without_overwriting_newer_auth(tmp_path, monkeypatch, concurrent):
    proc = Process()
    stream, rt, run, seen = await begin(tmp_path, monkeypatch, proc)
    cloned = Path(seen['env']['CODEX_HOME']) / 'auth.json'
    changed = '{"tokens":{"refresh_token":"refreshed-fixture"}}'
    cloned.write_text(changed)
    original = tmp_path / 'codex-home' / 'auth.json'
    if concurrent:
        original.write_text('{"tokens":{"refresh_token":"newer-fixture"}}')
    await stream.cancel(run)
    assert original.read_text() == ('{"tokens":{"refresh_token":"newer-fixture"}}' if concurrent else changed)
    assert not cloned.parent.exists()
    assert proc.returncode is not None


@pytest.mark.anyio
async def test_missing_catalog_never_emits_a_delta_or_final(tmp_path, monkeypatch):
    proc = Process(catalog=False)
    stream, rt, run, _ = await begin(tmp_path, monkeypatch, proc)
    # Simulate a transport that never provides the required request catalog.
    proc.stderr.queue.put_nowait(b'')
    proc.complete()
    got = [ev async for ev in stream.events(rt, run)]
    assert got[-1].kind == 'error'
    assert not any(ev.kind in {'text_delta', 'final'} for ev in got)


@pytest.mark.anyio
async def test_late_catalog_violation_is_drained_before_final(tmp_path, monkeypatch):
    proc = Process()
    stream, rt, run, _ = await begin(tmp_path, monkeypatch, proc)
    events = stream.events(rt, run)
    assert (await anext(events)).kind == 'text_delta'
    assert (await anext(events)).kind == 'text_delta'
    proc.complete()
    proc.stderr.queue.put_nowait(b'TRACE {"tools":[{"type":"function","name":"shell"}]}\n')
    got = [ev async for ev in events]
    assert got[-1].kind == 'violation'
    assert not any(ev.kind == 'final' for ev in got)


@pytest.mark.anyio
async def test_codex_runtime_routes_schema_supervisors_to_app_server(tmp_path, monkeypatch):
    import ferb.runtimes.codex_exec as codex
    import ferb.runtimes.codex_stream as stream
    seen = []
    async def version(binary): pass
    async def start(rt, task, request, run_token):
        seen.append(rt.mode)
        return 'app-server-handle'
    monkeypatch.setattr(codex, 'check_version', version)
    monkeypatch.setattr(stream, 'start', start)
    rt = runtime(tmp_path)
    t = Task('t', None, 'reply', 'd', Identity.OWNER, set(), 'mixed', 'r', [], None)
    assert await CODEX_START(rt, t, ModelRequest('system', ('context',)), '') == 'app-server-handle'
    assert seen == ['supervisor']


@pytest.mark.anyio
async def test_missing_catalog_with_open_stderr_reaches_completed_and_fails_promptly(tmp_path, monkeypatch):
    proc = Process(catalog=False)
    stream, rt, run, _ = await begin(tmp_path, monkeypatch, proc)
    # The app-server stays alive, with stderr open. Reading completion must reap it rather than waiting forever
    # for the catalog that this protocol fixture deliberately omitted. This is only a test deadline.
    proc.complete()
    async with asyncio.timeout(1):
        got = [ev async for ev in stream.events(rt, run)]
    assert got[-1].kind == 'error'
    assert 'catalog' in got[-1].data['message']
    assert not any(ev.kind in {'text_delta', 'final'} for ev in got)
    assert proc.returncode is not None

class SteeringProcess(Process):
    def __init__(self):
        super().__init__(tools=[{'type': 'function', 'name': 'mcp__ferb.mac_read'}])
        self.steers = asyncio.Queue()

    def write(self, data):
        msg = json.loads(data)
        if msg.get('method') == 'turn/steer':
            self.requests.append(msg)
            self.steers.put_nowait(msg)
        else:
            super().write(data)


async def begin_worker(tmp_path, monkeypatch, proc):
    import ferb.runtimes.codex_exec as codex
    seen = {}
    async def spawn(*cmd, **kw):
        seen.update(cmd=cmd, **kw)
        return proc
    async def version(binary): pass
    monkeypatch.setattr(asyncio, 'create_subprocess_exec', spawn)
    monkeypatch.setattr(codex, 'check_version', version)
    rt = runtime(tmp_path)
    rt.mode, rt.shim_cmd = 'worker', ['ferb', 'mcp']
    rt.allowed = rt.allowed | {'mcp__ferb.mac_read'}
    task = Task('t', None, 'food', 'd', Identity.OWNER, set(), 'mixed', 'r', [], None)
    run = await CODEX_START(rt, task, ModelRequest('FERB instructions', ('context',)), 'private-token')
    return rt, run, seen


@pytest.mark.anyio
async def test_worker_steers_same_turn_while_events_are_read_without_restarting(tmp_path, monkeypatch):
    proc = SteeringProcess()
    rt, run, seen = await begin_worker(tmp_path, monkeypatch, proc)
    assert run.state.get('app_server')
    events = rt.events(run)
    # Existing worker tools remain authenticated; the raw token never enters argv.
    assert 'private-token' not in ' '.join(seen['cmd'])
    token = run.state['token_file']
    assert token.read_text() == 'private-token' and token.stat().st_mode & 0o777 == 0o600
    assert 'model_reasoning_effort="low"' not in seen['cmd']
    assert 'effort' not in next(m['params'] for m in proc.requests if m['method'] == 'turn/start')
    first = asyncio.create_task(anext(events))
    steering = asyncio.create_task(rt.continue_(run, 'Keep it under ₹300'))
    msg = await asyncio.wait_for(proc.steers.get(), 1)
    assert msg['params'] == {'threadId': 'th', 'expectedTurnId': 'tu',
                            'input': [{'type': 'text', 'text': 'Keep it under ₹300'}]}
    proc.stdout.put({'id': msg['id'], 'result': {'turnId': 'tu'}})
    await asyncio.wait_for(steering, 1)
    assert proc.returncode is None
    assert (await first).kind == 'text_delta'
    proc.notify('item/completed', {'item': {'id': 'm', 'type': 'mcpToolCall', 'server': 'ferb',
        'tool': 'mac_read', 'arguments': {}, 'status': 'completed'}})
    proc.complete()
    rest = [event async for event in events]
    assert any(e.kind == 'tool_call' and e.data['tool'] == 'mac_read' for e in rest)
    assert rest[-1].kind == 'final'
    assert sum(m['method'] == 'turn/start' for m in proc.requests) == 1
    assert not token.exists()


@pytest.mark.anyio
@pytest.mark.parametrize('close', ['completed', 'disconnect', 'cancel', 'rejected', 'malformed'])
async def test_worker_steer_completion_and_transport_races_never_claim_acceptance(tmp_path, monkeypatch, close):
    from ferb.core.errors import Unsupported
    proc = SteeringProcess()
    rt, run, _ = await begin_worker(tmp_path, monkeypatch, proc)
    steering = asyncio.create_task(rt.continue_(run, 'Only vegetarian'))
    msg = await asyncio.wait_for(proc.steers.get(), 1)
    if close == 'completed':
        proc.complete()
    elif close == 'disconnect':
        proc.kill()
    elif close == 'cancel':
        await rt.cancel(run)
    elif close == 'malformed':
        proc.stdout.put({'id': ['invalid-id'], 'result': {}})
    else:
        proc.stdout.put({'id': msg['id'], 'error': {'code': -32600, 'message': 'turn mismatch'}})
    with pytest.raises(Unsupported):
        await asyncio.wait_for(steering, 1)
    await rt.cancel(run)


@pytest.mark.anyio
async def test_concurrent_steers_match_rpc_ids_not_notification_order(tmp_path, monkeypatch):
    proc = SteeringProcess()
    rt, run, _ = await begin_worker(tmp_path, monkeypatch, proc)
    steers = [asyncio.create_task(rt.continue_(run, text)) for text in ('under ₹300', 'two breads')]
    first, second = [await asyncio.wait_for(proc.steers.get(), 1) for _ in range(2)]
    for msg in (second, first):
        proc.notify('item/agentMessage/delta', {'itemId': 'a', 'delta': ''})
        proc.stdout.put({'id': msg['id'], 'result': {'turnId': 'tu'}})
    await asyncio.wait_for(asyncio.gather(*steers), 1)
    assert proc.returncode is None
    await rt.cancel(run)


@pytest.mark.anyio
@pytest.mark.parametrize('item', [
    {'id': 'm', 'type': 'mcpToolCall', 'server': 'untrusted', 'tool': 'mac_read'},
    {'id': 'm', 'type': 'mcpToolCall', 'server': 'ferb', 'tool': 'not_registered'},
    {'id': 'm', 'type': 'commandExecution', 'command': 'touch /tmp/not-permitted'},
])
async def test_worker_only_accepts_registered_ferb_mcp_calls(tmp_path, monkeypatch, item):
    proc = SteeringProcess()
    rt, run, _ = await begin_worker(tmp_path, monkeypatch, proc)
    proc.notify('item/started', {'item': item})
    proc.complete()
    got = [event async for event in rt.events(run)]
    assert got[-1].kind == 'violation'
    assert not any(event.kind == 'final' for event in got)
    assert proc.returncode is not None
    assert not run.state['token_file'].exists()


@pytest.mark.anyio
async def test_worker_completion_validates_schema_and_closed_run_rejects_steering(tmp_path, monkeypatch):
    from ferb.core.errors import Unsupported
    proc = SteeringProcess()
    rt, run, _ = await begin_worker(tmp_path, monkeypatch, proc)
    rt.output_schema = {'type': 'object', 'required': ['missing']}
    proc.complete()
    got = [event async for event in rt.events(run)]
    assert got[-1].kind == 'error' and 'schema' in got[-1].data['message']
    with pytest.raises(Unsupported):
        await rt.continue_(run, 'new correction')
    assert not any(m['method'] == 'turn/steer' for m in proc.requests)
    assert rt.capabilities.continuation and rt.capabilities.live_messaging
