"""The owner's 0.22 contract: agents decide; the gateway executes and records."""

from ferb.core.types import Identity
from tests.test_gateway import FakeCap, call, request, setup
from tests.test_supervisor import chat, cond, done, install, outcome, sup, task
import json


def test_execution_does_not_run_per_action_policy_or_remove_tools_after_reading(daemon):
    read = FakeCap('fake.read', 'read', {'browser:read'}, reads=(('web', 'https://example.com'),))
    send = FakeCap('fake.send', 'send', {'browser:act'}, site='example.com')
    shell = FakeCap('fake.shell', 'run_shell', {'shell'})
    grant, _ = setup(daemon, {'browser:read', 'browser:act', 'shell'}, request('send the result'), [read, send, shell])
    def obsolete_gate(action):
        raise AssertionError('per-action policy must not decide whether a requested action is allowed')
    daemon.daemon.engine.evaluate = obsolete_gate
    assert not call(daemon, grant, read.name)['is_error']
    assert shell.name in {t['name'] for t in daemon.daemon.gateway.tools_for(grant)}
    assert not call(daemon, grant, send.name)['is_error']
    assert send.calls == 1
    assert not daemon.daemon.engine.approvals.pending()
    assert not daemon.daemon.engine.holds.pending()


def test_registry_browser_family_is_not_withheld_by_task_identity(daemon):
    cap = FakeCap('ferb_browser.probe', 'read', {'identity:browse'})
    cap.identities = {Identity.FERB}
    grant, _ = setup(daemon, {'identity:browse'}, request('research this'), [cap], Identity.OWNER)
    assert not call(daemon, grant, cap.name)['is_error']


def test_setup_files_and_shell_are_not_restricted_by_request_wording(daemon, tmp_path):
    path = tmp_path / 'assignment.txt'
    path.write_text('assignment context')
    grant, _ = setup(daemon, {'fs:read', 'shell'}, request('read assignment and use the coding session'), [])
    assert not call(daemon, grant, 'fs.read', path=str(path))['is_error']
    result = call(daemon, grant, 'shell.run', command="printf 'launchctl osascript ~/.codex'", cwd=str(tmp_path))
    assert not result['is_error'], result


def test_orchestrator_answers_already_authorized_final_step_without_owner_prompt(daemon):
    from tests.test_supervisor import tool
    answers = []
    def worker(token):
        answers.append(tool(daemon.sock, token, 'owner.ask', question='The deck is ready. Send it to Rahul?',
                            options=['Send', 'Review first']))
        return done('Sent.')
    decision = json.dumps({'ask_owner':False, 'answer':'Send it; the original request authorizes sending.', 'question':''})
    install(daemon, [sup('task','Sending',task('Send Rahul the deck','owner'))],
            {'worker':worker, 'worker.question':decision})
    out = outcome(chat(daemon, 'Send Rahul the deck'))
    assert out['status'] == 'done'
    assert answers[0]['content']['answered_by'] == 'orchestrator'
    assert not daemon.daemon.engine.approvals.pending()


def test_scaler_gmail_keeps_setup_tools_and_orchestrator_switches_worker(daemon):
    s = daemon.daemon.supervisor
    seen = []
    report = json.dumps({'status':'blocked','summary':'This worker cannot control the coding session.',
                         'artifacts':[], 'evidence':['Need the coding worker runtime.']})
    def completed(token):
        seen.append(daemon.daemon.tokens.validate(token).scopes)
        return done('Assignment prepared; no submission requested.')
    choice = json.dumps({'action':'retry','worker_type':'codex','instructions':'Use the coding session capability.',
                         'question':''})
    install(daemon, [sup('task','Reading',task('check scaler gmail then complete the assignment','owner'))],
            {'worker':[report, completed], 'worker.dispatch': choice})
    events = chat(daemon, 'check in scaler gmail about FMI assignment 2 and use Codex to complete it')
    async def errors():
        return [e.data for e in daemon.daemon.journal.entries() if e.kind == 'chat.failed']
    assert any(e['type'] == 'outcome' for e in events), daemon.run(errors())
    out = outcome(events)
    assert out['status'] == 'done', out
    assert {'browser:read', 'browser:act', 'shell'} <= seen[0]


