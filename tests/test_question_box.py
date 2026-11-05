"""How the owner answers a question card (batch 3b, owner 2026-11-05; review §0.1-§0.9): FERB's own check-ins take
only their buttons (ORCH-536); a task's question takes its buttons or the owner's words (ORCH-536, ORCH-537); a safety
check of a task takes Yes / No, or words for that task, which are a correction and never a Yes (GATE-510). A correction
sets aside its task's open safety checks after it reached the worker (ORCH-538)."""
import asyncio
import json
import threading
import time
from types import SimpleNamespace
from typing import Any

import pytest

from ferb.cli.client import Client
from ferb.core.types import Identity, RunHandle
from ferb.daemon.gateway import CONFIRM_ASK
from ferb.orchestrator.plans import Plan, Step
from ferb.policy.rules import ActionRequest
from ferb.sanitize.redact import REDACTED
from tests.conftest import (Answers, FakeCap, FakeRuntime, RunningDaemon, chat, done, gateway_request, gateway_setup,
                            gateway_tool_call, install, sup, task, tool)

FAKE_GITHUB = "ghp_" + "Fake0Token0For0Tests0Only0abcdefgh12"     # invented: GitHub's format, no real token


def _until(check: Any, timeout: float = 10.0) -> None:
    end = time.monotonic() + timeout
    while not check():
        assert time.monotonic() < end, "timed out"
        time.sleep(0.02)


def _pending(d: RunningDaemon, capability: str) -> list[Any]:
    return [ap for ap in d.daemon.state.approvals.pending() if ap.capability == capability]


def _call(d: RunningDaemon, method: str, params: dict[str, Any]) -> Any:
    c = Client(d.sock)
    try:
        return c.call(method, params)
    finally:
        c.close()


def _journal(d: RunningDaemon, kind: str) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        return [e.data for e in d.daemon.journal.entries() if e.kind == kind]
    return d.run(go())  # type: ignore[no-any-return]


def _confirmation_rows(d: RunningDaemon) -> list[Any]:
    async def go() -> list[Any]:
        return d.daemon.db.conn.execute("SELECT * FROM confirmations").fetchall()
    return d.run(go())  # type: ignore[no-any-return]


def _asking_task(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch, *, takes_steering: bool) -> Any:
    """A running one-step plan whose step posts to an address nobody named (FERB asks first), then, once released,
    tries the same post again. `takes_steering`: the worker's runtime takes a live correction (each delivery records
    the words and the open confirmations' states at that moment); else it can't, as the scripted runtime can't."""
    cap = FakeCap("probe.post", "post", {"probe"})
    daemon.daemon.registry.capabilities[cap.name] = cap
    results: list[dict[str, Any]] = []
    proceed = threading.Event()
    delivered: list[tuple[str, list[str]]] = []
    if takes_steering:
        async def continue_(self: Any, run: RunHandle, message: str) -> None:
            delivered.append((message, [ap.state for ap in daemon.daemon.state.approvals.of_request(
                daemon.daemon.supervisor._tree_of[run.task_id].worker.request_id) if ap.capability == CONFIRM_ASK]))
        monkeypatch.setattr(FakeRuntime, "continue_", continue_)

    def step(token: str) -> str:
        results.append(tool(daemon.sock, token, "probe.post", to="new@x.com", text="the update"))
        proceed.wait(15)
        results.append(tool(daemon.sock, token, "probe.post", to="new@x.com", text="the update"))
        return done("Posted.")
    spec = task("Post the update", "owner")
    spec["plan"] = Plan(steps=[Step(id="post", kind="do", goal="Post the update")]).model_dump()
    script = install(daemon, [sup("task", "Posting.", spec)],
                     {"step.post": [step, done("Posted.")], "worker": done("Checked.")})
    turn = threading.Thread(target=lambda: chat(daemon, "post the update", session="s1"))
    turn.start()
    _until(lambda: bool(_pending(daemon, CONFIRM_ASK)))
    check = _pending(daemon, CONFIRM_ASK)[0]
    tree = daemon.daemon.tasks.tree_of(check.task_id)
    return SimpleNamespace(cap=cap, results=results, proceed=proceed, delivered=delivered, turn=turn, check=check,
                           tree=tree, script=script)


