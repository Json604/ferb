"""L16: learning works again (F8, D9). Internal ids survive redaction, the consolidator's answer schema is
strict-compatible, the daily security digest reads every recorded `network` shape, and one failing hourly memory job
never stops the others."""
import asyncio
import hashlib
import json
import random
import re
from datetime import datetime, timezone
from typing import Any

import pytest

from ferb.core.ids import SequentialIds
from ferb.core.types import ModelLimits, RuntimeEvent
from ferb.memory.consolidate import PROPOSAL_SCHEMA, Consolidator
from ferb.memory.learning import Learning
from ferb.orchestrator import episodes
from ferb.policy import security_digest
from ferb.sanitize.redact import REDACTED, Redactor, payment_card
from ferb.store.db import Database
from ferb.store.spans import SpanStore
from ferb.trace import compose
from tests.support.learning_fixtures import label_card
from tests.support.strict_schema import strict_problems
from tests.support.trace_builder import TraceBuilder

DAY = '2026-10-11'


class Clock:
    def now(self) -> datetime:
        return datetime(2026, 10, 11, 9, tzinfo=timezone.utc)


@pytest.fixture
def stores(tmp_path):
    db = Database(tmp_path / 'memory.db')
    db.migrate()
    redactor = Redactor()
    spans = SpanStore(db, Clock(), SequentialIds('span'), redactor)
    yield db, redactor, spans, Learning(db, Clock(), lambda *args: None, redactor.text)
    db.close()


def card_shaped(node_id: str) -> bool:
    """The live failure shape: a Luhn-valid 13-19 digit run inside an id's hex (m-8e953d2b8ca8f…)."""
    return any(13 <= len(run) <= 19 and payment_card(run) for run in re.findall(r'\d+', node_id))


# (a) ---------------------------------------------------------------------------------------------------------------

def test_internal_ids_pass_redaction_unchanged_while_cards_in_prose_stay_redacted() -> None:
    r = Redactor()
    rng = random.Random(16)
    ids = ['m-' + hashlib.sha256(rng.randbytes(32)).hexdigest() for _ in range(20000)]
    assert sum(card_shaped(i) for i in ids) > 0                 # the sample holds the shape that broke learning
    assert [i for i in ids if r.text(i) != i] == []
    assert r.text(json.dumps(ids)) == json.dumps(ids)           # put() redacts a body's JSON text
    assert r({'source_refs': ids[:50], 'note': 'cites ' + ', '.join(ids[:50])}) == \
        {'source_refs': ids[:50], 'note': 'cites ' + ', '.join(ids[:50])}
    steps = [rng.randbytes(16).hex() + '.1' for _ in range(2000)]   # uuid-hex task ids and their steps
    assert [i for i in steps if r.text('trail:' + i) != 'trail:' + i] == []

    assert r.text('Paid with card 4111 1111 1111 1111 yesterday.') == 'Paid with card [redacted] yesterday.'
    assert r.text('card:4111111111111111') == 'card:[redacted]'
    assert r.text('4111-1111-1111-1111') == REDACTED
    assert r({'note': 'my card is 4111111111111111.'})['note'] == 'my card is [redacted].'
    assert '4111111111111111' not in r.text(json.dumps({'narrative': 'Paid with\n4111111111111111'}))
    assert r.text('Card 4111 1111 1111 1112') == 'Card 4111 1111 1111 1112'     # no checksum, no card


def test_span_inputs_keep_structured_ids_and_still_redact_everything_else(stores) -> None:
    db, r, spans, _ = stores
    ref = 'journal:' + 'ab12cd34' * 4
    r.add_secret_value('ab12cd34')                 # a sealed value that happens to occur inside an id
    span = spans.start('context.exposure', input={
        'source_refs': [ref, 'https://x.test/a?token=abc123def456'],     # not an id: still redacted
        'reference': 'typed ab12cd34 and card 4111 1111 1111 1111'})
    assert span.input['source_refs'][0] == ref
    assert 'abc123def456' not in json.dumps(span.input)
    assert span.input['reference'] == 'typed [redacted] and card [redacted]'
    stored = db.conn.execute('SELECT input_json FROM spans WHERE id=?', (span.id,)).fetchone()[0]
    assert json.loads(stored) == span.input


# L16 hardening: id treatment never lets a secret through; cards stay redacted wherever they are a credential -------

JWT = 'eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U'


