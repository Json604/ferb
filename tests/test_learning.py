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
    # review M6: an app update alone sends it back to candidate (re-verified on the new version), never suspect for good
    assert learning.procedure(p['id'], {'com.apple.mail': '2'})['state'] == 'candidate'
    assert learning.procedure(p['id'], {'com.apple.mail': '1'})['state'] == 'learned'
    newer = card(learning, 'on-2', slot='c', version='2')                    # a success on the updated app
    learning.receipt(p['id'], newer['id'], {'recipient': 'c'})
    assert learning.procedure(p['id'], {'com.apple.mail': '2'})['state'] == 'learned'


def test_only_failures_make_a_procedure_suspect(learning):
    a, b, c = [card(learning, str(i), slot=s) for i, s in enumerate(('a', 'b', 'a'))]
    p = procedure(learning, [a, b, c])
    for n in range(2):
        learning.receipt(p['id'], card(learning, f'f{n}', verified=False)['id'])
    assert learning.procedure(p['id'], {'com.apple.mail': '1'})['state'] == 'suspect'


def test_two_slot_values_are_two_real_values(learning):
    """Review M1 (probe P3): a run's receipt carries no slot values ({}), and it counted as a second slot value, so one
    real value plus two runs made a procedure learned."""
    first = card(learning, 't1', slot='a@x')
    p = procedure(learning, [first])
    for t in ('t2', 't3'):
        learning.receipt(p['id'], card(learning, t, slot='a@x')['id'], {})      # as a run records it: no slots
    state = learning.procedure(p['id'], {'com.apple.mail': '1'})
    assert state['state'] == 'candidate' and state['counters']['slot_variants'] == 1


def test_a_receipt_for_something_that_is_not_a_procedure_writes_nothing(learning):
    """Review M7: receipt() inserted its row before checking that the id named a procedure."""
    a, b = card(learning, 'a'), card(learning, 'b')
    with pytest.raises(LearningRefused):
        learning.receipt(a['id'], b['id'])
    assert learning.db.conn.execute('SELECT COUNT(*) FROM learning_receipts').fetchone()[0] == 0


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
    assert learning.prepare_episode('later-task')['forgotten'] == [source['id']]
    assert learning.capture_episode('later-task') is None
    assert 'Private message' not in json.dumps([dict(r) for r in learning.db.conn.execute('SELECT * FROM forgotten_refs')])


def test_a_suggestion_the_owner_turned_down_never_comes_back(learning):
    """Review M3: "Not right" (retire) left the suggestion free to be proposed again from the same evidence."""
    first = learning.put('identity_proposal', 'reports', {'text': 'Sends the weekly report by mail'}, [],
                         made_by='model:test')
    learning.db.conn.execute("UPDATE learning_nodes SET state='retired' WHERE id=?", (first['id'],))
    with pytest.raises(LearningRefused, match='not right'):
        learning.put('identity_proposal', 'reports', {'text': 'Sends reports by mail on Fridays'}, [],
                     made_by='model:test', batch='next-day')
    with pytest.raises(LearningRefused, match='not right'):
        learning.put('identity_proposal', 'weekly', {'text': 'Sends the weekly report by mail'}, [],
                     made_by='model:test', batch='next-day')
    assert learning.put('identity', 'reports', {'text': 'I send reports on Fridays'}, [], made_by='owner:identity/1',
                        origin_label='owner', owner=True, state='active')        # the owner's own word still counts


def _proposal(cards, steps, intent='send report'):
    return {'intent': intent, 'slots': {'recipient': 'string'}, 'preconditions': {'apps': {'com.apple.mail': '1'}},
            'steps': steps, 'ui_hints': '',
            'examples': [{'episode': c['id'], 'slots': c['body']['slots']} for c in cards]}


