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
        return [dict(r) for r in rows], daemon.daemon.gateway._outside_read(result['task_id'])
    rows, outside = daemon.run(recorded())
    assert outside
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
        assert gateway._outside_read(grant.task_id)
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
