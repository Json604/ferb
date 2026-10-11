"""Provider fragments arrive before completion without exposing thought or tool text as an answer."""
import asyncio
import json
from types import SimpleNamespace

import httpx
import pytest

from ferb.core.types import Identity, ModelRequest, RunHandle, Task
from ferb.runtimes.anthropic_api import AnthropicProvider
from ferb.runtimes.api import ApiRuntime
from ferb.runtimes.claude_sdk import ClaudeRuntime
from ferb.runtimes.google_api import GoogleProvider
from ferb.runtimes.openai_api import OpenAIProvider

TASK = Task('t', None, 'reply', 'dev', Identity.OWNER, set(), 'mixed', 'r', [], None)
SCHEMA = {'type': 'object', 'properties': {'text': {'type': 'string'}}, 'required': ['text']}
FIRST, LAST = '{"text":"Your draft', ' is ready."}'


def wire_events(provider):
    if provider == 'openai':
        return ([{'type': 'response.reasoning_text.delta', 'delta': 'private reasoning'},
                 {'type': 'response.output_text.delta', 'delta': FIRST}],
                [{'type': 'response.output_text.delta', 'delta': LAST},
                 {'type': 'response.completed', 'response': {'status': 'completed', 'usage': {'input_tokens': 7},
                  'output': [{'type': 'message', 'content': [{'type': 'output_text', 'text': FIRST + LAST}]}]}}])
    if provider == 'anthropic':
        return ([{'type': 'message_start', 'message': {'role': 'assistant', 'content': [], 'usage': {'input_tokens': 7}}},
                 {'type': 'content_block_start', 'index': 0, 'content_block': {'type': 'thinking', 'thinking': ''}},
                 {'type': 'content_block_delta', 'index': 0, 'delta': {'type': 'thinking_delta', 'thinking': 'private reasoning'}},
                 {'type': 'content_block_stop', 'index': 0},
                 {'type': 'content_block_start', 'index': 1, 'content_block': {'type': 'text', 'text': ''}},
                 {'type': 'content_block_delta', 'index': 1, 'delta': {'type': 'text_delta', 'text': FIRST}}],
                [{'type': 'content_block_delta', 'index': 1, 'delta': {'type': 'text_delta', 'text': LAST}},
                 {'type': 'content_block_stop', 'index': 1},
                 {'type': 'message_delta', 'delta': {'stop_reason': 'end_turn'}, 'usage': {'output_tokens': 9}},
                 {'type': 'message_stop'}])
    return ([{'candidates': [{'index': 0, 'content': {'role': 'model', 'parts': [
                {'thought': True, 'text': 'private reasoning'}, {'text': FIRST}]}}]}],
            [{'candidates': [{'index': 0, 'content': {'role': 'model', 'parts': [{'text': LAST}]},
                              'finishReason': 'STOP'}], 'usageMetadata': {'promptTokenCount': 7}}])


class SplitStream(httpx.AsyncByteStream):
    def __init__(self, first, rest, gate, *, broken=False):
        self.first, self.rest, self.gate, self.broken = first, rest, gate, broken
        self.closed = False

    async def __aiter__(self):
        for event in self.first:
            data = ('data: ' + json.dumps(event) + '\n\n').encode()
            yield data[:9]
            yield data[9:]
        await self.gate.wait()
        if self.broken:
            raise httpx.ReadError('connection failed with sk-secret')
        for event in self.rest:
            yield ('data: ' + json.dumps(event) + '\n\n').encode()

    async def aclose(self):
        self.closed = True


def runtime(provider, body, requests):
    async def handle(request):
        requests.append(request)
        return httpx.Response(200, headers={'content-type': 'text/event-stream'}, stream=body)

    async def key(name):
        return 'sk-secret'

    return ApiRuntime({'openai': OpenAIProvider, 'anthropic': AnthropicProvider, 'google': GoogleProvider}[provider](),
                      mode='supervisor', role='orchestrator', tool_names=[], output_schema=SCHEMA, bridge=None,
                      key=key, transport=httpx.MockTransport(handle))


