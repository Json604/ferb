"""Chat connections may disappear; resumable task context must survive on disk."""
import io
import json

from ferb.cli.chat import Chat
from ferb.cli.client import Client
from tests.conftest import RunningDaemon
from tests.conftest import chat, install, outcome, sup, task


def attach(d, **params):
    c = Client(d.sock)
    try:
        return c.call("chat.attach", params)["session_id"]
    finally:
        c.close()


def test_reopened_cli_reuses_conversation_and_explicit_new_is_separate(daemon):
    first = Chat(lambda: Client(daemon.sock), io.StringIO(), lambda _: "")
    first.opening()
    second = Chat(lambda: Client(daemon.sock), io.StringIO(), lambda _: "")
    second.opening()
    assert second.session_id == first.session_id
    separate = attach(daemon, new=True)
    assert separate != first.session_id
    assert attach(daemon, resume=first.session_id) == first.session_id


def test_retry_after_daemon_restart_restores_goal_profile_and_real_task_id(home):
    d = RunningDaemon(home, watch_owner_input=False)
    try:
        d.daemon.supervisor.availability = d.daemon.supervisor._default_availability
        session = attach(d)
        install(d, [sup("task", "Starting", task("order lunch using Work profile", "owner", sites=("shop.example",)))],
                {"worker": json.dumps({"status": "needs_user", "summary": "Login required", "artifacts": [], "evidence": []})})
        first = outcome(chat(d, "order lunch on shop.example in Work profile", session=session))
    finally:
        d.stop()

    d = RunningDaemon(home, watch_owner_input=False)
    try:
        supervisor = d.daemon.supervisor
        supervisor.availability = supervisor._default_availability
        supervisor.environment = lambda words: {"stable": "", "profile_test": "Work" if "Work profile" in words else None}
        assert attach(d) == session
        retry = json.loads(sup("task", "Retrying", task("continue login", "owner")))
        retry["control"] = {"task_id": first["task_id"], "action": "continue"}
        seen = []

        def worker(token):
            grant = d.daemon.tokens.validate(token)
            rec = d.daemon.tasks.get(grant.task_id)
            request = d.daemon.state.requests.get(rec.task.request_id)
            seen.append((rec.task.goal, request, supervisor._env[request.request_id]))
            return json.dumps({"status": "needs_user", "summary": "Waiting for login", "artifacts": [], "evidence": []})

        script = install(d, [json.dumps(retry)], {"worker": worker})
        result = outcome(chat(d, "try the lunch task again", session=session))
        assert result["task_id"] != first["task_id"]
        assert "Work profile" in seen[0][0] and "Login required" in seen[0][0]
        assert "shop.example" in seen[0][1].words
        assert seen[0][2]["profile_test"] == "Work"
        assert first["task_id"] in script.contexts[0][2]
    finally:
        d.stop()


def test_context_is_redacted_and_legacy_tasks_are_recovered_without_grants(daemon):
    install(daemon, [sup("task", "Starting", task("use Work profile on shop.example", "owner"))],
            {"worker": json.dumps({"status": "needs_user", "summary": "Login required", "artifacts": [], "evidence": []})})
    first = outcome(chat(daemon, "use Work profile on shop.example", session="old-session"))

    async def verify():
        store = daemon.daemon.supervisor.continuity
        store.db.conn.execute("DELETE FROM task_context WHERE task_id=?", (first["task_id"],))
        conversation = store.attach()
        spec, request, state = store.load(first["task_id"], conversation)
        assert "Work profile" in request.words and "Login required" in state
        assert not daemon.daemon.state.approvals.pending()
        daemon.daemon.redact.add_secret_value("482913")
        spec.goal += " 482913"
        store.save(first["task_id"], conversation, spec, "known code 482913")
        stored = store.db.conn.execute("SELECT spec_json,owner_words FROM task_context WHERE task_id=?",
                                      (first["task_id"],)).fetchone()
        assert "482913" not in str(tuple(stored))
    daemon.run(verify())


