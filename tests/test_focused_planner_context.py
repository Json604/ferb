"""Recorded failure shapes: unrelated old history, exact contextual follow-ups and confined reads.

Scripted plumbing checks; actual recorded-task model replay remains a separate acceptance step.
"""
import json
from types import SimpleNamespace

import pytest
from pydantic import ValidationError

from ferb.orchestrator.planner_context import SUMMARY_VERSION, focused_exchange, read_context
from ferb.orchestrator.schemas import ContextRead, SupervisorOutcome, SUPERVISOR_SCHEMA, TaskControl
from tests.conftest import chat, install, sup


def test_focus_uses_last_owner_message_before_summary_coverage():
    records = [SimpleNamespace(kind=k, text=t) for k, t in [
        ('chat.message', 'old playback topic'), ('chat.reply', 'old answer'),
        ('chat.message', 'retry the US check'), ('task', 'check task'), ('task.answer', 'portal blocker')]]
    latest, older = focused_exchange(records)
    assert [r.text for r in latest] == ['retry the US check', 'check task', 'portal blocker']
    assert [r.text for r in older] == ['old playback topic', 'old answer']
    assert focused_exchange(records, scheduled=True) == ([], records)


def test_opening_notices_remain_visible_before_the_first_owner_exchange():
    notice = SimpleNamespace(kind='chat.notice', text='A WhatsApp message came in.')
    assert focused_exchange([notice]) == ([notice], [])
    assert focused_exchange([]) == ([], [])


@pytest.mark.parametrize('field,value', [('text', 'I already did it'), ('remember', [{'text': 'x'}]),
                                       ('control', {'task_id': 'x', 'action': 'stop'}), ('unknown', None)])
def test_intermediate_rejects_text_actions_and_extra_fields(field, value):
    raw = {'kind': 'context_read', 'context_read': {'source': 'history'}, 'text': '', field: value}
    with pytest.raises(ValidationError):
        SupervisorOutcome.model_validate(raw)


def test_context_read_cannot_choose_a_session_or_be_combined_with_final():
    with pytest.raises(ValidationError):
        ContextRead.model_validate({'source': 'history', 'session_id': 'other'})
    with pytest.raises(ValidationError):
        SupervisorOutcome.model_validate({'kind': 'reply', 'text': 'done', 'context_read': {'source': 'history'}})
    assert SupervisorOutcome.model_validate_json(sup('reply', 'legacy final')).text == 'legacy final'


def test_runtime_schema_can_express_existing_owner_steering_control():
    control_schema = SUPERVISOR_SCHEMA['properties']['control']['anyOf'][1]
    actions = control_schema['properties']['action']['enum']
    assert set(actions) == set(TaskControl.model_json_schema()['properties']['action']['enum'])
    assert 'steer' in actions


def test_frozen_reader_preserves_exact_pages_origins_and_denies_cross_conversation():
    record = {'id': 'mine', 'kind': 'chat.reply', 'origin': 'external:page',
              'data': {'text': 'recorded detail ' * 200}}
    kwargs = dict(history=lambda tasks: [record], ability=lambda name: {}, skill=lambda name: {}, skill_ids=set())
    first, made = read_context(ContextRead(source='history', id='mine', max_chars=1000), **kwargs)
    body = json.loads(first.payload)
    second, _ = read_context(ContextRead(source='history', id='mine', offset=body['next_offset'], max_chars=60000), **kwargs)
    assert body['text'] + json.loads(second.payload)['text'] == json.dumps(record, ensure_ascii=False)
    assert made == {'external:page'} and first.source_refs == ('entry:mine',)
    denied, _ = read_context(ContextRead(source='history', id='other-session-entry'), **kwargs)
    assert 'not in this task' in denied.text
    assert '<untrusted-data' in first.text


def test_guidance_redaction_and_registered_skill_boundary():
    reads = []
    def skill(name):
        reads.append(name)
        return {'body': 'secret-value'}
    kwargs = dict(history=lambda tasks: [], ability=lambda name: {}, skill=skill, skill_ids={'known'},
                  redact=lambda value: {'body': '[redacted]'})
    block, _ = read_context(ContextRead(source='skill', id='../outside'), **kwargs)
    assert not reads and 'installed owner skill' in block.text
    block, made = read_context(ContextRead(source='skill', id='known'), **kwargs)
    assert 'secret-value' not in block.text and '[redacted]' in block.text and made == {'owner'}


