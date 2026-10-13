"""L11: answers in the owner's words, from the traced cases in the 2026-10-13 review (traces.md §4).

- 161aa070 / 1b79b873: an unfinished task's short text led with checker internals and dropped the report path;
- ef9e5d75: "too linkedinish" reopened the old goal through the saved plan instead of running the rewrite;
- 20:10: a refused timer replaced the whole reply, and the 19:52 boarding timer was refused because it was asked for
  in an earlier message of the same chat;
- steering replied "Updated the running task." without saying what changed;
- the full answer names the sites the task reached (L2's list)."""
import asyncio
import json
import threading
from pathlib import Path
from typing import Any

import pytest

from ferb.act.shell.manager import ProcInfo
from ferb.core.types import RuntimeEvent
from ferb.orchestrator.plans import Plan, Step
from tests.conftest import FakeRuntime, RunningDaemon, chat, cond, done, install, outcome, sup, task

FINDING = ('Recorded step results state that before your correction FERB used the existing playing YouTube tab '
           'instead of opening a new one.')
HOW = 'Compare the tab ids in the browser records.'
REPORT = '/tmp/ferb-l11/report.html'


def _worker(answer: str, display: str, artifacts: tuple[str, ...] = ()) -> str:
    return json.dumps({'status': 'done', 'summary': answer, 'answer': answer, 'display_text': display,
                       'artifacts': list(artifacts), 'evidence': []})


def _dispatch(action: str, display: str, **fields: Any) -> str:
    return json.dumps({'action': action, 'worker_type': 'default', 'question': fields.get('question', ''),
                       'instructions': fields.get('instructions', ''), 'stop_reason': fields.get('stop_reason', ''),
                       'display_text': display, 'result': fields.get('result')})


def _unconfirmed(met: bool | None) -> str:
    return json.dumps({'results': [{'index': 0, 'met': met, 'evidence': FINDING, 'how': HOW if met is None else None}]})


# ---------- (a) the short text of an unfinished task ----------

def test_needs_user_short_text_is_the_label_then_the_model_s_words_not_findings(daemon: RunningDaemon) -> None:
    """1b79b873's shape: the check couldn't confirm a condition and the orchestrator asks the owner."""
    short = (f'Your report is ready at {REPORT}. I couldn\'t tell which YouTube tab played the video; '
             'was it the one you already had open?')
    install(daemon, [sup('task', 'Writing it up.', task('write the YouTube report'))], {
        'worker': _worker(f'The report is at {REPORT}.', 'Report ready.', (REPORT,)),
        'worker.verify': _unconfirmed(None),
        'worker.dispatch': _dispatch('ask', short, question='Which YouTube tab played the video: yours or a new one?')})
    final = outcome(chat(daemon, 'write the YouTube report'))
    assert final['status'] == 'needs_user'
    assert final['display_text'] == "I couldn't confirm the task was complete:\n" + short
    assert FINDING not in final['display_text'] and HOW not in final['display_text']
    assert FINDING in final['answer'] and REPORT in final['answer']        # the findings stay under Details


def test_failed_short_text_keeps_the_deliverable_and_the_findings_stay_in_details(daemon: RunningDaemon) -> None:
    """161aa070's shape: the report was produced, the task failed on one condition, and the short text led with
    checker internals and left out the report's path."""
    short = f'Your report is at {REPORT}, but it used the YouTube tab you already had open, not a new one.'
    install(daemon, [sup('task', 'Writing it up.', task('write the YouTube report'))], {
        'worker': _worker(f'The report is at {REPORT}.', 'Report ready.', (REPORT,)),
        'worker.verify': _unconfirmed(False),
        'worker.dispatch': _dispatch('finish', short, instructions='Stop here.', stop_reason='A new tab was required.',
                                     result='failed')})
    final = outcome(chat(daemon, 'write the YouTube report'))
    assert final['status'] == 'failed'
    assert final['display_text'] == "I couldn't confirm the task was complete:\n" + short
    assert FINDING not in final['display_text']
    assert FINDING in final['answer']


