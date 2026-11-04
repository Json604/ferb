"""Pinned app-server protocol fixtures; no Codex/model process is started."""
import asyncio
import json
from pathlib import Path

import pytest

from ferb.core.types import Identity, ModelRequest, Task
from ferb.runtimes.codex_exec import CodexRuntime
from tests.support.runtime_fakes import Process, next_output

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
    first = await next_output(events)
    assert first.kind == 'text_delta' and first.data == {'text': '{"text":"Hello '}
    second = await next_output(events)
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
@pytest.mark.parametrize('phase', ['commentary', None, 'unexpected_phase', 'partial_answer'])
async def test_commentary_and_unknown_phase_never_become_deltas(tmp_path, monkeypatch, phase):
    """Only a declared final answer (or a phaseless message) is the answer. Codex 0.166.0's `partial_answer` is answer
    text that may be followed by more output, so it is neither streamed nor taken as the final."""
    proc = Process(phase=phase)
    stream, rt, run, _ = await begin(tmp_path, monkeypatch, proc)
    proc.complete()
    got = [ev async for ev in stream.events(rt, run)]
    assert not any(ev.kind == 'text_delta' for ev in got)
    assert got[-1].kind == ('final' if phase is None else 'error')


# decision: RUN-405
@pytest.mark.anyio
async def test_unexpected_actual_tool_catalog_blocks_text_and_final(tmp_path, monkeypatch):
    proc = Process(tools=[{'type': 'function', 'name': 'shell'}])
    stream, rt, run, _ = await begin(tmp_path, monkeypatch, proc)
    proc.complete()
    got = [ev async for ev in stream.events(rt, run)]
    assert any(ev.kind == 'violation' for ev in got)
    assert not any(ev.kind in {'text_delta', 'final'} for ev in got)


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


# decision: RUN-417
@pytest.mark.anyio
@pytest.mark.parametrize("shipped, offered, says", [
    (["gpt-test"], ["other-model"], "Your ChatGPT plan doesn't include gpt-test right now"),   # the account lacks it
    (["gpt-test"], None, "Your ChatGPT plan doesn't include gpt-test right now"),              # no list kept
    (["other-model"], None, "can't run gpt-test: it needs a newer Codex"),                     # the pin lacks it
    (["gpt-test"], ["gpt-test"], "ChatGPT didn't run gpt-test"),                               # neither is shown
    (None, None, "ChatGPT didn't run gpt-test"),                                               # no catalog to read
])
async def test_a_model_codex_cant_run_says_the_cause_the_catalogs_show(tmp_path, monkeypatch, shipped, offered, says):
    """Past bug (live 2026-11-04): Codex's `modelNotFound` always blamed an old Codex, while the owner's ChatGPT plan
    had stopped offering the model. The cause now comes from the catalog shipped with the pinned Codex and the model
    list Codex fetched for FERB's login, never from Codex's own words; those still follow."""
    from ferb.runtimes.codex_exec import CODEX_VERSION
    (tmp_path / 'run').mkdir(exist_ok=True)
    if shipped is not None:
        (tmp_path / 'run' / f'codex-bundled-{CODEX_VERSION}.json').write_text(
            json.dumps({'models': [{'slug': m} for m in shipped]}))
    proc = Process()
    stream, rt, run, _ = await begin(tmp_path, monkeypatch, proc)
    rt.model = 'gpt-test'
    if offered is not None:
        (tmp_path / 'codex-home' / 'models_cache.json').write_text(json.dumps({'models': [{'slug': m} for m in offered]}))
    proc.notify('error', {'error': {'message': "The 'gpt-test' model is not supported when using Codex with a ChatGPT "
                                               "account.", 'codexErrorInfo': 'modelNotFound'}, 'willRetry': False})
    got = [ev async for ev in stream.events(rt, run)]
    message = got[-1].data['message']
    assert got[-1].kind == 'error' and says in message
    assert 'not supported when using Codex' in message and 'modelNotFound' in message


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


# decision: RUN-405
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


# decision: RUN-405
@pytest.mark.anyio
async def test_late_catalog_violation_is_drained_before_final(tmp_path, monkeypatch):
    proc = Process()
    stream, rt, run, _ = await begin(tmp_path, monkeypatch, proc)
    events = stream.events(rt, run)
    assert (await next_output(events)).kind == 'text_delta'
    assert (await next_output(events)).kind == 'text_delta'
    proc.complete()
    proc.stderr.queue.put_nowait(b'TRACE {"tools":[{"type":"function","name":"shell"}]}\n')
    got = [ev async for ev in events]
    assert got[-1].kind == 'violation'
    assert not any(ev.kind == 'final' for ev in got)


