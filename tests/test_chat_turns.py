"""Step 8c end-to-end runs (TEST-500): a chat turn through the real daemon, gateway and scripted model. The
restart rule and its shared time limit (ORCH-311, ORCH-318), cross-chat control, a stop that finds the work
finished, folders in the plan line (HAND-204), "keep the tabs open" (HAND-201), the planner's context
ordered most-stable first with task state once and bounded history (ORCH-303), questions recorded with their kind,
and a remembered fact sourced to the message that holds it."""
import json
import threading
import time
from pathlib import Path
from typing import Any

from ferb.cli.client import Client
from ferb.core.context import count_tokens
from ferb.orchestrator.supervisor import (FOLD_CHUNK_TOKENS, RECENT_CONVERSATION_TOKENS, RECENT_EXCHANGES, SUMMARY_DAYS,
                                          SUMMARY_TOKENS)
from tests.conftest import STALL, RunningDaemon, chat, done, install, outcome, report, sup, task


class Stream:
    """One chat.send stream read in the background (the owner sends while an earlier message is still planning)."""

    def __init__(self, d: RunningDaemon, text: str, session: str = "s1") -> None:
        self.events: list[dict[str, Any]] = []
        self.ended = threading.Event()
        threading.Thread(target=self._read, args=(d, text, session), daemon=True).start()

    def _read(self, d: RunningDaemon, text: str, session: str) -> None:
        c = Client(d.sock, timeout=60)
        try:
            for event in c.stream("chat.send", {"session_id": session, "text": text}):
                self.events.append(event)
        finally:
            c.close()
            self.ended.set()

    def acked(self, timeout: float = 10) -> dict[str, Any]:
        _until(lambda: bool(self.events), timeout)
        return self.events[0]

    def done(self, timeout: float = 30) -> list[dict[str, Any]]:
        assert self.ended.wait(timeout), "the stream didn't end"
        return self.events[1:]


def _until(check: Any, timeout: float = 20) -> None:
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if check():
            return
        time.sleep(0.02)
    raise AssertionError("the condition never held")


def _planner_calls(script: Any) -> list[Any]:
    return [r for _, key, r in script.requests if key == "supervisor"]


def _journal(d: RunningDaemon, kind: str) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        return [e.data for e in d.daemon.journal.of_kind(kind)]
    return d.run(go())


def _control(task_id: str, action: str, text: str = "", t: dict[str, Any] | None = None) -> str:
    decision = json.loads(sup("task" if t else "reply", text, t))
    decision["control"] = {"task_id": task_id, "action": action, "new_goal": False, "standing_sends": "unchanged",
                           "supersedes": []}
    return json.dumps(decision)


def _running(d: RunningDaemon) -> str:
    _until(lambda: bool(d.run(_trees(d))))
    return d.run(_trees(d))[0][0]


async def _trees(d: RunningDaemon) -> list[tuple[str, str]]:
    return d.daemon.supervisor.running_trees()


# decision: ORCH-318
def test_four_quick_messages_are_two_turns(daemon: RunningDaemon) -> None:
    """ORCH-318: the second message comes before FERB's first reply, so the turn restarts once and reads both; the
    third and fourth come while the restarted turn is still thinking, so they wait and are read together next."""
    release = threading.Event()

    def thinking(_token: str) -> str:
        assert release.wait(20)
        return sup("reply", "Both first messages answered.")
    script = install(daemon, [STALL, thinking, sup("reply", "Both later messages answered.")])
    m1 = Stream(daemon, "book a table for friday")
    m1.acked()
    _until(lambda: len(_planner_calls(script)) == 1)
    m2 = Stream(daemon, "for four people")
    lead = m1.events[0]["request_id"]
    _until(lambda: len(_planner_calls(script)) == 2)            # the turn started again with both messages
    m3 = Stream(daemon, "also remind me tomorrow")
    m3.acked()
    time.sleep(0.2)                                             # in this order
    m4 = Stream(daemon, "at nine")
    m4.acked()
    time.sleep(0.2)
    assert len(_planner_calls(script)) == 2                     # no second restart: they wait their turn
    release.set()
    first, second, third, fourth = m1.done(), m2.done(), m3.done(), m4.done()
    assert [e["type"] for e in second] == ["merged"] and second[0]["into"] == lead
    assert [e["type"] for e in fourth] == ["merged"] and fourth[0]["into"] == m3.events[0]["request_id"]
    assert [e["text"] for e in first if e["type"] == "reply"] == ["Both first messages answered."]
    assert [e["text"] for e in third if e["type"] == "reply"] == ["Both later messages answered."]
    assert len(_planner_calls(script)) == 3                     # 4 messages, 2 turns (one restarted once)
    contexts = [r.context for r in _planner_calls(script)]
    assert "book a table for friday" in contexts[1] and "for four people" in contexts[1]
    assert "also remind me tomorrow" in contexts[2] and "at nine" in contexts[2]
    assert len(_journal(daemon, "chat.restarted")) == 1


