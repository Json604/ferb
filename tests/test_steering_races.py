"""Owner corrections survive runtime startup, cleanup and finalization races (D3/D32)."""
import asyncio
import json
import threading
from contextlib import contextmanager

import pytest

from ferb.core.types import RuntimeEvent
from tests.conftest import FakeRuntime, chat, done, install, outcome, sup, task

SESSION = 'steering-races'
CORRECTION = 'Keep lunch under 300'


async def wait_gate(gate):
    while not gate.is_set():
        await asyncio.sleep(.005)


@contextmanager
def running_chat(daemon, words, release, *, timeout=10):
    events, errors = [], []

    def call():
        try:
            events.extend(chat(daemon, words, session=SESSION, timeout=timeout))
        except BaseException as exc:
            errors.append(exc)

    thread = threading.Thread(target=call)
    thread.start()
    try:
        yield events, thread
    finally:
        release.set()
        thread.join(5)
        assert not thread.is_alive(), 'chat did not settle after its gate was released'
        assert not errors, errors


def route_update(script, tree_id):
    update = json.loads(sup('task', 'Updating lunch.', task(CORRECTION)))
    update['control'] = {'task_id': tree_id, 'action': 'continue', 'standing_sends': 'unchanged', 'supersedes': []}
    script.sup.append(json.dumps(update))


def forbid_stop(supervisor, monkeypatch):
    calls = []
    original = supervisor.stop_tree

    def stop(tree_id):
        calls.append(tree_id)
        return original(tree_id)

    monkeypatch.setattr(supervisor, 'stop_tree', stop)
    return calls


def test_correction_during_slow_runtime_start_is_injected_after_start(daemon, monkeypatch):
    entered, release = threading.Event(), threading.Event()
    tree_ids, received, cancelled = [], [], []
    original_start, original_cancel = FakeRuntime.start, FakeRuntime.cancel

    async def start(rt, worker, request, token):
        if worker.id.endswith('.worker'):
            tree_ids.append(worker.parent_id)
            entered.set()
            await wait_gate(release)
        return await original_start(rt, worker, request, token)

    async def steer(rt, run, message):
        received.append((run.task_id, message))

    async def cancel(rt, run):
        cancelled.append(run.task_id)
        await original_cancel(rt, run)

    monkeypatch.setattr(FakeRuntime, 'start', start)
    monkeypatch.setattr(FakeRuntime, 'continue_', steer)
    monkeypatch.setattr(FakeRuntime, 'cancel', cancel)
    script = install(daemon, [sup('task', 'Searching.', task('Find lunch'))], {'worker': done('Updated lunch')})
    stops = forbid_stop(daemon.daemon.supervisor, monkeypatch)
    with running_chat(daemon, 'Find lunch', release) as (initial, thread):
        assert entered.wait(5)
        route_update(script, tree_ids[0])
        reply = chat(daemon, CORRECTION, session=SESSION, timeout=2)
        assert received == []
        assert tree_ids[0] + '.worker' not in cancelled
        assert any(e['type'] == 'reply' for e in reply)
        release.set()
    assert outcome(initial)['status'] == 'done'
    assert tree_ids == [outcome(initial)['task_id']]
    assert len(received) == 1 and CORRECTION in received[0][1]
    assert not stops


@pytest.mark.parametrize('boundary', ['cleanup', 'dispatcher'])
def test_correction_at_post_worker_boundary_prevents_stale_finish(daemon, monkeypatch, boundary):
    supervisor = daemon.daemon.supervisor
    entered, release = threading.Event(), threading.Event()
    tree_ids = []
    method = '_release_resources' if boundary == 'cleanup' else '_dispatch'
    original = getattr(supervisor, method)

    async def boundary_call(tree, *args):
        if not entered.is_set():
            tree_ids.append(tree.id)
            entered.set()
            await wait_gate(release)
        return await original(tree, *args)

    monkeypatch.setattr(supervisor, method, boundary_call)
    first = done('Old lunch') if boundary == 'cleanup' else json.dumps({
        'status': 'failed', 'summary': 'Old failure', 'artifacts': [], 'evidence': []})
    script = install(daemon, [sup('task', 'Searching.', task('Find lunch'))],
                     {'worker': [first, done('Updated lunch under 300')]})
    stops = forbid_stop(supervisor, monkeypatch)
    with running_chat(daemon, 'Find lunch', release) as (initial, thread):
        assert entered.wait(5)
        route_update(script, tree_ids[0])
        reply = chat(daemon, CORRECTION, session=SESSION, timeout=2)
        assert any(e['type'] == 'reply' for e in reply)
        assert not any(e['type'] == 'outcome' for e in initial)
        release.set()
    final = outcome(initial)
    assert final['status'] == 'done' and final['task_id'] == tree_ids[0]
    assert 'Updated lunch' in final['summary']
    worker_contexts = [ctx for _, key, ctx, _ in script.contexts if key == 'worker']
    assert len(worker_contexts) == 2 and CORRECTION in worker_contexts[1]
    assert not stops


