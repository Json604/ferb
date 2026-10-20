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
from datetime import timedelta

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
    short = f'Your report is ready at {REPORT}. I couldn\'t tell which YouTube tab played the video.'
    question = 'Which YouTube tab played the video: yours or a new one?'
    install(daemon, [sup('task', 'Writing it up.', task('write the YouTube report'))], {
        'worker': _worker(f'The report is at {REPORT}.', 'Report ready.', (REPORT,)),
        'worker.verify': _unconfirmed(None),
        'worker.dispatch': _dispatch('ask', short, question=question)})
    final = outcome(chat(daemon, 'write the YouTube report'))
    assert final['status'] == 'needs_user'
    # code's label, the model's short text, then code's own line with the open question
    assert final['display_text'] == "I couldn't confirm the task was complete:\n" + short + '\nQuestion: ' + question
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


def test_without_a_short_text_or_files_the_label_reads_as_a_sentence(daemon: RunningDaemon) -> None:
    install(daemon, [sup('task', 'Writing it up.', task('write the YouTube report'))], {
        'worker': _worker('The report is written.', 'Done!'),
        'worker.verify': _unconfirmed(False),
        'worker.dispatch': _dispatch('finish', '', instructions='Stop here.', result='failed')})
    final = outcome(chat(daemon, 'write the YouTube report'))
    assert final['display_text'] == "I couldn't confirm the task was complete."


def test_without_a_short_text_code_shows_why_work_stopped(daemon: RunningDaemon) -> None:
    """The orchestrator couldn't choose what to do next: the reason code wrote is shown, not a bare label."""
    install(daemon, [sup('task', 'Booking.', task('book the table'))], {
        'worker': json.dumps({'status': 'blocked', 'summary': 'The booking page would not load.', 'artifacts': [],
                              'evidence': []}),
        'worker.dispatch': 'not a decision'})
    final = outcome(chat(daemon, 'book the table'))
    assert final['status'] == 'failed'
    assert final['display_text'].startswith("I couldn't finish this task.\n")
    assert 'The booking page would not load.' in final['display_text']


def test_a_worker_report_failure_never_shows_an_exception_name(daemon: RunningDaemon) -> None:
    install(daemon, [sup('task', 'Booking.', task('book the table'))], {'worker': 'not a report'})
    final = outcome(chat(daemon, 'book the table'))
    assert final['status'] == 'failed'
    for name in ('JSONDecodeError', 'ValueError', 'ValidationError', 'runtime'):
        assert name not in final['display_text'] and name not in final['answer']


def test_a_runtime_stop_is_told_in_plain_words(daemon: RunningDaemon) -> None:
    install(daemon, [sup('task', 'Booking.', task('book the table'))],
            {'worker': RuntimeEvent('violation', {'unexpected_tools': ['mcpToolCall']})})
    final = outcome(chat(daemon, 'book the table'))
    for text in (final['display_text'], final['answer']):
        assert 'runtime' not in text and 'unexpected_tools' not in text and 'mcpToolCall' not in text


def test_a_failed_check_keeps_its_failure_for_the_orchestrator(daemon: RunningDaemon) -> None:
    """The checker's own failure is worded as a check, and its exact detail still reaches the orchestrator."""
    script = install(daemon, [sup('task', 'On it.', task('write the report'))],
                     {'worker': _worker('The report is written.', 'Report ready.'), 'worker.verify': 'not a verdict',
                      'worker.dispatch': _dispatch('finish', 'I wrote the report but couldn\'t check it.',
                                                   instructions='Stop here.', result='failed')})
    final = outcome(chat(daemon, 'write the report'))
    assert all(v['evidence'].startswith("The check couldn't finish: ") for v in final['verification'])
    findings = next(ctx for _, key, ctx, _ in script.contexts if key == 'worker.dispatch').split('Inspection findings')[1]
    assert 'Expecting value' in findings                               # the exact failure, for recovery
    assert 'Expecting value' not in final['display_text']


def test_an_interrupted_task_says_so_in_plain_words(daemon: RunningDaemon) -> None:
    """A daemon restart or a dropped client used to show "I couldn't finish that: Task interrupted: CancelledError"."""
    async def broken(tree_id: str) -> None:
        raise RuntimeError('the baseline sample failed')
    daemon.daemon.supervisor.on_task_start = broken
    install(daemon, [sup('task', 'Checking.', task('check the gate'))], {'worker': done('Gate B12.')})
    try:
        chat(daemon, 'check the gate')
    except Exception:
        pass                                                   # the turn itself may end with an error
    async def rows(kind: str) -> list[Any]:
        return [e.data for e in daemon.daemon.journal.of_kind(kind)]
    answers = daemon.run(rows('task.answer'))
    assert answers and answers[-1]['status'] == 'failed'
    assert 'RuntimeError' not in answers[-1]['display_text'] and 'RuntimeError' not in answers[-1]['text']
    assert answers[-1]['display_text'].startswith("I couldn't finish that: ")
    noted = daemon.run(rows('task.interrupted'))
    assert noted and noted[-1]['error'] == 'RuntimeError'               # the class name stays in the records


