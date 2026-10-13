"""Retrieved outside content remains outside data at the sink, even before a tool read."""
from ferb.core.context import ContextBlock


def test_only_retained_memory_is_carried_from_planner_to_task(daemon):
    from tests.conftest import chat, done, install, outcome, sup, task
    supervisor = daemon.daemon.supervisor
    supervisor.context_budget_tokens = 30_000
    supervisor.memory_candidates = lambda words: [
        {'text': 'A remembered outside instruction.', 'reference': 'memory.learned id=outside',
         'origin': 'external:web:kartikey.xyz', 'source_refs': ['outside']},
        {'text': 'OMITTED ' * 100_000, 'reference': 'memory.learned id=omitted',
         'origin': 'private', 'source_refs': ['omitted']},
    ]
    install(daemon, [sup('task', 'Checking.', task('Inspect the source'))], {'worker': done('Inspected.')})
    result = outcome(chat(daemon, 'Inspect the source'))
    async def recorded():
        rows = daemon.daemon.db.conn.execute("SELECT input_json,origin FROM spans WHERE kind='context.exposure' AND task_id=?",
                                            (result['task_id'],)).fetchall()
        spans = daemon.daemon.spans
        return [dict(r) for r in rows], spans.exposure_origins(result['task_id'])
    rows, exposed = daemon.run(recorded())
    assert 'external:web:kartikey.xyz' in exposed
    assert any('outside' in row['input_json'] for row in rows)
    assert not any('omitted' in row['input_json'] for row in rows)


def test_exposed_context_text_participates_in_private_url_and_argument_checks(daemon):
    from tests.conftest import gateway_setup, gateway_request
    grant, _ = gateway_setup(daemon, set(), gateway_request('Inspect my files'), [])
    async def check():
        spans = daemon.daemon.spans
        one = spans.start('context.exposure', task_id=grant.task_id, input={'source_refs': ['memory:one']})
        spans.finish(one, output={'text': 'private-customer-phrase'}, origin='private')
        two = spans.start('context.exposure', task_id=grant.task_id, input={'source_refs': ['memory:two']})
        spans.finish(two, output={'text': 'new-recipient@example.com'}, origin='external:web:kartikey.xyz')
        gateway = daemon.daemon.gateway
        private, outside = gateway._sink_texts(grant.task_id)
        assert len(private) == 1 and len(outside) == 1
        assert spans.exposure_origins(grant.task_id) == {'private', 'external:web:kartikey.xyz'}
    daemon.run(check())


def test_activity_then_displayed_history_preserves_forget_edges(daemon):
    from ferb.sense.trail import Observation
    from tests.conftest import chat, done, install, outcome, sup, task
    supervisor = daemon.daemon.supervisor
    supervisor.memory_candidates = lambda _: []
    supervisor.context_budget_tokens = 1_000_000
    async def record():
        return daemon.daemon.trail.record(Observation('clipboard', {'text': 'Owner session comparison note'}, {'actor': 'owner'})).id
    source = daemon.run(record())
    install(daemon, [sup('task', 'Inspecting.', task('Inspect session note'))], {'worker': done('Inspected session note.')})
    first = outcome(chat(daemon, 'Inspect session note'))
    supervisor.recent_activity = lambda: ('', 'ferb')
    supervisor.relevant_context = lambda _: ('', 'ferb')
    supervisor.relevant_source_refs = lambda _: []
    install(daemon, [sup('task', 'Continuing.', task('Compare recorded session result'))], {'worker': done('Compared recorded result.')})
    second = outcome(chat(daemon, 'Compare that recorded session result'))
    async def check():
        learning = daemon.daemon.learning
        cards = {node['key']: node for node in learning.items('episode')}
        a, b = cards[first['task_id']], cards[second['task_id']]
        assert 'trail:' + source in a['sources']
        assert a['id'] in b['sources'] and 'trail:' + source in b['sources']
        learning.forget(['trail:' + source])
        assert learning.get(a['id']) is None and learning.get(b['id']) is None
    daemon.run(check())


