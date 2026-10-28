"""Offline reproductions of the October 27–28 coding-session and visa-check incidents."""
import json

import pytest
from pydantic import ValidationError

from ferb.orchestrator.schemas import Condition, VerifierResult
from tests.conftest import chat, cond, done, install, outcome, sup, task


def finish(result='unconfirmed'):
    return json.dumps(dict(action='finish', worker_type='default', instructions='Report the remaining uncertainty.', question='',
                           result=result, stop_reason=''))


def verdict(result=True, conduct=None, report=True, correction=False):
    rows = [dict(index=0, met=result, evidence='All five locations showed no slots.' if result else
                 'The portal error prevented a complete check.'),
            dict(index=1, met=conduct, evidence='Owner input coincided with the window switch; no FERB focus action.'),
            dict(index=2, met=report, evidence='The report matches the recorded results.' if report else
                 'The claimed booking is contradicted by no-slot observations.')]
    if correction:
        rows[1].update(effective_condition='FERB did not change the owner’s foreground.',
                       correction_reason='The request restricts FERB’s actions, not the owner’s window switches.')
    return json.dumps(dict(results=rows))


def setup(daemon, report, checked, resources=None):
    conditions = [cond('Availability checked'), cond('Foreground unchanged')]
    conditions[1]['category'] = 'conduct'
    scripts = {'worker': done(report), 'worker.verify': checked, 'worker.dispatch': finish()}
    if resources:
        scripts['worker.resources'] = resources
    return install(daemon, [sup('task', 'Checking', task(conds=conditions))], scripts)


def test_partial_retry_leads_with_checked_visa_findings_not_playback_uncertainty(daemon):
    report = 'All five locations show no slots. Nothing booked; the six-hour schedule remains active.'
    setup(daemon, report, verdict())
    out = outcome(chat(daemon, 'Retry the visa check without disturbing playback'))
    assert out['status'] == 'done_unverified'
    assert out['answer'].startswith(report + '\n\n')
    assert 'unverified' in out['answer'] and 'window switch' in out['answer']


def test_rejected_report_is_replaced_with_concrete_passed_findings(daemon):
    rejected = 'I booked an appointment.'
    setup(daemon, rejected, verdict(report=False))
    out = outcome(chat(daemon, 'Check and book if available'))
    assert out['status'] == 'failed'
    assert rejected not in out['answer']
    assert 'Confirmed:\n- All five locations showed no slots.' in out['answer']


def test_correction_keeps_original_and_rationale_in_final_verification(daemon):
    setup(daemon, 'Three coding sessions found; FERB used background reads.',
          verdict(conduct=True, correction=True))
    out = outcome(chat(daemon, 'Inspect coding sessions without switching my windows'))
    assert out['status'] == 'done'
    corrected = out['verification'][1]
    assert corrected['condition'] == 'Foreground unchanged'
    assert corrected['category'] == 'conduct'
    assert corrected['effective_condition'] == 'FERB did not change the owner’s foreground.'
    assert 'owner’s window switches' in corrected['correction_reason']


@pytest.mark.parametrize('checked,expected', [(True, 'done'), (None, 'done_unverified'), (False, 'failed')])
def test_negative_check_and_inability_to_check_keep_distinct_verdicts(daemon, checked, expected):
    setup(daemon, 'No booking was made.', verdict(result=checked, conduct=True))
    assert outcome(chat(daemon, 'Check availability'))['status'] == expected


def test_legacy_conditions_and_verdicts_load_but_correction_needs_reason():
    assert Condition.model_validate(cond('checked')).category == 'result'
    assert VerifierResult(index=0, met=True, evidence='read').effective_condition is None
    with pytest.raises(ValidationError):
        VerifierResult(index=0, met=True, evidence='read', effective_condition='different requirement')


def test_cleanup_failure_cannot_republish_pre_cleanup_report(daemon):
    async def cleanup(task_id):
        return [{'tab': 't1', 'closed': True, 'released': True}] if task_id.endswith('.worker') else []
    daemon.daemon.supervisor.on_task_end = cleanup
    report = 'All five locations show no slots. Playback is still running.'
    failed = json.dumps(dict(status='failed', summary='Cleanup closed the playback tab.', artifacts=[], evidence=[]))
    setup(daemon, report, verdict(), resources=failed)
    out = outcome(chat(daemon, 'Retry the visa check without disturbing playback'))
    assert report not in out['answer']
    assert 'Confirmed before cleanup (final state may differ):' in out['answer']
    assert out['status'] == 'failed'


def test_proven_ferb_focus_violation_still_fails_even_with_truthful_report(daemon):
    rows = json.loads(verdict(conduct=False))
    rows['results'][1]['evidence'] = 'FERB activated the terminal despite the background-only request.'
    setup(daemon, 'Availability was checked. FERB also activated the terminal.', json.dumps(rows))
    out = outcome(chat(daemon, 'Check availability in the background'))
    assert out['status'] == 'failed' and not out['verified']
    assert 'failed: FERB activated the terminal' in out['answer']


def test_coverage_evidence_recheck_runs_no_second_worker(daemon):
    script = setup(daemon, 'All five locations showed no slots.', verdict(result=None, conduct=True))
    script.workers['worker.verify'] = [verdict(result=None, conduct=True), verdict(conduct=True)]
    script.workers['worker.dispatch'] = [json.dumps(dict(action='recheck', worker_type='default',
        instructions='Inspect the recorded selector options and five results; do not repeat the external checks.',
        question=''))]
    out = outcome(chat(daemon, 'Check all locations and book if available'))
    assert out['status'] == 'done'
    assert sum(key == 'worker' for _, key, _, _ in script.contexts) == 1
    assert sum(key == 'worker.verify' for _, key, _, _ in script.contexts) == 2
    checked_contexts = [ctx for _, key, ctx, _ in script.contexts if key == 'worker.verify']
    assert 'Inspect the recorded selector options and five results' in checked_contexts[-1]
    assert 'Inspect the recorded selector options and five results' not in checked_contexts[0]


def test_a_later_owner_correction_prevents_reusing_checked_prose():
    from types import SimpleNamespace
    from ferb.orchestrator.supervisor import Outcome, _report
    from ferb.orchestrator.verify import ConditionResult

    tree = SimpleNamespace(spec=SimpleNamespace(success_conditions=[SimpleNamespace(owner_text='Availability checked')]),
        owner_question='', stop_reason='', verification=[1], watch=False, check_problem='',
        checked_report='The old requested location has no slots.', checked_report_invalidated=False,
        checked_report_revision=0, steering=SimpleNamespace(messages=['Use another location instead']))
    results = [ConditionResult(0, 'Availability checked', None, 'verifier', 'New location remains unchecked.'),
               ConditionResult(1, 'Report truthfulness', True, 'verifier', 'The previous report was truthful.')]
    answer = _report(tree, results, Outcome('t', 'done', '', [], []), 'Unconfirmed:', '')
    assert tree.checked_report not in answer
    assert 'New location remains unchecked.' in answer


def test_unfinished_checker_preserves_conduct_classification(daemon):
    setup(daemon, 'Background reads completed.', 'not a verdict')
    out = outcome(chat(daemon, 'Inspect sessions in the background'))
    assert out['verification'][1]['category'] == 'conduct'
    assert out['verification'][1]['path'] == 'unchecked'