def test_retry_receives_unknown_prior_action_for_inspection(daemon):
    session = attach(daemon)
    install(daemon, [sup("task", "Starting", task("send on shop.example", "owner"))],
            {"worker": json.dumps({"status": "needs_user", "summary": "Unclear", "artifacts": [], "evidence": []})})
    first = outcome(chat(daemon, "send on shop.example", session=session))

    async def lost():
        ledger = daemon.daemon.ledger
        intent = ledger.intend(first["task_id"] + ".worker", "browser.act", {"site": "shop.example"}, "lost")
        ledger.mark(intent.id, "unknown")
    daemon.run(lost())
    retry = json.loads(sup("task", "Retrying", task("try again", "owner")))
    retry["control"] = {"task_id": first["task_id"], "action": "retry"}
    script = install(daemon, [json.dumps(retry)], {"worker": json.dumps({"status":"done", "summary":"Inspected delivery.", "artifacts":[], "evidence":[]})})
    result = chat(daemon, "try again", session=session)
    assert outcome(result)["status"] == "done"
    ctx = next(ctx for _, key, ctx, _ in script.contexts if key == "worker")
    assert "Uncertain prior actions" in ctx and "shop.example" in ctx


def test_unknown_task_id_is_repaired_to_a_specific_question_without_running(daemon):
    bad = json.loads(sup("task", "Retrying", task("try again", "owner")))
    bad["control"] = {"task_id": "t1", "action": "retry"}
    script = install(daemon, [json.dumps(bad), sup("clarify", "Do you mean lunch or the spreadsheet search?")], {})
    result = chat(daemon, "try that again")
    assert result[-1] == {"type": "clarify", "text": "Do you mean lunch or the spreadsheet search?"}
    assert len(script.contexts) == 2


def test_new_conversation_cannot_control_another_conversations_task(daemon):
    first = attach(daemon)
    install(daemon, [sup("task", "Starting", task("find notes"))],
            {"worker": json.dumps({"status": "needs_user", "summary": "Which notes?", "artifacts": [], "evidence": []})})
    previous = outcome(chat(daemon, "find notes", session=first))
    second = attach(daemon, new=True)

    async def check():
        supervisor = daemon.daemon.supervisor
        assert supervisor.continuity.load(previous["task_id"], second) is None
        assert "find notes" not in supervisor.recent(conversation=second)[0]
        assert "find notes" in supervisor.recent(conversation=first)[0]
    daemon.run(check())


def test_pending_question_survives_daemon_restart(home):
    d = RunningDaemon(home, watch_owner_input=False)
    try:
        session = attach(d)
        install(d, [sup("clarify", "Which project folder?")], {})
        chat(d, "find the architecture notes", session=session)
    finally:
        d.stop()
    d = RunningDaemon(home, watch_owner_input=False)
    try:
        assert attach(d) == session
        script = install(d, [sup("reply", "Understood")], {})
        chat(d, "the FERB project", session=session)
        assert "find the architecture notes" in script.contexts[0][2]
        assert "Which project folder?" in script.contexts[0][2]
        async def check():
            assert d.daemon.supervisor.continuity.pending(session) == ("", "")
        d.run(check())
    finally:
        d.stop()


def test_legacy_recovery_rejects_conflicting_request_sources(daemon):
    from tests.conftest import done
    install(daemon, [sup('task', 'Starting', task('inspect source'))], {'worker': done('read')})
    first = outcome(chat(daemon, 'inspect source', session='legacy'))
    async def check():
        from pytest import raises
        store = daemon.daemon.supervisor.continuity
        task_id = first['task_id']
        row = store.db.conn.execute('SELECT request_id FROM tasks WHERE id=?', (task_id,)).fetchone()
        store.db.conn.execute('DELETE FROM task_context WHERE task_id=?', (task_id,))
        daemon.daemon._journal().append('chat.message', {'request_id': row[0], 'text': 'different source'})
        with raises(ValueError, match='Conflicting legacy request'):
            store.attach()
    daemon.run(check())
