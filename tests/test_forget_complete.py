"""Memory review I2 (fix wave 3): forget is complete, and the chat can forget.

Forgetting an item also removes what FERB's own records say of it: the code line "Remembered: …", FERB's whole answer
to the forgotten message, and each line of FERB's replies given the item that states it, wherever it was recorded
(journal rows, the planner's stored turns and exposures). A forget asked in chat waits for a plain Yes or No to a
question that names what goes, like `ferb memory forget`. Ported from the review's probes P2 and P7."""
import json
import re
from typing import Any

from ferb.memory.echoes import REMOVED
from tests.conftest import RunningDaemon, chat, install
from tests.support.forgets import (QUILL, _answers, _held_in_spans, _identity, _lesson, _let_go, _on_loop,
                                  _remember, _reply, _result, _running_across_the_forget, _task_records)


def _holding(d: RunningDaemon, word: str) -> tuple[list[str], list[str]]:
    db = d.daemon.db
    return _on_loop(d, lambda: (
        [r[0] for r in db.conn.execute("SELECT kind FROM journal WHERE data_json LIKE ?", (f"%{word}%",))],
        [r[0] for r in db.conn.execute("SELECT kind FROM spans WHERE input_json LIKE ? OR output_json LIKE ?",
                                       (f"%{word}%", f"%{word}%"))]))


def test_forget_removes_ferbs_remembered_line_and_the_planner_no_longer_reads_it(daemon: RunningDaemon) -> None:
    """P2 and P7: after a forget, FERB's "Remembered: …" reply and the planner's stored turns still held the item, and
    the next planner turn read it from the chat."""
    item = _remember(daemon, "remember that my dentist is Dr. Quillfeather", "my dentist is Dr. Quillfeather",
                     "dentist")
    install(daemon, [_reply("Noted.")])
    chat(daemon, "what else should I know?")
    _on_loop(daemon, lambda: daemon.daemon.memory.forget(item.id))
    assert _holding(daemon, "Quillfeather") == ([], [])
    script = install(daemon, [_reply("Hi.")])
    chat(daemon, "who is my dentist?")
    context = "\n".join(str(c) for _, key, c, _ in script.contexts if key == "supervisor")
    assert "Quillfeather" not in context


def test_forget_removes_a_reply_line_that_stated_the_item_in_other_words(daemon: RunningDaemon) -> None:
    """FERB answered from the item in its own words. That line goes; the rest of the chat stays."""
    item = _remember(daemon, "remember that my test lab colour is teal-otter", "my test lab colour is teal-otter",
                     "test_lab_colour")
    install(daemon, [_reply("Your test lab colour is teal-otter.\nAnything else?")])
    chat(daemon, "what is my test lab colour?")
    _on_loop(daemon, lambda: daemon.daemon.memory.forget(item.id))
    journal, spans = _holding(daemon, "otter")
    assert journal == [] and spans == []
    rows = _on_loop(daemon, lambda: [(e.kind, e.data.get("text")) for e in daemon.daemon.journal.entries_of(
        "chat.message", "chat.reply")])
    assert ("chat.message", "what is my test lab colour?") in rows                # the owner's question stays
    assert any(kind == "chat.reply" and "Anything else?" in str(text) for kind, text in rows)
    script = install(daemon, [_reply("I don't know.")])
    chat(daemon, "what is my test lab colour?")
    assert not re.search("teal|otter", "\n".join(str(c) for _, k, c, _ in script.contexts if k == "supervisor"))


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
    assert "Forgot" in told
    daemon.run(daemon.daemon._settle_forgets())          # its echo scrub finishes off the loop (the next turn waits)
    assert _holding(daemon, "heron")[0] == []


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


def test_forget_reaches_what_a_task_did_with_the_item(daemon: RunningDaemon) -> None:
    """Review Q1: the planner was given the fact (Layer 4) and the task's answer used it. After the forget, the card,
    the task's answer and outcome, its stored records and the next planner turn no longer hold it."""
    from tests.conftest import done, outcome, sup, task
    d = daemon.daemon
    item = _added(daemon, "My dentist is Dr. Zorblat", "dentist")
    install(daemon, [sup("task", "Emailing.", task("email my dentist to book Monday"))],
            {"worker": done("Emailed Dr. Zorblat's office to book Monday.")})
    task_id = outcome(chat(daemon, "email my dentist to book Monday"))["task_id"]
    daemon.run(d.supervisor.finish_episodes())
    assert "Zorblat" in json.dumps(_on_loop(daemon, lambda: d.learning.current_card(task_id)))
    _on_loop(daemon, lambda: d.memory.forget(item.id))
    assert _on_loop(daemon, lambda: d.learning.current_card(task_id)) is None
    assert _on_loop(daemon, lambda: d.learning.prepare_episode(task_id)) is None          # never rebuilt
    journal, spans = _holding(daemon, "Zorblat")
    assert journal == [] and spans == []
    held = _on_loop(daemon, lambda: [r[0] for r in d.db.conn.execute(
        "SELECT 'episode' FROM episodes WHERE answer LIKE '%Zorblat%' OR goal LIKE '%Zorblat%' UNION ALL "
        "SELECT 'task' FROM tasks WHERE outcome_json LIKE '%Zorblat%' OR goal LIKE '%Zorblat%'")])
    assert held == []
    assert not _on_loop(daemon, lambda: [p for p in d.learning.search("who is my dentist") if p["kind"] == "episode"])
    script = install(daemon, [_reply("Hi.")])
    chat(daemon, "who is my dentist?")
    assert "Zorblat" not in "\n".join(str(c) for _, key, c, _ in script.contexts if key == "supervisor")


def test_forget_reaches_a_loops_round_results_note_and_baseline(daemon: RunningDaemon) -> None:
    """L31 (D14): a loop step's stored state holds model-written round results, a probe refusal note and an outside
    baseline value, and the next round is shown the results and the note. A task given the item loses its lines there
    too; another task's loop keeps its own."""
    from tests.conftest import done, outcome, sup, task
    d = daemon.daemon
    item = _added(daemon, "My dentist is Dr. Zorblat", "dentist")
    install(daemon, [sup("task", "Emailing.", task("email my dentist to book Monday"))],
            {"worker": done("Emailed Dr. Zorblat's office to book Monday.")})
    task_id = outcome(chat(daemon, "email my dentist to book Monday"))["task_id"]

    def seed() -> None:
        for tree in (task_id, "tree-other"):
            d.supervisor.loops.load(tree, "keep")
            d.supervisor.loops.save(tree, "keep", note="the probe could not read: no reply from Dr. Zorblat yet",
                                    recent_json=json.dumps(["done: Asked Dr. Zorblat's office again.",
                                                            "done: Checked the inbox."]),
                                    baseline_json=json.dumps({"from": "Dr. Zorblat", "unread": 2}))
    _on_loop(daemon, seed)
    _on_loop(daemon, lambda: d.memory.forget(item.id))

    def row(tree: str) -> dict[str, Any]:
        return dict(d.db.conn.execute("SELECT recent_json, note, baseline_json FROM loop_state WHERE task_id = ?",
                                      (tree,)).fetchone())
    given, other = _on_loop(daemon, lambda: row(task_id)), _on_loop(daemon, lambda: row("tree-other"))
    assert "Zorblat" not in json.dumps(given)
    assert "done: Checked the inbox." in json.loads(given["recent_json"])
    assert json.loads(given["baseline_json"])["unread"] == 2
    assert "Zorblat" in other["note"] and "Zorblat" in other["recent_json"]


def test_a_reply_sharing_one_word_with_the_item_keeps_its_line(daemon: RunningDaemon) -> None:
    """Review Q2: a reply about something else that shares one rare word with the item was emptied."""
    item = _added(daemon, "My wifi hint is the blue router", "wifi_hint")
    install(daemon, [_reply("The router light is blue when it updates.\nAnything else?")])
    chat(daemon, "what does my router light colour mean?")
    _on_loop(daemon, lambda: daemon.daemon.memory.forget(item.id))
    texts = _on_loop(daemon, lambda: [e.data.get("text") for e in daemon.daemon.journal.of_kind("chat.reply")])
    assert "The router light is blue when it updates.\nAnything else?" in texts


def test_a_chat_forget_quote_must_be_a_whole_clause_of_the_owners_words(daemon: RunningDaemon) -> None:
    """Review Q4: a single common word the owner typed ("the") was enough for FERB to ask about deleting a memory."""
    item = _added(daemon, "My dentist is Dr. Zorblat", "dentist")
    install(daemon, [_reply("Okay.", forget=[{"item_id": item.id, "quote": "the"}])])
    chat(daemon, "summarize the page I'm looking at")
    assert _forget_question(daemon) is None
    install(daemon, [_reply("Okay.", forget=[{"item_id": item.id, "quote": "please forget my dentist"}])])
    chat(daemon, "please forget my dentist, thanks")
    assert _forget_question(daemon) is not None