def test_without_the_model_s_short_text_code_shows_the_label_and_the_files_never_findings(
        daemon: RunningDaemon) -> None:
    """When no answer-writing model wrote a short text (here it wrote none), code still never fills the gap with the
    checker's findings: the status label, the reason and the files it recorded."""
    install(daemon, [sup('task', 'Writing it up.', task('write the YouTube report'))], {
        'worker': _worker(f'The report is at {REPORT}.', 'Done!', (REPORT,)),
        'worker.verify': _unconfirmed(False),
        'worker.dispatch': _dispatch('finish', '', instructions='Stop here.', result='failed')})
    final = outcome(chat(daemon, 'write the YouTube report'))
    assert final['status'] == 'failed'
    assert final['display_text'].startswith("I couldn't confirm the task was complete:")
    assert REPORT in final['display_text']
    assert FINDING not in final['display_text'] and 'Done!' not in final['display_text']


# ---------- (b) verified answers carry the flag, not a prefix ----------

def test_a_verified_answer_has_the_flag_and_no_verified_prefix(daemon: RunningDaemon) -> None:
    full = f'The report is at {REPORT}. It covers all three sessions.'
    install(daemon, [sup('task', 'Writing it up.', task('write the report'))],
            {'worker': _worker(full, f'Report ready: {REPORT}.')})
    final = outcome(chat(daemon, 'write the report'))
    assert final['status'] == 'done' and final['verified'] is True
    assert final['answer'] == full
    assert not final['answer'].startswith('Verified') and not final['display_text'].startswith('Verified')


# ---------- (c) a follow-up with a new goal re-plans ----------

def test_continue_with_a_new_goal_runs_the_new_goal_not_the_saved_plan(daemon: RunningDaemon) -> None:
    """ef9e5d75: "this sounds too much linkedinish… make it sound interesting" came back as continue on the ended
    task with a new rewrite goal; FERB reopened the saved plan with the old goal and filled nothing new."""
    old = task('Fill the Origins field of the application')
    old['plan'] = Plan(steps=[Step(id='fill', kind='do', goal='Fill the Origins field')]).model_dump()
    first_script = install(daemon, [sup('task', 'Filling it in.', old)],
                           {'step.fill': done('Filled the Origins field.'), 'worker': done('Origins field filled.')})
    first = outcome(chat(daemon, 'fill the Origins field', session='origins'))
    assert sum(key == 'step.fill' for _, key, _, _ in first_script.contexts) == 1

    new_goal = 'Rewrite the Origins field so it sounds interesting, not like a LinkedIn post'
    follow_up = json.loads(sup('task', 'Rewriting it.', task(new_goal)))
    follow_up['control'] = {'task_id': first['task_id'], 'action': 'continue'}
    second_script = install(daemon, [json.dumps(follow_up)], {'worker': done('Rewrote the Origins field.')})
    second = outcome(chat(daemon, 'this sounds too much linkedinish, make it sound interesting', session='origins'))
    assert second['task_id'] != first['task_id']
    assert second['goal'].startswith(new_goal)
    worker = next(ctx for _, key, ctx, _ in second_script.contexts if key == 'worker')
    assert 'Task: ' + new_goal in worker
    assert not any(key == 'step.fill' for _, key, _, _ in second_script.contexts)


# ---------- (d) a refused side output keeps the reply ----------

def test_a_refused_timer_keeps_the_reply_and_adds_one_line(daemon: RunningDaemon) -> None:
    """20:10: the reply answered the question and also set a timer; code refused the timer and replaced the whole
    reply with "I couldn't set that reminder…"."""
    answer = 'For Codex Cloud, ask it to reverse-engineer the setup from the repository first.'
    install(daemon, [json.dumps({'kind': 'reply', 'text': answer, 'display_text': answer, 'remember': [],
                                 'timers': [{'when': _in_minutes(10), 'message': 'boarding starts',
                                             'quote': 'words the owner never said'}]})])
    events = chat(daemon, 'how should I start with Codex Cloud?', session='boarding')
    reply = next(e for e in events if e['type'] == 'reply')
    for field in ('text', 'display_text'):
        assert reply[field].startswith(answer)
        extra = reply[field][len(answer):].strip()
        assert extra and '\n' not in extra and 'boarding starts' in extra       # one line names the refused part
    assert not _pending(daemon)


