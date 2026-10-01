"""Owner's Unit 1 (2026-10-01): faster replies within Codex, plain reasons when the model service fails, FERB's
accurate self-description, and FERB's voice (`ferb chat --speak`)."""

import io
import json
from pathlib import Path
from typing import Any

from ferb.cli.chat import Chat
from ferb.core.types import Task
from ferb.orchestrator.worker import service_problem
from ferb.runtimes.codex_exec import SUPERVISOR_SETTINGS, CodexRuntime, codex_home
from ferb.voice.speak import SaySpeaker
from tests.conftest import RunningDaemon
from tests.test_supervisor import chat, install, sup


# ---------- Codex, faster ----------

def test_the_planner_runs_lean_and_tasks_keep_their_settings(tmp_path: Path) -> None:
    sup_rt = CodexRuntime(Path("/x/codex"), "supervisor", tmp_path, tmp_path)
    cmd = sup_rt._cmd(None, "", tmp_path / "instructions.md")
    joined = " ".join(cmd)
    assert all(s in cmd for s in SUPERVISOR_SETTINGS) and 'model_reasoning_effort="low"' in joined
    assert f"model_instructions_file={json.dumps(str(tmp_path / 'instructions.md'))}" in cmd   # Codex's own replaced
    worker = CodexRuntime(Path("/x/codex"), "worker", tmp_path, tmp_path, shim_cmd=["ferb", "mcp"])
    wcmd = " ".join(worker._cmd(None, "tok"))
    assert "model_reasoning_effort" not in wcmd and "model_instructions_file" not in wcmd      # tasks unchanged


def test_ferbs_own_codex_home_once_logged_in(tmp_path: Path) -> None:
    assert codex_home(tmp_path) is None                                     # not logged in yet: default home
    (tmp_path / "codex-home").mkdir()
    (tmp_path / "codex-home" / "auth.json").write_text("{}")
    assert codex_home(tmp_path) == tmp_path / "codex-home"


def test_older_turns_are_summarized_and_recent_ones_kept(daemon: RunningDaemon) -> None:
    replies = [sup("reply", f"answer {i}") for i in range(13)]
    script = install(daemon, replies)
    for i in range(12):
        chat(daemon, f"message number {i} " + "x" * 700)
    chat(daemon, "latest")
    ctx = script.contexts[-1][2]
    assert "Earlier (summarized):" in ctx and "message number 0" in ctx     # the day's start, one short line
    assert ctx.count("\n[c") == 8                                           # the last 8 verbatim
    assert "x" * 401 not in ctx                                             # each kept to 400 characters


# ---------- plain reasons ----------

def test_a_service_failure_is_recorded_and_told_plainly(daemon: RunningDaemon) -> None:
    assert "usage limit is reached" in service_problem("You've hit your usage limit. Try again at 9:00")
    assert "isn't signed in" in service_problem("401 Unauthorized: token expired")
    assert "connection failed" in service_problem("stream disconnected: network error")
    script = install(daemon, [])

    async def failing(*a: Any, **k: Any) -> Any:
        from ferb.orchestrator.worker import ServiceError
        raise ServiceError("You've hit your usage limit")
    daemon.daemon.supervisor._run = failing                                # type: ignore[method-assign]
    evs = chat(daemon, "who are you?")
    assert evs[0]["text"].startswith("The AI service I use (Codex) says your usage limit is reached")

    async def failures() -> list[Any]:
        return [e.data for e in daemon.daemon._journal().entries() if e.kind == "chat.failed"]
    rec = daemon.run(failures())
    assert rec[-1]["why"] == "ServiceError" and "usage limit" in rec[-1]["detail"]
    assert script is not None


# ---------- who FERB is ----------

def test_planner_and_answer_writer_know_what_ferb_can_do(daemon: RunningDaemon) -> None:
    s = daemon.daemon.supervisor

    async def system(prompt: str) -> str:
        return str(s._system(prompt))                                       # type: ignore[union-attr]
    for prompt in ("supervisor.md", "answer.md"):
        text = daemon.run(system(prompt))
        assert "What workers can do right now" in text and "Who FERB is" in text, prompt
        assert "Never present the limits of one task" in text and "{capabilities}" not in text


# ---------- FERB's voice ----------

class FakeSay:
    def __init__(self) -> None:
        self.said: list[str] = []
        self.stopped = 0

    def say(self, text: str) -> None:
        self.said.append(text)

    def stop(self) -> None:
        self.stopped += 1


class Stream:
    def __init__(self, events: list[Any]) -> None:
        self.events = events

    def stream(self, method: str, params: Any) -> Any:
        yield from self.events

    def close(self) -> None:
        pass


def test_speak_says_ferbs_words_not_internals() -> None:
    voice = FakeSay()
    events = [{"type": "plan", "text": "Looking now.", "task_id": "t1"},
              {"type": "event", "topic": "tool.called", "data": {"tool": "fs.read"}},       # not shown, not said
              {"type": "event", "topic": "action.held", "data": {"kind": "call", "to": "Rahul", "seconds": 10}},
              {"type": "outcome", "status": "done", "summary": "ok", "answer": "Done: 3 files.", "verified": True}]
    c = Chat(lambda: Stream(events), io.StringIO(), lambda q: "", verbose=False, speaker=voice)  # type: ignore[arg-type,return-value]
    c.turn("look")
    assert voice.said == ["ferb> Looking now.", "   📞 Calling Rahul in 10 s… (Ctrl-C to cancel)", "ferb> Done: 3 files."]
    assert voice.stopped >= 1                                               # a new message cut off the last one


def test_say_runs_the_macs_voice_and_can_be_cut_off() -> None:
    runs: list[list[str]] = []

    class P:
        def __init__(self, cmd: list[str], **k: Any) -> None:
            runs.append(cmd)
            self.terminated = False

        def poll(self) -> None:
            return None

        def terminate(self) -> None:
            runs.append(["terminated"])
    s = SaySpeaker(run=P)                                                   # type: ignore[arg-type]
    s.say("ferb> Hello there.")
    s.say("ferb> Second thing.")
    assert runs == [["say", "Hello there."], ["terminated"], ["say", "Second thing."]]
