"""Supervisor-only pinned Codex app-server transport (D32).

The public v2 protocol carries final-answer deltas; trace bytes are used only for the existing tool-catalog audit.
Workers retain codex exec. Each supervisor has an empty configuration home and an ephemeral thread. Only FERB's
subscription credentials are copied in; a refresh is saved back if the source credentials have not changed.
"""
from __future__ import annotations

import asyncio
import json
import os
import secrets
import tempfile
from collections import deque
from contextlib import suppress
from dataclasses import asdict
from pathlib import Path
from typing import TYPE_CHECKING, Any, AsyncIterator

from ferb.core.errors import Unsupported
from ferb.core.structured import check_final
from ferb.core.types import ModelRequest, RunHandle, RuntimeEvent, Task
from ferb.runtimes.tooltrace import tools_in
from ferb.runtimes.usage import from_codex

if TYPE_CHECKING:
    from ferb.runtimes.codex_exec import CodexRuntime


class ProtocolError(ValueError):
    pass


def _private_write(path: Path, data: bytes) -> None:
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, 'wb') as file:
        file.write(data)


def _save_refresh(state: dict[str, Any]) -> None:
    source, copied = state['auth_source'], state['auth_copy']
    try:
        refreshed = copied.read_bytes()
        if refreshed == state['auth_original'] or not isinstance(json.loads(refreshed), dict):
            return
        if source.read_bytes() != state['auth_original']:
            return                           # Another run/login owns the newer credentials.
        fd, name = tempfile.mkstemp(prefix='.ferb-auth-refresh-', dir=source.parent)
        try:
            with os.fdopen(fd, 'wb') as file:
                file.write(refreshed)
            if source.read_bytes() == state['auth_original']:
                os.replace(name, source)
        finally:
            Path(name).unlink(missing_ok=True)
    except (OSError, ValueError):
        # Never include credentials or provider contents in diagnostics.
        return


def _command(runtime: CodexRuntime, catalog: Path | None) -> list[str]:
    from ferb.runtimes.codex_exec import DISABLED, SUPERVISOR_SETTINGS
    cmd = [str(runtime.binary), 'app-server', '--stdio', '--strict-config']
    for feature in DISABLED:
        cmd += ['--disable', feature]
    cmd += SUPERVISOR_SETTINGS
    for key, value in {'model_provider': 'openai', 'approval_policy': 'never', 'sandbox_mode': 'read-only',
                       'web_search': 'disabled', 'cli_auth_credentials_store': 'file',
                       'analytics.enabled': False}.items():
        cmd += ['-c', key + '=' + json.dumps(value)]
    if catalog:
        cmd += ['-c', 'model_catalog_json=' + json.dumps(str(catalog))]
    return cmd


async def start(runtime: CodexRuntime, task: Task, request: ModelRequest) -> RunHandle:
    from ferb.runtimes.codex_exec import CODEX_USER, codex_home, locked_catalog
    home = codex_home(runtime.ferb_home)
    if home is None:
        raise Unsupported('FERB Codex subscription login missing: run `ferb setup --codex-login`')
    temporary = tempfile.TemporaryDirectory(prefix='ferb-codex-supervisor-')
    root = Path(temporary.name)
    isolated = root / 'codex'
    isolated.mkdir(mode=0o700)
    workdir = root / 'work'
    workdir.mkdir(mode=0o700)
    state: dict[str, Any] = {'app_server': True, 'temporary': temporary, 'proc': None, 'watcher': None,
        'seen': set(), 'violation': None, 'catalog_checked': asyncio.Event(), 'catalog_observed': False,
        'pending': deque(), 'request_id': 0, 'auth_source': home / 'auth.json',
        'auth_copy': isolated / 'auth.json', 'auth_original': b'', 'cancelled': False}
    run = RunHandle('run-' + secrets.token_hex(6), runtime.name, task.id, state)
    try:
        state['auth_original'] = state['auth_source'].read_bytes()
        _private_write(state['auth_copy'], state['auth_original'])
        user_home = runtime.ferb_home / CODEX_USER
        user_home.mkdir(mode=0o700, parents=True, exist_ok=True)
        env = {k: v for k, v in os.environ.items() if k not in {
            'OPENAI_API_KEY', 'CODEX_API_KEY', 'OPENAI_BASE_URL', 'CODEX_ACCESS_TOKEN',
            'CODEX_AUTHAPI_BASE_URL', 'CODEX_AGENT_IDENTITY_AUTHAPI_BASE_URL'}}
        env.update(HOME=str(user_home), CODEX_HOME=str(isolated), RUST_LOG='trace')
        state['proc'] = await asyncio.create_subprocess_exec(*_command(runtime, locked_catalog(runtime.ferb_home)),
            stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
            env=env, cwd=str(workdir), limit=64 * 1024 * 1024)
        state['watcher'] = asyncio.create_task(_watch_catalog(runtime, run))
        async with asyncio.timeout(runtime.timeout_s):
            await _rpc(run, 'initialize', {'clientInfo': {'name': 'ferb', 'title': 'FERB', 'version': '0.27'},
                                         'capabilities': {'experimentalApi': True}})
            await _write(run, {'method': 'initialized'})
            account = await _rpc(run, 'account/read', {'refreshToken': False})
            if (account.get('account') or {}).get('type') != 'chatgpt':
                raise ProtocolError('Codex supervisor requires the FERB subscription login')
            thread = await _rpc(run, 'thread/start', {'model': runtime.model, 'modelProvider': 'openai',
                'cwd': str(workdir), 'approvalPolicy': 'never', 'sandbox': 'read-only',
                'baseInstructions': request.system, 'developerInstructions': '', 'ephemeral': True,
                'dynamicTools': [], 'environments': [], 'selectedCapabilityRoots': [],
                'allowProviderModelFallback': False, 'experimentalRawEvents': False})
            if (thread.get('approvalPolicy') != 'never' or thread.get('sandbox', {}).get('type') != 'readOnly'
                    or thread.get('sandbox', {}).get('networkAccess', False)
                    or thread.get('model') != runtime.model or thread.get('modelProvider') != 'openai'
                    or thread.get('instructionSources') or not thread.get('thread', {}).get('ephemeral')):
                raise ProtocolError('Codex supervisor configuration did not match FERB isolation')
            state['thread_id'] = thread['thread']['id']
            turn = await _rpc(run, 'turn/start', {'threadId': state['thread_id'],
                'input': [{'type': 'text', 'text': request.context}], 'effort': 'low',
                'outputSchema': runtime.output_schema, 'environments': []})
            state['turn_id'] = turn['turn']['id']
        return run
    except BaseException:
        await cancel(run)
        raise