# ---------- (b) verified answers carry the flag, not a prefix ----------

def test_a_verified_answer_has_the_flag_and_no_verified_prefix(daemon: RunningDaemon) -> None:
    full = f'The report is at {REPORT}. It covers all three sessions.'
    install(daemon, [sup('task', 'Writing it up.', task('write the report'))],
            {'worker': _worker(full, f'Report ready: {REPORT}.')})
    final = outcome(chat(daemon, 'write the report'))
    assert final['status'] == 'done' and final['verified'] is True
    assert final['answer'] == full
    assert not final['answer'].startswith('Verified') and not final['display_text'].startswith('Verified')


# ---------- (c) a follow-up with a new goal re-plans, inside the same task ----------

OLD_GOAL = 'Fill the Origins field of the application'
NEW_GOAL = 'Rewrite the Origins field so it sounds interesting, not like a LinkedIn post'


def _filled_origins(daemon: RunningDaemon) -> str:
    """The first task: a saved plan whose one step finished. Then a send of that task whose outcome is unknown."""
    old = task(OLD_GOAL)
    old['plan'] = Plan(steps=[Step(id='fill', kind='do', goal='Fill the Origins field')]).model_dump()
    script = install(daemon, [sup('task', 'Filling it in.', old)],
                     {'step.fill': done('Filled the Origins field.'), 'worker': done('Origins field filled.')})
    tree = str(outcome(chat(daemon, 'fill the Origins field', session='origins'))['task_id'])
    assert sum(key == 'step.fill' for _, key, _, _ in script.contexts) == 1

    async def lost() -> None:
        assert daemon.daemon.ledger is not None
        intent = daemon.daemon.ledger.intend(tree + '.worker', 'mail.send', {'to': 'admissions@example.test'}, 'sent')
        daemon.daemon.ledger.mark(intent.id, 'unknown')
    daemon.run(lost())
    return tree


def _continue(tree: str, goal: str, new_goal: bool, plan: Plan | None = None) -> str:
    follow_up = json.loads(sup('task', 'On it.', task(goal)))
    follow_up['task']['plan'] = plan.model_dump() if plan else None
    follow_up['control'] = {'task_id': tree, 'action': 'continue', 'new_goal': new_goal,
                            'standing_sends': 'unchanged', 'supersedes': []}
    return json.dumps(follow_up)


def test_continue_with_a_new_goal_re_plans_inside_the_same_task(daemon: RunningDaemon) -> None:
    """ef9e5d75: "this sounds too much linkedinish… make it sound interesting" came back as continue on the ended
    task with a new rewrite goal; FERB reopened the saved plan with the old goal and filled nothing new. The planner
    now marks a new goal; FERB runs it in the same task, so the finished step, the record of the uncertain send (P19)
    and the task's history carry over."""
    tree = _filled_origins(daemon)
    refusals: list[Any] = []

    def worker(token: str) -> str:
        grant = daemon.daemon.tokens.validate(token)

        async def check() -> Any:                  # the database belongs to the daemon's loop
            assert daemon.daemon.ledger is not None
            return daemon.daemon.ledger.refusal(grant.task_id, 'mail.send', 'sent')
        refusals.append(daemon.run(check()))
        return done('Rewrote the Origins field.')

    script = install(daemon, [_continue(tree, NEW_GOAL, True)], {'worker': worker})
    second = outcome(chat(daemon, 'this sounds too much linkedinish, make it sound interesting', session='origins'))
    assert second['task_id'] == tree                                     # the same task and its records
    assert second['goal'].startswith(NEW_GOAL)
    context = next(ctx for _, key, ctx, _ in script.contexts if key == 'worker')
    assert 'Task: ' + NEW_GOAL in context
    assert 'Filled the Origins field.' in context                       # the finished step's output carried over
    assert 'Uncertain prior actions' in context and 'admissions@example.test' in context
    assert refusals and refusals[0] is not None                          # an identical send is still refused (P19)
    assert not any(key == 'step.fill' for _, key, _, _ in script.contexts)


def test_continue_toward_the_same_goal_in_new_words_reopens_the_saved_plan(daemon: RunningDaemon) -> None:
    """Rewording alone is not a new goal (the planner always restates the goal for a continue): the saved plan and
    its goal resume."""
    tree = _filled_origins(daemon)
    install(daemon, [_continue(tree, 'Finish filling in the Origins field', False)],
            {'worker': done('Origins field filled.')})
    second = outcome(chat(daemon, 'carry on with it', session='origins'))
    assert second['task_id'] == tree and second['goal'] == OLD_GOAL


