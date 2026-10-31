"""Pinned app-server protocol fixtures; no Codex/model process is started."""
import asyncio
import json
from pathlib import Path

import pytest

from ferb.core.types import Identity, ModelRequest, Task
from ferb.runtimes.codex_exec import CodexRuntime
from tests.support.runtime_fakes import Process

CODEX_START = CodexRuntime.start


def runtime(tmp_path, **kw):
    home = tmp_path / 'codex-home'
    home.mkdir()
    (home / 'auth.json').write_text('{"tokens":{"refresh_token":"test-only-fixture"}}')
    return CodexRuntime(Path('/fake/codex'), 'supervisor', tmp_path,
                        output_schema={'type': 'object', 'properties': {'text': {'type': 'string'}}},
                        model='test-model', timeout_s=None, **kw)


async def begin(tmp_path, monkeypatch, proc, context='context', **kw):
    import ferb.runtimes.codex_stream as stream
    seen = {}
    async def spawn(*cmd, **kw):
        seen.update(cmd=cmd, **kw)
        return proc
    monkeypatch.setattr(asyncio, 'create_subprocess_exec', spawn)
    rt = runtime(tmp_path, **kw)
    task = Task('t', None, 'reply', 'd', Identity.OWNER, set(), 'mixed', 'r', [], None)
    run = await stream.start(rt, task, ModelRequest('FERB instructions', (context,)))
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
    assert 'interrupted' in got[-1].data['message']          # the failed turn's own error, as Codex wrote it
    assert not any(ev.kind == 'final' for ev in got)


INPUT_CAP_ERROR = {'code': -32602, 'message': 'Input exceeds the maximum length of 1048576 characters.',
                   'data': {'input_error_code': 'input_too_large', 'max_chars': 1048576, 'actual_chars': 1100000}}


@pytest.mark.anyio
async def test_a_rejected_turn_start_carries_codexs_own_error_and_code_never_the_request(tmp_path, monkeypatch):
    """Live 2026-10-11 to 10-20: 210 learning jobs failed as a bare "Codex could not complete turn/start"; Codex's own
    reason (its app-server input cap) was dropped. The error now carries Codex's message and code, never what FERB
    sent (L28, D32)."""
    import ferb.runtimes.codex_stream as stream
    proc = Process(errors={'turn/start': INPUT_CAP_ERROR})
    with pytest.raises(stream.ProtocolError) as caught:
        await begin(tmp_path, monkeypatch, proc, context='REQUEST-CONTENT-MARKER')
    text = str(caught.value)
    assert 'turn/start' in text and 'Input exceeds the maximum length of 1048576 characters.' in text
    assert '-32602' in text and 'input_too_large' in text and '1100000' in text
    assert 'REQUEST-CONTENT-MARKER' not in text and 'FERB instructions' not in text
    assert proc.returncode is not None                       # the run is closed


@pytest.mark.anyio
async def test_codex_error_text_passes_through_ferb_redaction(tmp_path, monkeypatch):
    """Rule 14: Codex's error text reaches learning_jobs, spans and `ferb trace` only after FERB's secret redaction:
    the pattern classes by default, and the daemon's own redactor (its registered values) when it builds the runtime."""
    import ferb.runtimes.codex_stream as stream
    from ferb.sanitize.redact import Redactor
    proc = Process(errors={'thread/start': {'code': -32603,
                                            'message': 'upstream refused Bearer abcdefghijklmnop0123456789'}})
    with pytest.raises(stream.ProtocolError) as caught:
        await begin(tmp_path, monkeypatch, proc)
    assert 'thread/start' in str(caught.value) and 'upstream refused' in str(caught.value)
    assert 'abcdefghijklmnop0123456789' not in str(caught.value)
    redact = Redactor()
    redact.add_secret_value('owner-stored-value-1234')
    proc = Process(errors={'turn/start': {'code': -32603, 'message': 'echoed owner-stored-value-1234'}})
    (tmp_path / 'second').mkdir()
    with pytest.raises(stream.ProtocolError) as caught:
        await begin(tmp_path / 'second', monkeypatch, proc, redact=redact)
    assert 'echoed' in str(caught.value) and 'owner-stored-value-1234' not in str(caught.value)


