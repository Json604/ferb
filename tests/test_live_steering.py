"""Owner corrections reach the existing active attempt without stopping it (D3, D32, MP2)."""
import asyncio
import json
import threading

import pytest

from ferb.core.types import RuntimeEvent
from tests.conftest import FakeRuntime, chat, cond, done, install, outcome, sup, task


@pytest.mark.parametrize('action', ['continue'])
def test_correction_reaches_same_active_run_without_stop_or_replacement(daemon, monkeypatch, action):
    started, release = threading.Event(), threading.Event()
    received, handles, cancelled = [], [], []
    original_events, original_cancel = FakeRuntime.events, FakeRuntime.cancel

    async def events(rt, run):
        if run.state['key'] != 'worker':
            async for event in original_events(rt, run):
                yield event
            return
        handles.append(run)
        started.set()
        while not release.is_set() and not run.state['cancel']:
            await asyncio.sleep(.01)
        yield RuntimeEvent('final', {'text': done('Report with the corrected budget')})

    async def steer(rt, run, message):
        received.append((run.id, message))

    async def cancel(rt, run):
        cancelled.append(run.id)
        await original_cancel(rt, run)

    monkeypatch.setattr(FakeRuntime, 'events', events)
    monkeypatch.setattr(FakeRuntime, 'continue_', steer)
    monkeypatch.setattr(FakeRuntime, 'cancel', cancel)
    script = install(daemon, [sup('task', 'Searching', task('Find lunch'))])
    initial = []
    thread = threading.Thread(target=lambda: initial.extend(chat(daemon, 'Find lunch', session='steering')))
    thread.start()
    try:
        assert started.wait(5)
        tree_id = handles[0].task_id.rsplit('.', 1)[0]
        update = json.loads(sup('task', 'Keeping it under 300.', task('Find lunch under 300',
            conds=(cond('Total below 300'),))))
        update['control'] = {'task_id': tree_id, 'action': action, 'standing_sends': 'unchanged', 'supersedes': []}
        script.sup.append(json.dumps(update))
        reply = chat(daemon, 'Keep it under 300', session='steering', timeout=2)
        assert len(received) == 1 and received[0][0] == handles[0].id
        assert 'Keep it under 300' in received[0][1]
        assert handles[0].id not in cancelled
        assert len(handles) == 1
        assert not any(event['type'] == 'plan' for event in reply)
        async def inspect():
            supervisor = daemon.daemon.supervisor
            saved = supervisor.continuity.load(tree_id, 'steering')
            assert saved[0].goal == 'Find lunch under 300'
            assert 'Keep it under 300' in saved[1].words
            assert 'Keep it under 300' in saved[1].payload
            assert not supervisor._tree_of[tree_id].stop
        daemon.run(inspect())
    finally:
        release.set()
        thread.join(5)
    assert not thread.is_alive()
    assert outcome(initial)['task_id'] == tree_id
    assert len(handles) == 1
    checks = [context for _, key, context, _ in script.contexts if key == 'worker.verify']
    assert checks and 'Total below 300' in checks[0] and 'Keep it under 300' in checks[0]