def test_planner_reads_current_conversation_then_replies_without_side_effects(daemon):
    sv = daemon.daemon.supervisor
    async def seed():
        d = daemon.daemon
        d.journal.append('chat.message', {'session_id': 's1', 'request_id': 'old', 'text': 'exact old visa detail'}, origin='owner')
        d.journal.append('chat.message', {'session_id': 'other', 'request_id': 'other', 'text': 'OTHER CHAT SECRET'}, origin='owner')
        d.journal.append('chat.message', {'session_id': 's1', 'request_id': 'newer', 'text': 'unrelated current exchange'}, origin='owner')
    daemon.run(seed())
    script = install(daemon, [json.dumps({'kind': 'context_read', 'text': '',
        'context_read': {'source': 'history', 'query': 'visa'}}), sup('reply', 'Here is the earlier detail.')])
    events = chat(daemon, 'What was that earlier visa detail?')
    requests = [r for _, key, r in script.requests if key == 'supervisor']
    assert len(requests) == 2
    assert 'exact old visa detail' not in requests[0].context
    assert 'exact old visa detail' in requests[1].context
    assert all('OTHER CHAT SECRET' not in r.context for r in requests)
    assert [e['text'] for e in events if e['type'] == 'reply'] == ['Here is the earlier detail.']
    async def trace():
        rows = list(daemon.daemon.db.conn.execute("SELECT input_json FROM spans WHERE kind='model.turn'"))
        reads = list(daemon.daemon.db.conn.execute("SELECT output_json FROM spans WHERE kind='planner.context_read'"))
        return [json.loads(row[0]) for row in rows], reads
    turns, reads = daemon.run(trace())
    assert [r['context_round'] for r in turns] == [0, 1]
    assert len(reads) == 1


def test_legacy_summary_does_not_hide_exact_recent_exchange_or_older_uncovered_history(daemon):
    sv = daemon.daemon.supervisor
    async def seed():
        old = sv.journal.append('chat.message', {'session_id': 's1', 'request_id': 'a', 'text': 'older topic'}, origin='owner')
        latest = sv.journal.append('chat.message', {'session_id': 's1', 'request_id': 'b', 'text': 'latest owner request'}, origin='owner')
        sv.journal.append('chat.summary', {'session_id': 's1', 'text': 'HUGE LEGACY SUMMARY',
                                         'covered': ['entry:' + old.id, 'entry:' + latest.id]}, origin='inferred')
        blocks, _ = sv._conversation_blocks('c', 's1', focused=True)
        return '\n'.join(b.text for b, _ in blocks), sv._env['c']['omitted_history']
    text, omitted = daemon.run(seed())
    assert 'latest owner request' in text and 'HUGE LEGACY SUMMARY' not in text
    assert 'not yet represented' in text and omitted == 1


def test_compact_summary_coverage_never_removes_latest_exchange(daemon):
    sv = daemon.daemon.supervisor
    async def seed():
        latest = sv.journal.append('chat.message', {'session_id': 's1', 'request_id': 'a', 'text': 'latest owner request'}, origin='owner')
        sv.journal.append('chat.summary', {'session_id': 's1', 'version': SUMMARY_VERSION, 'text': 'topic pointer',
                                         'covered': ['entry:' + latest.id]}, origin='inferred')
        blocks, _ = sv._conversation_blocks('b', 's1', focused=True)
        return '\n'.join(b.text for b, _ in blocks)
    text = daemon.run(seed())
    assert 'latest owner request' in text and 'topic pointer' in text


