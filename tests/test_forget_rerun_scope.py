"""Fix wave 6a: a forget's scrub run again touches only the work running at the forget.

When the owner forgets an item while tasks or chat turns are running, the daemon records the requests of that work
(and the spans then unfinished); the scrub runs again as that work finishes or ends, and touches only what those
requests wrote: never a task, reply or card of work started after the forget, the owner stating the item again, or a
task resumed in place (a continue, retry or correction) under a new request (re-reviews 2 and 3)."""
import asyncio
import json
from typing import Any

from ferb.memory.echoes import REMOVED
from tests.conftest import RunningDaemon
from tests.support.forgets import (QUILL, _answers, _held_in_spans, _identity, _lesson, _let_go, _on_loop, _remember,
                                   _result, _running_across_the_forget, _task_records, _texts)


# ---------- round 2 (re-review 2, I-A and m-B) ----------

def test_a_rerun_leaves_a_task_started_after_an_identity_forget_alone(daemon: RunningDaemon) -> None:
    """Re-review I-A (probe 2): an identity statement's scope is every task since it was set, and the scrub run again
    when a job running at the forget ended re-read it then, so a task started after the forget (the owner stating it
    again) lost the statement from its answer, spans and a later chat reply. Nothing started after the forget is
    rewritten: the scope ends at the forget and a re-run touches only the work then running."""
    d = daemon.daemon
    node = _identity(daemon, "I sign emails as Kestrovan")
    gate, job = _running_across_the_forget(daemon, lambda: None)
    _on_loop(daemon, lambda: d._forget_nodes([node["id"]]))
    daemon.run(d._settle_forgets())

    def after() -> None:
        now = d.clock.now().isoformat()
        d.db.conn.execute(
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
            "outcome_json, created_at, updated_at) VALUES ('t-new', 't-new', NULL, 'r-new', 'sign it', 'worker', "
            "'worker', '[]', 'done', ?, ?, ?)", (json.dumps({"answer": "I sign emails as Kestrovan"}), now, now))
        d.journal.append("task.answer", {"task_id": "t-new", "request_id": "r-new", "status": "done",
                                         "text": "Signed it: I sign emails as Kestrovan\nDone"})
        d.journal.append("chat.reply", {"session_id": "s1", "request_id": "r-new2",
                                        "text": "Noted: I sign emails as Kestrovan", "display_text": "x"})
        span = d.spans.start("tool.call", task_id="t-new", input={"tool": "mail.draft"})
        d.spans.finish(span, output={"text": "I sign emails as Kestrovan"})
    _on_loop(daemon, after)
    before = (_texts(daemon, "task.answer"), _texts(daemon, "chat.reply"), _held_in_spans(daemon, "Kestrovan"))
    _let_go(daemon, gate, job)
    assert (_texts(daemon, "task.answer"), _texts(daemon, "chat.reply"), _held_in_spans(daemon, "Kestrovan")) == before


def test_a_rerun_leaves_what_follows_the_owner_restating_a_forgotten_item_alone(daemon: RunningDaemon) -> None:
    """Re-review I-A (probe 1): after a memory forget the owner states the item again; the scrub run again when a job
    running at the forget ended removed FERB's "Remembered" line for the new item."""
    d = daemon.daemon
    item = _remember(daemon, f"remember that {QUILL}", QUILL, "dentist")
    gate, job = _running_across_the_forget(daemon, lambda: None)
    _on_loop(daemon, lambda: d._forget_memory(item.id))
    daemon.run(d._settle_forgets())
    _remember(daemon, f"remember that {QUILL}", QUILL, "dentist")               # the owner states it again
    before = (_texts(daemon, "chat.reply"), _held_in_spans(daemon, "Quillfeather"))
    _let_go(daemon, gate, job)
    assert (_texts(daemon, "chat.reply"), _held_in_spans(daemon, "Quillfeather")) == before