# decision: RUN-405
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
    first = asyncio.create_task(next_output(events))
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
# decision: RUN-405
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

class HeldProcess(Process):
    """The app-server stand-in that can hold its answer to thread/start (a start that stalls) and, at turn/start,
    writes a refreshed sign-in into its own copy of the login before its request trace (Codex refreshing a stale
    login as its request goes out)."""

    def __init__(self, hold=False):
        super().__init__()
        self.hold = hold
        self.thread_requested = asyncio.Event()
        self.thread_message = None
        self.auth_path = None
        self.refresh = None

    def write(self, data):
        message = json.loads(data)
        if message['method'] == 'thread/start' and self.hold:
            self.requests.append(message)
            self.thread_message = message
            self.thread_requested.set()
            return
        if message['method'] == 'turn/start' and self.refresh is not None:
            self.auth_path.write_text(self.refresh)
        super().write(data)

    def answer_thread(self):
        self.hold = False
        message, self.thread_message = self.thread_message, None
        self.requests.pop()
        super().write(json.dumps(message))


# decision: RUN-422
@pytest.mark.anyio
async def test_a_turn_makes_no_account_call(tmp_path, monkeypatch):
    """Owner 2026-11-04: no sign-in check on every message. Codex's account/read waited on a ChatGPT round trip
    (0.25-2.7 s a call, live); the start goes straight from initialize to the thread."""
    proc = Process()
    stream, rt, run, _ = await begin(tmp_path, monkeypatch, proc)
    proc.complete()
    got = [ev async for ev in stream.events(rt, run)]
    assert got[-1].kind == 'final'
    assert [m['method'] for m in proc.requests if 'id' in m] == ['initialize', 'thread/start', 'turn/start']
    assert 'account' not in run.state['start_steps']


# decision: RUN-422
@pytest.mark.anyio
@pytest.mark.parametrize('login', [
    '{"OPENAI_API_KEY": "sk-test-only-fixture", "tokens": null}',
    '{"auth_mode": "apikey", "OPENAI_API_KEY": "sk-test-only-fixture"}',
    '{"OPENAI_API_KEY": "sk-test-only-fixture", "tokens": {"refresh_token": "test-only-fixture"}}',
    '{}', 'not json', '[]',
])
async def test_only_a_chatgpt_login_runs_and_never_an_api_key(tmp_path, monkeypatch, login):
    """The account/read check is gone, its billing guard is not: FERB's Codex login file must hold ChatGPT tokens and
    no API key (what Codex reports as an `apiKey` account), read from the file before anything starts."""
    import ferb.runtimes.codex_stream as stream
    spawned = []

    async def spawn(*cmd, **kw):
        spawned.append(cmd)
        return Process()
    monkeypatch.setattr(asyncio, 'create_subprocess_exec', spawn)
    rt = runtime(tmp_path)
    (tmp_path / 'codex-home' / 'auth.json').write_text(login)
    task = Task('t', None, 'reply', 'd', Identity.OWNER, set(), 'mixed', 'r', [], None)
    with pytest.raises(stream.AccountError, match='sign in to ChatGPT in Settings') as caught:
        await stream.start(rt, task, ModelRequest('instructions', ('context',)))
    assert caught.value.reason == 'auth_required' and caught.value.account == 'signed_out'
    assert 'sk-test-only-fixture' not in str(caught.value)
    assert spawned == []                                     # nothing started, nothing sent


# decision: RUN-422, ORCH-315
@pytest.mark.anyio
@pytest.mark.parametrize('error, account', [
    ({'message': 'unauthorized', 'codexErrorInfo': 'unauthorized'}, 'signed_out'),
    ({'message': 'refresh failed', 'data': {'code': 'refresh_token_expired'}}, 'signed_out'),
    ({'message': 'refresh failed', 'data': {'error': 'invalid_grant'}}, 'signed_out'),
    ({'message': 'upstream temporarily unavailable'}, None),
])
async def test_a_sign_in_the_turn_itself_refuses_is_an_account_fact(tmp_path, monkeypatch, error, account):
    """With no check before the turn, Codex's own refusal of the sign-in is what tells: its CodexErrorInfo, or the
    OAuth code of a refresh it couldn't make, marks the error `signed_out` (ORCH-315's sign-in offer); anything else
    stays a plain error. Only Codex's message and codes are kept, never the rest of its error object."""
    proc = Process()
    stream, rt, run, _ = await begin(tmp_path, monkeypatch, proc)
    proc.stdout.put({'method': 'error', 'params': {'threadId': 'th', 'turnId': 'tu',
                                                   'error': {**error, 'secret': 'secret-provider-payload'}}})
    got = [ev async for ev in stream.events(rt, run)]
    assert got[-1].kind == 'error' and got[-1].data.get('account') == account
    assert 'secret-provider-payload' not in json.dumps(got[-1].data)


