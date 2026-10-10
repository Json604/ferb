"""Regressions from the 4 October study-material trace, with disposable scripted task evidence.

These establish information delivery and truthful reporting contracts, not model teaching quality.
"""

import json

import pytest

from tests.conftest import chat, cond, done, install, outcome, sup, task


def test_task_observer_hooks_wrap_worker_and_cleanup(daemon):
    observed = []

    async def start(task_id):
        observed.append(("start", task_id))

    async def finish(task_id):
        observed.append(("finish", task_id))

    def worker(_):
        assert [kind for kind, _ in observed] == ["start"]
        return done("Created")

    daemon.daemon.supervisor.on_task_start = start
    daemon.daemon.supervisor.on_task_finish = finish
    install(daemon, [sup("task", "Working", task())], {"worker": worker})
    result = outcome(chat(daemon, "Create the requested file"))
    assert observed == [("start", result["task_id"]), ("finish", result["task_id"])]


def test_checker_receives_full_learning_request_not_only_planner_goal(daemon):
    request = (
        "Create three separate one-week study files. I have not studied anything in any subject. "
        "Cover each and every part of the syllabus in easy explanations, with enough material "
        "that I can actually study and pass with good numbers. " + "Additional syllabus detail. " * 100
    ).strip()
    script = install(daemon, [sup("task", "Preparing the materials", task("Create three study files"))],
                     {"worker": done("Created three files")})
    chat(daemon, request)
    worker_context = next(ctx for _, key, ctx, _ in script.contexts if key == "worker")
    checker_context = next(ctx for _, key, ctx, _ in script.contexts if key == "worker.verify")
    assert request in worker_context
    assert request in checker_context


def test_pasted_syllabus_reaches_worker_and_checker_as_data(daemon):
    source = "SYLLABUS-TOPIC: " + "Derivations and exercises. " * 300
    request = "Teach every topic in this syllabus from scratch:\n```\n" + source + "\n```"
    script = install(daemon, [sup("task", "Preparing", task("Create a complete study guide"))],
                     {"worker": done("Created a guide")})
    chat(daemon, request)
    for role in ("worker", "worker.verify"):
        context = next(ctx for _, key, ctx, _ in script.contexts if key == role)
        assert source in context
        assert "<untrusted-data" in context


def test_checker_receives_successful_intake_memory_receipt(daemon):
    receipt = "remembered (memory-study-123): The current syllabus is https://example.test/syllabus."
    daemon.daemon.supervisor.remember = lambda *_: [receipt]
    script = install(daemon, [sup("task", "Preparing", task(conds=(cond("Syllabus link is stored durably"),)))],
                     {"worker": done("Created the files")})
    events = chat(daemon, "Remember https://example.test/syllabus and create study files")
    assert any(event.get("text") == receipt for event in events)
    checker_context = next(ctx for _, key, ctx, _ in script.contexts if key == "worker.verify")
    assert receipt in checker_context


@pytest.mark.parametrize("met", [False, None])
def test_unresolved_conditions_precede_facts_without_success_claim(daemon, met):
    condition = "Every syllabus topic has enough explanation and worked practice to study from scratch."
    evidence = "Only headings and term-presence matches were checked; teaching depth remains unverified."
    verdict = json.dumps({"results": [{"index": 0, "met": met, "evidence": evidence}]})
    unchecked_claim = "I completed every requirement and all three files are ready to study from."
    finish = json.dumps({"action": "finish", "worker_type": "default", "instructions": unchecked_claim,
                         "question": ""})
    install(daemon, [sup("task", "Preparing", task(conds=(cond(condition),)))],
            {"worker": done(unchecked_claim), "worker.verify": verdict, "worker.dispatch": finish})
    result = outcome(chat(daemon, "Write complete beginner study material"))
    # V4, rule 6: a finish without the orchestrator's "unconfirmed" judgment is a failure, whatever the check said.
    assert result["status"] == "failed" and not result["verified"]
    assert condition in result["answer"] and evidence in result["answer"]
    assert unchecked_claim not in result["answer"]