async def _write(run: RunHandle, message: dict[str, Any]) -> None:
    stdin = run.state['proc'].stdin
    if stdin is None:
        raise ProtocolError('Codex supervisor input is unavailable')
    stdin.write((json.dumps(message) + '\n').encode())
    await stdin.drain()


async def _read(run: RunHandle) -> dict[str, Any]:
    proc = run.state['proc']
    line = await proc.stdout.readline()
    if not line:
        raise ProtocolError('Codex supervisor disconnected before its turn completed')
    try:
        obj = json.loads(line)
    except (UnicodeDecodeError, ValueError):
        raise ProtocolError('Codex supervisor returned an invalid protocol message') from None
    if not isinstance(obj, dict):
        raise ProtocolError('Codex supervisor returned an invalid protocol message')
    if 'id' in obj and 'method' in obj:
        await _write(run, {'id': obj['id'], 'error': {'code': -32601, 'message': 'FERB supervisor has no client tools'}})
        raise ProtocolError('Codex supervisor requested a disallowed client action')
    return obj


async def _rpc(run: RunHandle, method: str, params: dict[str, Any]) -> dict[str, Any]:
    run.state['request_id'] += 1
    ident = run.state['request_id']
    await _write(run, {'id': ident, 'method': method, 'params': params})
    while True:
        obj = await _read(run)
        if obj.get('id') == ident:
            result = obj.get('result')
            if 'error' in obj or not isinstance(result, dict):
                raise ProtocolError('Codex supervisor could not complete ' + method)
            return result
        run.state['pending'].append(obj)


async def _watch_catalog(runtime: CodexRuntime, run: RunHandle) -> None:
    state, proc = run.state, run.state['proc']
    try:
        async for raw in proc.stderr:
            # Only parse the request's tool array. No trace output/content is persisted or forwarded.
            text = raw.decode(errors='replace')
            marker = text.find('"tools":[')
            if marker < 0:
                continue
            try:
                array, _ = json.JSONDecoder().raw_decode(text, marker + len('"tools":'))
            except ValueError:
                continue
            if not isinstance(array, list):
                continue
            seen = {n.removeprefix('functions.') for n in tools_in(text)}
            state['seen'] |= seen
            state['catalog_observed'] = True
            if extra := seen - runtime.allowed:
                state['violation'] = extra
                if proc.returncode is None:
                    proc.kill()
                await proc.wait()
            state['catalog_checked'].set()
            if state['violation']:
                return
    except (ValueError, OSError):
        state['catalog_observed'] = False
    finally:
        state['catalog_checked'].set()


async def _finish_trace(run: RunHandle) -> None:
    # The long-lived server has no more work after this one-shot turn. Reap it and consume all stderr before
    # accepting the final result, so a later queued catalog violation cannot race the completion notification.
    proc = run.state['proc']
    if proc.returncode is None:
        with suppress(ProcessLookupError):
            proc.kill()
    await proc.wait()
    if run.state.get('watcher') is not None:
        await run.state['watcher']


