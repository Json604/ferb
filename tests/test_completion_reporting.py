"""Regressions from the owner's Coursera / Claude redo audit."""

import json

import pytest

from tests.conftest import chat, cond, done, install, outcome, sup, task


def task_rows(daemon, task_id):
    async def read():
        return daemon.daemon.tasks.rows(task_id)
    return daemon.run(read())


def test_recovery_cannot_hide_failed_checks_behind_a_success_summary(daemon):
    evidence = 'Visible SDP duration is 4 hours and HCI is 5 hours; both were omitted.'
    verdict = json.dumps({'results': [{'index': 0, 'met': False, 'evidence': evidence}]})
    finish = json.dumps({'action': 'finish', 'worker_type': 'default',
                         'instructions': 'I checked every assignment.', 'question': ''})
    install(daemon, [sup('task', 'Checking', task(conds=(cond('Report visible requirements'),)))],
            {'worker': done('I checked every assignment.'), 'worker.verify': verdict, 'worker.dispatch': finish})
    out = outcome(chat(daemon, 'Check my assignments'))
    assert out['status'] == 'failed' and not out['verified']
    assert evidence in out['answer'] and evidence in out['summary']
    assert 'couldn\'t confirm' in out['answer']


def test_verifier_receives_and_owner_keeps_the_complete_long_report(daemon):
    report = 'Prepared calculations. ' * 300 + 'Final result: Q5 recommends Project A.'
    verdict = json.dumps({'results': [{'index': 0, 'met': True, 'evidence': 'Read Q5.'}]})
    script = install(daemon, [sup('task', 'Preparing', task(conds=(cond('Q5 answered'),)))],
                     {'worker': done(report), 'worker.verify': verdict})
    out = outcome(chat(daemon, 'Prepare all answers'))
    context = next(ctx for _, key, ctx, _ in script.contexts if key == 'worker.verify')
    assert report in context
    assert out['answer'] == report
    rows = task_rows(daemon, out['task_id'])
    assert all(row['status'] not in ('running', 'planned') for row in rows)
    check = next(row for row in rows if row['role'] == 'verifier')
    assert json.loads(check['outcome_json'])['results'][0]['met'] is True


def test_pending_question_leads_without_losing_any_condition_or_evidence():
    from types import SimpleNamespace

    from ferb.orchestrator.supervisor import Outcome, _report
    from ferb.orchestrator.verify import UNCHECKED, ConditionResult

    question = 'Which Gmail account should receive the draft?'
    conditions = ['the intended Gmail account is open', 'the attachment is included',
                  'the draft is saved', 'the message is not sent']
    tree = SimpleNamespace(
        spec=SimpleNamespace(success_conditions=[SimpleNamespace(owner_text=text) for text in conditions]),
        owner_question=question, stop_reason='Gmail has signed out.', verification=[1])
    results = [
        ConditionResult(0, conditions[0], False, 'verifier', 'Gmail shows a different account.'),
        ConditionResult(1, conditions[1], None, 'verifier', 'The attachment preview did not load.',
                        how='Open the draft and check the attachment.'),
        ConditionResult(2, conditions[2], None, UNCHECKED, 'The check did not finish.'),
        ConditionResult(3, conditions[3], True, 'verifier', 'The draft is still unsent.'),
    ]
    report = _report(tree, results, Outcome('t', 'needs_user', '', ['draft.txt'], []),
                     'I need your answer to finish this task.', '')
    assert report.startswith('I need your answer to finish this task.\n\nQuestion: ' + question + '\n')
    assert report.count(question) == 1
    assert 'the intended Gmail account is open — failed: Gmail shows a different account.' in report
    assert 'the attachment is included — unverified: The attachment preview did not load.' in report
    assert 'To check: Open the draft and check the attachment.' in report
    assert 'Why work stopped: Gmail has signed out.' in report
    assert 'Confirmed:\n- the message is not sent' in report
    assert 'Still to check (the check couldn\'t run, a problem on my side):\n- the draft is saved' in report
    assert 'Artifact paths (these do not establish completeness):\n- draft.txt' in report


@pytest.mark.parametrize('verdict,status', [('{}', 'failed'), ('invalid json', 'failed'),
    (json.dumps({'results': [{'index': 0, 'met': False, 'evidence': 'Missing Q5'}]}), 'done')])
def test_verifier_terminal_state_survives_bad_reports_and_unmet_conditions(daemon, verdict, status):
    install(daemon, [sup('task', 'Checking', task(conds=(cond('Q5 answered'),)))],
            {'worker': done('Prepared'), 'worker.verify': verdict})
    out = outcome(chat(daemon, 'Prepare all answers'))
    check = next(r for r in task_rows(daemon, out['task_id']) if r['role'] == 'verifier')
    assert check['status'] == status


def test_retried_verifier_is_running_during_each_attempt_then_finishes(daemon):
    states = []
    def inspect(token):
        tid = daemon.daemon.tokens.validate(token).task_id
        async def state():
            return daemon.daemon.db.conn.execute('SELECT status FROM tasks WHERE id=?', (tid,)).fetchone()[0]
        states.append(daemon.run(state()))
        return json.dumps({'results': [{'index': 0, 'met': len(states) > 1, 'evidence': 'Read current draft.'}]})
    retry = json.dumps({'action': 'retry', 'worker_type': 'default', 'instructions': 'Add Q5.', 'question': ''})
    install(daemon, [sup('task', 'Preparing', task(conds=(cond('Q5 answered'),)))],
            {'worker': [done('Draft'), done('Complete')], 'worker.verify': [inspect, inspect],
             'worker.dispatch': retry})
    out = outcome(chat(daemon, 'Prepare all answers'))
    assert states == ['running', 'running']
    assert out['verified']
    assert all(r['status'] != 'running' for r in task_rows(daemon, out['task_id']))


def test_unknown_check_is_not_cut_off_after_a_long_report(daemon):
    verdict = json.dumps({'results': [{'index': 0, 'met': None, 'evidence': 'Attachment could not be read.'}]})
    install(daemon, [sup('task', 'Checking', task(conds=(cond('All attachment pages checked'),)))],
            {'worker': done('Long report. ' * 400), 'worker.verify': verdict})
    out = outcome(chat(daemon, 'Check the attachment'))
    assert not out['verified']
    assert 'Attachment could not be read.' in out['answer']


def test_stopping_a_verifier_persists_its_cancelled_state(daemon):
    import time
    from ferb.cli.client import Client
    from tests.conftest import STALL
    script = install(daemon, [sup('task', 'Checking', task(conds=(cond('All pages checked'),)))],
                     {'worker': done('Draft'), 'worker.verify': STALL})
    client = Client(daemon.sock)
    try:
        stream = client.stream('chat.send', {'session_id': 's1', 'text': 'Check the report'})
        root = next(stream)['task_id']
        for _ in range(100):
            if any(key == 'worker.verify' for _, key, _, _ in script.contexts):
                break
            time.sleep(.01)
        stopper = Client(daemon.sock)
        try:
            stopper.call('task.stop', {'task_ids': [root]})
        finally:
            stopper.close()
        assert outcome(list(stream))['status'] == 'cancelled'
        check = next(r for r in task_rows(daemon, root) if r['role'] == 'verifier')
        assert check['status'] == 'cancelled'
    finally:
        client.close()
