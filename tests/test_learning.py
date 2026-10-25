"""F8 regression scenarios: task provenance, stale procedures, deletion and owner identity."""
import json
from datetime import datetime, timezone

import pytest

from ferb.memory.learning import CARD_MAKER, Learning, LearningRefused
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


def test_verified_threshold_slots_and_version_range(learning):
    a, b, c = [card(learning, str(i), slot=s) for i, s in enumerate(('a', 'b', 'a'))]
    p = procedure(learning, [a, b, c])
    assert p['body']['preconditions']['apps'] == {'com.apple.mail': ['1', '1']}
    assert learning.procedure(p['id'], {'com.apple.mail': '1'})['state'] == 'learned'
    # L36 §8: an update keeps it learned; the worker is told to verify each step on the new version
    assert learning.procedure(p['id'], {'com.apple.mail': '2'})['state'] == 'learned'
    assert 'New version: verify each step' in learning.procedure_parts(p['id'], {'com.apple.mail': '2'}, None)[0].text


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
    with pytest.raises(LearningRefused, match='stored safely'):                            # never stable: refused
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
    store.secret_values = redactor.secret_values                   # as the daemon wires it
    try:
        with pytest.raises(LearningRefused, match='could not be stored safely: redaction would still change it'):
            store.put('lesson', 'k', {'a': [1234, 5678]}, [], made_by='model:test')
    finally:
        db.close()


# L29 fix round 3 (re-review R1): ordinary cards and procedures are stored ---------------------------------------

def test_ordinary_cards_are_never_refused():
    """Re-review R1: compact JSON read as prose ran a label ("Pin", "Token", "password") across the following keys to a
    value with digits, so the final check refused ordinary cards (WhatsApp's "Pin message", "Pin chat"). The whole-text
    passes and the check read JSON with a space after ',' and ':': none of the 216 is refused."""
    from ferb.memory.learning import redacted_json
    from ferb.sanitize.redact import Redactor
    from tests.support.redaction_corpus import ordinary_cards
    redactor = Redactor()
    refused = []
    for name, body in ordinary_cards():
        try:
            redacted_json(redactor.text, body, redactor.secret_values())
        except LearningRefused:
            refused.append(name)
    assert refused == []


@pytest.mark.parametrize('click', ['Pin message', 'Pin chat', 'Token usage'])
def test_a_whatsapp_card_with_a_pin_click_is_stored(tmp_path, click):
    """The production path (`Learning.put`, a real Redactor as the daemon wires it): the card is stored, its click
    text as written."""
    from ferb.sanitize.redact import Redactor
    db = Database(tmp_path / 'memory.db')
    db.migrate()
    redactor = Redactor()
    store = Learning(db, Clock(), lambda *args: None, redactor.text)
    store.secret_values = redactor.secret_values
    from tests.support.redaction_corpus import ordinary_card
    body = ordinary_card()
    body['actions'][1]['args']['text'] = click
    try:
        node = store.put('episode', body['task_id'], body, [], origin_label='ferb', made_by='code:evidence-card/2',
                         state='active')
        assert node['body']['actions'][1]['args']['text'] == click
    finally:
        db.close()


def test_procedures_for_pinning_a_message_and_token_usage_are_stored(tmp_path):
    """Model-written procedures (`Learning.put` as `propose_procedure` stores them) whose steps click "Pin message" or
    "Token usage" are stored unchanged."""
    from ferb.sanitize.redact import Redactor
    db = Database(tmp_path / 'memory.db')
    db.migrate()
    redactor = Redactor()
    store = Learning(db, Clock(), lambda *args: None, redactor.text)
    store.secret_values = redactor.secret_values
    ev = ['9d8c7b6a-5f4e-4d3c-2b1a-0f9e8d7c6b5a', '1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d']
    bodies = [
        {'intent': 'pin a WhatsApp message', 'slots': {'chat': 'string'},
         'preconditions': {'apps': {'net.whatsapp.WhatsApp': '24.20.79'}},
         'steps': [{'tool': 'gui.click', 'args': {'text': '{chat}'}, 'expect': 'chat open', 'optional': False,
                    'evidence': ev[:1]},
                   {'tool': 'gui.click', 'args': {'text': 'Pin message'}, 'expect': 'pinned', 'optional': False,
                    'evidence': ev[1:]}],
         'ui_hints': 'Right-click the message, then Pin message.',
         'examples': [{'episode': 'm-' + 'a1' * 32, 'slots': {'chat': 'FERB test group'}}]},
        {'intent': 'check API token usage', 'slots': {}, 'preconditions': {'apps': {}},
         'steps': [{'tool': 'browser.navigate', 'args': {'url': 'https://platform.example.com/usage'},
                    'expect': 'usage page', 'optional': False, 'evidence': ev[:1]},
                   {'tool': 'browser.click', 'args': {'text': 'Token usage'}, 'expect': 'chart', 'optional': False,
                    'evidence': ev[1:]}],
         'ui_hints': '', 'examples': [{'episode': 'm-' + 'b2' * 32, 'slots': {}}]}]
    try:
        for body in bodies:
            node = store.put('procedure', body['intent'], body, [], made_by='model:test|prompt:1')
            assert node['body'] == body
    finally:
        db.close()


@pytest.mark.parametrize('body', [{'args': {'text': 'my password is sunshine'}, 'error': None},
                                  {'text': 'my password is sunshine'},
                                  {'items': ['my wifi password is sunshine', 'Reply'], 'n': 3}])
def test_a_secret_ending_a_string_is_redacted(redacting, body):
    """Inside JSON text the redactor misses a secret that ends a string (it is caught in compact JSON only when another
    key follows, by accident, and never with a space after ','): each string is also redacted on its own."""
    store, _ = redacting
    node = store.put('lesson', 'k', body, [], made_by='model:test')
    text = json.dumps(node['body'])
    assert 'sunshine' not in text and 'password is [redacted]' in text


# L29 fix round 4 (re-review Q1): a code labelled by its previous sibling value ------------------------------------

_SIBLING_CODES = [('PIN', '4821'), ('PIN:', '4821'), ('UPI PIN', '4821'), ('passcode', '7392'), ('Passcode:', '7392'),
                  ('the code is', '482913'), ('Enter the code', '482913')]


@pytest.mark.parametrize('body, code', [(shape(label, code), code) for label, code in _SIBLING_CODES for shape in (
    lambda label, code: {'label': label, 'value': code},
    lambda label, code: {'name': label, 'text': code},
    lambda label, code: {'a': label, 'b': code, 'c': 'x'},
    lambda label, code: {'a': label, 'b': int(code)},
    lambda label, code: {'nodes': [{'text': label}, {'text': code}]})])
def test_a_code_labelled_by_its_previous_sibling_is_redacted(redacting, body, code):
    """Re-review Q1: compact JSON read a label ending one value ("PIN") together with the next value; the spaced
    reading stops at the structure and stored the code in clear. Neighbouring values (a list's nodes by their text too)
    are also read as two lines of prose, and a value that reading masks is redacted."""
    store, _ = redacting
    node = store.put('lesson', 'k', body, [], made_by='model:test')
    assert code not in json.dumps(node['body']), node['body']


def test_the_sibling_reading_leaves_ordinary_bodies_as_they_were(monkeypatch):
    """The 216 ordinary cards and re-review 4's 443 realistic bodies: never refused, and stored exactly as without the
    sibling reading ("Pin message" beside an id, "Token usage" beside a count, "Enter OTP" beside a date)."""
    import ferb.memory.learning as learning_module
    from ferb.memory.learning import encoded, redacted_json
    from ferb.sanitize.redact import Redactor
    from tests.support.redaction_corpus import ordinary_cards, realistic_bodies
    redactor = Redactor()
    bodies = [*ordinary_cards(), *realistic_bodies()]
    stored = {name: encoded(redacted_json(redactor.text, body, ())) for name, body in bodies}
    monkeypatch.setattr(learning_module, '_siblings', lambda redact, value: value)
    without = {name: encoded(redacted_json(redactor.text, body, ())) for name, body in bodies}
    assert len(stored) == 659 and [n for n in stored if stored[n] != without[n]] == []