def test_recovery_preserves_artifacts_and_reports_uncertain_actions(daemon):
    from tests.test_supervisor import tool
    cap = FakeCap('fake.send', 'send', {'browser:act'})
    async def lost_ack(args, ctx):
        raise ConnectionError('lost acknowledgement')
    cap.run = lost_ack
    daemon.daemon.gateway.caps[cap.name] = cap
    def first(token):
        assert tool(daemon.sock, token, cap.name)['is_error']
        return json.dumps({'status':'blocked', 'summary':'Connection dropped.',
                           'artifacts':['/tmp/prepared-deck.pdf'], 'evidence':['draft prepared']})
    choice = json.dumps({'action':'retry', 'worker_type':'default',
                         'instructions':'Inspect whether the send happened before doing anything else.', 'question':''})
    script = install(daemon, [sup('task','Sending',task('Send deck','owner'))],
                     {'worker':[first, done('Delivery confirmed by inspection.')], 'worker.dispatch':choice})
    out = outcome(chat(daemon, 'Send the prepared deck'))
    assert '/tmp/prepared-deck.pdf' in out['artifacts']
    retry_context = [ctx for _, key, ctx, _ in script.contexts if key == 'worker'][-1]
    assert 'fake.send' in retry_context and 'unknown' in retry_context
    assert '/tmp/prepared-deck.pdf' in retry_context
    rows = daemon.daemon.daily_activity.search('fake.send')
    assert any(row['status'] == 'unknown' for row in rows)


def test_invalid_recovery_output_finishes_truthfully_and_cleans_resources(daemon):
    cleaned = []
    async def cleanup(task_id):
        cleaned.append(task_id)
    daemon.daemon.supervisor.on_task_end = cleanup
    blocked = json.dumps({'status':'blocked','summary':'Disconnected.', 'artifacts':[], 'evidence':[]})
    install(daemon, [sup('task','Starting',task())], {'worker':blocked, 'worker.dispatch':'{}'})
    out = outcome(chat(daemon, 'Do the task'))
    assert out['status'] == 'failed'
    assert cleaned and not daemon.daemon.supervisor._tree_of
    assert not daemon.daemon.supervisor._active


def test_each_turn_searches_memory_activity_and_registry(daemon):
    recalled = []
    daemon.daemon.supervisor.recall = lambda words: recalled.append(words) or 'Remembered FMI project'
    daemon.call_soon(daemon.daemon._publish, 'action.observed',
                     {'what':'browser.read', 'where':'FMI assignment in Gmail', 'actor':'ferb', 'status':'done'})
    daemon.call_soon(daemon.daemon._publish, 'action.observed',
                     {'what':'mac.scroll', 'where':'unrelated chess game', 'actor':'ferb', 'status':'done'})
    script = install(daemon, [sup('reply','Here is the context.'), sup('reply','More context.')])
    chat(daemon, 'FMI assignment browser')
    chat(daemon, 'FMI browser progress')
    assert len(recalled) == 2
    for _, _, ctx, _ in script.contexts:
        assert 'Remembered FMI project' in ctx and 'FMI assignment in Gmail' in ctx
        assert 'Relevant installed actions:' in ctx and 'unrelated chess game' not in ctx


def test_repeated_calls_do_not_have_a_hard_action_limit(daemon):
    cap = FakeCap('fake.read', 'read', {'browser:read'})
    daemon.daemon.gateway.caps[cap.name] = cap
    from tests.test_supervisor import tool
    def worker(token):
        for _ in range(1005):
            assert not tool(daemon.sock, token, cap.name)['is_error']
        return done('All 1005 actions complete.')
    install(daemon, [sup('task','Working',task())], {'worker':worker})
    assert outcome(chat(daemon, 'Read all 1005 entries'))['status'] == 'done'
    assert cap.calls == 1005


def test_worker_runtimes_have_no_task_duration_or_turn_cap(daemon):
    from ferb.runtimes.claude_sdk import ClaudeRuntime
    assert ClaudeRuntime('worker', None, '/tmp/unused').max_turns is None
    rt = daemon.daemon._make_runtime('worker:codex', mode='worker', tool_names=[], schema={})
    assert rt.timeout_s is None


def test_registry_search_returns_available_capability_details(daemon):
    grant, _ = setup(daemon, set(), request('discover coding tools'), [])
    r = call(daemon, grant, 'actions.search', query='shell command', limit=2)
    assert not r['is_error'] and 0 < len(r['content']['actions']) <= 2
    assert all('scopes' in item and 'description' in item for item in r['content']['actions'])


