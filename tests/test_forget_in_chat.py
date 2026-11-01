"""Memory review I2: the chat can forget. A forget asked in chat waits for a plain Yes or No to a question that names
exactly what goes and what stays, like `ferb memory forget`. The forget itself is simple (MEM-2):
the owner's chat message stays and FERB's past answers and traces are not scrubbed."""
import json
from typing import Any

from tests.conftest import RunningDaemon, chat, install
from tests.support.forgets import (_held_in_spans, _lesson, _on_loop,
                                  _remember, _reply)


def _holding(d: RunningDaemon, word: str) -> tuple[list[str], list[str]]:
    db = d.daemon.db
    return _on_loop(d, lambda: (
        [r[0] for r in db.conn.execute("SELECT kind FROM journal WHERE data_json LIKE ?", (f"%{word}%",))],
        [r[0] for r in db.conn.execute("SELECT kind FROM spans WHERE input_json LIKE ? OR output_json LIKE ?",
                                       (f"%{word}%", f"%{word}%"))]))


def test_the_forget_confirmation_names_the_items_that_go(daemon: RunningDaemon) -> None:
    """Review M4: the confirmation showed counts only, while items built on the same sources went too."""
    from ferb.cli.client import Client
    item = _remember(daemon, "remember that my dentist is Dr. Quillfeather", "my dentist is Dr. Quillfeather",
                     "dentist")
    c = Client(daemon.sock)
    try:
        planned = c.call("memory.forget", {"id": item.id})
    finally:
        c.close()
    assert planned["confirmed"] is False
    assert any("“remember that my dentist is Dr. Quillfeather”" in line for line in planned["will_delete"]), planned


def _forget_question(d: RunningDaemon) -> Any:
    return _on_loop(d, lambda: next((q for q in d.daemon.state.approvals.pending()
                                     if q.capability == "memory.forget"), None))


def test_chat_can_forget_after_a_yes_to_a_question_naming_what_goes(daemon: RunningDaemon) -> None:
    """Review live test 4: "forget my test lab colour" in the notch said it forgot, but the item stayed."""
    item = _remember(daemon, "remember that my test lab colour is violet-heron", "my test lab colour is violet-heron",
                     "test_lab_colour")
    install(daemon, [_reply("Okay.", forget=[{"item_id": item.id, "quote": "forget my test lab colour"}])])
    asked = chat(daemon, "forget my test lab colour")
    question = _forget_question(daemon)
    assert question is not None and "violet-heron" in question.reason and question.options == ("Yes", "No")
    assert any("violet-heron" in str(e.get("text")) for e in asked)
    assert _on_loop(daemon, lambda: daemon.daemon.memory.get(item.id)) is not None      # nothing gone before the Yes
    install(daemon, [json.dumps({"kind": "reply", "text": "", "answer_question": {"approval_id": question.id,
                                                                                    "answer": "Yes"}})])
    told = "\n".join(str(e.get("text")) for e in chat(daemon, "Yes"))
    assert _on_loop(daemon, lambda: daemon.daemon.memory.get(item.id)) is None
    assert "Forgot" in told and "Your own messages stay" in question.reason
    daemon.run(daemon.daemon._settle_forgets())
    assert "chat.message" in _holding(daemon, "heron")[0]                # the owner's own words stay in the history


def test_a_no_keeps_the_item(daemon: RunningDaemon) -> None:
    item = _remember(daemon, "remember that my dentist is Dr. Quillfeather", "my dentist is Dr. Quillfeather",
                     "dentist")
    install(daemon, [_reply("Okay.", forget=[{"item_id": item.id, "quote": "forget my dentist"}])])
    chat(daemon, "forget my dentist")
    question = _forget_question(daemon)
    install(daemon, [json.dumps({"kind": "reply", "text": "", "answer_question": {"approval_id": question.id,
                                                                                    "answer": "No"}})])
    chat(daemon, "No")
    assert _on_loop(daemon, lambda: daemon.daemon.memory.get(item.id)) is not None


def test_a_chat_forget_needs_the_owners_own_words(daemon: RunningDaemon) -> None:
    """Only the owner's words carry authority (MP2): a forget the planner wrote without them asks nothing."""
    item = _remember(daemon, "remember that my dentist is Dr. Quillfeather", "my dentist is Dr. Quillfeather",
                     "dentist")
    install(daemon, [_reply("Okay.", forget=[{"item_id": item.id, "quote": "forget everything"}])])
    chat(daemon, "summarize the page I'm looking at")
    assert _forget_question(daemon) is None
    assert _on_loop(daemon, lambda: daemon.daemon.memory.get(item.id)) is not None