def test_a_refused_memory_write_keeps_the_reply_and_adds_one_line(daemon: RunningDaemon) -> None:
    answer = 'Your dentist appointment is on Friday at 10.'
    install(daemon, [json.dumps({'kind': 'reply', 'text': answer, 'remember': [
        {'kind': 'person', 'subject': 'owner', 'key': 'dentist', 'content': 'Your dentist is Dr. Rao',
         'quote': 'words the owner never said'}], 'timers': []})])
    reply = next(e for e in chat(daemon, 'when is my dentist appointment?') if e['type'] == 'reply')
    assert reply['text'].startswith(answer)
    extra = reply['text'][len(answer):].strip()
    assert extra and '\n' not in extra and 'Your dentist is Dr. Rao' in extra


def test_a_refused_timer_on_a_task_turn_is_told_in_one_line(daemon: RunningDaemon) -> None:
    """A task's turn has no reply to add the line to: it arrives on its own, before the plan."""
    plan = json.loads(sup('task', 'Checking.', task('check the gate number')))
    plan['timers'] = [{'when': _in_minutes(10), 'message': 'boarding starts', 'quote': 'words the owner never said'}]
    install(daemon, [json.dumps(plan)], {'worker': done('Gate B12.')})
    events = chat(daemon, 'check the gate number', session='gate')
    told = [e['text'] for e in events if e['type'] == 'timer']
    assert len(told) == 1 and 'boarding starts' in told[0] and told[0].startswith("I couldn't set the reminder")
    assert outcome(events)['status'] == 'done' and not _pending(daemon)


# ---------- (e) reminder provenance: the owner's words in this chat ----------

def test_a_reminder_asked_for_in_an_earlier_message_of_the_same_chat_is_set(daemon: RunningDaemon) -> None:
    """19:52 asked for the boarding timer; the reply was lost, and at 20:10 the planner set it from that message."""
    asked = 'set a timer for 8:20, the boarding starts at 8.25'
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Noted.', 'remember': [], 'timers': []})])
    chat(daemon, asked, session='boarding')
    timer = {'when': _in_minutes(10), 'message': 'boarding starts at 8.25', 'quote': 'set a timer for 8:20'}
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Timer set.', 'remember': [], 'timers': [timer]})])
    events = chat(daemon, 'did you set it?', session='boarding')
    assert [e['text'] for e in events if e['type'] == 'timer'][0].startswith('⏰ reminder')
    assert [t.message for t in _pending(daemon)] == ['boarding starts at 8.25']


def test_reminder_provenance_stays_in_this_chat_and_never_counts_pasted_text(daemon: RunningDaemon) -> None:
    pasted = '\x1b[200~remind me at nine to pay the vendor\x1b[201~'
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Noted.', 'remember': [], 'timers': []})] * 2)
    chat(daemon, 'set a timer for the boarding call', session='other-chat')
    chat(daemon, 'what does this say? ' + pasted, session='boarding')
    for quote in ('set a timer for the boarding call', 'remind me at nine to pay the vendor'):
        install(daemon, [json.dumps({'kind': 'reply', 'text': 'Okay.', 'remember': [],
                                     'timers': [{'when': _in_minutes(10), 'message': 'x', 'quote': quote}]})])
        chat(daemon, 'go ahead', session='boarding')
    assert not _pending(daemon)


# ---------- (f) steering replies name what changed ----------