@pytest.mark.anyio
@pytest.mark.parametrize('provider', ['openai', 'anthropic', 'google'])
async def test_provider_delta_arrives_while_completion_is_still_blocked(provider):
    gate = asyncio.Event()
    first, rest = wire_events(provider)
    body = SplitStream(first, rest, gate)
    requests = []
    rt = runtime(provider, body, requests)
    run = await rt.start(TASK, ModelRequest('instructions', ('message',)), '')
    events = rt.events(run)
    try:
        early = await asyncio.wait_for(anext(events), 1)
        assert early.kind == 'text_delta' and early.data == {'text': FIRST}
        assert not gate.is_set()
        gate.set()
        tail = [event async for event in events]
        assert ''.join(e.data['text'] for e in [early, *tail] if e.kind == 'text_delta') == FIRST + LAST
        assert tail[-1].kind == 'final' and tail[-1].data['text'] == FIRST + LAST
        assert next(e.data for e in tail if e.kind == 'usage')['input_tokens'] == 7
        payload = json.loads(requests[0].content)
        if provider == 'google':
            assert requests[0].url.path.endswith(':streamGenerateContent') and requests[0].url.params['alt'] == 'sse'
        else:
            assert payload['stream'] is True
    finally:
        gate.set()
        await rt.cancel(run)
        await events.aclose()
    assert body.closed


@pytest.mark.anyio
@pytest.mark.parametrize('provider', ['openai', 'anthropic', 'google'])
async def test_incomplete_provider_stream_never_becomes_a_final_answer(provider):
    gate = asyncio.Event()
    gate.set()
    body = SplitStream(wire_events(provider)[0], [], gate)
    rt = runtime(provider, body, [])
    run = await rt.start(TASK, ModelRequest('', ('message',)), '')
    events = [event async for event in rt.events(run)]
    assert events[-1].kind == 'error' and not any(e.kind == 'final' for e in events)


@pytest.mark.anyio
@pytest.mark.parametrize('provider', ['openai', 'anthropic', 'google'])
async def test_provider_refusal_or_cutoff_cannot_finalize_a_streamed_draft(provider):
    first, rest = wire_events(provider)
    if provider == 'openai':
        rest[-1]['type'] = 'response.incomplete'
        rest[-1]['response']['status'] = 'incomplete'
        rest[-1]['response']['incomplete_details'] = {'reason': 'max_output_tokens'}
    elif provider == 'anthropic':
        rest[-2]['delta']['stop_reason'] = 'refusal'
    else:
        rest[-1]['candidates'][0]['finishReason'] = 'SAFETY'
    gate = asyncio.Event()
    gate.set()
    rt = runtime(provider, SplitStream(first, rest, gate), [])
    run = await rt.start(TASK, ModelRequest('', ('message',)), '')
    events = [event async for event in rt.events(run)]
    assert any(e.kind == 'text_delta' for e in events)
    assert events[-1].kind == 'error' and not any(e.kind == 'final' for e in events)


@pytest.mark.anyio
async def test_stream_failure_after_text_is_not_retried_or_disclosed_with_the_key():
    gate = asyncio.Event()
    gate.set()
    requests = []
    rt = runtime('openai', SplitStream(wire_events('openai')[0], [], gate, broken=True), requests)
    run = await rt.start(TASK, ModelRequest('', ('message',)), '')
    events = [event async for event in rt.events(run)]
    assert events[-1].kind == 'error' and len(requests) == 1
    assert 'sk-secret' not in json.dumps([e.data for e in events])


@pytest.mark.anyio
@pytest.mark.parametrize('provider', ['openai', 'anthropic', 'google'])
async def test_cancel_closes_a_stream_waiting_for_more_text(provider):
    gate = asyncio.Event()
    body = SplitStream(*wire_events(provider), gate)
    rt = runtime(provider, body, [])
    run = await rt.start(TASK, ModelRequest('', ('message',)), '')
    events = rt.events(run)
    await asyncio.wait_for(anext(events), 1)
    pending = asyncio.create_task(anext(events, None))
    await rt.cancel(run)
    assert await asyncio.wait_for(pending, 1) is None
    await events.aclose()
    assert body.closed


