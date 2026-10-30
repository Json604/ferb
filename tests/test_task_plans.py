"""D30/P15: session-file fan-out, complete notes and restart without replaying completed work."""
import asyncio
from pathlib import Path

import pytest

from ferb.orchestrator.plans import EventMatch, Plan, Step, inventory, problems
from ferb.orchestrator.plan_store import PlanStore
from ferb.orchestrator.plan_executor import PlanExecutor
from ferb.store.db import Database


def test_input_lists_cover_every_inventoried_input_once_and_each_fits(tmp_path):
    """Owner decision 5: map stays as an ability (a do step with an input list; independent steps run at once) and the
    plan check that every input is covered exactly once stays."""
    for name in ('session-a.json', 'session-b.json'):
        (tmp_path / name).write_text('owner controlled session')
    inputs = asyncio.run(inventory([str(tmp_path)]))
    a, b = [i['path'] for i in inputs]
    plan = Plan(steps=[Step(id='read-a', kind='do', goal='Read session a', inputs=[a])], input_paths=[str(tmp_path)])
    assert any('unassigned' in e for e in problems(plan, inputs, context_tokens=1000))
    plan.steps.append(Step(id='read-b', kind='do', goal='Read session b', inputs=[b]))
    assert not problems(plan, inputs, context_tokens=1000)
    assert any('context' in e for e in problems(plan, inputs, context_tokens=1))
    plan.steps[1].inputs = [a, b]
    assert any('more than once' in e for e in problems(plan, inputs, context_tokens=1000))
    plan.steps[1].inputs = [b, str(tmp_path / 'not-inventoried.json')]
    assert any('absent from inventory' in e for e in problems(plan, inputs, context_tokens=1000))


def test_plans_saved_in_the_old_step_kinds_still_load_as_do_steps():
    """Stored plans (task_plans) and old-vocabulary steps: reduce is a do step, map a do step with its batches' inputs,
    loop a do step with a repeat rule carrying its end condition and event."""
    plan = Plan.model_validate({'steps': [
        {'id': 'read', 'kind': 'map', 'goal': 'Read', 'batches': [['a', 'b'], ['c']]},
        {'id': 'rank', 'kind': 'reduce', 'goal': 'Rank', 'depends_on': ['read']},
        {'id': 'keep', 'kind': 'loop', 'goal': 'Keep it playing', 'end_when': 'the song ends', 'event': 'ci.done'}]})
    read, rank, keep = plan.steps
    assert (read.kind, read.inputs, read.repeat) == ('do', ['a', 'b', 'c'], None)
    assert (rank.kind, rank.depends_on, rank.loops) == ('do', ['read'], False)
    assert keep.kind == 'do' and keep.loops and keep.end_when == 'the song ends' and keep.wakes_on == 'ci.done'
    assert 'batches' not in read.model_dump() and Plan.model_validate(plan.model_dump()) == plan


def test_only_a_do_step_repeats_or_takes_inputs():
    plan = Plan(steps=[Step(id='w', kind='wait', goal='Wait', event='x', inputs=['a'], repeat={'end_when': 'y'})])
    found = problems(plan, [], context_tokens=100)
    assert any('only a do step repeats' in e for e in found) and any('input list' in e for e in found)


def test_plan_rejects_cycles_and_missing_dependencies():
    plan = Plan(steps=[Step(id='one', kind='do', goal='Read', depends_on=['two']),
                      Step(id='two', kind='do', goal='Rank', depends_on=['one'])])
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
        engine.store.event('ci.finished', {'repository': 'other', 'state': 'passed'}); engine.wake()
        await asyncio.sleep(0)
        assert not job.done()
        job.cancel()
        with pytest.raises(asyncio.CancelledError):
            await job
        assert store.steps('tree')['wait']['status'] == 'interrupted'     # the tree's finish marks the plan itself
        restarted = PlanExecutor(PlanStore(db, lambda value: value), worker, lambda: False)
        restarted.store.event('ci.finished', {'repository': 'FERB', 'state': 'passed'}); restarted.wake()
        result = await asyncio.wait_for(restarted.run('tree'), 2)
        assert result['status'] == 'done'
    asyncio.run(run())
    assert seen == ['check']
    db.close()