def test_the_echo_scrub_runs_off_the_event_loop_and_the_next_turn_waits_for_it(daemon: RunningDaemon) -> None:
    """Review minor: forget's echo scrub (about 2.2 s at scale) ran on the event loop. It runs in a worker thread on
    its own connection; the RPC returns when it is done, and a chat turn waits for one still running."""
    import threading
    from ferb.cli.client import Client
    from ferb.memory import echoes
    item = _remember(daemon, "remember that my dentist is Dr. Quillfeather", "my dentist is Dr. Quillfeather",
                     "dentist")
    ran: list[str] = []
    real = echoes.scrub

    def recorded(db: Any, forgotten: Any, *rest: Any) -> Any:
        ran.append(threading.current_thread().name)
        return real(db, forgotten, *rest)
    echoes.scrub = recorded                                     # type: ignore[assignment]
    try:
        c = Client(daemon.sock)
        try:
            c.call("memory.forget", {"id": item.id, "confirm": True})
        finally:
            c.close()
    finally:
        echoes.scrub = real                                     # type: ignore[assignment]
    assert ran and ran[0] != daemon.thread.name
    assert _holding(daemon, "Quillfeather") == ([], [])


def test_a_forget_answered_in_chat_is_complete_before_the_next_turn(daemon: RunningDaemon) -> None:
    item = _remember(daemon, "remember that my test lab colour is violet-heron", "my test lab colour is violet-heron",
                     "test_lab_colour")
    install(daemon, [_reply("Okay.", forget=[{"item_id": item.id, "quote": "forget my test lab colour"}])])
    chat(daemon, "forget my test lab colour")
    question = _forget_question(daemon)
    install(daemon, [json.dumps({"kind": "reply", "text": "", "answer_question": {"approval_id": question.id,
                                                                                    "answer": "Yes"}})])
    chat(daemon, "Yes")
    script = install(daemon, [_reply("Hi.")])
    chat(daemon, "what is my test lab colour?")
    assert "heron" not in "\n".join(str(c) for _, key, c, _ in script.contexts if key == "supervisor")


# ---------- round 2 (re-review F1, F2) ----------

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


def test_forget_leaves_nothing_of_the_item_in_what_a_task_did_with_it(daemon: RunningDaemon) -> None:
    """Re-review F1 (probe a): the planner put the name in the goal and its plan line, the worker wrote it in a tool
    call and its answer. After the forget, no table holds it: not the task's tool-call spans, the planner's intake
    turn, or the "while you were away" result."""
    from tests.conftest import outcome, sup, task, tool
    d = daemon.daemon
    item = _added(daemon, "My dentist is Dr. Zorblat", "dentist")

    def act(token: str) -> str:
        r = tool(daemon.sock, token, "fs.write", path="note.txt", content="Dear Dr. Zorblat,\nPlease book Monday.",
                 overwrite=True, effect="edit")
        return json.dumps({"status": "failed" if r["is_error"] else "done",
                           "summary": "Wrote the note to Dr. Zorblat asking for Monday.", "artifacts": ["note.txt"],
                           "evidence": []})
    install(daemon, [sup("task", "Writing to Dr. Zorblat.", task("write a note to Dr. Zorblat to book Monday"))],
            {"worker": act})
    outcome(chat(daemon, "write my dentist a note to book Monday"))
    daemon.run(d.supervisor.finish_episodes())
    assert _everywhere(daemon, "Zorblat")
    from ferb.cli.client import Client
    c = Client(daemon.sock)
    try:
        c.call("memory.forget", {"id": item.id, "confirm": True})
    finally:
        c.close()
    assert _everywhere(daemon, "Zorblat") == []


def _procedure_scene(d: RunningDaemon, answer: str, steps: list[Any] | None = None,
                     request: str = "email Carol to book Tuesday") -> tuple[Any, str, Any]:
    """The item captured from a chat message E1, a card citing E1, a procedure built on that card, and a task T3 shown
    only that procedure (review probe f)."""
    from tests.conftest import done, outcome, sup, task
    dd = d.daemon
    e1 = str(_on_loop(d, lambda: dd._journal().append("chat.message", {"text": "remember my dentist is Dr. Zorblat",
                                                                        "request_id": "r-e1"}).id))
    item = _on_loop(d, lambda: dd.memory.propose("person", "owner", "dentist", "my dentist is Dr. Zorblat",
                                                 "owner_stated", [("journal", e1)], 1.0))
    c1 = _on_loop(d, lambda: dd.learning.put("episode", "T1", {"task_id": "T1", "verified": True, "request": "x",
                                                               "apps": {}, "slots": {}, "status": "done",
                                                               "actions": []}, ["journal:" + e1],
                                             origin_label="ferb", made_by="code:evidence-card/2", state="active"))
    procedure = _on_loop(d, lambda: dd.learning.put("procedure", "book appointment", {
        "intent": "book appointment", "steps": steps or [{"tool": "gmail.send"}], "preconditions": {"apps": {}}},
        [c1["id"]], made_by="model:t", state="candidate"))
    dd.supervisor.memory_candidates = lambda words, focus: []                      # T3's planner sees no fact
    install(d, [sup("task", "Booking.", task(request))], {"worker": done(answer)})
    t3 = outcome(chat(d, request))["task_id"]
    _on_loop(d, lambda: dd.db.conn.execute("INSERT OR IGNORE INTO task_memory_refs VALUES (?,?)",
                                           (t3, procedure["id"])))
    d.run(dd.supervisor.finish_episodes())
    return item, t3, procedure


def test_a_task_shown_only_a_node_built_on_the_items_source_is_not_given_the_item(daemon: RunningDaemon) -> None:
    """Re-review F2 (probe f): T3 was shown only a procedure built on a card that cites the item's source message. The
    procedure doesn't state the item, so T3 wasn't given it: its rows and its card stay."""
    d = daemon.daemon
    item, t3, _ = _procedure_scene(daemon, "Emailed Dr. Carol to book Tuesday.\nUsed the booking template.")
    _on_loop(daemon, lambda: d.memory.forget(item.id))
    rows = _on_loop(daemon, lambda: [e.data.get("text") for e in d.journal.of_kind("task.answer")])
    assert rows == ["Emailed Dr. Carol to book Tuesday.\nUsed the booking template."]
    assert _on_loop(daemon, lambda: d.learning.current_card(t3)) is not None


def test_a_card_goes_only_when_its_body_restates_the_item(daemon: RunningDaemon) -> None:
    """Re-review F2 ruling: a task given the item (its planner was shown the fact) whose card shares one rare word with
    it keeps its card; a card is forgotten (and tombstoned) only when its body restates the item."""
    from tests.conftest import done, outcome, sup, task
    d = daemon.daemon
    item = _added(daemon, "My dentist is Dr. Zorblat", "dentist")
    install(daemon, [sup("task", "Checking.", task("check the clinic page"))],
            {"worker": done("The clinic page lists Zorblat Street as its address.")})
    one_word = outcome(chat(daemon, "check my dentist clinic page"))["task_id"]
    install(daemon, [sup("task", "Emailing.", task("email my dentist to book Monday"))],
            {"worker": done("Emailed Dr. Zorblat's office to book Monday.")})
    restating = outcome(chat(daemon, "email my dentist to book Monday"))["task_id"]
    daemon.run(d.supervisor.finish_episodes())
    _on_loop(daemon, lambda: d.memory.forget(item.id))
    assert _on_loop(daemon, lambda: d.learning.current_card(one_word)) is not None
    assert _on_loop(daemon, lambda: d.learning.current_card(restating)) is None


# ---------- round 3 (re-review NEW-1, NEW-2, f3, a3) ----------

def _paraphrased(d: RunningDaemon, content: str = "Owner's boss is Quentarr Vexley") -> Any:
    """FERB's wording of what the owner said (an inferred item) with its pending Yes/No question."""
    from ferb.orchestrator.schemas import Remember
    dd = d.daemon
    said = "my manager is Quentarr"
    src = str(_on_loop(d, lambda: dd._journal().append("chat.message", {"text": said}).id))
    _on_loop(d, lambda: dd._remember([Remember(kind="person", subject="owner", key="boss",
                                               content=content, quote=said)], said, src))
    return _on_loop(d, lambda: next(i for i in dd.memory.items(None, valid_now=False) if i.key == "boss"))