@pytest.mark.anyio
async def test_a_turn_error_event_carries_codexs_message(tmp_path, monkeypatch):
    proc = Process()
    stream, rt, run, _ = await begin(tmp_path, monkeypatch, proc)
    proc.notify('error', {'error': {'message': 'stream disconnected before completion',
                                    'codexErrorInfo': 'responseStreamDisconnected'}, 'willRetry': False})
    got = [ev async for ev in stream.events(rt, run)]
    assert got[-1].kind == 'error'
    assert 'stream disconnected before completion' in got[-1].data['message']
    assert 'responseStreamDisconnected' in got[-1].data['message']
    assert 'overflow' not in got[-1].data


@pytest.mark.anyio
async def test_codexs_context_window_exceeded_error_is_marked_as_an_overflow(tmp_path, monkeypatch):
    """Codex's app-server CodexErrorInfo `contextWindowExceeded` (v2 protocol): the run's model window overflowed.
    Marked so the checker can run once more without its optional recorded calls (fix wave 6b round 1)."""
    proc = Process()
    stream, rt, run, _ = await begin(tmp_path, monkeypatch, proc)
    proc.notify('error', {'error': {'message': 'Codex ran out of room in the model\'s context window.',
                                    'codexErrorInfo': 'contextWindowExceeded'}, 'willRetry': False})
    got = [ev async for ev in stream.events(rt, run)]
    assert got[-1].kind == 'error' and got[-1].data.get('overflow') is True
    assert 'contextWindowExceeded' in got[-1].data['message']


@pytest.mark.anyio
async def test_an_error_codex_retries_itself_is_reported_and_the_run_goes_on(tmp_path, monkeypatch):
    """Codex's v2 ErrorNotification: willRetry true "will not interrupt a turn" (app-server-protocol
    v2/notification.rs:62-69, rust-v0.157.1). FERB reports it (redacted) and keeps the run; only a non-retrying error
    or a failed turn ends it."""
    proc = Process()
    stream, rt, run, _ = await begin(tmp_path, monkeypatch, proc)
    proc.notify('error', {'error': {'message': 'stream disconnected, retrying 1/5 Bearer abcdefghijklmnop0123456789',
                                    'codexErrorInfo': 'responseStreamDisconnected'}, 'willRetry': True})
    proc.complete()
    got = [ev async for ev in stream.events(rt, run)]
    retrying = [ev for ev in got if ev.kind == 'retrying']
    assert len(retrying) == 1 and 'stream disconnected, retrying 1/5' in retrying[0].data['message']
    assert 'responseStreamDisconnected' in retrying[0].data['message']
    assert 'abcdefghijklmnop0123456789' not in retrying[0].data['message']
    assert got[-1].kind == 'final' and not any(ev.kind == 'error' for ev in got)


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
async def test_worker_answer_is_checked_once_by_ferb_and_closed_run_rejects_steering(tmp_path, monkeypatch):
    from ferb.core.errors import Unsupported
    from ferb.core.structured import check_final
    proc = SteeringProcess()
    rt, run, _ = await begin_worker(tmp_path, monkeypatch, proc)
    rt.output_schema = {'type': 'object', 'required': ['missing']}
    proc.complete()
    got = [event async for event in rt.events(run)]
    # the adapter hands the answer on; FERB's one check (core/structured.check_final, run by every caller) finds it
    # outside the schema, which the caller treats as format repair (L12 fix round 3)
    assert got[-1].kind == 'final' and not any(event.kind == 'error' for event in got)
    with pytest.raises(ValueError, match='schema'):
        check_final(got[-1].data['text'], rt.output_schema)
    with pytest.raises(Unsupported):
        await rt.continue_(run, 'new correction')
    assert not any(m['method'] == 'turn/steer' for m in proc.requests)

