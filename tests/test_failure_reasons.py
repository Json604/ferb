"""A failure FERB can't get past is told by its reason, never a vague "something went wrong on my side" (owner,
2026-10-24: "make sure that if ferb cannot do anything because of internal or internal permission restriction then
that is stated, not just this vague message"). The live case: a turn ended with "Sorry — something went wrong on my
side. Could you try that again?" while the journal held `PermissionError: [Errno 1] Operation not permitted:
'/private/var/folders/…/com.apple.LaunchServices.dv'`."""

import asyncio
import json
from typing import Any

import pytest
from pydantic import BaseModel, ValidationError

from ferb.core.errors import InputTooLarge, PlanRejected, Refused
from ferb.core.structured import InvalidAnswer
from ferb.orchestrator.plans import Plan, Step
from ferb.orchestrator.worker import (RuntimeViolation, ServiceError, couldnt, problem_of, reason,
                                      too_large_problem)
from ferb.sanitize.redact import Redactor
from tests.conftest import RunningDaemon, chat, install, outcome, sup, task

LOCKED = "/private/var/folders/x/y"
SECRET = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789ABCD"
BUG = "That's a bug in FERB, not something you asked wrong."


def _journal(daemon: RunningDaemon, kind: str) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        return [e.data for e in daemon.daemon._journal().entries() if e.kind == kind]
    return daemon.run(go())


# ---------- one reason per failure class ----------

def test_a_macos_permission_refusal_names_the_path_and_macos_words() -> None:
    assert (reason(PermissionError(1, "Operation not permitted", LOCKED))
            == f"macOS didn't let FERB open {LOCKED} (Operation not permitted).")
    assert reason(PermissionError(13, "Permission denied")) == "macOS refused an operation FERB needed (Permission denied)."


def test_a_refusal_by_ferbs_own_rules_says_so() -> None:
    assert (reason(Refused("~/.ssh is on FERB's never-read list"))
            == "FERB's protection rules stopped it: ~/.ssh is on FERB's never-read list.")


def test_the_ai_service_wording_is_unchanged() -> None:
    e = ServiceError("You've hit your usage limit")
    assert reason(e) == problem_of(e) and couldnt("start that", e) == problem_of(e)
    big = InputTooLarge(5000, 2000)
    assert reason(big) == too_large_problem(big) and couldnt("start that", big) == too_large_problem(big)


def test_a_runtime_that_broke_ferbs_rules_says_which() -> None:
    said = reason(RuntimeViolation("codex: {'unexpected_tools': ['shell']}", "codex", {"unexpected_tools": ["shell"]}))
    assert said == "the AI runtime (codex) broke one of FERB's rules: it used tools FERB didn't offer it: shell."
    bare = reason(RuntimeViolation("codex: {'stream': 'closed'}"))       # no fact FERB knows: its own message
    assert bare == "the AI runtime broke one of FERB's rules: codex: {'stream': 'closed'}."


def test_a_plan_still_invalid_after_repair_names_its_problems() -> None:
    said = reason(PlanRejected(['Inventory paths are protected: ["/Users/k/.ssh"]']))
    assert said == 'the plan still had problems after one repair: Inventory paths are protected: ["/Users/k/.ssh"].'


def test_an_answer_outside_ferbs_form_says_what_was_wrong() -> None:
    class Answer(BaseModel):
        status: str
    with pytest.raises(ValidationError) as caught:
        Answer.model_validate({})
    assert reason(caught.value) == "the AI's answer didn't fit the form FERB requires (Field required at status)."
    assert (reason(InvalidAnswer("the answer is not JSON (Expecting value)"))
            == "the AI's answer didn't fit the form FERB requires (the answer is not JSON (Expecting value)).")


def test_any_other_exception_is_named_as_a_bug_in_ferb() -> None:
    assert reason(KeyError("worker_type")) == f"something broke inside FERB (KeyError: 'worker_type'). {BUG}"
    assert reason(RuntimeError()) == f"something broke inside FERB (RuntimeError). {BUG}"


def test_a_long_message_is_capped() -> None:
    said = reason(RuntimeError("x" * 2000))
    assert len(said) < 420 and "…" in said and said.endswith(BUG)


def test_secrets_are_redacted_before_the_cap() -> None:
    whole = reason(RuntimeError(f"token {SECRET} rejected"), Redactor())
    assert SECRET not in whole and "[redacted]" in whole
    # a secret straddling the cap is redacted whole first, never cut into a piece the redactor no longer knows
    edge = reason(RuntimeError("x" * 280 + " " + SECRET), Redactor())
    assert "sk-ant" not in edge
    path = reason(PermissionError(1, "Operation not permitted", f"/tmp/{SECRET}/a"), Redactor())
    assert SECRET not in path and "Operation not permitted" in path