@pytest.mark.parametrize('label', ['PIN:', 'Password:', 'Passcode:', 'API key:', 'Token:'])
def test_the_sibling_reading_never_masks_ferbs_own_ids(redacting, label):
    """Re-review 5, V1: a value ending in a secret label with a colon masked the FERB id beside it (an action's `id`
    after its `error`; a label's `intent_id` after its `description`), moving the intent under "[redacted]". FERB's own
    ids are neither read as a neighbour nor masked by that reading."""
    from ferb.memory import intents
    store, _ = redacting
    iid = intents.intent_id('enter the code')
    label_node = store.put(intents.INTENT_KIND, 'card-x', {'intent_id': iid, 'name': 'enter the code',
                           'description': f'Type the code where the page asks {label}', 'slots': {}, 'values': {}},
                           [], made_by='model:test')
    assert label_node['body']['intent_id'] == iid
    card = store.put('episode', 't-ids', {'task_id': 't-ids', 'actions': [
        {'error': f'no field labelled {label}', 'id': 't-ids-a1', 'tool': 'browser.type'}]},
        [], made_by='code:evidence-card/2')
    assert card['body']['actions'][0]['id'] == 't-ids-a1' and card['body']['task_id'] == 't-ids'


# L29 fix round 4 (re-review O3): slot types are a closed set, never redacted -----------------------------------

@pytest.mark.parametrize('name', ['token', 'pin', 'password', 'passcode', 'api_key', 'secret'])
def test_a_slot_named_like_a_secret_keeps_its_type_and_never_crashes(redacting, name):
    """Re-review O3: a slot named like a credential had its type redacted by key (`{"token": "[redacted]"}`), and
    typing the planner's values against it raised KeyError. Slot types come from a closed set: restored after
    redaction, for a procedure and for an intent label, and the planner's values type against them."""
    from ferb.memory import intents
    store, _ = redacting
    procedure = store.put('procedure', 'check price', {'intent': 'check price', 'slots': {name: 'string'},
                          'preconditions': {'apps': {}}, 'steps': []}, [], made_by='model:test')
    assert procedure['body']['slots'] == {name: 'string'}
    assert store.slot_values(procedure['id'], [{'name': name, 'value': '"BTC"'}]) is not None
    assert store.slot_problem(procedure['id'], [{'name': name, 'value': '"BTC"'}]) is None
    assert store.slot_problem(procedure['id'], [{'name': name, 'value': '3'}]) == f'slot {name} is not a string'
    c = card(store, 'slot-' + name)
    label = store.put(intents.INTENT_KIND, c['id'], {'intent_id': intents.intent_id('check price'),
                                                     'name': 'check price', 'description': 'a price',
                                                     'slots': {name: 'string'}, 'values': {name: 'BTC'}},
                      [c['id']], made_by='model:test', state='active')
    assert label['body']['slots'] == {name: 'string'}


def test_an_unknown_slot_type_is_a_mismatch_not_a_crash():
    """A type outside the closed set (a slot type stored redacted before the fix) is refused like any mismatch."""
    from ferb.memory import intents
    with pytest.raises(LearningRefused, match='JSON type'):
        intents.typed({'token': 'BTC'}, {'token': '[redacted]'})


def test_an_actions_target_provenance_is_read_in_the_gateways_vocabulary(tmp_path):
    """L29 Task 6 review I1 (an F8 bug): the card reader tested the gateway's `arg_sources` for origin labels ("owner",
    an outside rank), but the gateway records provenance labels (`policy.provenance`: "owner_words", "owner_answer",
    the outside content's origins, or "unknown"), so every action with a target read as outside and its procedure was
    quarantined, even one whose target the owner typed. The labels here come from `provenance.arg_sources` itself."""
    from ferb.core.types import Target
    from ferb.policy import provenance
    from tests.support.cards import card_store, finished_task
    db, _, spans, store = card_store(tmp_path)
    page = [('external:web:shop.example', 'write to sales@shop.example for the price')]
    cases = {                              # task -> (its arg_sources, or None when none were recorded; expected state)
        'typed': (provenance.arg_sources(Target(recipients=('sales@shop.example',)),
                                         'email sales@shop.example for a quote', page), 'candidate'),
        'answered': (provenance.arg_sources(Target(recipients=('sales@shop.example',)), 'email the shop', page,
                                            answer_text='sales@shop.example'), 'candidate'),
        'outside': (provenance.arg_sources(Target(recipients=('sales@shop.example',)), 'email the shop', page),
                    'quarantined'),
        'chosen': (provenance.arg_sources(Target(recipients=('sales@shop.example',)), 'email the shop', []),
                   'candidate'),
        'unrecorded': (None, 'quarantined'),    # a send without provenance: before F3, or damaged (plan 2026-10-23)
    }
    assert [cases[k][0] for k in ('typed', 'answered', 'outside', 'chosen')] == [   # the gateway's labels
        {'recipient:0': ['owner_words', 'external:web:shop.example']},
        {'recipient:0': ['owner_answer', 'external:web:shop.example']},
        {'recipient:0': ['external:web:shop.example']}, {'recipient:0': ['unknown']}]
    try:
        for task_id, (sources, expected) in cases.items():
            finished_task(db, spans, store, task_id)
            db.conn.execute("UPDATE episodes SET verified=1, status='done' WHERE task_id=?", (task_id,))
            inp = {'tool': 'mail.send', 'args': {'to': 'sales@shop.example'}, 'kind': 'send',
                   'target': {'recipients': ['sales@shop.example']}} | (
                {} if sources is None else {'arg_sources': sources})
            call = spans.start('tool.call', task_id=task_id, input=inp)
            spans.finish(call, output={'ok': True}, origin='ferb')
            c = store.capture_episode(task_id)
            [action] = [a for a in c['body']['actions'] if a['tool'] == 'mail.send']
            assert action['outside_action'] is (expected == 'quarantined'), task_id
            p = store.propose_procedure({'intent': 'ask the shop for a quote', 'slots': {},
                                         'preconditions': {'apps': {}}, 'ui_hints': '',
                                         'steps': [{'tool': 'mail.send', 'args': {'to': 'sales@shop.example'},
                                                    'expect': 'sent', 'evidence': [action['id']]}],
                                         'examples': [{'episode': c['id'], 'slots': {}}]},
                                        'b-' + task_id, 'model:test', intent_id='i-' + task_id)
            assert p['state'] == expected, task_id
    finally:
        db.close()
    from ferb.memory.learning import outside_targets
    assert outside_targets({'recipient:0': ['owner']}) is True          # not a provenance label: no writer uses it
    assert outside_targets({'account': ['a-label-it-does-not-know']}) is True  # an unknown label counts as outside
    assert outside_targets({'site': ['owner_words'], 'recipient:0': ['external:mail']}) is True    # per key
    assert outside_targets({}) is False                                 # an action without a target value


