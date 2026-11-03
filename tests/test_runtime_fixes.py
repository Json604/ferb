"""Step 4 runtime fixes (kb area runtimes): Codex timeouts, MCP timeouts, start-once checks, violations that
say what happened, stderr always drained, worker instructions; the Claude token file, sign-in, cost across steered turns
and sign-out; the direct-API time limit; worker types the planner can name. Offline: no Codex, Claude or model process
is started."""
import asyncio
import json
import os
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

import ferb.runtimes.codex_exec as cx
from ferb.core.types import Identity, ModelRequest, Task
from ferb.runtimes.catalog import CALL_LIMIT_S, PLANNER_LIMIT_S, call_limit
from tests.support.claude_home import signed_in_home
from tests.support.runtime_fakes import FakeBridge, FakeProvider, Process, Scripted

CODEX_START = cx.CodexRuntime.start
from ferb.runtimes.claude_sdk import ClaudeRuntime as _Claude  # noqa: E402
CLAUDE_START = _Claude.start             # saved before the offline fixture replaces it (module code runs first)
TASK = Task("t", None, "reply", "d", Identity.OWNER, set(), "mixed", "r", [], None)


def codex_home(tmp_path: Path) -> None:
    home = tmp_path / "codex-home"
    home.mkdir(exist_ok=True)
    (home / "auth.json").write_text('{"tokens":{"refresh_token":"test-only-fixture"}}')


def spawn_returns(monkeypatch: pytest.MonkeyPatch, proc: Any, seen: dict[str, Any] | None = None) -> None:
    async def spawn(*cmd: Any, **kw: Any) -> Any:
        if seen is not None:
            seen.update(cmd=cmd, **kw)
        return proc
    monkeypatch.setattr(asyncio, "create_subprocess_exec", spawn)


# ---------- Codex ----------

# decision: RUN-407
def test_codex_mcp_config_names_long_startup_and_call_timeouts(tmp_path: Path) -> None:
    rt = cx.CodexRuntime(Path("/x/codex"), "worker", tmp_path, shim_cmd=["ferb", "mcp"])
    settings = rt._mcp_settings(tmp_path / "tok")
    assert f"mcp_servers.ferb.startup_timeout_sec={cx.MCP_STARTUP_TIMEOUT_S}" in settings
    assert f"mcp_servers.ferb.tool_timeout_sec={cx.MCP_TOOL_TIMEOUT_S}" in settings
    assert cx.MCP_STARTUP_TIMEOUT_S >= 60 and cx.MCP_TOOL_TIMEOUT_S >= 24 * 3600   # never Codex's 10 s / 60 s


# decision: RUN-402, RUN-414
def test_auto_compaction_is_out_of_reach_for_supervisor_runs_only(tmp_path: Path) -> None:
    from ferb.runtimes.codex_stream import _command
    sup = " ".join(_command(cx.CodexRuntime(Path("/x/codex"), "supervisor", tmp_path), None))
    work = " ".join(_command(cx.CodexRuntime(Path("/x/codex"), "worker", tmp_path, shim_cmd=["ferb", "mcp"]), None,
                             tmp_path / "tok"))
    assert "model_auto_compact_token_limit=" in sup and "model_auto_compact_token_limit" not in work


# decision: RUN-410
@pytest.mark.anyio
async def test_a_worker_gets_ferbs_instructions_in_place_of_codexs_base_prompt(tmp_path, monkeypatch) -> None:
    import ferb.runtimes.codex_stream as stream
    codex_home(tmp_path)
    proc = Process(tools=[{"type": "function", "name": "mcp__ferb.mac_read"}])
    spawn_returns(monkeypatch, proc)
    rt = cx.CodexRuntime(Path("/fake/codex"), "worker", tmp_path, shim_cmd=["ferb", "mcp"], tool_names=["mac.read"],
                         output_schema={"type": "object"}, model="test-model")
    run = await stream.start(rt, TASK, ModelRequest("FERB worker instructions", ("ctx",)), "tok")
    thread = next(m["params"] for m in proc.requests if m["method"] == "thread/start")
    assert thread["baseInstructions"] == "FERB worker instructions" and thread["developerInstructions"] == ""
    await stream.cancel(run)


