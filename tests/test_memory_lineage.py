import asyncio
import json
from types import SimpleNamespace

from ferb.memory.lineage import call_sources
from ferb.orchestrator.evidence import TaskEvidence


def test_replayed_memory_and_trail_results_keep_their_source_ids():
    records = [
        {'kind': 'tool.call', 'input': {'tool': 'memory.read', 'args': {}},
         'output': {'items': [{'id': 'private-fact'}]}},
        {'kind': 'tool.call', 'input': {'tool': 'trail.query', 'args': {}},
         'output': {'events': [{'id': 'private-event'}]}}]
    for record, expected in zip(records, ('memory:private-fact', 'trail:private-event')):
        assert call_sources({'tool': 'task.evidence'}, {'text': json.dumps(record)}) == {expected}


def test_paged_evidence_keeps_context_lineage_outside_partial_json():
    asyncio.run(_paged_evidence())


async def _paged_evidence():
    record = {'id': 'context', 'kind': 'context.source', 'origin': 'private',
              'input': {'label': 'Selected procedure', 'source_refs': ['m-procedure', 'trail:source']},
              'output': {'text': 'Long source text ' * 200}}

    class Spans:
        def tool_call(self, *args, **kwargs):
            return record

        def tool_calls(self, *args, **kwargs):
            return {'records': [{'id': 'context', 'kind': 'context.source', 'origin': 'private'}]}

    tool = TaskEvidence(Spans(), lambda _: None)
    ctx = SimpleNamespace(task_id='task')
    page = await tool.run(tool.Input(span_id='context', max_chars=1000, offset=1000), ctx)
    assert page.content['next_offset'] is not None
    assert call_sources({'tool': 'task.evidence'}, page.content) == {'m-procedure', 'trail:source'}
    listing = await tool.run(tool.Input(), ctx)
    assert call_sources({'tool': 'task.evidence'}, listing.content) == {'m-procedure', 'trail:source'}


def test_learned_candidate_pages_keep_source_ids_when_json_is_partial():
    from ferb.memory.tools import LearningRead
    node = {'id': 'm-private', 'kind': 'lesson', 'key': 'long phrase ' * 200, 'state': 'candidate',
            'origin': 'private', 'origins': ['private'], 'recorded_at': '2026-10-11', 'sources': [],
            'body': {'trigger': 'a long phrase', 'text': 'check the label'}}
    tool = LearningRead(lambda: SimpleNamespace(search=lambda *a, **k: [node]))
    page = asyncio.run(tool.run(tool.Input(max_chars=1000, offset=1000), SimpleNamespace(task_id='task')))
    assert page.content['next_offset'] is not None
    assert call_sources({'tool': 'memory.learned'}, page.content) == {'m-private'}


def test_daily_summary_reader_exposes_stored_and_raw_source_lineage():
    from ferb.sense.tools import TrailDays
    from unittest.mock import patch
    stored = {'id': 'day-summary', 'day': '2026-10-11', 'kind': 'summary', 'data': {}}
    trail = SimpleNamespace(db=object(), days=lambda **kwargs: [stored])
    tool = TrailDays(lambda: trail)
    with patch('ferb.memory.summary.build', return_value=SimpleNamespace(handles={'A1': [('trail', 'event')]})):
        page = asyncio.run(tool.run(tool.Input(), SimpleNamespace(task_id='task')))
    assert call_sources({'tool': 'trail.days'}, page.content) == {'journal:day-summary', 'trail:event'}