def luhn(prefix: str) -> str:
    """`prefix` plus the check digit that makes it pass the payment-card checksum."""
    return next(prefix + d for d in '0123456789' if payment_card(prefix + d))


@pytest.mark.parametrize('secret', ['sk-ant-api03-a1B2c3D4e5F6g7H8i9J0', 'ghp_A1b2C3d4E5f6G7h8I9j0K1l2', 'AKIAABCDEFGHIJKLMNOP',
                                    JWT, '4111111111111111', 'Zq9-hunter-77', 'pa5'])
def test_a_secret_placed_in_source_refs_is_still_redacted(stores, secret) -> None:
    db, r, spans, _ = stores
    r.add_secret_value('Zq9-hunter-77')            # exact registered secret values (two sealed and short)
    r.remember_hidden_secret('pa5')
    r.remember_hidden_secret('a5a5')               # hex: it occurs inside a generated id body below
    rng = random.Random(12)
    ids = [i for i in ('m-' + hashlib.sha256(rng.randbytes(32)).hexdigest() for _ in range(20000)) if card_shaped(i)]
    real = [ids[0], 'trail:' + 'ab12cd34' * 4, 'episode:' + 'f00d' * 8 + '.1', 'journal:' + 'a5' * 16]
    for ref in (secret, 'trail:' + secret):
        span = spans.start('context.exposure', input={'source_refs': [*real, ref, 'journal:pa5pa5'],
                                                      'reference': 'Layer 4'})
        assert span.input['source_refs'][:4] == real            # FERB's own ids pass unchanged
        assert secret not in span.input['source_refs'][4]
        # L28: only a generated id body is spared; a registered value in any other ref text is redacted
        assert span.input['source_refs'][5] == 'journal:[redacted]'            # adjacent hits: one marker
        stored = db.conn.execute('SELECT input_json FROM spans WHERE id=?', (span.id,)).fetchone()[0]
        assert json.loads(stored) == span.input


@pytest.mark.parametrize('control', ['\x08', '\x0c', '\x0b', '\x1f'])
def test_a_json_escape_before_a_card_still_separates_it(control) -> None:
    text = json.dumps('Card:' + control + '4111111111111111')       # \b, \f and \uXXXX escapes in JSON text
    assert '4111111111111111' not in Redactor().text(text)


def test_cards_in_a_url_query_or_fragment_are_redacted_but_ids_in_its_path_are_not() -> None:
    r = Redactor()
    status = 'https://x.com/kartikey/status/' + luhn('187654321098765432')
    discord = 'https://discord.com/channels/{}/{}/{}'.format(luhn('1100223344556677'), luhn('12233445566778899'),
                                                            luhn('129384756473829101'))
    assert all(payment_card(part) for part in discord.split('/')[-3:]) and payment_card(status.rsplit('/', 1)[1])
    assert r.text(status) == status
    assert r.text('see ' + discord + ' and ' + status + '.') == 'see ' + discord + ' and ' + status + '.'
    assert r.text('https://4111111111111111.example/4111111111111111') == \
        'https://4111111111111111.example/4111111111111111'
    assert r.text('https://shop.example/pay?cc=4111111111111111') == 'https://shop.example/pay?cc=[redacted]'
    assert r.text('https://shop.example/pay#4111111111111111') == 'https://shop.example/pay#[redacted]'
    assert r.text('card 4111111111111111 ' + status) == 'card [redacted] ' + status
    assert r.text(status + ' card 4111 1111 1111 1111') == status + ' card [redacted]'


@pytest.mark.parametrize('text', ['order,https://shop.example/o/12,4111111111111111,12/27',
                                  'https://shop.example/checkout,4111 1111 1111 1111',
                                  'pay|https://shop.example/checkout|4111111111111111',
                                  '(https://shop.example/checkout)4111111111111111',
                                  'https://shop.example/pay/4111 1111 1111 1111 exp 12/27',
                                  json.dumps('Pay here: https://shop.example/checkout\n4111 1111 1111 1111')])
def test_a_card_after_a_url_is_redacted_even_where_the_url_pattern_runs_on(text) -> None:
    """The URL pattern runs on through , | ) and a JSON \\n; only a whole path segment or host label (right after
    / or .) is an id."""
    out = Redactor().text(text)
    assert '4111111111111111' not in out and '4111 1111 1111 1111' not in out and REDACTED in out


# (b) ---------------------------------------------------------------------------------------------------------------