def test_which_recorded_provenance_counts_as_outside():
    """Plan 2026-10-23 Task 2 (owner: "cant we just leave browser thigns as it is"; L29, MP2, L2): the card reader no
    longer reads the site an action ran on as outside (under L2 any site is reached without a question, while the page
    FERB read names its own address), nor an action without provenance whose call the gateway records none for: a
    reversible kind without a declared purpose (`gateway.py`: consequential = kind not in REVERSIBLE or declared). Any
    other action without provenance (a record from before F3, or a damaged one) stays outside, so an old send keeps its
    hold; so does provenance in another shape. Every key but the site is read as before, and the site too when it is
    the call's destination (fix round 1, SQ7)."""
    from ferb.memory.learning import outside_action, outside_targets
    page = 'external:web:shop.example'
    for kind in ('navigate', 'open_tab', 'trash', 'timer', 'snapshot'):     # the gateway records none for these
        assert outside_action({'kind': kind}) is False, kind                            # no field at all
        assert outside_action({'kind': kind, 'arg_sources': None}) is False, kind
        assert outside_action({'kind': kind, 'arg_sources': 'damaged'}) is True, kind  # present, another shape
        assert outside_action({'kind': kind, 'declared': 'send'}) is True, kind        # recorded when declared
        assert outside_action({'kind': kind, 'declared': 'send',
                               'arg_sources': {'recipient:0': ['owner_words']}}) is False, kind
        assert outside_action({'kind': kind, 'declared': 'send',
                               'arg_sources': {'recipient:0': ['external:mail']}}) is True, kind
    for kind in ('send', 'fill_form', 'click', 'mac_input', 'pay', None):  # recorded since F3: none isn't FERB's choice
        assert outside_action({'kind': kind}) is True, kind
        assert outside_action({'kind': kind, 'arg_sources': None}) is True, kind
        assert outside_action({'kind': kind, 'arg_sources': ['external:mail']}) is True, kind
    assert outside_action({}) is True                                   # no kind recorded either
    # a damaged kind (or none: before F0) is read as sending, never as a read or a reversible kind, and never raises
    for kind in (['send'], {'kind': 'send'}, 7, None):
        assert outside_action({'kind': kind}) is True
        assert outside_action({'kind': kind, 'arg_sources': None}) is True
        assert outside_action({'kind': kind, 'arg_sources': {'site': [page]}}) is True             # its destination
        assert outside_action({'kind': kind, 'arg_sources': {'site': ['owner_words']}}) is False
        assert outside_action({'kind': kind, 'arg_sources': {'site': [page], 'recipient:0': ['owner_words']}}) is False
    # the declared purpose is read as the gateway's own reader reads it (`rules.declared_of`); a shape the gateway never
    # writes (not a kind or a list of kinds) is read as sending
    for declared in ({'damaged': 'send'}, [['send']], ['send', 3], 3, ('send',), {'send'}):
        assert outside_action({'kind': 'click', 'declared': declared, 'arg_sources': {'site': [page]}}) is True
        assert outside_action({'kind': 'navigate', 'declared': declared}) is True, declared   # recorded when declared
    for declared in ('', []):                                           # an empty one declares nothing
        assert outside_action({'kind': 'click', 'declared': declared, 'arg_sources': {'site': [page]}}) is False
        assert outside_action({'kind': 'navigate', 'declared': declared}) is False
    # Fix round 1 (SQ7): the site is judged when it is where the call sends, as the floor and sink rule (a) treat it
    # (`confirm.site_is_destination`): a send-class kind or declared purpose with no recipient or contact
    # not sending: the site is only where it happens
    for action in ({'kind': 'fill_form'}, {'kind': 'click'}, {'kind': 'mac_input'},
                   {'kind': 'click', 'declared': 'pay'}, {'kind': 'click', 'declared': 'delete_permanent'}):
        assert outside_action(action | {'arg_sources': {'site': [page]}}) is False, action
    for action in ({'kind': 'send'}, {'kind': 'post'}, {'kind': 'submit'}, {'kind': 'click', 'declared': 'send'},
                   {'kind': 'mac_input', 'declared': 'call'}, {'kind': 'click', 'declared': ['pay', 'send']},
                   {'kind': 'click', 'declared': {'damaged': 'send'}}, {'kind': 'click', 'declared': [['send']]}):
        assert outside_action(action | {'arg_sources': {'site': [page]}}) is True, action            # its destination
        assert outside_action(action | {'arg_sources': {'site': ['owner_words', page]}}) is False, action
        assert outside_action(action | {'arg_sources': {'site': ['unknown']}}) is False, action
        assert outside_action(action | {'arg_sources': {'site': [page], 'recipient:0': ['owner_words']}}) is False
        assert outside_action(action | {'arg_sources': {'site': [page], 'contact': ['owner_answer']}}) is False
        assert outside_action(action | {'arg_sources': {'site': [page], 'contact': [page]}}) is True
    assert outside_targets(None) is True                                # no provenance never clears a target
    assert outside_targets({}) is False
    assert outside_targets('site: external:web:shop.example') is True   # a damaged record: not a key -> labels map
    assert outside_targets(['external:mail']) is True
    assert outside_targets({'site': [page]}) is False                   # the site: only where the call happens
    assert outside_targets({'site': ['a-label-it-does-not-know']}) is False
    assert outside_targets({'site': [page]}, read_site=True) is True             # where it sends
    assert outside_targets({'site': ['owner_words', page]}, read_site=True) is False
    for key in ('account', 'path', 'contact', 'recipient:0', 'amount', 'a-key-a-later-gateway-writes'):
        assert outside_targets({'site': ['owner_words'], key: [page]}) is True, key
        assert outside_targets({'site': [page], key: ['owner_words', page]}) is False, key
        assert outside_targets({'site': [page], key: ['owner_answer']}) is False, key
        assert outside_targets({'site': [page], key: ['unknown']}) is False, key
    assert outside_action({'kind': 'read', 'arg_sources': {'recipient:0': [page]}}) is False   # a read: never
    assert outside_action({'kind': 'mailbox_read'}) is False


def test_a_card_read_before_the_provenance_fix_is_read_again_once(tmp_path):
    """L29 Task 6 review I1: a card the earlier composer built marks every targeted action outside; it is read again
    once (a new composer), so the runs already on the Mac are learned from with their targets read correctly."""
    from ferb.memory.learning import digest
    from tests.support.cards import card_store, finished_task
    db, _, spans, store = card_store(tmp_path)
    try:
        finished_task(db, spans, store, 't')
        built_at, status, verified = db.conn.execute(
            'SELECT built_at, status, verified FROM episodes WHERE task_id = ?', ('t',)).fetchone()
        assert CARD_MAKER != 'code:evidence-card/2'
        store.card_read('t', digest(['code:evidence-card/2', built_at, status, verified, None]))
        assert store.cards_to_read() == ['t']
        store.capture_episode('t')
        assert store.cards_to_read() == []
    finally:
        db.close()


# Plan 2026-10-23 Task 2 (L29, MP2, L2): browser procedures without a hold. Runs recorded as the gateway records them:
# each call's `tool`, `args`, `kind` and `target`, and `arg_sources` (from `provenance.arg_sources`) for a consequential
# kind only. The owner never names the shop; its menu page names its own address and two shop addresses.
SHOP = 'https://shop.example/menu'
SHOP_PAGE = [('external:web:shop.example', f'Menu {SHOP}: egg curry 120, paneer 140. Orders: orders@shop.example, '
                                           'invoices: billing@shop.example')]
CHROME = {'com.google.Chrome': '141.0'}
SHOP_DISHES = ('egg curry', 'paneer', 'egg curry')                    # three runs, two dishes


def _gateway_task(db, spans, store, task_id, calls):
    """A verified finished task whose tool calls are recorded as `calls` (input, the result's origin)."""
    from tests.support.cards import finished_task
    finished_task(db, spans, store, task_id)
    db.conn.execute("UPDATE episodes SET verified=1, status='done' WHERE task_id=?", (task_id,))
    for inp, where in calls:
        call = spans.start('tool.call', task_id=task_id, input=inp)
        spans.finish(call, output={'ok': True}, origin=where)


def _gateway_run(db, spans, store, task_id, calls, apps):
    """`_gateway_task`, read into its card by the card reader (`capture_episode`)."""
    _gateway_task(db, spans, store, task_id, calls)
    return store.capture_episode(task_id, apps)


def _shop_calls(request, dish):
    """One run on the shop: navigate to its menu (reversible: no `arg_sources`), read it, type the dish, click order,
    and type each shop address into a field (typed text shaped like an address is the call's recipient, S4 b)."""
    from ferb.core.types import Target
    from ferb.policy import provenance
    on_page = {'site': 'shop.example', 'page': SHOP}
    site = provenance.arg_sources(Target(site='shop.example', page=SHOP), request, SHOP_PAGE)
    assert site == {'site': ['external:web:shop.example']}                  # the page named the site, the owner didn't

    def act(element, action, text=None, recipient=None):
        args = {'tab': 't1', 'element': element, 'action': action} | ({'text': text} if text else {})
        sources = site if recipient is None else provenance.arg_sources(
            Target(site='shop.example', page=SHOP, recipients=(recipient,)), request, SHOP_PAGE)
        return ({'tool': 'browser.act', 'args': args, 'kind': 'click' if action == 'click' else 'fill_form',
                 'target': on_page | ({'recipients': [recipient]} if recipient else {}), 'arg_sources': sources},
                'ferb')
    return [({'tool': 'browser.navigate', 'args': {'tab': 't1', 'url': SHOP}, 'kind': 'navigate',
              'target': {'site': 'shop.example'}}, 'ferb'),
            ({'tool': 'browser.read', 'args': {'tab': 't1'}, 'kind': 'read', 'target': {'site': 'shop.example'}},
             'external:web:shop.example'),
            act('e4', 'type', dish), act('e9', 'click'),
            act('e12', 'type', 'orders@shop.example', 'orders@shop.example'),
            act('e13', 'type', 'billing@shop.example', 'billing@shop.example')]


