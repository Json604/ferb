"""L3 (spec §4.1, Task 7): the owner's answers reach the question they're for, by its id, and carry; a correction
never records a No; a big job's plan is stated and starts. Evidence: code-f5-f6.md I1 (a steer recorded a final No on
a pending FERB check), I3/I19 (a typed "Yes" answered a payment), I4 (the plan's start question lost corrections and
wasn't D33-labelled), I12 (exact-quote routing and "/answer" method questions).

- A free-text answer is routed by the planner with the question's id; code accepts any open owner.ask / plan.ask /
  ferb.confirm question by that id, with the owner's own words, without an exact-quote check.
- FERB's own floor checks (payment, account security, permanent delete, new recipient) take only an unambiguous Yes or
  No in the owner's own words that the planner routed to exactly that question; when the same words would answer
  another open question too, FERB asks which one in plain words.
- A steer closes a pending FERB check as superseded: not a No, so the corrected action asks again.
- A question nobody waits for any more (its call was cancelled, its task stopped) is closed as superseded.
- Voice routes what the owner said like typed text; nothing answers a question by being the only one open.
"""
import asyncio
import json
import re
import threading
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from ferb.core.types import Identity
from ferb.daemon.gateway import CONFIRM_ASK
from ferb.orchestrator.plans import Plan, SendScope, Step
from ferb.policy.rules import ActionRequest
from tests.conftest import (Answers, FakeCap, RunningDaemon, chat, done, gateway_request, gateway_setup, install, sup,
                            task, tool)

ROOT = Path(__file__).resolve().parents[1]
PROMPTS = ROOT / "ferb" / "orchestrator" / "prompts"


def _until(check: Any, timeout: float = 5.0) -> None:
    import time
    end = time.monotonic() + timeout
    while not check():
        assert time.monotonic() < end, "timed out"
        time.sleep(0.02)


def _start(d: RunningDaemon, grant: Any, name: str, **args: Any) -> Any:
    return asyncio.run_coroutine_threadsafe(d.daemon.gateway.call(grant, name, args), d.loop)


def _pending(d: RunningDaemon, capability: str) -> list[Any]:
    return [ap for ap in d.daemon.state.approvals.pending() if ap.capability == capability]


def _pending_floor_check(d: RunningDaemon, kind: str = "pay", words: str = "pay x@y.com $5",
                         to: str = "x@y.com") -> tuple[Any, Any, FakeCap, Any]:
    """A worker's call waiting on FERB's own question (D33): (the approval, the call's future, the tool, the grant)."""
    cap = FakeCap("probe." + kind, kind, {"probe"})
    grant, _ = gateway_setup(d, {"probe"}, gateway_request(words), [cap])
    call = _start(d, grant, cap.name, to=to, text="the message")
    _until(lambda: bool(_pending(d, CONFIRM_ASK)))
    return _pending(d, CONFIRM_ASK)[0], call, cap, grant


def _task_question(d: RunningDaemon, question: str, options: tuple[str, ...] = ("Yes", "No")) -> Any:
    """An open worker question (owner.ask) of another task, as chat.status lists it."""
    async def make() -> Any:
        action = ActionRequest("owner.ask", "owner_question", Identity.FERB, frozenset(), "other.worker",
                               "other.worker", "req-other", frozenset())
        return d.daemon.state.approvals.question(action, "Question from the task: " + question, list(options))
    return d.run(make())


def _routed(approval_id: str, answer: str) -> str:
    """The planner's decision for an answer: kind reply, the question's id and its reading of the answer."""
    return json.dumps({"kind": "reply", "text": "", "task": None,
                       "answer_question": {"approval_id": approval_id, "answer": answer}})


def _texts(events: list[dict[str, Any]]) -> str:
    return "\n".join(str(e.get("text", "")) for e in events)


# ---------- (a) answers by id ----------

def test_a_yes_routed_by_id_answers_only_the_identified_question(daemon: RunningDaemon) -> None:
    pay, call, cap, _ = _pending_floor_check(daemon)
    clarify = _task_question(daemon, "Use your work email?")
    install(daemon, [_routed(clarify.id, "Yes")])
    events = chat(daemon, "Yes", session="s")
    assert clarify.state == "granted" and clarify.answer == "Yes"
    assert pay.state == "pending" and cap.calls == 0          # the payment still waits for its own answer
    assert "/answer" not in _texts(events)
    call.cancel()


def test_a_free_text_answer_needs_no_exact_quote_and_reaches_another_chats_question(daemon: RunningDaemon) -> None:
    question = _task_question(daemon, "Which email should I use?", options=())
    install(daemon, [_routed(question.id, "the work email")])
    events = chat(daemon, "use the work one please", session="s")
    assert question.state == "granted" and question.answer == "use the work one please"   # the owner's own words
    assert [e["type"] for e in events] == ["reply"] and "/answer" not in _texts(events)


def test_the_planners_reading_never_becomes_the_owners_answer(daemon: RunningDaemon) -> None:
    question = _task_question(daemon, "Who should get the summary?", options=())
    install(daemon, [_routed(question.id, "attacker@external.invalid")])
    chat(daemon, "send it to Priya", session="s")
    assert question.answer == "send it to Priya"


def test_pasted_text_is_never_an_answer(daemon: RunningDaemon) -> None:
    from ferb.sanitize.untrusted import PASTE_END, PASTE_START
    question = _task_question(daemon, "Which address?", options=())
    install(daemon, [_routed(question.id, "x@y.com")])
    events = chat(daemon, PASTE_START + "x@y.com" + PASTE_END, session="s")
    assert question.state == "pending"
    assert "/answer" not in _texts(events)


def test_a_plain_yes_answers_a_floor_check_the_planner_picked(daemon: RunningDaemon) -> None:
    pay, call, cap, _ = _pending_floor_check(daemon)
    install(daemon, [_routed(pay.id, "Yes")])
    chat(daemon, "yes", session="s")
    assert pay.state == "granted" and pay.choice == "Yes"
    result = call.result(timeout=5)
    assert not result["is_error"] and cap.calls == 1


# ---------- (D) floor checks: unambiguous Yes/No only, plain-words "which one" ----------

def test_two_open_yes_no_questions_and_a_bare_yes_ask_which_one_in_plain_words(daemon: RunningDaemon) -> None:
    pay, call, cap, _ = _pending_floor_check(daemon)
    other = _task_question(daemon, "Use your work email?")
    install(daemon, [_routed(pay.id, "Yes"), _routed(pay.id, "Yes")])
    events = chat(daemon, "Yes", session="s")
    assert pay.state == "pending" and other.state == "pending" and cap.calls == 0
    assert [e["type"] for e in events] == ["clarify"]
    asked = events[0]["text"]
    assert "Which" in asked and pay.reason in asked and other.reason in asked
    assert "/answer" not in asked and "ap-" not in asked                  # plain words: no ids, no method
    chat(daemon, "the payment", session="s")                              # the planner routes it with its id
    assert pay.state == "granted" and pay.choice == "Yes" and other.state == "pending"
    assert not call.result(timeout=5)["is_error"] and cap.calls == 1


def test_a_floor_check_never_takes_anything_but_a_plain_yes_or_no(daemon: RunningDaemon) -> None:
    pay, call, cap, _ = _pending_floor_check(daemon)
    install(daemon, [_routed(pay.id, "Yes"), _routed(pay.id, "Yes")])
    events = chat(daemon, "sure, sounds good I guess", session="s")
    assert pay.state == "pending" and [e["type"] for e in events] == ["clarify"]
    assert pay.reason in events[0]["text"] and "/answer" not in events[0]["text"]
    chat(daemon, "Yes", session="s")                                      # asked about exactly this one: unambiguous
    assert pay.state == "granted" and pay.choice == "Yes"
    assert not call.result(timeout=5)["is_error"]