# decision: RUN-405
@pytest.mark.anyio
async def test_the_item_gate_records_the_kind_tool_and_server(tmp_path, monkeypatch) -> None:
    import ferb.runtimes.codex_stream as stream
    codex_home(tmp_path)
    proc = Process(tools=[{"type": "function", "name": "mcp__ferb.mac_read"}])
    spawn_returns(monkeypatch, proc)
    rt = cx.CodexRuntime(Path("/fake/codex"), "worker", tmp_path, shim_cmd=["ferb", "mcp"], tool_names=["mac.read"],
                         output_schema={"type": "object"}, model="test-model")
    run = await stream.start(rt, TASK, ModelRequest("sys", ("ctx",)), "tok")
    proc.notify("item/started", {"item": {"id": "m", "type": "mcpToolCall", "server": "other", "tool": "rm_all"}})
    got = [ev async for ev in stream.events(rt, run)]
    assert got[-1].kind == "violation"
    assert got[-1].data == {"unexpected_tools": ["other.rm_all"],
                            "item": {"kind": "mcpToolCall", "tool": "rm_all", "server": "other"}}


class RequestingProcess(Process):
    """Codex asking its client for something mid-turn (a server-to-client request)."""

    def write(self, data: bytes) -> None:
        super().write(data)
        msg = json.loads(data)
        if msg.get("method") == "turn/start":
            self.stdout.put({"id": 99, "method": "item/tool/requestUserInput", "params": {"threadId": "th"}})


# decision: RUN-405
@pytest.mark.anyio
async def test_a_server_to_client_request_is_a_violation_naming_the_request(tmp_path, monkeypatch) -> None:
    import ferb.runtimes.codex_stream as stream
    codex_home(tmp_path)
    proc = RequestingProcess()
    spawn_returns(monkeypatch, proc)
    rt = cx.CodexRuntime(Path("/fake/codex"), "supervisor", tmp_path, output_schema={"type": "object"},
                         model="test-model")
    run = await stream.start(rt, TASK, ModelRequest("sys", ("ctx",)), "")
    got = [ev async for ev in stream.events(rt, run)]
    assert got[-1].kind == "violation"                      # not "Codex protocol transport closed or failed"
    assert got[-1].data["item"] == {"kind": "serverRequest", "method": "item/tool/requestUserInput"}
    answer = next(m for m in proc.requests if m.get("id") == 99)
    assert answer["error"]["code"] == -32601                 # FERB told Codex no


# decision: RUN-406
@pytest.mark.anyio
async def test_one_deadline_covers_start_and_answer(tmp_path, monkeypatch) -> None:
    """One limit for the whole call (was 900 s to start plus another 900 s for the answer)."""
    import ferb.runtimes.codex_stream as stream
    codex_home(tmp_path)
    proc = Process()
    spawn_returns(monkeypatch, proc)
    rt = cx.CodexRuntime(Path("/fake/codex"), "supervisor", tmp_path, output_schema={"type": "object"},
                         model="test-model", timeout_s=0.3)
    loop = asyncio.get_running_loop()
    began = loop.time()
    run = await stream.start(rt, TASK, ModelRequest("sys", ("ctx",)), "")
    assert run.state["deadline"] == pytest.approx(began + 0.3, abs=0.1)
    got = [ev async for ev in stream.events(rt, run)]        # the fixture never completes its turn
    assert got[-1].kind == "error" and "timed out" in got[-1].data["message"]
    assert loop.time() - began < 1.0


