"""Answers show the full detailed text (owner, 2026-10-24: "the details response is better. remove the summarised
short response as of now"; amends P30's "short by default" and e14d691's concise `display_text`). The model writes
one owner answer, which streams and is shown; questions keep their separate display (P27). P31's evidence report
for the checker stays apart from the owner answer."""
import json
from pathlib import Path

import pytest

from ferb.orchestrator.reply_stream import ReplyDraft
from ferb.orchestrator.schemas import (DISPATCH_SCHEMA, QUESTION_SCHEMA, SUPERVISOR_SCHEMA, WORKER_SCHEMA,
                                       SupervisorOutcome)
from ferb.sanitize.redact import Redactor
from tests.conftest import chat, install, outcome, sup, task

PROMPTS = Path(__file__).resolve().parents[1] / 'ferb' / 'orchestrator' / 'prompts'


def document(text, **changes):
    values = SupervisorOutcome(kind="reply", text=text).model_dump() | changes
    return json.dumps({key: values[key] for key in SUPERVISOR_SCHEMA["properties"]})


def words(events):
    return "".join(e["text"] for e in events if e["type"] == "reply_delta")


def test_each_answer_schema_has_one_owner_answer_and_questions_keep_their_display():
    assert 'display_text' not in SUPERVISOR_SCHEMA['properties']
    assert list(SUPERVISOR_SCHEMA['properties'])[-1] == 'text'          # streamed last, after every action field
    assert 'display_text' not in WORKER_SCHEMA['properties'] and 'answer' in WORKER_SCHEMA['properties']
    assert 'summary' in WORKER_SCHEMA['properties']                     # P31: the checker's evidence report stays
    assert 'display_text' not in DISPATCH_SCHEMA['properties']
    assert 'display_text' in QUESTION_SCHEMA['properties']


def test_a_direct_reply_streams_and_shows_its_full_text(daemon):
    full = ('Your coding-agent session review scans the CLI logs from the last two weeks and builds a report.\n'
            'It is at the scanning stage; no report file exists yet.')
    raw = document(full)
    events = []
    draft = ReplyDraft(Redactor(), events.append)
    for char in raw:
        draft.feed(char)
    draft.finish(raw)
    assert words(events) == full and not any(e['type'] == 'reply_abort' for e in events)
    install(daemon, [raw])
    final = chat(daemon, "What's it?")[-1]
    assert final['type'] == 'reply' and final['text'] == full
    assert 'display_text' not in final
    async def recorded():
        return [e.data for e in daemon.daemon.journal.of_kind('chat.reply')]
    assert 'display_text' not in daemon.run(recorded())[-1]


def test_a_task_s_final_answer_is_the_full_answer(daemon):
    full = ('The report is at /tmp/session-review/index.html. The Codex session had working artifacts and '
            'documented limits; the Claude session stopped before its tests.')
    worker = json.dumps({'status': 'done', 'summary': 'Evidence: read 14 logs.', 'answer': full,
                         'artifacts': [], 'evidence': []})
    script = install(daemon, [sup('task', 'Reviewing.', task('review coding sessions'))], {'worker': worker})
    final = outcome(chat(daemon, 'review coding sessions'))
    assert final['answer'] == full and final['verified']
    assert 'display_text' not in final
    checked = next(ctx for _, key, ctx, _ in script.contexts if key == 'worker.verify')
    assert full in checked and 'Concise owner display' not in checked
    async def stored():
        return (daemon.daemon.tasks.rows(final['task_id'])[0],
                [e.data for e in daemon.daemon.journal.of_kind('task.answer')][-1])
    row, said = daemon.run(stored())
    assert 'display_text' not in json.loads(row['outcome_json']) and 'display_text' not in said
    assert said['text'] == full


def test_an_unfinished_answer_is_the_full_report_never_the_worker_s_claim(daemon):
    worker = json.dumps({'status': 'done', 'summary': 'Report prepared.', 'answer': 'Everything is ready.',
                         'artifacts': [], 'evidence': []})
    check = json.dumps({'results': [{'index': 0, 'met': False, 'evidence': 'The requested report is missing.'}]})
    install(daemon, [sup('task', 'Reviewing.', task('review coding sessions'))],
            {'worker': worker, 'worker.verify': check})
    final = outcome(chat(daemon, 'review coding sessions'))
    assert final['status'] == 'failed' and 'display_text' not in final
    assert final['answer'].startswith("I couldn't confirm the task was complete:")   # code's status line leads
    assert 'The requested report is missing.' in final['answer']                    # the findings are shown
    assert 'Everything is ready.' not in final['answer']


def test_a_failed_task_shows_why_it_stopped_and_what_is_left(daemon):
    reason = 'The report generator cannot access the requested Claude session logs. The Codex inventory is complete.'
    worker = json.dumps({'status': 'blocked', 'summary': reason, 'answer': reason,
                         'artifacts': [], 'evidence': ['Claude log source unavailable']})
    dispatch = json.dumps({'action': 'finish', 'worker_type': 'default', 'question': '',
                           'instructions': reason, 'stop_reason': reason, 'result': 'failed'})
    install(daemon, [sup('task', 'Reviewing.', task('review coding sessions'))],
            {'worker': worker, 'worker.dispatch': dispatch})
    final = outcome(chat(daemon, 'review coding sessions'))
    assert 'display_text' not in final
    assert final['answer'].startswith("I couldn't finish this task.")
    assert 'Why work stopped: ' + reason in final['answer'] and 'Still to do:' in final['answer']