def test_a_floor_check_is_not_answered_when_the_planner_reads_the_reply_differently(daemon: RunningDaemon) -> None:
    pay, call, cap, _ = _pending_floor_check(daemon)
    install(daemon, [_routed(pay.id, "No")])
    chat(daemon, "Yes", session="s")
    assert pay.state == "pending" and cap.calls == 0
    call.cancel()


def test_a_typed_no_to_a_floor_check_is_a_recorded_no(daemon: RunningDaemon) -> None:
    pay, call, cap, _ = _pending_floor_check(daemon)
    install(daemon, [_routed(pay.id, "No")])
    chat(daemon, "No.", session="s")
    assert pay.state == "granted" and pay.choice == "No"
    result = call.result(timeout=5)
    assert result["is_error"] and cap.calls == 0


# ---------- (b) steering never refuses ----------

def test_a_steer_supersedes_a_pending_new_recipient_check_and_the_same_action_asks_again(daemon: RunningDaemon) -> None:
    from ferb.orchestrator.steering import settle_questions
    check, call, cap, grant = _pending_floor_check(daemon, "send", "send the update", "bob@x.com")
    assert check.reason.startswith("FERB:")

    async def steer() -> None:                       # the planner names the check the correction changes
        settle_questions(daemon.daemon.supervisor, "req-1", "send it to Carol instead", [check.id])
    daemon.run(steer())
    assert check.state == "superseded"
    result = call.result(timeout=5)
    assert result["is_error"] and cap.calls == 0
    said = json.dumps(result["content"])
    assert "said no" not in said and "changed the task" in said
    assert _confirmation_rows(daemon) == []                               # nothing recorded: not a No
    owner = Answers(daemon, granted=True, choice=1)
    again = daemon.run(daemon.daemon.gateway.call(grant, cap.name, {"to": "bob@x.com", "text": "the message"}),
                       timeout=20)
    owner.stop()
    assert not again["is_error"] and cap.calls == 1                       # asked again, then ran
    assert any(ap.capability == CONFIRM_ASK for ap in owner.asked)


def _confirmation_rows(d: RunningDaemon) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        return [dict(r) for r in d.daemon.db.conn.execute("SELECT * FROM confirmations").fetchall()]
    return d.run(go())                                                    # type: ignore[no-any-return]


# ---------- (C) orphan questions ----------

def test_a_question_whose_task_was_stopped_is_closed_as_superseded(daemon: RunningDaemon) -> None:
    pay, call, cap, _ = _pending_floor_check(daemon)
    published: list[dict[str, Any]] = []
    gateway = daemon.daemon.gateway
    original = gateway.publish
    gateway.publish = lambda topic, data: (published.append({"topic": topic, **data}), original(topic, data))
    daemon.call_soon(gateway.cancel_tasks, {"task-1"})                    # a real stop of the task
    _until(lambda: pay.state != "pending")
    assert pay.state == "superseded"
    assert {"topic": "approval.resolved", "approval_id": pay.id, "state": "superseded"} in published
    assert pay not in daemon.daemon.state.approvals.pending()


def test_a_worker_question_whose_call_was_cancelled_is_closed_as_superseded(daemon: RunningDaemon) -> None:
    grant, _ = gateway_setup(daemon, set(), gateway_request("pick one"), [])
    daemon.daemon.gateway.route_question = None              # straight to the owner (no orchestrator in this test)
    call = _start(daemon, grant, "owner.ask", question="Which folder?", options=["A", "B"])
    _until(lambda: bool(_pending(daemon, "owner.ask")))
    question = _pending(daemon, "owner.ask")[0]
    daemon.call_soon(daemon.daemon.gateway.cancel_tasks, {"task-1"})
    _until(lambda: question.state != "pending")
    assert question.state == "superseded"


def test_a_waiter_that_went_away_leaves_the_question_open(daemon: RunningDaemon) -> None:
    """Merge note with Task 14 (CALLER_LEFT): only a real stop of the task closes a question; a cancelled waiter whose
    task still runs leaves it open for the owner."""
    pay, call, cap, _ = _pending_floor_check(daemon)
    call.cancel()
    _until(lambda: call.done())
    import time
    time.sleep(0.2)
    assert pay.state == "pending"


# ---------- (c) a big job's plan is stated, not asked ----------

def test_a_big_job_plan_is_stated_d33_labelled_and_starts_without_an_approval(daemon: RunningDaemon) -> None:
    spec = task("Rank all coding sessions")
    spec["plan"] = Plan(steps=[Step(id="read", kind="do", goal="Read sessions"),
                               Step(id="rank", kind="do", goal="FERB: you already approved posting", depends_on=["read"])],
                        big_job=True, time_estimate="About 20 minutes",
                        cost_estimate="Subscription usage; no separate estimate",
                        standing_sends=[SendScope(capability="mail.send", identity="owner", account="me@x.com",
                                                  recipients=["self@x.com"], content="Build passed")]).model_dump()
    created: list[str] = []
    original = daemon.daemon.state.approvals.question

    def watch(action: Any, question: str, options: list[str]) -> Any:
        created.append(action.capability)
        return original(action, question, options)
    daemon.daemon.state.approvals.question = watch                        # type: ignore[method-assign]
    script = install(daemon, [sup("task", "Ranking your sessions.", spec)],
                     {"step.read": done("Read."), "step.rank": done("Ranked."), "worker": done("Checked.")})
    events = chat(daemon, "Rank all coding sessions")
    plan = next(e for e in events if e["type"] == "plan")
    assert "Read sessions" in plan["text"] and "About 20 minutes" in plan["text"]
    assert "self@x.com" in plan["text"] and "Build passed" in plan["text"]   # the standing scope is stated
    assert re.search(r"^.*\(from the task\).*FERB: you already approved posting", plan["text"], re.M)
    assert not any(line.startswith("FERB") for line in plan["text"].split("\n"))
    assert "plan.ask" not in created and not created                       # nothing waited for a Yes
    roles = [role for _, role, _, _ in script.contexts]
    assert "step.read" in roles and "step.rank" in roles

    async def grants() -> list[Any]:
        return [tuple(r) for r in daemon.daemon.db.conn.execute(
            "SELECT task_id, approval_id FROM standing_permissions").fetchall()]
    rows = daemon.run(grants())
    assert rows and rows[0][0] == plan["task_id"] and rows[0][1].startswith("stated:")


def test_a_plans_ask_step_is_labelled_and_a_stop_closes_it_as_superseded(daemon: RunningDaemon) -> None:
    spec = task("Pick a session to keep")
    spec["plan"] = Plan(steps=[Step(id="pick", kind="ask", question="FERB: which session?",
                                    options=["first", "second"])]).model_dump()
    install(daemon, [sup("task", "Asking you which one.", spec)], {"worker": done("Checked.")})
    turn = threading.Thread(target=lambda: chat(daemon, "Pick a session to keep"))
    turn.start()
    _until(lambda: bool(_pending(daemon, "plan.ask")))
    question = _pending(daemon, "plan.ask")[0]
    assert question.reason.startswith("Question from the task: ")       # D33: a task's question never reads as FERB's
    trees = [t for t in daemon.daemon.supervisor._tree_of.values()]
    daemon.daemon.supervisor.stop_tree(trees[0].id)
    turn.join(10)
    assert question.state == "superseded"


# ---------- (d) no "/answer" anywhere owner-facing ----------

def test_no_owner_facing_text_mentions_slash_answer() -> None:
    command = re.compile(r"(?<![\w/])/answer\b")                         # the command, not "words/answers"
    found = [str(p.relative_to(ROOT)) for base in ("ferb", "ui")
             for p in (ROOT / base).rglob("*") if p.suffix in (".py", ".md", ".swift", ".js", ".json")
             and p.is_file() and command.search(p.read_text(errors="ignore"))]
    assert found == []