# decision: ORCH-311, ORCH-318
def test_a_planner_that_runs_out_of_time_replies_plainly_and_the_restart_shares_the_limit(daemon) -> None:
    """ORCH-311 and ORCH-318: one time limit covers the turn, its restart included; when it runs out the owner reads a
    plain reply (no stack, no internal words) instead of waiting."""
    daemon.daemon.supervisor.planner_limit_s = 1.0
    script = install(daemon, [STALL, STALL])
    started = time.monotonic()
    m1 = Stream(daemon, "what's on my screen?")
    m1.acked()
    _until(lambda: len(_planner_calls(script)) == 1)
    time.sleep(0.4)
    m2 = Stream(daemon, "and in the other window?")
    events = m1.done()
    elapsed = time.monotonic() - started
    assert [e["type"] for e in m2.done()] == ["merged"]
    replies = [e for e in events if e["type"] == "reply"]
    assert len(replies) == 1 and replies[0]["status"] == "failed"
    text = replies[0]["text"]
    assert "1 seconds" in text and not any(w in text for w in ("Error", "Traceback", "Timeout", "planner"))
    assert elapsed < 1.9                                        # not 1 s more for the restart
    assert [row["why"] for row in _journal(daemon, "chat.failed")] == ["TimeoutError"]


def test_another_chats_running_task_can_be_stopped(daemon: RunningDaemon) -> None:
    """Live failure 2026-10-24: the planner is shown every chat's running tasks, so a stop from another chat acts."""
    script = install(daemon, [sup("task", "On it.", task("Wait for the build"))], {"worker": STALL})
    first = Stream(daemon, "wait for the build", session="chat-a")
    tree = _running(daemon)
    script.sup.append(_control(tree, "stop"))
    said = [e["text"] for e in chat(daemon, "stop the build task", session="chat-b") if e["type"] == "reply"]
    assert said == ["Stopped, as you asked."]
    assert outcome(first.done())["status"] == "cancelled"
    context = _planner_calls(script)[-1].context
    assert '"chat": "another chat"' in context and context.count(tree) == 1    # task state, once


def test_a_stop_after_the_work_finished_never_records_it_as_cancelled(daemon: RunningDaemon) -> None:
    """Fix list (Intake): a stop that arrives while a finished task is tidying up leaves its finished work done."""
    sv = daemon.daemon.supervisor
    tidying, go_on = threading.Event(), threading.Event()
    original = sv.on_task_end

    async def end(task_id: str) -> Any:
        import asyncio
        tidying.set()
        while not go_on.is_set():
            await asyncio.sleep(0.02)
        return await original(task_id)
    sv.on_task_end = end
    script = install(daemon, [sup("task", "On it.", task("Say hello"))], {"worker": done("Said hello.")})
    first = Stream(daemon, "say hello")
    tree = _running(daemon)
    assert tidying.wait(10)
    script.sup.append(_control(tree, "stop"))
    stopper = Stream(daemon, "stop that")
    time.sleep(0.3)
    go_on.set()
    said = [e["text"] for e in stopper.done() if e["type"] == "reply"]
    assert said == ["That task had already finished its work, so there was nothing to stop."]
    assert outcome(first.done())["status"] == "done"