def test_a_plan_whose_step_failed_ends_as_failed_not_as_a_crash(daemon: RunningDaemon) -> None:
    """Found while building the next test: a plan left blocked by a failed step reported "blocked" as the task's
    status, which the task records refuse, so the owner got "Sorry — something went wrong" instead of an answer."""
    old = task(OLD_GOAL)
    old['plan'] = Plan(steps=[Step(id='submit', kind='do', goal='Submit the application')]).model_dump()
    install(daemon, [sup('task', 'Submitting.', old)], {'step.submit': json.dumps(
        {'status': 'blocked', 'summary': 'The submit button was disabled.', 'artifacts': [], 'evidence': []})})
    final = outcome(chat(daemon, 'submit the application'))
    assert final['status'] == 'failed'


def test_a_new_goal_without_a_plan_never_reruns_the_old_plan_s_unfinished_steps(daemon: RunningDaemon) -> None:
    """[fill done, submit blocked] + "don't submit it, just rewrite the origins field" (new goal, no plan): one worker
    runs the new goal in the same task, with the old steps' results as data; the unfinished submit never runs."""
    old = task(OLD_GOAL)
    old['plan'] = Plan(steps=[Step(id='fill', kind='do', goal='Fill the Origins field'),
                              Step(id='submit', kind='do', goal='Submit the application', depends_on=['fill'])]
                       ).model_dump()
    blocked = json.dumps({'status': 'blocked', 'summary': 'The submit button was disabled.', 'artifacts': [],
                          'evidence': []})
    install(daemon, [sup('task', 'Filling it in.', old)],
            {'step.fill': done('Filled the Origins field.'), 'step.submit': blocked, 'worker': done('Filled.')})
    tree = outcome(chat(daemon, 'fill the Origins field and submit', session='origins'))['task_id']
    script = install(daemon, [_continue(tree, NEW_GOAL, True)], {'worker': done('Rewrote the Origins field.'),
                                                                 'step.submit': done('Submitted.')})
    second = outcome(chat(daemon, "don't submit it, just rewrite the origins field", session='origins'))
    assert second['task_id'] == tree
    assert not any(key.startswith('step.') for _, key, _, _ in script.contexts)
    context = next(ctx for _, key, ctx, _ in script.contexts if key == 'worker')
    assert 'Filled the Origins field.' in context and 'The submit button was disabled.' in context

    async def pending() -> list[str]:
        return daemon.daemon.supervisor.plans.pending()
    assert tree not in daemon.run(pending())                         # a restart never resumes the old plan either


def test_a_new_plan_keeps_an_unchanged_finished_step_and_shows_the_others_as_data(daemon: RunningDaemon) -> None:
    """A finished send that the new plan keeps unchanged isn't redone and stays visible; a finished step the new plan
    drops is handed to the new steps as data."""
    old = task(OLD_GOAL)
    send = Step(id='send', kind='do', goal='Send the application to admissions')
    old['plan'] = Plan(steps=[Step(id='fill', kind='do', goal='Fill the Origins field'), send]).model_dump()
    install(daemon, [sup('task', 'Filling it in.', old)],
            {'step.fill': done('Filled the Origins field.'), 'step.send': done('Sent the application.'),
             'worker': done('Filled and sent.')})
    tree = outcome(chat(daemon, 'fill the Origins field and send it', session='origins'))['task_id']
    plan = Plan(steps=[send, Step(id='note', kind='do', goal='Write a note about the Origins wording')])
    script = install(daemon, [_continue(tree, NEW_GOAL, True, plan)],
                     {'step.note': done('Wrote the note.'), 'step.send': done('Sent again.'), 'worker': done('Done.')})
    second = outcome(chat(daemon, 'now write a note about the wording', session='origins'))
    assert second['task_id'] == tree
    assert not any(key == 'step.send' for _, key, _, _ in script.contexts)          # the finished send isn't redone
    note = next(ctx for _, key, ctx, _ in script.contexts if key == 'step.note')
    assert 'Filled the Origins field.' in note                                    # the dropped step's result, as data
    final = next(ctx for _, key, ctx, _ in script.contexts if key == 'worker')
    assert 'Sent the application.' in final


