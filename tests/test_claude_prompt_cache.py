"""The Claude runtime caches FERB's stable prefix (P20, D32): FERB's system text and each context part as their own text
blocks in the one user message, a 1-hour cache breakpoint on the last stable part (on the system block when no part is
stable), images after the text. No role changes: everything stays in the user message, as before.

Claude Code 2.1.283 adds breakpoints of its own (its system prompt, its trailing environment message); the API allows
four and requires a longer TTL never to follow a shorter one, so FERB pins Claude Code's own TTL to 1 hour too. The
pinned test sends FERB's real request through the bundled CLI to a loopback server (a fake API key, an empty config
directory: no model, no credential) and checks both rules on what the CLI actually sent."""
import asyncio
import json
import re
import tempfile
import time
from pathlib import Path
from typing import Any

import pytest

from ferb.core.types import Identity, ModelImage, ModelRequest, Task
from ferb.orchestrator.schemas import SUPERVISOR_SCHEMA
from ferb.runtimes.claude_sdk import ClaudeRuntime
from tests.support.model_loopback import ModelLoopback

CLAUDE_START = ClaudeRuntime.start          # saved before the no_real_models fixture patches it
PINNED_CLAUDE_CODE = "2.1.283"
HOUR = {"type": "ephemeral", "ttl": "1h"}
TASK = Task("t", None, "chat turn", "dev", Identity.OWNER, set(), "mixed", "r", [], None)
JPEG = b"\xff\xd8\xff a redacted window"


async def sent_content(monkeypatch: pytest.MonkeyPatch, request: ModelRequest) -> list[dict[str, Any]]:
    import claude_agent_sdk
    sent: list[Any] = []

    class Client:
        def __init__(self, **kwargs: Any) -> None:
            pass

        async def connect(self) -> None:
            pass

        async def query(self, prompt: Any) -> None:
            sent.append(prompt if isinstance(prompt, str) else [m async for m in prompt])

        async def disconnect(self) -> None:
            pass
    monkeypatch.setattr(claude_agent_sdk, "ClaudeSDKClient", Client)
    rt = ClaudeRuntime("supervisor", None, "/unused", output_schema={"type": "object"})   # type: ignore[arg-type]
    monkeypatch.setattr(rt, "_options", lambda *args: None)
    await CLAUDE_START(rt, TASK, request, "")
    [frames] = sent
    [message] = frames
    assert message["type"] == "user" and message["message"]["role"] == "user"
    return message["message"]["content"]         # type: ignore[no-any-return]


@pytest.mark.anyio
async def test_each_part_is_its_own_block_with_the_breakpoint_on_the_last_stable_one(monkeypatch) -> None:
    content = await sent_content(monkeypatch, ModelRequest("FERB instructions", ("layer 1", "chat", "per turn"), 2))
    assert content == [{"type": "text", "text": "FERB instructions"},
                       {"type": "text", "text": "layer 1"},
                       {"type": "text", "text": "chat", "cache_control": HOUR},
                       {"type": "text", "text": "per turn"}]


@pytest.mark.anyio
async def test_with_no_stable_part_the_system_block_carries_the_breakpoint(monkeypatch) -> None:
    content = await sent_content(monkeypatch, ModelRequest("FERB instructions", ("per turn",), 0))
    assert content == [{"type": "text", "text": "FERB instructions", "cache_control": HOUR},
                       {"type": "text", "text": "per turn"}]


@pytest.mark.anyio
async def test_images_follow_the_text_so_the_cached_prefix_stays(monkeypatch) -> None:
    image = ModelImage(JPEG, "ev-1")
    content = await sent_content(monkeypatch, ModelRequest("FERB instructions", ("stable", "per turn"), 1, (image,)))
    assert [c["type"] for c in content] == ["text", "text", "text", "image"]
    assert content[1] == {"type": "text", "text": "stable", "cache_control": HOUR}
    assert content[3] == {"type": "image", "source": {"type": "base64", "media_type": image.mime,
                                                      "data": image.base64()}}


@pytest.mark.anyio
async def test_empty_text_is_never_sent_as_a_block(monkeypatch) -> None:
    # A direct call without instructions (ModelRequest("", ...)): the API refuses empty text blocks
    content = await sent_content(monkeypatch, ModelRequest("", ("only part",), 0))
    assert content == [{"type": "text", "text": "only part"}]
    content = await sent_content(monkeypatch, ModelRequest("FERB instructions", ("", "per turn"), 1))
    assert content == [{"type": "text", "text": "FERB instructions", "cache_control": HOUR},
                       {"type": "text", "text": "per turn"}]


def test_claude_codes_own_breakpoints_are_pinned_to_one_hour() -> None:
    # Unset, Claude Code picks 5 minutes once a subscription is past its usage limits; a 5-minute marker before FERB's
    # 1-hour one is refused (TTL order), so FERB's turn would fail there
    proxy = type("P", (), {"base_url": lambda self, run_id: "http://unused"})()
    rt = ClaudeRuntime("supervisor", proxy, "/unused", output_schema={"type": "object"})   # type: ignore[arg-type]
    assert rt._options("run", "").env["CLAUDE_CODE_PROMPT_CACHE_TTL"] == "1h"
    assert ClaudeRuntime.capabilities.cache == "markers"          # `ferb runtimes`: FERB marks the cacheable prefix