# decision: ORCH-327
def test_the_plan_line_states_the_folders_and_the_shell_works_only_there(daemon: RunningDaemon) -> None:
    """HAND-204: the planner resolves the owner's folder words into `folders`; the plan line says them, so a
    wrong guess can be corrected, and they are the task's shell scope. One that names no specific folder is repaired."""
    sv = daemon.daemon.supervisor
    seen: list[tuple[str, ...]] = []

    def worker(token: str) -> str:
        grant = daemon.daemon.tokens.validate(token)
        seen.append(sv.gateway.task_folders.get(grant.task_id.split(".", 1)[0], ()))
        return done("Tidied.")
    broad = task("Tidy my downloads") | {"folders": ["~"]}
    named = task("Tidy my downloads") | {"folders": ["~/Downloads"]}
    script = install(daemon, [sup("task", "Tidying.", broad), sup("task", "Tidying.", named)], {"worker": worker})
    events = chat(daemon, "tidy my downlods folder")
    plan = next(e for e in events if e["type"] == "plan")
    assert "I'll work in ~/Downloads." in plan["text"]
    assert seen == [(str(Path.home() / "Downloads"),)]
    assert len(_planner_calls(script)) == 2 and "folders must each name one specific folder" in \
        _planner_calls(script)[1].context
    assert outcome(events)["status"] == "done"
    assert daemon.run(_folders_left(daemon)) == {}               # gone with the task


async def _folders_left(d: RunningDaemon) -> dict[str, Any]:
    return dict(d.daemon.supervisor.gateway.task_folders)


def test_keep_the_tabs_open_reaches_the_browser(daemon: RunningDaemon) -> None:
    """HAND-201: the owner's "keep the tabs open", marked by the planner, asks the browser drivers to hand the
    task's tabs over at its end instead of closing them."""
    kept: list[str] = []
    daemon.daemon.supervisor.keep_tabs_open = kept.append
    install(daemon, [sup("task", "Opening them.", task("Open the three docs") | {"keep_tabs_open": True})],
            {"worker": done("Opened.")})
    result = outcome(chat(daemon, "open the three docs and keep the tabs open"))
    assert kept == [result["task_id"]]


# decision: ORCH-325
def test_the_planner_context_is_ordered_most_stable_first(daemon: RunningDaemon) -> None:
    """ORCH-325: instructions hold no live values; the stable facts, the summary and the recent
    conversation come first; live facts (a reminder) come after the conversation, then memory, activity, the time, the
    task state and the questions, and the owner's new message last."""
    sv = daemon.daemon.supervisor
    sv.live_facts = lambda: "- Reminder r1: stretch — Sat 31 Oct 2026 17:00"
    script = install(daemon, [sup("reply", "first"), sup("reply", "second")])
    chat(daemon, "an earlier message")
    chat(daemon, "the new message")
    request = _planner_calls(script)[-1]
    assert "Reminder r1" not in request.system
    body = request.context
    order = ["Layer 1 — stable device", "Layer 5 — conversation:", "an earlier message", "Layer 2 — right now",
             "Reminder r1", "Layer 3 — today's activity", "Layer 2 — situation:", "Layer 6 — open questions",
             "Layer 5 — owner's complete new message:\nthe new message"]
    assert [body.index(part) for part in order] == sorted(body.index(part) for part in order)
    assert request.cacheable >= 3                              # Layer 1, the conversation header, its entries


# decision: ORCH-303, ORCH-304, ORCH-326
def test_a_long_chat_keeps_the_planner_and_every_summary_call_within_their_bounds(daemon: RunningDaemon) -> None:
    """ORCH-303: the planner reads the summary and the tail after it: the last RECENT_EXCHANGES exchanges word for
    word however long they are (owner, 2026-10-30: always at least the last 20; no token cap folds one of them), and at
    most about one chunk beyond them; a summary call holds the summary and at most FOLD_CHUNK_TOKENS of the oldest
    entries, never "what fits the window" (live 2026-10-28: one fold read 533k tokens), and is told the days it
    keeps."""
    import asyncio
    turns = RECENT_EXCHANGES + 10
    script = install(daemon, [sup("reply", f"answer {i} " + "detail " * 1500) for i in range(turns + 1)],
                     {"summary": json.dumps({"summary": "EARLIER-SUMMARY"})})

    async def settled() -> None:
        while daemon.daemon.supervisor._summarizing:
            await asyncio.sleep(0.02)
    sizes = []
    for i in range(turns):
        chat(daemon, f"message number {i} " + "filler " * 1500)
        daemon.run(settled())
        context = _planner_calls(script)[-1].context
        start = context.index("Layer 5 — conversation:")
        sizes.append(count_tokens(context[start:context.index("Layer 2 —", start)]))
    chat(daemon, "what next?")
    context = _planner_calls(script)[-1].context
    assert "message number 0 " not in context and "EARLIER-SUMMARY" in context
    # the last 20 exchanges, each far longer than RECENT_CONVERSATION_TOKENS / 20, all there word for word
    assert all(f"message number {i} " in context for i in range(turns - RECENT_EXCHANGES, turns))
    assert RECENT_EXCHANGES * sizes[1] > RECENT_CONVERSATION_TOKENS
    one_exchange = sizes[1]                                     # the first turn's conversation: one whole exchange
    assert max(sizes) <= (RECENT_EXCHANGES + 1) * one_exchange + FOLD_CHUNK_TOKENS + SUMMARY_TOKENS
    folds = [r for _, key, r in script.requests if key == "summary"]
    assert folds
    for fold in folds:
        assert count_tokens(fold.context) <= FOLD_CHUNK_TOKENS + SUMMARY_TOKENS + 1_000
        assert f"last {SUMMARY_DAYS} days" in fold.context