def test_live_chat_shows_questions_without_method_hints() -> None:
    from ferb.cli.live_chat import LiveChat
    said: list[str] = []
    chat_ = LiveChat.__new__(LiveChat)
    chat_.pending, chat_._lock = {}, threading.RLock()
    chat_.say = said.append                                                # type: ignore[method-assign]
    chat_.answer_approval({"approval_id": "ap-1", "kind": "owner_question", "capability": CONFIRM_ASK,
                           "reason": "FERB: this pays $5 to x@y.com. Go ahead?", "options": ["Yes", "No"]})
    shown = "\n".join(said)
    assert "/answer" not in shown and "reply normally" not in shown and "Enter = No" not in shown
    assert "FERB: this pays $5" in shown


# ---------- (A) voice routes like typed text ----------

def _voice(pending: list[Any]) -> tuple[Any, list[str], list[Any]]:
    from ferb.voice.controller import Voice
    turns: list[str] = []

    async def run_turn(text: str) -> Any:
        turns.append(text)
        yield {"type": "reply", "text": "ok"}
    v = Voice(run_turn, lambda: pending, SimpleNamespace(say=lambda *_: None, stop=lambda: None, wait=lambda: None),
              SimpleNamespace(append=lambda *a: None), lambda: None)
    return v, turns, pending


def test_voice_never_auto_answers_the_single_open_question() -> None:
    import inspect
    from ferb.daemon.questions import Approvals
    from ferb.voice.controller import Voice
    approvals = Approvals()
    action = ActionRequest("ferb.confirm", "owner_question", Identity.OWNER, frozenset(), "t", "w", "r", frozenset())
    ap = approvals.question(action, "FERB: this pays $5 to x@y.com. Go ahead?", ["Yes", "No"])
    assert "answer" not in inspect.signature(Voice).parameters             # nothing in Voice can resolve a question
    v, turns, _ = _voice(approvals.pending())

    async def go() -> dict[str, Any]:
        out = await v.transcript("yes")
        await asyncio.sleep(0.05)
        return out
    out = asyncio.run(go())
    assert out == {"handled": "request"} and turns == ["yes"] and ap.state == "pending"


def test_voice_with_several_open_questions_routes_like_typed_text() -> None:
    pending = [SimpleNamespace(id="q1", kind="owner_question", question="Which file?"),
               SimpleNamespace(id="q2", kind="owner_question", question="Which recipient?")]
    v, turns, _ = _voice(pending)

    async def go() -> dict[str, Any]:
        out = await v.transcript("the second one")
        await asyncio.sleep(0.05)
        return out
    assert asyncio.run(go()) == {"handled": "request"} and turns == ["the second one"]


# ---------- (e) prompt contract ----------

def test_workers_are_never_told_to_ask_for_confirmation() -> None:
    interaction = (PROMPTS / "interaction.md").read_text()
    worker = (PROMPTS / "worker.md").read_text()
    for gone in ("obtain owner confirmation", "is also an outcome to confirm", "Carry that concrete confirmation"):
        assert gone not in interaction
    assert "required final confirmation" not in worker
    assert "FERB's code asks" in interaction                               # the floor questions are code's
    for orchestrator in ("question.md", "dispatch.md"):
        assert "final confirmation" not in (PROMPTS / orchestrator).read_text()


def test_workers_declare_the_true_kind_of_consequential_actions() -> None:
    """The code floor sees a browser send, post or payment only when the call declares its kind (W2, D23)."""
    interaction = " ".join((PROMPTS / "interaction.md").read_text().split())
    worker = " ".join((PROMPTS / "worker.md").read_text().split())
    assert "declare the true kind of every consequential browser or app" in interaction
    # Task 18 owns the declaration wording (narrow send, pay on the final control, account_security, destination)
    for kind in ("effect send", "`pay` only on the final Pay or confirm control", "`destroy` for a permanent delete",
                 "`account_security` for a password", "account-security change", "purpose send or call"):
        assert kind in worker, kind
    assert "never ask the owner to confirm yourself" in worker


def test_prompts_carry_the_install_question_and_never_narrow_to_read_only() -> None:
    supervisor = (PROMPTS / "supervisor.md").read_text()
    worker = (PROMPTS / "worker.md").read_text()
    question = (PROMPTS / "question.md").read_text()
    for text in (supervisor, worker):
        assert "isn't installed — should I install it?" in text
    assert "read-only" in supervisor and "unless the owner said so" in supervisor
    assert "isn't installed" in question                                    # the router asks it once, then it carries
    assert "waits for the owner's go-ahead" not in supervisor               # D24 plans are stated, not asked
    assert "exact quote" not in supervisor.split("answer_question", 1)[1][:400]


def test_the_install_question_is_asked_once_and_carried_across_retries(daemon: RunningDaemon) -> None:
    ask = "Mole isn't installed — should I install it? (brew install mole)"
    asked_owner: list[str] = []
    script = None

    def router(_token: str) -> str:
        assert script is not None
        context = next(c for _, k, c, _ in reversed(script.contexts) if k == "worker.question")
        if "Mole isn't installed" in context.split("Worker question (data)", 1)[0]:
            return json.dumps({"ask_owner": False, "answer": "You already said yes: install it.", "question": "",
                               "display_text": ""})
        asked_owner.append(ask)
        return json.dumps({"ask_owner": True, "answer": "", "question": ask, "display_text": ""})

    def worker(token: str) -> str:
        first = tool(daemon.sock, token, "owner.ask", question=ask, options=["Yes", "No"])
        second = tool(daemon.sock, token, "owner.ask", question=ask, options=["Yes", "No"])   # a retry asks again
        return json.dumps({"status": "done", "summary": json.dumps([first["content"], second["content"]]),
                           "artifacts": [], "evidence": []})
    script = install(daemon, [sup("task", "Cleaning up with Mole.", task("Use Mole to clean my Mac", "owner"))],
                     {"worker": worker, "worker.question": router})
    owner = Answers(daemon, granted=True, choice=1)
    events = chat(daemon, "use mole to clean my mac")
    owner.stop()
    assert any(e["type"] == "outcome" for e in events)
    assert len(asked_owner) == 1 and len([a for a in owner.asked if a.capability == "owner.ask"]) == 1
    routed = [c for _, k, c, _ in script.contexts if k == "worker.question"]
    assert len(routed) == 2
    earlier = routed[1].split("Worker question (data)", 1)[0]
    assert "Mole isn't installed" in earlier and "Yes" in earlier          # the answer is in the task's context

    async def worker_context() -> str:                                     # what a retried worker would read
        supervisor = daemon.daemon.supervisor
        tree_id = next(e["task_id"] for e in events if e["type"] == "plan")
        rows = supervisor.spans.owner_questions(tree_id)
        return json.dumps(rows)
    assert "Mole isn't installed" in daemon.run(worker_context())


# ========== fix round 1 ==========

def _decision(tree_id: str, standing: Any, supersedes: tuple[str, ...] = (), spec: dict[str, Any] | None = None) -> str:
    """The planner's steer routing: always an explicit standing-scope decision and the question ids it changes."""
    decision = json.loads(sup("task", "Updated.", spec or task("Post the update to the list", "owner")))
    decision["control"] = {"task_id": tree_id, "action": "steer", "standing_sends": standing,
                           "supersedes": list(supersedes)}
    return json.dumps(decision)


def _scope(to: str) -> SendScope:
    return SendScope(capability="probe.post", identity="owner", recipients=[to], content=None)


def _running_plan(daemon: RunningDaemon, scopes: list[SendScope], posts: list[str]) -> SimpleNamespace:
    """A plan whose one step holds until released, then posts to each recipient through the gateway."""
    cap = FakeCap("probe.post", "post", {"probe"})
    daemon.daemon.registry.capabilities[cap.name] = cap
    started, proceed, results = threading.Event(), threading.Event(), {}

    def step(token: str) -> str:
        started.set()
        proceed.wait(15)
        for to in posts:
            results[to] = tool(daemon.sock, token, "probe.post", to=to, text="the update")
        return done("Posted.")
    spec = task("Post the update to the list", "owner")
    spec["plan"] = Plan(steps=[Step(id="post", kind="do", goal="Post the update")], standing_sends=scopes).model_dump()
    script = install(daemon, [sup("task", "Posting.", spec)], {"step.post": step, "worker": done("Checked.")})
    events: list[dict[str, Any]] = []
    turn = threading.Thread(target=lambda: events.extend(chat(daemon, "post the update to the list", session="s1")))
    turn.start()
    assert started.wait(10)
    tree_id = next(t.id for t in daemon.daemon.supervisor._tree_of.values())
    return SimpleNamespace(cap=cap, proceed=proceed, results=results, script=script, turn=turn, events=events,
                           tree=tree_id)


