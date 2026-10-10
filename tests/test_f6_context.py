"""P3/P27/P31 protected context and owner-facing reporting are separate from evidence reports."""
from ferb.core.context import ContextBlock, assemble_context
from ferb.orchestrator.language import internal_terms


def test_context_drops_activity_before_memory_and_keeps_complete_owner_request():
    blocks = [ContextBlock('identity', 'identity', protected=True),
              ContextBlock('ACTIVITY ' * 100, 'trail.query', priority=1),
              ContextBlock('IMPORTANT MEMORY', 'memory.read', priority=2),
              ContextBlock('YOUR COMPLETE REQUEST', 'request', protected=True),
              ContextBlock('CONFIRMATIONS AND OUTCOMES', 'state', protected=True)]
    result = assemble_context(blocks, budget_tokens=400)
    assert 'YOUR COMPLETE REQUEST' in result and 'CONFIRMATIONS AND OUTCOMES' in result
    assert 'IMPORTANT MEMORY' in result
    assert 'ACTIVITY ' * 100 not in result
    assert 'trail.query' in result


def test_language_audit_does_not_rewrite_the_answer():
    text = 'The gateway could not check the worker runtime.'
    assert internal_terms(text) == ['gateway', 'worker runtime']
    assert text == 'The gateway could not check the worker runtime.'


def test_selected_procedure_and_owner_answer_are_checked_in_same_task(daemon):
    import json
    from tests.conftest import chat, install, outcome, sup, task
    spec = task('Inspect the session file')
    spec['procedure_id'] = 'procedure-session'
    inspected = []
    supervisor = daemon.daemon.supervisor
    supervisor.procedure_details = lambda key, worker: inspected.append((key, worker.id)) or 'Read and verify the file.'
    supervisor.on_episode = lambda key: inspected.append(('episode', key))
    supervisor.procedure_outcome = lambda key, task_id: inspected.append(('outcome', key, task_id))
    install(daemon, [sup('task', 'Checking your file.', spec)], {'worker': json.dumps({
        'status': 'done', 'summary': 'Technical file inspection evidence.', 'answer': 'Your session file is complete.',
        'artifacts': [], 'evidence': ['File inspected'], 'input_notes': []})})
    result = outcome(chat(daemon, 'Inspect the session file'))
    assert result['answer'] == 'Verified. Your session file is complete.'
    assert any(row[0] == 'procedure-session' for row in inspected)
    assert any(row[0] == 'episode' for row in inspected)
    assert any(row[0] == 'outcome' for row in inspected)


def test_ranked_memory_budget_keeps_best_item_when_all_candidates_do_not_fit(monkeypatch):
    from ferb.orchestrator.context_layers import ranked_memory_blocks
    monkeypatch.setattr('ferb.core.context.count_tokens', len)
    blocks = ranked_memory_blocks([
        {'text': 'BEST ' * 12, 'reference': 'memory.learned id=best', 'origin': 'private'},
        {'text': 'LOWEST ' * 100, 'reference': 'memory.learned id=low', 'origin': 'external:web:example.com'},
    ])
    result = assemble_context([ContextBlock('owner request', 'request', protected=True)] +
                              [block for block, _ in blocks], budget_tokens=600)
    assert 'BEST ' * 12 in result and 'LOWEST ' * 100 not in result
    assert 'memory.learned id=low' in result
    assert blocks[1][1] == {'external:web:example.com'}