# decision: RUN-406, ORCH-311
@pytest.mark.anyio
async def test_a_worker_run_has_no_deadline(tmp_path, monkeypatch) -> None:
    import ferb.runtimes.codex_stream as stream
    codex_home(tmp_path)
    proc = Process(tools=[{"type": "function", "name": "mcp__ferb.mac_read"}])
    spawn_returns(monkeypatch, proc)
    rt = cx.CodexRuntime(Path("/fake/codex"), "worker", tmp_path, shim_cmd=["ferb", "mcp"],
                         output_schema={"type": "object"}, model="test-model")
    run = await stream.start(rt, TASK, ModelRequest("sys", ("ctx",)), "tok")
    assert run.state["deadline"] is None and rt.timeout_s is None
    await stream.cancel(run)


# decision: RUN-406, ORCH-311
def test_call_limits_planner_other_calls_and_workers() -> None:
    assert PLANNER_LIMIT_S == 90 and call_limit("supervisor", True) == 90
    assert call_limit("supervisor", False) == CALL_LIMIT_S and call_limit("worker", False) is None


# decision: RUN-407
@pytest.mark.anyio
async def test_the_pinned_version_is_checked_once_not_per_call(tmp_path, monkeypatch) -> None:
    import ferb.runtimes.codex_stream as stream
    checks: list[Path] = []

    async def version(binary: Path) -> None:
        checks.append(binary)

    async def start(rt: Any, task: Any, request: Any, token: str) -> str:
        return "handle"
    monkeypatch.setattr(cx, "check_version", version)
    monkeypatch.setattr(cx, "_checked", set())
    monkeypatch.setattr(stream, "start", start)
    rt = cx.CodexRuntime(Path("/fake/codex-once"), "supervisor", tmp_path, output_schema={"type": "object"})
    for _ in range(3):
        await CODEX_START(rt, TASK, ModelRequest("sys", ("ctx",)), "")
    await cx.prepare(tmp_path, Path("/fake/codex-once"))
    assert checks == [Path("/fake/codex-once")]


# decision: RUN-405, RUN-407
def test_the_locked_catalog_is_written_once_and_again_only_when_codex_s_changes(tmp_path, monkeypatch) -> None:
    codex_home(tmp_path)
    source = tmp_path / "codex-home" / "models_cache.json"
    source.write_text(json.dumps({"models": [{"slug": "m", "tool_mode": "code", "apply_patch_tool_type": "freeform"}]}))
    writes = []
    real = cx.write_locked_catalog

    def counting(home: Path) -> Any:
        writes.append(home)
        return real(home)
    monkeypatch.setattr(cx, "write_locked_catalog", counting)
    monkeypatch.setattr(cx, "_locked", {})
    first = cx.locked_catalog(tmp_path)
    locked = json.loads(first.read_text())["models"][0] if first is not None else {}
    assert locked["tool_mode"] is None and locked["apply_patch_tool_type"] is None   # Codex 0.166.0 offers apply_patch
    assert cx.locked_catalog(tmp_path) == first and len(writes) == 1
    source.write_text(json.dumps({"models": [{"slug": "m"}, {"slug": "n"}]}))
    os.utime(source, ns=(1, 1))
    assert len(json.loads(cx.locked_catalog(tmp_path).read_text())["models"]) == 2 and len(writes) == 2  # type: ignore[union-attr]


