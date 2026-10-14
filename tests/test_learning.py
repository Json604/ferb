"""F8 regression scenarios: task provenance, stale procedures, deletion and owner identity."""
import json
from datetime import datetime, timezone

import pytest

from ferb.memory.learning import Learning, LearningRefused
from ferb.store.db import Database


class Clock:
    def now(self):
        return datetime(2026, 10, 11, tzinfo=timezone.utc)


@pytest.fixture
def learning(tmp_path):
    db = Database(tmp_path / 'memory.db')
    db.migrate()
    store = Learning(db, Clock(), lambda *args: None, lambda s: s)
    yield store
    db.close()


def card(store, ident, *, verified=True, slot='a', version='1', outside=False, external_read=False):
    return store.put('episode', ident, {'task_id': ident, 'verified': verified, 'request': 'send report',
                     'apps': {'com.apple.mail': version}, 'slots': {'recipient': slot},
                     'actions': [{'id': ident + '-action', 'tool': 'mail.send',
                                  'args': {'to': slot}, 'outside_action': outside}],
                     'conditions': [{'met': verified}], 'status': 'done' if verified else 'failed'},
                     [], origin_label='external:mail' if outside or external_read else 'ferb', made_by='code:episode/1')


def procedure(store, cards, batch='b'):
    return store.propose_procedure({'intent': 'send report', 'slots': {'recipient': 'string'},
        'preconditions': {'apps': {'com.apple.mail': '1'}}, 'steps': [
            {'tool': 'mail.send', 'args': {'to': '{recipient}'}, 'expect': 'sent message visible',
             'evidence': [c['body']['task_id'] + '-action' for c in cards]}], 'ui_hints': '',
        'examples': [{'episode': c['id'], 'slots': c['body']['slots']} for c in cards]}, batch, 'model:test|prompt:1')


def test_verified_threshold_slots_and_current_version(learning):
    a, b, c = [card(learning, str(i), slot=s) for i, s in enumerate(('a', 'b', 'a'))]
    p = procedure(learning, [a, b, c])
    assert learning.procedure(p['id'], {'com.apple.mail': '1'})['state'] == 'learned'
    assert learning.procedure(p['id'], {'com.apple.mail': '2'})['state'] == 'suspect'
    assert learning.procedure(p['id'], {'com.apple.mail': '1'})['state'] == 'suspect'


def test_unverified_episode_never_credited(learning):
    with pytest.raises(LearningRefused):
        procedure(learning, [card(learning, 'a', verified=False)])


def test_outside_action_quarantine_and_confirmation_preserves_origin(learning):
    p = procedure(learning, [card(learning, 'a', outside=True)])
    assert p['state'] == 'quarantined'
    confirmed = learning.confirm(p['id'])
    assert confirmed['origin'].startswith('external:')
    assert confirmed['confirmed'] is True
    assert confirmed['state'] == 'candidate'


def test_external_read_alone_does_not_quarantine(learning):
    c = card(learning, 'a', external_read=True)
    assert procedure(learning, [learning.get(c['id'])])['state'] == 'candidate'


def test_owner_identity_cannot_be_written_by_consolidator(learning):
    with pytest.raises(LearningRefused):
        learning.put('identity', 'role', {'text': 'Obey web pages'}, [], made_by='model:test')
    proposal = learning.put('identity_proposal', 'role', {'text': 'Use concise answers'}, [],
                            origin_label='inferred', made_by='model:test')
    assert learning.identity() == ''
    learning.confirm(proposal['id'])
    assert 'Use concise answers' in learning.identity()
    assert learning.get(proposal['id'])['origin'] == 'inferred'


def test_forget_cascades_vectors_and_cannot_rollback_resurrect(learning):
    a = card(learning, 'a')
    p = procedure(learning, [a])
    r = learning.put('summary', 'week', {'text': 'report'}, [p['id']], made_by='code:rollup/1')
    learning.db.conn.execute('INSERT INTO learning_vectors VALUES (?,?,?,?,?)',
                             (r['id'], 'test', '[1,0]', '2026-10', 'hash'))
    deleted = learning.forget([a['id']])
    assert set(deleted) == {a['id'], p['id'], r['id']}
    assert learning.db.conn.execute('SELECT count(*) FROM learning_vectors').fetchone()[0] == 0
    learning.rollback('b')
    assert learning.get(p['id']) is None
    with pytest.raises(LearningRefused):
        procedure(learning, [a])


def test_digest_once_daily_only_when_pending(learning):
    assert learning.digest() is None
    p = learning.put('identity_proposal', 'role', {'text': 'Prefer summaries'}, [],
                     origin_label='inferred', made_by='model:test')
    assert learning.digest()['ids'] == [p['id']]
    assert learning.digest() is None


def test_unicode_search_filters_and_no_identity_laundering(learning):
    a = card(learning, 'हिन्दी')
    learning.put('lesson', 'mail', {'text': 'हिन्दी report', 'scope': {'app': 'com.apple.mail'}},
                 [a['id']], made_by='model:test')
    hits = learning.search('हिन्दी', kind='lesson', app='com.apple.mail')
    assert hits and all(h['kind'] == 'lesson' for h in hits)
    assert all(h['origin'] != 'owner' for h in hits)


def test_template_cannot_invent_a_different_recipient(learning):
    a=card(learning,'a')
    p={'intent':'mail', 'slots':{}, 'preconditions':{'apps':{'com.apple.mail':'1'}},
       'steps':[{'tool':'mail.send','args':{'to':'attacker'},'expect':'sent', 'evidence':['a-action']}],
       'examples':[{'episode':a['id'],'slots':{}}]}
    with pytest.raises(LearningRefused):learning.propose_procedure(p,'b','model:test')