# decision: ORCH-303
def test_no_entry_is_ever_in_neither_the_window_nor_the_summary(daemon: RunningDaemon, monkeypatch) -> None:
    """Target design 5: the recent conversation is append-only after the summary's coverage, so every earlier entry is
    either folded into the summary or shown; the planner always sees at least the last RECENT_EXCHANGES exchanges."""
    import asyncio
    import re
    from ferb.orchestrator import supervisor as sup_module
    monkeypatch.setattr(sup_module, "RECENT_EXCHANGES", 2)
    monkeypatch.setattr(sup_module, "FOLD_CHUNK_TOKENS", 600)
    script = install(daemon, [sup("reply", f"answer {i} " + "detail " * 60) for i in range(16)],
                     {"summary": json.dumps({"summary": "EARLIER-SUMMARY"})})

    async def settled() -> None:
        while daemon.daemon.supervisor._summarizing:
            await asyncio.sleep(0.02)

    async def recorded() -> tuple[list[str], set[str]]:
        journal = daemon.daemon.journal
        entries = [e.id for e in journal.entries_of("chat.message", "chat.reply")]
        covered = {str(h).removeprefix("entry:") for row in journal.of_kind("chat.summary")
                   for h in row.data.get("covered", [])}
        return entries, covered
    for i in range(15):
        chat(daemon, f"message {i} " + "filler " * 60)
        daemon.run(settled())
        entries, covered = daemon.run(recorded())
        shown = set(re.findall(r"chat\.history entry_id=(\S+)\]", _planner_calls(script)[-1].context))
        earlier = entries[:-2]                                  # before this turn's own message and reply
        assert all(e in covered or e in shown for e in earlier), i
        assert set(earlier[-4:]) <= shown                       # the last 2 exchanges, always word for word
    assert covered                                              # folds did run


# decision: ORCH-325
def test_two_turns_without_a_fold_share_the_cached_prefix(daemon: RunningDaemon) -> None:
    """Target design 5: the recent conversation only grows at its end between folds, so the next turn's cacheable
    prefix starts with the whole of this turn's."""
    script = install(daemon, [sup("reply", f"answer {i}") for i in range(3)])
    for text in ("first message", "second message", "third message"):
        chat(daemon, text)
    earlier, later = _planner_calls(script)[-2:]
    assert later.parts[:earlier.cacheable] == earlier.parts[:earlier.cacheable]
    assert later.cacheable > earlier.cacheable


# decision: ORCH-318
def test_a_side_entry_reply_never_lets_the_next_message_in_before_the_task_registers(daemon, monkeypatch) -> None:
    """Fix list (Intake): a turn's side entries can send a reply-type event (here a conversation change refused) before
    its task registers; the next message still waits until the plan line is out, then has a turn of its own."""
    import asyncio
    sv = daemon.daemon.supervisor
    gate = threading.Event()
    original = sv.run_tree

    async def slow_registration(*args: Any, **kwargs: Any) -> Any:
        while not gate.is_set():
            await asyncio.sleep(0.02)
        async for event in original(*args, **kwargs):
            yield event
    monkeypatch.setattr(sv, "run_tree", slow_registration)
    decision = json.loads(sup("task", "On it.", task("Say hello")))
    decision["conversation_changes"] = [{"id": "nope", "action": "pause", "quote": "words never typed"}]
    script = install(daemon, [json.dumps(decision), sup("reply", "Second answered.")], {"worker": done("Hello.")})
    first = Stream(daemon, "say hello")
    _until(lambda: any(e.get("type") == "reply" for e in first.events))     # the side entry's reply is out
    second = Stream(daemon, "and then?")
    second.acked()
    time.sleep(0.3)
    assert len(_planner_calls(script)) == 1                  # the next message hasn't started: the task isn't registered
    gate.set()
    events = first.done()
    assert [e["type"] for e in events].index("reply") < [e["type"] for e in events].index("plan")
    later = second.done()
    assert [e["text"] for e in later if e["type"] == "reply"] == ["Second answered."]   # its own turn, not merged
    assert len(_planner_calls(script)) == 2