def test_couldnt_says_what_did_not_happen() -> None:
    assert (couldnt("start that", PermissionError(1, "Operation not permitted", LOCKED))
            == f"I couldn't start that: macOS didn't let FERB open {LOCKED} (Operation not permitted).")


# ---------- the owner's chat turn ----------

def test_a_macos_refusal_while_planning_is_told_with_the_path(daemon: RunningDaemon,
                                                               monkeypatch: pytest.MonkeyPatch) -> None:
    """The live case: the plan's file count walked a folder macOS won't let FERB look at."""
    async def walk(paths: list[str], denied: Any = None) -> list[dict[str, Any]]:
        raise PermissionError(1, "Operation not permitted", LOCKED)
    monkeypatch.setattr("ferb.orchestrator.supervisor.inventory", walk)
    cleaning = task("clean my temp folders")
    cleaning["plan"] = Plan(steps=[Step(id="clean", kind="do", goal="Clean the temp folders")],
                            input_paths=["/private/var/folders/x"]).model_dump()
    install(daemon, [sup("task", "Cleaning.", cleaning)])
    replies = [e for e in chat(daemon, "ok start cleaning") if e["type"] == "reply"]
    said = f"I couldn't start that: macOS didn't let FERB open {LOCKED} (Operation not permitted)."
    assert [e["text"] for e in replies] == [said]
    failed = _journal(daemon, "chat.failed")
    assert failed and failed[-1]["why"] == "PermissionError" and LOCKED in failed[-1]["detail"]
    assert said in [r["text"] for r in _journal(daemon, "chat.reply")]       # the next turn's history has it (L21)


def test_a_failure_before_any_task_started_says_start(daemon: RunningDaemon) -> None:
    async def environment(words: str) -> dict[str, Any]:
        raise PermissionError(1, "Operation not permitted", LOCKED)
    daemon.daemon.supervisor.environment = environment                  # type: ignore[union-attr]
    install(daemon, [sup("reply", "never used")])
    replies = [e["text"] for e in chat(daemon, "what's in my temp folder?") if e["type"] == "reply"]
    assert replies == [f"I couldn't start that: macOS didn't let FERB open {LOCKED} (Operation not permitted)."]
    assert not any("went wrong" in text for text in replies)
    assert _journal(daemon, "chat.failed")[-1]["why"] == f"PermissionError: [Errno 1] Operation not permitted: '{LOCKED}'"


def test_a_failure_after_a_task_started_says_finish(daemon: RunningDaemon) -> None:
    async def send(session_id: str, text: str, **kwargs: Any) -> Any:
        yield {"type": "plan", "text": "On it.", "task_id": "t-started", "goal": "x", "identity": "ferb",
               "subtasks": []}
        raise KeyError("worker_type")
    daemon.daemon.supervisor.send = send                                # type: ignore[union-attr]
    replies = [e["text"] for e in chat(daemon, "do it") if e["type"] == "reply"]
    assert replies == [f"I couldn't finish that: something broke inside FERB (KeyError: 'worker_type'). {BUG}"]
    assert _journal(daemon, "chat.failed")[-1]["why"] == "KeyError: 'worker_type'"


def test_a_plan_still_invalid_after_one_repair_tells_the_remaining_problem(daemon: RunningDaemon) -> None:
    stop = json.loads(sup("reply", "Stopping it."))
    stop["control"] = {"task_id": "no-such-task", "action": "stop", "standing_sends": "unchanged", "supersedes": []}
    install(daemon, [json.dumps(stop), json.dumps(stop)])
    replies = [e["text"] for e in chat(daemon, "stop that") if e["type"] == "reply"]
    assert replies == ["I couldn't start that: the plan still had problems after one repair: Unknown task_id; no "
                       "matching attempt exists in this conversation."]
    failed = _journal(daemon, "chat.failed")
    assert [f["why"] for f in failed] == ["task still invalid after repair", "PlanRejected"]


