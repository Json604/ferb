"""D30/P15: session-file fan-out, complete notes and restart without replaying completed work."""
import asyncio
from pathlib import Path

import pytest

from ferb.orchestrator.plans import EventMatch, Plan, Step, inventory, problems
from ferb.orchestrator.plan_store import PlanStore
from ferb.orchestrator.plan_executor import PlanExecutor
from ferb.store.db import Database


def test_map_requires_every_session_once_and_a_batch_that_fits(tmp_path):
    for name in ('session-a.json', 'session-b.json'):
        (tmp_path / name).write_text('owner controlled session')
    inputs = inventory([str(tmp_path)])
    a, b = [i['path'] for i in inputs]
    plan = Plan(steps=[Step(id='read', kind='map', goal='Read every session', batches=[[a]])],
                input_paths=[str(tmp_path)])
    assert any('unassigned' in e for e in problems(plan, inputs, context_tokens=1000))
    plan.steps[0].batches = [[a], [b]]
    assert not problems(plan, inputs, context_tokens=1000)
    assert any('context' in e for e in problems(plan, inputs, context_tokens=1))
    plan.steps[0].batches = [[a, b], [a]]
    assert any('more than once' in e for e in problems(plan, inputs, context_tokens=1000))


def test_plan_rejects_cycles_missing_dependencies_and_reduce_without_sources():
    plan = Plan(steps=[Step(id='one', kind='do', goal='Read', depends_on=['two']),
                      Step(id='two', kind='reduce', goal='Rank', depends_on=['one'])])
    assert any('cycle' in e for e in problems(plan, [], context_tokens=100))
    plan.steps[0].depends_on = ['missing']
    assert any('missing' in e for e in problems(plan, [], context_tokens=100))


def test_restart_retains_completed_steps_and_durable_wait(tmp_path):
    db = Database(tmp_path / 'state.db'); db.migrate()
    store = PlanStore(db, lambda x: x)
    plan = Plan(steps=[Step(id='read', kind='do', goal='Read session'),
                      Step(id='wait', kind='wait', goal='Wait for CI', depends_on=['read'], event='ci.finished',
                           event_match=[EventMatch(field='repository', value='FERB')]),
                      Step(id='check', kind='do', goal='Check CI', depends_on=['wait'])])
    store.create('tree', plan, {}, 'chat')
    store.finish('tree', 'read', {'status': 'done', 'summary': 'read'})
    seen = []
    async def worker(step, inputs, prior):
        seen.append(step.id)
        return {'status': 'done', 'summary': step.goal}
    async def run():
        engine = PlanExecutor(store, worker, lambda: False)
        job = asyncio.create_task(engine.run('tree'))
        while store.steps('tree')['wait']['status'] != 'waiting':
            await asyncio.sleep(0)
        assert store.steps('tree')['wait']['status'] == 'waiting'
        engine.notify('ci.finished', {'repository': 'other', 'state': 'passed'})
        await asyncio.sleep(0)
        assert not job.done()
        job.cancel()
        with pytest.raises(asyncio.CancelledError):
            await job
        assert store.load('tree')['status'] == 'suspended'
        restarted = PlanExecutor(PlanStore(db, lambda value: value), worker, lambda: False)
        restarted.notify('ci.finished', {'repository': 'FERB', 'state': 'passed'})
        result = await asyncio.wait_for(restarted.run('tree'), 2)
        assert result['status'] == 'done'
    asyncio.run(run())
    assert seen == ['check']
    db.close()


def test_plan_steps_use_normal_workers_and_final_inspection(daemon):
    from tests.conftest import chat, done, install, outcome, sup, task
    spec = task('Read sessions and rank them')
    spec['plan'] = Plan(steps=[Step(id='read', kind='do', goal='Read the session notes'),
                              Step(id='rank', kind='reduce', goal='Rank the read notes', depends_on=['read'])]).model_dump()
    script = install(daemon, [sup('task', 'Reading your sessions.', spec)],
                     {'step.read': done('Session notes read.'), 'step.rank': done('Sessions ranked.'),
                      'worker': done('Ranking checked.')})
    result = outcome(chat(daemon, 'Read sessions and rank them'))
    assert result['verified']
    roles = [role for _, role, _, _ in script.contexts]
    assert roles.index('step.read') < roles.index('step.rank') < roles.index('worker') < roles.index('worker.verify')
    async def stored():
        return daemon.daemon.supervisor.plans.steps(result['task_id'])
    states = daemon.run(stored())
    assert all(item['status'] == 'done' for item in states.values())
    assert Path(states['read']['output']['notes_file']).is_file()


def test_big_job_waits_for_owner_before_any_step(daemon):
    from tests.conftest import chat, install, sup, task
    spec = task('Rank all coding sessions')
    spec['plan'] = Plan(steps=[Step(id='read', kind='do', goal='Read sessions')], big_job=True,
                        time_estimate='About 20 minutes', cost_estimate='Subscription usage; no separate estimate').model_dump()
    script = install(daemon, [sup('task', 'Here is the plan.', spec)], {})
    async def decline():
        while not daemon.daemon.state.approvals.pending():
            await asyncio.sleep(0)
        question = daemon.daemon.state.approvals.pending()[0]
        assert question.capability == 'plan.ask'
        assert '20 minutes' in question.reason
        assert not any(role.startswith('step.') for _, role, _, _ in script.contexts)
        daemon.daemon.state.approvals.resolve(question.id, True, choice=2)
    import asyncio as aio
    answering = aio.run_coroutine_threadsafe(decline(), daemon.loop)
    chat(daemon, 'Rank all coding sessions')
    answering.result(timeout=2)
    assert not any(role.startswith('step.') for _, role, _, _ in script.contexts)


def test_independent_steps_run_together_and_missing_map_notes_block_reduce(tmp_path):
    db = Database(tmp_path / 'state.db'); db.migrate()
    store = PlanStore(db, lambda x: x)
    plan = Plan(steps=[Step(id='map', kind='map', goal='Read sessions', batches=[['a'], ['b']]),
                      Step(id='rank', kind='reduce', goal='Rank', depends_on=['map'])])
    store.create('tree', plan, {}, 'chat')
    seen = []
    async def run():
        barrier = asyncio.Event()
        async def worker(step, inputs, prior):
            seen.append(step.id)
            if len(seen) == 2: barrier.set()
            await barrier.wait()
            return {'status': 'done', 'summary': 'claimed done', 'input_notes': []}
        result = await PlanExecutor(store, worker, lambda: False).run('tree')
        assert result['status'] == 'blocked'
    asyncio.run(run())
    assert len(seen) == 2 and 'rank' not in seen
    db.close()


def test_explicit_reopen_preserves_done_steps_notes_and_wait_cursor(tmp_path):
    db = Database(tmp_path / 'state.db'); db.migrate()
    store = PlanStore(db, lambda x: x)
    plan = Plan(steps=[Step(id='read', goal='Read'), Step(id='send', goal='Send', depends_on=['read'])])
    store.create('tree', plan, {'words': 'original'}, 'chat')
    store.finish('tree', 'read', {'status': 'done', 'summary': 'saved notes'})
    store.finish('tree', 'send', {'status': 'blocked', 'event_cursor': 7, 'summary': 'unknown delivery'})
    store.set_status('tree', 'failed')
    store.reopen('tree')
    states = store.steps('tree')
    assert states['read']['status'] == 'done'
    assert states['send']['status'] == 'pending'
    assert states['send']['output']['event_cursor'] == 7
    assert states['send']['output']['summary'] == 'unknown delivery'
    assert store.load('tree')['status'] == 'running'
    db.close()