def _memory_questions(d: RunningDaemon) -> list[Any]:
    return _on_loop(d, lambda: [q for q in d.daemon.state.approvals.pending()
                                if q.capability in ("memory.confirm", "memory.forget")])


def test_a_forget_closes_the_pending_question_about_that_memory(daemon: RunningDaemon) -> None:
    """Re-review NEW-1 (probe a2): the owner forgot FERB's wording from the dashboard/CLI instead of answering its
    question; the question stayed, kept the forgotten text, and every later planner turn read it again."""
    item = _paraphrased(daemon)
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


def test_a_chat_forget_of_a_pending_proposal_closes_its_question(daemon: RunningDaemon) -> None:
    """Probe a5: "forget my manager" → Yes removed the item but left the "Keep this…?" question waiting."""
    item = _paraphrased(daemon)
    install(daemon, [_reply("Okay.", forget=[{"item_id": item.id, "quote": "forget my manager"}])])
    chat(daemon, "forget my manager")
    forget = next(q for q in _memory_questions(daemon) if q.capability == "memory.forget")
    _on_loop(daemon, lambda: daemon.daemon._resolve_approval(forget.id, True, 1))
    daemon.run(daemon.daemon._settle_forgets())
    assert _memory_questions(daemon) == []
    assert _everywhere(daemon, "Vexley") == []


def test_a_dashboard_confirm_closes_the_chat_question(daemon: RunningDaemon) -> None:
    """The owner confirmed FERB's wording on the dashboard: the chat question goes, so a later No can't retract it."""
    from ferb.cli.client import Client
    item = _paraphrased(daemon)
    c = Client(daemon.sock)
    try:
        [node] = [n for n in c.call("memory.learning", {"op": "inventory"})["items"] if n["kind"] == "fact"]
        c.call("memory.learning", {"op": "confirm", "id": node["id"]})
    finally:
        c.close()
    assert _memory_questions(daemon) == []
    assert _on_loop(daemon, lambda: daemon.daemon.memory.get(item.id)).status == "active"


def test_a_task_shown_a_node_naming_a_rare_word_of_the_item_is_given_it_for_its_lines(daemon: RunningDaemon) -> None:
    """Re-review f3 and NEW-2 (probe f5, controller ruling): T5 was shown only a procedure, built on the item's source,
    whose body names "Zorblat office"; T1 was shown the fact itself. Both wrote "Emailed Zorblat…". A node holding
    the item's text or one of its rare words makes a task given the item for line scrubbing, and the rare words are
    measured once against every candidate (they no longer change with who was kept): both lose the line. A card still
    goes only when its body restates the item."""
    from tests.conftest import done, outcome, sup, task
    d = daemon.daemon
    item, t5, _ = _procedure_scene(daemon, "Emailed Zorblat to book Tuesday.\nUsed the booking template.",
                                   [{"tool": "gmail.send", "args": {"to": "Zorblat office"}}],
                                   "email my dentist to book Tuesday")
    d.supervisor.memory_candidates = None                                     # T1's planner is shown the fact
    _on_loop(daemon, lambda: d.learning.sync_facts(d.memory))
    install(daemon, [sup("task", "Booking.", task("email my dentist to book Friday"))],
            {"worker": done("Emailed Zorblat to book Friday.\nDone.")})
    t1 = outcome(chat(daemon, "email my dentist to book Friday"))["task_id"]
    fact = _on_loop(daemon, lambda: [n["id"] for n in d.learning.items("fact")])
    _on_loop(daemon, lambda: d.db.conn.executemany("INSERT OR IGNORE INTO task_memory_refs VALUES (?,?)",
                                                   [(t1, f) for f in fact]))
    daemon.run(d.supervisor.finish_episodes())
    _on_loop(daemon, lambda: d.memory.forget(item.id))
    answers = _on_loop(daemon, lambda: {e.data.get("task_id"): e.data.get("text")
                                        for e in d.journal.of_kind("task.answer")})
    assert "Zorblat" not in answers[t5] and "Used the booking template." in answers[t5]
    assert "Zorblat" not in answers[t1] and "Done." in answers[t1]
    assert _on_loop(daemon, lambda: d.learning.current_card(t5)) is not None      # one rare word: the card stays


def test_a_task_that_read_the_item_with_a_memory_tool_is_given_it(daemon: RunningDaemon) -> None:
    """Re-review a3 (probe a3): a worker's memory.read of the item wasn't recorded on the task (only its exposures
    were), so a task that read the item before its card existed wasn't given it. The read is recorded when it
    happens, and the forget reaches the task's answer."""
    from tests.conftest import done, outcome, sup, task, tool
    d = daemon.daemon
    item = _added(daemon, "My dentist is Dr. Zorblat", "dentist")
    d.supervisor.memory_candidates = lambda words, focus: []                       # the planner is shown no memory
    d.supervisor.on_episode = lambda task_id: None                          # and no card is built

    def act(token: str) -> str:
        tool(daemon.sock, token, "memory.read", item_id=item.id)
        return done("Booked with Zorblat for Monday.\nThe office confirmed.")
    install(daemon, [sup("task", "Booking.", task("book my usual appointment for Monday"))], {"worker": act})
    task_id = outcome(chat(daemon, "book my usual appointment for Monday"))["task_id"]
    refs = _on_loop(daemon, lambda: [tuple(r) for r in d.db.conn.execute(
        "SELECT task_id, ref FROM task_memory_refs WHERE task_id = ?", (task_id,))])
    assert (task_id, "memory:" + item.id) in refs
    _on_loop(daemon, lambda: d.memory.forget(item.id))
    answers = _on_loop(daemon, lambda: [e.data.get("text") for e in d.journal.of_kind("task.answer")])
    assert answers and "Zorblat" not in answers[-1] and "The office confirmed." in answers[-1]


# ---------- round 4 (re-review R1-R5) ----------

def _asked_to_forget(d: RunningDaemon, item: Any) -> Any:
    """The owner's "forget my manager" in chat: FERB's "Forget for good?" question about the item, now waiting."""
    install(d, [_reply("Okay.", forget=[{"item_id": item.id, "quote": "forget my manager"}])])
    chat(d, "forget my manager")
    return next(q for q in _memory_questions(d) if q.capability == "memory.forget")


def _forget_still_answers(d: RunningDaemon, item: Any, forget: Any) -> None:
    """The owner's forget question still waits after the other answer, and its Yes leaves nothing of the item."""
    assert [q.id for q in _memory_questions(d)] == [forget.id]
    assert _on_loop(d, lambda: [r[0] for r in d.daemon.db.conn.execute("SELECT kind FROM memory_questions")]) == [
        "forget:memory"]
    _on_loop(d, lambda: d.daemon._resolve_approval(forget.id, True, 1))
    d.run(d.daemon._settle_forgets())
    assert _on_loop(d, lambda: d.daemon.memory.get(item.id)) is None
    assert _memory_questions(d) == []
    assert _everywhere(d, "Vexley") == []


def test_a_no_to_keep_this_leaves_the_owners_forget_question_waiting(daemon: RunningDaemon) -> None:
    """Re-review R1 (probe h1): with both "Keep this?" and the owner's "Forget for good?" waiting, a No to "Keep this?"
    closed the forget question too; the item stayed, retracted, with its text in memory, search, learning, the journal
    and spans. A confirm or a retract closes only the "Keep this?" question; a forget question closes only when its
    memory is deleted."""
    item = _paraphrased(daemon)
    forget = _asked_to_forget(daemon, item)
    keep = next(q for q in _memory_questions(daemon) if q.capability == "memory.confirm")
    _on_loop(daemon, lambda: daemon.daemon._resolve_approval(keep.id, False, None))
    assert _on_loop(daemon, lambda: daemon.daemon.memory.get(item.id)).status == "retracted"
    _forget_still_answers(daemon, item, forget)


def test_a_guess_dropped_through_the_notch_op_leaves_the_owners_forget_question_waiting(daemon: RunningDaemon) -> None:
    """L36 (scan I1) through the notch's entry point: `memory.learning {op: guess, action: drop}` while the owner's
    "Forget for good?" waits retracts the guess instead of deleting it, and the forget's Yes still purges it."""
    item = _paraphrased(daemon)
    forget = _asked_to_forget(daemon, item)
    reply = daemon.run(daemon.daemon._memory_learning(None, {"op": "guess", "id": item.id, "action": "drop"}))
    assert "still waits" in reply["line"]
    assert _on_loop(daemon, lambda: daemon.daemon.memory.get(item.id)).status == "retracted"
    _forget_still_answers(daemon, item, forget)