def test_cleanup_reviewer_cannot_resurrect_rejected_success_prose(daemon):
    condition = "The attachment is the requested local file, including matching content."
    verdict = json.dumps({"results": [{"index": 0, "met": False,
                                       "evidence": "Source and content were never established."}]})
    unchecked_claim = "I attached and verified the correct file."

    async def cleanup(task_id):
        return [{"tab": "owned-test-tab", "closed": True, "released": True}]

    daemon.daemon.supervisor.on_task_end = cleanup
    install(daemon, [sup("task", "Checking", task(conds=(cond(condition),)))],
            {"worker": done(unchecked_claim), "worker.verify": verdict,
             "worker.resources": done(unchecked_claim)})
    result = outcome(chat(daemon, "Attach the exact local file"))
    assert result["status"] == "failed" and not result["verified"]
    assert condition in result["answer"]
    assert unchecked_claim not in result["answer"]


def test_recent_history_does_not_silently_cut_owner_requirement(daemon):
    request = "Context detail. " * 60 + "The final study guide must teach complete derivations."
    install(daemon, [sup("reply", "Remembered"), sup("reply", "Continuing")])
    chat(daemon, request)

    async def context():
        return daemon.daemon.supervisor.recent(conversation="s1")[0]

    recent = daemon.run(context())
    assert request in recent or ("omitted" in recent.lower() and "chat.history" in recent)


def test_task_evidence_reads_own_root_facts_and_full_context_not_other_tasks(daemon):
    from tests.conftest import tool

    def worker(token):
        grant = daemon.daemon.tokens.validate(token)

        async def record():
            root = daemon.daemon.tasks.get(grant.task_id).tree_id
            own = daemon.daemon.spans.start("disturbance.change", task_id=root, input={"field": "cursor"})
            daemon.daemon.spans.finish(own, output={"facts": "OWN OBSERVATION"})
            other = daemon.daemon.spans.start("disturbance.change", task_id="unrelated-task", input={})
            daemon.daemon.spans.finish(other, output={"facts": "PRIVATE OTHER TASK"})
            return own.id, other.id

        own, other = daemon.run(record())
        result = tool(daemon.sock, token, "task.evidence", span_id=own)
        assert "OWN OBSERVATION" in result["content"]["text"]
        assert tool(daemon.sock, token, "task.evidence", span_id=other)["is_error"]
        return done("Inspected")

    install(daemon, [sup("task", "Working", task())], {"worker": worker})
    assert outcome(chat(daemon, "Inspect recorded task facts"))["status"] == "done"


def test_history_reader_fetches_full_earlier_message_in_same_conversation(daemon):
    from tests.conftest import tool

    source = "Detailed source material. " * 1000 + "FINAL DERIVATION REQUIRED"
    install(daemon, [sup("reply", "Saved")])
    chat(daemon, source)

    def worker(token):
        listed = tool(daemon.sock, token, "chat.history", query="FINAL DERIVATION REQUIRED")["content"]
        entry_id = listed["records"][0]["id"]
        parts, offset = [], 0
        while True:
            page = tool(daemon.sock, token, "chat.history", entry_id=entry_id, offset=offset, max_chars=4000)["content"]
            parts.append(page["text"])
            if page["next_offset"] is None:
                break
            offset = page["next_offset"]
        assert source in "".join(parts)
        return done("Read the complete source")

    install(daemon, [sup("task", "Reading", task())], {"worker": worker})
    assert outcome(chat(daemon, "Continue from my earlier source"))["status"] == "done"


def test_provider_error_is_reported_without_keyword_diagnosis_or_clipping():
    from ferb.orchestrator.worker import service_problem
    detail = "Not an auth or quota failure. " + "diagnostic detail " * 100 + "FINAL PROVIDER FACT"
    assert detail in service_problem(detail, "fake")


def test_account_check_does_not_select_first_tab_account(daemon):
    observed = []

    async def select_first(*_):
        observed.append("wrong layer selected an account")
        return "first@example.test"

    daemon.daemon.supervisor.checks.account_of = select_first
    condition = cond("Requested signed-in account was used", "account", site="example.test", expected="chosen")
    install(daemon, [sup("task", "Checking", task(conds=(condition,)))], {"worker": done("Checked")})
    chat(daemon, "Check the selected account from complete evidence")
    assert observed == []