def test_finalization_window_waits_for_natural_finish_without_stopping(daemon, monkeypatch):
    supervisor = daemon.daemon.supervisor
    entered, release, update_done = threading.Event(), threading.Event(), threading.Event()
    waiting_for_finish = threading.Event()
    ids, update_events, update_errors = [], [], []
    original = supervisor.on_task_finish

    async def finishing(tree_id):
        if not entered.is_set():
            ids.append(tree_id)
            entered.set()
            await wait_gate(release)
        if original is not None:
            await original(tree_id)

    monkeypatch.setattr(supervisor, 'on_task_finish', finishing)
    script = install(daemon, [sup('task', 'Searching.', task('Find lunch'))],
                     {'worker': [done('First lunch'), done('Updated lunch')]})
    stops = forbid_stop(supervisor, monkeypatch)

    def update():
        try:
            update_events.extend(chat(daemon, CORRECTION, session=SESSION, timeout=10))
        except BaseException as exc:
            update_errors.append(exc)
        finally:
            update_done.set()

    with running_chat(daemon, 'Find lunch', release) as (initial, thread):
        assert entered.wait(5)
        completion = supervisor._tree_of[ids[0]].finished
        original_wait = completion.wait

        async def wait_for_finish():
            waiting_for_finish.set()
            return await original_wait()

        monkeypatch.setattr(completion, 'wait', wait_for_finish)
        route_update(script, ids[0])
        followup = threading.Thread(target=update)
        followup.start()
        try:
            assert waiting_for_finish.wait(5)
            assert not update_done.is_set()
            assert not stops
        finally:
            release.set()
            followup.join(5)
            assert not followup.is_alive()
    assert not update_errors, update_errors
    assert outcome(initial)['status'] == 'done'
    assert outcome(update_events)['status'] == 'done'
    assert not stops


def test_tools_not_loaded_retry_receives_latest_correction_with_original_context(daemon, monkeypatch):
    entered, release = threading.Event(), threading.Event()
    original = FakeRuntime.events
    worker_runs, received = [], []

    async def events(rt, run):
        if run.state['key'] == 'worker':
            worker_runs.append(run)
            if len(worker_runs) == 1:
                entered.set()
                await wait_gate(release)
                yield RuntimeEvent('final', {'text': done('Old answer without tools'), 'tools_seen': []})
                return
        async for event in original(rt, run):
            yield event

    async def steer(rt, run, message):
        received.append(message)

    monkeypatch.setattr(FakeRuntime, 'events', events)
    monkeypatch.setattr(FakeRuntime, 'continue_', steer)
    script = install(daemon, [sup('task', 'Searching.', task('Find lunch'))],
                     {'worker': done('Updated lunch with tools')})
    with running_chat(daemon, 'Find lunch', release) as (initial, thread):
        assert entered.wait(5)
        tree_id = worker_runs[0].task_id.rsplit('.', 1)[0]
        route_update(script, tree_id)
        chat(daemon, CORRECTION, session=SESSION, timeout=2)
        assert received and CORRECTION in received[0]
        release.set()
    contexts = [ctx for _, key, ctx, _ in script.contexts if key == 'worker']
    assert len(contexts) == 2 and CORRECTION not in contexts[0]
    assert CORRECTION in contexts[1]
    assert 'previous attempt at this exact step never loaded' in contexts[1]
    assert outcome(initial)['status'] == 'done'


