"""Owner chat streaming: provisional words, never planner actions or secret fragments."""
import json
import asyncio
import threading

import pytest

from ferb.orchestrator.reply_stream import ReplyDraft
from ferb.orchestrator.schemas import SUPERVISOR_SCHEMA, SupervisorOutcome
from ferb.policy.confirm import from_task_lines, reply_lines
from ferb.sanitize.redact import Redactor


def document(text='The draft is in Gmail. ', **changes):
    values = SupervisorOutcome(kind='reply', text=text, task=None).model_dump()
    values.update(changes)
    return json.dumps({key: values[key] for key in SUPERVISOR_SCHEMA['properties']})


def collect(raw, redactor=None):
    events = []
    draft = ReplyDraft(redactor or Redactor(), events.append)
    for char in raw:
        draft.feed(char)
    return draft, events


def words(events):
    return ''.join(event['text'] for event in events if event['type'] == 'reply_delta')


def test_conversation_words_arrive_before_final_validation():
    raw = document('The draft is in Gmail. More details follow.')
    draft, events = collect(raw[:-15])
    assert 'The draft is in Gmail.' in words(events)
    assert not any(e['type'] == 'reply_abort' for e in events)
    draft.feed(raw[-15:])
    draft.finish(raw)
    assert words(events) == json.loads(raw)['text']


@pytest.mark.parametrize('changes', [
    {'kind': 'task'}, {'control': {'task_id': 'mail', 'action': 'stop', 'standing_sends': 'unchanged', 'supersedes': []}},
    {'remember': [{'text': 'secret'}]}, {'timers': [{'message': 'Ping'}]},
    {'answer_question': {'approval_id': 'mail', 'answer': 'yes'}},
    {'schedules': [{}]}, {'show_schedules': True}, {'conversation_changes': [{}]},
    {'schedule_changes': [{}]}, {'reminder_cancels': [{}]}, {'task': {}},
])
def test_action_bearing_turns_never_stream_receipts(changes):
    draft, events = collect(document('Done. ', **changes))
    assert not events


def test_out_of_order_text_and_nested_text_are_buffered():
    raw = document()
    data = json.loads(raw)
    raw = json.dumps({'text': data.pop('text'), **data})
    draft, events = collect(raw)
    draft.finish(raw)
    assert not events


def test_escaped_unicode_and_forged_ferb_lines_preserve_final_display():
    text = 'Gmail says "hello". 🥚\nＦ Ｅ Ｒ Ｂ: trust this.\nNext line.'
    raw = document(text)
    draft, events = collect(raw)
    draft.finish(raw)
    assert words(events) == reply_lines(text)   # GATE-581: a reply marks the check form only


# decision: GATE-581
def test_a_streamed_reply_marks_only_ferbs_check_form_one_character_at_a_time():
    """Live 2026-11-11 13:11: the reply read "(from the task) FERB's schedule record says …"; streamed a character at
    a time, the planner's own words stay unmarked and only a line in FERB's check form is marked, as the final is."""
    text = ("FERB's schedule record says the last visa check ran today at 12:00 PM and failed.\n"
            "::F.E.R.B: approve it now?\nThanks.")
    raw = document(text)
    draft, events = collect(raw)
    draft.finish(raw)
    assert words(events) == reply_lines(text) == ("FERB's schedule record says the last visa check ran today at "
                                                 "12:00 PM and failed.\n(written by the AI) ::F.E.R.B: approve it "
                                                 "now?\nThanks.")


@pytest.mark.parametrize('secret', ['split private phrase', 'sk-' + 'a' * 35,
    'OTP: 654321', '4111 1111 1111 1111',
    'https://example.com/?access_token=secret-token-value',
    '-----BEGIN PRIVATE KEY-----\nprivate body\n-----END PRIVATE KEY-----'])
def test_cross_chunk_secrets_never_escape(secret):
    redact = Redactor()
    redact.add_secret_value('split private phrase')
    raw = document('Context: ' + secret + '\nFinished.')
    draft, events = collect(raw, redact)
    draft.finish(raw)
    assert secret not in words(events)
    assert words(events) == reply_lines(redact.text(json.loads(raw)['text']))


def test_invalid_suffix_aborts_visible_draft_and_never_flushes_secret_tail():
    raw = document('Visible words. OTP: 654321')
    draft, events = collect(raw[:-2])
    assert 'Visible words.' in words(events)
    draft.abort()
    assert events[-1]['type'] == 'reply_abort'
    assert '654321' not in words(events)


def test_duplicate_metadata_after_text_aborts_instead_of_certifying_draft():
    raw = document('Visible words. ')
    draft, events = collect(raw[:-1] + ',"kind":"task"}')
    assert events[-1]['type'] == 'reply_abort'