# decision: RUN-406, ORCH-311, RUN-401
def test_daemon_builds_codex_with_the_planners_90_s_and_others_their_limit(daemon, monkeypatch) -> None:
    from ferb.orchestrator.schemas import QUESTION_SCHEMA, SUPERVISOR_SCHEMA, WORKER_SCHEMA
    d = daemon.daemon
    codex_home(d.home)
    monkeypatch.setattr(cx, "codex_binary", lambda ferb_home, repo=None: Path("/fake/codex"))
    planner = d._build_runtime("codex", "orchestrator", mode="supervisor", tool_names=[], schema=SUPERVISOR_SCHEMA)
    dispatch = d._build_runtime("codex", "orchestrator", mode="supervisor", tool_names=[], schema=QUESTION_SCHEMA)
    worker = d._build_runtime("codex", "worker:default", mode="worker", tool_names=[], schema=WORKER_SCHEMA)
    assert (planner.timeout_s, dispatch.timeout_s, worker.timeout_s) == (PLANNER_LIMIT_S, CALL_LIMIT_S, None)
    assert planner.cache_role == "planner"                # P20 on by default since the live injection eval passed
    assert dispatch.cache_role is None and worker.cache_role is None     # only the planner's turn
    d.config.set("runtimes", "codex_planner_cache", False)       # the owner can still turn it off
    off = d._build_runtime("codex", "orchestrator", mode="supervisor", tool_names=[], schema=SUPERVISOR_SCHEMA)
    assert off.cache_role is None


# ---------- Claude Agent SDK ----------

class FakeSDKClient:
    made: list[Any] = []

    def __init__(self, options: Any) -> None:
        self.options = options
        FakeSDKClient.made.append(self)

    async def connect(self) -> None:
        pass

    async def query(self, frames: Any) -> None:
        async for _ in frames:
            pass

    async def disconnect(self) -> None:
        pass


# decision: RUN-409
@pytest.mark.anyio
async def test_the_claude_run_token_goes_through_an_owner_only_file(monkeypatch) -> None:
    import claude_agent_sdk
    from ferb.runtimes.claude_sdk import ClaudeRuntime
    monkeypatch.setattr(claude_agent_sdk, "ClaudeSDKClient", FakeSDKClient)
    proxy = SimpleNamespace(base_url=lambda run_id: "http://unused", forget=lambda run_id: None)
    rt = ClaudeRuntime("worker", proxy, signed_in_home(), shim_cmd=["ferb", "mcp"], model="opus")  # type: ignore[arg-type]
    run = await CLAUDE_START(rt, TASK, ModelRequest("sys", ("ctx",)), "secret-run-token")
    options = FakeSDKClient.made[-1].options
    server = options.mcp_servers["ferb"]
    assert "secret-run-token" not in json.dumps(options.mcp_servers)       # Claude Code gets this config in argv
    token_file = Path(server["env"]["FERB_RUN_TOKEN_FILE"])
    assert token_file.read_text() == "secret-run-token" and token_file.stat().st_mode & 0o777 == 0o600
    assert token_file.parent.stat().st_mode & 0o777 == 0o700
    assert options.env["CLAUDE_CODE_OAUTH_TOKEN"] == ""
    await rt.cancel(run)
    assert not token_file.parent.exists()


# decision: RUN-409
def test_the_shim_reads_the_token_file(tmp_path, monkeypatch) -> None:
    from ferb.cli.mcp import Shim
    token = tmp_path / "run-token"
    token.write_text("from-the-file")
    monkeypatch.delenv("FERB_RUN_TOKEN", raising=False)
    monkeypatch.setenv("FERB_RUN_TOKEN_FILE", str(token))
    assert Shim()._token == "from-the-file"


# decision: RUN-404, RUN-409
def test_claude_is_built_only_with_ferbs_sign_in_and_a_model(daemon) -> None:
    from ferb.core.errors import Unsupported
    from ferb.runtimes.claude_login import mark
    d = daemon.daemon
    build = lambda: d._build_runtime("claude", "orchestrator", mode="supervisor", tool_names=[],  # noqa: E731
                                     schema={"type": "object"})
    with pytest.raises(Unsupported, match="isn't signed in for FERB"):
        build()
    mark(d.home)
    with pytest.raises(Unsupported, match="no model is chosen"):
        build()
    d.config.set("models", "claude", "sonnet")
    rt = build()
    assert rt.model == "sonnet" and rt.limit_s == CALL_LIMIT_S


