"""Build step 14a, real (pytest -m real; Codex only): the owner's first session, replayed.
1. "hi, who are you, who am i and what all can you do?" -> FERB introduces itself as FERB, no internal jargon.
2. The storage question -> an answer from FERB; no internal error text anywhere in what chat shows.
3. Repeated work on the same target with FERB asking before every action (careful mode) -> asked once per target
   and action type, and the task finishes after the approval.
4. A task stopped with Ctrl-C, then a new chat: FERB knows what it was doing and that it was stopped.
"""

import json
import shutil
import tempfile
import threading
from pathlib import Path
from typing import Any, Iterator

import pytest

from ferb.cli.chat import render
from ferb.cli.client import Client
from ferb.clock.clock import SystemClock
from tests.conftest import Answers, RunningDaemon, only_tools

pytestmark = pytest.mark.real
INTERNALS = ("supervisor", "subtask", "worker", "only readers", "before i do that", "internal error",
             "not verified", "plan", "needs_user", "[task")


@pytest.fixture
def codex() -> Iterator[RunningDaemon]:
    base = Path(tempfile.mkdtemp(prefix="fux-", dir="/tmp"))
    d = RunningDaemon(base / "home", clock=SystemClock())
    d.daemon.config.set("runtimes", "orchestrator", "codex")
    d.daemon.config.set("runtimes", "worker", "codex")
    yield d
    d.stop()
    shutil.rmtree(base, ignore_errors=True)


def chat(d: RunningDaemon, text: str, session: str = "s1", timeout: float = 900) -> list[dict[str, Any]]:
    c = Client(d.sock, timeout=timeout)
    try:
        return list(c.stream("chat.send", {"session_id": session, "text": text}))
    finally:
        c.close()


def shown(evs: list[dict[str, Any]]) -> str:
    """Exactly what `ferb chat` prints by default."""
    return "\n".join(line for e in evs if (line := render(e)))


def test_1_ferb_introduces_itself_as_ferb(codex: RunningDaemon) -> None:
    only_tools(codex, "fs.", "time.")                                   # locked down (CLAUDE.md)
    out = shown(chat(codex, "hi, who are you, who am i and what all can you do?"))
    low = out.lower()
    assert "ferb" in low and not any(w in low for w in INTERNALS), out


def test_2_the_storage_question_gets_an_answer_not_an_internal_error(codex: RunningDaemon) -> None:
    only_tools(codex, "fs.", "mac.", "shell.")                          # about the Mac and its storage (CLAUDE.md)
    out = shown(chat(codex, "what all apps are there on my mac?? can you analyse how can i manage my mac storage "
                            "better?? it fills too fast adn idk how"))
    low = out.lower()
    assert not any(w in low for w in INTERNALS), out
    assert len(out) > 200, (out, codex.run(_repairs(codex)))             # an actual answer, not a status line


def test_3_one_approval_per_target_and_the_task_finishes(codex: RunningDaemon) -> None:
    only_tools(codex, "fs.")
    codex.daemon.config.set("policy", "autonomy", "careful")             # FERB asks before every action
    codex.run(_reload(codex))
    yes = Answers(codex)
    try:
        evs = chat(codex, "in your workspace create notes.md with the line 'one', then append a line 'two' to it, "
                          "then append a line 'three'")
    finally:
        yes.stop()
    out = shown(evs)
    scopes = [(a.kind, a.scope) for a in yes.asked]
    assert len(scopes) == len(set(scopes)), scopes                        # never asked twice about the same target
    assert (codex.daemon.home / "workspace" / "notes.md").read_text().split() == ["one", "two", "three"]
    assert not any(w in out.lower() for w in INTERNALS), out


async def _repairs(d: RunningDaemon) -> list[Any]:
    return [e.data for e in d.daemon._journal().entries() if e.kind in ("task.repair", "chat.failed")]


async def _reload(d: RunningDaemon) -> None:
    d.daemon._load_config()


def test_4_continuity_after_a_stop_and_a_new_chat(codex: RunningDaemon) -> None:
    only_tools(codex, "fs.")
    started = threading.Event()
    stopped: dict[str, Any] = {}
    c = Client(codex.sock, timeout=900)
    stream = c.stream("chat.send", {"session_id": "first", "text": "research the history of the Delhi metro in "
                                                                     "depth and write it to metro.md"})
    for ev in stream:
        if ev.get("type") == "plan":
            c2 = Client(codex.sock)
            stopped.update(c2.call("task.stop", {"task_ids": [ev["task_id"]]}))
            c2.close()
            started.set()
        if ev.get("type") == "outcome":
            stopped["outcome"] = ev
            break
    c.close()
    assert started.is_set() and stopped["outcome"]["status"] == "cancelled"
    out = shown(chat(codex, "hey, what were you doing earlier? did you finish it?", session="second"))
    low = out.lower()
    assert "metro" in low and ("stop" in low or "didn't finish" in low or "not finish" in low), out
    assert not any(w in low for w in INTERNALS), out