def _flags(card):
    """Each action's outside flag by its tool and typed text (one card's spans start at one instant: any order)."""
    return {(a['tool'], a['args'].get('text')): a['outside_action'] for a in card['body']['actions']}


def _shop_runs(db, spans, store, prefix='shop'):
    return [_gateway_run(db, spans, store, f'{prefix}{n}', _shop_calls(f'find me the cheapest {dish}', dish), CHROME)
            for n, dish in enumerate(SHOP_DISHES)]


_SHOP_STEPS = {'nav': ('browser.navigate', {'tab': 't1', 'url': SHOP}),
               'type': ('browser.act', {'tab': 't1', 'element': 'e4', 'action': 'type', 'text': '{dish}'}),
               'click': ('browser.act', {'tab': 't1', 'element': 'e9', 'action': 'click'}),
               'orders': ('browser.act', {'tab': 't1', 'element': 'e12', 'action': 'type',
                                          'text': 'orders@shop.example'}),
               'billing': ('browser.act', {'tab': 't1', 'element': 'e13', 'action': 'type',
                                           'text': 'billing@shop.example'})}


def _shop_procedure(store, cards, *names):
    """The intent's procedure with these steps over `cards`, as a model proposes it (each step cites its action in
    every run)."""
    from ferb.memory.learning import render_slots

    def cited(card, dish, name):
        tool, args = _SHOP_STEPS[name]
        return next(a['id'] for a in card['body']['actions']
                    if a['tool'] == tool and a['args'] == render_slots(args, {'dish': dish}))
    steps = [{'tool': _SHOP_STEPS[n][0], 'args': _SHOP_STEPS[n][1], 'expect': 'ok', 'optional': False,
              'evidence': [cited(c, d, n) for c, d in zip(cards, SHOP_DISHES)]} for n in names]
    return store.propose_procedure({'intent': 'cheapest food order', 'slots': {'dish': 'string'},
                                    'preconditions': {'apps': CHROME}, 'ui_hints': '', 'steps': steps,
                                    'examples': [{'episode': c['id'], 'slots': {'dish': d}}
                                                 for c, d in zip(cards, SHOP_DISHES)]},
                                   'b-' + '-'.join(names) + str(len(cards)), 'model:test', intent_id='i-shop')


def test_a_browser_procedure_on_a_site_ferb_read_is_learned_without_a_confirm(tmp_path):
    """Task 2: navigating, typing and clicking on a site whose page FERB read, which the owner never named, held every
    browser procedure until `ferb memory confirm` (the page names its own site; a navigation records no provenance).
    Now it is a candidate after one run and learned after three verified runs with two dishes, with no confirm."""
    from tests.support.cards import card_store
    db, _, spans, store = card_store(tmp_path)
    try:
        cards = _shop_runs(db, spans, store)
        assert _flags(cards[0]) == {('browser.navigate', None): False, ('browser.read', None): False,
                                    ('browser.act', 'egg curry'): False, ('browser.act', None): False,
                                    ('browser.act', 'orders@shop.example'): True,      # only the page gave them
                                    ('browser.act', 'billing@shop.example'): True}
        first = _shop_procedure(store, cards[:1], 'nav', 'type', 'click')
        assert (first['state'], first['confirmed']) == ('candidate', False)
        learned = _shop_procedure(store, cards, 'nav', 'type', 'click')        # revised after the third run
        assert (learned['state'], learned['confirmed']) == ('learned', False) and 'requarantined' not in learned
        assert store.procedure(learned['id'], CHROME)['counters'] == {'verified_successes': 3, 'failures': 0,
                                                                     'slot_variants': 2}
        assert [p['node']['id'] for p in store.learned_procedures()] == [learned['id']]
    finally:
        db.close()


def test_a_send_to_a_recipient_found_only_in_an_email_still_holds_its_procedure(tmp_path):
    """Task 2 keeps MP2: a WhatsApp-style send to a number that only an email body gave is held until the owner
    confirms; the same number in the owner's own words is a candidate."""
    from ferb.core.types import Target
    from ferb.policy import provenance
    from tests.support.cards import card_store
    whatsapp, number = 'net.whatsapp.whatsapp', '+91 98765 43210'
    email = [('external:mail', f'Hi! Please confirm the order with my assistant on WhatsApp: {number}. Thanks')]
    db, _, spans, store = card_store(tmp_path)
    cases = {'emailed': ("confirm the caterer's order on whatsapp with the number in their email", 'quarantined',
                         {'site': ['unknown'], 'recipient:0': ['external:mail']}),
             'typed': (f"confirm the caterer's order on whatsapp with {number}", 'candidate',
                       {'site': ['unknown'], 'recipient:0': ['owner_words', 'external:mail']})}
    try:
        for task_id, (request, expected, labels) in cases.items():
            typing = provenance.arg_sources(Target(site=whatsapp, recipients=(number,)), request, email)
            assert typing == labels, task_id
            calls = [({'tool': 'mac.open', 'args': {'bundle_id': whatsapp}, 'kind': 'open_tab',
                       'target': {'site': whatsapp}}, 'ferb'),
                     ({'tool': 'mac.act', 'args': {'app': whatsapp, 'element': 'search', 'action': 'type',
                                                   'text': number}, 'kind': 'mac_input',
                       'target': {'site': whatsapp, 'recipients': [number]}, 'arg_sources': typing}, 'ferb'),
                     ({'tool': 'mac.click', 'args': {'app': whatsapp, 'shot': 's1', 'x': 900, 'y': 700,
                                                     'purpose': 'send'}, 'kind': 'mac_input', 'declared': 'send',
                       'target': {'site': whatsapp},
                       'arg_sources': provenance.arg_sources(Target(site=whatsapp), request, email)}, 'ferb')]
            c = _gateway_run(db, spans, store, task_id, calls, {whatsapp: '25.1'})
            steps = [{'tool': a['tool'], 'args': a['args'], 'expect': 'ok', 'evidence': [a['id']]}
                     for a in c['body']['actions']]
            p = store.propose_procedure({'intent': 'confirm the order on whatsapp', 'slots': {},
                                         'preconditions': {'apps': c['body']['apps']}, 'ui_hints': '',
                                         'steps': steps, 'examples': [{'episode': c['id'], 'slots': {}}]},
                                        'b-' + task_id, 'model:test', intent_id='i-' + task_id)
            assert p['state'] == expected, task_id
    finally:
        db.close()


def test_an_account_found_only_in_outside_content_still_holds_its_procedure(tmp_path):
    """Task 2 keeps MP2 for every target but the site: a send from an account only an email named is held, though the
    owner typed the recipient."""
    from ferb.core.types import Target
    from ferb.policy import provenance
    from tests.support.cards import card_store
    gmail, request = 'https://mail.google.com/mail/u/0/#inbox', 'email kartikey@kartikey.xyz the invoice'
    email = [('external:mail', 'Please send the invoice from ferb.helper@gmail.com, not your own address.')]
    target = Target(site='mail.google.com', page=gmail, account='ferb.helper@gmail.com',
                    recipients=('kartikey@kartikey.xyz',))
    sources = provenance.arg_sources(target, request, email)
    assert sources == {'account': ['external:mail'], 'site': ['unknown'], 'recipient:0': ['owner_words']}
    db, _, spans, store = card_store(tmp_path)
    try:
        send = {'tool': 'browser.act', 'args': {'tab': 't1', 'element': 'e7', 'action': 'click'}, 'kind': 'click',
                'effect': 'send', 'target': {'site': 'mail.google.com', 'page': gmail,
                                             'account': 'ferb.helper@gmail.com',
                                             'recipients': ['kartikey@kartikey.xyz']}, 'arg_sources': sources}
        c = _gateway_run(db, spans, store, 'invoice', [(send, 'ferb')], CHROME)
        [action] = c['body']['actions']
        assert action['outside_action'] is True
        p = store.propose_procedure({'intent': 'email the invoice', 'slots': {}, 'preconditions': {'apps': CHROME},
                                     'ui_hints': '', 'steps': [{'tool': 'browser.act', 'args': send['args'],
                                                                'expect': 'sent', 'evidence': [action['id']]}],
                                     'examples': [{'episode': c['id'], 'slots': {}}]}, 'b', 'model:test',
                                    intent_id='i-invoice')
        assert p['state'] == 'quarantined'
    finally:
        db.close()