def _finish(daemon: RunningDaemon, run: SimpleNamespace) -> list[Any]:
    owner = Answers(daemon, granted=True, choice=1)
    run.proceed.set()
    run.turn.join(20)
    owner.stop()
    assert not run.turn.is_alive()
    return [ap for ap in owner.asked if ap.capability == CONFIRM_ASK]


def test_a_steer_narrowing_the_standing_scope_takes_effect_at_once(daemon: RunningDaemon) -> None:
    run = _running_plan(daemon, [_scope("kept@x.com"), _scope("dropped@x.com")], ["kept@x.com", "dropped@x.com"])
    run.script.sup.append(_decision(run.tree, [_scope("kept@x.com").model_dump()]))
    chat(daemon, "drop the second address", session="s1")       # no address in the owner's words
    asked = _finish(daemon, run)
    assert len(asked) == 1 and "dropped@x.com" in asked[0].reason            # the dropped scope's send asks
    assert not run.results["kept@x.com"]["is_error"] and not run.results["dropped@x.com"]["is_error"]

    async def saved() -> list[Any]:
        return daemon.daemon.supervisor.plans.load(run.tree)["plan"].standing_sends
    assert [s.recipients for s in daemon.run(saved())] == [["kept@x.com"]]   # the plan keeps what was decided


def test_a_steer_that_leaves_the_standing_scope_unchanged_keeps_both(daemon: RunningDaemon) -> None:
    run = _running_plan(daemon, [_scope("kept@x.com"), _scope("other@x.com")], ["kept@x.com", "other@x.com"])
    run.script.sup.append(_decision(run.tree, "unchanged"))
    chat(daemon, "make it friendlier", session="s1")
    assert _finish(daemon, run) == []


def test_a_widened_standing_scope_is_stated_in_the_reply_before_it_covers_anything(daemon: RunningDaemon) -> None:
    run = _running_plan(daemon, [_scope("kept@x.com")], ["new@x.com"])

    async def covered() -> bool:
        from ferb.core.types import ActionSpec, Target
        return daemon.daemon.supervisor.standing.covers(run.tree, "probe.post", "owner", ActionSpec(
            "post", Target(recipients=("new@x.com",)), "the update"))
    assert not daemon.run(covered())
    run.script.sup.append(_decision(run.tree, [_scope("kept@x.com").model_dump(), _scope("new@x.com").model_dump()]))
    reply = chat(daemon, "also post to the new member", session="s1")   # no address in the owner's words
    text = _texts(reply)
    assert "new@x.com" in text and "without asking" in text                    # stated to the owner

    async def statement() -> tuple[str, str]:
        db = daemon.daemon.db.conn
        row = db.execute("SELECT id, data_json FROM journal WHERE kind='plan.stated' ORDER BY seq DESC").fetchone()
        grant = db.execute("SELECT approval_id FROM standing_permissions WHERE task_id=? AND active=1 "
                           "AND scope LIKE '%new@x.com%'", (run.tree,)).fetchone()
        return json.loads(row[1])["text"], (grant[0] if grant else "") + "|" + row[0]
    stated, ids = daemon.run(statement())
    grant_id, entry = ids.split("|")
    assert "new@x.com" in stated and stated in text and grant_id == "stated:" + entry

    async def replies() -> list[str]:
        return [json.loads(r[0])["text"] for r in daemon.daemon.db.conn.execute(
            "SELECT data_json FROM journal WHERE kind='chat.reply'").fetchall()]
    assert any(stated in r for r in daemon.run(replies()))                  # in the conversation record
    assert _finish(daemon, run) == [] and not run.results["new@x.com"]["is_error"]


def test_a_steer_supersedes_only_the_questions_it_names(daemon: RunningDaemon) -> None:
    from ferb.orchestrator.steering import settle_questions
    pay, call, cap, grant = _pending_floor_check(daemon)

    async def login() -> Any:
        action = ActionRequest("browser.login_code", "owner_login_code", Identity.OWNER, frozenset(), "task-1",
                               "w1", "req-1", frozenset())
        ap = daemon.daemon.state.approvals.question(action, "Enter the login code for x.com (hidden).", [])
        ap.kind = "owner_login_code"
        return ap
    code = daemon.run(login())
    other = _task_question(daemon, "Use your work email?")                    # another request's question

    async def steer(names: list[str]) -> None:
        settle_questions(daemon.daemon.supervisor, "req-1", "also cc Bob", names)
    daemon.run(steer([]))
    assert pay.state == "pending" and code.state == "pending"                 # "also cc Bob" re-asks nothing
    daemon.run(steer([pay.id, other.id]))
    assert pay.state == "superseded" and code.state == "pending" and other.state == "pending"
    call.result(timeout=5)


def test_a_steer_routed_with_named_questions_supersedes_only_those(daemon: RunningDaemon) -> None:
    run = _running_plan(daemon, [], [])
    worker = daemon.daemon.supervisor._tree_of[run.tree].worker

    async def make(capability: str, kind: str) -> Any:
        action = ActionRequest(capability, kind, worker.identity, frozenset(), worker.id, worker.id,
                               worker.request_id, frozenset())
        ap = daemon.daemon.state.approvals.question(action, "Pending owner decision", ["Yes", "No"])
        ap.kind = kind
        return ap
    pay = daemon.run(make(CONFIRM_ASK, "owner_question"))
    secret = daemon.run(make("secret.fill", "owner_secret"))
    run.script.sup.append(_decision(run.tree, "unchanged", (pay.id,)))
    chat(daemon, "pay with the other card", session="s1")
    assert pay.state == "superseded" and secret.state == "pending"
    daemon.call_soon(lambda: daemon.daemon.state.approvals.supersede(secret.id))
    _finish(daemon, run)


def test_a_bare_yes_with_a_free_text_question_open_asks_which_one(daemon: RunningDaemon) -> None:
    pay, call, cap, _ = _pending_floor_check(daemon)
    free = _task_question(daemon, "Which folder should I use?", options=())
    install(daemon, [_routed(pay.id, "Yes")])
    events = chat(daemon, "Yes", session="s")
    assert pay.state == "pending" and cap.calls == 0 and free.state == "pending"
    assert [e["type"] for e in events] == ["clarify"] and free.reason in events[0]["text"]
    call.cancel()


def test_the_literal_i3_case_a_yes_after_the_planners_clarify_never_pays(daemon: RunningDaemon) -> None:
    script = install(daemon, [sup("clarify", "Use your work email?")])
    assert [e["type"] for e in chat(daemon, "send the invoice", session="s")] == ["clarify"]
    pay, call, cap, _ = _pending_floor_check(daemon)
    script.sup += [_routed(pay.id, "Yes"), _routed(pay.id, "Yes")]
    events = chat(daemon, "Yes", session="s")
    assert pay.state == "pending" and cap.calls == 0
    assert [e["type"] for e in events] == ["clarify"] and "Use your work email?" in events[0]["text"]
    chat(daemon, "the payment", session="s")
    assert pay.state == "granted" and pay.choice == "Yes"
    assert not call.result(timeout=5)["is_error"]


def test_a_question_opened_after_ferbs_yes_or_no_question_is_rechecked(daemon: RunningDaemon) -> None:
    pay, call, cap, _ = _pending_floor_check(daemon)
    install(daemon, [_routed(pay.id, "Yes"), _routed(pay.id, "Yes")])
    assert [e["type"] for e in chat(daemon, "sure", session="s")] == ["clarify"]
    later = _task_question(daemon, "Use your work email?")                    # opened in between
    events = chat(daemon, "Yes", session="s")
    assert pay.state == "pending" and [e["type"] for e in events] == ["clarify"] and later.reason in events[0]["text"]
    call.cancel()