def test_a_rerun_never_forgets_the_card_of_a_task_started_after_an_identity_forget(daemon: RunningDaemon) -> None:
    """Re-review I-A (probe 3): the card of a task created after the identity forget, whose body states it, was
    deleted and tombstoned when the job running at the forget ended."""
    d = daemon.daemon
    node = _identity(daemon, "I sign emails as Kestrovan")
    gate, job = _running_across_the_forget(daemon, lambda: None)
    _on_loop(daemon, lambda: d._forget_nodes([node["id"]]))
    daemon.run(d._settle_forgets())

    def after() -> Any:
        now = d.clock.now().isoformat()
        d.db.conn.execute(
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
            "outcome_json, created_at, updated_at) VALUES ('T9', 'T9', NULL, 'r-9', 'sign it', 'worker', "
            "'worker', '[]', 'done', '{}', ?, ?)", (now, now))
        return d.learning.put("episode", "T9", {"task_id": "T9", "verified": True, "request": "sign the mail",
                                                "answer": "I sign emails as Kestrovan"}, [], made_by="code:episode",
                              state="active")
    card = _on_loop(daemon, after)
    _let_go(daemon, gate, job)
    assert _on_loop(daemon, lambda: d.db.conn.execute("SELECT COUNT(*) FROM learning_nodes WHERE id = ?",
                                                      (card["id"],)).fetchone()[0]) == 1


def test_a_task_running_at_the_forget_is_scrubbed_once_its_tree_finishes(daemon: RunningDaemon) -> None:
    """Re-review m-B: the task's answer is journaled when its tree finishes, but its job ends only after writing its
    "while you were away" result (which can wait on a notification). A chat turn starting in between passed the forget
    settle with nothing queued and read the answer. The scrub is queued as the tree finishes (`task.finished`), so the
    next turn waits for it; the job's end runs it again for the result."""

    d = daemon.daemon
    item = daemon.run(d._memory_add(None, {"content": QUILL, "key": "dentist"}))      # type: ignore[arg-type]
    first, second = asyncio.Event(), asyncio.Event()

    async def start() -> Any:
        now = d.clock.now().isoformat()
        d.db.conn.execute(
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
            "outcome_json, created_at, updated_at) VALUES ('t-fin', 't-fin', NULL, 'r-fin', 'book it', 'worker', "
            "'worker', '[]', 'running', '{}', ?, ?)", (now, now))
        d.db.conn.execute("INSERT INTO task_memory_refs VALUES ('t-fin', ?)", ("memory:" + item["id"],))

        async def job() -> None:
            await first.wait()
            d.journal.append("task.answer", {"task_id": "t-fin", "request_id": "r-fin", "status": "done",
                                             "text": "Booked Dr. Quillfeather\nAll set"})
            d._publish("task.finished", {"task_id": "t-fin", "status": "done"})
            await second.wait()                           # e.g. `_task_result` waiting on a notification
            _result(daemon, "t-fin", "Booked Dr. Quillfeather")
        task = asyncio.get_running_loop().create_task(job())
        d._turns.add(task)
        d._turn_requests[task] = "r-fin"
        task.add_done_callback(d._turns.discard)
        return task
    job = daemon.run(start())
    _on_loop(daemon, lambda: d._forget_memory(item["id"]))
    daemon.run(d._settle_forgets())
    daemon.call_soon(first.set)
    daemon.run(asyncio.sleep(0))
    daemon.run(d._settle_forgets())                       # what a chat turn starting now waits for
    assert _answers(daemon, "task.answer", "task_id", "t-fin") == [f"{REMOVED}\nAll set"]
    _let_go(daemon, second, job)
    assert _task_records(daemon, "t-fin")[1] == [REMOVED]


# ---------- round 3 (re-review 3: I-B, m-1, m-2, the stronger probe) ----------

def _end(d: RunningDaemon, gate: Any, job: Any) -> None:
    _let_go(d, gate, job)


def _span_out(d: RunningDaemon, span_id: str) -> Any:
    return _on_loop(d, lambda: json.loads(d.daemon.db.conn.execute("SELECT output_json FROM spans WHERE id = ?",
                                                                   (span_id,)).fetchone()[0]))