def test_continue_without_a_new_goal_on_a_task_without_a_plan_stays_in_that_task(daemon: RunningDaemon) -> None:
    install(daemon, [sup('task', 'Starting.', task('send the application on shop.example', 'owner'))],
            {'worker': json.dumps({'status': 'needs_user', 'summary': 'Login required', 'artifacts': [],
                                   'evidence': []})})
    tree = outcome(chat(daemon, 'send the application', session='apply'))['task_id']

    async def lost() -> None:
        assert daemon.daemon.ledger is not None
        intent = daemon.daemon.ledger.intend(tree + '.worker', 'browser.act', {'site': 'shop.example'}, 'sent')
        daemon.daemon.ledger.mark(intent.id, 'unknown')
    daemon.run(lost())
    refusals: list[Any] = []

    def worker(token: str) -> str:
        grant = daemon.daemon.tokens.validate(token)

        async def check() -> Any:
            assert daemon.daemon.ledger is not None
            return daemon.daemon.ledger.refusal(grant.task_id, 'browser.act', 'sent')
        refusals.append(daemon.run(check()))
        return done('Logged in and checked the application.')

    install(daemon, [_continue(tree, 'carry on with the application', False)], {'worker': worker})
    second = outcome(chat(daemon, 'I logged in, carry on', session='apply'))
    assert second['task_id'] == tree
    assert second['goal'].startswith('send the application on shop.example')
    assert refusals and refusals[0] is not None                                   # P19 follows the task


def _refusals(daemon: RunningDaemon, capability: str, payload: str, report: str) -> tuple[list[Any], Any]:
    """A worker that records whether FERB would refuse an identical action now (P19), then reports `report`."""
    seen: list[Any] = []

    def worker(token: str) -> str:
        grant = daemon.daemon.tokens.validate(token)

        async def check() -> Any:                  # the database belongs to the daemon's loop
            assert daemon.daemon.ledger is not None
            return daemon.daemon.ledger.refusal(grant.task_id, capability, payload)
        seen.append(daemon.run(check()))
        return report
    return seen, worker


def _lost(daemon: RunningDaemon, task_id: str, capability: str, payload: str) -> None:
    async def lost() -> None:
        assert daemon.daemon.ledger is not None
        intent = daemon.daemon.ledger.intend(task_id, capability, {'to': 'admissions@example.test'}, payload)
        daemon.daemon.ledger.mark(intent.id, 'unknown')
    daemon.run(lost())


def test_a_retry_stays_in_the_same_task_so_an_unknown_send_is_still_refused(daemon: RunningDaemon) -> None:
    install(daemon, [sup('task', 'Sending.', task('send the application', 'owner'))],
            {'worker': json.dumps({'status': 'needs_user', 'summary': 'Unclear', 'artifacts': [], 'evidence': []})})
    tree = outcome(chat(daemon, 'send the application', session='apply'))['task_id']
    _lost(daemon, tree + '.worker', 'mail.send', 'sent')
    seen, worker = _refusals(daemon, 'mail.send', 'sent', done('Checked the Sent folder.'))
    retry = json.loads(sup('task', 'Trying again.', task('try again', 'owner')))
    retry['control'] = {'task_id': tree, 'action': 'retry', 'new_goal': False, 'standing_sends': 'unchanged',
                        'supersedes': []}
    install(daemon, [json.dumps(retry)], {'worker': worker})
    second = outcome(chat(daemon, 'try again', session='apply'))
    assert second['task_id'] == tree
    assert seen and seen[0] is not None            # the send may have happened: FERB asks before repeating it (P19)


def test_a_retry_reruns_a_failed_step_and_still_refuses_its_unknown_send(daemon: RunningDaemon) -> None:
    old = task('Send the application')
    old['plan'] = Plan(steps=[Step(id='send', kind='do', goal='Send the application to admissions')]).model_dump()
    install(daemon, [sup('task', 'Sending.', old)], {'step.send': json.dumps(
        {'status': 'blocked', 'summary': 'The page timed out after Send.', 'artifacts': [], 'evidence': []})})
    tree = outcome(chat(daemon, 'send the application', session='apply'))['task_id']
    _lost(daemon, tree + '.step.send', 'mail.send', 'sent')
    seen, step = _refusals(daemon, 'mail.send', 'sent', done('Found it in the Sent folder.'))
    retry = json.loads(sup('task', 'Trying again.', old))
    retry['control'] = {'task_id': tree, 'action': 'retry', 'new_goal': False, 'standing_sends': 'unchanged',
                        'supersedes': []}
    install(daemon, [json.dumps(retry)], {'step.send': step, 'worker': done('Sent.')})
    second = outcome(chat(daemon, 'try again', session='apply'))
    assert second['task_id'] == tree
    assert len(seen) == 1 and seen[0] is not None    # the failed step ran again, and its unknown send is refused


def test_a_new_goal_with_its_own_plan_runs_that_plan_in_the_same_task(daemon: RunningDaemon) -> None:
    """A step of the new plan runs even when a finished step of the old plan had the same id."""
    tree = _filled_origins(daemon)
    plan = Plan(steps=[Step(id='fill', kind='do', goal='Rewrite the Origins field in a lively voice')])
    script = install(daemon, [_continue(tree, NEW_GOAL, True, plan)],
                     {'step.fill': done('Rewrote it.'), 'worker': done('Rewrote the Origins field.')})
    second = outcome(chat(daemon, 'make it sound interesting', session='origins'))
    assert second['task_id'] == tree
    steps = [ctx for _, key, ctx, _ in script.contexts if key == 'step.fill']
    assert len(steps) == 1 and 'Rewrite the Origins field in a lively voice' in steps[0]


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