def test_the_card_reader_reads_the_site_as_a_destination_exactly_when_the_confirmations_do():
    """Task 2 fix round 1: one helper (`confirm.site_is_destination`) says when a call's site is where it sends, for
    the confirmations and the card reader alike: over every kind, declared purpose and recipient here, `decide` asks
    about the site itself (the floor for an unseen recipient, or the sink rule's new destination) exactly when the
    helper says so, and the card reader then judges the site's provenance."""
    from ferb.core.types import Target
    from ferb.memory.learning import outside_action
    from ferb.policy import confirm
    seen = set()
    for kind in ('click', 'fill_form', 'mac_input', 'send', 'post', 'submit', 'reply', 'mailbox_send', 'write_file'):
        for declared in (None, 'send', 'call', 'pay', 'delete_permanent', ['pay', 'send']):
            for who in ((), ('a@x.example',)):
                target = Target(site='refunds.evil.example', page='https://refunds.evil.example/claim',
                                recipients=who)
                sources = {'site': ['external:web:news.example']} | {f'recipient:{i}': ['owner_words']
                                                                     for i in range(len(who))}
                needs = confirm.decide(kind, declared, target, sources, read_private=True, read_external=True,
                                       known=lambda *_: False)
                about_site = any(n.rule == 'sink:new_destination' or (n.rule == 'floor:new_recipient'
                                                                       and not n.values['recipients']) for n in needs)
                kinds = {kind} | ({declared} if isinstance(declared, str) else set(declared or ()))
                helper = confirm.site_is_destination(kinds, sources)
                assert about_site is helper, (kind, declared, who)
                card = {'kind': kind, 'arg_sources': sources} | ({'declared': declared} if declared else {})
                assert outside_action(card) is helper, (kind, declared, who)
                seen.add(helper)
    assert seen == {True, False}


def test_a_send_whose_only_destination_is_a_site_only_a_page_named_still_holds_its_procedure(tmp_path):
    """Task 2 fix round 1 (SQ7, MP2): a click declared to send that names no recipient sends to the site itself, as the
    floor and the sink rule (a) read it (`confirm.site_is_destination`). On a site only a news page named it is held;
    on the site the owner named it is not; with a recipient the owner typed the site is only where it happens. The
    card keeps the declared purpose, so a card read again after its trace aged out judges it the same."""
    from ferb.core.types import Target
    from ferb.policy import provenance
    from tests.support.cards import card_store
    claim = 'https://refunds.evil.example/claim'
    news = [('external:web:news.example', f'Readers can claim a refund at {claim} today')]
    cases = {   # task -> (request, the declared recipient or None, expected state, the gateway's labels)
        'paged': ('claim my refund from the news story', None, 'quarantined',
                  {'site': ['external:web:news.example']}),
        'named': ('claim my refund at refunds.evil.example', None, 'candidate',
                  {'site': ['owner_words', 'external:web:news.example']}),
        'to-owner': ('send the refund form to kartikey@kartikey.xyz', 'kartikey@kartikey.xyz', 'candidate',
                     {'site': ['external:web:news.example'], 'recipient:0': ['owner_words']})}
    db, _, spans, store = card_store(tmp_path)
    try:
        for task_id, (request, to, expected, labels) in cases.items():
            target = Target(site='refunds.evil.example', page=claim, recipients=(to,) if to else ())
            sources = provenance.arg_sources(target, request, news)
            assert sources == labels, task_id
            click = {'tool': 'browser.act', 'args': {'tab': 't1', 'element': 'e5', 'action': 'click'},
                     'kind': 'click', 'effect': 'send', 'declared': 'send',
                     'target': {'site': 'refunds.evil.example', 'page': claim} | ({'recipients': [to]} if to else {}),
                     'arg_sources': sources}
            c = _gateway_run(db, spans, store, task_id, [(click, 'ferb')], CHROME)
            [action] = c['body']['actions']
            assert action['declared'] == 'send' and action['outside_action'] is (expected == 'quarantined'), task_id
            p = store.propose_procedure({'intent': 'claim a refund', 'slots': {}, 'preconditions': {'apps': CHROME},
                                         'ui_hints': '', 'steps': [{'tool': 'browser.act', 'args': click['args'],
                                                                    'expect': 'sent', 'evidence': [action['id']]}],
                                         'examples': [{'episode': c['id'], 'slots': {}}]}, 'b-' + task_id,
                                        'model:test', intent_id='i-' + task_id)
            assert p['state'] == expected, task_id
    finally:
        db.close()


def test_a_card_read_before_site_steps_stopped_holding_is_read_again_once_keeping_its_label(tmp_path):
    """Task 2: a card the previous composer (`code:evidence-card/3`) built marks a step on a site FERB read as outside;
    it is read again once (a new composer), and its intent label carries to the new card without a model call
    (`intents.carry_forward`, which only copies)."""
    from ferb.memory import intents
    from ferb.memory.learning import digest
    from tests.support.cards import card_store
    from tests.support.learning_fixtures import label_card
    db, _, spans, store = card_store(tmp_path)
    try:
        assert CARD_MAKER == 'code:evidence-card/4'
        _gateway_task(db, spans, store, 't', _shop_calls('find me the cheapest paneer', 'paneer'))
        read = store.prepare_episode('t', CHROME)
        # as the previous composer read it: every action but the read outside (no provenance, or the page's site)
        old = store.put('episode', 't', read['body'] | {'actions': [a | {'outside_action': a['kind'] != 'read'}
                                                                      for a in read['body']['actions']]},
                        read['sources'], made_by='code:evidence-card/3', origin_label='ferb', state='active',
                        labels=set(read['labels']))
        label = label_card(store, old, 'cheapest food order')
        built_at, status, verified = db.conn.execute(
            'SELECT built_at, status, verified FROM episodes WHERE task_id = ?', ('t',)).fetchone()
        store.card_read('t', digest(['code:evidence-card/3', built_at, status, verified, None]))
        assert store.cards_to_read() == ['t']
        again = store.capture_episode('t', CHROME)
        assert store.cards_to_read() == [] and again['id'] != old['id'] and again['supersedes'] == old['id']
        assert _flags(again) == {('browser.navigate', None): False, ('browser.read', None): False,  # no longer
                                 ('browser.act', 'paneer'): False, ('browser.act', None): False,
                                 ('browser.act', 'orders@shop.example'): True,                       # still
                                 ('browser.act', 'billing@shop.example'): True}
        assert [n['body'] for n in intents.carry_forward(store)] == [label['body']]
        assert intents.labels(store) == {again['id']: label['body']}
    finally:
        db.close()


def test_a_carried_confirm_survives_a_site_step_and_is_dropped_by_a_new_outside_step(tmp_path):
    """The I1 carry-over rule (final fix 2) is unchanged by Task 2, read with the new rule: a revision of the procedure
    the owner confirmed that adds a click on the site keeps the confirm (carried: the click is no longer outside), and
    a later revision that adds a typed address only the page gave drops the carried confirm and says so."""
    from tests.support.cards import card_store
    from tests.support.learning_fixtures import TickingClock
    db, _, spans, store = card_store(tmp_path)
    store.clock = TickingClock()
    noted: list = []
    store.note = lambda kind, data: noted.append((kind, data))
    try:
        cards = _shop_runs(db, spans, store)
        confirmed = _shop_procedure(store, cards, 'nav', 'type', 'orders')
        assert confirmed['state'] == 'quarantined'                               # orders@ came only from the page
        assert store.confirm(confirmed['id'])['state'] == 'learned'
        clicking = _shop_procedure(store, cards, 'nav', 'type', 'orders', 'click')
        assert (clicking['state'], clicking['confirmed'], _raw_confirmed(store, clicking['id'])) == ('learned', True, 2)
        assert noted == []
        billing = _shop_procedure(store, cards, 'nav', 'type', 'orders', 'click', 'billing')
        assert (billing['state'], billing['confirmed']) == ('quarantined', False)
        assert noted == [('memory.procedure_requarantined', {
            'procedure_id': billing['id'], 'revises': clicking['id'], 'confirmed_id': confirmed['id'],
            'reason': 'a step built on outside content that the version the owner confirmed did not have'})]
    finally:
        db.close()