# decision: RUN-422
@pytest.mark.anyio
async def test_refresh_published_as_the_request_goes_out_and_again_after_turn(tmp_path, monkeypatch):
    """A refreshed login is published when the run's request goes out (Codex refreshes before it), so a run starting
    while the first goes on copies the new login instead of a spent refresh token; and again when the run ends."""
    import ferb.runtimes.codex_stream as stream
    rt = runtime(tmp_path)
    source = tmp_path / 'codex-home' / 'auth.json'
    first, second = HeldProcess(), HeldProcess()
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
    run1 = await asyncio.wait_for(stream.start(rt, task, request), 1)
    await asyncio.wait_for(run1.state['catalog_checked'].wait(), 1)     # its request went out
    run2 = await asyncio.wait_for(stream.start(rt, task, request), 1)
    assert copies[1] == first.refresh and source.read_text() == first.refresh
    assert first.returncode is None  # Refresh is shared while the first model turn remains active.
    later = '{"tokens":{"refresh_token":"second-refresh-fixture"}}'
    first.auth_path.write_text(later)
    await stream.cancel(run1)
    await stream.cancel(run2)
    assert source.read_text() == later


@pytest.mark.anyio
async def test_a_stalled_start_does_not_block_another_start(tmp_path, monkeypatch):
    import ferb.runtimes.codex_stream as stream
    rt = runtime(tmp_path)
    first, second = HeldProcess(hold=True), HeldProcess()
    processes = []

    async def spawn(*cmd, **kw):
        proc = first if not processes else second
        processes.append(proc)
        return proc
    monkeypatch.setattr(asyncio, 'create_subprocess_exec', spawn)
    task = Task('t', None, 'reply', 'd', Identity.OWNER, set(), 'mixed', 'r', [], None)
    request = ModelRequest('instructions', ('context',))
    initial = asyncio.create_task(stream.start(rt, task, request))
    await asyncio.wait_for(first.thread_requested.wait(), 1)
    run = await asyncio.wait_for(stream.start(rt, task, request), 1)
    assert not initial.done() and first.returncode is None
    initial.cancel()
    with pytest.raises(asyncio.CancelledError):
        await initial
    await stream.cancel(run)
    assert first.returncode is not None and second.returncode is not None


@pytest.mark.anyio
async def test_early_refresh_does_not_overwrite_owner_login_or_adopt_its_baseline(tmp_path, monkeypatch):
    import ferb.runtimes.codex_stream as stream
    rt = runtime(tmp_path)
    source = tmp_path / 'codex-home' / 'auth.json'
    original = source.read_bytes()
    proc = HeldProcess(hold=True)
    proc.refresh = '{"tokens":{"refresh_token":"old-run-refresh-fixture"}}'

    async def spawn(*cmd, **kw):
        proc.auth_path = Path(kw['env']['CODEX_HOME']) / 'auth.json'
        return proc
    monkeypatch.setattr(asyncio, 'create_subprocess_exec', spawn)
    task = Task('t', None, 'reply', 'd', Identity.OWNER, set(), 'mixed', 'r', [], None)
    starting = asyncio.create_task(stream.start(rt, task, ModelRequest('instructions', ('context',))))
    await asyncio.wait_for(proc.thread_requested.wait(), 1)
    owner_login = '{"tokens":{"refresh_token":"owner-new-login-fixture"}}'
    source.write_text(owner_login)
    proc.answer_thread()
    run = await asyncio.wait_for(starting, 1)
    await asyncio.wait_for(run.state['catalog_checked'].wait(), 1)
    assert source.read_text() == owner_login and run.state['auth_original'] == original
    await stream.cancel(run)
    assert source.read_text() == owner_login


def test_account_error_never_retains_a_non_numeric_provider_code():
    from ferb.runtimes.codex_stream import AccountError
    error = AccountError('auth_required', 'secret-provider-payload')    # type: ignore[arg-type]
    assert error.reason == 'auth_required' and error.code is None
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