def test_episode_capture_with_refs_recorded_through_a_redacted_span_succeeds(stores) -> None:
    db, _, spans, learning = stores
    entity = next(node for node in (learning.put('entity', f'owner-{i}', {'text': 'The owner'}, [],
                                                 made_by='code:entity-summary/1', state='active')
                                    for i in range(20000)) if card_shaped(node['id']))
    trace = TraceBuilder(task_id='t-recall', request='What do you know about me?', goal='answer from memory').finish()
    episodes.write(db, compose.compose(trace) | {'made_by': episodes.MADE_BY, 'built_at': learning.now()})
    read = spans.start('tool.call', task_id='t-recall', input={'tool': 'task.evidence', 'args': {}, 'kind': 'read'})
    spans.finish(read, output={'source_refs': [entity['id']], 'records': []}, origin='inferred')   # a direct source
    captured = learning.capture_episode('t-recall')               # was: "unknown source m-8e953d2b8ca8f[redacted]…"
    assert captured is not None and entity['id'] in captured['sources']


# (c) ---------------------------------------------------------------------------------------------------------------

def test_consolidator_schema_is_strict_compatible() -> None:
    assert strict_problems(PROPOSAL_SCHEMA) == []


class NoMemory:
    def items(self, **_: Any) -> list[Any]:
        return []

    def get(self, _ident: str) -> None:
        return None


class ProposalRuntime:
    """A model runtime that answers one consolidation with a fixed strict-shaped JSON answer."""
    name, model = 'fake', 'fake-model'
    limits = ModelLimits('fake-model', 1_000_000, 100_000, 'test double')

    def __init__(self, answer: dict[str, Any]) -> None:
        self.answer, self.schemas = answer, []

    def make(self, role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any]) -> 'ProposalRuntime':
        self.schemas.append(schema)
        return self

    async def start(self, task: Any, request: Any, run_token: str) -> str:
        return 'run'

    async def events(self, run: str):
        yield RuntimeEvent('usage', {'input_tokens': 10, 'output_tokens': 5})
        yield RuntimeEvent('final', {'text': json.dumps(self.answer)})

    async def cancel(self, run: str) -> None:
        return None


def test_consolidator_round_trip_returns_proposals_in_the_shapes_learning_checks(stores) -> None:
    _, _, _, learning = stores
    card = learning.put('episode', 't-1', {
        'task_id': 't-1', 'request': 'send the weekly report', 'verified': True, 'status': 'done',
        'apps': {'com.apple.mail': '16.0'}, 'slots': {},
        'actions': [{'id': 't-1-action', 'tool': 'mail.send', 'args': {'to': 'a@kartikey.xyz', 'cc': []},
                     'outside_action': False}]}, [], origin_label='ferb', made_by='code:evidence-card/1',
        state='active')
    label_card(learning, card, 'send the weekly report')        # labelled (L29): only the consolidation call
    slot = [{'name': 'recipient', 'value': '"a@kartikey.xyz"'}]
    rt = ProposalRuntime({
        'episodes': [
            {'episode': card['id'], 'intent_signature': 'send report', 'slots': slot,
             'reflection': 'Sent on the first try.', 'narrative': 'Sent the weekly report.'},
            {'episode': card['id'], 'intent_signature': 'send report', 'reflection': '', 'narrative': '',
             'slots': [{'name': 'recipient', 'value': '"a@kartikey.xyz"'},
                       {'name': 'recipient', 'value': '"b@kartikey.xyz"'}]}],         # a name given twice: refused
        'procedures': [{
            'intent': 'send the weekly report', 'slots': [{'name': 'recipient', 'value': 'string'}],
            'preconditions': {'apps': [{'name': 'com.apple.mail', 'value': '16.0'}]},
            'steps': [{'tool': 'mail.send', 'expect': 'the report is in Sent', 'evidence': ['t-1-action'],
                       'optional': False,
                       'args': [{'name': 'to', 'value': '"{recipient}"'}, {'name': 'cc', 'value': '[]'}]}],
            'ui_hints': '', 'examples': [{'episode': card['id'], 'slots': slot}]}],
        'lessons': [],
        'identity': [{'key': 'reports', 'text': 'Sends the weekly report by mail', 'evidence': [card['id']]}]})
    result = asyncio.run(Consolidator(learning, NoMemory(), rt.make).catch_up())

    assert rt.schemas == [PROPOSAL_SCHEMA]
    assert result['status'] == 'done'
    [source] = result['sources']
    assert 'error' not in source, source
    assert len(source['accepted']) == 3 and len(source['refused']) == 1
    kinds = {learning.get(i)['kind']: learning.get(i)['body'] for i in source['accepted']}
    assert kinds['episode_annotation']['slots'] == {'recipient': 'a@kartikey.xyz'}
    procedure = kinds['procedure']
    assert procedure['slots'] == {'recipient': 'string'}
    assert procedure['preconditions'] == {'apps': {'com.apple.mail': '16.0'}}
    assert procedure['steps'][0]['args'] == {'to': '{recipient}', 'cc': []}
    assert procedure['steps'][0]['optional'] is False
    assert procedure['examples'] == [{'episode': card['id'], 'slots': {'recipient': 'a@kartikey.xyz'}}]
    assert kinds['identity_proposal'] == {'text': 'Sends the weekly report by mail'}
    assert learning.db.conn.execute("SELECT status FROM learning_jobs WHERE source_ref=?",
                                    (card['id'],)).fetchone()[0] == 'done'