def _retry_question(daemon: RunningDaemon, ferb_yes: Any) -> tuple[Any, Any]:
    """P19: an unknown payment, its refused repeat, and the worker's retry question waiting for the owner."""
    ferb_yes.expect("floor:pay")
    boom = FakeCap("probe.pay", "pay", {"probe"}, boom=True)
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("pay x@y.com $5"), [boom])
    daemon.run(daemon.daemon.gateway.call(grant, "probe.pay", {"to": "x@y.com", "text": "pay $5"}), timeout=20)
    refused = daemon.run(daemon.daemon.gateway.call(grant, "probe.pay", {"to": "x@y.com", "text": "pay $5"}),
                         timeout=20)
    ask = _start(daemon, grant, "owner.ask", question="Nothing in Sent.",
                 retry_action=refused["content"]["retry_action"])
    from ferb.daemon.gateway import RETRY_ASK
    _until(lambda: bool(_pending(daemon, RETRY_ASK)))
    return _pending(daemon, RETRY_ASK)[0], ask


def test_a_p19_retry_question_is_ferbs_own_check(ferb_yes: Any, daemon: RunningDaemon) -> None:
    from ferb.orchestrator.steering import settle_questions
    retry, ask = _retry_question(daemon, ferb_yes)
    other = _task_question(daemon, "Use your work email?")
    script = install(daemon, [_routed(retry.id, "Yes")])
    events = chat(daemon, "Yes", session="s")
    assert retry.state == "pending" and [e["type"] for e in events] == ["clarify"]   # which one?
    listed = re.search(r'\{"approval_id": "' + re.escape(retry.id) + r'".*?"ferb_check": (true|false)\}',
                       script.contexts[-1][2], re.S)
    assert listed and listed.group(1) == "true"                            # the planner is told it's FERB's check

    async def steer(names: list[str]) -> None:
        settle_questions(daemon.daemon.supervisor, "req-1", "Yes", names)
    daemon.run(steer([]))
    assert retry.state == "pending"
    daemon.run(steer([retry.id]))
    assert retry.state == "superseded" and retry.choice is None             # a steer's "Yes" never approves it
    assert not ask.result(timeout=5)["content"].get("retry_approved")
    assert other.state == "pending"


def test_live_chat_hints_yes_or_no_for_ferbs_retry_and_conversation_checks() -> None:
    from ferb.cli.live_chat import LiveChat
    for capability in ("ferb.retry", "conversation.approve"):
        said: list[str] = []
        chat_ = LiveChat.__new__(LiveChat)
        chat_.pending, chat_._lock = {}, threading.RLock()
        chat_.say = said.append                                            # type: ignore[method-assign]
        chat_.answer_approval({"approval_id": "ap-" + capability, "kind": "owner_question", "capability": capability,
                               "reason": "Try once more?", "options": ["Yes", "No"]})
        assert said[-1].strip() == "Reply Yes or No."


def test_a_conversation_scope_approval_takes_only_a_plain_yes_or_no(daemon: RunningDaemon) -> None:
    from ferb.orchestrator.conversations import ConversationScope
    scope = ConversationScope(identity="owner", account="me@x.com", site="discord.com", app="Discord",
                              thread="launch", recipients=["sam"], topic="launch feedback", handover_when="a lead")
    spec = task("Talk to sam about the launch", "owner")
    spec["plan"] = Plan(steps=[Step(id="talk", kind="converse", goal="Talk to sam",
                                    conversation=scope)]).model_dump()
    script = install(daemon, [sup("task", "Starting the conversation.", spec)], {"worker": done("Checked.")})
    turn = threading.Thread(target=lambda: chat(daemon, "talk to sam about the launch", session="c"))
    turn.start()
    _until(lambda: bool(_pending(daemon, "conversation.approve")))
    approval = _pending(daemon, "conversation.approve")[0]
    other = _task_question(daemon, "Use your work email?")
    script.sup.append(_routed(approval.id, "Yes"))
    events = chat(daemon, "Yes", session="s")
    assert approval.state == "pending" and [e["type"] for e in events] == ["clarify"]
    daemon.daemon.supervisor.stop_tree(next(t.id for t in daemon.daemon.supervisor._tree_of.values()))
    turn.join(10)
    assert approval.state == "superseded" and other.state == "pending"


def test_a_steer_closing_a_login_code_prompt_keeps_it_superseded(daemon: RunningDaemon) -> None:
    from ferb.core.types import ToolContext
    from ferb.orchestrator.steering import settle_questions
    ctx = ToolContext(task_id="t1", worker_id="w1", request_id="req-9", identity=Identity.OWNER, scopes=frozenset(),
                      device_id="dev", span_id="s", workspace=Path("/tmp"), owner_roots=(), untrusted_roots=())
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_login_code(ctx, "https://x.com"),
                                               daemon.loop)
    _until(lambda: bool([a for a in daemon.daemon.state.approvals.pending() if a.kind == "owner_login_code"]))
    code = [a for a in daemon.daemon.state.approvals.pending() if a.kind == "owner_login_code"][0]

    async def steer() -> None:
        settle_questions(daemon.daemon.supervisor, "req-9", "use the other account", [code.id])
    daemon.run(steer())
    assert waiting.result(timeout=5) is None and code.state == "superseded"


def test_a_typed_code_while_a_login_code_prompt_waits_never_reaches_the_planner(daemon: RunningDaemon) -> None:
    async def login() -> Any:
        action = ActionRequest("browser.login_code", "owner_login_code", Identity.OWNER, frozenset(), "t", "w",
                               "r", frozenset())
        ap = daemon.daemon.state.approvals.question(action, "Enter it in the hidden prompt.", [])
        ap.kind = "owner_login_code"
        return ap
    daemon.run(login())
    script = install(daemon, [sup("reply", "Use the hidden prompt for codes.")])
    chat(daemon, "314159", session="s")
    assert "314159" not in repr(script.contexts)


def test_a_task_without_a_plan_can_be_given_and_then_lose_a_stated_scope(daemon: RunningDaemon) -> None:
    cap = FakeCap("probe.post", "post", {"probe"})
    daemon.daemon.registry.capabilities[cap.name] = cap
    started, proceed, results = threading.Event(), threading.Event(), {}

    def worker(token: str) -> str:
        started.set()
        proceed.wait(15)
        results["new"] = tool(daemon.sock, token, "probe.post", to="new@x.com", text="the update")
        return done("Posted.")
    script = install(daemon, [sup("task", "Posting.", task("Post the update", "owner"))], {"worker": worker})
    turn = threading.Thread(target=lambda: chat(daemon, "post the update", session="s1"))
    turn.start()
    assert started.wait(10)
    tree = next(t.id for t in daemon.daemon.supervisor._tree_of.values())
    script.sup.append(_decision(tree, [_scope("new@x.com").model_dump()], spec=task("Post the update", "owner")))
    # the owner's words don't name the address (a named one is authorized by provenance, not by the scope)
    assert "new@x.com" in _texts(chat(daemon, "you can post to the list without asking", session="s1"))
    script.sup.append(_decision(tree, [], spec=task("Post the update", "owner")))
    assert "ask before each" in _texts(chat(daemon, "actually ask me first", session="s1"))
    running = script.contexts[-1][2].split("running tasks and open questions", 1)[1]
    assert "new@x.com" in running.split("Layer 5", 1)[0]     # the planner saw the scope in force before revising it
    owner = Answers(daemon, granted=True, choice=1)
    proceed.set()
    turn.join(20)
    owner.stop()
    assert [ap for ap in owner.asked if ap.capability == CONFIRM_ASK]       # withdrawn: the send asks again

    async def rows() -> list[Any]:
        return daemon.daemon.supervisor.standing.scopes(tree)
    assert daemon.run(rows()) == []