@pytest.mark.anyio
async def test_closing_the_event_iterator_cancels_the_active_stream_reader():
    gate = asyncio.Event()
    body = SplitStream(*wire_events('openai'), gate)
    rt = runtime('openai', body, [])
    run = await rt.start(TASK, ModelRequest('', ('message',)), '')
    events = rt.events(run)
    assert (await asyncio.wait_for(anext(events), 1)).kind == 'text_delta'
    await events.aclose()
    assert body.closed and run.state['run'].inflight.done() and run.state['run'].client.is_closed


@pytest.mark.anyio
async def test_closing_after_a_fragment_retrieves_an_already_failed_api_call():
    import gc
    import weakref

    gate = asyncio.Event()
    gate.set()
    rt = runtime('openai', SplitStream(wire_events('openai')[0], [], gate, broken=True), [])
    run = await rt.start(TASK, ModelRequest('', ('message',)), '')
    events = rt.events(run)
    loop = asyncio.get_running_loop()
    previous_handler = loop.get_exception_handler()
    reported = []
    loop.set_exception_handler(lambda _, context: reported.append(context.get('message', '')))
    try:
        assert (await anext(events)).kind == 'text_delta'
        inflight = weakref.ref(run.state['run'].inflight)
        assert inflight() is not None and inflight().done()
        await events.aclose()
        del events, run
        gc.collect()
        await asyncio.sleep(0)
        assert inflight() is None
        assert not any('exception was never retrieved' in message.lower() for message in reported)
    finally:
        loop.set_exception_handler(previous_handler)


@pytest.mark.anyio
async def test_claude_streams_only_the_structured_output_tool_payload():
    from claude_agent_sdk import ResultMessage, SystemMessage
    from claude_agent_sdk.types import StreamEvent

    gate = asyncio.Event()
    def event(data):
        return StreamEvent('event', 'session', data)

    class Client:
        async def receive_messages(self):
            yield SystemMessage('init', {'apiKeySource': 'none'})
            yield event({'type': 'content_block_start', 'index': 0, 'content_block': {'type': 'text', 'text': ''}})
            yield event({'type': 'content_block_delta', 'index': 0,
                         'delta': {'type': 'text_delta', 'text': 'commentary, not structured output'}})
            yield event({'type': 'content_block_start', 'index': 1,
                         'content_block': {'type': 'tool_use', 'name': 'StructuredOutput', 'input': {}}})
            yield event({'type': 'content_block_delta', 'index': 1,
                         'delta': {'type': 'input_json_delta', 'partial_json': FIRST}})
            await gate.wait()
            yield event({'type': 'content_block_delta', 'index': 1,
                         'delta': {'type': 'input_json_delta', 'partial_json': LAST}})
            yield ResultMessage('success', 1, 1, False, 1, 'session', structured_output=json.loads(FIRST + LAST))

        async def disconnect(self):
            pass

    proxy = SimpleNamespace(seen=lambda ident: set(), base_url=lambda ident: 'http://unused', forget=lambda ident: None)
    rt = ClaudeRuntime('supervisor', proxy, '/unused', output_schema=SCHEMA)
    assert rt._options('run', '').include_partial_messages is True
    events = rt.events(RunHandle('run', 'claude', 't', {'client': Client(), 'checked_auth': False}))
    try:
        early = await asyncio.wait_for(anext(events), 1)
        assert early.kind == 'text_delta' and early.data['text'] == FIRST
        gate.set()
        rest = [event async for event in events]
        assert [e.data['text'] for e in rest if e.kind == 'text_delta'] == [LAST]
        assert rest[-1].kind == 'final' and json.loads(rest[-1].data['text']) == json.loads(FIRST + LAST)
    finally:
        gate.set()
        await events.aclose()