def test_a_dashboard_retract_leaves_the_owners_forget_question_waiting(daemon: RunningDaemon) -> None:
    """Probe h1b: the same through the dashboard's Retract."""
    from ferb.cli.client import Client
    item = _paraphrased(daemon)
    forget = _asked_to_forget(daemon, item)
    c = Client(daemon.sock)
    try:
        c.call("memory.retract", {"id": item.id})
    finally:
        c.close()
    _forget_still_answers(daemon, item, forget)


def test_a_yes_to_keep_this_leaves_the_owners_forget_question_waiting(daemon: RunningDaemon) -> None:
    """Probe h1c: a Yes to "Keep this?" confirms the wording and closes that question only."""
    item = _paraphrased(daemon)
    forget = _asked_to_forget(daemon, item)
    keep = next(q for q in _memory_questions(daemon) if q.capability == "memory.confirm")
    _on_loop(daemon, lambda: daemon.daemon._resolve_approval(keep.id, True, 1))
    assert _on_loop(daemon, lambda: daemon.daemon.memory.get(item.id)).status == "active"
    _forget_still_answers(daemon, item, forget)


def test_a_question_showing_ferbs_wording_space_normalised_leaves_no_copy_in_a_span(daemon: RunningDaemon) -> None:
    """Re-review R2 (probes h4, h4b): the question shows FERB's wording with its spacing normalised, and a planner turn
    recorded it so; the echo pass matched spans by the raw text only, so a double space or a line break inside the
    wording left the question's copy in that turn after the forget."""
    from ferb.cli.client import Client
    for content, word in [("Owner's boss is  Quentarr Vexley", "Vexley"), ("Owner's boss is\nQuentarr Zyxwell", "Zyxwell")]:
        item = _paraphrased(daemon, content)
        install(daemon, [_reply("Hi.")])
        chat(daemon, "what's on today?")
        assert _everywhere(daemon, word)
        c = Client(daemon.sock)
        try:
            c.call("memory.forget", {"id": item.id, "confirm": True})
        finally:
            c.close()
        assert _everywhere(daemon, word) == [], content


def test_forgetting_a_learned_item_leaves_no_copy_of_its_question_in_a_span(daemon: RunningDaemon) -> None:
    """Re-review R3 (probe h9): learned-item forgets had no echo pass. The owner's "Forget for good?" question about a
    lesson, shown in a later planner turn, stayed in that turn's span after the dashboard forgot the lesson."""
    from ferb.cli.client import Client
    node = _lesson(daemon, "use brave zentrix")
    _on_loop(daemon, lambda: daemon.daemon._chat_forget(node["id"], "r-x"))
    install(daemon, [_reply("Hi.")])
    chat(daemon, "what's on today?")
    assert ("spans", "input_json") in _everywhere(daemon, "zentrix")
    c = Client(daemon.sock)
    try:
        c.call("memory.learning", {"op": "forget", "id": node["id"], "confirm": True})
    finally:
        c.close()
    assert _memory_questions(daemon) == []
    assert _everywhere(daemon, "zentrix") == []


def test_a_learned_item_forgotten_from_chat_leaves_no_copy_in_ferbs_replies_or_spans(daemon: RunningDaemon) -> None:
    """R3, from chat: FERB's question naming the lesson (by its key, here its scope and trigger) is its reply in the
    conversation record; after the owner's Yes, neither that reply nor any span holds the lesson's key or text. The
    owner's own words stay as they typed them."""
    node = _on_loop(daemon, lambda: daemon.daemon.learning.put(
        "lesson", '{"app": "Mail"}:send greyed out in zentrix', {"text": "wait for the draft to save in vorpalt",
                                                                 "trigger": "send greyed out in zentrix"},
        [], made_by="model:t", state="active"))
    install(daemon, [_reply("Okay.", forget=[{"item_id": node["id"], "quote": "forget that lesson"}])])
    asked = chat(daemon, "forget that lesson")
    assert any("zentrix" in str(e.get("text")) for e in asked)
    question = _forget_question(daemon)
    install(daemon, [json.dumps({"kind": "reply", "text": "", "answer_question": {"approval_id": question.id,
                                                                                    "answer": "Yes"}})])
    chat(daemon, "Yes")
    daemon.run(daemon.daemon._settle_forgets())
    assert _on_loop(daemon, lambda: daemon.daemon.learning.get(node["id"])) is None
    assert _everywhere(daemon, "zentrix") == [] and _everywhere(daemon, "vorpalt") == []
    texts = _on_loop(daemon, lambda: [e.data.get("text") for e in daemon.daemon.journal.of_kind("chat.message")])
    assert "forget that lesson" in texts


def test_a_failure_to_record_a_memory_read_never_costs_the_read(daemon: RunningDaemon) -> None:
    """Re-review R4 (probe h6c): when recording a worker's memory read (`Gateway.memory_read`) raised, the memory.read
    call failed with an RPC error and its tool.call span was never finished. The read keeps its result and its span;
    the failure is journaled, redacted."""
    from tests.conftest import done, outcome, sup, task, tool
    d = daemon.daemon
    item = _added(daemon, "My dentist is Dr. Zorblat", "dentist")
    secret = "sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWX"

    def boom(task_id: str, refs: list[str]) -> None:
        raise RuntimeError(f"database is locked ({secret})")
    d.gateway.memory_read = boom
    got: dict[str, Any] = {}

    def act(token: str) -> str:
        got["read"] = tool(daemon.sock, token, "memory.read", item_id=item.id)
        return done("Read the dentist's name.")
    install(daemon, [sup("task", "Reading.", task("read my dentist's name from memory"))], {"worker": act})
    outcome(chat(daemon, "what's my dentist's name?"))
    assert got["read"]["is_error"] is False and "Zorblat" in json.dumps(got["read"]["content"])
    ended = _on_loop(daemon, lambda: [r[0] for r in d.db.conn.execute(
        "SELECT ended_at FROM spans WHERE kind = 'tool.call' AND json_extract(input_json, '$.tool') = 'memory.read'")])
    assert ended and None not in ended
    problems = _on_loop(daemon, lambda: [e.data for e in d.journal.of_kind("memory.read_unrecorded")])
    assert len(problems) == 1 and "RuntimeError" in problems[0]["error"]
    assert secret not in json.dumps(problems)


def test_rare_words_are_measured_once_against_every_candidate_task(daemon: RunningDaemon) -> None:
    """Re-review test gap for ruling (a) (probe h8): T3, shown only a procedure that doesn't name the item (so not
    given it), wrote "Emailed Zorblat…"; T1, shown the fact, wrote the same. The rare words are measured once against
    every candidate task, so T3's rows don't make "zorblat" common: T1 loses the line and T3 keeps its rows. Rules
    measured again against only the kept tasks would leave T1's line."""
    from tests.conftest import done, outcome, sup, task
    d = daemon.daemon
    item, t3, _ = _procedure_scene(daemon, "Emailed Zorblat to book Tuesday.\nUsed the booking template.")
    d.supervisor.memory_candidates = None                                     # T1's planner is shown the fact
    _on_loop(daemon, lambda: d.learning.sync_facts(d.memory))
    install(daemon, [sup("task", "Booking.", task("email my dentist to book Friday"))],
            {"worker": done("Emailed Zorblat to book Friday.\nDone.")})
    t1 = outcome(chat(daemon, "email my dentist to book Friday"))["task_id"]
    fact = _on_loop(daemon, lambda: [n["id"] for n in d.learning.items("fact")])
    _on_loop(daemon, lambda: d.db.conn.executemany("INSERT OR IGNORE INTO task_memory_refs VALUES (?,?)",
                                                   [(t1, f) for f in fact]))

    def answers() -> dict[str, Any]:
        return _on_loop(daemon, lambda: {e.data.get("task_id"): e.data.get("text")
                                         for e in d.journal.of_kind("task.answer")})
    before = answers()
    _on_loop(daemon, lambda: d.memory.forget(item.id))
    after = answers()
    assert after[t3] == before[t3]
    assert "Zorblat" not in after[t1] and "Done." in after[t1]


def _pending_rows(d: RunningDaemon) -> int:
    return _on_loop(d, lambda: d.daemon.db.conn.execute("SELECT COUNT(*) FROM memory_questions").fetchone()[0])


