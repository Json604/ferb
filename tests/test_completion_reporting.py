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