def _end(daemon: RunningDaemon, run: Any) -> None:
    daemon.call_soon(daemon.daemon.supervisor.stop_tree, run.tree)
    run.proceed.set()
    run.turn.join(20)
    assert not run.turn.is_alive()


# ---------- a safety check's box: a correction of its task, never a Yes ----------

# decision: GATE-510, ORCH-538, DMN-543
def test_words_in_a_safety_checks_box_correct_its_task_and_the_same_action_asks_again(
        daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    run = _asking_task(daemon, monkeypatch, takes_steering=True)
    assert run.check.corrects and not run.check.options_only
    told = _call(daemon, "chat.correct", {"approval_id": run.check.id, "text": "yes but send it to the team list"})
    assert told["state"] == "superseded" and told["task_id"] == run.tree
    assert run.check.state == "superseded" and run.check.choice is None and run.check.answer is None
    assert _confirmation_rows(daemon) == [] and run.cap.calls == 0          # neither Yes nor No; nothing ran
    _until(lambda: len(run.results) == 1)
    first = json.dumps(run.results[0]["content"])
    assert run.results[0]["is_error"] and "changed the task" in first
    assert "team list" not in first                                         # the words never ride in a tool result
    # delivered to that task's worker before its check was set aside (review §0.6)
    assert len(run.delivered) == 1 and "send it to the team list" in run.delivered[0][0]
    assert run.delivered[0][1] == ["pending"]
    steered = _journal(daemon, "task.steered")
    assert [s["task_id"] for s in steered] == [run.tree]
    said = [m for m in _journal(daemon, "chat.message") if m.get("correction_of") == run.check.id]
    assert len(said) == 1 and "team list" in said[0]["text"]               # in the task's chat as the owner's words
    owner = Answers(daemon, granted=True, choice=1)
    run.proceed.set()
    _until(lambda: len(run.results) == 2)
    owner.stop()
    asked = [ap for ap in owner.asked if ap.capability == CONFIRM_ASK]
    assert asked and asked[0].id != run.check.id                            # the same action asked again
    assert not run.results[1]["is_error"] and run.cap.calls == 1
    run.turn.join(20)


# decision: ORCH-538
def test_no_new_question_opens_before_the_correction_reaches_the_worker(
        daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    run = _asking_task(daemon, monkeypatch, takes_steering=False)            # this runtime can't take it live
    _call(daemon, "chat.correct", {"approval_id": run.check.id, "text": "use the cheaper one"})
    assert run.check.state == "superseded"
    run.proceed.set()
    _until(lambda: len(run.results) == 2)
    again = run.results[1]
    assert again["is_error"] and "correction" in json.dumps(again["content"]) and run.cap.calls == 0
    assert _pending(daemon, CONFIRM_ASK) == []                              # no identical card before the words
    _end(daemon, run)


# decision: GATE-510
def test_the_box_masks_a_pasted_secret_and_a_password_worded_check_never_hides_the_correction(
        daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    run = _asking_task(daemon, monkeypatch, takes_steering=True)
    run.check.reason = run.check.summary = "FERB: type your password into the sign-in page?"   # review §0.9
    told = _call(daemon, "chat.correct", {"approval_id": run.check.id,
                                          "text": f"use the work account instead, my token is {FAKE_GITHUB}"})
    assert told["hidden"] == ["GitHub token"]
    assert "use the work account instead" in told["message"] and REDACTED in told["message"]
    assert FAKE_GITHUB not in json.dumps(told)
    assert "use the work account instead" in run.delivered[0][0] and FAKE_GITHUB not in run.delivered[0][0]
    hidden = _journal(daemon, "chat.secret_hidden")
    assert [h["kinds"] for h in hidden] == [["GitHub token"]]
    assert not any(FAKE_GITHUB in json.dumps(e) for e in _journal(daemon, "chat.message"))
    _end(daemon, run)


# decision: GATE-510
def test_an_exact_option_typed_in_the_box_counts_as_that_option(daemon: RunningDaemon) -> None:
    cap = FakeCap("probe.pay", "pay", {"probe"})
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("pay x@y.com $5"), [cap])
    call = asyncio.run_coroutine_threadsafe(
        daemon.daemon.gateway.call(grant, cap.name, {"to": "x@y.com", "text": "pay"}), daemon.loop)
    _until(lambda: bool(_pending(daemon, CONFIRM_ASK)))
    pay = _pending(daemon, CONFIRM_ASK)[0]
    _call(daemon, "chat.correct", {"approval_id": pay.id, "text": "yes"})   # the owner's own keystrokes
    assert pay.choice == "Yes"
    assert not call.result(timeout=5)["is_error"] and cap.calls == 1


# decision: GATE-510
def test_typed_words_never_answer_a_safety_check(daemon: RunningDaemon) -> None:
    """Review §0.3 and item 8: words on a safety check through `chat.answer` (an older app) were dropped as "neither Yes
    nor No"; they are refused with "Pick ...", and the box is refused for checks outside a task and closed ones."""
    cap = FakeCap("probe.pay", "pay", {"probe"})
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("pay x@y.com $5"), [cap])
    call = asyncio.run_coroutine_threadsafe(
        daemon.daemon.gateway.call(grant, cap.name, {"to": "x@y.com", "text": "pay"}), daemon.loop)
    _until(lambda: bool(_pending(daemon, CONFIRM_ASK)))
    pay = _pending(daemon, CONFIRM_ASK)[0]
    with pytest.raises(Exception, match="Pick Yes or No"):
        _call(daemon, "chat.answer", {"approval_id": pay.id, "text": "sure, go ahead"})
    assert pay.state == "pending" and cap.calls == 0

    async def forget() -> Any:
        action = ActionRequest("memory.forget", "owner_question", Identity.FERB, frozenset(), "", "", "memory",
                               frozenset())
        return daemon.daemon.state.approvals.question(action, "Forget that you like tea?", ["Yes", "No"])
    no_task = daemon.run(forget())
    with pytest.raises(Exception, match="only Yes or No"):
        _call(daemon, "chat.correct", {"approval_id": no_task.id, "text": "forget the coffee one instead"})
    assert no_task.state == "pending"
    _call(daemon, "chat.approve", {"approval_id": pay.id, "granted": True, "choice": 2})
    with pytest.raises(Exception, match="isn't waiting"):
        _call(daemon, "chat.correct", {"approval_id": pay.id, "text": "pay $4 instead"})
    assert call.result(timeout=5)["is_error"] and cap.calls == 0


# decision: GATE-512, GATE-510, ORCH-536, ORCH-537
def test_an_options_number_never_answers_ferbs_own_check(daemon: RunningDaemon,
                                                          monkeypatch: pytest.MonkeyPatch) -> None:
    """Security run T-1058: a "1" typed on a payment's card (chat.answer) or in its box (chat.correct) was read as
    option 1, Yes, and the payment ran. Only an option's own words answer FERB's checks; a number is words."""
    from ferb.daemon.questions import OWN_CHECKS
    from ferb.orchestrator.steering import FERB_CHECKS
    assert OWN_CHECKS == FERB_CHECKS
    cap = FakeCap("probe.pay", "pay", {"probe"})
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("pay x@y.com $5"), [cap])
    call = asyncio.run_coroutine_threadsafe(
        daemon.daemon.gateway.call(grant, cap.name, {"to": "x@y.com", "text": "pay"}), daemon.loop)
    _until(lambda: bool(_pending(daemon, CONFIRM_ASK)))
    pay = _pending(daemon, CONFIRM_ASK)[0]
    with pytest.raises(Exception, match="Pick Yes or No"):
        _call(daemon, "chat.answer", {"approval_id": pay.id, "text": "1"})
    assert pay.state == "pending" and pay.choice is None and cap.calls == 0
    _call(daemon, "chat.approve", {"approval_id": pay.id, "granted": True, "choice": 2})
    assert call.result(timeout=5)["is_error"] and cap.calls == 0
    run = _asking_task(daemon, monkeypatch, takes_steering=True)          # the box of a task's safety check
    told = _call(daemon, "chat.correct", {"approval_id": run.check.id, "text": "1"})
    assert told["state"] == "superseded" and run.check.choice is None and run.cap.calls == 0
    assert _confirmation_rows(daemon) == [] or all(not r["granted"] for r in _confirmation_rows(daemon))
    _end(daemon, run)