def test_forgetting_a_memory_closes_the_question_about_a_learned_item_built_on_it(daemon: RunningDaemon) -> None:
    """Re-review test gap (probe h3b): a procedure built on the item's fact had a waiting "Forget for good?"; the
    item's forget deletes the procedure (`_forget_learning_sources`), so its question closes, and the procedure's text
    that question quoted leaves the planner turn that showed it (R3)."""
    d = daemon.daemon
    item = _added(daemon, "My dentist is Dr. Zorblat", "dentist")
    _on_loop(daemon, lambda: d.learning.sync_facts(d.memory))
    [fact] = _on_loop(daemon, lambda: d.learning.items("fact"))
    proc = _on_loop(daemon, lambda: d.learning.put("procedure", "book qindlewort visit", {
        "intent": "book qindlewort visit", "steps": []}, [fact["id"]], made_by="model:t", state="candidate"))
    _on_loop(daemon, lambda: d._chat_forget(proc["id"], "r-y"))
    install(daemon, [_reply("Hi.")])
    chat(daemon, "what's on today?")
    assert _everywhere(daemon, "qindlewort")
    from ferb.cli.client import Client
    c = Client(daemon.sock)
    try:
        c.call("memory.forget", {"id": item.id, "confirm": True})
    finally:
        c.close()
    assert _memory_questions(daemon) == [] and _pending_rows(daemon) == 0
    assert _everywhere(daemon, "qindlewort") == [] and _everywhere(daemon, "Zorblat") == []


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


def test_a_card_forgotten_for_restating_the_item_closes_the_question_about_it(daemon: RunningDaemon) -> None:
    """Re-review test gap: a task given the item wrote it in its answer, and the owner had asked to forget that task's
    card. The item's forget deletes the card after the echo scrub (`_forget_stating_cards`), and its question closes."""
    from tests.conftest import done, outcome, sup, task
    d = daemon.daemon
    item = _added(daemon, "My dentist is Dr. Zorblat", "dentist")
    install(daemon, [sup("task", "Emailing.", task("email my dentist to book Monday"))],
            {"worker": done("Emailed Dr. Zorblat's office to book Monday.")})
    task_id = outcome(chat(daemon, "email my dentist to book Monday"))["task_id"]
    daemon.run(d.supervisor.finish_episodes())
    card = _on_loop(daemon, lambda: d.learning.current_card(task_id))
    _on_loop(daemon, lambda: d._chat_forget(card["id"], "r-c"))
    assert len(_memory_questions(daemon)) == 1
    from ferb.cli.client import Client
    c = Client(daemon.sock)
    try:
        c.call("memory.forget", {"id": item.id, "confirm": True})
    finally:
        c.close()
    assert _on_loop(daemon, lambda: d.learning.current_card(task_id)) is None
    assert _memory_questions(daemon) == [] and _pending_rows(daemon) == 0


def test_a_learned_items_echo_never_scrubs_a_label_key(daemon: RunningDaemon) -> None:
    """R3's echo pass removes FERB's naming of the item and what it states, never its key alone: an owner identity
    statement is keyed "owner", a word every planner turn holds."""
    from ferb.cli.client import Client
    from ferb.memory.echoes import REMOVED
    d = daemon.daemon
    node = _on_loop(daemon, lambda: d.learning.put("identity", "owner", {"text": "I sign emails as Kestrovan"}, [],
                                                   origin_label="owner", made_by="owner:identity/1", owner=True,
                                                   state="active"))
    install(daemon, [_reply("Hi.")])
    chat(daemon, "what's on today?")

    def owners() -> int:                         # "owner" in spans, beside the removal marker's own "owner"
        held = _on_loop(daemon, lambda: "".join(json.dumps(json.loads(r[0])) for r in d.db.conn.execute(
            "SELECT input_json FROM spans")))
        return held.count("owner") - held.count(REMOVED)
    before = owners()
    assert before and _everywhere(daemon, "Kestrovan")
    c = Client(daemon.sock)
    try:
        c.call("memory.learning", {"op": "forget", "id": node["id"], "confirm": True})
    finally:
        c.close()
    assert _everywhere(daemon, "Kestrovan") == []
    assert owners() == before


def test_a_forgotten_lesson_leaves_no_copy_in_a_planner_turn_that_listed_it(daemon: RunningDaemon) -> None:
    """R3: a planner turn listed the lesson among its candidates (by its key: scope and trigger). After the dashboard
    forgets it, no span holds its trigger or text; only the owner's own message in the conversation record does."""
    from ferb.cli.client import Client
    d = daemon.daemon
    node = _on_loop(daemon, lambda: d.learning.put(
        "lesson", '{"app": "Mail"}:send greyed out in zentrix', {"text": "wait for the draft to save in vorpalt",
                                                                 "trigger": "send greyed out in zentrix"},
        [], made_by="model:t", state="active"))
    script = install(daemon, [_reply("Hi.")])
    chat(daemon, "why is send greyed out in zentrix?")
    assert "greyed out in zentrix" in "\n".join(str(c) for _, key, c, _ in script.contexts if key == "supervisor")
    c = Client(daemon.sock)
    try:
        c.call("memory.learning", {"op": "forget", "id": node["id"], "confirm": True})
    finally:
        c.close()
    assert _everywhere(daemon, "vorpalt") == []
    assert _everywhere(daemon, "zentrix") == [("journal", "data_json")]
    assert _on_loop(daemon, lambda: [r[0] for r in d.db.conn.execute(
        "SELECT kind FROM journal WHERE data_json LIKE '%zentrix%'")]) == ["chat.message"]


# ---------- round 5 (re-review: NEW-1, NEW-2, test gaps) ----------


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


def test_a_procedure_forget_scrubs_only_the_spans_that_showed_it(daemon: RunningDaemon) -> None:
    """Round 5 ruling: what a learned item states is scrubbed only where it was shown or cited (the tasks given it and
    the planner turns that listed it), never by a substring replace across every span. A later turn whose planner was
    not shown the procedure keeps the owner's words that happen to repeat its intent."""
    d = daemon.daemon
    proc = _on_loop(daemon, lambda: d.learning.put("procedure", "book qindle visit", {
        "intent": "book qindle visit", "steps": []}, [], made_by="model:t", state="candidate"))
    script = install(daemon, [_reply("Okay.")])
    chat(daemon, "how do I book qindle visit?")
    assert proc["id"] in "\n".join(str(c) for _, key, c, _ in script.contexts if key == "supervisor")
    d.supervisor.memory_candidates = lambda words, focus: []                   # the next planner is shown no learned item
    script = install(daemon, [_reply("Okay.")])
    chat(daemon, "please book qindle visit for friday")
    assert proc["id"] not in "\n".join(str(c) for _, key, c, _ in script.contexts if key == "supervisor")
    requests = _on_loop(daemon, lambda: {e.data["text"]: e.data["request_id"]
                                         for e in d.journal.of_kind("chat.message")})
    turn = "kind = 'model.turn' AND json_extract(input_json, '$.request_id') = ?"
    shown, unshown = requests["how do I book qindle visit?"], requests["please book qindle visit for friday"]
    assert _held_in_spans(daemon, "qindle", turn, shown) and _held_in_spans(daemon, "qindle", turn, unshown)
    from ferb.cli.client import Client
    c = Client(daemon.sock)
    try:
        c.call("memory.learning", {"op": "forget", "id": proc["id"], "confirm": True})
    finally:
        c.close()
    assert _held_in_spans(daemon, "qindle", turn, shown) == 0
    assert _held_in_spans(daemon, "please book qindle visit for friday", turn, unshown)


def test_a_cards_naming_in_a_waiting_question_goes_when_the_item_forget_deletes_the_card(daemon: RunningDaemon) -> None:
    """Round 5 test gap: FERB's naming of a learned item in its question (`_named_node`) is scrubbed with it. Here the
    owner had asked to forget a task's card and a planner turn showed that question; the item's forget deletes the
    card after its echo scrub (a scrub queued from inside a running one), and the forget RPC answers only once the
    card's naming is gone from that turn."""
    from tests.conftest import done, outcome, sup, task
    d = daemon.daemon
    item = _added(daemon, "My dentist is Dr. Zorblat", "dentist")
    install(daemon, [sup("task", "Emailing.", task("email my dentist to book Monday"))],
            {"worker": done("Emailed Dr. Zorblat's office to book Monday.")})
    task_id = outcome(chat(daemon, "email my dentist to book Monday"))["task_id"]
    daemon.run(d.supervisor.finish_episodes())
    card = _on_loop(daemon, lambda: d.learning.current_card(task_id))
    _on_loop(daemon, lambda: d._chat_forget(card["id"], "r-c"))
    install(daemon, [_reply("Hi.")])
    chat(daemon, "what's on today?")
    naming = json.dumps(f"learned item “{task_id}” (episode)")[1:-1]

    def held() -> int:
        return _on_loop(daemon, lambda: sum(1 for (s,) in d.db.conn.execute("SELECT input_json FROM spans")
                                            if naming in s or json.dumps(naming)[1:-1] in s))
    assert held() == 1
    from ferb.cli.client import Client
    c = Client(daemon.sock)
    try:
        c.call("memory.forget", {"id": item.id, "confirm": True})
    finally:
        c.close()
    assert held() == 0 and not d._forgetting