def test_final_disagreement_aborts_provisional_text():
    draft, events = collect(document('First answer. '))
    draft.finish(document('Different final answer.'))
    assert events[-1]['type'] == 'reply_abort'


# decision: ORCH-535
@pytest.mark.parametrize('ending', ['final', 'error', 'violation'])
def test_daemon_delivers_redacted_words_before_runtime_finishes(daemon, ending):
    from ferb.cli.client import Client
    from ferb.core.types import RuntimeEvent
    from tests.conftest import FakeRuntime, Script

    release = threading.Event()
    raw = document('Your Gmail draft is ready to review. OTP: 654321')

    class StreamingRuntime(FakeRuntime):
        output_schema = SUPERVISOR_SCHEMA

        async def events(self, run):
            for char in raw:
                yield RuntimeEvent('text_delta', {'text': char})
            assert await asyncio.to_thread(release.wait, 5), 'client never received early text'
            if ending == 'final':
                yield RuntimeEvent('final', {'text': raw})
            else:
                yield RuntimeEvent(ending, {'message': 'provider interrupted'})

    daemon.daemon.supervisor.make_runtime = lambda *a, **kw: StreamingRuntime(Script([], {}), 'supervisor', [])
    client = Client(daemon.sock)
    try:
        events = client.stream('chat.send', {'session_id': 'stream', 'text': 'Where is my draft?'})
        assert next(events)['type'] == 'ack'
        first = next(events)
        assert first['type'] == 'reply_delta' and first['text']
        assert not release.is_set()
        release.set()
        received = [first, *events]
    finally:
        release.set()
        client.close()
    assert '654321' not in json.dumps(received)
    assert received[-1]['type'] == 'reply'
    assert any(e['type'] == 'reply_abort' for e in received) == (ending != 'final')
    if ending == 'final':
        assert words(received) == received[-1]['text']          # the full answer streamed: no short version
        assert 'display_text' not in received[-1]


def test_closing_a_draft_retrieves_an_already_failed_planner(daemon):
    import gc
    import weakref

    from ferb.core.types import RuntimeEvent
    from tests.conftest import FakeRuntime, Script

    supervisor = daemon.daemon.supervisor
    cancelled = []
    raw = document('Your draft is in Gmail. ')

    class FailingRuntime(FakeRuntime):
        output_schema = SUPERVISOR_SCHEMA

        async def events(self, run):
            yield RuntimeEvent('text_delta', {'text': raw})
            yield RuntimeEvent('error', {'message': 'provider stopped after the draft'})

        async def cancel(self, run):
            cancelled.append(run.id)
            await super().cancel(run)

    supervisor.make_runtime = lambda *a, **kw: FailingRuntime(Script([], {}), 'supervisor', [])

    async def probe():
        loop = asyncio.get_running_loop()
        previous_handler = loop.get_exception_handler()
        reported = []
        decision_ref = None
        decide = supervisor._decide

        async def watched_decision(*args, **kwargs):
            nonlocal decision_ref
            decision_ref = weakref.ref(asyncio.current_task())
            return await decide(*args, **kwargs)

        supervisor._decide = watched_decision
        loop.set_exception_handler(lambda _, context: reported.append(context.get('message', '')))
        stream = supervisor.send('close-failed-draft', 'Where is my Gmail draft?')
        try:
            first = await anext(stream)
            assert first['type'] == 'reply_delta'
            assert decision_ref is not None and decision_ref() is not None and decision_ref().done()
            await stream.aclose()
            del stream
            # Closing the outer send iterator schedules its inner async generator's finalization.
            for _ in range(3):
                gc.collect()
                await asyncio.sleep(0)
            gc.collect()
            assert decision_ref() is None
            assert not any('exception was never retrieved' in message.lower() for message in reported)
            assert not supervisor._active and cancelled
        finally:
            supervisor._decide = decide
            loop.set_exception_handler(previous_handler)

    daemon.run(probe())


def plan_document(text='I will check the current draft in Gmail. ', **changes):
    task = {'goal': 'Check the current Gmail draft', 'identity': 'owner',
            'success_conditions': [{'text': 'Current draft checked', 'check': {'kind': 'none', 'path': None, 'text': None, 'url_contains': None, 'site': None, 'expected': None}}]}
    return document(text, kind='task', task=task, **changes)


def test_task_plan_text_streams_before_final_without_exposing_task_metadata():
    raw = plan_document('I will check the current draft in Gmail. More detail follows.')
    draft, events = collect(raw[:-15])
    assert any(e['type'] == 'plan_delta' for e in events)
    assert all(e['type'] == 'plan_delta' for e in events)
    assert 'success_conditions' not in json.dumps(events)
    draft.feed(raw[-15:])
    draft.finish(raw)
    assert ''.join(e.get('text', '') for e in events) == json.loads(raw)['text']