def test_file_contains_checks_beyond_old_prefix_limit(daemon):
    workspace = daemon.daemon.home / "workspace"
    workspace.mkdir(exist_ok=True)
    (workspace / "complete.txt").write_text("x" * 2_100_000 + "REQUIRED ENDING")
    condition = cond("Required ending exists", "file_contains", path="complete.txt", text="REQUIRED ENDING")
    install(daemon, [sup("task", "Checking", task(conds=(condition,)))], {"worker": done("Checked")})
    assert outcome(chat(daemon, "Check the complete file"))["verified"]


def test_finish_decision_cannot_hide_answer_in_question_field():
    from pydantic import ValidationError
    from ferb.orchestrator.schemas import DispatchOutcome
    with pytest.raises(ValidationError):
        DispatchOutcome(action="finish", worker_type="default", instructions="", question="Done, all complete.")


def test_true_file_predicate_cannot_approve_inadequate_learning_material(daemon):
    from tests.conftest import writes
    t = task('Complete study material', paths=('study.md',), conds=(
        cond('The material teaches every syllabus topic well enough to study from', 'file_exists', path='study.md'),))
    script = install(daemon, [sup('task', 'Preparing material.', t)], {
        'worker': writes(daemon.sock, 'study.md', 'Topic list only.'),
        'worker.verify': json.dumps({'results': [{'index': 0, 'met': False,
                    'evidence': 'File exists, but a topic list does not teach the requested material.'}]})})
    result = outcome(chat(daemon, 'Create full explanations and worked examples for every syllabus topic.'))
    assert result['status'] == 'failed' and result['verified'] is False
    context = next(c for _, key, c, _ in script.contexts if key == 'worker.verify')
    assert 'study.md exists' in context
    assert 'full explanations and worked examples' in context


def test_cleanup_success_without_any_inspection_is_not_reported_done(daemon):
    condition = 'The requested source and destination identity are established.'
    async def cleanup(task_id):
        return [{'tab': 'temporary', 'closed': True}]
    daemon.daemon.supervisor.on_task_end = cleanup
    install(daemon, [sup('task', 'Preparing', task(conds=(cond(condition),)))], {
        'worker': json.dumps({'status': 'blocked', 'summary': 'Input route unavailable.', 'artifacts': [], 'evidence': []}),
        'worker.resources': done('Everything is complete.')})
    result = outcome(chat(daemon, 'Upload the requested source to the intended destination.'))
    assert result['status'] == 'failed' and not result['verified']
    # V5: no check ran on the blocked work, so the condition is still to do, never "unverified".
    assert condition in result['answer'] and 'unverified' not in result['answer']
    assert 'Everything is complete.' not in result['answer']


def test_optional_planner_environment_is_saved_as_fetchable_task_evidence(daemon):
    source = 'Unique device fact ' * 500 + ' IMPORTANT INVENTORY TAIL'
    supervisor = daemon.daemon.supervisor
    supervisor.context_budget_tokens = 300
    supervisor.environment = lambda _: {'live': source}  # P3 stable identity/device layer is protected now
    install(daemon, [sup('task', 'Inspecting', task())], {'worker': done('Inspected')})
    result = outcome(chat(daemon, 'Inspect the available devices.'))
    async def inspect():
        rows = daemon.daemon.db.conn.execute(
            "SELECT output_json FROM spans WHERE task_id=? AND kind='context.source'", (result['task_id'],)).fetchall()
        return [json.loads(row[0] or '{}').get('text', '') for row in rows]
    assert any(source in text for text in daemon.run(inspect()))


