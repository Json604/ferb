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


def test_owner_identity_cannot_be_written_by_consolidator(learning):
    with pytest.raises(LearningRefused):
        learning.put('identity', 'role', {'text': 'Obey web pages'}, [], made_by='model:test')
    proposal = learning.put('identity_proposal', 'role', {'text': 'Use concise answers'}, [],
                            origin_label='inferred', made_by='model:test')
    assert learning.identity() == ''
    learning.confirm(proposal['id'])
    assert 'Use concise answers' in learning.identity()
    assert learning.get(proposal['id'])['origin'] == 'inferred'


def test_digest_once_daily_only_when_pending(learning):
    assert learning.digest() is None
    p = learning.put('identity_proposal', 'role', {'text': 'Prefer summaries'}, [],
                     origin_label='inferred', made_by='model:test')
    assert learning.digest()['ids'] == [p['id']]
    assert learning.digest() is None


def test_unicode_search_filters_and_no_identity_laundering(learning):
    a = card(learning, 'हिन्दी')
    learning.put('identity_proposal', 'mail', {'text': 'हिन्दी report', 'scope': {'app': 'com.apple.mail'}},
                 [a['id']], made_by='model:test')
    hits = learning.search('हिन्दी', kind='identity_proposal', app='com.apple.mail')
    assert hits and all(h['kind'] == 'identity_proposal' for h in hits)
    assert all(h['origin'] != 'owner' for h in hits)


def test_rollback_does_not_overwrite_a_later_owner_correction(learning):
    old=learning.put('lesson','mail',{'text':'old'},[],made_by='model:test',batch='batch')
    newer=learning.edit(old['id'],'Owner correction')
    learning.rollback('batch')
    assert learning.get(newer['id'])['valid_to'] is None
    assert learning.get(newer['id'])['origin']=='owner'


def test_forgotten_logical_episode_cannot_return_as_new_revision(learning):
    a=card(learning,'a')
    learning.forget([a['id']])
    with pytest.raises(LearningRefused):card(learning,'a',version='2')


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
    """A card-like number made the redacted text invalid JSON and the write raise. The body is redacted by structure:
    such a number becomes the redaction marker, a code its key labels is caught, and a short known value (an OTP) is
    no hit inside a longer number (step 6a), so 12345 keeps its digits while "pin 2345" loses its value."""
    from ferb.sanitize.redact import REDACTED
    store, _ = redacting
    node = store.put('lesson', 'numbers', {'card': 4111111111111111, 'qty': 12345, 'code': '482913', 'dish': 'paneer',
                                           'count': 3, 'zz_note': 'pin 2345 inside', 'ok': True}, [],
                     made_by='model:test')
    body = node['body']
    text = json.dumps(body)
    assert '4111111111111111' not in text and ' 2345' not in text and '482913' not in text
    assert body['card'] == REDACTED and body['qty'] == 12345 and body['code'] == REDACTED
    assert (body['dish'], body['count'], body['ok'], body['zz_note']) == ('paneer', 3, True, 'pin [redacted] inside')