def test_echo_scrubs_run_one_at_a_time(daemon: RunningDaemon) -> None:
    """Round 5 test gap: echo scrubs are queued one behind another (`_after_forgets`), so two never rewrite the same
    row from the same old copy."""
    import time
    from ferb.memory import echoes
    d = daemon.daemon
    a, b = _lesson(daemon, "glorpwax one"), _lesson(daemon, "flinvek two")
    events: list[tuple[str, str]] = []
    real = echoes.finish_node_at

    def slow(path: Any, echo: Any, running: Any = None) -> Any:
        [said] = echo.versions.values()
        events.append(("start", said[0]))
        time.sleep(0.3)
        out = real(path, echo, running)
        events.append(("end", said[0]))
        return out
    echoes.finish_node_at = slow                                          # type: ignore[assignment]
    try:
        _on_loop(daemon, lambda: (d._forget_nodes([a["id"]]), d._forget_nodes([b["id"]])))
        daemon.run(d._settle_forgets())
    finally:
        echoes.finish_node_at = real                                      # type: ignore[assignment]
    assert events == [("start", "glorpwax one"), ("end", "glorpwax one"), ("start", "flinvek two"),
                      ("end", "flinvek two")]


def test_the_consolidators_clean_up_forgets_through_the_daemons_path(daemon: RunningDaemon) -> None:
    """Round 5 (docs said "by any path"): the consolidator's clean-up of what its rollups wrote on a source forgotten
    meanwhile called Learning.forget directly, so a waiting question about a node it deleted stayed open. It forgets
    through `_forget_nodes`."""
    d = daemon.daemon
    node = _lesson(daemon, "brindlewax rule")
    _on_loop(daemon, lambda: d._chat_forget(node["id"], "r-n"))
    _on_loop(daemon, lambda: d.consolidator.forget([node["id"]]))
    daemon.run(d._settle_forgets())
    assert _memory_questions(daemon) == [] and _pending_rows(daemon) == 0
    assert _everywhere(daemon, "brindlewax") == []


def test_a_forgotten_lessons_multi_line_text_leaves_the_turn_that_listed_it(daemon: RunningDaemon) -> None:
    """Round 5 (probe r3_unicode): a lesson whose text spans two lines, with accents, curly quotes and a double space,
    listed in a planner turn's memory layer, stayed there after the dashboard forgot it. The owner's own message in
    that turn, which shares two words with it, stays."""
    from ferb.cli.client import Client
    d = daemon.daemon
    text = "Prefer café  qwyrtz\nover “brimble”"
    node = _on_loop(daemon, lambda: d.learning.put("lesson", text, {"text": text}, [], made_by="model:t",
                                                   state="active"))
    install(daemon, [_reply("Hi.")])
    chat(daemon, "what about qwyrtz and brimble?")
    assert _everywhere(daemon, "Prefer caf")
    c = Client(daemon.sock)
    try:
        c.call("memory.learning", {"op": "forget", "id": node["id"], "confirm": True})
    finally:
        c.close()
    assert _everywhere(daemon, "Prefer caf") == [] and _everywhere(daemon, "brimble\\u201d") == []
    assert _held_in_spans(daemon, "what about qwyrtz and brimble?")


# ---------- closing round (final check of round 5) ----------

def _dashboard_forget(d: RunningDaemon, ident: str) -> None:
    from ferb.cli.client import Client
    c = Client(d.sock)
    try:
        c.call("memory.learning", {"op": "forget", "id": ident, "confirm": True})
    finally:
        c.close()


def _attention_run(d: RunningDaemon, title: str) -> None:
    install(d, [json.dumps({"tell": False, "text": "", "source_refs": [], "quotes": []})])
    d.run(d.daemon.supervisor.attention(["trail:x"], [{"data": {"title": title}}], []))


def test_a_forgotten_identity_statement_leaves_every_model_run_since(daemon: RunningDaemon) -> None:
    """Closing round (1): an owner identity statement is in the system prompt of every model run, not only planner
    turns: the worker, dispatch, question, resources and attention runs record it too. Its forget scoped only
    `model.turn` spans, so the worker's and the attention check's records kept it."""
    from tests.conftest import done, outcome, sup, task
    node = _identity(daemon, "I sign emails as Kestrovan")
    install(daemon, [sup("task", "Checking.", task("check the weather"))], {"worker": done("Sunny.")})
    outcome(chat(daemon, "check the weather"))
    _attention_run(daemon, "weather")
    kinds = _on_loop(daemon, lambda: {r[0] for r in daemon.daemon.db.conn.execute(
        "SELECT kind FROM spans WHERE input_json LIKE '%Kestrovan%'")})
    assert {"worker", "attention.consider"} <= kinds
    _dashboard_forget(daemon, node["id"])
    assert _everywhere(daemon, "Kestrovan") == []


def test_the_identity_scope_compares_times_across_utc_offsets(daemon: RunningDaemon) -> None:
    """Closing round (5): the identity scope compared ISO strings, so a statement recorded at 13:00+05:30 (07:30 UTC)
    missed a model run at 09:00+00:00, which sorts first as text."""
    from datetime import datetime, timedelta, timezone
    from tests.conftest import done, outcome, sup, task
    learning = daemon.daemon.learning
    clock = learning.clock

    class India:
        def now(self) -> datetime:
            return datetime(2026, 1, 1, 13, 0, tzinfo=timezone(timedelta(hours=5, minutes=30)))
    learning.clock = India()
    try:
        node = _identity(daemon, "I sign emails as Kestrovan")
    finally:
        learning.clock = clock
    assert node["recorded_at"] == "2026-01-01T13:00:00+05:30"
    install(daemon, [sup("task", "Checking.", task("check the weather"))], {"worker": done("Sunny.")})
    outcome(chat(daemon, "check the weather"))           # its model runs start at 09:00+00:00, after 07:30 UTC
    assert ("spans", "input_json") in _everywhere(daemon, "Kestrovan")
    _dashboard_forget(daemon, node["id"])
    assert _everywhere(daemon, "Kestrovan") == []


def test_a_lesson_listed_to_the_attention_check_leaves_its_record(daemon: RunningDaemon) -> None:
    """Closing round (2): the background attention check has no request, so no exposure records what its memory layer
    listed; a forgotten lesson stayed in its `attention.consider` span. A span that names the node by its id showed
    it."""
    node = _lesson(daemon, "use brave glimmorax")
    _attention_run(daemon, "use brave glimmorax")
    assert _on_loop(daemon, lambda: [r[0] for r in daemon.daemon.db.conn.execute(
        "SELECT kind FROM spans WHERE input_json LIKE '%glimmorax%'")]) == ["attention.consider"]
    _dashboard_forget(daemon, node["id"])
    assert _everywhere(daemon, "glimmorax") == []


def test_a_reply_that_copied_a_lesson_in_a_turn_not_shown_it_loses_the_line(daemon: RunningDaemon) -> None:
    """Closing round (3, ruling): FERB restated a lesson in a turn that listed it, then repeated it word for word in a
    later turn that wasn't shown it; that reply kept the text and chat history fed it to the planner. FERB's chat
    replies, and their turns, lose a line holding a forgotten (not ID-keyed) item's text word for word, as in round
    4, besides the scope."""
    d = daemon.daemon
    node = _lesson(daemon, "always cc the trelloquin team")
    install(daemon, [_reply("Noted: always cc the trelloquin team")])
    chat(daemon, "who do I cc on trelloquin updates?")
    d.supervisor.memory_candidates = lambda words, focus: []
    install(daemon, [_reply("As before: always cc the trelloquin team")])
    chat(daemon, "remind me again about trelloquin")
    _dashboard_forget(daemon, node["id"])
    assert _everywhere(daemon, "always cc the trelloquin team") == []
    script = install(daemon, [_reply("Hi.")])
    chat(daemon, "anything else?")
    assert "always cc the trelloquin team" not in "\n".join(str(c) for _, key, c, _ in script.contexts
                                                            if key == "supervisor")