# decision: RUN-409
def test_claude_sign_out_removes_the_marker(tmp_path, monkeypatch) -> None:
    from ferb.cli import setup as S
    from ferb.runtimes import claude_login as CL
    CL.mark(tmp_path)
    ran: list[list[str]] = []

    def run(argv: list[str], **kw: Any) -> Any:
        ran.append(argv)
        return SimpleNamespace(returncode=0, stdout='{"loggedIn": false}')
    monkeypatch.setattr(CL, "cli_binary", lambda: Path("/c"))
    said: list[str] = []
    env = SimpleNamespace(inst=SimpleNamespace(home=tmp_path), say=said.append)
    assert S.claude_logout(env, run=run) == 0                                # type: ignore[arg-type]
    assert ran[0] == ["/c", "auth", "logout"] and CL.claude_home(tmp_path) is None
    assert "Signed out" in said[-1]


# decision: RUN-409
def test_a_failed_claude_sign_out_keeps_the_marker(tmp_path, monkeypatch) -> None:
    from ferb.cli import setup as S
    from ferb.runtimes import claude_login as CL
    CL.mark(tmp_path)
    monkeypatch.setattr(CL, "cli_binary", lambda: Path("/c"))
    still = SimpleNamespace(returncode=1, stdout='{"loggedIn": true, "authMethod": "claude.ai"}')
    env = SimpleNamespace(inst=SimpleNamespace(home=tmp_path), say=lambda text: None)
    assert S.claude_logout(env, run=lambda argv, **kw: still) == 1              # type: ignore[arg-type]
    assert CL.claude_home(tmp_path) is not None


# ---------- direct APIs ----------

# decision: RUN-406
@pytest.mark.anyio
async def test_a_direct_api_call_ends_at_its_time_limit() -> None:
    from ferb.runtimes.api import ApiRuntime

    async def key(provider: str) -> str:
        return "k"
    script = Scripted([None])                                  # the provider never answers
    rt = ApiRuntime(FakeProvider(), mode="supervisor", role="orchestrator", tool_names=[], output_schema=None,
                    bridge=FakeBridge(), key=key, transport=script.transport(), limit_s=0.2)
    run = await rt.start(TASK, ModelRequest("sys", ("ctx",)), "")
    async with asyncio.timeout(2):
        events = [e async for e in rt.events(run)]
    assert events[-1].kind == "error" and "0.2 s limit" in events[-1].data["message"]
    assert run.state["run"].client.is_closed


# ---------- selection ----------


# decision: RUN-403
def test_an_unknown_worker_type_is_told_to_the_planner_without_blaming_config(daemon) -> None:
    from tests.conftest import chat, install, sup, task
    bad = {**task("make a file"), "worker_type": "gpt-9-runner"}
    install(daemon, [sup("task", "On it.", bad), sup("reply", "Done differently.")])
    chat(daemon, "make a file")

    async def read() -> Any:
        return daemon.daemon.journal.of_kind("task.repair")
    repairs = daemon.run(read())
    problems = " ".join(p for entry in repairs for p in entry.data["problems"])
    assert "worker_type 'gpt-9-runner' isn't one of the worker types available now: default, codex, claude" in problems
    assert "config.toml" not in problems


# decision: RUN-403
def test_a_worker_type_outside_the_table_is_named_as_a_worker_type(daemon) -> None:
    from ferb.core.errors import FerbError
    with pytest.raises(FerbError, match="there is no worker type 'gpt-9'") as raised:
        daemon.daemon._build_runtime("gpt-9", "worker:gpt-9", mode="worker", tool_names=[], schema={})
    assert "config.toml" not in str(raised.value)


# decision: RUN-403
def test_worker_types_reach_the_planner_in_layer_1_not_the_activity_block(daemon) -> None:
    d = daemon.daemon
    env = daemon.run(d._environment("hello"))
    assert 'Worker types: ["default", "codex", "claude"]' in env["stable"]
    text = d._relevant_context("hello")[0]
    assert "Worker types" not in text
