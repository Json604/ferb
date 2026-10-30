"""Same Claude session consumes owner updates, including SDK-merged inputs (D3/D32)."""
import asyncio
from types import SimpleNamespace

import pytest
from claude_agent_sdk import ResultMessage, UserMessage

from ferb.core.errors import Unsupported
from ferb.core.types import RunHandle
from ferb.runtimes.claude_sdk import ClaudeRuntime
from tests.support.claude_home import signed_in_home


def result(text):
    return ResultMessage('success', 1, 1, False, 1, 'session', result=text)


class Client:
    def __init__(self):
        self.messages = asyncio.Queue()
        self.sent = []
        self.closed = False

    async def query(self, frames):
        async for frame in frames:
            self.sent.append(frame)

    async def receive_messages(self):
        while not self.closed:
            yield await self.messages.get()

    async def receive_response(self):
        async for message in self.receive_messages():
            yield message
            if isinstance(message, ResultMessage):
                return

    async def disconnect(self):
        self.closed = True


def setup():
    client = Client()
    proxy = SimpleNamespace(seen=lambda ident: set(), forget=lambda ident: None)
    rt = ClaudeRuntime('worker', proxy, signed_in_home(), shim_cmd=['ferb', 'mcp'])
    run = RunHandle('same-run', 'claude', 'task', {'client': client, 'checked_auth': True})
    return rt, run, client


@pytest.mark.anyio
async def test_claude_waits_for_queued_correction_after_stale_result():
    rt, run, client = setup()
    await rt.continue_(run, 'Keep it under 300')
    await client.messages.put(result('stale'))
    await client.messages.put(UserMessage('Keep it under 300', uuid=client.sent[0]['uuid']))
    await client.messages.put(result('updated'))
    events = await asyncio.wait_for(_collect(rt, run), 1)
    assert [e.data['text'] for e in events if e.kind == 'final'] == ['updated']
    assert not client.closed
    with pytest.raises(Unsupported):
        await rt.continue_(run, 'too late')


@pytest.mark.anyio
async def test_claude_merged_owner_messages_need_one_final_not_one_per_message():
    rt, run, client = setup()
    await rt.continue_(run, 'Keep it under 300')
    await rt.continue_(run, 'Two breads')
    assert [f['message']['content'] for f in client.sent] == ['Keep it under 300', 'Two breads']
    for frame in client.sent:
        await client.messages.put(UserMessage(frame['message']['content'], uuid=frame['uuid']))
    await client.messages.put(result('updated'))
    events = await asyncio.wait_for(_collect(rt, run), 1)
    assert [e.data['text'] for e in events if e.kind == 'final'] == ['updated']


@pytest.mark.anyio
async def test_claude_update_at_result_usage_is_not_dropped():
    rt, run, client = setup()
    await client.messages.put(result('stale'))
    events = []
    async for event in rt.events(run):
        events.append(event)
        if event.kind == 'usage' and not client.sent:
            await rt.continue_(run, 'Two breads')
            await client.messages.put(UserMessage('Two breads', uuid=client.sent[0]['uuid']))
            await client.messages.put(result('updated'))
    assert [e.data['text'] for e in events if e.kind == 'final'] == ['updated']


@pytest.mark.anyio
async def test_claude_cancelled_run_rejects_update():
    rt, run, client = setup()
    await rt.cancel(run)
    with pytest.raises(Unsupported):
        await rt.continue_(run, 'too late')


async def _collect(rt, run):
    return [event async for event in rt.events(run)]


@pytest.mark.anyio
async def test_claude_final_event_rejects_updates_before_cleanup():
    rt, run, client = setup()
    await client.messages.put(result('done'))
    async for event in rt.events(run):
        if event.kind == 'final':
            with pytest.raises(Unsupported):
                await rt.continue_(run, 'too late')


@pytest.mark.anyio
async def test_claude_failed_input_write_reports_failure_and_closes(monkeypatch):
    rt, run, client = setup()

    async def broken(frames):
        raise OSError('transport closed')

    monkeypatch.setattr(client, 'query', broken)
    with pytest.raises(OSError, match='transport closed'):
        await rt.continue_(run, 'Two breads')
    assert client.closed and not run.state['pending_updates']
    with pytest.raises(Unsupported):
        await rt.continue_(run, 'too late')


def test_claude_requests_replay_acknowledgements_without_persisting_sessions():
    rt, _, _ = setup()
    rt.proxy.base_url = lambda run_id: 'http://unused'
    from pathlib import Path
    assert rt._options('run', Path('/tmp/x/run-token')).extra_args == {
        'no-session-persistence': None, 'replay-user-messages': None,
    }