def test_each_version_of_a_forgotten_lesson_leaves_where_it_was_shown(daemon: RunningDaemon) -> None:
    """Closing round (4): a forget deletes every version of a lesson, but only the named version's text was scrubbed;
    a task whose worker read an older version kept that version's text in its tool call."""
    from tests.conftest import done, outcome, sup, task, tool
    d = daemon.daemon
    v1 = _on_loop(daemon, lambda: d.learning.put("lesson", "send button key", {"text": "wait for zorbleck draft"}, [],
                                                 made_by="model:t", state="active"))

    def act(token: str) -> str:
        tool(daemon.sock, token, "memory.learned", item_id=v1["id"])
        return done("Read the lesson.")
    install(daemon, [sup("task", "Reading.", task("read the send button lesson"))], {"worker": act})
    outcome(chat(daemon, "read the send button lesson"))
    v2 = _on_loop(daemon, lambda: d.learning.put("lesson", "send button key", {"text": "wait for quiggat draft"}, [],
                                                 made_by="model:t", state="active"))
    assert v1["id"] != v2["id"] and ("spans", "output_json") in _everywhere(daemon, "wait for zorbleck")
    _dashboard_forget(daemon, v2["id"])
    assert _everywhere(daemon, "wait for zorbleck") == []


# ---------- final adjudicated fix ----------

def _rows_of(d: RunningDaemon, *kinds: str) -> list[tuple[str, Any]]:
    return _on_loop(d, lambda: [(r[0], json.loads(r[1]).get("text")) for r in d.daemon.db.conn.execute(
        f"SELECT kind, data_json FROM journal WHERE kind IN ({','.join('?' * len(kinds))}) ORDER BY seq", kinds)])


def _span_texts(d: RunningDaemon) -> str:
    return _on_loop(d, lambda: "\n".join((r[0] or "") + (r[1] or "") for r in d.daemon.db.conn.execute(
        "SELECT input_json, output_json FROM spans")))


def test_a_one_word_trigger_never_removes_unrelated_lines(daemon: RunningDaemon) -> None:
    """Final fix (I-1, probe q1): the closing round's reply rule had no minimum. A lesson whose trigger is "Gmail"
    removed "I opened Gmail and found 3 new emails." and a choice line "Gmail" from replies of turns never shown it,
    and the span scrub then replaced "Gmail" as a plain substring everywhere ("check my [removed: …] inbox please").
    Only an item's statement (two words or more, whole) leaves replies wherever written; a trigger stays scoped, and
    the span scrub matches whole words only, never a single word."""
    d = daemon.daemon
    install(daemon, [_reply("I opened Gmail and found 3 new emails.\nAll done.")])
    chat(daemon, "check my Gmail inbox please")
    install(daemon, [_reply("Which one?\nGmail\nOutlook")])
    chat(daemon, "send it from my mail app")
    node = _on_loop(daemon, lambda: d.learning.put(
        "lesson", '{"app":"Gmail"}:Gmail', {"scope": {"app": "Gmail", "site": "", "intent": ""}, "trigger": "Gmail",
                                             "text": "wait for the compose window before typing",
                                             "failure": "e1", "success": "e2", "helpful": 0, "harmful": 0},
        [], made_by="model:t", state="active"))
    rows, spans = _rows_of(daemon, "chat.reply", "chat.clarify", "chat.message"), _span_texts(daemon)
    _dashboard_forget(daemon, node["id"])
    assert _rows_of(daemon, "chat.reply", "chat.clarify", "chat.message") == rows
    assert _span_texts(daemon) == spans and "check my Gmail inbox please" in spans


def test_a_procedure_intent_never_removes_a_reply_it_was_not_shown_to(daemon: RunningDaemon) -> None:
    """Final fix (I-1, probe q2): forgetting a procedure "send report" removed "Sure, I'll send report drafts…" from a
    reply its turn was never shown it in. A procedure's intent stays scoped."""
    d = daemon.daemon
    install(daemon, [_reply("Sure, I'll send report drafts to you first from now on.\nAnything else?")])
    chat(daemon, "show me drafts before sending")
    node = _on_loop(daemon, lambda: d.learning.put("procedure", "send report", {
        "intent": "send report", "slots": {}, "steps": []}, [], made_by="model:t", state="candidate"))
    _dashboard_forget(daemon, node["id"])
    assert ("chat.reply", "Sure, I'll send report drafts to you first from now on.\nAnything else?") in \
        _rows_of(daemon, "chat.reply")


def test_a_forgotten_identity_statement_leaves_every_task_record_since(daemon: RunningDaemon) -> None:
    """Final fix (I-2, probe q3): a forgotten identity statement stayed in the records of tasks run while it was in
    the system prompt (the answer row, the task's outcome, the "while you were away" result, the episode, the task's
    spans) and flowed back to the planner through chat history. In spans only the statement goes, never a whole
    single-line output."""
    from tests.conftest import done, outcome, sup, task
    d = daemon.daemon
    node = _identity(daemon, "I sign emails as Kestrovan")
    install(daemon, [_reply("Okay: I sign emails as Kestrovan.\nAlso, your 3pm meeting moved to 4pm.")])
    chat(daemon, "how do I sign?")
    install(daemon, [sup("task", "Checking.", task("check the weather"))],
            {"worker": done("Sunny. I sign emails as Kestrovan\nRain later.")})
    tid = outcome(chat(daemon, "check the weather"))["task_id"]
    daemon.run(d.supervisor.finish_episodes())
    _dashboard_forget(daemon, node["id"])
    assert _everywhere(daemon, "Kestrovan") == []
    held = _on_loop(daemon, lambda: [r[0] for r in d.db.conn.execute(
        "SELECT output_json FROM spans WHERE kind IN ('model.turn', 'worker') AND output_json IS NOT NULL")])
    assert any("your 3pm meeting moved to 4pm" in s for s in held) and any("Rain later." in s for s in held)
    assert "Rain later." in _on_loop(daemon, lambda: d.db.conn.execute(
        "SELECT answer FROM task_results WHERE task_id = ?", (tid,)).fetchone()[0])
    script = install(daemon, [_reply("Hi.")])
    chat(daemon, "anything else?")
    assert "Kestrovan" not in "\n".join(str(c) for _, key, c, _ in script.contexts if key == "supervisor")


def test_a_notice_restating_a_forgotten_lesson_loses_it(daemon: RunningDaemon) -> None:
    """Final fix (4): a notice from the attention check that restated a lesson kept it, in its chat.notice row and
    in attention_notices. Its statement goes wherever a notice holds it; a trigger goes from the notices of a check
    that was shown the lesson."""
    d = daemon.daemon
    node = _on_loop(daemon, lambda: d.learning.put("lesson", '{"app":"Mail"}:glimmorax menu', {
        "text": "always cc the trelloquin team", "trigger": "glimmorax menu"}, [], made_by="model:t", state="active"))
    _attention_run(daemon, "glimmorax menu")            # the check is shown the lesson (its memory layer names it)
    for ident, text, rows in (("batch:a", "Reminder: always cc the trelloquin team.", ["trail:y"]),  # another check
                              ("batch:b", "The glimmorax menu opened in Mail.", ["trail:x"])):      # the shown one
        notice = _on_loop(daemon, lambda: d.attention_notices.add(ident, rows, text, [], rows))
        daemon.run(d._post_notice(notice))
    _dashboard_forget(daemon, node["id"])
    for word in ("trelloquin", "glimmorax"):
        assert not [hit for hit in _everywhere(daemon, word) if hit[0] in ("attention_notices", "spans")], word
    assert not [t for _, t in _rows_of(daemon, "chat.notice") if "trelloquin" in t or "glimmorax" in t]
    assert _on_loop(daemon, lambda: [r[0] for r in d.db.conn.execute(
        "SELECT text FROM attention_notices WHERE id = 'batch:b'")]) == [
        "The [removed: it held something the owner asked FERB to forget] opened in Mail."]   # whole words, not the line


def test_a_single_word_line_removed_in_scope_is_never_scrubbed_elsewhere(daemon: RunningDaemon) -> None:
    """Final fix (I-1): a reply of a turn shown the lesson lost its choice line "Gmail" (the trigger, in scope); that
    one-word line was then told to every span, so another turn's owner message lost "Gmail". A removed line of one
    word is never told."""
    d = daemon.daemon
    node = _on_loop(daemon, lambda: d.learning.put(
        "lesson", "Gmail", {"text": "wait for the compose window before typing", "trigger": "Gmail"}, [],
        made_by="model:t", state="active"))
    install(daemon, [_reply("Which one?\nGmail\nOutlook")])
    chat(daemon, "which Gmail should I use?")
    d.supervisor.memory_candidates = lambda words, focus: []
    install(daemon, [_reply("Done.")])
    chat(daemon, "check my Gmail inbox please")
    held = _held_in_spans(daemon, "check my Gmail inbox please")
    _dashboard_forget(daemon, node["id"])
    assert "Gmail" not in str(_rows_of(daemon, "chat.reply")[0][1])                       # in scope: the line went
    assert held and _held_in_spans(daemon, "check my Gmail inbox please") == held


