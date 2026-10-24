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