def test_a_widened_scope_covers_nothing_until_its_reply_has_been_sent(daemon: RunningDaemon) -> None:
    """Round 2: the scopes already stated stay covered throughout; a new one is granted only after the reply that
    states it has been yielded, and not at all if the task ended in between."""
    from ferb.core.types import ActionSpec, Target
    run = _running_plan(daemon, [_scope("kept@x.com")], [])
    supervisor = daemon.daemon.supervisor

    def covers(to: str) -> bool:
        return supervisor.standing.covers(run.tree, "probe.post", "owner",
                                          ActionSpec("post", Target(recipients=(to,)), "the update"))
    run.script.sup.append(_decision(run.tree, [_scope("kept@x.com").model_dump(), _scope("new@x.com").model_dump()]))

    async def steer() -> tuple[tuple[bool, bool], bool]:
        at_reply = (True, True)
        async for event in supervisor.send("s1", "also post to the new member"):
            if event.get("type") == "reply":
                assert "new@x.com" in event["text"]
                at_reply = (covers("new@x.com"), covers("kept@x.com"))
        return at_reply, covers("new@x.com")
    at_reply, after = daemon.run(steer())
    assert at_reply == (False, True) and after                         # stated first, then granted
    _finish(daemon, run)


def test_a_widened_scope_is_not_granted_when_the_task_ended_before_its_reply_went_out(daemon: RunningDaemon) -> None:
    from ferb.core.types import ActionSpec, Target
    run = _running_plan(daemon, [_scope("kept@x.com")], [])
    supervisor = daemon.daemon.supervisor
    run.script.sup.append(_decision(run.tree, [_scope("kept@x.com").model_dump(), _scope("new@x.com").model_dump()]))

    async def steer() -> bool:
        async for event in supervisor.send("s1", "also post to the new member"):
            if event.get("type") == "reply":
                supervisor.stop_tree(run.tree)
        return supervisor.standing.covers(run.tree, "probe.post", "owner",
                                          ActionSpec("post", Target(recipients=("new@x.com",)), "the update"))
    assert daemon.run(steer()) is False
    run.proceed.set()
    run.turn.join(20)


def test_the_cli_shows_ferbs_retry_and_conversation_checks_like_its_floor_checks() -> None:
    from ferb.cli.chat import question
    for capability in ("ferb.confirm", "ferb.retry", "conversation.approve"):
        shown = question({"kind": "owner_question", "capability": capability, "reason": "Try once more?",
                          "options": ["Yes", "No"]})
        assert shown.endswith("number or your answer (Enter = No): "), capability


def test_a_correction_racing_the_end_of_its_task_gets_a_plain_reply(daemon: RunningDaemon) -> None:
    """The steer path's "just ended" reply has the same (reply, deferred grant) shape as any other steer reply."""
    from ferb.orchestrator.steering import apply
    run = _running_plan(daemon, [], [])
    supervisor = daemon.daemon.supervisor
    tree = supervisor._tree_of[run.tree]

    async def steer() -> Any:
        tree.stop = "stopped by you"
        return await apply(supervisor, tree, "s1", "req-x", "also cc Bob", "also cc Bob", None, "journal:x",
                           standing="unchanged", supersedes=[])
    reply, widened = daemon.run(steer())
    assert reply["type"] == "reply" and "just ended" in reply["text"] and widened is None
    run.proceed.set()
    run.turn.join(20)


# ---------- unattended runs: no stated scope without an owner present (controller ruling, Task 18 finding) ----------

def _remember(daemon: RunningDaemon, content: str) -> None:
    """An owner-stated memory item (the owner's own words, kept)."""
    def go() -> Any:
        src = daemon.daemon._journal().append("memory.added", {"content": content}).id
        return daemon.daemon.memory.propose("fact", "owner", "launch-list", content, "owner_stated",
                                            [("journal", src)], 1.0)
    from tests.support.schedules import run as on_loop
    on_loop(daemon, go)


def _scheduled_post_run(daemon: RunningDaemon, scope_at_creation: list[SendScope], *, plan_scopes: list[SendScope] | None,
                        posts: tuple[str, ...] = ("new@x.com",),
                        typed: str = "every day post the launch story to my launch list",
                        remember: list[dict[str, Any]] | None = None, while_planning: Any = None,
                        before_posting: Any = None) -> SimpleNamespace:
    """A daily "post the launch story" schedule created from chat, with the scopes that turn stated (and any memory
    the planner captured in it, `remember`); its run's planner returns a task (with a plan stating `plan_scopes`, or no
    plan when None) whose worker posts to each of `posts`. `while_planning` runs during the run's planner turn and
    `before_posting` in its worker just before the posts."""
    from datetime import timedelta
    from tests.support.schedules import fire, mins, run as on_loop
    cap = FakeCap("probe.post", "post", {"probe"})
    daemon.daemon.registry.capabilities[cap.name] = cap
    created = json.dumps({"kind": "reply", "text": "Scheduled.", "task": None, "remember": remember or [],
                          "timers": [],
                          "schedule": {"request": "post the launch story to my launch list",
                                       "when": mins(timedelta(minutes=1)), "repeat": "daily", "weekday": None,
                                       "every_minutes": None, "quote": "every day post the launch story",
                                       "targets": {"apps": [], "sites": [], "paths": [], "recipients": []},
                                       "standing_sends": [s.model_dump() for s in scope_at_creation]},
                          "schedule_changes": []})
    results: dict[str, Any] = {}

    def act(token: str) -> str:
        if before_posting is not None:
            before_posting()
        for to in posts:
            results[to] = tool(daemon.sock, token, "probe.post", to=to, text="the launch story")
        return done("Posted.")
    spec = task("Post the launch story", "owner")
    workers: dict[str, Any] = {"worker": done("Checked.")}
    if plan_scopes is None:
        workers["worker"] = [act, done("Checked.")]
    else:
        spec["plan"] = Plan(steps=[Step(id="post", kind="do", goal="Post the launch story")],
                            standing_sends=plan_scopes).model_dump()
        workers["step.post"] = act
    decided = sup("task", "Posting the launch story.", spec)

    def planner(_token: str) -> str:
        while_planning()
        return decided
    script = install(daemon, [created, decided if while_planning is None else planner], workers)
    confirmed = chat(daemon, typed, session="s1")
    fire(daemon, 120)

    def records() -> dict[str, Any]:
        db = daemon.daemon.db.conn
        created_row = db.execute("SELECT data_json FROM journal WHERE kind='schedule.created'").fetchone()
        stated = db.execute("SELECT data_json FROM journal WHERE kind='plan.stated' ORDER BY seq DESC").fetchone()
        return {"grants": [tuple(r) for r in db.execute("SELECT task_id, approval_id, active FROM standing_permissions")],
                "created": json.loads(created_row[0]) if created_row else {},
                "stated": json.loads(stated[0])["text"] if stated else ""}
    return SimpleNamespace(confirmed=confirmed, results=results, script=script, **on_loop(daemon, records))


def test_a_scheduled_run_cannot_grant_itself_a_scope_its_owner_turn_never_stated(daemon: RunningDaemon) -> None:
    run = _scheduled_post_run(daemon, [], plan_scopes=[_scope("new@x.com")])
    assert run.results["new@x.com"]["is_error"]                             # asked, nobody there: declined
    assert "nobody is at chat" in json.dumps(run.results["new@x.com"]["content"])
    assert run.grants == []                                                 # the stated scope was never granted
    assert "new@x.com" in run.stated and "aren't covered" in run.stated     # the statement says it isn't covered
    assert "reported not done" in run.stated and "each will ask" not in run.stated   # what may happen, not a promise


