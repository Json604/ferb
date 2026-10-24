"""Codex prompt caching (P20, D32), pinned to Codex 0.157.1.

Skills off for every role: Codex otherwise adds a `<skills_instructions>` developer message holding the run's random
temporary path right after FERB's instructions (~611 tokens and 1-3 s per run, measured 2026-10-25), so nothing after
FERB's system prompt could repeat across runs. The pinned tests read what the binary actually sends, through a loopback
model server (tests/support/codex_loopback.py)."""
import json
import subprocess
import tempfile
from pathlib import Path

from ferb.runtimes import codex_exec as cx
from ferb.runtimes import codex_stream as stream
from tests.support import codex_loopback as lb
from tests.support.model_loopback import ModelLoopback

SKILLS_OFF = ["-c", "skills.include_instructions=false", "-c", "skills.bundled.enabled=false"]


def _has(cmd: list[str], flags: list[str]) -> bool:
    return any(cmd[i:i + len(flags)] == flags for i in range(len(cmd)))


def test_every_app_server_role_runs_with_skills_off(tmp_path: Path) -> None:
    planner = cx.CodexRuntime(Path("/x/codex"), "supervisor", tmp_path, tmp_path)
    worker = cx.CodexRuntime(Path("/x/codex"), "worker", tmp_path, tmp_path, shim_cmd=["ferb", "mcp"])
    assert _has(stream._command(planner, None), SKILLS_OFF)
    assert _has(stream._command(worker, None, tmp_path / "token"), SKILLS_OFF)


def test_every_exec_role_runs_with_skills_off(tmp_path: Path) -> None:
    planner = cx.CodexRuntime(Path("/x/codex"), "supervisor", tmp_path, tmp_path)
    worker = cx.CodexRuntime(Path("/x/codex"), "worker", tmp_path, tmp_path, shim_cmd=["ferb", "mcp"])
    assert _has(planner._cmd(None, None, tmp_path / "instructions.md"), SKILLS_OFF)
    assert _has(worker._cmd(None, tmp_path / "token"), SKILLS_OFF)


def test_pinned_codex_exec_sends_no_skills_message() -> None:
    binary = lb.pinned_codex()
    loopback = ModelLoopback()
    with tempfile.TemporaryDirectory(prefix="ferb-codex-shape-") as name:
        root = Path(name)
        ferb_home = lb.ferb_home_with_catalog(root)
        codex_home, user = root / "codex", root / "user"
        lb.api_key_login(codex_home)
        user.mkdir()
        instructions = root / "instructions.md"
        instructions.write_text("FERB-SYSTEM-MARKER")
        rt = cx.CodexRuntime(binary, "supervisor", root / "work", ferb_home)
        (root / "work").mkdir()
        cmd = rt._cmd(None, None, instructions) + lb.base_url_flag(loopback) + ["-"]
        subprocess.run(cmd, input=b"FERB-TURN-MARKER", capture_output=True, env=lb.loopback_env(codex_home, user),
                       timeout=120, check=False)          # the loopback refuses the request: Codex exits non-zero
        body = lb.model_requests(loopback, 2)[-1]
    loopback.close()
    assert lb.roles(body) == ["additional_tools:developer", "message:developer", "message:user"]
    assert lb.texts(body)[1:] == ["FERB-SYSTEM-MARKER", "FERB-TURN-MARKER"]
    assert "skills_instructions" not in json.dumps(body) and str(root) not in json.dumps(body)


