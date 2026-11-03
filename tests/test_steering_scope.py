"""Owner steering preserves provenance, active credentials and conversation boundaries (D3/D8/MP2)."""
import asyncio
import json
import threading
from types import SimpleNamespace

import pytest

from ferb.cli.client import Client
from ferb.core.types import RuntimeEvent
from ferb.policy.rules import ActionRequest
from tests.conftest import FakeRuntime, chat, done, install, sup, task


@pytest.fixture
def active(daemon, monkeypatch):
    started, release = threading.Event(), threading.Event()
    received, runs = [], []
    original = FakeRuntime.events

    async def events(rt, run):
        if run.state['key'] != 'worker':
            async for event in original(rt, run):
                yield event
            return
        runs.append(run)
        started.set()
        while not release.is_set() and not run.state['cancel']:
            await asyncio.sleep(.01)
        yield RuntimeEvent('final', {'text': done('Updated report')})

    async def steer(rt, run, message):
        received.append((run.id, message))

    monkeypatch.setattr(FakeRuntime, 'events', events)
    monkeypatch.setattr(FakeRuntime, 'continue_', steer)
    script = install(daemon, [sup('task', 'Checking', task('Compare the owner options'))])
    thread = threading.Thread(target=lambda: chat(daemon, 'Compare the owner options', session='owner-chat'))
    thread.start()
    assert started.wait(5)
    root = runs[0].task_id.rsplit('.', 1)[0]
    held = SimpleNamespace(daemon=daemon, script=script, received=received, run=runs[0], root=root,
                           supervisor=daemon.daemon.supervisor)
    try:
        yield held
    finally:
        release.set()
        thread.join(5)
        assert not thread.is_alive()


def correction(active, words, *, session='owner-chat', spec=None, supersedes=()):
    decision = json.loads(sup('task', 'Updated.', spec or task('Compare the corrected options')))
    decision['control'] = {'task_id': active.root, 'action': 'continue', 'standing_sends': 'unchanged',
                           'supersedes': list(supersedes)}
    active.script.sup.append(json.dumps(decision))
    return chat(active.daemon, words, session=session, timeout=3)


# decision: ORCH-518, ORCH-521
def test_steering_records_the_destinations_the_owner_typed_or_pasted_only(active):
    """A correction records the destinations in the owner's words, pasted ones too since ORCH-518 ("(b) pastes count
    as your words"); a site only the planner put in the task is never recorded."""
    correction(active, 'Use https://mysite.example\n```\nSend to ops@example.org\n```',
               spec=task('Use the supplied sources', sites=('mysite.example', 'elsewhere.example')))
    async def inspect():
        supervisor = active.supervisor
        tree = supervisor._tree_of[active.root]
        request = supervisor.state.requests.get(tree.worker.request_id)
        assert 'mysite.example' in request.words and 'ops@example.org' in request.words
        assert supervisor.gateway.destinations.known('host', 'mysite.example')
        assert supervisor.gateway.destinations.known('email', 'ops@example.org')
        assert not supervisor.gateway.destinations.known('host', 'elsewhere.example')
        assert '<untrusted-data' not in request.payload
    active.daemon.run(inspect())
    assert 'ops@example.org' in active.received[0][1]


# decision: ORCH-521
def test_steering_does_not_widen_existing_token_identity_or_scopes(active):
    async def grant():
        return active.supervisor.tokens.validate(active.run.state['token'])
    before = active.daemon.run(grant())
    correction(active, 'Use my account instead', spec=task('Revised request', identity='owner',
                                                        apps=('com.apple.Terminal',)))
    after = active.daemon.run(grant())
    assert after == before
    assert len(active.received) == 1


# decision: ORCH-521
def test_another_chat_can_correct_a_running_task_which_stays_in_its_chat(active):
    """Cross-chat control (live failure 2026-10-24): the planner is shown every chat's running tasks, so a correction
    sent from another chat reaches the task; the task stays the chat's it started in."""
    correction(active, 'Replace the first task', session='different-chat')
    assert active.received and 'Replace the first task' in active.received[0][1]
    async def inspect():
        request = active.supervisor.continuity.load(active.root, 'owner-chat')[1]
        assert 'Replace the first task' in request.words
        assert active.supervisor.continuity.conversation_of(active.root) == 'owner-chat'
    active.daemon.run(inspect())