# decision: GATE-510, ORCH-538
def test_a_yes_tapped_after_the_box_set_the_check_aside_approves_nothing(
        daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    """Security run T-1058, the answer-and-correct race: the correction typed in a safety check's box sets the check
    aside; a Yes that arrives after it (another surface, a late tap) is refused, records nothing and runs nothing. The
    action asks again with a new card (the other order, a Yes before a chat correction, is pinned above)."""
    run = _asking_task(daemon, monkeypatch, takes_steering=True)
    _call(daemon, "chat.correct", {"approval_id": run.check.id, "text": "post it in the team channel instead"})
    assert run.check.state == "superseded"
    with pytest.raises(Exception):
        _call(daemon, "chat.approve", {"approval_id": run.check.id, "granted": True, "choice": 1})
    assert run.check.state == "superseded" and run.check.choice is None
    _until(lambda: len(run.results) == 1)
    assert run.results[0]["is_error"] and run.cap.calls == 0 and _confirmation_rows(daemon) == []
    _end(daemon, run)


# ---------- a task's question: its buttons or the owner's words ----------

# decision: ORCH-536
def test_words_typed_for_a_task_question_with_options_reach_the_worker_as_the_owners(daemon: RunningDaemon) -> None:
    async def route(task_id: str, question: str, options: list[str]) -> dict[str, Any]:
        return {"ask_owner": True, "answer": "", "question": question}
    daemon.daemon.gateway.route_question = route
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("send the notes"), [])
    result: list[Any] = []
    asking = threading.Thread(target=lambda: result.append(gateway_tool_call(
        daemon, grant, "owner.ask", question="Which list?", options=["Team", "Family"])))
    asking.start()
    _until(lambda: bool(_pending(daemon, "owner.ask")))
    asked = _pending(daemon, "owner.ask")[0]
    listed = next(a for a in _call(daemon, "chat.status", {})["approvals"] if a["approval_id"] == asked.id)
    assert listed.get("from_task") and not listed.get("options_only") and not listed.get("corrects")
    _call(daemon, "chat.answer", {"approval_id": asked.id, "text": "both, but Team first"})
    asking.join(10)
    assert result[0]["content"]["answer"] == "both, but Team first"