def test_pinned_codex_app_server_sends_no_skills_message() -> None:
    binary = lb.pinned_codex()
    loopback = ModelLoopback()
    with tempfile.TemporaryDirectory(prefix="ferb-codex-shape-") as name:
        root = Path(name)
        ferb_home = lb.ferb_home_with_catalog(root)
        codex_home, user, work = root / "codex", root / "user", root / "work"
        lb.api_key_login(codex_home)
        user.mkdir()
        work.mkdir()
        rt = cx.CodexRuntime(binary, "supervisor", work, ferb_home, output_schema={"type": "object"})
        server = lb.AppServer(stream._command(rt, cx.locked_catalog(ferb_home)) + lb.base_url_flag(loopback),
                              lb.loopback_env(codex_home, user), work)
        try:
            server.initialize()
            thread = server.rpc("thread/start", {
                "model": cx.MODEL, "modelProvider": "openai", "cwd": str(work), "approvalPolicy": "never",
                "sandbox": "read-only", "baseInstructions": "FERB-SYSTEM-MARKER", "developerInstructions": "",
                "ephemeral": True, "dynamicTools": [], "environments": [], "selectedCapabilityRoots": [],
                "allowProviderModelFallback": False, "experimentalRawEvents": False})["result"]
            server.rpc("turn/start", {"threadId": thread["thread"]["id"], "effort": "low", "environments": [],
                                      "input": [{"type": "text", "text": "FERB-TURN-MARKER"}]})
            body = lb.model_requests(loopback, 2)[-1]
        finally:
            server.close()
            loopback.close()
    assert lb.roles(body) == ["additional_tools:developer", "message:developer", "message:user"]
    assert lb.texts(body)[1:] == ["FERB-SYSTEM-MARKER", "FERB-TURN-MARKER"]
    assert "skills_instructions" not in json.dumps(body) and str(root) not in json.dumps(body)


# ---------- Part C: the planner's context in the developer role, under a fixed cache key (owner, 2026-10-25) ----------
# Behind `[runtimes] codex_planner_cache` (default off until the owner passes the live injection eval,
# tests/security/test_planner_injection_real.py). The key is the thread id, so FERB resumes a template rollout it
# writes into the run's own CODEX_HOME under an id fixed per role and model.

import asyncio  # noqa: E402
import stat  # noqa: E402
import uuid  # noqa: E402
from typing import Any  # noqa: E402

import pytest  # noqa: E402

from ferb.core.types import Identity, ModelImage, ModelRequest, Task  # noqa: E402
from tests.support.runtime_fakes import Process  # noqa: E402

TASK = Task("t", None, "chat turn", "dev", Identity.OWNER, set(), "mixed", "r", [], None)
PLANNER_REQUEST = ModelRequest("FERB instructions", ("Layer 1", "Layer 5 chat", "Layer 2 now", "Layer 5 message",
                                                     "Context budget"), 2)


def _planner(tmp_path: Path, cache_role: str | None = "planner", **kw: Any) -> cx.CodexRuntime:
    home = tmp_path / "codex-home"
    home.mkdir(exist_ok=True)
    (home / "auth.json").write_text('{"tokens":{"refresh_token":"test-only-fixture"}}')
    return cx.CodexRuntime(Path("/fake/codex"), "supervisor", tmp_path / "work", tmp_path,
                           output_schema={"type": "object", "properties": {"text": {"type": "string"}}},
                           model="test-model", timeout_s=None, cache_role=cache_role, **kw)


async def _turn(monkeypatch: pytest.MonkeyPatch, rt: cx.CodexRuntime, proc: Process,
                request: ModelRequest = PLANNER_REQUEST) -> tuple[list[Any], dict[str, Any]]:
    seen: dict[str, Any] = {}

    async def spawn(*cmd: Any, **kw: Any) -> Process:
        seen.update(cmd=cmd, **kw)
        return proc
    monkeypatch.setattr(asyncio, "create_subprocess_exec", spawn)
    run = await stream.start(rt, TASK, request)
    seen["home"] = Path(seen["env"]["CODEX_HOME"])
    seen["template_existed"] = any(seen["home"].rglob("rollout-*.jsonl"))
    proc.complete()
    events = [e async for e in stream.events(rt, run)]
    return events, seen


def _sent(proc: Process, method: str) -> list[dict[str, Any]]:
    return [m["params"] for m in proc.requests if m["method"] == method]