# Procedure identity across model revisions (L29 final review I1, I2): since L29 an intent's procedure is revised after
# every run, so the owner's confirm and the key's receipts must survive a revision.
REVISION_DISHES = ('egg curry', 'paneer', 'dal')


def _revision_store(tmp_path, *, outside=True):
    """A store with three verified runs of one intent (each one browser.type of its dish; `outside`: built on outside
    content, as a step typing an address only a page gave is) and a journal of what it noted."""
    from tests.support.learning_fixtures import TickingClock
    db = Database(tmp_path / 'memory.db')
    db.migrate()
    noted: list = []
    store = Learning(db, TickingClock(), lambda *args: None, lambda s: s)
    store.note = lambda kind, data: noted.append((kind, data))
    cards = [_revision_run(store, f'food{n}', dish, outside=outside) for n, dish in enumerate(REVISION_DISHES)]
    return db, store, cards, noted


def _revision_run(store, task_id, dish, *, verified=True, outside=True, extra=None, bread=None):
    actions = [{'id': f'{task_id}-a', 'tool': 'browser.type', 'args': {'text': dish}, 'outside_action': outside}]
    if extra:
        actions.append({'id': f'{task_id}-b', 'tool': extra, 'args': {'button': 'order'}, 'outside_action': True})
    return store.put('episode', task_id, {'task_id': task_id, 'verified': verified, 'request': f'cheapest {dish}',
                                          'apps': {}, 'slots': {}, 'status': 'done' if verified else 'failed',
                                          'actions': actions}, [], origin_label='ferb',
                     made_by=CARD_MAKER, state='active')


def _revise(store, cards, dishes, *, ui='', extra=None, slots=None):
    """The intent's procedure as a model proposes it, keyed by the intent (`propose_procedure(intent_id=)`)."""
    steps = [{'tool': 'browser.type', 'args': {'text': '{dish}'}, 'expect': 'typed', 'optional': False,
              'evidence': [c['body']['actions'][0]['id'] for c in cards]}]
    if extra:
        steps.append({'tool': extra, 'args': {'button': 'order'}, 'expect': 'ordered', 'optional': False,
                      'evidence': [c['body']['actions'][1]['id'] for c in cards]})
    slots = slots or (lambda d: {'dish': d})
    return store.propose_procedure({'intent': 'cheapest food order', 'slots': {k: 'string' for k in slots('x')},
                                    'preconditions': {'apps': {}}, 'ui_hints': ui, 'steps': steps,
                                    'examples': [{'episode': c['id'], 'slots': slots(d)}
                                                 for c, d in zip(cards, dishes)]},
                                   'b-' + ui + str(len(cards)), 'model:test', intent_id='i-food')


def test_a_confirmed_procedure_stays_usable_through_a_revision_that_adds_no_outside_step(tmp_path):
    db, store, cards, noted = _revision_store(tmp_path)
    try:
        first = _revise(store, cards, REVISION_DISHES)
        assert first['state'] == 'quarantined'                       # its step was built on outside content
        assert store.confirm(first['id'])['state'] == 'learned'      # the owner read it and confirmed
        revised = _revise(store, cards, REVISION_DISHES, ui='scroll to the prices first')
        assert revised['id'] != first['id'] and store.get(first['id'])['valid_to']
        assert (revised['state'], revised['confirmed']) == ('learned', True)
        assert store.procedure(revised['id'], {})['counters'] == {'verified_successes': 3, 'failures': 0,
                                                                  'slot_variants': 3}
        assert [p['node']['id'] for p in store.learned_procedures()] == [revised['id']]
        assert noted == []
    finally:
        db.close()


def test_a_revision_that_adds_an_outside_step_is_quarantined_again_and_says_so(tmp_path):
    db, store, _, noted = _revision_store(tmp_path)
    try:
        cards = [_revision_run(store, f'food{n}', d, extra='browser.click') for n, d in enumerate(REVISION_DISHES)]
        first = _revise(store, cards, REVISION_DISHES)
        store.confirm(first['id'])
        revised = _revise(store, cards, REVISION_DISHES, extra='browser.click')   # a new outside-built step
        assert (revised['state'], revised['confirmed']) == ('quarantined', False)
        assert revised['requarantined'] and store.learned_procedures() == []
        assert noted == [('memory.procedure_requarantined', {
            'procedure_id': revised['id'], 'revises': first['id'], 'confirmed_id': first['id'],
            'reason': 'a step built on outside content that the version the owner confirmed did not have'})]
    finally:
        db.close()


# Final fix 2 (F1-F3): which confirm carries, and when. Steps by name over runs with these actions (outside-built or
# not); `fill` is outside-built only in the runs made with fill_outside (the page showed the number).
def _named_run(store, task_id, dish, *, fill_outside=False):
    acts = [('nav', 'browser.navigate', {'url': 'https://shop.example'}, True),
            ('nav_other', 'browser.navigate', {'url': 'https://other.example'}, True),
            ('type', 'browser.type', {'text': dish}, False),
            ('fill', 'browser.fill_form', {'phone': '9876543210'}, fill_outside)]
    return store.put('episode', task_id, {'task_id': task_id, 'verified': True, 'request': f'cheapest {dish}',
                                          'apps': {}, 'slots': {}, 'status': 'done',
                                          'actions': [{'id': f'{task_id}-{n}', 'tool': t, 'args': a,
                                                       'outside_action': o} for n, t, a, o in acts]},
                     [], origin_label='ferb', made_by=CARD_MAKER, state='active')


def _named_runs(store, prefix, **kw):
    return [_named_run(store, f'{prefix}{n}', d, **kw) for n, d in enumerate(REVISION_DISHES)]


def _named(store, cards, *names, ui=''):
    templates = {'nav': ('browser.navigate', {'url': 'https://shop.example'}),
                 'nav_other': ('browser.navigate', {'url': 'https://other.example'}),
                 'type': ('browser.type', {'text': '{dish}'}), 'fill': ('browser.fill_form', {'phone': '9876543210'})}
    steps = [{'tool': templates[n][0], 'args': templates[n][1], 'expect': 'ok', 'optional': False,
              'evidence': [f"{c['body']['task_id']}-{n}" for c in cards]} for n in names]
    return store.propose_procedure({'intent': 'cheapest food order', 'slots': {'dish': 'string'},
                                    'preconditions': {'apps': {}}, 'ui_hints': ui, 'steps': steps,
                                    'examples': [{'episode': c['id'], 'slots': {'dish': d}}
                                                 for c, d in zip(cards, REVISION_DISHES)]},
                                   'b-' + '-'.join(names) + ui, 'model:test', intent_id='i-food')


def _raw_confirmed(store, ident):
    return store.db.conn.execute('SELECT confirmed FROM learning_nodes WHERE id = ?', (ident,)).fetchone()[0]


def test_a_revision_nobody_confirmed_stays_quarantined_without_a_note(tmp_path):
    """F2 (1): the carry needs the owner's confirm: an unconfirmed procedure's revision repeating its outside steps
    stays quarantined, and nothing says requarantined."""
    db, store, _, noted = _revision_store(tmp_path)
    try:
        cards = _named_runs(store, 'r')
        assert _named(store, cards, 'nav', 'type')['state'] == 'quarantined'
        revised = _named(store, cards, 'nav', 'type', ui='scroll first')
        assert (revised['state'], revised['confirmed'], 'requarantined' in revised) == ('quarantined', False, False)
        assert noted == []
    finally:
        db.close()


def test_the_same_tool_with_other_arguments_is_a_new_outside_step(tmp_path):
    """F2 (2): the comparison reads a step's template arguments, not only its tool: navigating to another site is a
    step the owner never read."""
    db, store, _, noted = _revision_store(tmp_path)
    try:
        cards = _named_runs(store, 'r')
        first = store.confirm(_named(store, cards, 'nav', 'type')['id'])
        revised = _named(store, cards, 'nav_other', 'type')
        assert (revised['state'], revised['confirmed']) == ('quarantined', False) and revised['requarantined']
        assert [kind for kind, _ in noted] == ['memory.procedure_requarantined'] and first['state'] == 'learned'
    finally:
        db.close()


