"""Owner's Unit 1 (2026-10-01): faster replies within Codex, plain reasons when the model service fails, and FERB's
accurate self-description. FERB's voice is tests/test_voice.py."""

import json
from pathlib import Path
from typing import Any
import pytest

from ferb.core.types import ModelRequest, Task
from ferb.orchestrator.worker import service_problem
from ferb.runtimes.codex_exec import SUPERVISOR_SETTINGS, CodexRuntime, codex_home
from ferb.runtimes.claude_sdk import ClaudeRuntime
from tests.conftest import RunningDaemon, chat, install, sup

CODEX_START = CodexRuntime.start
CLAUDE_START = ClaudeRuntime.start


@pytest.mark.anyio
async def test_codex_start_failure_removes_created_files(tmp_path, monkeypatch):
    import ferb.runtimes.codex_exec as adapter
    from ferb.core.types import Identity
    created = []
    original = adapter.tempfile.TemporaryDirectory
    def temporary(*args, **kwargs):
        kwargs['dir'] = tmp_path
        directory = original(*args, **kwargs)
        created.append(Path(directory.name))
        return directory
    async def version(binary): pass
    async def spawn(*args, **kwargs): raise OSError('cannot launch runtime')
    monkeypatch.setattr(adapter, 'check_version', version)
    monkeypatch.setattr(adapter.tempfile, 'TemporaryDirectory', temporary)
    monkeypatch.setattr(adapter.asyncio, 'create_subprocess_exec', spawn)
    home = tmp_path / 'codex-home'
    home.mkdir()
    (home / 'auth.json').write_text('{}')
    rt = CodexRuntime(Path('/missing'), 'worker', tmp_path,
                      shim_cmd=['ferb', 'mcp'], output_schema={'type':'object'})
    t = Task('t', None, 'g', 'd', Identity.OWNER, set(), 'worker', 'r', [], None)
    with pytest.raises(OSError, match='cannot launch'):
        await CODEX_START(rt, t, ModelRequest('system', ('context',)), 'token')
    assert created and not any(p.exists() for p in created)


@pytest.mark.anyio
async def test_claude_query_failure_disconnects_client(monkeypatch):
    import claude_agent_sdk
    from ferb.runtimes.claude_sdk import ClaudeRuntime
    from ferb.core.types import Identity
    class Client:
        closed = False
        def __init__(self, **kwargs): pass
        async def connect(self): pass
        async def query(self, text): raise RuntimeError('query transport failed')
        async def disconnect(self): Client.closed = True
    monkeypatch.setattr(claude_agent_sdk, 'ClaudeSDKClient', Client)
    rt = ClaudeRuntime('supervisor', None, '/unused')
    monkeypatch.setattr(rt, '_options', lambda *args: None)
    t = Task('t', None, 'g', 'd', Identity.OWNER, set(), 'worker', 'r', [], None)
    with pytest.raises(RuntimeError, match='query transport'):
        await CLAUDE_START(rt, t, ModelRequest('system', ('context',)), 'token')
    assert Client.closed


# ---------- Codex, faster ----------

def test_the_planner_runs_lean_and_tasks_keep_their_settings(tmp_path: Path) -> None:
    from ferb.runtimes.codex_stream import _command
    sup_rt = CodexRuntime(Path("/x/codex"), "supervisor", tmp_path)
    cmd = _command(sup_rt, None)
    joined = " ".join(cmd)
    assert all(s in cmd for s in SUPERVISOR_SETTINGS) and 'model_reasoning_effort="low"' in joined
    worker = CodexRuntime(Path("/x/codex"), "worker", tmp_path, shim_cmd=["ferb", "mcp"])
    wcmd = " ".join(_command(worker, None, tmp_path / "tok"))
    assert "model_reasoning_effort" not in wcmd                                                  # tasks unchanged


def test_ferbs_own_codex_home_once_logged_in(tmp_path: Path) -> None:
    assert codex_home(tmp_path) is None                                     # not logged in yet: default home
    (tmp_path / "codex-home").mkdir()
    (tmp_path / "codex-home" / "auth.json").write_text("{}")
    assert codex_home(tmp_path) == tmp_path / "codex-home"


def test_earlier_turns_keep_full_owner_words_within_context_budget(daemon: RunningDaemon) -> None:
    replies = [sup("reply", f"answer {i}") for i in range(13)]
    script = install(daemon, replies)
    for i in range(12):
        chat(daemon, f"message number {i} " + "x" * 700)
    chat(daemon, "latest")
    ctx = script.contexts[-1][2]
    assert "message number 0" in ctx and "message number 11" in ctx
    assert "x" * 700 in ctx and "Context budget:" in ctx