@pytest.mark.parametrize('exit_mode', ['close_after_plan', 'cancel_during_worker'])
def test_task_observer_finishes_and_rows_close_on_interrupted_stream(daemon, exit_mode):
    import asyncio
    from ferb.orchestrator.schemas import TaskSpec
    observed = []
    async def check():
        supervisor = daemon.daemon.supervisor
        entered = asyncio.Event()
        async def start(task_id):
            observed.append(('start', task_id))
        async def finish(task_id):
            observed.append(('finish', task_id))
        async def blocked(tree, span):
            entered.set()
            await asyncio.Event().wait()
        supervisor.on_task_start, supervisor.on_task_finish = start, finish
        supervisor._worker = blocked
        stream = supervisor.run_tree(TaskSpec.model_validate(task()), 'Starting', 'disposable-request', 'Inspect target')
        plan = await anext(stream)
        if exit_mode == 'close_after_plan':
            await stream.aclose()
        else:
            advance = asyncio.create_task(anext(stream))
            await advance  # worker.started event; the worker itself remains running
            await entered.wait()
            advance = asyncio.create_task(anext(stream))
            await asyncio.sleep(0)
            advance.cancel()
            with pytest.raises(asyncio.CancelledError):
                await advance
            await stream.aclose()
        assert observed == [('start', plan['task_id']), ('finish', plan['task_id'])]
        assert not supervisor.running_trees()
        assert all(row['status'] not in ('running', 'planned') for row in supervisor.tasks.rows(plan['task_id']))
    daemon.run(check())


def test_cancelled_cleanup_is_recorded_before_observer_finishes(daemon):
    import asyncio
    from ferb.core.types import Outcome
    from ferb.orchestrator.schemas import TaskSpec
    async def check():
        supervisor = daemon.daemon.supervisor
        cleanup_started = asyncio.Event()
        order = []
        async def worker(tree, span):
            return Outcome(tree.worker.id, 'done', 'Prepared', [], [])
        async def cleanup(task_id):
            order.append('cleanup')
            if order.count('cleanup') == 1:
                cleanup_started.set()
                await asyncio.Event().wait()
            return []
        async def finish(task_id):
            rows = daemon.daemon.db.conn.execute(
                "SELECT error,ended_at FROM spans WHERE kind='task.cleanup' AND task_id=?", (task_id + '.worker',)).fetchall()
            assert any(row['error'] and row['ended_at'] for row in rows)
            order.append('finish')
        supervisor._worker, supervisor.on_task_end, supervisor.on_task_finish = worker, cleanup, finish
        supervisor.on_task_start = None
        stream = supervisor.run_tree(TaskSpec.model_validate(task()), 'Starting', 'cleanup-request', 'Inspect')
        plan = await anext(stream)
        async def consume():
            return [event async for event in stream]
        consumer = asyncio.create_task(consume())
        await cleanup_started.wait()
        consumer.cancel()
        with pytest.raises(asyncio.CancelledError):
            await consumer
        assert order[-1] == 'finish' and order.count('cleanup') >= 2
        assert not supervisor.running_trees()
        root = next(row for row in supervisor.tasks.rows(plan['task_id']) if row['id'] == plan['task_id'])
        assert root['status'] == 'failed'
    daemon.run(check())


def test_every_runtime_role_accounts_for_complete_appended_evidence_once(daemon):
    from tests.conftest import tool
    from ferb.core.context import count_tokens
    report = 'Complete required report and evidence. ' * 400
    supervisor = daemon.daemon.supervisor
    supervisor.context_budget_tokens = 500
    async def cleanup(_):
        return [{'tab': 'temporary', 'closed': True}]
    supervisor.on_task_end = cleanup
    def worker(token):
        result = tool(daemon.sock, token, 'owner.ask', question=report, options=[])
        assert not result['is_error']
        return done(report)
    script = install(daemon, [sup('task', 'Working', task())], {
        'worker': worker,
        'worker.question': json.dumps({'ask_owner': False, 'answer': 'Continue', 'question': ''}),
        'worker.verify': json.dumps({'results': [{'index': 0, 'met': None, 'evidence': report}]}),
        'worker.dispatch': json.dumps({'action': 'finish', 'instructions': report, 'question': '', 'worker_type': 'default'}),
        'worker.resources': done(report)})
    chat(daemon, 'Inspect the complete evidence before declaring success.')
    for role in ('worker.question', 'worker.verify', 'worker.dispatch', 'worker.resources'):
        context = next(text for _, key, text, _ in script.contexts if key == role)
        assert context.count('Context budget: ') == 1
        body, header = context.rsplit('\n\nContext budget: ', 1)
        accounting = json.loads(header)
        assert accounting['content_tokens_estimate'] == count_tokens(body)
        assert accounting['over_budget'] is True and report in body