def test_retrieval_exposures_from_all_rounds_reach_the_task(daemon):
    from tests.conftest import done, outcome, task
    sv = daemon.daemon.supervisor
    async def seed():
        old = sv.journal.append('chat.reply', {'session_id': 's1', 'request_id': 'old', 'text': 'outside finding'}, origin='external:web:example.test')
        sv.journal.append('chat.message', {'session_id': 's1', 'request_id': 'newer', 'text': 'current exchange'}, origin='owner')
        return old.id
    entry = daemon.run(seed())
    script = install(daemon, [json.dumps({'kind': 'context_read', 'text': '',
        'context_read': {'source': 'history', 'id': entry}}),
        json.dumps({'kind': 'context_read', 'text': '', 'context_read': {'source': 'ability', 'id': 'browser_verification'}}),
        sup('task', 'Inspecting.', task('Inspect the recorded finding'))], {'worker': done('Inspected.')})
    result = outcome(chat(daemon, 'Inspect the recorded finding'))
    async def trace():
        origins = daemon.daemon.spans.exposure_origins(result['task_id'])
        rows = list(daemon.daemon.db.conn.execute("SELECT input_json, output_json FROM spans WHERE kind='context.exposure' AND task_id=?", (result['task_id'],)))
        return origins, '\n'.join(str(tuple(row)) for row in rows)
    origins, rows = daemon.run(trace())
    assert 'external:web:example.test' in origins
    assert 'entry:' + entry in rows and 'outside finding' in rows
    assert 'ability:browser_verification' in rows
    assert all(mode == 'supervisor' and not tools for mode, key, _, tools in script.contexts if key == 'supervisor')


def test_cancel_between_context_rounds_never_dispatches_an_action(daemon):
    import asyncio
    from ferb.orchestrator.supervisor import ChatSession
    sv = daemon.daemon.supervisor
    install(daemon, [])
    async def exercise():
        rounds = []
        async def run(*args, **kwargs):
            rounds.append(1)
            if len(rounds) == 1:
                return json.dumps({'kind': 'context_read', 'text': '', 'context_read': {'source': 'history'}})
            await asyncio.Future()
        original = sv._run
        sv._run = run
        try:
            call = asyncio.create_task(sv._supervise(ChatSession('s1'), 'earlier detail?', 'cancel-read'))
            while len(rounds) < 2:
                await asyncio.sleep(0)
            call.cancel()
            with pytest.raises(asyncio.CancelledError):
                await call
            return len(rounds)
        finally:
            sv._run = original
    assert daemon.run(exercise()) == 2


@pytest.mark.parametrize('preview', [False, True])
def test_newly_known_secret_is_redacted_before_history_page_or_preview_boundary(preview):
    """History can predate registration of a secret; transport must never cut it before redaction."""
    from ferb.sanitize.redact import Redactor
    secret = 'newly-known-private-phrase'
    record = {'id': 'historic', 'kind': 'chat.message', 'origin': 'owner', 'data': {'text': secret + ' tail'}}
    serialized = json.dumps(record['data'] if preview else record, ensure_ascii=False)
    boundary = 500 if preview else 1000  # existing history transport sizes
    padding = boundary - 5 - serialized.index(secret)
    record['data']['text'] = 'x' * padding + secret + ' tail'
    redactor = Redactor()
    redactor.retain_secret(secret)
    kwargs = dict(history=lambda tasks: [record], ability=lambda name: {}, skill=lambda name: {},
                  skill_ids=set(), redact=redactor)
    request = ContextRead(source='history', id=None if preview else 'historic', max_chars=1000)
    first, made = read_context(request, **kwargs)
    body = json.loads(first.payload)
    assert secret[:5] not in first.text
    assert made == {'owner'}
    if preview:
        expected = json.dumps(redactor(record)['data'], ensure_ascii=False)[:500]
        assert body['records'][0]['preview'] == expected
    else:
        pieces = [body['text']]
        offset = body['next_offset']
        while offset is not None:
            block, _ = read_context(ContextRead(source='history', id='historic', offset=offset, max_chars=1000), **kwargs)
            page = json.loads(block.payload)
            pieces.append(page['text'])
            offset = page['next_offset']
        assert ''.join(pieces) == json.dumps(redactor(record), ensure_ascii=False)
        assert secret not in ''.join(pieces)
    # Searching cannot disclose a newly-known secret through matches/counts either.
    found, _ = read_context(ContextRead(source='history', query=secret), **kwargs)
    assert json.loads(found.payload)['total_records'] == 0