def _with_extra_action(store, ident, slot):
    """A verified run that also opened the drafts folder before sending."""
    return store.put('episode', ident, {'task_id': ident, 'verified': True, 'request': 'send report',
                     'apps': {'com.apple.mail': '1'}, 'slots': {'recipient': slot},
                     'actions': [{'id': ident + '-open', 'tool': 'mail.open', 'args': {'folder': 'Drafts'},
                                  'outside_action': False},
                                 {'id': ident + '-action', 'tool': 'mail.send', 'args': {'to': slot},
                                  'outside_action': False}],
                     'conditions': [{'met': True}], 'status': 'done'}, [], origin_label='ferb',
                     made_by='code:episode/1')


def test_an_optional_step_need_only_reproduce_the_runs_that_did_it(learning):
    """L29: one extra action in one run blocked the whole procedure; a step some runs skipped is optional."""
    a, b = card(learning, 'p1', slot='a'), card(learning, 'p2', slot='b')
    c = _with_extra_action(learning, 'p3', 'c')
    send = {'tool': 'mail.send', 'args': {'to': '{recipient}'}, 'expect': 'sent', 'optional': False,
            'evidence': ['p1-action', 'p2-action', 'p3-action']}
    drafts = {'tool': 'mail.open', 'args': {'folder': 'Drafts'}, 'expect': 'drafts shown', 'optional': True,
              'evidence': ['p3-open']}
    p = learning.propose_procedure(_proposal([a, b, c], [drafts, send]), 'b', 'model:test|prompt:1')
    assert [s['optional'] for s in p['body']['steps']] == [True, False]


def test_a_required_step_must_still_reproduce_every_run(learning):
    a, b = card(learning, 'q1', slot='a'), card(learning, 'q2', slot='b')
    c = _with_extra_action(learning, 'q3', 'c')
    drafts = {'tool': 'mail.open', 'args': {'folder': 'Drafts'}, 'expect': 'drafts shown', 'optional': False,
              'evidence': ['q3-open']}
    with pytest.raises(LearningRefused, match='reproduce'):
        learning.propose_procedure(_proposal([a, b, c], [drafts]), 'b', 'model:test|prompt:1')


def test_an_optional_step_that_reproduces_no_run_is_refused(learning):
    a, b, c = card(learning, 'r1', slot='a'), card(learning, 'r2', slot='b'), card(learning, 'r3', slot='c')
    wrong = {'tool': 'mail.send', 'args': {'to': 'attacker@example.com'}, 'expect': 'sent', 'optional': True,
             'evidence': ['r1-action']}
    with pytest.raises(LearningRefused, match='reproduce'):
        learning.propose_procedure(_proposal([a, b, c], [wrong]), 'b', 'model:test|prompt:1')


def test_a_procedure_for_an_intent_is_keyed_by_the_intent_id(learning):
    a, b, c = card(learning, 's1', slot='a'), card(learning, 's2', slot='b'), card(learning, 's3', slot='a')
    send = {'tool': 'mail.send', 'args': {'to': '{recipient}'}, 'expect': 'sent', 'optional': False,
            'evidence': ['s1-action', 's2-action', 's3-action']}
    p = learning.propose_procedure(_proposal([a, b, c], [send]), 'b', 'model:test|prompt:1', intent_id='i-report')
    assert p['key'] == 'i-report' and p['body']['intent'] == 'send report'


def test_a_step_proposed_without_optional_is_stored_as_required(learning):
    a, b, c = card(learning, 'u1', slot='a'), card(learning, 'u2', slot='b'), card(learning, 'u3', slot='a')
    p = procedure(learning, [a, b, c])                            # its step names no `optional`
    assert p['key'] == 'send report' and [s['optional'] for s in p['body']['steps']] == [False]


# L29 fix round 1 (I1): redaction keeps stored JSON valid ---------------------------------------------------------

@pytest.fixture
def redacting(tmp_path):
    """A store redacting as the daemon does (Redactor.text), with a registered secret value."""
    from ferb.sanitize.redact import Redactor
    db = Database(tmp_path / 'memory.db')
    db.migrate()
    redactor = Redactor()
    redactor.add_secret_value('2345')
    store = Learning(db, Clock(), lambda *args: None, redactor.text)
    noted = []
    store.note = lambda kind, data: noted.append((kind, data))
    yield store, noted
    db.close()