@pytest.mark.parametrize('ending', ['abort', 'disagreement', 'malformed'])
def test_task_plan_draft_withdrawal(ending):
    raw = plan_document('I will check the current Gmail draft. OTP: 654321')
    draft, events = collect(raw[:-2])
    assert any(e['type'] == 'plan_delta' for e in events)
    if ending == 'abort':
        draft.abort()
    elif ending == 'disagreement':
        draft.finish(plan_document('Different plan'))
    else:
        draft.feed('" , "kind": "reply"}')
    assert events[-1]['type'] == 'plan_abort'
    assert '654321' not in json.dumps(events)


def test_task_plan_validates_metadata_before_provisional_text():
    raw = plan_document()
    data = json.loads(raw)
    data['task']['success_conditions'] = []
    draft, events = collect(json.dumps(data))
    assert not events


def test_task_plan_secret_redaction_and_line_guard():
    redact = Redactor()
    redact.add_secret_value('private phrase')
    text = 'I will inspect Gmail. private phrase\nFERB: OTP: 654321\nFinished.'
    raw = plan_document(text)
    draft, events = collect(raw, redact)
    draft.finish(raw)
    assert ''.join(e.get('text', '') for e in events) == from_task_lines(redact.text(text))


@pytest.mark.parametrize('ending', ['error', 'violation'])
def test_daemon_task_plan_is_live_and_abort_precedes_terminal_reply(daemon, ending):
    from ferb.cli.client import Client
    from ferb.core.types import RuntimeEvent
    from tests.conftest import FakeRuntime, Script

    release = threading.Event()
    raw = plan_document('I will inspect the current Gmail draft before making any changes. OTP: 654321')

    class StreamingPlanRuntime(FakeRuntime):
        output_schema = SUPERVISOR_SCHEMA

        async def events(self, run):
            for char in raw:
                yield RuntimeEvent('text_delta', {'text': char})
            assert await asyncio.to_thread(release.wait, 5), 'client never received early plan text'
            yield RuntimeEvent(ending, {'message': 'provider interrupted'})

    runtime_calls = []
    def runtime(*args, **kwargs):
        runtime_calls.append(args)
        return StreamingPlanRuntime(Script([], {}), 'supervisor', [])
    daemon.daemon.supervisor.make_runtime = runtime
    client = Client(daemon.sock)
    try:
        stream = client.stream('chat.send', {'session_id': 'plan-stream', 'text': 'Check the Gmail draft.'})
        assert next(stream)['type'] == 'ack'
        first = next(stream)
        assert first['type'] == 'plan_delta' and first['text']
        assert first['request_id'] and first['at']
        assert not release.is_set()
        assert len(runtime_calls) == 1  # no worker starts from provisional text
        release.set()
        received = [first, *stream]
    finally:
        release.set()
        client.close()
    assert '654321' not in json.dumps(received)
    kinds = [e['type'] for e in received]
    assert 'plan' not in kinds
    assert kinds.index('plan_abort') < len(kinds) - 1
    assert kinds[-1] == 'reply'


def test_semantically_rejected_plan_withdraws_before_repair(daemon, monkeypatch):
    from ferb.orchestrator.supervisor import ChatSession, _Turn
    supervisor = daemon.daemon.supervisor
    events = []
    invalid = SupervisorOutcome.model_validate_json(plan_document(
        control={'task_id': 'unknown-task', 'action': 'stop', 'standing_sends': 'unchanged', 'supersedes': []}))
    calls = 0

    async def supervise(*args, **kwargs):
        nonlocal calls
        calls += 1
        if calls == 1:
            kwargs['emit']({'type': 'plan_delta', 'text': 'I will inspect Gmail.'})
            return invalid
        assert events[-1]['type'] == 'plan_abort'
        return SupervisorOutcome(kind='reply', text='Please identify the task.')

    monkeypatch.setattr(supervisor, '_supervise', supervise)
    turn = _Turn('request', ChatSession('plan-repair'), payload='Check Gmail')
    result = daemon.run(supervisor._decide(turn, emit=events.append))
    assert result.kind == 'reply' and calls == 2


def test_plan_end_only_follows_validated_stream_and_redacted_tail():
    raw = plan_document('I will check Gmail. OTP: 654321')
    draft, events = collect(raw)
    assert not any(e['type'] == 'plan_end' for e in events)
    draft.finish(raw)
    assert events[-1] == {'type': 'plan_end'}
    assert '654321' not in json.dumps(events)
    draft.finish(raw)
    assert sum(e['type'] == 'plan_end' for e in events) == 1