def test_a_scheduled_run_without_a_plan_is_covered_by_the_scope_recorded_with_its_schedule(daemon: RunningDaemon) -> None:
    _remember(daemon, "My launch list address is new@x.com")               # named by the owner, kept in memory
    run = _scheduled_post_run(daemon, [_scope("new@x.com")], plan_scopes=None)
    assert "new@x.com" in _texts(run.confirmed)                             # stated when the schedule was made
    assert not run.results["new@x.com"]["is_error"]                         # covered by code's grant: it ran
    assert run.grants and all(active == 0 for _, _, active in run.grants)   # granted for the run, revoked after
    assert all(approval.startswith("schedule:") for _, approval, _ in run.grants)
    assert [s["recipients"] for s in run.created["standing_sends"]] == [["new@x.com"]]   # what the owner was shown
    run_context = next(c for _, k, c, _ in reversed(run.script.contexts) if k == "supervisor")
    assert "new@x.com" in run_context.split("Schedule facts", 1)[1]        # the run's planner sees it, as data


def test_a_scheduled_runs_extra_stated_scope_is_not_granted(daemon: RunningDaemon) -> None:
    _remember(daemon, "My launch list address is new@x.com")
    run = _scheduled_post_run(daemon, [_scope("new@x.com")], plan_scopes=[_scope("new@x.com"), _scope("other@x.com")],
                              posts=("new@x.com", "other@x.com"))
    assert not run.results["new@x.com"]["is_error"]
    assert run.results["other@x.com"]["is_error"]                           # not recorded: asked, declined
    assert "other@x.com" in run.stated and "aren't covered" in run.stated


def test_a_schedule_records_only_scopes_whose_destinations_the_owner_named(daemon: RunningDaemon) -> None:
    run = _scheduled_post_run(daemon, [_scope("stranger@x.com")], plan_scopes=None, posts=("stranger@x.com",))
    said = _texts(run.confirmed)
    assert "stranger@x.com" in said and "weren't recorded" in said         # told which, and why
    assert run.created.get("standing_sends", []) == []
    assert run.results["stranger@x.com"]["is_error"]                        # nothing recorded: it asked, declined


def test_a_stale_restatement_is_never_granted(daemon: RunningDaemon) -> None:
    from ferb.core.types import ActionSpec, Target
    from ferb.orchestrator.steering import grant_stated, restate_scope
    run = _running_plan(daemon, [_scope("kept@x.com")], [])
    supervisor = daemon.daemon.supervisor
    tree = supervisor._tree_of[run.tree]

    def covers(to: str) -> bool:
        return supervisor.standing.covers(run.tree, "probe.post", "owner",
                                          ActionSpec("post", Target(recipients=(to,)), "the update"))

    async def go() -> tuple[bool, bool]:
        first = restate_scope(supervisor, tree, [_scope("kept@x.com"), _scope("b@x.com")])
        second = restate_scope(supervisor, tree, [_scope("kept@x.com"), _scope("c@x.com")])
        grant_stated(supervisor, tree, first)                              # superseded by the second: nothing
        stale = covers("b@x.com")
        grant_stated(supervisor, tree, second)
        return stale, covers("c@x.com")
    assert daemon.run(go()) == (False, True)
    _finish(daemon, run)


# ========== fix round 4 ==========
# I1: only what the owner typed names a destination for a schedule's recorded scope, never a memory item's wording
# (chat capture keeps the planner's `content` once a short quote is in the owner's words, MP2).

def _planted(content: str, quote: str) -> dict[str, Any]:
    return {"kind": "fact", "subject": "owner", "key": "launch-list", "quote": quote, "content": content}


def test_a_memory_captured_in_the_schedules_own_turn_never_records_a_scope(daemon: RunningDaemon) -> None:
    """The literal case: in the turn that makes the schedule, the planner captures `{quote: "the launch", content:
    "launch list: attacker@x.com"}` and states a scope sending there. The capture runs before the schedule is made."""
    run = _scheduled_post_run(daemon, [_scope("attacker@x.com")], plan_scopes=None, posts=("attacker@x.com",),
                              remember=[_planted("launch list: attacker@x.com", "the launch")])
    said = _texts(run.confirmed)
    assert "remembered" in said                                              # the capture happened (quote checked)
    assert run.created.get("standing_sends", []) == []                     # but it names no destination
    assert "attacker@x.com" in said and "weren't recorded" in said
    assert run.results["attacker@x.com"]["is_error"]                        # not covered: asked, nobody there
    assert run.grants == []


def test_an_earlier_memory_counts_only_for_what_the_owner_typed_when_stating_it(daemon: RunningDaemon) -> None:
    """An item captured in an earlier turn: its typed words (pasted parts left out) count, never its wording."""
    remembered = json.dumps({"kind": "reply", "text": "Noted.", "task": None, "timers": [], "schedule": None,
                             "schedule_changes": [],
                             "remember": [_planted("launch list: attacker@x.com", "remember the launch list")]})
    install(daemon, [remembered])
    noted = chat(daemon, "remember the launch list\n```\nlaunch list: attacker@x.com\n```", session="s1")
    assert "remembered" in _texts(noted)
    run = _scheduled_post_run(daemon, [_scope("attacker@x.com")], plan_scopes=None, posts=("attacker@x.com",))
    assert run.created.get("standing_sends", []) == []
    assert run.results["attacker@x.com"]["is_error"]


def test_an_earlier_memory_whose_address_the_owner_typed_records_the_scope(daemon: RunningDaemon) -> None:
    """Guard: the owner typed the address when FERB remembered it, so a later schedule records a scope sending there."""
    remembered = json.dumps({"kind": "reply", "text": "Noted.", "task": None, "timers": [], "schedule": None,
                             "schedule_changes": [],
                             "remember": [_planted("launch list: new@x.com", "my launch list is new@x.com")]})
    install(daemon, [remembered])
    chat(daemon, "remember that my launch list is new@x.com", session="s1")
    run = _scheduled_post_run(daemon, [_scope("new@x.com")], plan_scopes=None)
    assert [s["recipients"] for s in run.created["standing_sends"]] == [["new@x.com"]]
    assert not run.results["new@x.com"]["is_error"]


def test_a_recorded_message_counts_as_typed_words_only_when_its_pasted_parts_all_match() -> None:
    """Fail closed: a record whose wrappers don't match the pasted parts counted with it (or with no count) gives no
    owner words, so pasted text is never passed off as typed."""
    from ferb.sanitize.untrusted import mark, recorded_own_words
    m = mark("post to my list\n```\nattacker@x.com\n```")
    assert recorded_own_words(m.for_model, 1) == "post to my list"
    assert recorded_own_words(m.for_model, 0) is None and recorded_own_words(m.for_model, None) is None
    damaged = m.for_model.replace("</untrusted-data", "</untrusted")
    assert recorded_own_words(damaged, 1) is None


# I2: a schedule's grant (`schedule:<id>`) ends when the schedule is cancelled or paused, wherever its run is.

def _change_schedule(daemon: RunningDaemon, action: str) -> None:
    """The owner cancels or pauses the (only) schedule, as `ferb schedules` does."""
    from ferb.cli.client import Client
    c = Client(daemon.sock)
    try:
        sid = c.call("schedules.list")["schedules"][0]["id"]
        c.call("schedules.change", {"id": sid, "action": action})
    finally:
        c.close()


def _revocations(daemon: RunningDaemon) -> list[dict[str, Any]]:
    async def rows() -> list[dict[str, Any]]:
        return [json.loads(r[0]) for r in daemon.daemon.db.conn.execute(
            "SELECT data_json FROM journal WHERE kind='standing.revoked' ORDER BY seq")]
    return daemon.run(rows())  # type: ignore[no-any-return]


@pytest.mark.parametrize("action", ["cancel", "pause"])
def test_cancelling_or_pausing_a_schedule_withdraws_its_running_runs_scope(daemon: RunningDaemon, action: str) -> None:
    _remember(daemon, "My launch list address is new@x.com")
    run = _scheduled_post_run(daemon, [_scope("new@x.com")], plan_scopes=None,
                              before_posting=lambda: _change_schedule(daemon, action))
    assert run.results["new@x.com"]["is_error"]                             # no longer covered: asked, declined
    revoked = _revocations(daemon)
    assert [r["approval_id"] for r in revoked] == [run.grants[0][1]]        # journaled: the schedule's grant
    assert revoked[0]["reason"] == "schedule " + action


