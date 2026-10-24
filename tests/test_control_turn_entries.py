"""A planner turn that stops, steers, continues or retries a task still applies its other entries: memory, forgets,
reminders, schedules, conversation and notification changes, each exactly once, with the same quote checks and lines.

Live 2026-10-24: the planner wrote remember entries on two steers ("take my creds once, don't ask every time" at
05:44; "book anywhere, don't wait for Chennai" at 12:22) and code dropped both, because the live-steer and stop paths
returned before the entries were applied; the visa schedule kept its stale scope."""
import asyncio
import json
import threading
from datetime import timedelta
from typing import Any

import pytest

from ferb.core.types import RuntimeEvent
from ferb.policy.targets import Targets
from tests.conftest import FakeRuntime, chat, done, install, outcome, sup, task
from tests.support.schedules import make_schedule, run

SCOPE = "book anywhere, don't wait for Chennai"


def _remember(quote: str = SCOPE) -> list[dict[str, Any]]:
    return [{"kind": "fact", "subject": "owner", "key": "visa-booking-scope", "content": quote, "quote": quote}]


def _saved(daemon: Any) -> list[Any]:
    return [i for i in run(daemon, lambda: daemon.daemon.memory.items(subject="owner"))
            if i.key == "visa_booking_scope"]                     # the store's form of the planner's key


class _Stalled:
    """A worker that runs until released (or stopped), so a correction finds it live."""

    def __init__(self, monkeypatch: pytest.MonkeyPatch) -> None:
        self.started, self.release = threading.Event(), threading.Event()
        self.handles: list[Any] = []
        original = FakeRuntime.events

        async def events(rt: Any, run_: Any) -> Any:
            if run_.state['key'] != 'worker':
                async for event in original(rt, run_):
                    yield event
                return
            self.handles.append(run_)
            self.started.set()
            while not self.release.is_set() and not run_.state['cancel']:
                await asyncio.sleep(.01)
            yield RuntimeEvent('final', {'text': done('Booked the earliest slot.')})

        async def steer(rt: Any, run_: Any, message: str) -> None:
            return None
        monkeypatch.setattr(FakeRuntime, 'events', events)
        monkeypatch.setattr(FakeRuntime, 'continue_', steer)


def _control(goal: str, tree_id: str, action: str, **entries: Any) -> str:
    turn = json.loads(sup('task', 'Changing it.', task(goal)))
    turn['control'] = {'task_id': tree_id, 'action': action, 'standing_sends': 'unchanged', 'supersedes': []}
    return json.dumps(turn | entries)