# ---------- pinned to Claude Code 2.1.283: what the bundled CLI sends ----------

def _bundled_cli() -> Path | None:
    import claude_agent_sdk
    path = Path(claude_agent_sdk.__file__).parent / "_bundled" / "claude"
    return path if path.exists() else None


class _LoopbackProxy:
    """The adapter's proxy interface (ferb.runtimes.proxy.ToolListProxy), pointing at the loopback server."""
    def __init__(self, url: str) -> None:
        self.url = url

    def base_url(self, run_id: str) -> str:
        return self.url

    def seen(self, run_id: str) -> set[str]:
        return set()

    def forget(self, run_id: str) -> None:
        pass


def _breakpoints(body: dict[str, Any]) -> list[tuple[str, dict[str, Any]]]:
    """Every cache_control in the API's processing order: tools, then system, then messages (and a top-level one)."""
    found: list[tuple[str, dict[str, Any]]] = []
    for tool in body.get("tools") or []:
        if "cache_control" in tool:
            found.append(("tool", tool["cache_control"]))
    system = body.get("system")
    for block in system if isinstance(system, list) else []:
        if "cache_control" in block:
            found.append(("system", block["cache_control"]))
    for message in body.get("messages") or []:
        for block in message["content"] if isinstance(message.get("content"), list) else []:
            if "cache_control" in block:
                found.append((message["role"] + ":" + str(block.get("text", ""))[:40], block["cache_control"]))
    if "cache_control" in body:
        found.append(("top", body["cache_control"]))
    return found


@pytest.mark.anyio
async def test_pinned_claude_code_request_keeps_ferbs_breakpoint_valid(monkeypatch: pytest.MonkeyPatch) -> None:
    import claude_agent_sdk._cli_version as cli
    if _bundled_cli() is None:
        pytest.skip("the bundled Claude Code CLI isn't installed")
    assert cli.__cli_version__ == PINNED_CLAUDE_CODE, (
        f"the bundled Claude Code is {cli.__cli_version__}, not the pinned {PINNED_CLAUDE_CODE}: re-measure its own "
        "cache breakpoints (count and TTL) before moving the pin")
    loopback = ModelLoopback()
    config_dir = tempfile.TemporaryDirectory(prefix="ferb-claude-shape-")
    proxy = _LoopbackProxy(loopback.url)
    rt = ClaudeRuntime("supervisor", proxy, "/unused", tool_names=[], output_schema=SUPERVISOR_SCHEMA,  # type: ignore[arg-type]
                       model="opus")
    real_options = rt._options

    def loopback_options(run_id: str, run_token: str) -> Any:
        options = real_options(run_id, run_token)
        # Loopback only: a fake key and an empty config directory, never the owner's login or settings
        options.env.update(ANTHROPIC_API_KEY="sk-ant-loopback-test", CLAUDE_CONFIG_DIR=config_dir.name)
        return options
    monkeypatch.setattr(rt, "_options", loopback_options)
    request = ModelRequest("FERB-SYSTEM-MARKER", ("FERB-LAYER-1", "FERB-CHAT-ENTRY", "FERB-TURN-TAIL"), 2)
    run = await CLAUDE_START(rt, TASK, request, "")
    try:
        deadline = time.monotonic() + 60
        while not any("FERB-TURN-TAIL" in json.dumps(b) for b in loopback.bodies()):
            assert time.monotonic() < deadline, "Claude Code sent no request to the loopback server"
            await asyncio.sleep(0.2)
    finally:
        await rt.cancel(run)
        loopback.close()
        config_dir.cleanup()
    body = next(b for b in loopback.bodies() if "FERB-TURN-TAIL" in json.dumps(b))
    version = re.search(r"cc_version=([0-9]+\.[0-9]+\.[0-9]+)", json.dumps(body.get("system")))
    assert version and version.group(1) == PINNED_CLAUDE_CODE, (version, "the CLI that sent it is not the pinned one")
    marks = _breakpoints(body)
    assert len(marks) <= 4, marks                                          # the API's limit
    ttls = [mark.get("ttl", "5m") for _, mark in marks]
    assert not any(a == "5m" and b == "1h" for i, a in enumerate(ttls) for b in ttls[i + 1:]), marks   # 1h never after 5m
    [user] = [m for m in body["messages"] if m["role"] == "user"]
    assert [b.get("text") for b in user["content"]] == ["FERB-SYSTEM-MARKER", "FERB-LAYER-1", "FERB-CHAT-ENTRY",
                                                        "FERB-TURN-TAIL"]
    assert user["content"][2]["cache_control"] == HOUR and sum("cache_control" in b for b in user["content"]) == 1