def _new_task(d: Any, task_id: str, request: str, outcome: dict[str, Any] | None = None) -> None:
    now = d.clock.now().isoformat()
    d.db.conn.execute(
        "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
        "outcome_json, created_at, updated_at) VALUES (?, ?, NULL, ?, 'book', 'worker', 'worker', '[]', 'done', ?, ?, "
        "?)",
        (task_id, task_id, request, json.dumps(outcome or {}), now, now))


def test_a_rerun_touches_the_running_task_not_the_owner_restating_it_or_a_new_task(daemon: RunningDaemon) -> None:
    """Re-review 3, the stronger probe: a real running task (its request and tree recorded) loses the item when it
    ends; the owner stating the item again and an unrelated new task, both after the forget, keep it."""
    d = daemon.daemon
    item = _remember(daemon, f"remember that {QUILL}", QUILL, "dentist")
    _on_loop(daemon, lambda: d.db.conn.execute("INSERT INTO task_memory_refs VALUES ('t-run', ?)",
                                               ("memory:" + item.id,)))

    def writes() -> None:
        d.journal.append("task.answer", {"task_id": "t-run", "request_id": "r-run", "status": "done",
                                         "text": "Booked Dr. Quillfeather for Monday\nAll set"})
    gate, job = _running_across_the_forget(daemon, writes, "r-run", "t-run")
    _on_loop(daemon, lambda: d._forget_memory(item.id))
    daemon.run(d._settle_forgets())
    _remember(daemon, f"remember that {QUILL}", QUILL, "dentist")          # the owner states it again

    def other() -> str:
        _new_task(d, "t-new", "r-new")
        d.journal.append("task.answer", {"task_id": "t-new", "request_id": "r-new", "status": "done",
                                         "text": "Booked Dr. Quillfeather for Friday\nDone"})
        span = d.spans.start("tool.call", task_id="t-new", input={"tool": "calendar.add"})
        d.spans.finish(span, output={"text": "Booked Dr. Quillfeather Friday"})
        return span.id
    new_span = _on_loop(daemon, other)
    replies = _answers(daemon, "chat.reply", "session_id", "s1")
    _end(daemon, gate, job)
    assert _answers(daemon, "task.answer", "task_id", "t-run") == [f"{REMOVED}\nAll set"]
    assert _answers(daemon, "task.answer", "task_id", "t-new") == ["Booked Dr. Quillfeather for Friday\nDone"]
    assert _span_out(daemon, new_span) == {"text": "Booked Dr. Quillfeather Friday"}
    assert _answers(daemon, "chat.reply", "session_id", "s1") == replies


def _retry_in_place(d: Any, tree: str, request: str, text: str) -> tuple[str, str]:
    """A continue or retry of an ended task carries on in place: the same tree id under a new request (the tasks row's
    upsert, a new root span), its planner turn holding the owner's words, re-read tool output, answer and result."""
    d.db.conn.execute("UPDATE tasks SET request_id = ?, status = 'running', outcome_json = '{}' WHERE id = ?",
                      (request, tree))
    turn = d.spans.start("model.turn", input={"role": "supervisor", "request_id": request,
                                              "message": f"retry that for Tuesday; {text}"})
    d.spans.finish(turn, output={"text": f"Retrying for Tuesday: {text}"})
    root = d.spans.start("task", task_id=tree, input={"goal": "book it", "request_id": request})
    tool = d.spans.start("tool.call", task_id=tree, parent_id=root.id, input={"tool": "calendar.add"})
    d.spans.finish(tool, output={"text": text})
    d.journal.append("task.answer", {"task_id": tree, "request_id": request, "status": "done",
                                     "text": f"{text}\nDone"})
    d.spans.finish(root, output={"answer": text})
    d.db.conn.execute("INSERT OR REPLACE INTO task_results (task_id, at, status, goal, answer, shown) VALUES "
                      "(?, ?, 'done', 'book it', ?, 1)", (tree, d.clock.now().isoformat(), text))
    return turn.id, tool.id