@pytest.mark.parametrize("failure", ["start", "factory"])
def test_correction_followed_by_runtime_start_failure_uses_normal_recovery(daemon, monkeypatch, failure):
    supervisor = daemon.daemon.supervisor
    entered, release = threading.Event(), threading.Event()
    tree_ids, starts = [], []
    original_release, original_start = supervisor._release_resources, FakeRuntime.start

    async def cleanup(tree, span):
        if not entered.is_set():
            tree_ids.append(tree.id)
            entered.set()
            await wait_gate(release)
        return await original_release(tree, span)

    async def start(rt, worker, request, token):
        if worker.id.endswith('.worker'):
            starts.append(request.context)
            if len(starts) == 2 and failure == 'start':
                raise OSError('runtime failed to start')
        return await original_start(rt, worker, request, token)

    monkeypatch.setattr(supervisor, '_release_resources', cleanup)
    monkeypatch.setattr(FakeRuntime, 'start', start)
    # A third attempt can finish so the pre-fix stale-revision bug fails deterministically,
    # rather than spinning forever on a repeatedly unavailable runtime.
    script = install(daemon, [sup('task', 'Searching.', task('Find lunch'))],
                     {'worker': [done('Old lunch'), done('Unexpected repeated attempt')]})
    factory_calls = []
    original_factory = supervisor.make_runtime

    def factory(role, **kwargs):
        if role.startswith('worker:'):
            factory_calls.append(role)
            if len(factory_calls) == 2 and failure == 'factory':
                raise OSError('runtime failed to start: missing login')
        return original_factory(role, **kwargs)

    monkeypatch.setattr(supervisor, 'make_runtime', factory)
    with running_chat(daemon, 'Find lunch', release) as (initial, thread):
        assert entered.wait(5)
        route_update(script, tree_ids[0])
        chat(daemon, CORRECTION, session=SESSION, timeout=2)
        release.set()
    assert len(factory_calls) == 2
    assert len(starts) == (2 if failure == 'start' else 1)
    if failure == 'start':
        assert CORRECTION in starts[1]
    assert outcome(initial)['status'] == 'failed'
    dispatched = [ctx for _, key, ctx, _ in script.contexts if key == 'worker.dispatch']
    assert dispatched and 'runtime failed to start' in dispatched[-1]
    assert CORRECTION in dispatched[-1]


def test_correction_during_failed_plan_child_cleanup_reaches_root_worker(daemon, monkeypatch):
    from ferb.orchestrator.plans import Plan, Step

    supervisor = daemon.daemon.supervisor
    entered, release = threading.Event(), threading.Event()
    tree_ids = []
    original = supervisor._release_resources

    async def cleanup(member, span):
        if member.worker.id.endswith('.step.search') and not entered.is_set():
            tree_ids.append(member.id)
            entered.set()
            await wait_gate(release)
        return await original(member, span)

    monkeypatch.setattr(supervisor, '_release_resources', cleanup)
    spec = task('Find nearby egg curry with breads and compare prices')
    spec['plan'] = Plan(steps=[
        Step(id='search', kind='do', goal='Find nearby restaurants serving egg curry and breads'),
        Step(id='compare', kind='do', goal='Choose the cheapest suitable lunch', depends_on=['search']),
    ]).model_dump()
    failed = json.dumps({'status': 'failed', 'summary': 'Nearby location is unavailable',
                         'artifacts': [], 'evidence': []})
    script = install(daemon, [sup('task', 'Searching.', spec)],
                     {'step.search': failed, 'worker': done('Updated lunch request handled')})
    stops = forbid_stop(supervisor, monkeypatch)
    with running_chat(daemon, 'Find nearby egg curry with breads and compare prices', release) as (initial, thread):
        assert entered.wait(5)
        assert not supervisor._worker_runs
        route_update(script, tree_ids[0])
        chat(daemon, CORRECTION, session=SESSION, timeout=2)
        release.set()
    root_contexts = [ctx for _, key, ctx, _ in script.contexts if key == 'worker']
    assert len(root_contexts) == 1, 'failed child cleanup must not discard its accepted correction'
    assert CORRECTION in root_contexts[0]
    assert 'Nearby location is unavailable' in root_contexts[0]
    assert 'blocked' in root_contexts[0] and 'compare' in root_contexts[0]
    assert not any(key == 'step.compare' for _, key, _, _ in script.contexts)
    assert outcome(initial)['status'] == 'done'
    assert not stops