def test_plan_steps_use_normal_workers_and_the_last_step_is_checked_against_the_task(daemon):
    """Decision 19: no final worker; the last step's check judges the task's success conditions."""
    from tests.conftest import chat, done, install, outcome, sup, task
    spec = task('Read sessions and rank them')
    spec['plan'] = Plan(steps=[Step(id='read', kind='do', goal='Read the session notes'),
                              Step(id='rank', kind='do', goal='Rank the read notes', depends_on=['read'])]).model_dump()
    script = install(daemon, [sup('task', 'Reading your sessions.', spec)],
                     {'step.read': done('Session notes read.'), 'step.rank': done('Sessions ranked.')})
    result = outcome(chat(daemon, 'Read sessions and rank them'))
    assert result['verified'] and result['answer'].startswith('Sessions ranked.')
    roles = [role for _, role, _, _ in script.contexts]
    assert roles.index('step.read') < roles.index('step.rank') < roles.index('step.rank.verify')
    assert 'worker' not in roles
    async def stored():
        return daemon.daemon.supervisor.plans.steps(result['task_id'])
    states = daemon.run(stored())
    assert all(item['status'] == 'done' for item in states.values())
    assert Path(states['read']['output']['notes_file']).is_file()


def test_big_job_plan_is_stated_and_starts_without_waiting(daemon):
    """D24 as amended (L3, spec §5): the plan and its estimates are stated in the plan line; nothing waits for a Yes."""
    from tests.conftest import chat, done, install, sup, task
    spec = task('Rank all coding sessions')
    spec['plan'] = Plan(steps=[Step(id='read', kind='do', goal='Read sessions')], big_job=True,
                        time_estimate='About 20 minutes', cost_estimate='Subscription usage; no separate estimate').model_dump()
    script = install(daemon, [sup('task', 'Here is the plan.', spec)],
                     {'step.read': done('Sessions read.'), 'worker': done('Checked.')})
    events = chat(daemon, 'Rank all coding sessions')
    plan = next(e for e in events if e['type'] == 'plan')
    assert '20 minutes' in plan['text'] and 'Read sessions' in plan['text']
    assert not daemon.daemon.state.approvals.pending()
    assert any(role.startswith('step.') for _, role, _, _ in script.contexts)


def test_independent_steps_run_together_and_a_step_that_did_not_finish_blocks_its_dependents(tmp_path):
    db = Database(tmp_path / 'state.db'); db.migrate()
    store = PlanStore(db, lambda x: x)
    plan = Plan(steps=[Step(id='a', kind='do', goal='Read a', inputs=['a']),
                      Step(id='b', kind='do', goal='Read b', inputs=['b']),
                      Step(id='rank', kind='do', goal='Rank', depends_on=['a', 'b'])])
    store.create('tree', plan, {}, 'chat')
    seen = []
    async def run():
        barrier = asyncio.Event()
        async def worker(step, inputs, prior):
            seen.append((step.id, inputs))
            if len(seen) == 2: barrier.set()
            await barrier.wait()
            return {'status': 'done' if step.id == 'a' else 'blocked', 'summary': 'notes missing'}
        result = await PlanExecutor(store, worker, lambda: False).run('tree')
        assert result['status'] == 'blocked'
    asyncio.run(asyncio.wait_for(run(), 5))
    assert sorted(seen) == [('a', ['a']), ('b', ['b'])]
    db.close()