def _retry_scenario(daemon: RunningDaemon, forget: Any, given: str, said: str) -> None:
    """A task given the item runs at the forget beside another job (a task waiting on an owner question); it ends and
    loses the item; then the owner retries it in place, and the other job's end must leave the retry alone."""
    d = daemon.daemon
    _on_loop(daemon, lambda: d.db.conn.execute("INSERT INTO task_memory_refs VALUES ('t-run', ?)", (given,)))

    def writes() -> None:
        d.journal.append("task.answer", {"task_id": "t-run", "request_id": "r-run", "status": "done",
                                         "text": f"{said}\nAll set"})
    gate1, job1 = _running_across_the_forget(daemon, writes, "r-run", "t-run")
    gate2, job2 = _running_across_the_forget(daemon, lambda: None, "r-other", "t-other")
    real = d.supervisor.running_trees
    d.supervisor.running_trees = lambda: [("t-run", "book it"), ("t-other", "other")]   # as `_tree_of` holds them
    try:
        _on_loop(daemon, forget)
    finally:
        d.supervisor.running_trees = real
    daemon.run(d._settle_forgets())
    _end(daemon, gate1, job1)
    assert _answers(daemon, "task.answer", "task_id", "t-run") == [f"{REMOVED}\nAll set"]
    turn_id, tool_id = _on_loop(daemon, lambda: _retry_in_place(d, "t-run", "r-retry", said))

    def held() -> Any:
        return (_answers(daemon, "task.answer", "request_id", "r-retry"), _span_out(daemon, turn_id),
                _span_out(daemon, tool_id), _task_records(daemon, "t-run")[1],
                _on_loop(daemon, lambda: d.db.conn.execute("SELECT input_json FROM spans WHERE id = ?",
                                                           (turn_id,)).fetchone()[0]))
    before = held()
    _on_loop(daemon, lambda: d._publish("task.finished", {"task_id": "t-run", "status": "done"}))
    daemon.run(d._settle_forgets())
    _end(daemon, gate2, job2)
    assert held() == before


def test_a_task_retried_in_place_after_the_forget_is_never_touched(daemon: RunningDaemon) -> None:
    """Re-review 3, I-B: the work running at a forget was keyed by tree id, and a retry resumes the same tree under a
    new request, so the retry's answer, planner turn (the owner's words), re-read tool output and result were stripped.
    It is keyed by the requests recorded at the forget: a resume in place is new work."""
    d = daemon.daemon
    item = daemon.run(d._memory_add(None, {"content": QUILL, "key": "dentist"}))      # type: ignore[arg-type]
    _retry_scenario(daemon, lambda: d._forget_memory(item["id"]), "memory:" + item["id"],
                    "Booked Dr. Quillfeather for Tuesday")


def test_a_task_retried_in_place_after_a_learned_items_forget_is_never_touched(daemon: RunningDaemon) -> None:
    """The learned-item variant of I-B: a task shown the lesson, retried in place after the forget, keeps it."""
    d = daemon.daemon
    node = _lesson(daemon, "always cc Zorblat on invoices")
    _retry_scenario(daemon, lambda: d._forget_nodes([node["id"]]), node["id"], "Sent; always cc Zorblat on invoices")


def test_a_rerun_drops_a_summary_folding_a_row_the_running_turn_wrote_with_the_item(daemon: RunningDaemon) -> None:
    """Re-review 3, m-1: a re-run kept only the running work's rows among those written with the item, but compared
    journal ids with sequence numbers, so it never dropped a running summary that folded one."""
    d = daemon.daemon
    item = daemon.run(d._memory_add(None, {"content": QUILL, "key": "dentist"}))      # type: ignore[arg-type]

    def writes() -> None:
        reply = d.journal.append("chat.reply", {"session_id": "s1", "request_id": "r-run",
                                                "text": "Your booking is sorted with the clinic.",
                                                "display_text": "Your booking is sorted with the clinic.",
                                                "source_refs": ["memory:" + item["id"]]})
        d.journal.append("chat.summary", {"session_id": "s1", "text": "after: booking sorted with the dentist",
                                          "source_refs": ["entry:" + reply.id]})
    gate, job = _running_across_the_forget(daemon, writes, "r-run")
    _on_loop(daemon, lambda: d._forget_memory(item["id"]))
    daemon.run(d._settle_forgets())
    _end(daemon, gate, job)
    assert _texts(daemon, "chat.summary") == []


