"""Export request provenance and every outcome layer without choosing ambiguous task prefixes."""

import pytest

from ferb.cli.client import Client
from ferb.core.types import Identity, Task
from ferb.orchestrator.tree import TaskRecord


def test_trace_exports_planner_intake_and_measured_outcomes(daemon):
    async def seed():
        d = daemon.daemon
        planner = d.spans.start('model.turn', input={'request_id': 'req26', 'role': 'supervisor'})
        d.spans.finish(planner, output={'decision': {'kind': 'task', 'text': 'plan'}})
        root = d.spans.start('task', task_id='export26', input={'request_id': 'req26', 'request': 'full owner request',
                            'authority_words': 'owner request', 'task_spec': {'goal': 'do it'}})
        d.tasks.add(TaskRecord(Task('export26', None, 'do it', 'test', Identity.OWNER, set(), 'mixed', 'req26', [], None),
                               d.clock.now(), root.id, 'export26'))
        for kind in ('task.intake', 'tool.call', 'verification', 'disturbance.change', 'task.cleanup',
                     'model.dispatch', 'model.question', 'model.resources', 'context.source', 'worker'):
            event = d.spans.start(kind, task_id='export26', parent_id=root.id, input={'kind': kind})
            d.spans.finish(event, output={'observed': kind})
        d.spans.finish(root, output={'status': 'failed', 'answer': 'not verified'})
    daemon.run(seed())
    client = Client(daemon.sock)
    result = client.call('trace.get', {'task_id': 'export26'})
    assert result['outcome']['request']['text'] == 'full owner request'
    assert len(result['outcome']['planner_decisions']) == 1
    for key in ('actions', 'verification', 'disturbances', 'cleanup', 'intake'):
        assert result['outcome'][key], key
    assert {e['kind'] for e in result['outcome']['approach_decisions']} == {
        'model.turn', 'model.dispatch', 'model.question', 'model.resources'}
    assert result['outcome']['context_sources'] and result['outcome']['worker_runs']
    assert result['outcome']['owner_verdict'] is None
    assert result['outcome']['result']['status'] == 'failed'
    assert any(s['kind'] == 'model.turn' for s in result['spans'])


def test_trace_prefix_requires_unique_identity(daemon):
    async def seed():
        d = daemon.daemon
        for tid in ('abcdef111', 'abcdef222'):
            span = d.spans.start('task', task_id=tid, input={'request_id': tid})
            d.tasks.add(TaskRecord(Task(tid, None, tid, 'test', Identity.OWNER, set(), 'mixed', tid, [], None),
                                   d.clock.now(), span.id, tid))
    daemon.run(seed())
    client = Client(daemon.sock)
    with pytest.raises(Exception, match='ambiguous.*abcdef111.*abcdef222'):
        client.call('trace.get', {'task_id': 'abcdef'})