def _task_question(d: RunningDaemon, question: str, options: tuple[str, ...]) -> Any:
    async def make() -> Any:
        action = ActionRequest("owner.ask", "owner_question", Identity.FERB, frozenset(), "other.worker",
                               "other.worker", "req-other", frozenset())
        ap = d.daemon.state.approvals.question(action, question, list(options))
        ap.from_task = True
        return ap
    return d.run(make())


def _routed(approval_id: str, answer: str) -> str:
    return json.dumps({"kind": "reply", "text": "", "task": None,
                       "answer_question": {"approval_id": approval_id, "answer": answer}})


# decision: ORCH-537
def test_a_chat_reply_to_a_task_question_goes_as_words_and_asks_which_only_for_two(daemon: RunningDaemon) -> None:
    lists = _task_question(daemon, "Which list?", ("Team", "Family"))
    install(daemon, [_routed(lists.id, "Team")])
    events = chat(daemon, "both lists, Team first", session="s")
    assert [e["type"] for e in events] == ["reply"]
    assert lists.state == "granted" and lists.choice is None and lists.answer == "both lists, Team first"
    first = _task_question(daemon, "Which list?", ("Team", "Family"))
    second = _task_question(daemon, "Which folder?", ())
    install(daemon, [_routed(first.id, "Team")])
    events = chat(daemon, "the shared one", session="s")
    assert [e["type"] for e in events] == ["clarify"]                       # one "which one?"
    assert first.reason in events[0]["text"] and second.reason in events[0]["text"]
    assert first.state == "pending" and second.state == "pending"