@pytest.mark.parametrize('status', ['needs_user', 'failed', 'cancelled'])
def test_a_step_that_ended_without_finishing_is_never_launched_again(tmp_path, status):
    """The endless relaunch (notes/orchestration.md): a step that needed the owner was relaunched forever. A step that
    ended (needing the owner in a run nobody attends, failed, cancelled) runs again only after the owner's continue."""
    db = Database(tmp_path / 'state.db'); db.migrate()
    store = PlanStore(db, lambda x: x)
    store.create('tree', Plan(steps=[Step(id='ask', kind='do', goal='Find it')]), {}, 'chat')
    runs = []
    async def worker(step, inputs, prior):
        runs.append(step.id)
        return {'status': status, 'summary': 'Which one?'}
    result = asyncio.run(asyncio.wait_for(PlanExecutor(store, worker, lambda: False).run('tree'), 2))
    assert runs == ['ask'] and result['status'] == ('needs_user' if status == 'needs_user' else 'blocked')
    db.close()


def test_a_step_done_but_unconfirmed_lets_its_dependents_run(tmp_path):
    db = Database(tmp_path / 'state.db'); db.migrate()
    store = PlanStore(db, lambda x: x)
    store.create('tree', Plan(steps=[Step(id='one', kind='do', goal='One'),
                                     Step(id='two', kind='do', goal='Two', depends_on=['one'])]), {}, 'chat')
    async def worker(step, inputs, prior):
        return {'status': 'done_unverified' if step.id == 'one' else 'done', 'summary': step.goal}
    result = asyncio.run(asyncio.wait_for(PlanExecutor(store, worker, lambda: False).run('tree'), 2))
    assert result['status'] == 'done' and store.steps('tree')['one']['status'] == 'done_unverified'
    db.close()


def test_explicit_reopen_preserves_done_steps_notes_and_wait_cursor(tmp_path):
    db = Database(tmp_path / 'state.db'); db.migrate()
    store = PlanStore(db, lambda x: x)
    plan = Plan(steps=[Step(id='read', kind='do', goal='Read'), Step(id='send', kind='do', goal='Send', depends_on=['read'])])
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


def test_chat_continue_reuses_saved_plan_tree_and_does_not_repeat_completed_step(daemon):
    import json
    from tests.conftest import chat, done, install, outcome, sup, task
    spec = task('Read session then verify report')
    spec['plan'] = Plan(steps=[Step(id='read', kind='do', goal='Read the session')]).model_dump()
    first_script = install(daemon, [sup('task', 'Reading.', spec)],
                           {'step.read': done('Read once.'), 'worker': done('Report checked.')})
    first = outcome(chat(daemon, 'Read session then verify report'))
    continuation = json.dumps({'kind': 'reply', 'text': 'Continuing the saved plan.',
                              'control': {'task_id': first['task_id'], 'action': 'continue', 'standing_sends': 'unchanged', 'supersedes': []}})
    second_script = install(daemon, [continuation], {'worker': done('Report checked again.')})
    second = outcome(chat(daemon, 'Continue that saved plan'))
    assert second['task_id'] == first['task_id']
    assert second['verified']
    assert sum(role == 'step.read' for _, role, _, _ in first_script.contexts) == 1
    assert not any(role == 'step.read' for _, role, _, _ in second_script.contexts)


def test_inventory_leaves_out_what_macos_wont_let_ferb_look_at(tmp_path):
    """Live 2026-10-24 (request 7429fb64, "ok start cleaning"): the planner listed /private/var/folders, where macOS
    won't even let FERB look at some entries (EPERM). The walk's PermissionError ended the owner's turn with "something
    went wrong on my side". An entry FERB can't look at is left out, as a protected file already is."""
    tree = tmp_path / 'tree'
    tree.mkdir()
    (tree / 'a.txt').write_text('hello')
    locked = tree / 'locked'
    locked.mkdir()
    (locked / 'b.txt').write_text('secret')
    locked.chmod(0o600)                      # its names can be listed, but nothing in it can be looked at
    try:
        facts = asyncio.run(inventory([str(tree), str(locked / 'b.txt')]))
    finally:
        locked.chmod(0o700)
    assert [f['path'] for f in facts] == [str((tree / 'a.txt').resolve())]