class AccountProcess(Process):
    def __init__(self, error=None, hold=False):
        super().__init__()
        self.error, self.hold = error, hold
        self.account_requested = asyncio.Event()
        self.account_id = None
        self.auth_path = None
        self.refresh = None

    def write(self, data):
        message = json.loads(data)
        if message['method'] != 'account/read':
            return super().write(data)
        self.requests.append(message)
        self.account_id = message['id']
        self.account_requested.set()
        if not self.hold:
            self.answer_account()

    def answer_account(self):
        if self.refresh is not None:
            self.auth_path.write_text(self.refresh)
        result = {'error': self.error} if self.error is not None else {'result': {'account': {'type': 'chatgpt'}}}
        self.stdout.put({'id': self.account_id, **result})


@pytest.mark.anyio
@pytest.mark.parametrize('marker,reason', [
    ('refresh_token_reused', 'auth_required'), ('refresh_token_expired', 'auth_required'),
    ('refresh_token_invalidated', 'auth_required'), ('invalid_grant', 'auth_required'),
    ('upstream temporarily unavailable', 'account_unavailable'),
])
async def test_account_failure_is_actionable_without_provider_secrets_or_starting_turn(tmp_path, monkeypatch, marker, reason):
    import ferb.runtimes.codex_stream as stream
    proc = AccountProcess(error={'code': -32000, 'message': marker + ' secret-provider-payload',
                                 'data': {'credential': 'secret-provider-payload'}})
    with pytest.raises(stream.AccountError) as caught:
        await begin(tmp_path, monkeypatch, proc)
    assert caught.value.reason == reason and caught.value.code == -32000
    assert 'secret-provider-payload' not in str(caught.value)
    assert ('sign in to ChatGPT in Settings → AI' in str(caught.value)) == (reason == 'auth_required')
    assert '`ferb' not in str(caught.value)                       # app words: no command
    assert not any(m['method'] in {'thread/start', 'turn/start'} for m in proc.requests)
    assert proc.returncode is not None


@pytest.mark.anyio
async def test_refresh_published_before_next_start_and_again_after_turn(tmp_path, monkeypatch):
    import ferb.runtimes.codex_stream as stream
    rt = runtime(tmp_path)
    source = tmp_path / 'codex-home' / 'auth.json'
    first, second = AccountProcess(hold=True), AccountProcess()
    first.refresh = '{"tokens":{"refresh_token":"first-refresh-fixture"}}'
    processes, copies = [], []
    async def spawn(*cmd, **kw):
        proc = first if not processes else second
        proc.auth_path = Path(kw['env']['CODEX_HOME']) / 'auth.json'
        copies.append(proc.auth_path.read_text())
        processes.append(proc)
        return proc
    monkeypatch.setattr(asyncio, 'create_subprocess_exec', spawn)
    task = Task('t', None, 'reply', 'd', Identity.OWNER, set(), 'mixed', 'r', [], None)
    request = ModelRequest('instructions', ('context',))
    initial = asyncio.create_task(stream.start(rt, task, request))
    await asyncio.wait_for(first.account_requested.wait(), 1)
    first.answer_account()
    run1 = await asyncio.wait_for(initial, 1)
    run2 = await asyncio.wait_for(stream.start(rt, task, request), 1)
    assert copies[1] == first.refresh and source.read_text() == first.refresh
    assert first.returncode is None  # Refresh is shared while the first model turn remains active.
    later = '{"tokens":{"refresh_token":"second-refresh-fixture"}}'
    first.auth_path.write_text(later)
    await stream.cancel(run1)
    await stream.cancel(run2)
    assert source.read_text() == later


@pytest.mark.anyio
async def test_stalled_account_start_does_not_block_another_start(tmp_path, monkeypatch):
    import ferb.runtimes.codex_stream as stream
    rt = runtime(tmp_path)
    first, second = AccountProcess(hold=True), AccountProcess()
    processes = []
    async def spawn(*cmd, **kw):
        proc = first if not processes else second
        processes.append(proc)
        return proc
    monkeypatch.setattr(asyncio, 'create_subprocess_exec', spawn)
    task = Task('t', None, 'reply', 'd', Identity.OWNER, set(), 'mixed', 'r', [], None)
    request = ModelRequest('instructions', ('context',))
    initial = asyncio.create_task(stream.start(rt, task, request))
    await asyncio.wait_for(first.account_requested.wait(), 1)
    run = await asyncio.wait_for(stream.start(rt, task, request), 1)
    assert not initial.done() and first.returncode is None
    initial.cancel()
    with pytest.raises(asyncio.CancelledError):
        await initial
    await stream.cancel(run)
    assert first.returncode is not None and second.returncode is not None