def test_a_reply_turn_names_what_was_set_and_saved_in_code_built_lines(daemon: RunningDaemon) -> None:
    """The planner doesn't confirm side outputs itself (it can't know yet); code adds what happened."""
    said = 'remind me in 10 minutes to call Mom, and remember that my dentist is Dr. Rao'
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Okay.', 'display_text': 'Okay.', 'remember': [
        {'kind': 'person', 'subject': 'owner', 'key': 'dentist', 'content': 'my dentist is Dr. Rao',
         'quote': 'my dentist is Dr. Rao'}], 'timers': [
        {'when': _in_minutes(10), 'message': 'call Mom', 'quote': 'remind me in 10 minutes to call Mom'}]})])
    events = chat(daemon, said)
    at = _pending(daemon)[0].fire_at.astimezone()
    reply = next(e for e in events if e['type'] == 'reply')
    for field in ('text', 'display_text'):
        lines = reply[field].split('\n')
        assert lines[0] == 'Okay.'
        assert f'⏰ Reminder set for {at:%a %d %b %H:%M}: call Mom' in lines
        # their whole sentence, as saved (I1; re-review F4: never split at a comma)
        assert 'Remembered: remind me in 10 minutes to call Mom, and remember that my dentist is Dr. Rao' in lines
    assert [e['type'] for e in events if e['type'] in ('memory', 'timer')] == ['memory', 'timer']   # still sent


def test_a_task_turn_carries_what_was_set_and_refused_into_the_outcome(daemon: RunningDaemon) -> None:
    """The Mac app shows timer events only as passing activity, so the outcome itself says what was set."""
    plan = json.loads(sup('task', 'Checking.', task('check the gate number')))
    plan['timers'] = [{'when': _in_minutes(10), 'message': 'stretch', 'quote': 'remind me in 10 minutes to stretch'},
                      {'when': _in_minutes(10), 'message': 'boarding starts', 'quote': 'words the owner never said'}]
    install(daemon, [json.dumps(plan)], {'worker': _worker('Your gate is B12.', 'Gate B12.')})
    final = outcome(chat(daemon, 'check the gate number and remind me in 10 minutes to stretch', session='gate'))
    for field in ('answer', 'display_text'):
        assert '⏰ Reminder set for' in final[field] and ': stretch' in final[field]
        assert "I couldn't set the reminder “boarding starts”" in final[field]
    assert final['display_text'].startswith('Gate B12.')


def test_a_clarify_question_is_kept_without_code_lines(daemon: RunningDaemon) -> None:
    install(daemon, [json.dumps({'kind': 'clarify', 'text': 'Which gate?', 'remember': [], 'timers': [
        {'when': _in_minutes(10), 'message': 'boarding starts', 'quote': 'words the owner never said'}]})])
    reply = next(e for e in chat(daemon, 'check my gate', session='gate') if e['type'] == 'clarify')
    assert reply['text'].startswith('Which gate?\n')                    # the owner sees the refusal line

    async def asked() -> str:
        return daemon.daemon.supervisor.continuity.pending('gate')[1]
    assert daemon.run(asked()) == 'Which gate?'                         # the question an answer continues


@pytest.mark.parametrize('quote, line', [('every day at 9 write report.md', '🗓 Scheduled: write report.md — every day at '),
                                         ('words the owner never said', "I couldn't set that up")])
def test_a_schedule_is_stated_by_code_in_the_reply(daemon: RunningDaemon, quote: str, line: str) -> None:
    from tests.support.schedules import schedule_reply
    install(daemon, [schedule_reply(daemon, 'write report.md', quote,
                                    daemon.daemon.clock.now() + timedelta(minutes=5), repeat='daily')])
    reply = next(e for e in chat(daemon, 'every day at 9 write report.md') if e['type'] == 'reply')
    lines = reply['text'].split('\n')
    assert lines[0] == 'Scheduled.' and len(lines) == 2 and lines[1].startswith(line)

    async def ids() -> list[str]:
        return [str(r['id']) for r in daemon.daemon.db.conn.execute('SELECT id FROM schedules')]
    assert not any(sid in reply['text'] for sid in daemon.run(ids()))   # the id stays in the records


