"""Short visible replies retain inspectable details and verified claims."""
import json

from ferb.orchestrator.reply_stream import ReplyDraft
from ferb.sanitize.redact import Redactor
from tests.conftest import chat, install, outcome, sup, task
from ferb.orchestrator.schemas import SUPERVISOR_SCHEMA, SupervisorOutcome


def document(text, **changes):
    values = SupervisorOutcome(kind="reply", text=text).model_dump() | changes
    return json.dumps({key: values[key] for key in SUPERVISOR_SCHEMA["properties"]})


def words(events):
    return "".join(e["text"] for e in events if e["type"] == "reply_delta")


def test_only_short_display_streams_while_full_reply_is_retained(daemon):
    full = 'The local coding-agent session analysis task scans your CLI logs and builds a report.'
    short = 'Your coding-agent session review.'
    raw = document(full, display_text=short)
    events = []
    draft = ReplyDraft(Redactor(), events.append)
    for char in raw:
        draft.feed(char)
    draft.finish(raw)
    assert words(events) == short
    assert full not in words(events)
    install(daemon, [raw])
    final = chat(daemon, "What's it?")[-1]
    assert final['text'] == full
    assert final['display_text'] == short


def test_task_display_is_checked_and_full_answer_survives(daemon):
    full = 'The report is at /tmp/session-review/index.html. The Codex session had working artifacts and documented limits.'
    short = 'Report ready: /tmp/session-review/index.html. Best candidate: the Codex session.'
    worker = json.dumps({'status': 'done', 'summary': full, 'answer': full, 'display_text': short,
                         'artifacts': [], 'evidence': []})
    script = install(daemon, [sup('task', 'Reviewing.', task('review coding sessions'))], {'worker': worker})
    final = outcome(chat(daemon, 'review coding sessions'))
    assert final['answer'] == full and final['verified']          # the check mark is the flag, not a prefix (L11)
    assert final['display_text'] == short
    checked = next(ctx for _, key, ctx, _ in script.contexts if key == 'worker.verify')
    assert short in checked and full in checked
    async def read_stored():
        return daemon.daemon.tasks.rows(final['task_id'])[0]
    stored = daemon.run(read_stored())
    assert json.loads(stored['outcome_json'])['display_text'] == short


def test_unverified_display_cannot_reuse_worker_success_claim(daemon):
    worker = json.dumps({'status': 'done', 'summary': 'Report prepared.', 'answer': 'Everything is ready.',
                         'display_text': 'Done!', 'artifacts': [], 'evidence': []})
    check = json.dumps({'results': [{'index': 0, 'met': False, 'evidence': 'The requested report is missing.'}]})
    install(daemon, [sup('task', 'Reviewing.', task('review coding sessions'))],
            {'worker': worker, 'worker.verify': check})
    final = outcome(chat(daemon, 'review coding sessions'))
    assert final['status'] == 'failed'
    assert final['display_text'] != 'Done!' and 'Done!' not in final['display_text']
    assert final['display_text'].startswith("I couldn't confirm the task was complete:")   # code's status label
    assert 'missing' not in final['display_text'] and 'missing' in final['answer']        # findings: Details (L11)


def test_question_projection_is_separate_from_authoritative_reason(daemon):
    from ferb.cli.client import Client
    from tests.conftest import done, tool

    full = 'Which session should I submit: Codex Amazon ML or Claude UI? The submission shares the selected logs.'
    display = 'Share the Codex Amazon ML logs or the Claude UI logs?'

    def worker(token):
        response = tool(daemon.sock, token, 'owner.ask', question=full, options=['Codex', 'Claude'])
        return done(response['answer'])

    install(daemon, [sup('task', 'Reviewing.', task('choose a session'))], {'worker': worker,
        'worker.question': json.dumps({'ask_owner': True, 'question': full, 'answer': '', 'display_text': display})})
    client = Client(daemon.sock)
    events = client.stream('chat.send', {'text': 'choose a session', 'session_id': 'presentation-question'})
    try:
        for event in events:
            if event.get('topic') != 'approval.requested':
                continue
            data = event['data']
            assert display in data['display_text'] and full in data['reason']
            status_client = Client(daemon.sock)
            try:
                status = status_client.call('chat.status', {'peek': True})
                ap = next(a for a in status['approvals'] if a['approval_id'] == data['approval_id'])
                assert ap['display_text'] == data['display_text'] and ap['reason'] == data['reason']
                status_client.call('chat.answer', {'approval_id': data['approval_id'], 'text': '1'})
            finally:
                status_client.close()
        assert event['type'] == 'outcome'
    finally:
        client.close()