# ---------- fix round (review Q1, Q2): forget reaches what a task did with the item; one shared word isn't a mention ----

def _added(d: RunningDaemon, content: str, key: str) -> Any:
    dd = d.daemon
    src = str(_on_loop(d, lambda: dd._journal().append("memory.added", {"content": content}).id))
    return _on_loop(d, lambda: dd.memory.propose("person", "owner", key, content, "owner_stated", [("journal", src)], 1.0))


def test_a_chat_forget_quote_must_be_a_whole_clause_of_the_owners_words(daemon: RunningDaemon) -> None:
    """Review Q4: a single common word the owner typed ("the") was enough for FERB to ask about deleting a memory."""
    item = _added(daemon, "My dentist is Dr. Zorblat", "dentist")
    install(daemon, [_reply("Okay.", forget=[{"item_id": item.id, "quote": "the"}])])
    chat(daemon, "summarize the page I'm looking at")
    assert _forget_question(daemon) is None
    install(daemon, [_reply("Okay.", forget=[{"item_id": item.id, "quote": "please forget my dentist"}])])
    chat(daemon, "please forget my dentist, thanks")
    assert _forget_question(daemon) is not None


def _everywhere(d: RunningDaemon, word: str) -> list[tuple[str, str]]:
    """Every (table, column) whose text holds `word`, across the whole database (index shadow tables too)."""
    db = d.daemon.db

    def scan() -> list[tuple[str, str]]:
        hits = []
        for (table,) in db.conn.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall():
            for column in [r[1] for r in db.conn.execute(f'PRAGMA table_info("{table}")')]:
                if db.conn.execute(f'SELECT 1 FROM "{table}" WHERE CAST("{column}" AS TEXT) LIKE ? LIMIT 1',
                                   (f"%{word}%",)).fetchone():
                    hits.append((table, column))
        return hits
    return _on_loop(d, scan)


def _paraphrased(d: RunningDaemon, content: str = "Owner's boss is Quentarr Vexley") -> Any:
    """FERB's wording of what the owner said, saved as its understanding (MEM-12)."""
    from ferb.orchestrator.schemas import Remember
    dd = d.daemon
    said = "my manager is Quentarr"
    src = str(_on_loop(d, lambda: dd._journal().append("chat.message", {"text": said}).id))
    d.run(dd._remember([Remember(kind="person", subject="owner", key="boss",
                                 content=content, quote=said)], said, src))
    return _on_loop(d, lambda: next(i for i in dd.memory.items(None, valid_now=False) if i.key == "boss"))


def _memory_questions(d: RunningDaemon) -> list[Any]:
    return _on_loop(d, lambda: [q for q in d.daemon.state.approvals.pending()
                                if q.capability == "memory.forget"])


def test_a_forget_closes_the_pending_question_about_that_memory(daemon: RunningDaemon) -> None:
    """Re-review NEW-1 (probe a2): the owner forgot a memory from the dashboard/CLI while FERB's chat "Forget for good?"
    about it waited; the question stayed, kept the forgotten text, and every later planner turn read it again."""
    item = _paraphrased(daemon)
    _on_loop(daemon, lambda: daemon.daemon._chat_forget(item.id, "r-forget"))
    assert len(_memory_questions(daemon)) == 1
    from ferb.cli.client import Client
    c = Client(daemon.sock)
    try:
        c.call("memory.forget", {"id": item.id, "confirm": True})
    finally:
        c.close()
    assert _memory_questions(daemon) == []
    script = install(daemon, [_reply("Hi.")])
    chat(daemon, "what's on today?")
    assert "Vexley" not in "\n".join(str(c) for _, key, c, _ in script.contexts if key == "supervisor")
    assert _everywhere(daemon, "Vexley") == []


def _pending_rows(d: RunningDaemon) -> int:
    return _on_loop(d, lambda: d.daemon.db.conn.execute("SELECT COUNT(*) FROM memory_questions").fetchone()[0])