def test_a_one_off_schedule_and_a_reminder_cancel_read_in_plain_words(daemon: RunningDaemon) -> None:
    from tests.support.schedules import schedule_reply
    install(daemon, [schedule_reply(daemon, 'write report.md', 'write report.md',
                                    daemon.daemon.clock.now() + timedelta(minutes=5))])
    reply = next(e for e in chat(daemon, 'in 5 minutes write report.md') if e['type'] == 'reply')
    assert reply['text'].split('\n')[1].startswith('🗓 Scheduled: write report.md — on ')
    install(daemon, [_reply_with_timer('remind me in 10 minutes', 'stretch')])
    chat(daemon, 'remind me in 10 minutes to stretch', session='plain')
    [(timer_id, _)] = _reminders(daemon)
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Done.', 'remember': [], 'timers': [],
                                 'reminder_cancels': [{'id': timer_id, 'quote': 'cancel the stretch reminder'}]})])
    reply = next(e for e in chat(daemon, 'cancel the stretch reminder', session='plain') if e['type'] == 'reply')
    told = reply['text'].split('\n')[1]
    assert told.startswith('⏰ Cancelled the reminder “stretch”') and timer_id not in told


# ---------- (e) reminder provenance: the owner's words in this chat ----------

def test_a_reminder_asked_for_in_an_earlier_message_of_the_same_chat_is_set(daemon: RunningDaemon) -> None:
    """19:52 asked for the boarding timer; the reply was lost, and at 20:10 the planner set it from that message."""
    asked = 'set a timer for 8:20, the boarding starts at 8.25'
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Noted.', 'remember': [], 'timers': []})])
    chat(daemon, asked, session='boarding')
    timer = {'when': _in_minutes(10), 'message': 'boarding starts at 8.25', 'quote': 'set a timer for 8:20'}
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Timer set.', 'remember': [], 'timers': [timer]})])
    events = chat(daemon, 'did you set it?', session='boarding')
    assert [e['text'] for e in events if e['type'] == 'timer'][0].startswith('⏰ Reminder set for')
    assert [t.message for t in _pending(daemon)] == ['boarding starts at 8.25']


def _reply_with_timer(quote: str, message: str = 'boarding starts at 8.25') -> str:
    return json.dumps({'kind': 'reply', 'text': 'Okay.', 'remember': [],
                       'timers': [{'when': _in_minutes(10), 'message': message, 'quote': quote}]})


def _reminders(daemon: RunningDaemon) -> list[Any]:
    async def read() -> list[Any]:
        return list(daemon.daemon.db.conn.execute("SELECT id, state FROM timers WHERE kind = 'reminder'"))
    return daemon.run(read())   # type: ignore[no-any-return]


@pytest.mark.parametrize('then', ['pending', 'cancelled', 'delivered'])
def test_an_earlier_message_authorizes_its_reminder_once(daemon: RunningDaemon, then: str) -> None:
    """An old message can't set its reminder again: not while it's pending, nor after it was cancelled or fired."""
    install(daemon, [_reply_with_timer('set a timer for 8:20')])
    chat(daemon, 'set a timer for 8:20, the boarding starts at 8.25', session='boarding')
    [(timer_id, _)] = _reminders(daemon)

    async def change() -> None:
        if then == 'cancelled':
            daemon.daemon.timers.cancel(timer_id)
        elif then == 'delivered':
            daemon.daemon.db.conn.execute("UPDATE timers SET state = 'delivered' WHERE id = ?", (timer_id,))
    daemon.run(change())
    async def fire_at() -> str:
        timer = daemon.daemon.timers.get(timer_id)
        assert timer is not None
        return f'{timer.fire_at.astimezone():%a %d %b %H:%M}'
    when = daemon.run(fire_at())
    install(daemon, [_reply_with_timer('set a timer for 8:20')])
    reply = next(e for e in chat(daemon, 'did you set the boarding timer?', session='boarding') if e['type'] == 'reply')
    assert len(_reminders(daemon)) == 1
    told = reply['text'].split('\n')[-1]
    assert told == {'pending': f'⏰ The reminder “boarding starts at 8.25” is already set for {when}.',
                    'cancelled': f'⏰ The reminder “boarding starts at 8.25” for {when} was cancelled; I didn\'t set it '
                                 'again.',
                    'delivered': f'⏰ The reminder “boarding starts at 8.25” already went off ({when}).'}[then]


def test_an_overlapping_quote_from_the_same_message_is_the_same_reminder(daemon: RunningDaemon) -> None:
    install(daemon, [_reply_with_timer('set a timer for 8:20')])
    chat(daemon, 'set a timer for 8:20, the boarding starts at 8.25', session='boarding')
    install(daemon, [_reply_with_timer('timer for 8:20')])
    chat(daemon, 'did you set it?', session='boarding')
    assert len(_reminders(daemon)) == 1


def test_two_distinct_reminders_from_one_earlier_message_are_both_set(daemon: RunningDaemon) -> None:
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Noted.', 'remember': [], 'timers': []})])
    chat(daemon, 'remind me in 10 minutes to stretch and remind me in 10 minutes to drink water', session='water')
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Okay.', 'remember': [], 'timers': [
        {'when': _in_minutes(10), 'message': 'stretch', 'quote': 'remind me in 10 minutes to stretch'},
        {'when': _in_minutes(10), 'message': 'drink water', 'quote': 'remind me in 10 minutes to drink water'}]})])
    chat(daemon, 'did you set those?', session='water')
    assert len(_reminders(daemon)) == 2