def test_a_body_whose_text_redaction_breaks_its_json_is_stored_redacted_and_valid(redacting):
    """A card-like number, or a registered secret inside a number, made the redacted text invalid JSON and the write
    raise. The body is redacted part by part instead: such a number becomes the redaction marker, and a code its key
    labels is still caught (it is only redacted beside its key)."""
    from ferb.sanitize.redact import REDACTED
    store, _ = redacting
    node = store.put('lesson', 'numbers', {'card': 4111111111111111, 'qty': 12345, 'code': '482913', 'dish': 'paneer',
                                           'count': 3, 'note': 'pin 2345 inside', 'ok': True}, [],
                     made_by='model:test')
    body = node['body']
    text = json.dumps(body)
    assert '4111111111111111' not in text and '2345' not in text and '482913' not in text
    assert body['card'] == REDACTED and body['qty'] == REDACTED and body['code'] == REDACTED
    assert (body['dish'], body['count'], body['ok']) == ('paneer', 3, True)


def test_a_receipts_number_slot_redaction_keeps_the_procedure_readable(redacting):
    """Review I1: a number slot whose digits hit a card pattern, or hold a registered secret, was stored as invalid
    JSON (`{"n":[redacted]}`), and every later refresh of the procedure raised (its state, its worker text,
    `ferb verdict`)."""
    from ferb.sanitize.redact import REDACTED
    store, noted = redacting
    cards = [card(store, f'n{i}', slot=s) for i, s in enumerate(('a', 'b', 'a'))]
    p = procedure(store, cards)
    runs = [card(store, ident, slot='c')['id'] for ident in ('n-card', 'n-secret')]
    for run, value in zip(runs, (4111111111111111, 12345)):
        store.receipt(p['id'], run, {'n': value})
    rows = [r[0] for r in store.db.conn.execute('SELECT slots_json FROM learning_receipts WHERE procedure_id=?',
                                                (p['id'],))]
    assert all(isinstance(json.loads(r), dict) for r in rows)
    assert not any('4111111111111111' in r or '2345' in r for r in rows)
    kept = [json.loads(r[0]) for r in store.db.conn.execute(
        f"SELECT slots_json FROM learning_receipts WHERE episode_id IN ({','.join('?' * len(runs))})", runs)]
    assert kept == [{'n': REDACTED}, {'n': REDACTED}] and not noted       # written valid, never repaired
    assert store.procedure(p['id'], {'com.apple.mail': '1'})['state'] == 'learned'


def test_an_unreadable_receipt_row_is_noted_and_never_breaks_the_procedure(redacting):
    """Defense in depth: a receipt row whose slot values don't parse (written before the fix) counts without values,
    is noted once and repaired, instead of raising on every refresh."""
    store, noted = redacting
    cards = [card(store, f'b{i}', slot=s) for i, s in enumerate(('a', 'b', 'a'))]
    p = procedure(store, cards)
    store.db.conn.execute('UPDATE learning_receipts SET slots_json=? WHERE episode_id=?',
                          ('{"recipient":[redacted]}', cards[0]['id']))
    for _ in range(2):
        assert store.procedure(p['id'], {'com.apple.mail': '1'})['counters']['verified_successes'] == 3
    assert noted == [('memory.receipt_unreadable', {'procedure_id': p['id'], 'episode_id': cards[0]['id']})]