def test_a_value_that_is_not_json_text_is_kept_as_the_literal_string(stores) -> None:
    """A model often writes a bare string where JSON text was asked for: it means that string, not a broken item."""
    _, _, _, learning = stores
    card = learning.put('episode', 't-2', {'task_id': 't-2', 'request': 'send it', 'verified': True, 'status': 'done',
                                           'slots': {}, 'actions': []}, [], origin_label='ferb',
                        made_by='code:evidence-card/1', state='active')
    rt = ProposalRuntime({})
    accepted, refused = Consolidator(learning, NoMemory(), rt.make).apply({'episodes': [
        {'episode': card['id'], 'intent_signature': 'send', 'reflection': '', 'narrative': '',
         'slots': [{'name': 'recipient', 'value': 'a@kartikey.xyz'}, {'name': 'count', 'value': '2'}]},
        {'episode': card['id'], 'intent_signature': 'send', 'reflection': '', 'narrative': '',
         'slots': [{'name': 'to', 'value': '"x"'}, {'name': 'to', 'value': '"y"'}]}]}, {card['id']}, 'b-1', 'fake')
    assert refused == ['to is given twice']
    assert [learning.get(i)['body']['slots'] for i in accepted] == [{'recipient': 'a@kartikey.xyz', 'count': 2}]


# (d) ---------------------------------------------------------------------------------------------------------------

def test_security_digest_reads_network_as_text_or_as_proxy_facts(stores) -> None:
    db, _, spans, _ = stores
    rules = spans.start('tool.call', task_id='t-2', input={'tool': 'shell.rules', 'args': {}})
    spans.finish(rules, output={'rules': [], 'network': 'no network beyond this Mac; FERB\'s proxy is not running'})
    run = spans.start('tool.call', task_id='t-2', input={'tool': 'shell.run', 'args': {'command': 'curl x.test'}})
    spans.finish(run, output={'exit_code': 7, 'network': {'destinations': [], 'refused': [{'host': 'x.test'}]}})
    digest = security_digest.build(db, DAY)
    assert digest['blocked_flows'] == [{'span': run.id, 'network': {'host': 'x.test'}}]


# (e) ---------------------------------------------------------------------------------------------------------------

def test_one_failing_hourly_memory_job_never_stops_the_others(daemon) -> None:
    d = daemon.daemon
    ran: list[str] = []

    def job(name: str, error: Exception | None = None):
        async def run() -> None:
            ran.append(name)
            if error is not None:
                raise error
        return run

    d.distiller.catch_up = job('distill')
    d.consolidator.catch_up = job('consolidate', RuntimeError('Codex runtime reported an error'))
    d._restore_memory_embedding = job('embedding')
    daemon.run(d._memory_pass())
    assert ran == ['distill', 'consolidate', 'embedding']

    ran.clear()
    d.distiller.catch_up = job('distill', AttributeError("'str' object has no attribute 'get'"))
    d.consolidator.catch_up = job('consolidate')
    daemon.run(d._memory_pass())
    assert ran == ['distill', 'consolidate', 'embedding']

    async def failures() -> list[tuple[str, str]]:
        return [(r[0], json.loads(r[1])['error']) for r in d.db.conn.execute(
            "SELECT kind, data_json FROM journal WHERE kind LIKE 'memory.%failed' ORDER BY seq")]
    assert daemon.run(failures()) == [('memory.consolidate_failed', 'RuntimeError: Codex runtime reported an error'),
                        ('memory.distill_failed', "AttributeError: 'str' object has no attribute 'get'")]
