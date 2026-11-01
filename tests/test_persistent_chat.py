"""Chat connections may disappear; resumable task context must survive on disk."""
import json

from ferb.cli.client import Client
from tests.conftest import RunningDaemon
from tests.conftest import chat, install, outcome, sup, task


def attach(d, **params):
    c = Client(d.sock)
    try:
        return c.call("chat.attach", params)["session_id"]
    finally:
        c.close()


def test_context_is_redacted_and_legacy_tasks_are_recovered_without_grants(daemon, unattended):
    install(daemon, [sup("task", "Starting", task("use Work profile on shop.example", "owner"))],
            {"worker": json.dumps({"status": "needs_user", "summary": "Login required", "artifacts": [], "evidence": []})})
    first = outcome(chat(daemon, "use Work profile on shop.example", session="old-session"))

    async def verify():
        store = daemon.daemon.supervisor.continuity
        store.db.conn.execute("DELETE FROM task_context WHERE task_id=?", (first["task_id"],))
        from ferb.orchestrator.continuity import upgrade_legacy     # the one-time upgrade of an older database
        assert upgrade_legacy(store.db, daemon.daemon.redact)["imported"] == 1
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


def test_retry_receives_unknown_prior_action_for_inspection(daemon, unattended):
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
    retry["control"] = {"task_id": first["task_id"], "action": "retry", "standing_sends": "unchanged", "supersedes": []}
    script = install(daemon, [json.dumps(retry)], {"worker": json.dumps({"status":"done", "summary":"Inspected delivery.", "artifacts":[], "evidence":[]})})
    result = chat(daemon, "try again", session=session)
    assert outcome(result)["status"] == "done"
    ctx = next(ctx for _, key, ctx, _ in script.contexts if key == "worker")
    assert "Uncertain prior actions" in ctx and "shop.example" in ctx


def test_new_conversation_cannot_control_another_conversations_task(daemon, unattended):
    first = attach(daemon)
    install(daemon, [sup("task", "Starting", task("find notes"))],
            {"worker": json.dumps({"status": "needs_user", "summary": "Which notes?", "artifacts": [], "evidence": []})})
    previous = outcome(chat(daemon, "find notes", session=first))
    second = attach(daemon, new=True)

    async def check():
        supervisor = daemon.daemon.supervisor
        assert supervisor.continuity.load(previous["task_id"], second) is None
        from tests.support.history import conversation_text
        assert "find notes" not in conversation_text(supervisor, second)
        assert "find notes" in conversation_text(supervisor, first)
    daemon.run(check())


# decision: ORCH-507
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
        store = daemon.daemon.supervisor.continuity
        task_id = first['task_id']
        row = store.db.conn.execute('SELECT request_id FROM tasks WHERE id=?', (task_id,)).fetchone()
        store.db.conn.execute('DELETE FROM task_context WHERE task_id=?', (task_id,))
        daemon.daemon._journal().append('chat.message', {'request_id': row[0], 'text': 'different source'})
        from ferb.orchestrator.continuity import upgrade_legacy
        assert upgrade_legacy(store.db, daemon.daemon.redact)['conflicting'] == 1      # never guessed between
        assert store.conversation_of(task_id) is None
    daemon.run(check())