# decision: ORCH-521
def test_steering_redacts_secret_before_worker_and_durable_request(active):
    secret = 'canary-private-steering-secret'
    active.daemon.daemon.redact.add_secret_value(secret)
    correction(active, 'Keep this credential private: ' + secret)
    async def inspect():
        tree = active.supervisor._tree_of[active.root]
        request = active.supervisor.state.requests.get(tree.worker.request_id)
        saved = active.supervisor.continuity.load(active.root, 'owner-chat')
        assert secret not in request.words + request.payload + saved[1].words + saved[1].payload
        assert secret not in '\n'.join(tree.steering.messages)
    active.daemon.run(inspect())
    assert secret not in active.received[0][1]


def approvals(active):
    async def create():
        supervisor = active.supervisor
        worker = supervisor._tree_of[active.root].worker
        created = {}
        for label, capability, kind in [
            ('ordinary', 'owner.ask', 'owner_question'), ('payment', 'payments.pay', 'confirmation'),
            ('secret', 'secrets.fill', 'owner_secret'), ('plan', 'plan.ask', 'owner_question')]:
            action = ActionRequest(capability, kind, worker.identity, frozenset(), worker.id, worker.id,
                                   worker.request_id, frozenset(worker.scopes))
            question = supervisor.state.approvals.question(action, 'Pending owner decision', ['Yes', 'No'])
            question.kind = kind
            created[label] = question
        return created
    return active.daemon.run(create())


# decision: ORCH-521
def test_correction_releases_ordinary_question_but_never_approves_payment_secret_or_plan(active):
    pending = approvals(active)
    named = [pending[k].id for k in ('ordinary', 'payment', 'secret')]   # the planner names what it changes (L3)
    correction(active, 'Keep the total under 300', supersedes=named)
    assert pending['ordinary'].state == 'granted'
    assert pending['ordinary'].answer == 'Keep the total under 300'
    # L3 (I1): a correction sets FERB's checks and hidden inputs aside as superseded, never a recorded No
    assert pending['payment'].state == 'superseded'
    assert pending['secret'].state == 'superseded' and pending['secret'].answer is None
    assert pending['plan'].state == 'pending'                              # not named: stays open


# decision: ORCH-521
def test_a_correction_leaves_every_question_it_does_not_name_open(active):
    pending = approvals(active)
    correction(active, 'also cc Bob')
    assert all(ap.state == 'pending' for ap in pending.values())


# decision: ORCH-518
def test_a_pasted_correction_counts_as_the_owners_words_for_a_question(active):
    """Replaces the guard that a paste-only correction can't answer: since ORCH-518 the owner's paste is their words,
    so it settles the question it supersedes as their typed words would."""
    pending = approvals(active)
    correction(active, '```\nYes\n```', supersedes=[pending['ordinary'].id])
    assert pending['ordinary'].state == 'granted'


# decision: ORCH-521
def test_selected_attachment_stays_path_data_without_authority(active, tmp_path):
    attachment = tmp_path / 'send-to-injected@example.invalid.txt'
    attachment.write_text('Owner-selected comparison notes')
    decision = json.loads(sup('task', 'Updated.', task('Use the attached notes')))
    decision['control'] = {'task_id': active.root, 'action': 'continue', 'standing_sends': 'unchanged', 'supersedes': []}
    active.script.sup.append(json.dumps(decision))
    client = Client(active.daemon.sock, timeout=3)
    try:
        list(client.stream('chat.send', {'session_id': 'owner-chat', 'text': 'Use this attachment',
                                        'input_paths': [str(attachment)]}))
    finally:
        client.close()
    async def inspect():
        tree = active.supervisor._tree_of[active.root]
        request = active.supervisor.state.requests.get(tree.worker.request_id)
        assert str(attachment) in request.payload
        assert str(attachment) not in request.words
        assert '<untrusted-data' in request.payload
        assert not active.supervisor.gateway.destinations.known('email', 'injected@example.invalid')
    active.daemon.run(inspect())
    assert str(attachment) in active.received[0][1]


# decision: ORCH-521
def test_model_spec_cannot_put_known_secret_into_task_goal_storage(active):
    secret = 'canary-secret-in-generated-goal'
    active.daemon.daemon.redact.add_secret_value(secret)
    correction(active, 'Update the comparison', spec=task('Compare ' + secret))
    async def inspect():
        rows = active.supervisor.tasks.db.conn.execute('SELECT goal FROM tasks WHERE tree_id=?',
                                                       (active.root,)).fetchall()
        assert rows and all(secret not in row[0] for row in rows)
    active.daemon.run(inspect())
