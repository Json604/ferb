"""Owner-UID frontend transport: attachment data, reconnect facts and private answers; no live models/apps."""
import json

import pytest

from ferb.cli.client import Client, RpcError
from ferb.core.types import Identity
from ferb.orchestrator.plans import Plan, Step
from ferb.policy.rules import ActionRequest
from tests.conftest import chat, done, install, outcome, sup, task, tool
from tests.mackit import grant_task


def send(daemon, text, paths):
    client = Client(daemon.sock)
    try:
        return list(client.stream('chat.send', {'session_id': 'frontend', 'text': text, 'input_paths': paths}))
    finally:
        client.close()


def test_attach_advertises_attachment_support(daemon):
    client = Client(daemon.sock)
    try:
        assert 'input_paths' in client.call('chat.attach')['capabilities']
    finally:
        client.close()


@pytest.mark.parametrize('paths', ['file.txt', [None], [''], ['a\x00b']])
def test_invalid_attachment_shape_starts_no_turn(daemon, paths):
    script = install(daemon, [sup('reply', 'Should not run')])
    with pytest.raises(RpcError, match='input_paths'):
        send(daemon, 'Inspect these files', paths)
    assert not script.contexts


def test_attachments_reach_worker_as_data_without_contents_or_extra_authority(daemon):
    workspace = daemon.daemon.home / 'workspace'
    source = workspace / 'report with spaces.txt'
    source.write_text('SOURCE-CONTENT-NOT-AUTOMATICALLY-READ')
    seen = []
    def worker(token):
        grant = daemon.daemon.tokens.validate(token)
        record = daemon.daemon.tasks.get(grant.task_id)
        seen.append(daemon.daemon.state.requests.get(record.task.request_id))
        return done('Inspected the requested source metadata.')
    script = install(daemon, [sup('task', 'Inspecting.', task('Inspect attached report'))], {'worker': worker})
    outcome(send(daemon, 'Inspect the attached report', ['./report with spaces.txt', str(source)]))
    assert seen[0].words == 'Inspect the attached report'
    assert not seen[0].targets.paths
    assert str(source) in seen[0].payload and '<untrusted-data' in seen[0].payload
    assert 'SOURCE-CONTENT-NOT-AUTOMATICALLY-READ' not in str(script.contexts)
    assert all(str(source) in context for _, key, context, _ in script.contexts if key in ('supervisor', 'worker'))


def test_attachments_survive_clarification_and_become_plan_inventory(daemon):
    source = daemon.daemon.home / 'workspace' / 'session.txt'
    source.write_text('owner-controlled session')
    spec = task('Read session')
    spec['plan'] = Plan(steps=[Step(id='read', kind='do', goal='Read source metadata')]).model_dump()
    install(daemon, [sup('clarify', 'Which comparison?'), sup('task', 'Reading.', spec)],
            {'step.read': done('Read.'), 'worker': done('Checked.')})
    assert send(daemon, 'Use this session', ['session.txt'])[0]['type'] == 'clarify'
    result = outcome(send(daemon, 'Compare its approach', []))
    async def stored():
        return daemon.daemon.supervisor.continuity.load(result['task_id'], 'frontend')[0]
    assert daemon.run(stored()).plan.input_paths == [str(source)]


def test_attaching_ferb_state_does_not_bypass_file_protection(daemon):
    source = str(daemon.daemon.home / 'config.toml')
    reads = []
    def worker(token):
        reads.append(tool(daemon.sock, token, 'fs.read', path=source))
        return done('The source was protected.')
    install(daemon, [sup('task', 'Inspecting.', task('Inspect selected file'))], {'worker': worker})
    outcome(send(daemon, 'Inspect the attached file', [source]))
    assert reads[0]['is_error']


def test_known_secret_never_enters_attachment_metadata_or_ordinary_chat(daemon):
    daemon.call_soon(daemon.daemon._add_secret_value, 'hidden-canary-314159')
    script = install(daemon, [sup('reply', 'Received your redacted message.')])
    with pytest.raises(RpcError, match='protected secret text'):
        send(daemon, 'Inspect this file', ['hidden-canary-314159.txt'])
    assert not script.contexts
    send(daemon, 'My code is hidden-canary-314159', [])
    assert 'hidden-canary-314159' not in str(script.contexts)


def test_status_and_trace_recover_recorded_artifacts_after_result_was_shown(daemon):
    artifact = str(daemon.daemon.home / 'workspace' / 'report.md')
    worker = json.dumps({'status': 'done', 'summary': 'Report ready.', 'artifacts': [artifact], 'evidence': []})
    script = install(daemon, [sup('task', 'Preparing report.', task('Prepare report'))], {'worker': worker})
    result = outcome(chat(daemon, 'Prepare my report'))
    async def unseen():
        daemon.daemon.db.conn.execute('UPDATE task_results SET shown=0 WHERE task_id=?', (result['task_id'],))
    daemon.run(unseen())
    client = Client(daemon.sock)
    try:
        status = client.call('chat.status', {'peek': True})
        assert status['results'][0]['artifacts'] == [artifact]
        assert status['results'][0]['answer'] == result['answer']
        assert client.call('chat.status', {'peek': True})['results'] == status['results']
        client.call('chat.status')
        assert client.call('chat.status')['results'] == []
        before = len(script.contexts)
        for _ in range(2):
            recovered = client.call('trace.get', {'task_id': result['task_id']})['outcome']['result']
            assert recovered['artifacts'] == [artifact] and recovered['answer'] == result['answer']
            assert recovered['status'] == result['status']
        assert len(script.contexts) == before
    finally:
        client.close()


def test_status_associates_hidden_prompt_with_task_and_answer_stays_private(daemon):
    grant = grant_task(daemon, 'frontend-login', Identity.OWNER, {'owner:ask'}, 'Sign in')
    async def pending():
        record = daemon.daemon.tasks.get(grant.task_id)
        action = ActionRequest('owner.login', 'owner_login_code', Identity.OWNER, frozenset(),
                               grant.task_id, grant.worker_id, record.task.request_id, frozenset())
        approval = daemon.daemon.state.approvals.question(action, 'Enter the login code', [])
        approval.kind = 'owner_login_code'
        return approval.id
    approval_id = daemon.run(pending())
    client = Client(daemon.sock)
    try:
        prompt = client.call('chat.status', {'peek': True})['approvals'][0]
        assert prompt['task_id'] == grant.task_id and prompt['kind'] == 'owner_login_code'
        response = client.call('chat.answer', {'approval_id': approval_id, 'text': '314159'})
        assert response['state'] == 'granted' and '314159' not in str(response)
        assert not client.call('chat.status', {'peek': True})['approvals']
    finally:
        client.close()
    async def recorded():
        return str([tuple(row) for row in daemon.daemon.db.conn.execute('SELECT data_json FROM journal')])
    assert '314159' not in daemon.run(recorded())