async def _check_catalog(run: RunHandle) -> None:
    await run.state['catalog_checked'].wait()
    if run.state['violation']:
        raise ProtocolError('Codex supervisor exposed tools outside FERB containment')
    if not run.state['catalog_observed']:
        raise ProtocolError('Codex supervisor tool catalog was not observed')


async def events(runtime: CodexRuntime, run: RunHandle) -> AsyncIterator[RuntimeEvent]:
    state = run.state
    phases: dict[str, str | None] = {}
    final = ''
    pending_text: list[str] = []
    reading: asyncio.Task[dict[str, Any]] | None = None
    try:
        async with asyncio.timeout(runtime.timeout_s):
            while not state['cancelled']:
                if pending_text and state['catalog_checked'].is_set():
                    await _check_catalog(run)
                    for text in pending_text:
                        yield RuntimeEvent('text_delta', {'text': text})
                    pending_text.clear()
                if state['pending']:
                    obj = state['pending'].popleft()
                else:
                    reading = reading or asyncio.create_task(_read(run))
                    if pending_text and not state['catalog_checked'].is_set():
                        audited = asyncio.create_task(state['catalog_checked'].wait())
                        try:
                            done, _ = await asyncio.wait({reading, audited}, return_when=asyncio.FIRST_COMPLETED)
                        finally:
                            audited.cancel()
                            with suppress(asyncio.CancelledError):
                                await audited
                        if reading not in done:
                            continue
                    obj = await reading
                    reading = None
                method, params = obj.get('method'), obj.get('params', {})
                if params.get('threadId') != state['thread_id']:
                    continue
                if method == 'turn/completed':
                    turn = params.get('turn', {})
                    if turn.get('id') != state['turn_id']:
                        continue
                    await _finish_trace(run)
                    await _check_catalog(run)
                    if turn.get('status') != 'completed' or turn.get('error') or not final:
                        raise ProtocolError('Codex supervisor did not complete its final answer')
                    try:
                        check_final(final, runtime.output_schema)
                    except ValueError:
                        raise ProtocolError('Codex supervisor final answer did not match the requested schema') from None
                    for text in pending_text:
                        yield RuntimeEvent('text_delta', {'text': text})
                    pending_text.clear()
                    yield RuntimeEvent('final', {'text': final, 'returncode': 0, 'tools_seen': sorted(state['seen'])})
                    return
                if params.get('turnId') != state['turn_id']:
                    continue
                if method in {'item/started', 'item/completed'}:
                    item = params.get('item', {})
                    kind = item.get('type')
                    if kind not in {'agentMessage', 'userMessage', 'reasoning', 'plan', 'contextCompaction'}:
                        state['violation'] = {str(kind)}
                        raise ProtocolError('Codex supervisor attempted a disallowed action')
                    if kind == 'agentMessage':
                        phases[item['id']] = item.get('phase')
                        if method == 'item/completed' and item.get('phase') in (None, 'final_answer'):
                            final = str(item.get('text', ''))
                elif method == 'item/agentMessage/delta' and phases.get(params.get('itemId')) == 'final_answer':
                    if runtime.output_schema is not None and isinstance(params.get('delta'), str):
                        pending_text.append(params['delta'])
                elif method == 'thread/tokenUsage/updated':
                    usage = params.get('tokenUsage', {}).get('last', {})
                    mapped = {'input_tokens': usage.get('inputTokens', 0),
                        'cached_input_tokens': usage.get('cachedInputTokens', 0),
                        'output_tokens': usage.get('outputTokens', 0),
                        'reasoning_output_tokens': usage.get('reasoningOutputTokens', 0)}
                    yield RuntimeEvent('usage', asdict(from_codex(runtime.model, mapped)))
                elif method == 'error':
                    raise ProtocolError('Codex supervisor reported an error')
    except (ProtocolError, TimeoutError) as exc:
        if state['violation']:
            yield RuntimeEvent('violation', {'unexpected_tools': sorted(state['violation'])})
        elif not state['cancelled']:
            yield RuntimeEvent('error', {'message': 'Codex supervisor timed out' if isinstance(exc, TimeoutError) else str(exc)})
    finally:
        if reading is not None:
            reading.cancel()
            with suppress(asyncio.CancelledError, ProtocolError):
                await reading
        await cancel(run)


async def cancel(run: RunHandle) -> None:
    state = run.state
    state['cancelled'] = True
    proc = state.get('proc')
    if proc is not None and proc.returncode is None:
        with suppress(ProcessLookupError):
            proc.kill()
        await proc.wait()
    watcher = state.get('watcher')
    if watcher is not None:
        watcher.cancel()
        with suppress(asyncio.CancelledError):
            await watcher
        state['watcher'] = None
    if temporary := state.pop('temporary', None):
        _save_refresh(state)
        temporary.cleanup()