def test_redacted_json_falls_back_value_by_value_when_structure_breaks():
    """The last resort: a redaction that breaks the JSON even with every number written as a string (here one that
    eats a closing quote) redacts each value with its key as one unit; a unit that no longer parses, or a number
    whose text changes, becomes the marker. A redaction that isn't stable on the stored text is refused."""
    from ferb.memory.learning import redacted_json
    from ferb.sanitize.redact import REDACTED

    def quote_eating(text: str) -> str:
        return text.replace('7', '#').replace('tok"', '[r]')
    assert redacted_json(quote_eating, {'a': 17, 'b': 'tok', 'c': [2, 'q'], 'd': None, 'e': False, 'f': 'x7'}) == \
        {'a': REDACTED, 'b': REDACTED, 'c': [2, 'q'], 'd': None, 'e': False, 'f': 'x#'}
    assert redacted_json(lambda t: t, {'a': 17, 'b': 'x7'}) == {'a': 17, 'b': 'x7'}       # no redaction: unchanged
    with pytest.raises(LearningRefused, match='secrets removed'):                          # never stable: refused
        redacted_json(lambda t: t + ' ', {'a': 1})


# L29 fix round 2 (re-review N1, N3): the last resort never stores a secret the whole text shows ------------------

_KEY = ["-----BEGIN RSA PRIVATE KEY-----", "MIIEowIBAAKCAQEAsecretkeymaterial", "-----END RSA PRIVATE KEY-----"]


@pytest.mark.parametrize('body, secrets', [
    ({'a': 'x', 'pin': '1234', 'z': 'password=abc', 'zz': ['y']}, ['1234', 'abc']),
    ({'password': 'hunter2', 'z': 'token=abc', 'zz': ['y']}, ['hunter2', 'abc']),
    ({'code': '482913', 'note': 'secret=abc', 'x': {'y': 'z'}}, ['482913', 'abc']),
    ({'actions': [{'args': {'command': 'mysql --password=hunter2'}, 'id': 'a1', 'tool': 'shell.run'}],
      'request': 'my pin: 4821', 'slots': {'pin': '4821'}}, ['hunter2', '4821']),
    ({'k': _KEY, 'n': 4111111111111111}, ['MIIEow', '4111111111111111']),
    ({'k': _KEY, 'z': 'password=abc', 'zz': ['y']}, ['MIIEow', 'abc']),       # the key's lines, in the last resort
])
def test_a_body_is_never_stored_with_a_secret_only_its_whole_text_shows(redacting, body, secrets):
    """Re-review N1: when both whole-text passes broke the JSON, the last resort redacted each key and string alone and
    stored a secret only its key labels ("pin":"1234") or a private key split across list items, in clear. Such a body
    is stored with them redacted (each value redacted with its key; the numbers-as-strings parse kept when only its
    shape changed), never with a secret its stored text would still show."""
    from ferb.sanitize.redact import Redactor
    store, _ = redacting
    node = store.put('lesson', 'k', body, [], made_by='model:test')
    text = json.dumps(node['body'], ensure_ascii=False)
    assert not any(s in text for s in secrets), text
    redactor = Redactor()
    redactor.add_secret_value('2345')
    stored = json.dumps(node['body'], ensure_ascii=False, sort_keys=True, separators=(',', ':'))
    assert redactor.text(stored) == stored                         # nothing left for a second redaction to find


def test_a_shape_changing_redaction_keeps_its_numbers_as_strings_parse(redacting):
    """When the numbers-as-strings pass parses but its shape changed (a private key's lines joined into one marker),
    that parse is kept, strings where the numbers were."""
    store, _ = redacting
    node = store.put('lesson', 'k', {'k': _KEY, 'n': 4111111111111111}, [], made_by='model:test')
    assert node['body'] == {'k': ['[redacted]'], 'n': '[redacted]'}


def test_a_secret_that_spans_two_numbers_is_refused_rather_than_stored(tmp_path):
    """Re-review N3: a registered value spanning two numbers (`1234,5678`) was un-redacted by writing the numbers as
    strings. The final check finds it again in the text to be stored, so the write is refused."""
    from ferb.sanitize.redact import Redactor
    db = Database(tmp_path / 'memory.db')
    db.migrate()
    redactor = Redactor()
    redactor.add_secret_value('1234,5678')
    store = Learning(db, Clock(), lambda *args: None, redactor.text)
    try:
        with pytest.raises(LearningRefused, match='secrets removed'):
            store.put('lesson', 'k', {'a': [1234, 5678]}, [], made_by='model:test')
    finally:
        db.close()