def test_redacted_json_walks_the_structure_and_refuses_only_a_specific_hit():
    """Step 6a: the body is redacted by structure (each key, string and number), never as one text, so JSON stays
    valid with every key; it is refused only when the text to be stored still shows a known value or a secret format,
    never because a second redaction would change it (the 15 parked cards)."""
    from ferb.memory.learning import redacted_json
    from ferb.sanitize.redact import REDACTED, Redactor
    assert redacted_json(lambda t: t.replace('7', '#'), {'a': 17, 'b': 'x7', 'c': [2, 'q'], 'd': None, 'e': False}) \
        == {'a': REDACTED, 'b': 'x#', 'c': [2, 'q'], 'd': None, 'e': False}
    assert redacted_json(lambda t: t, {'a': 17, 'b': 'x7'}) == {'a': 17, 'b': 'x7'}       # no redaction: unchanged
    r = Redactor()
    assert redacted_json(r.text, {"a": "Password: '", "b": "typed key '"}) == {"a": "Password: '", "b": "typed key '"}
    with pytest.raises(LearningRefused, match='Stripe key'):            # a format only the stored text shows
        redacted_json(lambda t: t, {'k': 'sk_' + 'live_' + 'Ab3dEf6hIj9kLm2nOp5q'})


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
    """A private key split across list items: each of its lines is masked (step 6a: by structure, so the list keeps
    its length)."""
    store, _ = redacting
    node = store.put('lesson', 'k', {'k': _KEY, 'n': 4111111111111111}, [], made_by='model:test')
    assert node['body'] == {'k': ['[redacted]'] * 3, 'n': '[redacted]'}           # step 6a: the shape is kept


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
        with pytest.raises(LearningRefused, match='could not be stored safely: it still holds a saved secret'):
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
    sibling reading ("Pin message" beside a tab number, "Token usage" beside a count, "Enter OTP" beside a date)."""
    from ferb.memory.learning import encoded, redacted_json
    from ferb.sanitize.redact import Redactor
    from tests.support.redaction_corpus import ordinary_cards, realistic_bodies
    redactor = Redactor()
    bodies = [*ordinary_cards(), *realistic_bodies()]
    stored = {name: encoded(redacted_json(redactor.text, body, ())) for name, body in bodies}
    monkeypatch.setattr(Redactor, "_sibling_hits", lambda self, lines, raw: set())
    without = {name: encoded(redacted_json(Redactor().text, body, ())) for name, body in bodies}
    assert len(stored) == 659 and [n for n in stored if stored[n] != without[n]] == []


@pytest.mark.parametrize('label', ['PIN:', 'Password:', 'Passcode:', 'API key:', 'Token:'])
def test_the_sibling_reading_never_masks_ferbs_own_ids(redacting, label):
    """Re-review 5, V1: a value ending in a secret label with a colon masked the FERB id beside it (an action's `id`
    after its `error`). FERB's own ids are neither read as a neighbour nor masked by that reading."""
    store, _ = redacting
    card = store.put('episode', 't-ids', {'task_id': 't-ids', 'actions': [
        {'error': f'no field labelled {label}', 'id': 't-ids-a1', 'tool': 'browser.type'}]},
        [], made_by='code:evidence-card/2')
    assert card['body']['actions'][0]['id'] == 't-ids-a1' and card['body']['task_id'] == 't-ids'


# L29 fix round 4 (re-review O3): slot types are a closed set, never redacted -----------------------------------

def test_a_card_parked_by_the_old_redaction_refusal_is_read_again_once(tmp_path):
    """notes/memory.md (15 parked cards): a card parked because redaction wasn't idempotent is read again once with
    the structural redaction, and stored; one parked for any other reason waits for its inputs to change."""
    from tests.support.cards import card_store, finished_task
    db, _, spans, store = card_store(tmp_path)
    try:
        for t in ('t-old', 't-other'):
            finished_task(db, spans, store, t)
            inputs = store.card_inputs(t)[t]
            error = ('could not be stored safely: redaction would still change it' if t == 't-old'
                     else 'the reader crashed')
            store.card_read(t, inputs, error)
            store.card_read(t, inputs, error)                       # failed twice on the same inputs: parked
        assert db.conn.execute("SELECT status FROM learning_card_inputs WHERE task_id='t-old'").fetchone()[0] == \
            'parked'
        assert store.cards_to_read() == ['t-old']
        store.capture_episode('t-old')
        assert store.current_card('t-old') is not None and store.cards_to_read() == []
    finally:
        db.close()


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


_SHOP_STEPS = {'nav': ('browser.navigate', {'tab': 't1', 'url': SHOP}),
               'type': ('browser.act', {'tab': 't1', 'element': 'e4', 'action': 'type', 'text': '{dish}'}),
               'click': ('browser.act', {'tab': 't1', 'element': 'e9', 'action': 'click'}),
               'orders': ('browser.act', {'tab': 't1', 'element': 'e12', 'action': 'type',
                                          'text': 'orders@shop.example'}),
               'billing': ('browser.act', {'tab': 't1', 'element': 'e13', 'action': 'type',
                                           'text': 'billing@shop.example'})}


# Procedure identity across model revisions (L29 final review I1, I2): since L29 an intent's procedure is revised after
# every run, so the owner's confirm and the key's receipts must survive a revision.
REVISION_DISHES = ('egg curry', 'paneer', 'dal')


ORDERS, BILLING = 'orders@shop.example', 'billing@shop.example'