def test_unrequested_final_send_waits_until_result_is_prepared_and_owner_decides(daemon):
    from ferb.cli.client import Client
    from tests.test_supervisor import tool
    sent = []
    def worker(token):
        result = tool(daemon.sock, token, 'fs.write', path='deck.md', content='Ready for Rahul')
        assert not result['is_error']
        answer = tool(daemon.sock, token, 'owner.ask', question='deck.md is ready. Send it to Rahul or review it?',
                      options=['Review first', 'Send'])
        if answer['content'].get('answer') == 'Send':
            sent.append(True)
        return done('Deck prepared for your review.')
    decision = json.dumps({'ask_owner':True, 'answer':'', 'question':'deck.md is ready. Send it to Rahul or review it?'})
    install(daemon, [sup('task','Preparing the deck.',task('Get deck ready for Rahul'))],
            {'worker':worker, 'worker.question':decision})
    c = Client(daemon.sock)
    asked = []
    try:
        for event in c.stream('chat.send', {'session_id':'s1', 'text':'Get the deck ready for Rahul'}):
            if event.get('topic') == 'approval.requested':
                assert (daemon.daemon.home / 'workspace/deck.md').exists()
                asked.append(event)
                answer_client = Client(daemon.sock)
                try:
                    answer_client.call('chat.answer', {'approval_id':event['data']['approval_id'], 'text':'1'})
                finally:
                    answer_client.close()
            if event.get('type') == 'outcome':
                assert event['status'] == 'done'
                break
    finally:
        c.close()
    assert len(asked) == 1 and not sent


def test_failed_result_check_returns_to_orchestrator_for_repair(daemon):
    t = task('prepare deck', conds=(cond('deck meets the brief'),))
    bad = json.dumps({'results':[{'index':0, 'met':False, 'evidence':'Missing requested slide.'}]})
    good = json.dumps({'results':[{'index':0, 'met':True, 'evidence':'Requested slide is present.'}]})
    recovery = json.dumps({'action':'retry', 'worker_type':'default', 'instructions':'Add the missing slide.', 'question':''})
    script = install(daemon, [sup('task','Preparing',t)],
                     {'worker':[done('Draft ready.'), done('Added slide.')],
                      'worker.verify':[bad,good], 'worker.dispatch':recovery})
    out = outcome(chat(daemon, 'Prepare my deck'))
    assert out['status'] == 'done' and out['verified']
    assert len([key for _,key,_,_ in script.contexts if key == 'worker']) == 2


def test_owner_stop_cancels_orchestrator_recovery(daemon):
    import time
    from ferb.cli.client import Client
    from tests.test_supervisor import STALL
    report = json.dumps({'status':'blocked', 'summary':'Connection lost.', 'artifacts':[], 'evidence':[]})
    script = install(daemon, [sup('task','Working',task())], {'worker':report, 'worker.dispatch':STALL})
    client = Client(daemon.sock)
    try:
        stream = client.stream('chat.send', {'session_id':'s1', 'text':'Prepare the report'})
        task_id = next(stream)['task_id']
        for _ in range(100):
            if any(key == 'worker.dispatch' for _,key,_,_ in script.contexts):
                break
            time.sleep(.01)
        stop = Client(daemon.sock)
        try:
            stop.call('task.stop', {'task_ids':[task_id]})
        finally:
            stop.close()
        assert outcome(list(stream))['status'] == 'cancelled'
        assert not daemon.daemon.supervisor._active
    finally:
        client.close()


def test_context_failure_after_planning_still_finishes_and_releases_task(daemon):
    cleanups = []
    calls = []
    def context(words):
        calls.append(words)
        if len(calls) > 1:
            raise OSError('context storage temporarily unavailable')
        return ''
    async def cleanup(task_id):
        cleanups.append(task_id)
    supervisor = daemon.daemon.supervisor
    supervisor.relevant_context = context
    supervisor.on_task_end = cleanup
    install(daemon, [sup('task','Starting',task())], {'worker':done()})
    out = outcome(chat(daemon, 'Prepare the report'))
    assert out['status'] == 'failed'
    assert cleanups and not supervisor._tree_of and not supervisor._active


def test_clarification_preserves_speakers_without_a_yes_phrase_rule(daemon):
    captured = []
    def worker(token):
        grant = daemon.daemon.tokens.validate(token)
        record = daemon.daemon.tasks.get(grant.task_id)
        captured.append(daemon.daemon.engine.requests.get(record.task.request_id).words)
        return done('Inspected the requested page.')
    install(daemon, [sup('clarify', 'Use your Brave profile for the page?'),
                     sup('task', 'Inspecting', task('Read the Brave page', 'owner'))], {'worker':worker})
    chat(daemon, 'Read the page')
    assert outcome(chat(daemon, 'yes'))['status'] == 'done'
    assert 'Use your Brave profile' in captured[0]
    assert 'Use my Brave profile' not in captured[0]
    assert 'Earlier FERB question' in captured[0]