def test_a_clarified_message_authorizes_its_reminder_once_too(daemon: RunningDaemon) -> None:
    """The answer to FERB's question carries the earlier message's words; they stay that message's, used once."""
    install(daemon, [sup('clarify', 'Morning or evening?')])
    chat(daemon, 'set a timer for 8:20 for boarding', session='boarding')
    install(daemon, [_reply_with_timer('set a timer for 8:20')])
    chat(daemon, 'evening', session='boarding')
    install(daemon, [_reply_with_timer('set a timer for 8:20')])
    chat(daemon, 'is it set?', session='boarding')
    assert len(_reminders(daemon)) == 1


def test_a_recovered_reminder_is_not_set_twice(daemon: RunningDaemon) -> None:
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Noted.', 'remember': [], 'timers': []})])
    chat(daemon, 'set a timer for 8:20, the boarding starts at 8.25', session='boarding')
    for _ in range(2):
        install(daemon, [_reply_with_timer('set a timer for 8:20')])
        chat(daemon, 'did you set it?', session='boarding')
    assert len(_reminders(daemon)) == 1


def test_two_reminders_quoting_the_same_words_in_one_turn_are_one(daemon: RunningDaemon) -> None:
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Okay.', 'remember': [], 'timers': [
        {'when': _in_minutes(10), 'message': 'boarding starts', 'quote': 'set a timer for 8:20'},
        {'when': _in_minutes(10), 'message': 'boarding', 'quote': 'timer for 8:20'}]})])
    reply = next(e for e in chat(daemon, 'set a timer for 8:20, the boarding starts at 8.25', session='boarding')
                 if e['type'] == 'reply')
    assert len(_reminders(daemon)) == 1
    assert '⏰ The reminder “boarding starts” is already set for' in reply['text']


def test_two_reminders_at_different_times_quoting_the_same_words_are_both_set(daemon: RunningDaemon) -> None:
    """Round 4: "remind me at 3 and at 5 to stretch", the planner quoting the whole sentence for both. Overlapping
    words alone don't make two reminders one: their code-resolved times differ."""
    said = 'remind me at 3 and at 5 to stretch'
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Okay.', 'remember': [], 'timers': [
        {'when': _at('15:00'), 'message': 'stretch', 'quote': said},
        {'when': _at('17:00'), 'message': 'stretch', 'quote': said}]})])
    reply = next(e for e in chat(daemon, said, session='stretch') if e['type'] == 'reply')
    assert len(_reminders(daemon)) == 2
    assert 'already' not in reply['text']


def test_an_earlier_message_s_second_reminder_at_another_time_is_set(daemon: RunningDaemon) -> None:
    said = 'remind me at 3 and at 5 to stretch'
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Okay.', 'remember': [], 'timers': [
        {'when': _at('15:00'), 'message': 'stretch', 'quote': said}]})])
    chat(daemon, said, session='stretch')
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Okay.', 'remember': [], 'timers': [
        {'when': _at('17:00'), 'message': 'stretch', 'quote': said}]})])
    chat(daemon, 'and the one at 5?', session='stretch')
    assert len(_reminders(daemon)) == 2


def test_a_relative_reminder_from_an_earlier_message_is_not_set_again_later(daemon: RunningDaemon) -> None:
    """"In 10 minutes", resolved again two minutes later, is a later time; it is still the reminder already set from
    those words: the time is compared as resolved when that reminder was set."""
    install(daemon, [_reply_with_timer('remind me in 10 minutes to stretch', 'stretch')])
    chat(daemon, 'remind me in 10 minutes to stretch', session='stretch')
    daemon.call_soon(daemon.daemon.clock.advance, 120)
    install(daemon, [_reply_with_timer('remind me in 10 minutes to stretch', 'stretch')])
    reply = next(e for e in chat(daemon, 'did you set it?', session='stretch') if e['type'] == 'reply')
    assert len(_reminders(daemon)) == 1
    assert '⏰ The reminder “stretch” is already set for' in reply['text']


def test_a_same_turn_duplicate_never_falls_back_to_an_earlier_message(daemon: RunningDaemon) -> None:
    """The same words in an earlier message whose reminder was never set: the turn's second copy is the reminder the
    turn just set, not a new one authorized by that earlier message."""
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Noted.', 'remember': [], 'timers': []})])
    chat(daemon, 'remind me in 10 minutes to stretch', session='stretch')
    twice = {'when': _in_minutes(10), 'message': 'stretch', 'quote': 'remind me in 10 minutes to stretch'}
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Okay.', 'remember': [], 'timers': [twice, twice]})])
    reply = next(e for e in chat(daemon, 'remind me in 10 minutes to stretch', session='stretch')
                 if e['type'] == 'reply')
    assert len(_reminders(daemon)) == 1
    assert '⏰ The reminder “stretch” is already set for' in reply['text']


