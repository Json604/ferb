"""L39, D32: a checker for a step that changed files runs at high reasoning effort, through the one runtime interface
(`make_runtime(..., effort=...)`); each adapter turns it into its own setting. Runs made without it are unchanged."""

from pathlib import Path

import pytest

from ferb.core.types import ModelRequest
from ferb.runtimes import codex_stream
from ferb.runtimes.claude_sdk import ClaudeRuntime
from ferb.runtimes.codex_exec import CodexRuntime
from tests.support.runtime_fakes import FakeBridge, FakeProvider, Scripted
from ferb.runtimes.api import ApiRuntime

REQUEST = ModelRequest("instructions", ("context",), 0)


def codex(tmp_path: Path, mode: str, **kw: object) -> CodexRuntime:
    return CodexRuntime(Path("/fake/codex"), mode, tmp_path / "work", tmp_path, shim_cmd=["ferb", "mcp"],  # type: ignore[arg-type]
                        model="test-model", timeout_s=None, **kw)                                         # type: ignore[arg-type]


def test_codex_turn_carries_the_effort_asked_for(tmp_path: Path) -> None:
    assert codex_stream._turn_params(codex(tmp_path, "worker", effort="high"), REQUEST, "th", False)["effort"] == "high"
    assert "effort" not in codex_stream._turn_params(codex(tmp_path, "worker"), REQUEST, "th", False)   # default
    assert codex_stream._turn_params(codex(tmp_path, "supervisor"), REQUEST, "th", False)["effort"] == "low"


@pytest.mark.anyio
async def test_api_runtime_sends_the_effort_asked_for() -> None:
    script = Scripted([(200, {"text": "ok"})])

    async def key(provider: str) -> str:
        return "sk"
    rt = ApiRuntime(FakeProvider(), mode="worker", role="checker", tool_names=[], output_schema=None,
                    bridge=FakeBridge(), key=key, transport=script.transport(), effort="high")
    from ferb.core.types import Identity, Task
    run = await rt.start(Task("t.verify", None, "verify", "d", Identity.OWNER, set(), "verifier", "r", [], None),
                         REQUEST, "tok")
    [_ async for _ in rt.events(run)]
    assert script.requests[0][1]["effort"] == "high"


def test_claude_runtime_options_carry_the_effort_asked_for() -> None:
    rt = ClaudeRuntime("worker", None, "/unused", shim_cmd=["ferb", "mcp"], effort="high")    # type: ignore[arg-type]
    rt.proxy = type("P", (), {"base_url": lambda self, run: "http://127.0.0.1:1"})()        # type: ignore[assignment]
    assert rt._options("run-1", "tok").effort == "high"
    plain = ClaudeRuntime("worker", rt.proxy, "/unused", shim_cmd=["ferb", "mcp"])
    assert plain._options("run-1", "tok").effort is None
