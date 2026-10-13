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
from ferb.core.types import RuntimeEvent
from ferb.memory.consolidate import PROPOSAL_SCHEMA, Consolidator
from ferb.memory.learning import Learning
from ferb.orchestrator import episodes
from ferb.policy import security_digest
from ferb.sanitize.redact import REDACTED, Redactor, payment_card
from ferb.store.db import Database
from ferb.store.spans import SpanStore
from ferb.trace import compose
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


# (b) ---------------------------------------------------------------------------------------------------------------

def test_episode_capture_with_refs_recorded_through_a_redacted_span_succeeds(stores) -> None:
    db, _, spans, learning = stores
    entity = next(node for node in (learning.put('entity', f'owner-{i}', {'text': 'The owner'}, [],
                                                 made_by='code:entity-summary/1', state='active')
                                    for i in range(20000)) if card_shaped(node['id']))
    trace = TraceBuilder(task_id='t-recall', request='What do you know about me?', goal='answer from memory').finish()
    episodes.write(db, compose.compose(trace) | {'made_by': episodes.MADE_BY, 'built_at': learning.now()})
    exposure = spans.start('context.exposure', task_id='t-recall',
                           input={'source_refs': [entity['id']], 'reference': 'Layer 4 — memory'})
    spans.finish(exposure, output={'text': 'The owner'}, origin='inferred')
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
    slot = [{'name': 'recipient', 'value': '"a@kartikey.xyz"'}]
    rt = ProposalRuntime({
        'episodes': [
            {'episode': card['id'], 'intent_signature': 'send report', 'slots': slot,
             'reflection': 'Sent on the first try.', 'narrative': 'Sent the weekly report.'},
            {'episode': card['id'], 'intent_signature': 'send report', 'reflection': '', 'narrative': '',
             'slots': [{'name': 'recipient', 'value': 'a@kartikey.xyz'}]}],          # not JSON text: refused
        'procedures': [{
            'intent': 'send the weekly report', 'slots': [{'name': 'recipient', 'value': 'string'}],
            'preconditions': {'apps': [{'name': 'com.apple.mail', 'value': '16.0'}]},
            'steps': [{'tool': 'mail.send', 'expect': 'the report is in Sent', 'evidence': ['t-1-action'],
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
    assert procedure['examples'] == [{'episode': card['id'], 'slots': {'recipient': 'a@kartikey.xyz'}}]
    assert kinds['identity_proposal'] == {'text': 'Sends the weekly report by mail'}
    assert learning.db.conn.execute("SELECT status FROM learning_jobs WHERE source_ref=?",
                                    (card['id'],)).fetchone()[0] == 'done'


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