def test_the_cache_key_is_fixed_per_role_and_model() -> None:
    key = stream.cache_thread_id("planner", "gpt-5.6-sol")
    assert key == stream.cache_thread_id("planner", "gpt-5.6-sol") and uuid.UUID(key)
    assert key != stream.cache_thread_id("planner", "gpt-6-sol") != stream.cache_thread_id("other", "gpt-5.6-sol")


def test_the_template_is_two_lines_with_multi_agent_disabled(tmp_path: Path) -> None:
    key = stream.cache_thread_id("planner", "gpt-5.6-sol")
    path = stream.write_template(tmp_path, tmp_path / "work", key, "gpt-5.6-sol", "FERB instructions")
    assert path.parent == tmp_path / "sessions" / "2026" / "01" / "01"
    assert path.name == f"rollout-2026-01-01T00-00-00-{key}.jsonl"           # Codex's canonical rollout name
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    meta, context = [json.loads(line) for line in path.read_text().splitlines()]
    # No world_state line: with one, Codex 0.157.1 drops the developer instructions (measured on its request)
    assert [meta["type"], context["type"]] == ["session_meta", "turn_context"]
    assert meta["payload"]["id"] == key and meta["payload"]["cli_version"] == cx.CODEX_VERSION
    assert meta["payload"]["base_instructions"]["text"] == "FERB instructions"
    assert context["payload"]["multi_agent_version"] == "disabled"           # else multi_agent_v1 tools come back
    assert context["payload"]["model"] == "gpt-5.6-sol" and context["payload"]["cwd"] == str(tmp_path / "work")
    assert context["payload"]["sandbox_policy"] == {"type": "read-only"}
    assert context["payload"]["approval_policy"] == "never"


@pytest.mark.anyio
async def test_a_cached_planner_turn_puts_the_stable_parts_in_the_developer_role(tmp_path: Path, monkeypatch) -> None:
    proc = Process()
    image = ModelImage(b"\xff\xd8\xff window", "ev-1")
    request = ModelRequest(PLANNER_REQUEST.system, PLANNER_REQUEST.parts, PLANNER_REQUEST.cacheable, (image,))
    events, seen = await _turn(monkeypatch, _planner(tmp_path), proc, request)
    assert events[-1].kind == "final"
    assert _sent(proc, "thread/start") == []
    [resume] = _sent(proc, "thread/resume")
    key = stream.cache_thread_id("planner", "test-model")
    assert resume["threadId"] == key and seen["template_existed"]
    assert Path(resume["path"]).is_relative_to(seen["home"])               # the run's own CODEX_HOME, removed after
    assert resume["baseInstructions"] == "FERB instructions"               # the system prompt is unchanged
    assert resume["developerInstructions"] == stream.CONTEXT_FRAMING + "\n\nLayer 1\n\nLayer 5 chat"
    assert (resume["approvalPolicy"], resume["sandbox"], resume["model"], resume["modelProvider"]) == (
        "never", "read-only", "test-model", "openai")
    [turn] = _sent(proc, "turn/start")
    assert turn["threadId"] == key
    assert turn["input"] == [{"type": "text", "text": "Layer 2 now\n\nLayer 5 message\n\nContext budget"},
                             {"type": "image", "url": image.data_url()}]
    assert not seen["home"].exists()                                        # nothing persists across runs


@pytest.mark.anyio
async def test_without_the_switch_the_planner_turn_is_unchanged(tmp_path: Path, monkeypatch) -> None:
    proc = Process()
    events, _ = await _turn(monkeypatch, _planner(tmp_path, cache_role=None), proc)
    assert events[-1].kind == "final" and _sent(proc, "thread/resume") == []
    [start] = _sent(proc, "thread/start")
    assert start["baseInstructions"] == "FERB instructions" and start["developerInstructions"] == ""
    assert _sent(proc, "turn/start")[0]["input"] == [{"type": "text", "text": PLANNER_REQUEST.context}]


