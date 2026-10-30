"""P3/P27/P31 protected context and owner-facing reporting are separate from evidence reports."""
from ferb.core.context import ContextBlock, assemble_context
from ferb.orchestrator.language import internal_terms


def test_context_drops_activity_before_memory_and_keeps_complete_owner_request(monkeypatch):
    monkeypatch.setattr("ferb.core.context.count_tokens", len)
    blocks = [ContextBlock('identity', 'identity', protected=True),
              ContextBlock('ACTIVITY ' * 100, 'trail.query', priority=1),
              ContextBlock('IMPORTANT MEMORY', 'memory.read', priority=2),
              ContextBlock('YOUR COMPLETE REQUEST', 'request', protected=True),
              ContextBlock('CONFIRMATIONS AND OUTCOMES', 'state', protected=True)]
    result = assemble_context(blocks, budget_tokens=600)
    assert 'YOUR COMPLETE REQUEST' in result and 'CONFIRMATIONS AND OUTCOMES' in result
    assert 'IMPORTANT MEMORY' in result
    assert 'ACTIVITY ' * 100 not in result
    assert 'trail.query' in result


def test_language_audit_does_not_rewrite_the_answer():
    text = 'The gateway could not check the worker runtime.'
    assert internal_terms(text) == ['gateway', 'worker runtime']
    assert text == 'The gateway could not check the worker runtime.'


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


# L29 (spec 5): the worker starts from the procedure's filled steps, its intent's lessons and the latest runs ----------

FOOD = "cheapest food order"


def _parts(daemon, proc, slots):
    from tests.support.learning_fixtures import on_loop
    return on_loop(daemon, daemon.daemon.learning.procedure_parts, proc, {}, slots)


def _card(daemon, task_id):
    from tests.support.learning_fixtures import on_loop
    return on_loop(daemon, daemon.daemon.learning.current_card, task_id)