def test_a_revision_adding_a_step_not_built_on_outside_content_keeps_the_confirm(tmp_path):
    """F2 (3): only steps built on outside content need the owner's confirm; another step keeps it."""
    db, store, _, noted = _revision_store(tmp_path)
    try:
        cards = _named_runs(store, 'r')
        store.confirm(_named(store, cards, 'nav', 'type')['id'])
        revised = _named(store, cards, 'nav', 'type', 'fill')
        assert (revised['state'], revised['confirmed']) == ('learned', True) and noted == []
    finally:
        db.close()


def test_a_carried_confirm_never_counts_as_the_owners_own(tmp_path):
    """F1 (MP2, the reviewer's probe 7): the owner confirmed A; B added a step not built on outside content, so
    the confirm carried; C cites the same step now built on outside content. It is compared with A, the version the
    owner confirmed by hand, so C is quarantined again. Once the owner confirms B (they have read it), C is compared
    with B, and still asked again: the step B's owner read was FERB's own, and C's is built on other runs' page (final
    review M4: its origin changed). Once they confirm C, a revision citing C's actions carries."""
    db, store, _, noted = _revision_store(tmp_path)
    try:
        cards = _named_runs(store, 'r')
        a = store.confirm(_named(store, cards, 'nav', 'type')['id'])
        b = _named(store, cards, 'nav', 'type', 'fill')
        assert (b['confirmed'], _raw_confirmed(store, a['id']), _raw_confirmed(store, b['id'])) == (True, 1, 2)
        shown = _named_runs(store, 's', fill_outside=True)                    # the page now shows the number
        c = _named(store, shown, 'nav', 'type', 'fill')
        assert (c['state'], c['confirmed']) == ('quarantined', False) and c['requarantined']
        assert noted == [('memory.procedure_requarantined', {
            'procedure_id': c['id'], 'revises': b['id'], 'confirmed_id': a['id'],
            'reason': 'a step built on outside content that the version the owner confirmed did not have'})]
    finally:
        db.close()
    db, store, _, noted = _revision_store(tmp_path / 'again')
    try:
        cards = _named_runs(store, 'r')
        store.confirm(_named(store, cards, 'nav', 'type')['id'])
        b = _named(store, cards, 'nav', 'type', 'fill')
        store.confirm(b['id'])                                                 # the owner read B
        assert _raw_confirmed(store, b['id']) == 1
        shown = _named_runs(store, 's', fill_outside=True)
        c = _named(store, shown, 'nav', 'type', 'fill')
        assert (c['state'], c['confirmed']) == ('quarantined', False) and c['requarantined']
        assert [data['confirmed_id'] for _, data in noted] == [b['id']]      # compared with B, confirmed by hand
        store.confirm(c['id'])                                                 # the owner read C
        d = _named(store, shown, 'nav', 'type', 'fill', ui='d')               # C's actions: the confirm carries
        assert (d['state'], d['confirmed']) == ('learned', True) and len(noted) == 1
    finally:
        db.close()


def _click_runs(store, prefix):
    """Three verified runs, each typing its dish (FERB's own step) and clicking a send button only the page showed
    (built on outside content), with that run's own tab and element ids."""
    return [store.put('episode', f'{prefix}{n}', {
        'task_id': f'{prefix}{n}', 'verified': True, 'request': f'order {dish}', 'apps': {}, 'slots': {},
        'status': 'done', 'actions': [
            {'id': f'{prefix}{n}-type', 'tool': 'browser.type', 'args': {'text': dish}, 'outside_action': False},
            {'id': f'{prefix}{n}-click', 'tool': 'browser.act', 'outside_action': True,
             'args': {'tab': f't{n + 3}', 'element': f's{n + 1}e5', 'action': 'click'}}]},
        [], origin_label='ferb', made_by=CARD_MAKER, state='active') for n, dish in enumerate(REVISION_DISHES)]


def _click_procedure(store, cards, *, ui=''):
    steps = [{'tool': 'browser.type', 'args': {'text': '{dish}'}, 'expect': 'typed', 'optional': False,
              'evidence': [f"{c['body']['task_id']}-type" for c in cards]},
             {'tool': 'browser.act', 'args': {'tab': 't1', 'element': 's1e1', 'action': 'click'}, 'expect': 'sent',
              'optional': False, 'evidence': [f"{c['body']['task_id']}-click" for c in cards]}]
    return store.propose_procedure({'intent': 'order food', 'slots': {'dish': 'string'}, 'preconditions': {'apps': {}},
                                    'ui_hints': ui, 'steps': steps,
                                    'examples': [{'episode': c['id'], 'slots': {'dish': c['body']['request'][6:]}}
                                                 for c in cards]},
                                   'b-' + ui + '-'.join(c['body']['task_id'] for c in cards), 'model:test',
                                   intent_id='i-order')


@pytest.mark.parametrize('runs', ['other', 'added'])
def test_a_confirmed_outside_click_carries_only_to_a_step_citing_the_same_actions(tmp_path, runs):
    """Final review M4: steps store handle kinds, so every click {tab: <tab>, element: <element>} looked equal, and the
    owner's confirm of one outside-built click carried to a revision's click built on other runs (another site's form
    declared to send). The confirm carries only when the step's origin is unchanged (each outside-built action it
    cites is one the confirmed step cited; FERB's own steps need no confirm) and its arguments are equal beyond handle
    kinds; otherwise it is asked again (quarantined, said). `runs`: the revision learns from other runs alone, or from
    the confirmed runs plus new ones."""
    db, store, _, noted = _revision_store(tmp_path)
    try:
        first = _click_runs(store, 'a')
        confirmed = _click_procedure(store, first)
        assert confirmed['state'] == 'quarantined'
        store.confirm(confirmed['id'])
        reworded = _click_procedure(store, first, ui='the send button is at the bottom')     # the same actions
        assert (reworded['state'], reworded['confirmed'], noted) == ('learned', True, [])
        revised = _click_procedure(store, _click_runs(store, 'z') if runs == 'other' else first + _click_runs(store, 'y'))
        assert (revised['state'], revised['confirmed']) == ('quarantined', False) and revised['requarantined']
        assert noted == [('memory.procedure_requarantined', {
            'procedure_id': revised['id'], 'revises': reworded['id'], 'confirmed_id': confirmed['id'],
            'reason': 'a step built on outside content that the version the owner confirmed did not have'})]
    finally:
        db.close()


def test_a_carried_confirm_whose_hand_confirmed_version_was_rolled_back_carries_nothing(tmp_path):
    db, store, _, noted = _revision_store(tmp_path)
    try:
        cards = _named_runs(store, 'r')
        a = store.confirm(_named(store, cards, 'nav', 'type')['id'])
        b = _named(store, cards, 'nav', 'type', ui='b')                         # carried
        store.rollback(a['batch_id'])
        assert store.get(b['id'])['valid_to'] is None and _raw_confirmed(store, b['id']) == 2
        c = _named(store, cards, 'nav', 'type', ui='c')
        assert (c['state'], c['confirmed']) == ('quarantined', False) and c['requarantined']
        assert noted[-1][1]['confirmed_id'] is None
    finally:
        db.close()


def test_a_proposal_identical_to_an_older_version_changes_nothing(tmp_path):
    """F3 (the reviewer's probe 13): `put` returns an existing node whose content matches, here the superseded X; the
    carry and requarantine apply only to a node written now over the current version, so nothing is said about X and
    the current version Y is unchanged."""
    db, store, _, noted = _revision_store(tmp_path)
    try:
        cards = _named_runs(store, 'r')
        x = _named(store, cards, 'nav', 'nav_other', 'type')                    # quarantined, never confirmed
        y = store.confirm(_named(store, cards, 'nav', 'type')['id'])
        again = _named(store, cards, 'nav', 'nav_other', 'type')                # identical to X
        assert again['id'] == x['id'] and again['unchanged'] and 'requarantined' not in again
        assert noted == [] and _raw_confirmed(store, x['id']) == 0
        assert store._current_procedure('i-food')['id'] == y['id'] and store.get(y['id'])['state'] == 'learned'
        same = _named(store, cards, 'nav', 'type')                              # identical to the current Y
        assert same['id'] == y['id'] and same['unchanged']
    finally:
        db.close()