def test_the_span_scrub_matches_whole_words_case_and_spacing_aside() -> None:
    """Final fix (I-1): the span scrub never replaces a plain substring: a phrase inside a longer word stays; a phrase
    in other case or spacing, or inside a JSON document a recorded string holds, goes."""
    from ferb.memory.echoes import REMOVED, _Words
    words = _Words(["my Gmail inbox"])
    assert words.text("in mymy Gmail inboxes and myGmail inbox") == "in mymy Gmail inboxes and myGmail inbox"
    assert words.text("check MY  gmail\ninbox now") == f"check {REMOVED} now"
    assert words.text(json.dumps({"text": "“my Gmail inbox”"})) == json.dumps({"text": f"“{REMOVED}”"})


# ---------- fix wave 6a round 1: what a task or chat turn running at the forget records afterwards ----------


def test_a_task_running_at_the_forget_loses_the_item_from_what_it_records_afterwards(daemon: RunningDaemon) -> None:
    """Fix wave 6a round 1 (concern 1, ruled): a task given the item was still running when the owner forgot it, and
    what it recorded after the forget's scrub (its spans, answer and "while you were away" result) kept the item. When
    it ends, the same scrub runs again with the same rules; an unrelated task's records stay as they are, and the
    forget's rules are kept in memory only until every task running at the forget has ended."""
    d = daemon.daemon
    item = daemon.run(d._memory_add(None, {"content": QUILL, "key": "dentist"}))      # type: ignore[arg-type]

    def other() -> None:                                  # never given the item: a word of it is not a mention
        _result(daemon, "t-other", "Quillfeather confirmed the slot")
        span = d.spans.start("tool.call", task_id="t-other", input={"tool": "calendar.read"})
        d.spans.finish(span, output={"text": "Quillfeather confirmed the slot"})
        d.db.conn.execute("INSERT INTO task_memory_refs VALUES ('t-run', ?)", ("memory:" + item["id"],))
    _on_loop(daemon, other)

    def writes() -> None:
        d.journal.append("task.answer", {"task_id": "t-run", "request_id": "r-run", "status": "done",
                                         "text": "Booked Dr. Quillfeather for Monday\nAll set"})
        span = d.spans.start("tool.call", task_id="t-run", input={"tool": "calendar.add"})
        d.spans.finish(span, output={"text": "Booked Quillfeather\nok"})
        _result(daemon, "t-run", "Booked Dr. Quillfeather for Monday")
    gate, job = _running_across_the_forget(daemon, writes, "r-run", "t-run")
    _on_loop(daemon, lambda: d._forget_memory(item["id"]))
    daemon.run(d._settle_forgets())
    assert _on_loop(daemon, lambda: len(d._rescrubs)) == 1
    # the same rules: the owner using the word meanwhile doesn't make it common to the running task's lines
    _on_loop(daemon, lambda: d.journal.append("chat.message", {"session_id": "s1", "text": "Is Quillfeather open?"}))
    _let_go(daemon, gate, job)
    assert _answers(daemon, "task.answer", "task_id", "t-run") == [f"{REMOVED}\nAll set"]
    assert _task_records(daemon, "t-run") == ([({"tool": "calendar.add"}, {"text": f"{REMOVED}\nok"})],
                                              [REMOVED])
    kept = "Quillfeather confirmed the slot"
    assert _task_records(daemon, "t-other") == ([({"tool": "calendar.read"}, {"text": kept})], [kept])
    assert _on_loop(daemon, lambda: len(d._rescrubs)) == 0               # its rules went with the last such task


def test_a_chat_turn_running_at_the_forget_loses_its_answer_to_the_forgotten_message(daemon: RunningDaemon) -> None:
    """A chat turn answering the message the owner then forgot writes its reply and planner turn after the scrub:
    when it ends, the scrub runs again and FERB's answer to the forgotten message goes whole."""
    d = daemon.daemon
    item = _remember(daemon, f"remember that {QUILL}", QUILL, "dentist")
    request = _on_loop(daemon, lambda: next(json.loads(r[0])["request_id"] for r in d.db.conn.execute(
        "SELECT data_json FROM journal WHERE kind = 'chat.message' AND data_json LIKE '%Quillfeather%'")))

    def writes() -> None:
        d.journal.append("chat.reply", {"session_id": "s1", "request_id": request, "text": "Also: noted for later.",
                                        "display_text": "Also: noted for later."})
        turn = d.spans.start("model.turn", input={"role": "supervisor", "request_id": request})
        d.spans.finish(turn, output={"text": "Also: noted for later."})
    gate, job = _running_across_the_forget(daemon, writes, request)
    _on_loop(daemon, lambda: d._forget_memory(item.id))
    daemon.run(d._settle_forgets())
    _let_go(daemon, gate, job)
    assert "Also: noted for later." not in _answers(daemon, "chat.reply", "request_id", request)
    assert _held_in_spans(daemon, "noted for later") == 0


def test_a_task_running_at_a_learned_items_forget_loses_what_it_states(daemon: RunningDaemon) -> None:
    """The same for a learned item's forget: a task shown the lesson, still running, records it after the echo pass;
    when it ends the pass runs again (the same NodeEcho)."""
    d = daemon.daemon
    node = _lesson(daemon, "always cc Zorblat on invoices")
    _on_loop(daemon, lambda: d.db.conn.execute("INSERT INTO task_memory_refs VALUES ('t-node', ?)", (node["id"],)))

    def writes() -> None:
        d.journal.append("task.answer", {"task_id": "t-node", "request_id": "r-node", "status": "done",
                                         "text": "Sent; always cc Zorblat on invoices\nDone"})
        span = d.spans.start("tool.call", task_id="t-node", input={"tool": "mail.send"})
        d.spans.finish(span, output={"text": "always cc Zorblat on invoices"})
    gate, job = _running_across_the_forget(daemon, writes, "r-node", "t-node")
    _on_loop(daemon, lambda: d._forget_nodes([node["id"]]))
    daemon.run(d._settle_forgets())
    _let_go(daemon, gate, job)
    assert _answers(daemon, "task.answer", "task_id", "t-node") == [f"{REMOVED}\nDone"]
    assert _task_records(daemon, "t-node")[0] == [({"tool": "mail.send"}, {"text": REMOVED})]


def test_shutdown_runs_the_scrub_again_for_a_task_it_stops(home: Any) -> None:
    """Shutdown settles forgets, then stops the tasks: one running at a forget records its outcome as it stops, and the
    scrub that runs again for it is waited for before the database closes."""
    import asyncio
    import sqlite3
    from ferb.clock.clock import FakeClock
    from ferb.core.ids import SequentialIds
    running = RunningDaemon(home, clock=FakeClock(), ids=SequentialIds("id"), watch_owner_input=False)
    d = running.daemon
    item = running.run(d._memory_add(None, {"content": QUILL, "key": "dentist"}))     # type: ignore[arg-type]

    async def start() -> None:
        now = d.clock.now().isoformat()
        d.db.conn.execute(
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
            "outcome_json, created_at, updated_at) VALUES ('t-stop', 't-stop', NULL, 'r-stop', 'book it', 'worker', "
            "'worker', '[]', 'running', '{}', ?, ?)", (now, now))
        d.db.conn.execute("INSERT INTO task_memory_refs VALUES ('t-stop', ?)", ("memory:" + item["id"],))

        async def job() -> None:
            try:
                await asyncio.Event().wait()
            finally:                                      # an interrupted task still records its outcome
                d.journal.append("task.answer", {"task_id": "t-stop", "request_id": "r-stop", "status": "interrupted",
                                                 "text": "Was booking Dr. Quillfeather\nStopped"})
        task = asyncio.get_running_loop().create_task(job())
        d._turns.add(task)
        d._turn_requests[task] = "r-stop"
        task.add_done_callback(d._turns.discard)
        d._forget_memory(item["id"])
        await d._settle_forgets()
    running.run(start())
    running.stop()
    db = sqlite3.connect(home / "ferb.db")
    try:
        texts = [json.loads(r[0])["text"] for r in db.execute(
            "SELECT data_json FROM journal WHERE kind = 'task.answer' AND "
            "json_extract(data_json, '$.task_id') = 't-stop'")]
    finally:
        db.close()
    assert texts == [f"{REMOVED}\nStopped"]