def test_a_yes_to_forget_a_learned_item_closes_the_question_about_one_built_on_it(daemon: RunningDaemon) -> None:
    """Probe h3c: questions waited on lesson A and on procedure B built on it; the Yes on A deletes B too, so B's
    question closes and neither text is left anywhere."""
    d = daemon.daemon
    a = _lesson(daemon, "qorvex rule")
    b = _on_loop(daemon, lambda: d.learning.put("procedure", "qorvex proc", {"intent": "qorvex proc", "steps": []},
                                                [a["id"]], made_by="model:t", state="candidate"))
    _on_loop(daemon, lambda: d._chat_forget(b["id"], "r-b"))
    _on_loop(daemon, lambda: d._chat_forget(a["id"], "r-a"))
    on_a = next(q for q in _memory_questions(daemon) if "qorvex rule" in q.reason)
    _on_loop(daemon, lambda: d._resolve_approval(on_a.id, True, 1))
    daemon.run(d._settle_forgets())
    assert _on_loop(daemon, lambda: d.learning.get(b["id"])) is None
    assert _memory_questions(daemon) == [] and _pending_rows(daemon) == 0
    assert _everywhere(daemon, "qorvex") == []


def test_forgetting_one_tasks_card_leaves_another_task_with_the_same_goal_alone(daemon: RunningDaemon) -> None:
    """Round 5 (NEW-1): a learned item's echo took the body's "intent" for every kind, and an episode card's intent is
    its task's goal, so forgetting task 1's card wiped the same goal from task 2's trace and from every planner turn
    (a plain substring replace; recurring tasks share goals). An episode card's goal is never scrubbed, not even in a
    task that was shown the card."""
    from tests.conftest import done, outcome, sup, task
    d = daemon.daemon
    goal = "check my inbox for new mail"
    ids = []
    for answer in ("No new mail in the inbox.", "Two new messages in the inbox."):
        install(daemon, [sup("task", "Checking.", task(goal))], {"worker": done(answer)})
        ids.append(outcome(chat(daemon, goal))["task_id"])
    daemon.run(d.supervisor.finish_episodes())
    card = _on_loop(daemon, lambda: d.learning.current_card(ids[0]))
    assert card["body"]["intent"] == goal
    _on_loop(daemon, lambda: d.db.conn.execute("INSERT OR IGNORE INTO task_memory_refs VALUES (?, ?)",
                                               (ids[1], card["id"])))     # task 2's planner was shown task 1's card
    other = "task_id = ? OR task_id LIKE ?"
    before = (_held_in_spans(daemon, goal), _held_in_spans(daemon, goal, other, ids[1], ids[1] + ".%"))
    from ferb.cli.client import Client
    c = Client(daemon.sock)
    try:
        c.call("memory.learning", {"op": "forget", "id": card["id"], "confirm": True})
    finally:
        c.close()
    assert before[1] and (_held_in_spans(daemon, goal), _held_in_spans(daemon, goal, other, ids[1],
                                                                       ids[1] + ".%")) == before
    assert _on_loop(daemon, lambda: [r[0] for r in d.db.conn.execute(
        "SELECT goal FROM tasks WHERE id IN (?, ?)", ids)]) == [goal, goal]
    assert _on_loop(daemon, lambda: d.learning.current_card(ids[1])) is not None


def test_waiting_for_forgets_never_spins_on_a_scrub_that_just_finished(daemon: RunningDaemon, monkeypatch: Any) -> None:
    """A finished scrub leaves the daemon's set only when its done callback runs, a loop pass later. A chat turn that
    waited in between found it still there, and gathering only finished tasks completes without yielding to the loop,
    so the turn spun the event loop at full CPU and the callback never ran (seen under parallel test load). Waiting
    skips work that has finished; so does waiting for the learning captures."""
    import asyncio
    d = daemon.daemon
    real = asyncio.gather
    calls: list[int] = []

    def bounded(*aws: Any, **kw: Any) -> Any:
        calls.append(len(aws))
        assert len(calls) < 50, "waiting spun on work that had already finished"
        return real(*aws, **kw)

    async def finished_but_not_yet_dropped() -> None:
        async def done() -> None:
            return None
        job = asyncio.get_running_loop().create_task(done())
        await job                                         # finished; still in the sets until its callback would run
        d._forgetting.add(job)
        d.supervisor._episode_jobs.add(job)
        try:
            await d._settle_forgets()
            await d.supervisor.finish_episodes()
        finally:
            d._forgetting.discard(job)
            d.supervisor._episode_jobs.discard(job)
    with monkeypatch.context() as patched:
        patched.setattr(asyncio, "gather", bounded)
        daemon.run(finished_but_not_yet_dropped())