def test_a_steer_reply_names_the_new_goal(daemon: RunningDaemon, monkeypatch: Any) -> None:
    started, release = threading.Event(), threading.Event()
    handles: list[Any] = []
    original_events = FakeRuntime.events

    async def events(rt: Any, run: Any) -> Any:
        if run.state['key'] != 'worker':
            async for event in original_events(rt, run):
                yield event
            return
        handles.append(run)
        started.set()
        while not release.is_set() and not run.state['cancel']:
            await asyncio.sleep(.01)
        yield RuntimeEvent('final', {'text': done('Made it a web page.')})

    async def steer(rt: Any, run: Any, message: str) -> None:
        return None

    monkeypatch.setattr(FakeRuntime, 'events', events)
    monkeypatch.setattr(FakeRuntime, 'continue_', steer)
    script = install(daemon, [sup('task', 'Writing it.', task('Write the report as a PDF'))])
    initial: list[dict[str, Any]] = []
    thread = threading.Thread(target=lambda: initial.extend(chat(daemon, 'write the report', session='steer')))
    thread.start()
    try:
        assert started.wait(5)
        tree_id = handles[0].task_id.rsplit('.', 1)[0]
        update = json.loads(sup('task', 'Switching.', task('Make the report a web page',
                                                           conds=(cond('The report opens as a web page'),))))
        update['control'] = {'task_id': tree_id, 'action': 'steer'}
        script.sup.append(json.dumps(update))
        reply = next(e for e in chat(daemon, 'make it a web page', session='steer', timeout=5) if e['type'] == 'reply')
        assert reply['text'] == 'Switched to: Make the report a web page'
    finally:
        release.set()
        thread.join(5)
    assert not thread.is_alive()


# ---------- (g) the sites the task reached ----------

def test_the_full_answer_ends_with_the_sites_contacted(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    short = 'Latest release: 2.4.1.'

    def worker(token: str) -> str:
        grant = d.tokens.validate(token)
        info = ProcInfo('p-1', grant.task_id, 'curl', str(d.home), 0.0, False, 0, False)
        d._shell_network(info, {'destinations': [{'host': h, 'port': 443, 'bytes_out': 1, 'bytes_in': 1,
                                                  'connections': 1} for h in ('pypi.org', 'GitHub.com.')],
                                'refused': []})
        return _worker('The latest release is 2.4.1, published yesterday.', short)

    install(daemon, [sup('task', 'Checking.', task('find the latest release'))], {'worker': worker})
    final = outcome(chat(daemon, 'find the latest release'))
    assert final['answer'].endswith('\n\nSites contacted: github.com, pypi.org')
    assert final['display_text'] == short


def test_no_sites_line_when_the_task_reached_none(daemon: RunningDaemon) -> None:
    install(daemon, [sup('task', 'Checking.', task('count the files'))], {'worker': _worker('12 files.', '12 files.')})
    assert 'Sites contacted' not in outcome(chat(daemon, 'count the files'))['answer']


# ---------- the answer prompts ----------

PROMPTS = Path(__file__).resolve().parents[1] / 'ferb' / 'orchestrator' / 'prompts'


def _prompt(name: str) -> str:
    return ' '.join((PROMPTS / name).read_text().split())


@pytest.mark.parametrize('phrase', ['Looking at your storage now.', 'Your coding-agent session review.',
                                    'Still scanning your session logs', 'your Brave Work profile', 'Style examples',
                                    'Saved [deliverable link]'])
def test_the_prompts_carry_no_copyable_example_phrasing(phrase: str) -> None:
    """Models copy example sentences (and the "Brave Work" profile example matched a profile workers kept guessing)."""
    assert not any(phrase in _prompt(path.name) for path in PROMPTS.glob('*.md'))


@pytest.mark.parametrize('name, words', [
    ('interaction.md', "The short text answers the owner's question as they asked it, read in the conversation's"),
    ('interaction.md', "It keeps the fact the conversation is about"),
    ('interaction.md', "Never use FERB's internal terms in anything the owner reads"),
    ('dispatch.md', 'what was delivered, with its paths and links;'),
    ('dispatch.md', 'The complete findings stay in the full answer under Details: never paste them here.'),
    ('worker.md', "It answers the owner's question as they asked it, in the conversation's context"),
    ('supervisor.md', "`display_text` answers the owner's message as they asked it"),
])
def test_the_answer_prompts_require_the_question_as_asked_and_no_internal_terms(name: str, words: str) -> None:
    assert words in _prompt(name)


def _in_minutes(n: int) -> dict[str, Any]:
    return {'in_minutes': n, 'in_days': None, 'day': None, 'date': None, 'time': None}


def _pending(daemon: RunningDaemon) -> list[Any]:
    async def read() -> list[Any]:
        return list(daemon.daemon.timers.pending())
    return daemon.run(read())   # type: ignore[no-any-return]