# ---------- plain reasons ----------

def test_account_startup_failure_reaches_chat_without_starting_a_task(daemon, monkeypatch):
    from ferb.core.errors import RuntimeUnavailable
    from tests.conftest import FakeRuntime

    install(daemon, [])
    starts = []
    message = "Codex account check failed (account_unavailable; RPC -32000). Please try again shortly."

    async def unavailable(self, task, request, token):
        starts.append(task.id)
        raise RuntimeUnavailable(message)

    monkeypatch.setattr(FakeRuntime, 'start', unavailable)
    events = chat(daemon, 'call asmit')
    reply = next(e['text'] for e in events if e['type'] == 'reply')
    assert message in reply
    assert 'Could you say that again' not in reply
    assert len(starts) == 1

    async def evidence():
        failures = [e.data for e in daemon.daemon._journal().entries() if e.kind == 'chat.failed']
        tasks = daemon.daemon.db.conn.execute('SELECT count(*) FROM tasks').fetchone()[0]
        return failures, tasks

    failures, tasks = daemon.run(evidence())
    assert failures[-1]['why'] == 'ServiceError'
    assert failures[-1]['detail'] == message
    assert tasks == 0


def test_a_service_failure_is_recorded_and_told_plainly(daemon: RunningDaemon) -> None:
    assert "You've hit your usage limit. Try again at 9:00" in service_problem("You've hit your usage limit. Try again at 9:00")
    assert "401 Unauthorized: token expired" in service_problem("401 Unauthorized: token expired")
    assert "stream disconnected: network error" in service_problem("stream disconnected: network error")
    script = install(daemon, [])

    async def failing(*a: Any, **k: Any) -> Any:
        from ferb.orchestrator.worker import ServiceError
        raise ServiceError("You've hit your usage limit")
    daemon.daemon.supervisor._run = failing                                # type: ignore[method-assign]
    evs = chat(daemon, "who are you?")
    assert evs[0]["text"].startswith("The AI service I use (Codex) reported an error: You've hit your usage limit")

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
    for prompt in ("supervisor.md",):
        text = daemon.run(system(prompt))
        assert "Installed worker capabilities" in text and "{capabilities}" not in text, prompt   # code's block


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


def test_gpt6_catalog_lockdown_and_namespaced_tools(tmp_path: Path, monkeypatch: Any) -> None:
    """Spike 21 (owner's move to GPT-6): the model catalog switched on code mode (functions.exec), multi-agent v2
    (collaboration.* sub-agents) and experimental tools no flag turns off. FERB hands Codex its own catalog with those
    neutralized for every model; GPT-6's "functions." namespace is read as the plain tool name."""
    import ferb.runtimes.codex_exec as cx
    home = tmp_path / "codex-home"
    home.mkdir()
    (home / "auth.json").write_text("{}")
    (home / "models_cache.json").write_text(json.dumps({"models": [
        {"slug": "gpt-6-sol", "tool_mode": "code_mode_only", "multi_agent_version": "v2",
         "experimental_supported_tools": ["clock"], "context_window": 1},
        {"slug": "gpt-5.5", "tool_mode": None, "context_window": 2}]}))
    out = cx.locked_catalog(tmp_path)
    assert out is not None
    models = json.loads(out.read_text())["models"]
    assert all(m["tool_mode"] is None and m["multi_agent_version"] is None and m["experimental_supported_tools"] == []
               for m in models)
    assert all(m["supports_search_tool"] is False for m in models)           # FERB's MCP tools stay directly visible
    assert {"apps", "plugins", "multi_agent"} <= set(cx.DISABLED)            # side effects of that stay disabled
    assert models[0]["context_window"] == 1                                   # everything else untouched
    from ferb.runtimes.codex_stream import _command
    rt = CodexRuntime(Path("/x/codex"), "supervisor", tmp_path)
    cmd = _command(rt, cx.locked_catalog(tmp_path))
    assert f"model_catalog_json={json.dumps(str(out))}" in cmd
    seen = {n.removeprefix("functions.") for n in ("functions.apply_patch", "functions.exec", "mcp__ferb.fs_read")}
    assert "apply_patch" in seen and "exec" in seen and not ("exec" in cx.BASE_TOOLS)   # exec still a violation