@pytest.mark.anyio
async def test_a_worker_never_resumes_a_template(tmp_path: Path, monkeypatch) -> None:
    proc = Process(tools=[{"type": "function", "name": "mcp__ferb.mac_read"}])
    rt = _planner(tmp_path, shim_cmd=["ferb", "mcp"])
    rt.mode = "worker"
    rt.allowed = rt.allowed | {"mcp__ferb.mac_read"}

    async def spawn(*cmd: Any, **kw: Any) -> Process:
        return proc
    monkeypatch.setattr(asyncio, "create_subprocess_exec", spawn)
    run = await stream.start(rt, TASK, PLANNER_REQUEST, "token")
    await stream.cancel(run)
    assert _sent(proc, "thread/resume") == [] and len(_sent(proc, "thread/start")) == 1


def _fallbacks(rt: cx.CodexRuntime) -> list[str]:
    said: list[str] = []
    rt.on_cache_fallback = said.append
    return said


@pytest.mark.anyio
async def test_a_refused_resume_falls_back_to_todays_turn_and_says_why(tmp_path: Path, monkeypatch) -> None:
    proc = Process(errors={"thread/resume": {"code": -32600, "message": "rollout path is not canonical"}})
    rt = _planner(tmp_path)
    said = _fallbacks(rt)
    events, _ = await _turn(monkeypatch, rt, proc)
    assert events[-1].kind == "final"                                       # the turn never fails for it
    assert len(_sent(proc, "thread/start")) == 1
    assert _sent(proc, "turn/start")[0]["input"] == [{"type": "text", "text": PLANNER_REQUEST.context}]
    assert len(said) == 1 and "rollout path is not canonical" in said[0] and "code -32600" in said[0]


@pytest.mark.anyio
@pytest.mark.parametrize("change", [{"sandbox": {"type": "readOnly", "networkAccess": True}},
                                    {"approvalPolicy": "onRequest"}, {"model": "another-model"},
                                    {"instructionSources": ["/Users/me/AGENTS.md"]}, {"thread": {"id": "another"}}])
async def test_a_resumed_thread_outside_ferbs_isolation_is_not_used(tmp_path: Path, monkeypatch, change) -> None:
    proc = Process(resume_changes=change)
    rt = _planner(tmp_path)
    said = _fallbacks(rt)
    events, _ = await _turn(monkeypatch, rt, proc)
    assert events[-1].kind == "final" and len(_sent(proc, "thread/start")) == 1
    assert _sent(proc, "turn/start")[0]["threadId"] == "th"                 # today's thread, not the resumed one
    assert said == ["the resumed planner thread did not match FERB's isolation"]


@pytest.mark.anyio
async def test_a_template_that_cannot_be_written_falls_back(tmp_path: Path, monkeypatch) -> None:
    def broken(*args: Any, **kw: Any) -> Path:
        raise OSError("disk full")
    monkeypatch.setattr(stream, "write_template", broken)
    proc = Process()
    rt = _planner(tmp_path)
    said = _fallbacks(rt)
    events, _ = await _turn(monkeypatch, rt, proc)
    assert events[-1].kind == "final" and len(_sent(proc, "thread/start")) == 1
    assert said == ["the planner template could not be written (OSError)"]


@pytest.mark.anyio
async def test_a_request_with_nothing_stable_takes_todays_path(tmp_path: Path, monkeypatch) -> None:
    proc = Process()
    rt = _planner(tmp_path)
    said = _fallbacks(rt)
    events, _ = await _turn(monkeypatch, rt, proc, ModelRequest("FERB instructions", ("only this turn",), 0))
    assert events[-1].kind == "final" and _sent(proc, "thread/resume") == [] and said == []


# ---------- the switch: only the planner's turn, only when the owner turns it on ----------

from ferb.orchestrator.schemas import SUMMARY_SCHEMA, SUPERVISOR_SCHEMA, WORKER_SCHEMA  # noqa: E402
from tests.conftest import RunningDaemon  # noqa: E402