@pytest.mark.anyio
@pytest.mark.parametrize('account', [None, {}, {'type': 'apiKey'}])
async def test_missing_or_wrong_account_explains_subscription_login(tmp_path, monkeypatch, account):
    import ferb.runtimes.codex_stream as stream
    class MissingAccount(Process):
        def write(self, data):
            message = json.loads(data)
            if message['method'] == 'account/read':
                self.requests.append(message)
                self.stdout.put({'id': message['id'], 'result': {'account': account}})
            else:
                super().write(data)
    proc = MissingAccount()
    with pytest.raises(stream.AccountError, match='sign in to ChatGPT in Settings') as caught:
        await begin(tmp_path, monkeypatch, proc)
    assert caught.value.reason == 'auth_required'
    assert not any(m['method'] in {'thread/start', 'turn/start'} for m in proc.requests)


@pytest.mark.anyio
async def test_early_refresh_does_not_overwrite_owner_login_or_adopt_its_baseline(tmp_path, monkeypatch):
    import ferb.runtimes.codex_stream as stream
    rt = runtime(tmp_path)
    source = tmp_path / 'codex-home' / 'auth.json'
    original = source.read_bytes()
    proc = AccountProcess(hold=True)
    proc.refresh = '{"tokens":{"refresh_token":"old-run-refresh-fixture"}}'
    async def spawn(*cmd, **kw):
        proc.auth_path = Path(kw['env']['CODEX_HOME']) / 'auth.json'
        return proc
    monkeypatch.setattr(asyncio, 'create_subprocess_exec', spawn)
    task = Task('t', None, 'reply', 'd', Identity.OWNER, set(), 'mixed', 'r', [], None)
    starting = asyncio.create_task(stream.start(rt, task, ModelRequest('instructions', ('context',))))
    await asyncio.wait_for(proc.account_requested.wait(), 1)
    owner_login = '{"tokens":{"refresh_token":"owner-new-login-fixture"}}'
    source.write_text(owner_login)
    proc.answer_account()
    run = await asyncio.wait_for(starting, 1)
    assert source.read_text() == owner_login and run.state['auth_original'] == original
    await stream.cancel(run)
    assert source.read_text() == owner_login


def test_account_error_never_retains_a_non_numeric_provider_code():
    from ferb.runtimes.codex_stream import _account_error
    error = _account_error({'code': 'secret-provider-payload', 'message': 'unknown provider failure'})
    assert error.reason == 'account_unavailable' and error.code is None
    assert 'secret-provider-payload' not in str(error)


@pytest.mark.anyio
@pytest.mark.parametrize('status, abandoned', [('failed', [('t', 'th/call_1')]), ('completed', [])])
async def test_a_tool_call_codex_gave_up_is_reported_so_the_daemon_releases_it(tmp_path, monkeypatch, status,
                                                                                abandoned):
    """Measured on the pinned 0.157.1: a tools/call that outlives Codex's own per-call timeout ends as a failed
    mcpToolCall item, and Codex sends the MCP server no cancellation. The item's id is the callId the shim got in
    tools/call _meta (measured: both "call_1"), so the adapter names the dropped call to the daemon."""
    proc = SteeringProcess()
    rt, run, _ = await begin_worker(tmp_path, monkeypatch, proc)
    told = []
    rt.on_abandoned = lambda task_id, call_id: told.append((task_id, call_id))
    item = {'id': 'call_1', 'type': 'mcpToolCall', 'server': 'ferb', 'tool': 'mac_read', 'arguments': {}}
    proc.notify('item/started', {'item': {**item, 'status': 'inProgress'}})
    proc.notify('item/completed', {'item': {**item, 'status': status, 'error': {
        'message': 'tool call error: timed out awaiting tools/call after 300s'} if status == 'failed' else None}})
    proc.complete()
    got = [event async for event in rt.events(run)]
    assert got[-1].kind == 'final'
    assert told == abandoned