def test_failed_report_has_distinct_blocker_display_and_complete_details(daemon):
    full = 'The report generator cannot access the requested Claude session logs. The Codex inventory is complete.'
    short = 'I found the Codex sessions, but need access to your Claude logs to finish the comparison.'
    worker = json.dumps({'status': 'blocked', 'summary': full, 'answer': full,
                         'artifacts': [], 'evidence': ['Claude log source unavailable']})
    dispatch = json.dumps({'action': 'finish', 'worker_type': 'default', 'question': '',
                           'instructions': full, 'stop_reason': full, 'display_text': short, 'result': 'failed'})
    install(daemon, [sup('task', 'Reviewing.', task('review coding sessions'))],
            {'worker': worker, 'worker.dispatch': dispatch})
    final = outcome(chat(daemon, 'review coding sessions'))
    assert final['display_text'] == "I couldn't finish this task.\n" + short       # code's label, the model's words
    assert full in final['answer']
    assert 'Still to do:' in final['answer']
    assert 'Still to do:' not in final['display_text']


def test_cancellation_cannot_show_stale_recovery_question():
    from ferb.orchestrator.presentation import outcome_display
    assert outcome_display('cancelled', 'Stopped, as you asked.', '', "I couldn't finish this task.",
                           'Which logs should I use?', ['Choose a source?']) == 'Stopped, as you asked.'


def test_cli_uses_same_visible_text_and_keeps_verbose_details():
    from ferb.cli.chat import render
    event = {'type': 'reply', 'text': 'The complete detailed explanation.', 'display_text': 'The direct answer.'}
    assert render(event) == 'ferb> The direct answer.'
    assert render(event, verbose=True) == 'ferb> The complete detailed explanation.'


def test_late_stop_overrides_recovery_display(daemon):
    from ferb.orchestrator.supervisor import STOPPED_BY_YOU
    worker = json.dumps({'status': 'blocked', 'summary': 'Source missing.', 'artifacts': [], 'evidence': []})
    recovery = json.dumps({'action': 'ask', 'worker_type': 'default', 'question': 'Which source?',
                           'instructions': '', 'display_text': 'Choose your source.'})
    install(daemon, [sup('task', 'Reviewing.', task('review coding sessions'))],
            {'worker': worker, 'worker.dispatch': recovery})
    supervisor = daemon.daemon.supervisor
    async def stopped(task_id):
        supervisor._tree_of[task_id].stop = STOPPED_BY_YOU
    supervisor.on_task_finish = stopped
    result = outcome(chat(daemon, 'review coding sessions'))
    assert result['status'] == 'cancelled'
    assert result['display_text'] == result['answer'] == 'Stopped, as you asked.'


def test_changed_cleanup_report_invalidates_prior_display(daemon, monkeypatch):
    from ferb.core.types import Identity, Outcome, Task
    from ferb.orchestrator.schemas import TaskSpec
    from ferb.orchestrator.supervisor import _Tree
    supervisor = daemon.daemon.supervisor
    worker = Task('presentation.worker', None, 'review', 'device', Identity.FERB, set(), 'test', 'request', [], None)
    tree = _Tree('presentation', TaskSpec(goal='review', identity='ferb'), worker)
    tree.outcome = Outcome(worker.id, 'done', 'Report ready.', [], [])
    tree.checked_display = 'Ready!'
    tree.recovery_display = 'All fixed.'
    monkeypatch.setattr(supervisor, '_context', lambda tree: [])
    monkeypatch.setattr(supervisor, 'make_runtime', lambda *args, **kwargs: object())
    async def report(*args, **kwargs):
        return json.dumps({'status': 'failed', 'summary': 'Report was removed during cleanup.', 'artifacts': [], 'evidence': []})
    monkeypatch.setattr(supervisor, '_run', report)
    async def reconcile():
        parent = supervisor.spans.start('test.cleanup', input={})
        await supervisor._reconcile_resources(tree, parent.id)
    daemon.run(reconcile())
    assert tree.outcome.status == 'failed'
    assert not tree.checked_display and not tree.recovery_display


def test_dispatch_cannot_remove_or_replace_code_s_status_line():
    """L11: the short text of an unfinished task is the orchestrator's, after code's status label. Whatever it says,
    the label (from the check's verdict) leads, and the findings stay in the full answer; code doesn't judge the
    wording (rule 6)."""
    from ferb.orchestrator.presentation import outcome_display
    label = "I couldn't confirm the task was complete:"
    visible = outcome_display('failed', label + '\n- Payment completes — failed: Payment did not complete.', '', label,
                              'The payment page timed out; nothing was charged that I can see.')
    assert visible.split('\n')[0] == label
    assert 'Payment did not complete.' not in visible
    assert outcome_display('failed', 'Full failure report.', 'Done!', label, '') == label     # never a success claim