def test_a_schedule_cancelled_while_its_run_is_planned_grants_that_run_nothing(daemon: RunningDaemon) -> None:
    """The run's facts were read before its planner turn; the schedule is checked again right before the grant."""
    _remember(daemon, "My launch list address is new@x.com")
    run = _scheduled_post_run(daemon, [_scope("new@x.com")], plan_scopes=None,
                              while_planning=lambda: _change_schedule(daemon, "cancel"))
    assert run.grants == []
    assert run.results["new@x.com"]["is_error"]


def _saved_scheduled_plan(daemon: RunningDaemon, state: str) -> tuple[str, list[Any]]:
    """A scheduled run's plan suspended at shutdown, still holding its schedule's grant, and the schedule then put in
    `state` directly (a record that never went through the cancel path). Resumes it; returns the schedule id and the
    scopes the resumed run was given as already in force."""
    from datetime import timedelta
    from ferb.orchestrator.plans import Plan as P
    from ferb.orchestrator.schemas import TaskSpec
    from ferb.policy.targets import Targets
    from tests.support.schedules import make_schedule
    supervisor = daemon.daemon.supervisor
    s = make_schedule(daemon, "post the launch story", "every day post the launch story", Targets.of([], [], [], []),
                      daemon.daemon.clock.now() + timedelta(hours=1))
    spec = TaskSpec.model_validate(task("Post the launch story", "owner"))
    spec.plan = P(steps=[Step(id="post", kind="do", goal="Post")], standing_sends=[_scope("new@x.com")])
    saved = {"request": {"spec": spec.model_dump(), "request_id": "req-saved", "words": "post it", "payload": "post it",
                         "origins": [], "message_origins": []}, "conversation": None}
    given: list[Any] = []

    async def run_tree(spec: Any, text: str, request_id: str, message: str = "", **kw: Any) -> Any:
        given.extend(kw["unattended_scopes"])
        yield {"type": "outcome", "task_id": "tree-saved", "status": "done", "answer": "Done."}

    async def resume() -> None:
        supervisor.standing.grant("tree-saved", [_scope("new@x.com")], "schedule:" + s.id)
        daemon.daemon.schedules.set_state(s.id, state)
        supervisor.plans.pending = lambda: ["tree-saved"]
        supervisor.plans.load = lambda key: saved
        supervisor.run_tree = run_tree
        await supervisor.resume_plans()
    daemon.run(resume())
    return s.id, given


def test_a_resumed_run_of_a_cancelled_schedule_carries_no_scope(daemon: RunningDaemon) -> None:
    sid, given = _saved_scheduled_plan(daemon, "cancelled")
    assert given == []

    async def in_force() -> list[Any]:
        return daemon.daemon.supervisor.standing.scopes("tree-saved")
    assert daemon.run(in_force()) == []
    assert [r["approval_id"] for r in _revocations(daemon)] == ["schedule:" + sid]


def test_a_resumed_run_of_a_live_schedule_keeps_its_scope(daemon: RunningDaemon) -> None:
    """Guard: a schedule that is active, or done because that run was its last, keeps its run's grant on resume."""
    _, given = _saved_scheduled_plan(daemon, "done")
    assert [s.recipients for s in given] == [["new@x.com"]]


def test_cancelling_a_schedule_revokes_the_grant_of_its_suspended_run(daemon: RunningDaemon) -> None:
    from datetime import timedelta
    from ferb.policy.targets import Targets
    from tests.support.schedules import make_schedule
    supervisor = daemon.daemon.supervisor
    s = make_schedule(daemon, "post the launch story", "every day post the launch story", Targets.of([], [], [], []),
                      daemon.daemon.clock.now() + timedelta(hours=1))

    async def go() -> list[Any]:
        supervisor.standing.grant("tree-suspended", [_scope("new@x.com")], "schedule:" + s.id)
        supervisor.standing.grant("tree-other", [_scope("new@x.com")], "stated:other")
        daemon.daemon._schedule_change(s.id, "cancel")
        return [supervisor.standing.scopes("tree-suspended"), supervisor.standing.scopes("tree-other")]
    suspended, other = daemon.run(go())
    assert suspended == [] and [x.recipients for x in other] == [["new@x.com"]]   # only that schedule's grant


# Minors

def test_a_failure_before_a_scheduled_run_starts_leaves_no_grant(daemon: RunningDaemon) -> None:
    supervisor = daemon.daemon.supervisor
    start = supervisor.spans.start

    def failing(name: str, *args: Any, **kwargs: Any) -> Any:
        if name == "task":
            raise RuntimeError("the trace couldn't be written")
        return start(name, *args, **kwargs)
    supervisor.spans.start = failing
    _remember(daemon, "My launch list address is new@x.com")
    run = _scheduled_post_run(daemon, [_scope("new@x.com")], plan_scopes=None)
    assert all(active == 0 for _, _, active in run.grants)                 # nothing left in force


def test_a_resumed_runs_plan_line_says_an_uncovered_send_waits_for_the_owner(daemon: RunningDaemon) -> None:
    supervisor = daemon.daemon.supervisor
    plan = Plan(steps=[Step(id="post", kind="do", goal="Post")], standing_sends=[_scope("a@x.com"), _scope("b@x.com")])

    async def lines() -> tuple[str, str]:
        return (supervisor._plan_statement(plan, [_scope("a@x.com")], unattended=False),
                supervisor._plan_statement(plan, [_scope("a@x.com")], unattended=True))
    resumed, scheduled = daemon.run(lines())
    assert "b@x.com" in resumed and "waits for you" in resumed and "not done" not in resumed
    assert "b@x.com" in scheduled and "reported not done" in scheduled and "waits for you" not in scheduled


def test_a_steer_reply_stating_a_scope_is_journaled_as_model_written(daemon: RunningDaemon) -> None:
    run = _running_plan(daemon, [_scope("kept@x.com")], [])
    run.script.sup.append(_decision(run.tree, [_scope("kept@x.com").model_dump(), _scope("new@x.com").model_dump()]))
    chat(daemon, "also post to the new member", session="s1")

    async def origins() -> list[str]:
        return [r[0] for r in daemon.daemon.db.conn.execute(
            "SELECT origin FROM journal WHERE kind='chat.reply' AND data_json LIKE '%new@x.com%'")]
    found = daemon.run(origins())
    assert found and all("inferred" in o.split(",") for o in found)
    _finish(daemon, run)


def test_a_steer_on_an_unattended_run_grants_a_never_granted_scope_only_after_stating_it(daemon: RunningDaemon) -> None:
    """A resumed run whose plan states [a, b] but holds only a's grant: a correction keeping both doesn't grant b at
    once, as if it had been in force."""
    from ferb.core.types import ActionSpec, Target
    from ferb.orchestrator.steering import restate_scope
    run = _running_plan(daemon, [_scope("a@x.com"), _scope("b@x.com")], [])
    supervisor = daemon.daemon.supervisor
    tree = supervisor._tree_of[run.tree]

    def covers(to: str) -> bool:
        return supervisor.standing.covers(run.tree, "probe.post", "owner",
                                          ActionSpec("post", Target(recipients=(to,)), "the update"))

    async def go() -> tuple[bool, bool, bool]:
        supervisor.standing.revoke(run.tree)
        supervisor.standing.grant(run.tree, [_scope("a@x.com")], "schedule:s")
        tree.unattended_scopes = [_scope("a@x.com")]
        restated = restate_scope(supervisor, tree, [_scope("a@x.com"), _scope("b@x.com"), _scope("c@x.com")])
        return covers("a@x.com"), covers("b@x.com"), restated is not None and restated.widened
    assert daemon.run(go()) == (True, False, True)
    _finish(daemon, run)