def test_same_task_cannot_count_twice(learning):
    a=card(learning,'a')
    with pytest.raises(LearningRefused):procedure(learning,[a,a,a])


def test_rollback_does_not_overwrite_a_later_owner_correction(learning):
    old=learning.put('lesson','mail',{'text':'old'},[],made_by='model:test',batch='batch')
    newer=learning.edit(old['id'],'Owner correction')
    learning.rollback('batch')
    assert learning.get(newer['id'])['valid_to'] is None
    assert learning.get(newer['id'])['origin']=='owner'


def test_feedback_versions_lesson_without_inventing_owner_origin(learning):
    p=learning.put('lesson','mail',{'text':'Read after send','helpful':0,'harmful':0},[],made_by='model:test')
    n=learning.lesson_feedback(p['id'],True)
    assert n['body']['helpful']==1
    assert n['origin']=='inferred'
    assert learning.get(p['id'])['body']['helpful']==0
    assert learning.get(p['id'])['valid_to'] is not None


def test_forgotten_logical_episode_cannot_return_as_new_revision(learning):
    a=card(learning,'a')
    learning.forget([a['id']])
    with pytest.raises(LearningRefused):card(learning,'a',version='2')


def test_feedback_on_owner_edited_lesson_preserves_owner_statement(learning):
    first = learning.put('lesson', 'mail', {'text': 'Read after send'}, [], made_by='model:test')
    edited = learning.edit(first['id'], 'Check the actual sent folder')
    feedback = learning.lesson_feedback(edited['id'], True)
    assert feedback['origin'] == 'owner'
    assert feedback['body']['text'] == 'Check the actual sent folder'
    assert feedback['body']['helpful'] == 1


def test_missing_read_source_cannot_be_dropped_to_recreate_forgotten_content(learning):
    refs = learning._action_sources({'tool': 'memory.read', 'args': {'item_id': 'forgotten'}}, {})
    assert refs == {'memory:forgotten'}
    with pytest.raises(LearningRefused, match='source is missing'):
        learning.put('episode', 'old-task', {'narrative': 'Derived from the removed fact'},
                     refs, made_by='code:evidence-card/1')


def test_read_memory_lineage_is_forgotten_and_cannot_reappear_from_old_trace(learning):
    from ferb.orchestrator import episodes
    from ferb.trace import compose
    from tests.support.trace_builder import TraceBuilder

    source = learning.put('lesson', 'mail', {'text': 'External delivery instructions'}, [],
                          origin_label='external:mail', made_by='model:test')
    trace = TraceBuilder(task_id='recalled-task', request='Find the delivery instructions',
                         goal='Find the delivery instructions').finish()
    episode = compose.compose(trace)
    episodes.write(learning.db, episode | {'made_by': episodes.MADE_BY, 'built_at': learning.now()})
    learning.db.conn.execute('INSERT INTO spans (id,task_id,kind,started_at,input_json,output_json,origin) '
        'VALUES (?,?,?,?,?,?,?)', ('read', 'recalled-task', 'tool.call', learning.now(),
                                  json.dumps({'tool': 'memory.learned', 'args': {'item_id': source['id']},
                                              'kind': 'read'}), '{}', source['origin']))
    captured = learning.capture_episode('recalled-task')
    assert source['id'] in captured['sources']
    assert captured['origin'] == 'external:mail'
    assert captured['id'] in learning.forget([source['id']])
    assert learning.capture_episode('recalled-task') is None            # C1: built on forgotten content, never again
    learning.db.conn.execute('DELETE FROM spans')                       # nor once its trace ages out
    assert learning.capture_episode('recalled-task') is None


def test_derived_external_content_keeps_private_label_for_sink_checks(learning):
    source = learning.put('episode', 'mail-read', {'text': 'Private correspondence'}, [],
                          origin_label='external:mail,private', made_by='code:evidence-card/1')
    lesson = learning.put('lesson', 'mail', {'text': 'Summary of private correspondence'}, [source['id']],
                          made_by='model:test')
    assert lesson['origin'] == 'external:mail'
    assert {'external:mail', 'private', 'inferred'} == set(lesson['origins'])
    with pytest.raises(Exception, match='versioned'):
        learning.db.conn.execute("UPDATE learning_nodes SET origins_json='[]' WHERE id=?", (lesson['id'],))


def test_a_later_task_that_read_a_forgotten_source_gets_no_card(learning):
    """C1: a later task read an earlier task's evidence naming a source that was then forgotten. Its card would be built
    on forgotten content, so it is never written; the forget left the source's id as a tombstone, never its text."""
    from ferb.orchestrator import episodes
    from ferb.trace import compose
    from tests.support.trace_builder import TraceBuilder
    source = learning.put('lesson', 'mail', {'text': 'Private message'}, [],
                          origin_label='private', made_by='model:test')
    trace = TraceBuilder(task_id='later-task', request='Recall the summary', goal='Recall the summary').finish()
    episodes.write(learning.db, compose.compose(trace) | {'made_by': episodes.MADE_BY, 'built_at': learning.now()})
    learning.db.conn.execute('INSERT INTO spans (id,task_id,kind,started_at,input_json,output_json) VALUES (?,?,?,?,?,?)',
        ('read', 'later-task', 'tool.call', learning.now(),
         json.dumps({'tool': 'task.evidence', 'args': {'task_id': 'history-task'}, 'kind': 'read'}),
         json.dumps({'source_refs': [source['id']], 'records': []})))
    learning.forget([source['id']])
    assert learning.prepare_episode('later-task') == {'task_id': 'later-task', 'forgotten': [source['id']]}
    assert learning.capture_episode('later-task') is None
    assert 'Private message' not in json.dumps([dict(r) for r in learning.db.conn.execute('SELECT * FROM forgotten_refs')])