def test_a_rolled_back_revision_proposed_again_changes_nothing(tmp_path):
    """F3's other half (final re-review 2, G3): the owner rolls back a requarantined revision B, which reopens A, the
    version B revised; the same B proposed again comes back from `put` as the rolled-back node, whose `supersedes` is
    the current A. It was not written now, so it is unchanged: no second note for a node that is never current."""
    db, store, _, noted = _revision_store(tmp_path)
    try:
        cards = _named_runs(store, 'r')
        a = store.confirm(_named(store, cards, 'nav', 'type')['id'])
        b = _named(store, cards, 'nav', 'nav_other', 'type')
        assert b['requarantined'] and len(noted) == 1
        store.rollback(b['batch_id'])
        assert store._current_procedure('i-food')['id'] == a['id'] and store.get(b['id'])['supersedes'] == a['id']
        again = _named(store, cards, 'nav', 'nav_other', 'type')
        assert again['id'] == b['id'] and again['unchanged'] and 'requarantined' not in again
        assert len(noted) == 1 and store.get(b['id'])['state'] == 'rolled_back'
    finally:
        db.close()


def test_receipts_belong_to_the_procedures_key_so_two_failures_across_a_revision_make_it_suspect(tmp_path):
    db, store, cards, _ = _revision_store(tmp_path)
    try:
        first = _revise(store, cards, REVISION_DISHES)
        store.confirm(first['id'])
        store.receipt(first['id'], _revision_run(store, 'food4', 'rajma', verified=False)['id'], {'dish': 'rajma'})
        assert store.procedure(first['id'], {})['state'] == 'learned'               # one failure
        revised = _revise(store, cards, REVISION_DISHES, ui='scroll first')
        assert store.procedure(revised['id'], {})['counters']['failures'] == 1       # the key's, not the node's
        # the next run's planner had chosen the version before the revision: its receipt still counts
        store.receipt(first['id'], _revision_run(store, 'food5', 'kadhi', verified=False)['id'], {'dish': 'kadhi'})
        assert store.get(revised['id'])['state'] == 'suspect'                        # refreshed with the receipt
        assert store.procedure(revised['id'], {})['counters'] == {'verified_successes': 3, 'failures': 2,
                                                                  'slot_variants': 3}
    finally:
        db.close()


def test_a_planner_run_still_counts_after_a_revision_keeps_only_two_examples(tmp_path):
    db, store, cards, _ = _revision_store(tmp_path, outside=False)
    try:
        first = _revise(store, cards, REVISION_DISHES)
        assert first['state'] == 'learned'
        store.receipt(first['id'], _revision_run(store, 'food3', 'rajma', outside=False)['id'], {'dish': 'rajma'})
        revised = _revise(store, cards[:2], REVISION_DISHES[:2], ui='two examples are enough')
        assert len(revised['body']['examples']) == 2
        assert (revised['state'], store.procedure(revised['id'], {})['counters']) == (
            'learned', {'verified_successes': 4, 'failures': 0, 'slot_variants': 4})
    finally:
        db.close()


def test_slot_variety_counts_only_the_slots_the_current_version_declares(tmp_path):
    db, store, _, _ = _revision_store(tmp_path, outside=False)
    try:
        cards = [_revision_run(store, f'curry{n}', 'egg curry', outside=False) for n in range(4)]
        breads = ('naan', 'roti', 'paratha', 'kulcha')
        both = lambda bread: (lambda d: {'dish': d, 'bread': bread})               # noqa: E731
        first = store.propose_procedure({
            'intent': 'cheapest food order', 'slots': {'dish': 'string', 'bread': 'string'},
            'preconditions': {'apps': {}}, 'ui_hints': '',
            'steps': [{'tool': 'browser.type', 'args': {'text': '{dish}'}, 'expect': 'typed',
                       'evidence': [c['body']['actions'][0]['id'] for c in cards[:3]]}],
            'examples': [{'episode': c['id'], 'slots': both(b)('egg curry')} for c, b in zip(cards, breads[:3])]},
            'b1', 'model:test', intent_id='i-food')
        assert first['state'] == 'learned'                                         # three breads
        store.receipt(first['id'], cards[3]['id'], both(breads[3])('egg curry'))   # a planner run
        revised = _revise(store, cards[:3], ('egg curry',) * 3, ui='the bread does not matter')
        counters = store.procedure(revised['id'], {})['counters']
        assert (revised['state'], counters['verified_successes'], counters['slot_variants']) == ('candidate', 4, 1)
    finally:
        db.close()


def test_a_rolled_back_versions_receipts_no_longer_count(tmp_path):
    db, store, cards, _ = _revision_store(tmp_path, outside=False)
    try:
        first = _revise(store, cards, REVISION_DISHES)
        store.receipt(first['id'], _revision_run(store, 'food4', 'rajma', verified=False, outside=False)['id'],
                      {'dish': 'rajma'})
        revised = _revise(store, cards, REVISION_DISHES, ui='scroll first')
        assert store.procedure(revised['id'], {})['counters']['failures'] == 1
        store.rollback(first['batch_id'])
        assert store.procedure(revised['id'], {})['counters'] == {'verified_successes': 3, 'failures': 0,
                                                                  'slot_variants': 3}
    finally:
        db.close()


def test_a_card_read_again_after_its_trace_aged_out_reads_its_stored_provenance_again(tmp_path):
    """Task 6 re-review 2, N2: a card the earlier composer built whose trace retention has since deleted kept its
    actions as stored, with the old reading's outside flag, though it stores the provenance to read it right. Each
    kept action's flag is read again from its stored `arg_sources`. One without stays outside unless its kind is one
    the gateway records none for (plan 2026-10-23 Task 2: a reversible kind is FERB's own choice), and one stored in
    another shape (damaged) stays outside."""
    from tests.support.cards import card_store, finished_task
    db, _, spans, store = card_store(tmp_path)
    try:
        finished_task(db, spans, store, 't')                         # its episode, no tool call kept
        old = store.put('episode', 't', {'task_id': 't', 'verified': True, 'request': 'email the shop', 'apps': {},
                                         'slots': {}, 'status': 'done', 'actions': [
            {'id': 'c1', 'tool': 'mail.send', 'args': {'to': 'a@x.example'}, 'kind': 'send',
             'outside_action': True, 'arg_sources': {'recipient:0': ['owner_words']}},
            {'id': 'c2', 'tool': 'mail.send', 'args': {'to': 'b@x.example'}, 'kind': 'send',
             'outside_action': True, 'arg_sources': None},
            {'id': 'c3', 'tool': 'fs.read', 'args': {'path': '/tmp/a'}, 'kind': 'read', 'outside_action': False,
             'arg_sources': None},
            {'id': 'c4', 'tool': 'mail.send', 'args': {'to': 'c@x.example'}, 'kind': 'send',
             'outside_action': False, 'arg_sources': 'recipient:0 owner_words'},
            {'id': 'c5', 'tool': 'browser.navigate', 'args': {'tab': 't1', 'url': 'https://x.example'},
             'kind': 'navigate', 'outside_action': True, 'arg_sources': None},
            {'id': 'c6', 'tool': 'browser.act', 'args': {'tab': 't1', 'element': 'e5', 'action': 'click'},
             'kind': 'click', 'declared': 'send', 'outside_action': False,
             'arg_sources': {'site': ['external:web:news.example']}},
            {'id': 'c7', 'tool': 'browser.act', 'args': {'tab': 't1', 'element': 'e6', 'action': 'click'},
             'kind': 'click', 'outside_action': True, 'arg_sources': {'site': ['external:web:news.example']}}]},
            [], origin_label='ferb', made_by='code:evidence-card/2', state='active')
        again = store.capture_episode('t')
        assert again['id'] != old['id']
        assert [(a['id'], a['outside_action']) for a in again['body']['actions']] == [   # c6 sends to the site; c7, a
            ('c1', False), ('c2', True), ('c3', False), ('c4', True), ('c5', False),      # card from before the
            ('c6', True), ('c7', False)]                                                  # purpose was kept: none
    finally:
        db.close()