def test_protected_inventory_paths_are_told_after_repair(daemon: RunningDaemon) -> None:
    from ferb.act.fs.files import normalize_path
    state = str(daemon.daemon.home / "ferb.db")                         # FERB's own state: closed to its tools
    reading = task("summarize the database")
    reading["plan"] = Plan(steps=[Step(id="read", kind="do", goal="Summarize it")], input_paths=[state]).model_dump()
    install(daemon, [sup("task", "Reading.", reading), sup("task", "Reading.", reading)])
    replies = [e["text"] for e in chat(daemon, "summarize ferb.db") if e["type"] == "reply"]
    closed = [normalize_path(state, daemon.daemon.supervisor.workspace)]   # type: ignore[union-attr]
    assert replies == ["I couldn't start that: the plan still had problems after one repair: Inventory paths are "
                       f"protected: {json.dumps(closed)}."]


# ---------- side outputs code refused ----------

def _in_minutes(n: int) -> dict[str, Any]:
    return {"in_minutes": n, "in_days": None, "day": None, "date": None, "time": None}


def test_a_refused_reminder_says_why(daemon: RunningDaemon) -> None:
    install(daemon, [json.dumps({"kind": "reply", "text": "Okay.", "remember": [], "timers": [
        {"when": _in_minutes(10), "message": "boarding starts", "quote": "words the owner never said"},
        {"when": _in_minutes(-5), "message": "stretch", "quote": "remind me to stretch"}]})])
    reply = next(e for e in chat(daemon, "remind me to stretch") if e["type"] == "reply")
    line = reply["text"].split("\n")[-1]
    assert line.startswith("I couldn't set the reminders “boarding starts” (the words I quoted from you aren't in any "
                           "message you typed in this chat), “stretch” (")
    assert line.endswith("). Nothing is scheduled for them. Could you ask again?") and "on my side" not in line


def test_a_refused_memory_says_why(daemon: RunningDaemon) -> None:
    install(daemon, [json.dumps({"kind": "reply", "text": "Okay.", "timers": [], "remember": [
        {"kind": "person", "subject": "owner", "key": "dentist", "content": "Your dentist is Dr. Rao",
         "quote": "words the owner never said"}]})])
    reply = next(e for e in chat(daemon, "when is my dentist appointment?") if e["type"] == "reply")
    assert reply["text"].split("\n")[-1] == ("I couldn't remember “Your dentist is Dr. Rao”: the words I quoted from "
                                             "you aren't in what you typed. It is not saved. Could you tell me again?")


# ---------- a task's own attempts ----------

def test_an_attempt_that_raises_reports_the_reason(daemon: RunningDaemon) -> None:
    script = install(daemon, [sup("task", "On it.", task("do it"))],
                     {"worker.dispatch": json.dumps({"action": "finish", "worker_type": "default",
                                                     "instructions": "Stop and report.", "question": "",
                                                     "stop_reason": ""})})
    scripted = daemon.daemon.supervisor.make_runtime

    def runtimes(role: str, **kwargs: Any) -> Any:
        if role.startswith("worker:"):
            raise KeyError("runtime table")
        return scripted(role, **kwargs)
    daemon.daemon.supervisor.make_runtime = runtimes                     # type: ignore[union-attr]
    out = outcome(chat(daemon, "do it"))
    said = f"This attempt couldn't run: something broke inside FERB (KeyError: 'runtime table'). {BUG}"
    assert "Why work stopped: " + said in out["answer"], out
    dispatch = next(c for _, key, c, _ in script.contexts if key == "worker.dispatch")
    assert "something broke inside FERB (KeyError: 'runtime table')" in dispatch   # the orchestrator reads it too
    assert "a problem on my side" not in out["answer"]


# ---------- voice ----------

def test_voice_speaks_the_reason() -> None:
    from ferb.voice.controller import Voice

    class Speaker:
        def __init__(self) -> None:
            self.said: list[str] = []

        def say(self, words: str) -> None:
            self.said.append(words)

        def wait(self) -> None:
            return None

    class Journal:
        def __init__(self) -> None:
            self.rows: list[tuple[str, dict[str, Any]]] = []

        def append(self, kind: str, data: dict[str, Any]) -> None:
            self.rows.append((kind, data))

    async def run_turn(text: str) -> Any:
        raise PermissionError(1, "Operation not permitted", LOCKED)
        yield {}                                                        # an async generator, as the daemon's is

    speaker, journal = Speaker(), Journal()
    voice = Voice(run_turn, lambda: [], speaker, journal, lambda: None, redact=Redactor())
    asyncio.run(voice._turn("clean my temp folders"))
    assert speaker.said == [f"I couldn't do that: macOS didn't let FERB open {LOCKED} (Operation not permitted)."]
    assert journal.rows and journal.rows[0][0] == "voice.failed"