def test_a_live_steer_saves_the_planners_memory_entry_once(daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    worker = _Stalled(monkeypatch)
    script = install(daemon, [sup('task', 'Booking.', task('Book the visa slot in Chennai'))])
    first: list[dict[str, Any]] = []
    thread = threading.Thread(target=lambda: first.extend(chat(daemon, 'Book the visa slot in Chennai',
                                                               session='visa')))
    thread.start()
    try:
        assert worker.started.wait(5)
        tree_id = worker.handles[0].task_id.rsplit('.', 1)[0]
        script.sup.append(_control('Book the earliest visa slot anywhere', tree_id, 'steer', remember=_remember()))
        events = chat(daemon, SCOPE, session='visa', timeout=5)
        assert [e['type'] for e in events].count('memory') == 1, events
        assert len(_saved(daemon)) == 1
        reply = [e for e in events if e['type'] == 'reply'][-1]
        # the steer's own reply, then code's line about what was saved, in what the owner reads first too
        assert reply['text'].startswith('Got it') and reply['text'].endswith(f'Remembered: {SCOPE}')
        assert reply['display_text'].endswith(f'Remembered: {SCOPE}')
        assert len(worker.handles) == 1                           # the same run took the correction
    finally:
        worker.release.set()
        thread.join(5)
    assert outcome(first)['task_id'] == tree_id
    assert len(_saved(daemon)) == 1


def test_a_stop_with_a_schedule_change_pauses_the_schedule(daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    now = daemon.daemon.clock.now()
    sched = make_schedule(daemon, "check visa slots", "every day at 10 check visa slots", Targets.of(),
                          now + timedelta(hours=20))
    worker = _Stalled(monkeypatch)
    script = install(daemon, [sup('task', 'Checking.', task('Check visa slots'))])
    first: list[dict[str, Any]] = []
    thread = threading.Thread(target=lambda: first.extend(chat(daemon, 'Check visa slots', session='visa')))
    thread.start()
    try:
        assert worker.started.wait(5)
        tree_id = worker.handles[0].task_id.rsplit('.', 1)[0]
        script.sup.append(_control('Check visa slots', tree_id, 'stop', schedule_changes=[
            {'id': sched.id, 'action': 'pause', 'quote': 'pause the visa task'}]))
        events = chat(daemon, "pause the visa task for now, I'll say when to continue", session='visa', timeout=15)
    finally:
        worker.release.set()
        thread.join(5)
    assert run(daemon, lambda: daemon.daemon.schedules.get(sched.id)).state == 'paused'
    timer_lines = [e['text'] for e in events if e['type'] == 'timer']
    assert len(timer_lines) == 1 and timer_lines[0].startswith('Paused')
    reply = [e for e in events if e['type'] == 'reply'][-1]['text']
    assert reply.startswith('Stopped, as you asked.') and timer_lines[0] in reply


def test_a_continue_of_an_ended_task_still_saves_its_memory_entry_once(daemon: Any) -> None:
    """The path that already applied the entries (a continue resumes the ended task in place) applies them once."""
    script = install(daemon, [sup('task', 'Booking.', task('Book the visa slot in Chennai'))],
                     {'worker': [done('Booked a Chennai slot.'), done('Booked the earliest slot anywhere.')]})
    first = chat(daemon, 'Book the visa slot in Chennai', session='visa')
    tree_id = outcome(first)['task_id']
    script.sup.append(_control('Book the earliest visa slot anywhere', tree_id, 'continue', remember=_remember()))
    events = chat(daemon, SCOPE, session='visa')
    assert [e['type'] for e in events].count('memory') == 1, events
    assert len(_saved(daemon)) == 1
    assert outcome(events)['task_id'] == tree_id


def _replies(daemon: Any, session: str) -> list[str]:
    """What the conversation record holds as FERB's replies in this chat (review M3)."""
    async def read() -> list[str]:
        return [json.loads(r[0])['text'] for r in daemon.daemon.db.conn.execute(
            "SELECT data_json FROM journal WHERE kind='chat.reply' AND json_extract(data_json, '$.session_id')=? "
            "ORDER BY seq", (session,))]
    return daemon.run(read())


def _live(daemon: Any, monkeypatch: pytest.MonkeyPatch, goal: str = 'Check visa slots') -> tuple[Any, Any, Any, Any]:
    worker = _Stalled(monkeypatch)
    script = install(daemon, [sup('task', 'On it.', task(goal))])
    first: list[dict[str, Any]] = []
    thread = threading.Thread(target=lambda: first.extend(chat(daemon, goal, session='visa')))
    thread.start()
    assert worker.started.wait(5)
    return worker, script, thread, worker.handles[0].task_id.rsplit('.', 1)[0]


def test_stop_and_steer_replies_reach_the_conversation_record_with_ferbs_lines(daemon: Any,
                                                                                monkeypatch: pytest.MonkeyPatch) -> None:
    worker, script, thread, tree_id = _live(daemon, monkeypatch)
    try:
        script.sup.append(_control('Book the earliest visa slot anywhere', tree_id, 'steer', remember=_remember()))
        chat(daemon, SCOPE, session='visa', timeout=5)
        script.sup.append(_control('Check visa slots', tree_id, 'stop'))
        chat(daemon, 'stop it', session='visa', timeout=15)
    finally:
        worker.release.set()
        thread.join(5)
    replies = _replies(daemon, 'visa')
    assert any(r.startswith('Got it') and r.endswith(f'Remembered: {SCOPE}') for r in replies), replies
    assert 'Stopped, as you asked.' in replies


def test_a_steer_that_finds_the_task_just_ended_applies_nothing_and_asks_for_the_whole_message(
        daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """Review M2: the race reply asks for a resend, so nothing of this message is applied now; the resend applies it
    all once, with no second reminder, schedule or memory."""
    from ferb.orchestrator import steering, supervisor
    async def just_ended(*args: Any, **kwargs: Any) -> Any:
        return {'type': 'reply', 'text': steering.JUST_ENDED}, None
    monkeypatch.setattr(supervisor, 'apply_steering', just_ended)
    worker, script, thread, tree_id = _live(daemon, monkeypatch)
    try:
        script.sup.append(_control('Book the earliest visa slot anywhere', tree_id, 'steer', remember=_remember()))
        events = chat(daemon, SCOPE, session='visa', timeout=5)
    finally:
        worker.release.set()
        thread.join(5)
    assert [e['type'] for e in events] == ['reply'] and events[0]['text'] == steering.JUST_ENDED
    assert _saved(daemon) == [] and steering.JUST_ENDED in _replies(daemon, 'visa')


def test_pasted_words_on_a_steer_or_a_stop_save_nothing_and_change_no_schedule(daemon: Any,
                                                                               monkeypatch: pytest.MonkeyPatch) -> None:
    """The quote checks hold on the control paths: words inside a paste are data, never the owner's instruction."""
    now = daemon.daemon.clock.now()
    sched = make_schedule(daemon, "check visa slots", "every day at 10 check visa slots", Targets.of(),
                          now + timedelta(hours=20))
    worker, script, thread, tree_id = _live(daemon, monkeypatch)
    try:
        script.sup.append(_control('Book the earliest visa slot anywhere', tree_id, 'steer', remember=_remember()))
        steered = chat(daemon, f'keep going ```{SCOPE}```', session='visa', timeout=5)
        script.sup.append(_control('Check visa slots', tree_id, 'stop', schedule_changes=[
            {'id': sched.id, 'action': 'pause', 'quote': 'pause the visa task'}]))
        stopped = chat(daemon, 'stop ```pause the visa task```', session='visa', timeout=15)
    finally:
        worker.release.set()
        thread.join(5)
    assert _saved(daemon) == [] and 'memory' not in [e['type'] for e in steered]
    assert "aren't in what you typed" in [e for e in steered if e['type'] == 'reply'][-1]['text']
    assert run(daemon, lambda: daemon.daemon.schedules.get(sched.id)).state == 'active'
    assert 'own words' in [e for e in stopped if e['type'] == 'reply'][-1]['text']


def test_a_forget_on_a_stop_only_asks(daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """A forget on the stop path deletes nothing: FERB asks Yes/No naming what goes (memory review I2)."""
    install(daemon, [json.dumps({'kind': 'reply', 'text': 'Noted.', 'task': None, 'remember': _remember()})])
    chat(daemon, SCOPE, session='visa')
    (item,) = _saved(daemon)
    worker, script, thread, tree_id = _live(daemon, monkeypatch)
    try:
        script.sup.append(_control('Check visa slots', tree_id, 'stop', forget=[
            {'item_id': item.id, 'quote': 'forget the visa booking scope'}]))
        events = chat(daemon, 'stop it; forget the visa booking scope', session='visa', timeout=15)
    finally:
        worker.release.set()
        thread.join(5)
    assert len(_saved(daemon)) == 1                               # still there until the owner's Yes
    asked = [a for a in daemon.daemon.state.approvals.pending() if 'Forget for good?' in a.reason]
    assert len(asked) == 1 and any(e['type'] == 'memory' and 'Forget for good?' in e['text'] for e in events)