def test_question_projection_is_separate_from_authoritative_reason(daemon):
    """Questions are unchanged: a routed question keeps its direct display and its complete reason."""
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


def test_a_late_stop_shows_what_the_worker_had_found(daemon):
    from ferb.orchestrator.supervisor import STOPPED_BY_YOU
    worker = json.dumps({'status': 'blocked', 'summary': 'Source missing.', 'artifacts': [], 'evidence': []})
    recovery = json.dumps({'action': 'ask', 'worker_type': 'default', 'question': 'Which source?',
                           'instructions': ''})
    install(daemon, [sup('task', 'Reviewing.', task('review coding sessions'))],
            {'worker': worker, 'worker.dispatch': recovery})
    supervisor = daemon.daemon.supervisor
    async def stopped(task_id):
        supervisor._tree_of[task_id].stop = STOPPED_BY_YOU
    supervisor.on_task_finish = stopped
    result = outcome(chat(daemon, 'review coding sessions'))
    assert result['status'] == 'cancelled' and 'display_text' not in result
    # owner live use, 2026-10-15: a stop keeps what the worker had found, labelled unchecked, never only the stop line;
    # never the recovery's stale question
    assert result['answer'] == "Stopped, as you asked.\n\nWhat I'd found before stopping (not checked): Source missing."


def test_a_stop_reports_what_the_worker_had_found(daemon):
    """The hotspot diagnosis stopped at 02:46 answered only "Stopped, as you asked.", dropping what the worker had
    already found."""
    from ferb.orchestrator.supervisor import STOPPED_BY_YOU
    worker = json.dumps({'status': 'blocked', 'summary': "The hotspot's DHCP lease expired at 02:30; the Mac kept a "
                         "stale address.", 'artifacts': [], 'evidence': []})
    recovery = json.dumps({'action': 'ask', 'worker_type': 'default', 'question': 'Renew it?', 'instructions': ''})
    install(daemon, [sup('task', 'Looking.', task('find why the hotspot dropped'))],
            {'worker': worker, 'worker.dispatch': recovery})
    supervisor = daemon.daemon.supervisor
    async def stopped(task_id):
        supervisor._tree_of[task_id].stop = STOPPED_BY_YOU
    supervisor.on_task_finish = stopped
    result = outcome(chat(daemon, 'find why the hotspot dropped'))
    assert result['status'] == 'cancelled' and result['answer'].startswith('Stopped, as you asked.')
    assert "What I'd found before stopping (not checked): The hotspot's DHCP lease expired" in result['answer']


def test_a_cleanup_review_that_lowers_the_status_keeps_its_own_words(daemon, monkeypatch):
    from ferb.core.types import Identity, Outcome, Task
    from ferb.orchestrator.schemas import TaskSpec
    from ferb.orchestrator.supervisor import _Tree
    supervisor = daemon.daemon.supervisor
    worker = Task('presentation.worker', None, 'review', 'device', Identity.FERB, set(), 'test', 'request', [], None)
    tree = _Tree('presentation', TaskSpec(goal='review', identity='ferb'), worker)
    tree.outcome = Outcome(worker.id, 'done', 'Report ready.', [], [])
    async def no_context(tree):
        return []
    monkeypatch.setattr(supervisor, '_context', no_context)
    monkeypatch.setattr(supervisor, 'make_runtime', lambda *args, **kwargs: object())
    async def report(*args, **kwargs):
        return json.dumps({'status': 'failed', 'summary': 'Report was removed during cleanup.', 'artifacts': [], 'evidence': []})
    monkeypatch.setattr(supervisor, '_run', report)
    async def reconcile():
        parent = supervisor.spans.start('test.cleanup', input={})
        await supervisor._reconcile_resources(tree, parent.id)
    daemon.run(reconcile())
    assert tree.outcome.status == 'failed' and tree.outcome.summary == 'Report was removed during cleanup.'


@pytest.mark.parametrize('name', ['supervisor.md', 'worker.md', 'dispatch.md', 'interaction.md', 'verifier.md'])
def test_answer_prompts_ask_for_no_separate_short_version(name):
    """The prompts are executable input: none asks for a concise display of an answer, or says it hides under
    Details. question.md still asks for a question's display."""
    text = ' '.join((PROMPTS / name).read_text().split())
    assert 'display_text' not in text and 'concise owner display' not in text.lower()
    assert 'answer in Details' not in text and 'available as Details' not in text


def test_the_question_prompt_keeps_its_display_rules():
    text = ' '.join((PROMPTS / 'question.md').read_text().split())
    assert 'write `display_text` as a direct choice or request for the missing input' in text


def test_the_planner_prompt_streams_text_last():
    text = ' '.join((PROMPTS / 'supervisor.md').read_text().split())
    assert 'Write the structured response in schema order, with `text` last.' in text