# decision: GATE-510, ORCH-538
def test_a_yes_tapped_while_a_correction_is_planned_runs_the_action_once(
        daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    """Pinned as built: the owner taps Yes while the planner is still reading a correction in chat. The Yes stands
    (the action runs once), the correction reaches the task afterwards and sets nothing aside, and code adds no line
    claiming the check was changed."""
    run = _asking_task(daemon, monkeypatch, takes_steering=True)
    planning, release = threading.Event(), threading.Event()

    def planner(token: str) -> str:
        planning.set()
        release.wait(10)
        decision = json.loads(sup("task", "Noted.", task("Post the update", "owner")))
        decision["control"] = {"task_id": run.tree, "action": "continue", "standing_sends": "unchanged",
                               "supersedes": []}
        return json.dumps(decision)
    holder: list[list[dict[str, Any]]] = []
    run.script.sup.append(planner)
    turn = threading.Thread(target=lambda: holder.append(chat(daemon, "actually make it shorter", session="s1")))
    turn.start()
    assert planning.wait(10)
    _call(daemon, "chat.approve", {"approval_id": run.check.id, "granted": True, "choice": 1})
    _until(lambda: len(run.results) == 1)
    release.set()
    turn.join(20)
    assert not run.results[0]["is_error"] and run.cap.calls == 1 and run.check.choice == "Yes"
    reply = [e for e in holder[0] if e["type"] == "reply"][-1]["text"]
    assert reply.startswith("Noted.") and "set aside" not in reply
    assert len(run.delivered) == 1 and run.delivered[0][1] == ["granted"]  # nothing left to set aside
    _end(daemon, run)


# decision: ORCH-536
def test_a_runtime_offer_takes_only_its_buttons(daemon: RunningDaemon) -> None:
    """Review §0.7: typed words on "Use Claude instead?" failed the task. The offer is buttons only (ORCH-315)."""
    from ferb.core.types import RuntimeEvent
    from ferb.orchestrator.supervisor import RUNTIME_ASK
    daemon.daemon.supervisor.worker_types = lambda: ["default", "claude"]    # type: ignore[method-assign]
    limit = RuntimeEvent("error", {"message": "You've hit your usage limit.", "account": "usage_limit"})
    install(daemon, [sup("task", "On it.", task("Tidy the desktop"))], {"worker": [limit, done("Tidied.")]})
    turn = threading.Thread(target=lambda: chat(daemon, "tidy my desktop"))
    turn.start()
    _until(lambda: bool(_pending(daemon, RUNTIME_ASK)))
    offer = _pending(daemon, RUNTIME_ASK)[0]
    assert offer.options_only
    with pytest.raises(Exception, match="Pick Use Claude or Stop"):
        _call(daemon, "chat.answer", {"approval_id": offer.id, "text": "sure, whatever works"})
    assert offer.state == "pending"
    _call(daemon, "chat.approve", {"approval_id": offer.id, "granted": True, "choice": 1})
    turn.join(20)
    assert not turn.is_alive()


# decision: ORCH-539
def test_the_planners_open_questions_name_their_task(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    run = _asking_task(daemon, monkeypatch, takes_steering=False)
    other = _task_question(daemon, "Which folder?", ())                    # outside any running task
    run.script.sup.append(sup("reply", "Noted."))
    chat(daemon, "how is it going?", session="s1")
    block = run.script.contexts[-1][2].split("Layer 6 — open questions", 1)[1]
    assert f'"approval_id": "{run.check.id}", "task": "Post the update"' in block
    assert f'"approval_id": "{other.id}", "question"' in block              # no task label
    _end(daemon, run)