def test_a_tree_a_running_request_starts_after_a_node_forget_is_scrubbed_whole(daemon: RunningDaemon) -> None:
    """Re-review 3, m-2: a chat turn still planning at a lesson's forget makes its task afterwards, from what its
    planner was shown before the forget. That tree belongs to the work running at the forget, so it is scrubbed whole:
    its spans, answer, records and card (the node's scopes stay pinned for everything else)."""
    d = daemon.daemon
    lesson = "always cc Zorblat on invoices"
    node = _lesson(daemon, lesson)
    made: dict[str, Any] = {}

    def writes() -> None:
        _new_task(d, "t-late", "r-plan", {"answer": f"Sent; {lesson}"})
        d.db.conn.execute("INSERT OR IGNORE INTO task_memory_refs VALUES ('t-late', ?)", (node["id"],))
        exposure = d.spans.start("context.exposure", task_id="t-late", input={"source_refs": [node["id"]]})
        d.spans.finish(exposure, output={"text": f"Lesson: {lesson}"})
        tool = d.spans.start("tool.call", task_id="t-late", input={"tool": "mail.send"})
        d.spans.finish(tool, output={"text": lesson})
        d.journal.append("task.answer", {"task_id": "t-late", "request_id": "r-plan", "status": "done",
                                         "text": f"Sent; {lesson}\nDone"})
        made.update(exposure=exposure.id, tool=tool.id, card=d.learning.put(
            "episode", "t-late", {"task_id": "t-late", "verified": True, "request": "send the invoices",
                                  "answer": f"Sent; {lesson}"}, [], made_by="code:episode", state="active")["id"])
    gate, job = _running_across_the_forget(daemon, writes, "r-plan")
    _on_loop(daemon, lambda: d._forget_nodes([node["id"]]))
    daemon.run(d._settle_forgets())
    _end(daemon, gate, job)
    assert _answers(daemon, "task.answer", "task_id", "t-late") == [f"{REMOVED}\nDone"]
    assert _span_out(daemon, made["exposure"]) == {"text": f"Lesson: {REMOVED}"}
    assert _span_out(daemon, made["tool"]) == {"text": REMOVED}
    assert _on_loop(daemon, lambda: json.loads(d.db.conn.execute(
        "SELECT outcome_json FROM tasks WHERE id = 't-late'").fetchone()[0])) == {"answer": REMOVED}
    assert _on_loop(daemon, lambda: d.db.conn.execute("SELECT COUNT(*) FROM learning_nodes WHERE id = ?",
                                                      (made["card"],)).fetchone()[0]) == 0


def test_only_a_recorded_trees_finish_queues_a_run_not_a_workers(daemon: RunningDaemon) -> None:
    """Re-review 3, nit: a worker's own `task.finished` (worker.py) matched its tree's recorded request and queued a
    whole extra pass that the next chat turn waited for. Only the recorded tree's finish, or a recorded job's end,
    queues one."""
    d = daemon.daemon
    item = daemon.run(d._memory_add(None, {"content": QUILL, "key": "dentist"}))      # type: ignore[arg-type]
    gate, job = _running_across_the_forget(daemon, lambda: None, "r-run", "t-run")

    def worker() -> None:
        now = d.clock.now().isoformat()
        d.db.conn.execute(
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
            "outcome_json, created_at, updated_at) VALUES ('t-run.worker', 't-run', 't-run', 'r-run', 'book', "
            "'worker', 'worker', '[]', 'running', '{}', ?, ?)", (now, now))
    _on_loop(daemon, worker)
    _on_loop(daemon, lambda: d._forget_memory(item["id"]))
    daemon.run(d._settle_forgets())

    def queued(task_id: str) -> int:
        d._publish("task.finished", {"task_id": task_id, "status": "done"})
        return len(d._forgetting)
    assert _on_loop(daemon, lambda: queued("t-run.worker")) == 0
    assert _on_loop(daemon, lambda: queued("t-run")) == 1
    daemon.run(d._settle_forgets())
    _end(daemon, gate, job)