def test_two_same_turn_copies_of_an_already_set_reminder_get_one_line(daemon: RunningDaemon) -> None:
    install(daemon, [_reply_with_timer('remind me in 10 minutes to stretch', 'stretch')])
    chat(daemon, 'remind me in 10 minutes to stretch', session='stretch')
    twice = {'when': _in_minutes(10), 'message': 'stretch', 'quote': 'remind me in 10 minutes to stretch'}
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Okay.', 'remember': [], 'timers': [twice, twice]})])
    reply = next(e for e in chat(daemon, 'did you set it?', session='stretch') if e['type'] == 'reply')
    assert len(_reminders(daemon)) == 1
    assert reply['text'].count('⏰ The reminder “stretch”') == 1, reply['text']


def test_an_earlier_today_reminder_re_emitted_after_its_time_is_told_as_existing(daemon: RunningDaemon) -> None:
    """Round 5 (the reviewer's probe): "remind me today at <in 10 minutes> to call", then 20 minutes later the
    planner emits it again from that message. Its time can't be resolved into the future any more, but it is the
    reminder already set from those words: FERB says what exists, never "a problem on my side… ask again"."""
    from ferb.clock.when import local_zone
    now = daemon.daemon.clock.now().astimezone(local_zone())
    if (now + timedelta(minutes=30)).date() != now.date():
        pytest.skip('too close to local midnight for "today"')
    at = (now + timedelta(minutes=10)).strftime('%H:%M')
    said = f'remind me today at {at} to call'
    today = {'in_minutes': None, 'in_days': None, 'day': 'today', 'date': None, 'time': at}
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Okay.', 'remember': [],
                                 'timers': [{'when': today, 'message': 'call', 'quote': said}]})])
    chat(daemon, said, session='today')
    assert len(_reminders(daemon)) == 1
    daemon.call_soon(daemon.daemon.clock.advance, 20 * 60)
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Okay.', 'remember': [],
                                 'timers': [{'when': today, 'message': 'call', 'quote': said}]})])
    reply = next(e for e in chat(daemon, 'did you set it?', session='today') if e['type'] == 'reply')
    assert "I couldn't set" not in reply['text'], reply['text']
    assert '⏰ The reminder “call”' in reply['text'] and len(_reminders(daemon)) == 1


def test_a_reminder_set_event_reads_in_plain_words_and_carries_its_id_apart(daemon: RunningDaemon) -> None:
    """The notch shows a timer event's text as it passes: no internal id in it (the id is its own field)."""
    install(daemon, [_reply_with_timer('remind me in 10 minutes', 'stretch')])
    events = chat(daemon, 'remind me in 10 minutes to stretch', session='plain')
    [(timer_id, _)] = _reminders(daemon)
    [event] = [e for e in events if e['type'] == 'timer']
    assert event['timer_id'] == timer_id and timer_id not in event['text']
    assert event['text'].startswith('⏰ Reminder set for ') and event['text'].endswith(': stretch')


def test_asking_again_in_a_new_message_sets_it_again(daemon: RunningDaemon) -> None:
    install(daemon, [_reply_with_timer('set a timer for 8:20')])
    chat(daemon, 'set a timer for 8:20, the boarding starts at 8.25', session='boarding')
    install(daemon, [_reply_with_timer('set a timer for 8:20')])
    chat(daemon, 'set a timer for 8:20 again please', session='boarding')
    assert len(_reminders(daemon)) == 2


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
        update['control'] = {'task_id': tree_id, 'action': 'steer', 'standing_sends': 'unchanged', 'supersedes': []}
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
    ('supervisor.md', 'FERB adds that line itself'),
    ('supervisor.md', 'an earlier one whose reminder was never set'),
    ('supervisor.md', 'new_goal'),
    ('dispatch.md', 'FERB shows your question on its own line'),
])
def test_the_answer_prompts_require_the_question_as_asked_and_no_internal_terms(name: str, words: str) -> None:
    assert words in _prompt(name)


def test_the_planner_is_not_told_to_confirm_a_reminder_it_cannot_know_was_set() -> None:
    assert 'Reply confirming' not in _prompt('supervisor.md')        # reminders and schedules alike


def _in_minutes(n: int) -> dict[str, Any]:
    return {'in_minutes': n, 'in_days': None, 'day': None, 'date': None, 'time': None}


def _pending(daemon: RunningDaemon) -> list[Any]:
    async def read() -> list[Any]:
        return list(daemon.daemon.timers.pending())
    return daemon.run(read())   # type: ignore[no-any-return]


def _at(clock: str) -> dict[str, Any]:
    return {'in_minutes': None, 'in_days': None, 'day': None, 'date': None, 'time': clock}