def test_recent_and_relevant_activity_both_keep_labels_and_source_refs(daemon):
    import json
    from tests.conftest import chat, done, install, sup, task
    supervisor = daemon.daemon.supervisor
    supervisor.relevant_context = lambda _: ('Recorded matching action', 'private')
    supervisor.relevant_source_refs = lambda _: ['trail:matching']
    supervisor.recent_activity = lambda: (json.dumps({'apps': [], 'source_refs': ['trail:recent']}), 'external:clipboard')
    script = install(daemon, [sup('task', 'Read the context.', task('Compare activity'))], {'worker': done('Compared.')})
    chat(daemon, 'Compare activity')
    assert 'Recorded matching action' in script.contexts[0][2]
    async def exposure():
        return [dict(row) for row in daemon.daemon.db.conn.execute(
            "SELECT input_json,origin FROM spans WHERE kind='context.exposure'")]
    rows = daemon.run(exposure())
    matched = [row for row in rows if 'trail:matching' in row['input_json']]
    assert matched and any('trail:recent' in row['input_json'] for row in matched)
    assert any('private' in row['origin'] and 'external:clipboard' in row['origin'] for row in matched)


def test_context_payload_omits_catalogue_but_preserves_recorded_memory(daemon):
    import json
    from tests.conftest import chat, done, install, outcome, sup, task
    supervisor = daemon.daemon.supervisor
    supervisor.relevant_context = lambda _: ('Installed catalogue: search metadata. Private account: Customer-Record',
                                              'private', 'Customer-Record')
    supervisor.recent_activity = lambda: ('', 'ferb')
    supervisor.memory_candidates = lambda _: [
        {'text': 'Remembered-Private-Value', 'reference': 'memory.learned id=private',
         'origin': 'private', 'source_refs': ['private']}]
    install(daemon, [sup('task', 'Checking.', task('Inspect the source'))], {'worker': done('Inspected.')})
    result = outcome(chat(daemon, 'Inspect the source'))
    async def check():
        rows = daemon.daemon.db.conn.execute(
            "SELECT output_json FROM spans WHERE kind='context.exposure' AND task_id=?", (result['task_id'],)).fetchall()
        values = [json.loads(row['output_json']) for row in rows]
        assert any('Installed catalogue' in value['text'] and value['payload'] == 'Customer-Record' for value in values)
        assert any(value.get('payload') == 'Remembered-Private-Value' for value in values)
        private, _ = daemon.daemon.gateway._sink_texts(result['task_id'])
        assert any(value.folded == 'remembered-private-value' for _, value in private)
        assert all('installed catalogue' not in value.folded for _, value in private)
    daemon.run(check())


def test_payload_cache_prepares_only_new_evidence_and_retains_late_context(daemon, monkeypatch):
    from ferb.policy import provenance
    from tests.conftest import gateway_setup, gateway_request
    grant, _ = gateway_setup(daemon, set(), gateway_request('Inspect my files'), [])
    prepared = []
    original = provenance.prepare
    def track(text):
        prepared.append(text)
        return original(text)
    monkeypatch.setattr(provenance, 'prepare', track)
    async def check():
        spans, gateway = daemon.daemon.spans, daemon.daemon.gateway
        pending = spans.start('context.exposure', task_id=grant.task_id, input={'source_refs':['memory:late']})
        one = spans.start('context.exposure', task_id=grant.task_id, input={})
        spans.finish(one, output={'text':'source wrapper', 'payload':'Initial-Private-Value'}, origin='private')
        gateway._sink_texts(grant.task_id)
        before = len(prepared)
        gateway._sink_texts(grant.task_id)
        assert len(prepared) == before
        spans.finish(pending, output={'text':'Late-Private-Value'}, origin='private')
        private, _ = gateway._sink_texts(grant.task_id)
        assert len(prepared) == before + 1
        assert [value.folded for _, value in private] == ['late-private-value', 'initial-private-value']
    daemon.run(check())