# ---------- closing round (re-review 4: N-1, N-2) ----------

def _resume(d: Any, tree: str, request: str) -> None:
    """A resume in place (supervisor.run_tree, resume_id): the tasks row's upsert to the new request, its root span."""
    d.db.conn.execute("UPDATE tasks SET request_id = ?, status = 'running', outcome_json = '{}' WHERE id = ?",
                      (request, tree))
    d.spans.start("task", task_id=tree, input={"goal": "book it", "request_id": request})


def _race(daemon: RunningDaemon, forget: Any, given: str, answer: str, tool_out: str) -> dict[str, Any]:
    """T (given the item) runs at the forget; it finishes (answer, result, task.finished) and a correction that raced
    its completion resumes it in place under r-corr before the queued tree-finish run reads `tasks`."""
    d = daemon.daemon
    made: dict[str, Any] = {}
    _on_loop(daemon, lambda: d.db.conn.execute("INSERT INTO task_memory_refs VALUES ('t-run', ?)", (given,)))

    def writes() -> None:
        root = d.spans.start("task", task_id="t-run", input={"goal": "book it", "request_id": "r-run"})
        tool = d.spans.start("tool.call", task_id="t-run", parent_id=root.id, input={"tool": "calendar.add"})
        d.spans.finish(tool, output={"text": tool_out})
        d.journal.append("task.answer", {"task_id": "t-run", "request_id": "r-run", "status": "done",
                                         "text": f"{answer}\nAll set"})
        d.spans.finish(root, output={"answer": answer})
        _result(daemon, "t-run", answer)
        d._publish("task.finished", {"task_id": "t-run", "status": "done"})
        _resume(d, "t-run", "r-corr")                      # the correction carries on in place
        made["tool"] = tool.id
    gate, job = _running_across_the_forget(daemon, writes, "r-run", "t-run")
    real = d.supervisor.running_trees
    d.supervisor.running_trees = lambda: [("t-run", "book it")]
    try:
        _on_loop(daemon, forget)
    finally:
        d.supervisor.running_trees = real
    daemon.run(d._settle_forgets())
    _end(daemon, gate, job)
    return made


def _cleaned(daemon: RunningDaemon, made: dict[str, Any]) -> Any:
    return (_answers(daemon, "task.answer", "request_id", "r-run"), _span_out(daemon, made["tool"]),
            _task_records(daemon, "t-run")[1])


def test_a_correction_resuming_the_task_before_its_run_reads_tasks_leaves_nothing_of_the_item(
        daemon: RunningDaemon) -> None:
    """Re-review 4, N-1: a correction racing the task's completion resumed it in place before the run queued at the
    tree's finish read `tasks`, so that run took the finished run's answer, spans and result as not its own and the
    item stayed. A tree running at the forget stays paired with the request recorded then."""
    d = daemon.daemon
    item = daemon.run(d._memory_add(None, {"content": QUILL, "key": "dentist"}))      # type: ignore[arg-type]
    made = _race(daemon, lambda: d._forget_memory(item["id"]), "memory:" + item["id"],
                 "Booked with Quillfeather on Monday", "Booked with Quillfeather Monday")
    assert _cleaned(daemon, made) == ([f"{REMOVED}\nAll set"], {"text": REMOVED}, [REMOVED])


def test_a_correction_resuming_the_task_before_its_run_after_a_learned_items_forget(daemon: RunningDaemon) -> None:
    """N-1 for a learned item: the finished run's answer and result lose the lesson too."""
    d = daemon.daemon
    lesson = "always cc Zorblat on invoices"
    node = _lesson(daemon, lesson)
    made = _race(daemon, lambda: d._forget_nodes([node["id"]]), node["id"], f"Sent; {lesson}", lesson)
    assert _cleaned(daemon, made) == ([f"{REMOVED}\nAll set"], {"text": REMOVED}, [REMOVED])