def test_direct_task_without_conditions_never_reports_unchecked_success(daemon):
    from ferb.core.types import Outcome
    from ferb.orchestrator.schemas import TaskSpec
    async def check():
        supervisor = daemon.daemon.supervisor
        async def worker(tree, span):
            return Outcome(tree.worker.id, 'done', 'Everything is complete.', [], [])
        supervisor._worker = worker
        events = [event async for event in supervisor.run_tree(
            TaskSpec.model_validate(task(conds=())), 'Working', 'direct-request', 'Inspect the result')]
        result = outcome(events)
        # V4: the work finished but nothing could be checked: done but unverified, saying why, never done. No
        # check ran, so the owner's text doesn't say "unverified" (V5).
        assert result['status'] == 'done_unverified' and not result['verified']
        assert 'unverified' not in result['answer'] and 'nothing to check' in result['answer']
        assert 'Everything is complete.' not in result['answer']
    daemon.run(check())


def test_unreadable_literal_predicate_does_not_prevent_outcome_judgment(daemon):
    folder = daemon.daemon.home / 'workspace'
    folder.mkdir(exist_ok=True)
    (folder / 'binary.dat').write_bytes(b'\xff\x00document data')
    script = install(daemon, [sup('task', 'Inspecting', task(conds=(
        cond('Check the requested content', 'file_contains', path='binary.dat', text='word'),
        cond('Check the second outcome'))))], {
        'worker': done('Inspected'),
        'worker.verify': json.dumps({'results': [
            {'index': 0, 'met': None, 'evidence': 'A binary-aware reader is needed.'},
            {'index': 1, 'met': True, 'evidence': 'Second outcome independently observed.'}]})})
    result = outcome(chat(daemon, 'Inspect the document and the second outcome.'))
    context = next(text for _, key, text, _ in script.contexts if key == 'worker.verify')
    assert 'UnicodeDecodeError' in context
    assert result['verification'][1]['met'] is True


def test_stop_reason_survives_unverified_completion_without_reusing_success_prose(daemon):
    reason = 'The requested account connection is unavailable. Nothing was submitted.'
    finish = json.dumps({'action':'finish','worker_type':'default','instructions':'Everything is complete.',
                         'question':'','stop_reason':reason})
    install(daemon, [sup('task','Checking',task(conds=(cond('Fresh source inspected'),)))],
            {'worker':json.dumps({'status':'blocked','summary':'Connection unavailable',
                                  'artifacts':[],'evidence':[]}), 'worker.dispatch':finish})
    result = outcome(chat(daemon,'Check the current source'))
    assert reason in result['answer']
    assert 'Fresh source inspected' in result['answer']
    assert 'Everything is complete.' not in result['answer']
    assert result['stop_reason'] == reason
    assert not result['verified']


def test_runtime_tool_catalog_facts_are_recorded_for_missing_tool_diagnosis(daemon, monkeypatch):
    from tests.conftest import FakeRuntime
    from ferb.core.types import RuntimeEvent
    from ferb.cli.client import Client
    original = FakeRuntime.events
    async def events(self, run):
        async for event in original(self, run):
            if event.kind == 'final':
                yield RuntimeEvent('final', dict(event.data, tools_seen=['tool_search','mcp__ferb.actions_search'],returncode=0))
            else:
                yield event
    monkeypatch.setattr(FakeRuntime,'events',events)
    install(daemon,[sup('task','Working',task())],{'worker':done('Done')})
    result=outcome(chat(daemon,'Inspect disposable source'))
    c=Client(daemon.sock)
    try:
        trace=c.call('trace.get',{'task_id':result['task_id']})
    finally:
        c.close()
    assert 'mcp__ferb.actions_search' in json.dumps(trace)
    assert 'runtime.tools' in json.dumps(trace)