def _codex_for(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> Any:
    login = daemon.daemon.home / "codex-home"
    login.mkdir(exist_ok=True)
    (login / "auth.json").write_text("{}")
    monkeypatch.setattr(cx, "codex_binary", lambda ferb_home, repo=None: Path("/fake/codex"))

    def build(role: str, mode: str, schema: dict[str, Any]) -> cx.CodexRuntime:
        return daemon.daemon._build_runtime("codex", role, mode=mode, tool_names=[], schema=schema)  # type: ignore[no-any-return]
    return build


def test_only_the_planners_turn_is_cached_and_only_when_switched_on(daemon: RunningDaemon, monkeypatch) -> None:
    build = _codex_for(daemon, monkeypatch)
    assert build("orchestrator", "supervisor", SUPERVISOR_SCHEMA).cache_role is None          # off by default
    daemon.daemon.config.set("runtimes", "codex_planner_cache", "true")                       # only the boolean
    assert build("orchestrator", "supervisor", SUPERVISOR_SCHEMA).cache_role is None
    daemon.daemon.config.set("runtimes", "codex_planner_cache", True)
    assert build("orchestrator", "supervisor", SUPERVISOR_SCHEMA).cache_role == "planner"
    assert build("orchestrator", "supervisor", SUMMARY_SCHEMA).cache_role is None             # other planner-side calls
    assert build("worker", "supervisor", SUMMARY_SCHEMA).cache_role is None                   # learners
    assert build("worker:default", "worker", WORKER_SCHEMA).cache_role is None                # tasks


def test_a_fallback_is_one_journal_line_saying_why(daemon: RunningDaemon, monkeypatch) -> None:
    build = _codex_for(daemon, monkeypatch)
    daemon.daemon.config.set("runtimes", "codex_planner_cache", True)
    rt = build("orchestrator", "supervisor", SUPERVISOR_SCHEMA)

    async def fall_back() -> list[Any]:
        assert rt.on_cache_fallback is not None
        rt.on_cache_fallback("Codex could not complete thread/resume: refused (code -32600)")
        return [e.data for e in daemon.daemon.supervisor.journal.of_kind("runtime.cache_fallback")]
    assert daemon.run(fall_back()) == [{"runtime": "codex", "role": "planner",
                                        "why": "Codex could not complete thread/resume: refused (code -32600)"}]


# ---------- pinned to Codex 0.157.1: what the binary sends for a cached planner turn ----------

def _pinned_server(root: Path, loopback: ModelLoopback) -> tuple[cx.CodexRuntime, Any, Path]:
    binary = lb.pinned_codex()
    ferb_home = lb.ferb_home_with_catalog(root)
    codex_home, user, work = root / "codex", root / "user", root / "work"
    lb.api_key_login(codex_home)
    user.mkdir()
    work.mkdir()
    rt = cx.CodexRuntime(binary, "supervisor", work, ferb_home, output_schema=SUPERVISOR_SCHEMA, cache_role="planner")
    server = lb.AppServer(stream._command(rt, cx.locked_catalog(ferb_home)) + lb.base_url_flag(loopback),
                          lb.loopback_env(codex_home, user), work)
    server.initialize()
    return rt, server, codex_home


def _comparable(body: dict[str, Any]) -> dict[str, Any]:
    """A request without what differs on every request anyway (ids, per-turn metadata, the cache key)."""
    body = json.loads(json.dumps(body))
    for key in ("client_metadata", "prompt_cache_key"):
        body.pop(key, None)
    for item in body.get("input", []):
        item.pop("id", None)
        for key in ("turn_id", "create_time"):
            (item.get("internal_chat_message_metadata_passthrough") or {}).pop(key, None)
    return body


def _pinned_turn(resume: bool) -> list[dict[str, Any]]:
    loopback = ModelLoopback()
    with tempfile.TemporaryDirectory(prefix="ferb-codex-shape-") as name:
        root = Path(name)
        rt, server, codex_home = _pinned_server(root, loopback)
        try:
            key = stream.cache_thread_id("planner", rt.model)
            if resume:
                path = stream.write_template(codex_home, rt.workdir, key, rt.model, PLANNER_REQUEST.system)
                thread = server.rpc("thread/resume", stream._resume_params(rt, PLANNER_REQUEST, rt.workdir, key, path))
            else:   # today's thread, with the same developer instructions, for comparison
                params = stream._start_params(rt, PLANNER_REQUEST, rt.workdir)
                params["developerInstructions"] = stream._resume_params(rt, PLANNER_REQUEST, rt.workdir, key,
                                                                        root)["developerInstructions"]
                thread = server.rpc("thread/start", params)
            assert "result" in thread, thread
            assert stream._isolated(thread["result"], rt)
            server.rpc("turn/start", stream._turn_params(rt, PLANNER_REQUEST, thread["result"]["thread"]["id"], True))
            bodies = lb.model_requests(loopback, 2)
        finally:
            server.close()
            loopback.close()
    return bodies


def test_pinned_cached_turn_request_shape() -> None:
    warm_up, body = _pinned_turn(resume=True)
    key = stream.cache_thread_id("planner", cx.MODEL)
    assert body["prompt_cache_key"] == key == warm_up["prompt_cache_key"]          # the fixed key reaches the service
    # tools, developer (FERB's system prompt), developer (the stable context), user (what changes this turn)
    assert lb.roles(body) == ["additional_tools:developer", "message:developer", "message:developer", "message:user"]
    assert lb.texts(body)[1:] == ["FERB instructions", stream.CONTEXT_FRAMING + "\n\nLayer 1\n\nLayer 5 chat",
                                  "Layer 2 now\n\nLayer 5 message\n\nContext budget"]
    # the template brings no multi-agent (or other) tool: the same catalog as today's thread, warm-up included
    for request in (warm_up, body):
        assert set(lb.tool_names(request)) <= {"functions." + name for name in cx.BASE_TOOLS}, lb.tool_names(request)
    # and otherwise exactly the request today's thread sends with those developer instructions
    assert _comparable(body) == _comparable(_pinned_turn(resume=False)[1])


def test_pinned_refused_template_leaves_todays_thread_working() -> None:
    """The fallback's trigger is real: Codex 0.157.1 refuses a template it can't read (here its session line lacks a
    field Codex needs) with a JSON-RPC error, FERB then takes thread/start (tests above), and today's thread still works
    in the same app-server."""
    loopback = ModelLoopback()
    with tempfile.TemporaryDirectory(prefix="ferb-codex-shape-") as name:
        root = Path(name)
        rt, server, codex_home = _pinned_server(root, loopback)
        try:
            key = stream.cache_thread_id("planner", rt.model)
            path = stream.write_template(codex_home, rt.workdir, key, rt.model, PLANNER_REQUEST.system)
            meta, context = [json.loads(line) for line in path.read_text().splitlines()]
            del meta["payload"]["cli_version"]
            path.write_text(json.dumps(meta) + "\n" + json.dumps(context) + "\n")
            refused = server.rpc("thread/resume", stream._resume_params(rt, PLANNER_REQUEST, rt.workdir, key, path))
            assert "error" in refused and isinstance(refused["error"].get("code"), int)
            thread = server.rpc("thread/start", stream._start_params(rt, PLANNER_REQUEST, rt.workdir))["result"]
            assert stream._isolated(thread, rt) and thread["thread"]["ephemeral"]
            server.rpc("turn/start", stream._turn_params(rt, PLANNER_REQUEST, thread["thread"]["id"], False))
            body = lb.model_requests(loopback, 2)[-1]
        finally:
            server.close()
            loopback.close()
    assert lb.roles(body) == ["additional_tools:developer", "message:developer", "message:user"]
    assert lb.texts(body)[1:] == ["FERB instructions", PLANNER_REQUEST.context]