def test_a_resume_in_place_waits_for_the_forget_scrub_before_reading_the_task(daemon: RunningDaemon) -> None:
    """N-1, the supervisor's path: a continue, retry or correction reads the task's saved state (its last recorded
    result goes into the resumed goal) and upserts it; it settles forgets first, so the scrub queued as that task
    finished has run, and the state is read again after it."""
    from tests.conftest import chat, install, outcome, sup, task
    d_sup = daemon.daemon.supervisor                     # the real supervisor path
    install(daemon, [sup("task", "Starting", task("tidy the notes folder", "owner"))],
            {"worker": json.dumps({"status": "done", "summary": "Tidied.", "artifacts": [], "evidence": []})})
    first = outcome(chat(daemon, "tidy the notes folder"))
    calls: list[str] = []
    settle, load = d_sup.settle, d_sup.continuity.load

    async def settled() -> None:
        calls.append("settle")
        await settle()

    def loaded(*args: Any) -> Any:
        calls.append("load")
        return load(*args)
    d_sup.settle, d_sup.continuity.load = settled, loaded
    retry = json.loads(sup("task", "Retrying", task("tidy the notes folder again", "owner")))
    retry["control"] = {"task_id": first["task_id"], "action": "retry", "standing_sends": "unchanged",
                        "supersedes": []}
    install(daemon, [json.dumps(retry)],
            {"worker": json.dumps({"status": "done", "summary": "Tidied again.", "artifacts": [], "evidence": []})})
    try:
        assert outcome(chat(daemon, "try that again"))["status"] == "done"
    finally:
        d_sup.settle, d_sup.continuity.load = settle, load
    # the turn's settle, the planner output's check and the control path's read, then the resume's own read after a
    # settle
    assert calls[:3] == ["settle", "load", "load"] and calls[-2:] == ["settle", "load"]


def test_an_identity_forgets_tree_started_since_by_a_message_planning_at_it_loses_the_statement(
        daemon: RunningDaemon) -> None:
    """Re-review 4, N-2: a message still being planned at an identity forget (the statement in its planner's system
    prompt) starts its task a minute later; that tree is the work running at the forget, so its answer, spans and
    outcome lose the statement as a tree running then does (the identity's "since" scope still ends at the forget for
    everything else)."""
    d = daemon.daemon
    node = _identity(daemon, "I sign emails as Kestrovan")
    made: dict[str, Any] = {}

    def writes() -> None:
        d.clock.advance(60)                              # the task starts after the forget
        _new_task(d, "t-late", "r-plan", {"answer": "I sign emails as Kestrovan"})
        root = d.spans.start("task", task_id="t-late", input={"goal": "sign as Kestrovan", "request_id": "r-plan"})
        tool = d.spans.start("tool.call", task_id="t-late", parent_id=root.id, input={"tool": "mail.draft"})
        d.spans.finish(tool, output={"text": "I sign emails as Kestrovan"})
        d.journal.append("task.answer", {"task_id": "t-late", "request_id": "r-plan", "status": "done",
                                         "text": "Signed it: I sign emails as Kestrovan\nDone"})
        d.spans.finish(root, output={"answer": "x"})
        made["tool"] = tool.id
    gate, job = _running_across_the_forget(daemon, writes, "r-plan")
    _on_loop(daemon, lambda: d._forget_nodes([node["id"]]))
    daemon.run(d._settle_forgets())
    _end(daemon, gate, job)
    assert _answers(daemon, "task.answer", "task_id", "t-late") == [f"{REMOVED}\nDone"]
    assert _span_out(daemon, made["tool"]) == {"text": REMOVED}
    assert _on_loop(daemon, lambda: json.loads(d.db.conn.execute(
        "SELECT outcome_json FROM tasks WHERE id = 't-late'").fetchone()[0])) == {"answer": REMOVED}