# decision: ORCH-328, ORCH-301
def test_questions_are_recorded_with_their_kind_and_answer(daemon: RunningDaemon) -> None:
    """Fix list: every question FERB asks, with its kind and how it was answered, so "few questions" can be measured."""
    install(daemon, [sup("task", "Booking.", task("Book the table"))],
            {"worker": [json.dumps(report("not_done", reason="Two branches.", question="Which branch, Indiranagar or "
                                          "Koramangala?")), done("Booked at Indiranagar.")]})
    first = Stream(daemon, "book the table")

    async def asked() -> str:
        import asyncio
        while True:
            for ap in daemon.daemon.state.approvals.pending():
                if ap.capability == "plan.ask":
                    return ap.id
            await asyncio.sleep(0.02)
    question = daemon.run(asked(), timeout=20)
    c = Client(daemon.sock)
    try:                                                        # the owner types the answer in the notch
        c.call("chat.answer", {"approval_id": question, "text": "Indiranagar"})
    finally:
        c.close()
    assert outcome(first.done())["status"] == "done"
    asked = [row for row in _journal(daemon, "question.asked") if row.get("approval_id")]
    answered = _journal(daemon, "question.answered")
    assert [row["kind"] for row in asked] == ["task"]
    assert [(row["kind"], row["state"], row["answer"]) for row in answered] == [("task", "granted", "words")]


def test_a_fact_said_before_a_clarifying_question_is_sourced_to_that_message(daemon: RunningDaemon) -> None:
    """Memory fix list: after FERB's clarifying question, the fact the owner stated in the first message is that
    message's, not the reply's."""
    remember = json.loads(sup("reply", "Booked for Friday."))
    remember["remember"] = [{"kind": "preference", "subject": "owner", "key": "diet", "content": "I'm vegetarian",
                             "quote": "I'm vegetarian", "login_origin": None, "until": None}]
    install(daemon, [sup("clarify", "Which day?"), json.dumps(remember)])
    chat(daemon, "I'm vegetarian, book us a table")
    chat(daemon, "friday")

    async def sources() -> tuple[list[str], list[str]]:
        db = daemon.daemon.db.conn
        return ([r[0] for r in db.execute("SELECT source_id FROM memory_sources WHERE kind = 'journal'")],
                [e.id for e in daemon.daemon.journal.of_kind("chat.message")])
    found, messages = daemon.run(sources())
    assert found == [messages[0]]


def test_an_answer_to_a_question_no_longer_waiting_keeps_the_planners_reply(daemon: RunningDaemon) -> None:
    """Fix list (Intake): a misrouted answer used to discard the planner's own reply."""
    decision = json.loads(sup("reply", "Blue it is; the rest of your message is noted."))
    decision["answer_question"] = {"approval_id": "ap-gone", "answer": "blue"}
    install(daemon, [json.dumps(decision)])
    [reply] = [e["text"] for e in chat(daemon, "blue, and thanks") if e["type"] == "reply"]
    assert reply.startswith("No question is waiting") and reply.endswith("the rest of your message is noted.")


def test_a_correction_that_changes_whose_account_a_running_task_uses_says_it_wasnt_applied(daemon) -> None:
    """Fix list (Intake): mid-task identity and plan changes were dropped silently; code says so in the reply."""
    script = install(daemon, [sup("task", "On it.", task("Check the order status"))], {"worker": STALL})
    first = Stream(daemon, "check my order status")
    tree = _running(daemon)
    script.sup.append(_control(tree, "continue", "Switching to your account.",
                               task("Check the order status in my account", identity="owner")))
    [reply] = [e["text"] for e in chat(daemon, "use my account for it") if e["type"] == "reply"]
    assert "Switching to your account." in reply and "keeps working as FERB" in reply
    daemon.run(_stop(daemon, tree))
    first.done()


async def _stop(d: RunningDaemon, tree: str) -> None:
    d.daemon.supervisor.stop_tree(tree)
