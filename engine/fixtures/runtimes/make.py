"""Writes the fixtures the Rust crate `ferb-runtimes` is checked against (ENG-3).

Run from the repo root:  uv run python rust/fixtures/runtimes/make.py

Each adapter of ferb/runtimes (and the tool bridge, ferb/cli/mcp.py) is driven by invented scenarios against a
scripted stand-in: the Codex app-server and Claude Code as in-process fake programs that answer each stdin frame from
the scenario's script, the provider APIs as an httpx transport, FERB's daemon as a Unix socket. Everything the Python
code writes is recorded (argv, environment, working folder, stdin frames, HTTP bodies, socket requests, stdout of the
bridge) with every event it yields and every error it raises. The Rust tests play the same scenarios against the same
scripts and compare.

- codex.json: CodexRuntime (codex_exec.py + codex_stream.py)
- claude.json: ClaudeRuntime (claude_sdk.py with the pinned claude-agent-sdk's transport, query and parser)
- mcp.json: the stdio MCP shim (cli/mcp.py on the `mcp` library), run as its own process
- api.json: ApiRuntime with the Anthropic, OpenAI and Gemini providers (api.py and the three translators)
- tables.json: usage, prices, limits, the runtime table's rules (catalog.py), effort rules

Nothing reads or writes ~/.ferb, ~/.codex, ~/.claude or the Keychain: FERB_HOME and HOME point into a temporary
folder, the process environment is replaced by a fixed one for each scenario, no real program is started (except the
shim, which talks only to the fake daemon socket here) and no provider is called. All data is invented. Time is a
constant clock, ids are sequential, the date is fixed: the output is the same bytes on every run.
"""

import asyncio
import base64
import json
import os
import re
import shutil
import sys
import tempfile
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
ROOT = Path(tempfile.mkdtemp(prefix="ferbrt-"))      # short: socket paths are limited to 104 bytes
os.environ["FERB_HOME"] = str(ROOT / "home")
os.environ["HOME"] = str(ROOT / "user")
(ROOT / "user").mkdir()
(ROOT / "tmp").mkdir()
tempfile.tempdir = str(ROOT / "tmp")
sys.path.insert(0, str(REPO))

import ferb.runtimes.codex_exec as codex_exec  # noqa: E402
import ferb.runtimes.codex_stream as codex_stream  # noqa: E402
import ferb.runtimes.usage as usage_mod  # noqa: E402
from ferb.core.types import Identity, ModelImage, ModelRequest, Task  # noqa: E402
from tests.support.runtime_fakes import Pipe  # noqa: E402

TASK = Task('task-1', None, 'reply', 'device-1', Identity.OWNER, set(), 'mixed', 'req-1', [], None)
TODAY = "2026-11-13"
BASE_ENV = {"PATH": "/usr/bin:/bin", "LANG": "en_US.UTF-8", "OPENAI_API_KEY": "placeholder-not-a-key",
            "CODEX_HOME": "/elsewhere/codex", "CLAUDECODE": "1", "ANTHROPIC_API_KEY": "placeholder-not-a-key",
            "FERB_FIXTURE": "kept"}


class FixedDate:
    @staticmethod
    def today() -> Any:
        class D:
            def isoformat(self) -> str:
                return TODAY
        return D()


usage_mod.date = FixedDate  # type: ignore[assignment]


class Counter:
    """secrets.token_hex(n) and friends, sequential."""
    def __init__(self) -> None:
        self.n = 0

    def hex(self, n: int = 16) -> str:
        self.n += 1
        return format(self.n, f"0{2 * n}x")


def normalize(value: Any, home: Path) -> Any:
    if isinstance(value, str):
        value = value.replace(str(home), "$FERB").replace(str(ROOT / "tmp"), "$TMP").replace(str(ROOT), "$ROOT")
        value = re.sub(r"ferb-codex-run-[A-Za-z0-9_]+", "ferb-codex-run-X", value)
        return re.sub(r"ferb-claude-run-[A-Za-z0-9_]+", "ferb-claude-run-X", value)
    if isinstance(value, list):
        return [normalize(v, home) for v in value]
    if isinstance(value, dict):
        return {normalize(k, home): normalize(v, home) for k, v in value.items()}
    return value


def request_of(spec: dict[str, Any]) -> ModelRequest:
    parts = list(spec.get("parts", []))
    if spec.get("huge"):
        parts.append("a" * spec["huge"])
    images = tuple(ModelImage(base64.b64decode(i["data_b64"]), i["ref"], i.get("mime", "image/jpeg"))
                   for i in spec.get("images", []))
    return ModelRequest(spec.get("system", ""), tuple(parts), spec.get("cacheable", 0), images)


def event_text(event: Any) -> str:
    return json.dumps({"kind": event.kind, "data": event.data})


def raised(e: BaseException) -> list[str]:
    return [type(e).__name__, str(e)]


def write_files(home: Path, files: dict[str, str]) -> None:
    for rel, text in files.items():
        path = home / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)


def substitute(value: Any, msg: dict[str, Any] | None) -> Any:
    if value == "$ID" and msg is not None:
        return msg.get("id") if "id" in msg else msg.get("request_id")
    if value == "$REQ" and msg is not None:
        return (msg.get("request_id"))
    if value == "$UUID" and msg is not None:
        return msg.get("uuid")
    if value == "$TEXT" and msg is not None:
        return msg.get("message", {}).get("content")
    if isinstance(value, list):
        return [substitute(v, msg) for v in value]
    if isinstance(value, dict):
        return {k: substitute(v, msg) for k, v in value.items()}
    return value


# ---------------------------------------------------------------- Codex

class FakeApp:
    """The pinned app-server stand-in: each stdin frame's `method` picks the next reaction from the script."""

    def __init__(self, script: dict[str, Any], env: dict[str, str]) -> None:
        self.stdout, self.stderr = Pipe(), Pipe()
        self.stdin = self
        self.returncode: int | None = None
        self.lines: list[str] = []
        self.counts: dict[str, int] = {}
        self.script = script
        self.env = env
        self.finished = asyncio.Event()

    def write(self, data: bytes) -> None:
        self.lines.append(data.decode())
        try:
            msg = json.loads(data)
        except ValueError:
            return
        key = str(msg.get("method"))
        n = self.counts.get(key, 0)
        self.counts[key] = n + 1
        reactions = self.script.get("on", {}).get(key, [])
        if n < len(reactions):
            self.apply(reactions[n], msg)

    def apply(self, actions: list[dict[str, Any]], msg: dict[str, Any] | None = None) -> None:
        for a in actions:
            if "out" in a:
                self.stdout.queue.put_nowait((json.dumps(substitute(a["out"], msg)) + "\n").encode())
            elif "raw" in a:
                self.stdout.queue.put_nowait(a["raw"].encode())
            elif "err" in a:
                self.stderr.queue.put_nowait(a["err"].encode())
            elif "auth" in a:
                Path(self.env["CODEX_HOME"], "auth.json").write_text(a["auth"])
            elif "exit" in a:
                self.kill()

    async def drain(self) -> None:
        pass

    def kill(self) -> None:
        if self.returncode is None:
            self.returncode = -9
            self.stdout.queue.put_nowait(b"")
            self.stderr.queue.put_nowait(b"")
            self.finished.set()

    async def wait(self) -> int | None:
        await self.finished.wait()
        return self.returncode


class VersionProc:
    def __init__(self, text: str) -> None:
        self.text = text
        self.returncode = 0

    async def communicate(self) -> tuple[bytes, None]:
        return self.text.encode(), None


CODEX_SCHEMA = {"type": "object", "properties": {"text": {"type": "string"}}}
AUTH = '{"tokens":{"refresh_token":"test-only-fixture"}}'


def turn_reply(final: str = '{"text":"Hello there"}', phase: Any = "final_answer", tools: Any = None,
               trace: bool = True) -> list[dict[str, Any]]:
    """turn/start's reaction: the request trace on stderr, the ack, the answer's item start and two deltas."""
    tools = tools if tools is not None else [{"type": "function", "name": "apply_patch"}]
    out: list[dict[str, Any]] = []
    if trace:
        out.append({"err": 'TRACE {"tools":' + json.dumps(tools, separators=(",", ":")) + "}\n"})
    half = len(final) // 2
    out += [{"out": {"id": "$ID", "result": {"turn": {"id": "tu", "status": "inProgress"}}}},
            note("item/started", {"item": {"type": "agentMessage", "id": "a", "phase": phase, "text": ""}}),
            note("item/agentMessage/delta", {"itemId": "a", "delta": final[:half]}),
            note("item/agentMessage/delta", {"itemId": "a", "delta": final[half:]})]
    return out


def note(method: str, params: dict[str, Any], thread: str = "th", turn: str | None = "tu") -> dict[str, Any]:
    return {"out": {"method": method, "params": {"threadId": thread, **({"turnId": turn} if turn else {}), **params}}}


def complete(final: str = '{"text":"Hello there"}', phase: Any = "final_answer", status: str = "completed",
             error: Any = None, usage: dict[str, Any] | None = None) -> list[dict[str, Any]]:
    return [note("item/completed", {"item": {"type": "agentMessage", "id": "a", "phase": phase, "text": final}}),
            note("thread/tokenUsage/updated", {"tokenUsage": {"last": usage or {
                "inputTokens": 8, "cachedInputTokens": 2, "outputTokens": 4, "reasoningOutputTokens": 1}}}),
            {"out": {"method": "turn/completed", "params": {"threadId": "th", "turn": {
                "id": "tu", "status": status, "error": error}}}}]


def thread_reply(**change: Any) -> list[dict[str, Any]]:
    result = {"thread": {"id": "th", "ephemeral": True}, "model": "test-model", "modelProvider": "ferb_chatgpt",
              "approvalPolicy": "never", "sandbox": {"type": "readOnly", "networkAccess": False},
              "instructionSources": []}
    result.update(change)
    return [{"out": {"id": "$ID", "result": result}}]


def resume_reply(**change: Any) -> list[dict[str, Any]]:
    result = {"thread": {"id": codex_stream.cache_thread_id("planner", "test-model", "device-1"), "ephemeral": False},
              "model": "test-model", "modelProvider": "ferb_chatgpt", "approvalPolicy": "never",
              "sandbox": {"type": "readOnly", "networkAccess": False}, "instructionSources": []}
    result.update(change)
    return [{"out": {"id": "$ID", "result": result}}]


INIT = [[{"out": {"id": "$ID", "result": {}}}]]


def app(turn: list[list[dict[str, Any]]] | None = None, **more: Any) -> dict[str, Any]:
    on = {"initialize": INIT, "thread/start": [thread_reply()], "turn/start": turn or [turn_reply()]}
    on.update(more)
    return {"on": on}


def sup(**kw: Any) -> dict[str, Any]:
    rt = {"mode": "supervisor", "model": "test-model", "output_schema": CODEX_SCHEMA, "tool_names": [],
          "shim_cmd": None, "effort": None, "timeout_s": None, "cache_role": None, "cache_scope": "",
          "cache_refusal": None, "redact": []}
    rt.update(kw)
    return rt


def worker(**kw: Any) -> dict[str, Any]:
    base = {"mode": "worker", "output_schema": None, "tool_names": ["mac.read"],
            "shim_cmd": ["/opt/ferb/bin/ferb", "mcp"]}
    base.update(kw)
    return sup(**base)


WORKER_TRACE = [{"type": "function", "name": "mcp__ferb.mac_read"}]
REQ = {"system": "FERB instructions", "parts": ["context"]}
ALL = [{"op": "start"}, {"op": "events"}]
COMPLETE = {"op": "push", "actions": complete()}


def codex(name: str, runtime: dict[str, Any], fake: dict[str, Any], ops: list[dict[str, Any]],
          files: dict[str, str] | None = None, request: dict[str, Any] | None = None, run_token: str = "",
          version: str = "codex-cli 0.166.0\n") -> dict[str, Any]:
    return {"name": name, "runtime": runtime, "fake": fake, "ops": ops, "run_token": run_token,
            "files": {"codex-home/auth.json": AUTH} if files is None else files, "request": request or REQ,
            "version": version}


BUNDLED = json.dumps({"models": [{"slug": "test-model", "context_window": 272000,
                                  "effective_context_window_percent": 95, "supported_reasoning_levels": []},
                                 {"slug": "gpt-5.6-sol", "context_window": 272000,
                                  "effective_context_window_percent": 95, "tool_mode": "code"}]})
OFFERED = json.dumps({"fetched_at": "2026-11-01T00:00:00Z", "models": [
    {"slug": "test-model", "context_window": 272000, "effective_context_window_percent": 95,
     "multi_agent_version": "v2", "experimental_supported_tools": ["ask"], "extra": 1.5}]})

CODEX_SCENARIOS: list[dict[str, Any]] = [
    codex("a typed answer streams its deltas, then usage and the final answer", sup(), app(),
          [{"op": "start"}, {"op": "events", "n": 3}, COMPLETE, {"op": "events"}]),
    codex("commentary is never a delta", sup(), app([turn_reply(phase="commentary")]),
          [{"op": "start"}, {"op": "push", "actions": complete(phase="commentary")}, {"op": "events"}]),
    codex("a phaseless answer is the answer but never streams", sup(), app([turn_reply(phase=None)]),
          [{"op": "start"}, {"op": "push", "actions": complete(phase=None)}, {"op": "events"}]),
    codex("a partial answer is not the answer", sup(), app([turn_reply(phase="partial_answer")]),
          [{"op": "start"}, {"op": "push", "actions": complete(phase="partial_answer")}, {"op": "events"}]),
    codex("a tool outside the catalog stops the run before any text",
          sup(), app([turn_reply(tools=[{"type": "function", "name": "shell"},
                                        {"type": "namespace", "name": "mcp__x", "tools": [{"name": "y"}]}])]),
          [{"op": "start"}, {"op": "push", "actions": complete()}, {"op": "events"}]),
    codex("a failed turn emits no final", sup(), app(),
          [{"op": "start"}, {"op": "push", "actions": complete(status="failed", error={"message": "interrupted"})},
           {"op": "events"}]),
    codex("a rejected turn start carries Codex's own error and code",
          sup(redact=[["hunter2", "[redacted]"]]),
          app([[{"out": {"id": "$ID", "error": {"code": -32600, "message": "Input exceeds the maximum length hunter2",
                                                "data": {"input_error_code": "input_too_large",
                                                         "actual_chars": 1048577}, "request": "never kept"}}}]]),
          [{"op": "start"}]),
    codex("a turn error event says Codex's words and its kind",
          sup(), app(),
          [{"op": "start"}, {"op": "push", "actions": [note("error", {"error": {
              "message": "stream disconnected", "codexErrorInfo": {"httpConnectionFailed": {"status": 502}},
              "secret": "never kept"}, "willRetry": False})]}, {"op": "events"}]),
    codex("modelNotFound with no catalogs blames neither", sup(), app(),
          [{"op": "start"}, {"op": "push", "actions": [note("error", {"error": {
              "message": "model not found", "codexErrorInfo": "modelNotFound"}})]}, {"op": "events"}]),
    codex("modelNotFound with the model shipped but not offered blames the plan", sup(model="gpt-5.6-sol"),
          app(**{"thread/start": [thread_reply(model="gpt-5.6-sol")]}),
          [{"op": "start"}, {"op": "push", "actions": [note("error", {"error": {
              "message": "model not found", "codexErrorInfo": "modelNotFound"}})]}, {"op": "events"}],
          files={"codex-home/auth.json": AUTH, "run/codex-bundled-0.166.0.json": BUNDLED,
                 "codex-home/models_cache.json": OFFERED}),
    codex("modelNotFound with a model the pinned Codex lacks blames Codex", sup(model="gpt-9"),
          app(**{"thread/start": [thread_reply(model="gpt-9")]}),
          [{"op": "start"}, {"op": "push", "actions": [note("error", {"error": {
              "message": "model not found", "codexErrorInfo": "modelNotFound"}})]}, {"op": "events"}],
          files={"codex-home/auth.json": AUTH, "run/codex-bundled-0.166.0.json": BUNDLED}),
    codex("contextWindowExceeded is an overflow", sup(), app(),
          [{"op": "start"}, {"op": "push", "actions": [note("error", {"error": {
              "message": "too long", "codexErrorInfo": "contextWindowExceeded", "code": 400}})]}, {"op": "events"}]),
    codex("an error Codex retries itself is reported and the run goes on", sup(), app(),
          [{"op": "start"}, {"op": "push", "actions": [note("error", {"error": {
              "message": "Reconnecting... 1/5", "codexErrorInfo": "responseStreamDisconnected"}, "willRetry": True})]
              + complete()}, {"op": "events"}]),
    codex("cancel cleans up and publishes a refreshed login", sup(),
          app([[{"auth": '{"tokens":{"refresh_token":"refreshed-fixture"}}'}] + turn_reply()]),
          [{"op": "start"}, {"op": "events", "n": 2}, {"op": "cancel"}, {"op": "events"}]),
    codex("a missing catalog never yields a delta or final", sup(), app([turn_reply(trace=False)]),
          [{"op": "start"}, {"op": "push", "actions": complete()}, {"op": "events"}]),
    codex("a late catalog violation is drained before the final", sup(), app([turn_reply()]),
          [{"op": "start"}, {"op": "events", "n": 1},
           {"op": "push", "actions": [{"err": 'TRACE {"tools":[{"type":"function","name":"shell_run"}]}\n'}]
            + complete()}, {"op": "events"}]),
    codex("a worker steers its turn and its tool call is reported", worker(),
          app([turn_reply(tools=WORKER_TRACE)], **{"turn/steer": [[{"out": {"id": "$ID",
                                                                     "result": {"turnId": "tu"}}}]]}),
          [{"op": "start"}, {"op": "steer", "text": "Keep it under ₹300"}, {"op": "events", "n": 2},
           {"op": "push", "actions": [note("item/started", {"item": {
               "id": "m", "type": "mcpToolCall", "server": "ferb", "tool": "mac_read", "arguments": {},
               "status": "inProgress"}}), note("item/completed", {"item": {
                   "id": "m", "type": "mcpToolCall", "server": "ferb", "tool": "mac_read", "arguments": {"x": 1},
                   "status": "completed"}})] + complete()}, {"op": "events"},
           {"op": "steer", "text": "too late"}], run_token="private-token"),
    codex("a refused steer is not accepted", worker(),
          app([turn_reply(tools=WORKER_TRACE)], **{"turn/steer": [[{"out": {"id": "$ID", "error": {
              "code": -32600, "message": "turn mismatch"}}}], [{"out": {"id": "$ID", "result": {"turnId": "other"}}}]]}),
          [{"op": "start"}, {"op": "steer", "text": "one"}, {"op": "steer", "text": "two"}, {"op": "cancel"},
           {"op": "steer", "text": "three"}], run_token="tok"),
    codex("a steer answered with an id nothing can match closes the transport", worker(),
          app([turn_reply(tools=WORKER_TRACE)], **{"turn/steer": [[{"out": {"id": ["invalid-id"], "result": {}}}]]}),
          [{"op": "start"}, {"op": "steer", "text": "one"}, {"op": "events"}], run_token="tok"),
    codex("a supervisor run can't be steered", sup(), app(), [{"op": "start"}, {"op": "steer", "text": "x"},
                                                             {"op": "cancel"}]),
] + [
    codex(f"the item gate stops {label}", worker(), app([turn_reply(tools=WORKER_TRACE)]),
          [{"op": "start"}, {"op": "push", "actions": [note("item/started", {"item": item})] + complete()},
           {"op": "events"}], run_token="tok")
    for label, item in [
        ("an untrusted server", {"id": "m", "type": "mcpToolCall", "server": "untrusted", "tool": "mac_read"}),
        ("an unregistered tool", {"id": "m", "type": "mcpToolCall", "server": "ferb", "tool": "not_registered"}),
        ("a command", {"id": "m", "type": "commandExecution", "command": "touch /tmp/not-permitted"}),
        ("reading a resource", {"id": "m", "type": "mcpToolCall", "server": "codex", "tool": "read_mcp_resource"}),
        ("a kind with no tool", {"id": "m", "type": None}),
        ("a tool that is a number", {"id": "m", "type": "mcpToolCall", "server": None, "tool": 7}),
    ]
] + [
    codex("Codex's own resource listings pass", worker(), app([turn_reply(tools=WORKER_TRACE)]),
          [{"op": "start"}, {"op": "push", "actions": [
              note("item/started", {"item": {"id": "l1", "type": "mcpToolCall", "server": "codex",
                                             "tool": "list_mcp_resources"}}),
              note("item/completed", {"item": {"id": "l2", "type": "mcpToolCall", "server": "ferb",
                                               "tool": "list_mcp_resource_templates", "status": "completed"}}),
              note("item/started", {"item": {"id": "r", "type": "reasoning"}}),
              note("item/started", {"item": {"id": "p", "type": "plan"}})] + complete()},
           {"op": "events"}], run_token="tok"),
    codex("a server-to-client request is a violation naming it", worker(),
          app([turn_reply(tools=WORKER_TRACE) + [{"out": {"id": 99, "method": "item/tool/requestUserInput",
                                                          "params": {}}}]]),
          [{"op": "start"}, {"op": "events"}], run_token="tok"),
] + [
    codex(f"only a ChatGPT login runs ({i})", sup(), app(), [{"op": "start"}],
          files={"codex-home/auth.json": login})
    for i, login in enumerate(['{"OPENAI_API_KEY": "sk-test-only-fixture", "tokens": null}',
                               '{"auth_mode": "apikey", "OPENAI_API_KEY": "sk-test-only-fixture"}',
                               '{"OPENAI_API_KEY": "sk-test-only-fixture", "tokens": {"refresh_token": "x"}}',
                               '{"auth_mode": "ApiKey", "tokens": {}}',
                               '{}', 'not json', '[]', '{"tokens": []}'])
] + [
    codex("no sign-in at all", sup(), app(), [{"op": "start"}], files={}),
] + [
    codex(f"a sign-in the turn refuses ({i})", sup(), app(),
          [{"op": "start"}, {"op": "push", "actions": [note("error", {"error": {**error,
                                                                               "secret": "never kept"}})]},
           {"op": "events"}])
    for i, error in enumerate([{"message": "unauthorized", "codexErrorInfo": "unauthorized"},
                               {"message": "refresh failed", "data": {"code": "refresh_token_expired"}},
                               {"message": "refresh failed", "data": {"error": "invalid_grant"}},
                               {"message": "refresh failed", "data": {"error": "xinvalid_grant"}},
                               {"message": "limit", "codexErrorInfo": "usageLimitExceeded", "code": True},
                               {"message": "upstream temporarily unavailable"}])
] + [
    codex(f"a tool call Codex gave up ({status})", worker(), app([turn_reply(tools=WORKER_TRACE)]),
          [{"op": "start"}, {"op": "push", "actions": [
              note("item/started", {"item": {"id": "call_1", "type": "mcpToolCall", "server": "ferb",
                                             "tool": "mac_read", "arguments": {}, "status": "inProgress"}}),
              note("item/completed", {"item": {"id": "call_1", "type": "mcpToolCall", "server": "ferb",
                                               "tool": "mac_read", "arguments": {}, "status": status}})] + complete()},
           {"op": "events"}], run_token="tok")
    for status in ("failed", "completed")
] + [
    codex("a worker compaction is reported with the context size before it", worker(),
          app([turn_reply(tools=WORKER_TRACE)]),
          [{"op": "start"}, {"op": "push", "actions": [
              note("thread/tokenUsage/updated", {"tokenUsage": {"last": {
                  "inputTokens": 241000, "cachedInputTokens": 0, "outputTokens": 900, "reasoningOutputTokens": 0},
                  "modelContextWindow": 272000}}),
              note("item/started", {"item": {"id": "c1", "type": "contextCompaction"}}),
              note("item/completed", {"item": {"id": "c1", "type": "contextCompaction"}})] + complete()},
           {"op": "events"}], run_token="tok"),
    codex("a cached planner turn resumes its thread with the stable parts as developer instructions",
          sup(cache_role="planner", cache_scope="device-1", effort="medium"),
          app(**{"thread/resume": [resume_reply()]}),
          [{"op": "start"}, {"op": "cancel"}],
          request={"system": "FERB instructions", "parts": ["stable one", "stable two", "changing"], "cacheable": 2}),
    codex("a cached planner turn whose resume Codex refuses takes the ephemeral path",
          sup(cache_role="planner", cache_scope="device-1"),
          app(**{"thread/resume": [[{"out": {"id": "$ID", "error": {"code": -32603, "message": "no rollout"}}}]]}),
          [{"op": "start"}, {"op": "push", "actions": complete()}, {"op": "events"}],
          request={"system": "FERB instructions", "parts": ["stable", "changing"], "cacheable": 1}),
    codex("a cached planner turn the daemon refuses still resumes, its context in the user turn",
          sup(cache_role="planner", cache_scope="device-1", cache_refusal="not an owner's chat turn"),
          app(**{"thread/resume": [resume_reply()]}), [{"op": "start"}, {"op": "cancel"}],
          request={"system": "FERB instructions", "parts": ["stable", "changing"], "cacheable": 1}),
    codex("a resumed thread that isn't isolated takes the ephemeral path",
          sup(cache_role="planner", cache_scope="device-1"),
          app(**{"thread/resume": [resume_reply(instructionSources=["AGENTS.md"])]}),
          [{"op": "start"}, {"op": "cancel"}],
          request={"system": "FERB instructions", "parts": ["stable", "changing"], "cacheable": 1}),
    codex("a thread/start that isn't isolated is refused", sup(),
          app(**{"thread/start": [thread_reply(sandbox={"type": "readOnly", "networkAccess": True})]}),
          [{"op": "start"}]),
    codex("images and an owner's effort go on the turn", sup(effort="high"), app(),
          [{"op": "start"}, {"op": "cancel"}],
          request={"system": "S", "parts": ["a", "b"], "images": [
              {"data_b64": base64.b64encode(b"\x89PNG fake bytes").decode(), "ref": "ev-1", "mime": "image/png"}]}),
    codex("a stalled start ends at the call's limit", sup(timeout_s=0.2), app(**{"thread/start": []}),
          [{"op": "start"}]),
    codex("a worker without the shim can't start", worker(shim_cmd=None), app(), [{"op": "start"}],
          run_token="tok"),
    codex("a Codex that isn't the pinned one is refused", sup(), app(), [{"op": "start"}],
          version="codex-cli 0.150.0\n"),
    codex("an input over Codex's cap is refused before anything starts", sup(), app(), [{"op": "start"}],
          request={"system": "S", "parts": [], "huge": (1 << 20) + 1}),
    codex("an invalid protocol line ends the run", sup(), app([turn_reply() + [{"raw": "{not json\n"}]]),
          [{"op": "start"}, {"op": "events"}]),
    codex("the app-server going away ends the run", sup(), app([turn_reply() + [{"exit": True}]]),
          [{"op": "start"}, {"op": "events"}]),
    codex("a notification whose params aren't an object raises", sup(),
          app([turn_reply() + [{"out": {"method": "item/started", "params": [1]}}]]),
          [{"op": "start"}, {"op": "events"}]),
    codex("tool_search output and namespaces are read from the trace", worker(),
          app([[{"err": 'x output=[{"type":"namespace","name":"mcp__ferb","tools":[{"type":"function",'
                         '"name":"mac_read"}]}] "tools":[{"type":"function","name":"functions.apply_patch"},'
                         '{"type":"web_search"}]\n'}] + turn_reply(tools=WORKER_TRACE)]),
          [{"op": "start"}, {"op": "push", "actions": complete()}, {"op": "events"}], run_token="tok"),
    codex("FERB's locked catalog is handed to Codex", sup(), app(),
          [{"op": "start"}, {"op": "cancel"}],
          files={"codex-home/auth.json": AUTH, "run/codex-bundled-0.166.0.json": BUNDLED,
                 "codex-home/models_cache.json": OFFERED}),
    codex("the run is closed by the consumer mid-stream", sup(), app(),
          [{"op": "start"}, {"op": "events", "n": 1}, {"op": "close"}]),
    codex("the answer is handed on once and a closed run takes no steering", worker(),
          app([turn_reply(tools=WORKER_TRACE)]),
          [{"op": "start"}, {"op": "push", "actions": complete(final='{"no": "schema"}')}, {"op": "events"},
           {"op": "steer", "text": "late"}], run_token="tok"),
]


async def run_codex(sc: dict[str, Any], index: int) -> dict[str, Any]:
    home = ROOT / f"codex{index}" / "home"
    home.mkdir(parents=True)
    write_files(home, sc["files"])
    codex_exec._checked.clear()
    codex_exec._locked.clear()
    ids = Counter()
    codex_stream.secrets.token_hex = ids.hex          # type: ignore[assignment]
    spawns: list[dict[str, Any]] = []
    apps: list[FakeApp] = []
    saved_env = dict(os.environ)
    os.environ.clear()
    os.environ.update(BASE_ENV)

    async def spawn(*cmd: str, **kw: Any) -> Any:
        spawns.append({"argv": list(cmd), "env": dict(sorted((kw.get("env") or dict(os.environ)).items())),
                       "cwd": kw.get("cwd")})
        if len(cmd) > 1 and cmd[1] == "--version":
            return VersionProc(sc["version"])
        proc = FakeApp(sc["fake"], kw["env"])
        apps.append(proc)
        return proc

    original = asyncio.create_subprocess_exec
    asyncio.create_subprocess_exec = spawn                # type: ignore[assignment]
    notes: list[list[str]] = []
    spec = sc["runtime"]

    def redact(text: Any) -> str:
        text = str(text)
        for a, b in spec["redact"]:
            text = text.replace(a, b)
        return text

    rt = codex_exec.CodexRuntime(
        Path("/opt/ferb/codex"), spec["mode"], home, shim_cmd=spec["shim_cmd"], tool_names=spec["tool_names"],
        output_schema=spec["output_schema"], model=spec["model"], timeout_s=spec["timeout_s"],
        on_abandoned=lambda t, c: notes.append(["abandoned", t, c]), redact=redact, cache_role=spec["cache_role"],
        cache_scope=spec["cache_scope"],
        cache_refusal=(lambda task: spec["cache_refusal"]) if spec["cache_refusal"] is not None else None,
        on_cache_skipped=lambda t, w: notes.append(["skipped", t, w]),
        on_cache_fallback=lambda t, w: notes.append(["fallback", t, w]),
        effort=spec["effort"], monotonic=lambda: 0.0)
    log: list[Any] = []
    run = None
    gen = None
    try:
        for op in sc["ops"]:
            kind = op["op"]
            if kind == "start":
                try:
                    run = await rt.start(TASK, request_of(sc["request"]), sc["run_token"])
                    log.append({"op": "start", "start_steps": run.state["start_steps"],
                                "context_role": run.state.get("context_role")})
                except Exception as e:
                    log.append({"op": "start", "raised": raised(e)})
            elif kind == "events":
                gen = gen or rt.events(run)
                count = 0
                while op.get("n") is None or count < op["n"]:
                    try:
                        event = await asyncio.wait_for(gen.__anext__(), 5)
                    except StopAsyncIteration:
                        log.append({"op": "end"})
                        break
                    except Exception as e:
                        log.append({"op": "raised", "raised": raised(e)})
                        break
                    log.append({"op": "event", "event": event_text(event)})
                    count += 1
            elif kind == "steer":
                try:
                    await asyncio.wait_for(rt.continue_(run, op["text"]), 5)
                    log.append({"op": "steer", "ok": True})
                except Exception as e:
                    log.append({"op": "steer", "raised": raised(e)})
            elif kind == "push":
                apps[-1].apply(op["actions"])
                await asyncio.sleep(0)
            elif kind == "cancel":
                await rt.cancel(run)
                log.append({"op": "cancel"})
            elif kind == "close":
                if gen is not None:
                    await gen.aclose()
                log.append({"op": "close"})
    finally:
        asyncio.create_subprocess_exec = original           # type: ignore[assignment]
        if run is not None:
            await codex_stream.cancel(run)
        os.environ.clear()
        os.environ.update(saved_env)
    stdin = [line for proc in apps for line in proc.lines]
    after: dict[str, Any] = {"auth": (home / "codex-home" / "auth.json").read_text()
                             if (home / "codex-home" / "auth.json").exists() else None,
                             "run_folders": sorted(p.name for p in Path(tempfile.gettempdir()).glob("ferb-codex-run-*")),
                             "locked_catalog": (home / "run" / "codex-models.json").read_text()
                             if (home / "run" / "codex-models.json").exists() else None,
                             "user_home": (home / "codex-user").is_dir()}
    return normalize({"name": sc["name"], "spawns": spawns, "stdin": stdin, "log": log, "notes": notes,
                      "after": after}, home)


# ---------------------------------------------------------------- Claude

import anyio  # noqa: E402
import claude_agent_sdk._internal.query as sdk_query  # noqa: E402
import ferb.runtimes.claude_login as claude_login  # noqa: E402
import ferb.runtimes.claude_sdk as claude_sdk  # noqa: E402
import ferb.runtimes.schema_shape as schema_shape  # noqa: E402
from ferb.core import structured  # noqa: E402

# What the Python schema port and the answer check answered, for the Rust test's stand-in of them (the `Forms` seam)
FORMS: dict[str, Any] = {"anthropic": [], "gemini": [], "checks": []}


def _recorded_anthropic(schema: dict[str, Any]) -> Any:
    shape = schema_shape.for_anthropic(schema)
    assert not shape.mapped, "fixtures use schemas whose answers need no mapping"
    if [schema, shape.wire] not in FORMS["anthropic"]:
        FORMS["anthropic"].append([schema, shape.wire])
    return shape


def _recorded_gemini(schema: dict[str, Any]) -> Any:
    shape = schema_shape.for_gemini(schema)
    assert not shape.mapped
    if [schema, shape.wire] not in FORMS["gemini"]:
        FORMS["gemini"].append([schema, shape.wire])
    return shape


def _recorded_check(text: str, schema: Any) -> str:
    try:
        structured.check_final(text, schema)
    except ValueError as e:
        entry = [text, schema, str(e)]
        if entry not in FORMS["checks"]:
            FORMS["checks"].append(entry)
        raise
    entry = [text, schema, None]
    if entry not in FORMS["checks"]:
        FORMS["checks"].append(entry)
    return text


claude_sdk.for_anthropic = _recorded_anthropic        # type: ignore[assignment]
claude_sdk.check_final = _recorded_check              # type: ignore[assignment]


class FakeStdout:
    def __init__(self) -> None:
        self.q: asyncio.Queue[bytes | None] = asyncio.Queue()

    async def receive(self, max_bytes: int = 65536) -> bytes:
        item = await self.q.get()
        if item is None:
            self.q.put_nowait(None)
            raise anyio.EndOfStream
        return item

    async def aclose(self) -> None:
        pass


class FakeStdin:
    def __init__(self, app: "FakeClaude") -> None:
        self.app = app

    async def send(self, data: bytes) -> None:
        self.app.receive(data.decode())

    async def aclose(self) -> None:
        self.app.exit(0)


class FakeClaude:
    """Claude Code's stand-in: each stdin frame (a control request by its subtype, else by its type) picks the next
    reaction; stdin closing ends it with status 0."""

    def __init__(self, script: dict[str, Any], version: str | None = None) -> None:
        self.stdout = FakeStdout()
        self.stdin = FakeStdin(self)
        self.stderr = None
        self.returncode: int | None = None
        self.lines: list[str] = []
        self.counts: dict[str, int] = {}
        self.script = script
        self.finished = asyncio.Event()
        if version is not None:
            self.stdout.q.put_nowait(version.encode())

    def receive(self, line: str) -> None:
        self.lines.append(line)
        try:
            msg = json.loads(line)
        except ValueError:
            return
        kind = msg.get("type")
        key = f"control:{msg.get('request', {}).get('subtype')}" if kind == "control_request" else str(kind)
        n = self.counts.get(key, 0)
        self.counts[key] = n + 1
        reactions = self.script.get("on", {}).get(key, [])
        if n < len(reactions):
            self.apply(reactions[n], msg)

    def apply(self, actions: list[dict[str, Any]], msg: dict[str, Any] | None = None) -> None:
        for a in actions:
            if "out" in a:
                self.stdout.q.put_nowait((json.dumps(substitute(a["out"], msg)) + "\n").encode())
            elif "raw" in a:
                self.stdout.q.put_nowait(a["raw"].encode())
            elif "exit" in a:
                self.exit(int(a["exit"]))

    def exit(self, code: int) -> None:
        if self.returncode is None:
            self.returncode = code
            self.stdout.q.put_nowait(None)
            self.finished.set()

    def terminate(self) -> None:
        self.exit(-15)

    def kill(self) -> None:
        self.exit(-9)

    def send_signal(self, sig: int) -> None:
        self.exit(-sig)

    async def wait(self) -> int | None:
        await self.finished.wait()
        return self.returncode


class FakeProxy:
    def __init__(self, seen: list[str], notes: list[Any]) -> None:
        self._seen, self.notes = set(seen), notes

    def base_url(self, run_id: str) -> str:
        return f"http://127.0.0.1:9/run/{run_id}/fixed-secret"

    def seen(self, run_id: str) -> set[str]:
        return set(self._seen)

    def forget(self, run_id: str) -> None:
        self.notes.append(["forget", run_id])


STRICT = {"type": "object", "properties": {"text": {"type": "string"}, "n": {"type": "integer"}},
          "required": ["text", "n"], "additionalProperties": False}
INIT_OK = [{"out": {"type": "control_response", "response": {"subtype": "success", "request_id": "$ID",
                                                               "response": {"commands": []}}}}]


def sys_init(source: str = "none") -> dict[str, Any]:
    return {"out": {"type": "system", "subtype": "init", "apiKeySource": source, "session_id": "s", "uuid": "u0",
                    "tools": ["mcp__ferb__mac_read"]}}


def assistant(*blocks: dict[str, Any], error: Any = None) -> dict[str, Any]:
    out = {"type": "assistant", "message": {"model": "claude-opus-5-5", "id": "msg", "content": list(blocks),
                                            "stop_reason": None}, "session_id": "s", "uuid": "ua",
           "parent_tool_use_id": None}
    if error is not None:
        out["error"] = error
    return {"out": out}


def text_block(text: str) -> dict[str, Any]:
    return {"type": "text", "text": text}


def tool_block(name: str, args: dict[str, Any], ident: str = "tu1") -> dict[str, Any]:
    return {"type": "tool_use", "id": ident, "name": name, "input": args}


def tool_result(ident: str = "tu1") -> dict[str, Any]:
    return {"out": {"type": "user", "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": ident, "content": "ok"}]}, "session_id": "s", "uuid": "ur",
        "parent_tool_use_id": None}}


def replay(text: str = "$TEXT") -> dict[str, Any]:
    return {"out": {"type": "user", "message": {"role": "user", "content": text}, "uuid": "$UUID", "session_id": "s",
                    "parent_tool_use_id": None}}


def result(text: Any = "done", **more: Any) -> dict[str, Any]:
    out = {"type": "result", "subtype": "success", "duration_ms": 1, "duration_api_ms": 1, "is_error": False,
           "num_turns": 1, "session_id": "s", "result": text, "total_cost_usd": 0.0125,
           "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 2,
                     "cache_creation_input_tokens": 3, "cache_creation": {"ephemeral_1h_input_tokens": 3}},
           "stop_reason": "end_turn"}
    out.update(more)
    return {"out": out}


def stream(event: dict[str, Any], parent: Any = None) -> dict[str, Any]:
    return {"out": {"type": "stream_event", "uuid": "us", "session_id": "s", "event": event,
                    "parent_tool_use_id": parent}}


def cl_rt(**kw: Any) -> dict[str, Any]:
    rt = {"mode": "worker", "model": "claude-opus-5-5", "output_schema": None, "tool_names": ["mac.read"],
          "shim_cmd": ["/opt/ferb/bin/ferb", "mcp"], "effort": None, "limit_s": None, "signed_in": True,
          "cli": True, "seen": []}
    rt.update(kw)
    return rt


def cl_sup(**kw: Any) -> dict[str, Any]:
    base = {"mode": "supervisor", "output_schema": STRICT, "tool_names": [], "shim_cmd": None}
    base.update(kw)
    return cl_rt(**base)


def claude(name: str, runtime: dict[str, Any], on_user: list[list[dict[str, Any]]] | None = None,
           ops: list[dict[str, Any]] | None = None, run_token: str = "tok", request: dict[str, Any] | None = None,
           apps: list[dict[str, Any]] | None = None, init: Any = None) -> dict[str, Any]:
    on = {"control:initialize": [init if init is not None else INIT_OK], "user": on_user or []}
    return {"name": name, "runtime": runtime, "fake": {"apps": apps or [{"on": on}]},
            "ops": ops or [{"op": "start"}, {"op": "events"}, {"op": "cancel"}], "run_token": run_token,
            "request": request or {"system": "FERB instructions", "parts": ["stable", "changing"], "cacheable": 1},
            "version": "2.1.310 (Claude Code)\n"}


ANSWER = '{"text": "hi", "n": 2}'
CLAUDE_SCENARIOS: list[dict[str, Any]] = [
    claude("a worker calls FERB's tool, reports, and answers", cl_rt(seen=["mcp__ferb__mac_read"]),
           [[sys_init(), assistant(tool_block("mcp__ferb__mac_read", {"path": "/x"})), tool_result(),
             assistant({"type": "thinking", "thinking": "hm", "signature": "sig"}, text_block("Found it.")),
             result("done")]]),
    claude("a tool the run wasn't given counts as a wrong call", cl_rt(),
           [[sys_init(), assistant(tool_block("Bash", {"command": "id"})), result("ok")]]),
    claude("a typed answer streams only the StructuredOutput payload", cl_sup(), [[
        sys_init(), stream({"type": "message_start", "message": {}}),
        stream({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking"}}),
        stream({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "x"}}),
        stream({"type": "content_block_stop", "index": 0}),
        stream({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use",
                                                                              "name": "StructuredOutput"}}),
        stream({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta",
                                                                      "partial_json": '{"text": "h'}}),
        stream({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta",
                                                                      "partial_json": 'i", "n": 2}'}}),
        stream({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta",
                                                                      "partial_json": ""}}),
        stream({"type": "content_block_stop", "index": 1}),
        stream({"type": "content_block_start", "index": 2, "content_block": {"type": "text"}}, parent="sub"),
        assistant(tool_block("StructuredOutput", {"text": "hi", "n": 2})),
        result("", structured_output={"text": "hi", "n": 2})]], run_token=""),
    claude("an API key source is refused", cl_rt(), [[sys_init("ANTHROPIC_API_KEY"), result("x")]]),
    claude("a correction waits for its own result, past a stale one", cl_rt(), [[sys_init()],
           [result("stale"), replay("Keep it under 300"), result("updated")]],
           [{"op": "start"}, {"op": "steer", "text": "Keep it under 300"}, {"op": "events"}, {"op": "steer",
                                                                                              "text": "late"},
            {"op": "cancel"}]),
    claude("two corrections in a row need one final", cl_rt(), [[sys_init()], [replay()], [
        replay(), result("both")]],
           [{"op": "start"}, {"op": "steer", "text": "one"}, {"op": "steer", "text": "two"}, {"op": "events"},
            {"op": "cancel"}]),
    claude("an empty answer is asked once and then answers", cl_rt(),
           [[sys_init(), result("")], [replay(), result("now done")]]),
    claude("an empty answer twice ends the run marked empty", cl_rt(),
           [[sys_init(), result("  ")], [replay(), result("")]]),
    claude("a refused typed format starts again stated", cl_sup(), run_token="", apps=[
        {"on": {"control:initialize": [INIT_OK], "user": [[sys_init(), result(
            "API Error: 400 The compiled grammar is too large", is_error=True, api_error_status=400)]]}},
        {"on": {"control:initialize": [INIT_OK], "user": [[sys_init(), assistant(text_block(ANSWER)),
                                                           result("```json\n" + ANSWER + "\n```")]]}}]),
    claude("a stated answer outside the form is told back once", cl_sup(), run_token="", apps=[
        {"on": {"control:initialize": [INIT_OK], "user": [[sys_init(), result(
            "API Error: 400", is_error=True, api_error_status=400)]]}},
        {"on": {"control:initialize": [INIT_OK], "user": [
            [sys_init(), result('{"text": "hi"}')], [replay(), result(ANSWER)]]}}]),
    claude("an error result carries the account fact", cl_rt(), [[
        sys_init(), assistant(text_block("x"), error="rate_limit"),
        result("API Error: 429", is_error=True, api_error_status=429)]]),
    claude("an error result with no words says its errors", cl_rt(), [[
        sys_init(), result(None, is_error=True, subtype="error_during_execution", errors=["boom", "it's"])]]),
    claude("answers never within the schema are a format repair", cl_sup(), run_token="", on_user=[[
        sys_init(), result(None, is_error=True, subtype="error_max_structured_output_retries")]]),
    claude("a tool outside the run's list in the API request stops it", cl_rt(seen=["mcp__ferb__mac_read",
                                                                                  "Bash"]),
           [[sys_init(), result("ok")]]),
    claude("not signed in: nothing starts", cl_rt(signed_in=False), [], [{"op": "start"}]),
    claude("no bundled Claude Code: nothing starts", cl_rt(cli=False), [], [{"op": "start"}]),
    claude("a worker without the shim can't start", cl_rt(shim_cmd=None), [], [{"op": "start"}]),
    claude("a stalled start ends at the call's limit", cl_rt(limit_s=0.2), [], [{"op": "start"}], init=[]),
    claude("an initialize the program refuses", cl_rt(), [], [{"op": "start"}], init=[{"out": {
        "type": "control_response", "response": {"subtype": "error", "request_id": "$ID", "error": "bad init"}}}]),
    claude("cancel mid-stream ends the run", cl_rt(), [[sys_init(), assistant(text_block("working"))]],
           [{"op": "start"}, {"op": "events", "n": 1}, {"op": "cancel"}, {"op": "events"},
            {"op": "steer", "text": "x"}]),
    claude("the program's own control requests are answered with errors", cl_rt(), [[
        {"out": {"type": "control_request", "request_id": "c1", "request": {"subtype": "can_use_tool",
                                                                            "tool_name": "Bash", "input": {}}}},
        {"out": {"type": "control_request", "request_id": "c2", "request": {"subtype": "mcp_message",
                                                                            "server_name": "x",
                                                                            "message": {"id": 4}}}},
        {"out": {"type": "control_request", "request_id": "c3", "request": {"subtype": "other"}}},
        {"out": {"type": "system", "subtype": "session_state_changed", "state": "idle", "sdk_host_only": True}},
        {"out": {"type": "rate_limit_event", "rate_limit_info": {"status": "allowed"}, "uuid": "x",
                 "session_id": "s"}},
        {"raw": "[SandboxDebug] not JSON\n"}, {"out": {"type": "mystery"}},
        sys_init(), result("fine")]]),
    claude("the program ending with an error status mid-run", cl_rt(), [[sys_init(), {"exit": 1}]]),
    claude("an assistant message without its model", cl_rt(), [[sys_init(), {"out": {
        "type": "assistant", "message": {"content": []}}}]]),
    claude("a line that isn't JSON though it looks like it", cl_rt(), [[sys_init(), {"raw": "{broken\n"}]]),
    claude("a planner turn with lookups, images, an alias and an effort", cl_sup(model="opus", effort="low",
                                                                                 tool_names=["chat.history"],
                                                                                 shim_cmd=["/f", "mcp"]),
           [[sys_init(), assistant(tool_block("StructuredOutput", {"text": "x", "n": 1})), result(
               "", structured_output=None)]], run_token="tok",
           request={"system": "", "parts": ["a", "", "c"], "cacheable": 2,
                    "images": [{"data_b64": base64.b64encode(b"img").decode(), "ref": "r", "mime": "image/png"}]}),
]


async def run_claude(sc: dict[str, Any], index: int) -> dict[str, Any]:
    home = ROOT / f"claude{index}" / "home"
    home.mkdir(parents=True)
    spec = sc["runtime"]
    if spec["signed_in"]:
        (home / "claude-home").mkdir()
        (home / "claude-home" / ".ferb-signed-in").write_text("x\n")
    ids, uuids = Counter(), Counter()
    import secrets as secrets_mod
    import uuid as uuid_mod
    saved = (secrets_mod.token_hex, uuid_mod.uuid4, sdk_query.os.urandom, anyio.open_process,
             claude_login.cli_binary)
    secrets_mod.token_hex = ids.hex                       # type: ignore[assignment]
    def next_uuid() -> Any:
        uuids.n += 1
        return uuid_mod.UUID(int=uuids.n)
    uuid_mod.uuid4 = next_uuid                            # type: ignore[assignment]
    sdk_query.os.urandom = lambda n: bytes([0xAB, 0xCD, 0xEF, 0x01][:n])   # type: ignore[assignment]

    def cli() -> Path:
        if not spec["cli"]:
            from ferb.core.errors import Unsupported
            raise Unsupported("the Claude Agent SDK's bundled Claude Code is missing")
        return Path("/opt/ferb/claude")
    claude_login.cli_binary = cli                         # type: ignore[assignment]
    spawns: list[dict[str, Any]] = []
    apps: list[FakeClaude] = []
    saved_env = dict(os.environ)
    os.environ.clear()
    os.environ.update(BASE_ENV)

    async def open_process(cmd: list[str], **kw: Any) -> Any:
        spawns.append({"argv": list(cmd), "env": dict(sorted((kw.get("env") or dict(os.environ)).items())),
                       "cwd": kw.get("cwd")})
        if len(cmd) == 2 and cmd[1] == "-v":
            return FakeClaude({}, sc["version"])
        scripts = sc["fake"]["apps"]
        proc = FakeClaude(scripts[min(len(apps), len(scripts) - 1)])
        apps.append(proc)
        return proc
    anyio.open_process = open_process                     # type: ignore[assignment]
    notes: list[Any] = []
    rt = claude_sdk.ClaudeRuntime(spec["mode"], FakeProxy(spec["seen"], notes), str(home),
                                  shim_cmd=spec["shim_cmd"], tool_names=spec["tool_names"],
                                  output_schema=spec["output_schema"], model=spec["model"], effort=spec["effort"],
                                  limit_s=spec["limit_s"], monotonic=lambda: 0.0,
                                  refusals=schema_shape.Refusals(lambda: 0.0))
    log: list[Any] = []
    run = None
    gen = None
    try:
        for op in sc["ops"]:
            kind = op["op"]
            if kind == "start":
                try:
                    run = await rt.start(TASK, request_of(sc["request"]), sc["run_token"])
                    log.append({"op": "start", "start_steps": run.state["start_steps"], "typed": run.state["typed"]})
                except Exception as e:
                    log.append({"op": "start", "raised": raised(e)})
            elif kind == "events":
                gen = gen or rt.events(run)
                count = 0
                while op.get("n") is None or count < op["n"]:
                    try:
                        event = await asyncio.wait_for(gen.__anext__(), 5)
                    except StopAsyncIteration:
                        log.append({"op": "end"})
                        break
                    except Exception as e:
                        log.append({"op": "raised", "raised": raised(e)})
                        break
                    log.append({"op": "event", "event": event_text(event)})
                    count += 1
            elif kind == "steer":
                try:
                    await asyncio.wait_for(rt.continue_(run, op["text"]), 5)
                    log.append({"op": "steer", "ok": True})
                except Exception as e:
                    log.append({"op": "steer", "raised": raised(e)})
            elif kind == "cancel":
                await rt.cancel(run)
                log.append({"op": "cancel"})
    finally:
        if run is not None:
            await rt.cancel(run)
        (secrets_mod.token_hex, uuid_mod.uuid4, sdk_query.os.urandom, anyio.open_process,
         claude_login.cli_binary) = saved
        os.environ.clear()
        os.environ.update(saved_env)
    stdin = [line for proc in apps for line in proc.lines]
    after = {"token_folders": sorted(p.name for p in Path(tempfile.gettempdir()).glob("ferb-claude-run-*"))}
    return normalize({"name": sc["name"], "spawns": spawns, "stdin": stdin, "log": log, "notes": notes,
                      "after": after}, home)


# ---------------------------------------------------------------- the tool bridge (cli/mcp.py)

FS_LIST = {"name": "fs.list", "description": "List a folder — naïve ✓.",
           "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"],
                            "additionalProperties": False}}
MAC_READ = {"name": "mac.read_text", "description": "Read the front window.",
            "input_schema": {"type": "object", "properties": {}}}
OK = {"content": {"entries": ["a.txt", "ünï"], "n": 2.5}, "footer": "[from: fs.list]", "is_error": False,
      "image": None}


def mcp_line(ident: Any, method: str, params: Any = None) -> str:
    msg: dict[str, Any] = {"jsonrpc": "2.0", "method": method}
    if ident is not None:
        msg = {"jsonrpc": "2.0", "id": ident, "method": method}
    if params is not None:
        msg["params"] = params
    return json.dumps(msg) + "\n"


INITIALIZE = mcp_line(0, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {"elicitation": {}},
                                        "clientInfo": {"name": "codex-mcp-client", "title": "Codex",
                                                       "version": "0.166.0"}})
INITIALIZED = mcp_line(None, "notifications/initialized")


def call(ident: int, name: str, args: Any, meta: Any = None) -> str:
    params: dict[str, Any] = {"name": name, "arguments": args}
    if meta is not None:
        params["_meta"] = meta
    return mcp_line(ident, "tools/call", params)


MCP_SCENARIOS: list[dict[str, Any]] = [
    {"name": "the handshake, the run's tools and a call through the gateway",
     "token": {"file": "token-from-file\n"}, "tools": [FS_LIST, MAC_READ],
     "calls": [{"result": OK}],
     "stdin": [INITIALIZE, INITIALIZED, mcp_line(1, "tools/list"),
               call(2, "fs_list", {"path": "/tmp"}, {"threadId": "th", "callId": "call_1",
                                                     "progressToken": 7})]},
    {"name": "a refusal, a broken connection, an image, an unknown name and empty arguments",
     "token": {"env": "token-from-env"}, "tools": [FS_LIST, MAC_READ],
     "calls": [{"error": {"code": -32004, "message": "the check said no"}}, {"break": True},
               {"result": {"content": "shot", "footer": "", "is_error": True,
                           "image": {"data": "aGVsbG8=", "mime": "image/png"}}},
               {"result": {"content": None, "footer": "f", "is_error": False, "image": None}},
               {"result": OK}],
     "stdin": [INITIALIZE, INITIALIZED, mcp_line(1, "tools/list"), call(2, "fs_list", {"path": "/"}),
               call(3, "fs_list", {"path": "/"}), call(4, "mac_read_text", None),
               call(5, "shell_run_now", {}), call(6, "fs_list", {"path": "/x"}, {"threadId": "", "callId": "c"})]},
    {"name": "a call before the list, resources, ping, an unknown method and a bad line",
     "token": {"env": "tok"}, "tools": [FS_LIST], "calls": [{"result": OK}, {"result": OK}],
     "stdin": [INITIALIZE, INITIALIZED, call(1, "fs_list", {"path": "/"}), mcp_line(2, "resources/list"),
               mcp_line(3, "resources/templates/list"), mcp_line(4, "ping"), mcp_line(5, "prompts/list"),
               mcp_line(6, "nope/never"), "{not json\n", mcp_line(7, "tools/list", {"cursor": "x"}),
               call(8, "fs_list", {"path": 5})]},
    {"name": "an older client's protocol version", "token": {"env": "tok"}, "tools": [],
     "calls": [], "stdin": [mcp_line(0, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                                       "clientInfo": {"name": "claude-code", "version": "2.1.310"}}),
                            INITIALIZED, mcp_line(1, "tools/list")]},
    {"name": "edges: versions, ids, bad params, schemas with more keys", "token": {"env": "tok"},
     "tools": [{"name": "a.b_c", "description": "", "input_schema": {
         "title": "In", "$defs": {"X": {"type": "string"}}, "type": "object", "description": "d",
         "properties": {"x": {"$ref": "#/$defs/X"}}, "required": ["x"], "additionalProperties": False}},
               {"name": "plain", "description": "p", "input_schema": {"type": "object"}}],
     "calls": [{"result": OK}],
     "stdin": [mcp_line("s1", "tools/list"),
               mcp_line(0, "initialize", {"protocolVersion": "1999-01-01", "capabilities": {},
                                          "clientInfo": {"name": "x", "version": "1"}}),
               mcp_line(1, "initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                                          "clientInfo": {"name": "x", "version": "1"}}),
               INITIALIZED, mcp_line("two", "tools/list"), mcp_line(3, "tools/call", {}),
               mcp_line(4, "tools/call", {"name": 5}), mcp_line(5, "tools/call", "x"),
               json.dumps({"jsonrpc": "2.0", "id": 77, "result": {}}) + "\n",
               call(6, "a_b_c", {"x": "y"}), mcp_line(7, "initialize", {"capabilities": {}})]},
    {"name": "the daemon refuses the list; a call the runtime gives up is closed", "token": {"env": "tok"},
     "tools": None, "calls": [{"hang": True}, {"result": OK}],
     "stdin": [INITIALIZE, INITIALIZED, mcp_line(1, "tools/list"), call(2, "fs_list", {"path": "/"}),
               mcp_line(None, "notifications/cancelled", {"requestId": 2, "reason": "timeout"}),
               mcp_line(3, "ping"), call(4, "fs_list", {"path": "/"})]},
    {"name": "no token: the bridge refuses to start", "token": {}, "tools": [], "calls": [], "stdin": []},
]


async def run_mcp(sc: dict[str, Any], index: int) -> dict[str, Any]:
    home = ROOT / f"m{index}"
    (home / "run").mkdir(parents=True)
    sock = home / "run" / "ferb.sock"
    connections: list[list[str]] = []
    calls = list(sc["calls"])

    async def serve(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        lines: list[str] = []
        connections.append(lines)
        while True:
            raw = await reader.readline()
            if not raw:
                break
            lines.append(raw.decode())
            msg = json.loads(raw)
            method, ident = msg.get("method"), msg.get("id")
            if method == "auth.hello":
                reply: Any = {"jsonrpc": "2.0", "id": ident, "result": {"ok": True, "role": "mcp_shim"}}
            elif method == "tools.list" and sc["tools"] is None:
                reply = {"jsonrpc": "2.0", "id": ident, "error": {"code": -32002, "message": "run token revoked"}}
            elif method == "tools.list":
                reply = {"jsonrpc": "2.0", "id": ident, "result": {"tools": sc["tools"]}}
            elif method == "tools.call":
                step = calls.pop(0)
                if step.get("hang"):
                    while await reader.readline():
                        pass
                    lines.append("<closed>")
                    writer.close()
                    return
                if step.get("break"):
                    writer.close()
                    return
                reply = ({"jsonrpc": "2.0", "id": ident, "error": step["error"]} if "error" in step
                         else {"jsonrpc": "2.0", "id": ident, "result": step["result"]})
            else:
                reply = {"jsonrpc": "2.0", "id": ident, "error": {"code": -32601, "message": "method not found"}}
            writer.write((json.dumps(reply) + "\n").encode())
            await writer.drain()
        writer.close()

    server = await asyncio.start_unix_server(serve, path=str(sock))
    env = {"PATH": "/usr/bin:/bin", "FERB_HOME": str(home), "PYTHONPATH": str(REPO), "HOME": str(ROOT / "user")}
    token = sc["token"]
    if "file" in token:
        (home / "token").write_text(token["file"])
        env["FERB_RUN_TOKEN_FILE"] = str(home / "token")
    if "env" in token:
        env["FERB_RUN_TOKEN"] = token["env"]
    proc = await asyncio.create_subprocess_exec(sys.executable, "-c", "from ferb.cli.mcp import main; main()",
                                                stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
                                                stderr=asyncio.subprocess.PIPE, env=env, cwd=str(home))
    assert proc.stdin and proc.stdout and proc.stderr
    out: list[str] = []
    try:
        for line in sc["stdin"]:
            proc.stdin.write(line.encode())
            await proc.stdin.drain()
            try:
                ident = json.loads(line).get("id")
            except ValueError:
                ident = "bad"
            if ident is None:
                await asyncio.sleep(0.3)               # a notification: nothing comes back
                continue
            while True:
                try:
                    got = await asyncio.wait_for(proc.stdout.readline(), 3)
                except TimeoutError:
                    break                              # nothing comes back for it
                if not got:
                    break
                out.append(got.decode())
                reply = json.loads(got)
                if ident == "bad" or reply.get("id") == ident:
                    break
        proc.stdin.close()
        rest = await asyncio.wait_for(proc.stdout.read(), 15)
        if rest:
            out.extend(rest.decode().splitlines(keepends=True))
        code = await asyncio.wait_for(proc.wait(), 15)
        err = (await proc.stderr.read()).decode()
    finally:
        if proc.returncode is None:
            proc.kill()
            await proc.wait()
        server.close()
    return {"name": sc["name"], "stdout": out, "daemon": connections, "exit": code,
            "stderr_last": err.strip().splitlines()[-1] if err.strip() else ""}


# ---------------------------------------------------------------- the direct APIs

import httpx  # noqa: E402

import ferb.runtimes.anthropic_api as anthropic_api  # noqa: E402
import ferb.runtimes.api as api_mod  # noqa: E402
import ferb.runtimes.google_api as google_api  # noqa: E402
import ferb.runtimes.openai_api as openai_api  # noqa: E402
from ferb.core.errors import Refused  # noqa: E402

api_mod.check_final = _recorded_check                  # type: ignore[assignment]
anthropic_api.for_anthropic = _recorded_anthropic      # type: ignore[assignment]
google_api.for_gemini = _recorded_gemini               # type: ignore[assignment]

KEY = "fixture-key-0000"
API_SCHEMA = {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"],
              "additionalProperties": False}
LOOKUP = {"name": "memory.read", "description": "Read memory — ünï.",
          "input_schema": {"type": "object", "properties": {"query": {"type": "string"}, "n": {"$ref": "#/$defs/N"}},
                           "$defs": {"N": {"type": "integer", "minimum": 1}}}}
FS = {"name": "fs.list", "description": "List a folder.",
      "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}
RESULT = {"content": {"entries": ["a.txt"], "size": 1.5}, "footer": "[fs]", "is_error": False, "image": None}
IMAGE_RESULT = {"content": "shot", "footer": "", "is_error": False, "image": {"data": "aGVsbG8=", "mime": "image/png"}}
HEADERS = ("anthropic-version", "content-type", "x-api-key", "authorization", "x-goog-api-key")


def ok_text(provider: str, text: str, usage: Any = None) -> dict[str, Any]:
    if provider == "anthropic":
        return {"json": {"content": [{"type": "text", "text": text}], "stop_reason": "end_turn",
                         "usage": usage or {"input_tokens": 5, "output_tokens": 3, "cache_read_input_tokens": 100,
                                            "cache_creation_input_tokens": 50,
                                            "cache_creation": {"ephemeral_5m_input_tokens": 10,
                                                               "ephemeral_1h_input_tokens": 40}}}}
    if provider == "openai":
        return {"json": {"status": "completed", "output": [
            {"type": "reasoning", "id": "rs_1", "encrypted_content": "enc"},
            {"type": "message", "content": [{"type": "output_text", "text": text}]}],
            "usage": usage or {"input_tokens": 120, "output_tokens": 9,
                               "input_tokens_details": {"cached_tokens": 100, "cache_write_tokens": 20},
                               "output_tokens_details": {"reasoning_tokens": 4}}}}
    return {"json": {"candidates": [{"content": {"role": "model", "parts": [
        {"text": "thinking", "thought": True, "thoughtSignature": "sig"}, {"text": text}]}, "finishReason": "STOP"}],
        "usageMetadata": usage or {"promptTokenCount": 130, "cachedContentTokenCount": 100,
                                   "candidatesTokenCount": 7, "thoughtsTokenCount": 3}}}


def ok_tool(provider: str, name: str, args: Any, ident: str = "call_1", extra_text: str = "") -> dict[str, Any]:
    if provider == "anthropic":
        content = ([{"type": "text", "text": extra_text}] if extra_text else []) + [
            {"type": "tool_use", "id": ident, "name": name, "input": args}]
        return {"json": {"content": content, "stop_reason": "tool_use",
                         "usage": {"input_tokens": 5, "output_tokens": 3}}}
    if provider == "openai":
        return {"json": {"status": "completed", "output": [
            {"type": "function_call", "id": "fc_" + ident, "call_id": ident, "name": name,
             "arguments": args if isinstance(args, str) else json.dumps(args)}],
            "usage": {"input_tokens": 10, "output_tokens": 2}}}
    return {"json": {"candidates": [{"content": {"role": "model", "parts": [
        {"functionCall": {"name": name, "args": args, **({"id": ident} if ident else {})},
         "thoughtSignature": "sig2"}]}, "finishReason": "STOP"}],
        "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 2}}}


def sse_events(provider: str, first: str, last: str) -> list[dict[str, Any]]:
    if provider == "openai":
        return [{"type": "response.reasoning_text.delta", "delta": "private"},
                {"type": "response.output_text.delta", "delta": first},
                {"type": "response.output_text.delta", "delta": last},
                {"type": "response.completed", "response": {"status": "completed", "usage": {"input_tokens": 7},
                 "output": [{"type": "message", "content": [{"type": "output_text", "text": first + last}]}]}}]
    if provider == "anthropic":
        return [{"type": "message_start", "message": {"role": "assistant", "content": [],
                                                      "usage": {"input_tokens": 7}}},
                {"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}},
                {"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "p"}},
                {"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "s"}},
                {"type": "content_block_stop", "index": 0},
                {"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}},
                {"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": first}},
                {"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": last}},
                {"type": "content_block_stop", "index": 1},
                {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 9}},
                {"type": "message_stop"}]
    return [{"candidates": [{"index": 0, "content": {"role": "model", "parts": [
                {"thought": True, "text": "private"}, {"text": first}]}}]},
            {"candidates": [{"index": 0, "content": {"role": "model", "parts": [{"text": last}]},
                             "finishReason": "STOP"}], "usageMetadata": {"promptTokenCount": 7}}]


def answer_tool_events(provider: str, args: str) -> list[dict[str, Any]]:
    if provider == "anthropic":
        return [{"type": "message_start", "message": {"role": "assistant", "content": [], "usage": {}}},
                {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "Let me "}},
                {"type": "content_block_stop", "index": 0},
                {"type": "content_block_start", "index": 1, "content_block": {
                    "type": "tool_use", "id": "ans", "name": "ferb_answer", "input": {}}},
                {"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta",
                                                                       "partial_json": args[:5]}},
                {"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta",
                                                                       "partial_json": args[5:]}},
                {"type": "content_block_stop", "index": 1},
                {"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 4}},
                {"type": "message_stop"}]
    if provider == "openai":
        return [{"type": "response.output_text.delta", "delta": "Let me "},
                {"type": "response.output_item.added", "item": {"type": "function_call", "id": "fc1",
                                                                "name": "ferb_answer"}},
                {"type": "response.function_call_arguments.delta", "item_id": "fc1", "delta": args},
                {"type": "response.completed", "response": {"status": "completed", "output": [
                    {"type": "function_call", "id": "fc1", "call_id": "c1", "name": "ferb_answer",
                     "arguments": args}], "usage": {"input_tokens": 3}}}]
    return [{"candidates": [{"index": 0, "content": {"role": "model", "parts": [
        {"text": "Let me "}, {"functionCall": {"name": "ferb_answer", "args": json.loads(args)}}]},
        "finishReason": "STOP"}]}]


def api_case(name: str, provider: str, responses: list[dict[str, Any]], ops: list[dict[str, Any]] | None = None,
             **kw: Any) -> dict[str, Any]:
    sc = {"name": f"{provider}: {name}", "provider": provider, "model": None, "mode": "worker", "role": "worker",
          "tool_names": ["fs.list"], "schema": API_SCHEMA, "planner": False, "effort": None, "limit_s": None,
          "refused": False, "bridge": {"tools": [FS, LOOKUP], "results": [RESULT]}, "responses": responses,
          "request": {"system": "FERB instructions", "parts": ["stable", "the message"], "cacheable": 1},
          "run_token": "tok", "ops": ops or [{"op": "start"}, {"op": "events"}]}
    sc.update(kw)
    return sc


def sup(**kw: Any) -> dict[str, Any]:
    return {"mode": "supervisor", "role": "orchestrator", "tool_names": [], "run_token": "", **kw}


ANSWER = '{"text": "hi"}'
API_SCENARIOS: list[dict[str, Any]] = []
for P in ("anthropic", "openai", "google"):
    API_SCENARIOS += [
        api_case("a typed answer, unstreamed for a worker", P, [ok_text(P, ANSWER)]),
        api_case("a supervisor's typed answer streams", P, [{"sse": sse_events(P, '{"text": "Your', ' draft"}')}],
                 **sup()),
        api_case("a supervisor's stream split mid-line and with \\r\\n", P,
                 [{"sse": sse_events(P, '{"text": "a', 'b"}'), "split": 7, "crlf": True}], **sup()),
        api_case("a tool call goes through FERB and back", P,
                 [ok_tool(P, "fs_list", {"path": "/tmp"}), ok_text(P, ANSWER)]),
        api_case("an image result reaches the model", P, [ok_tool(P, "fs_list", {"path": "/"}), ok_text(P, ANSWER)],
                 bridge={"tools": [FS], "results": [IMAGE_RESULT]}),
        api_case("a tool the run wasn't offered gets an error and the run goes on", P,
                 [ok_tool(P, "shell_run", {"command": "id"}), ok_text(P, ANSWER)]),
        api_case("FERB's refusal of a call is told back", P, [ok_tool(P, "fs_list", {"path": "/"}), ok_text(P, ANSWER)],
                 bridge={"tools": [FS], "results": [{"refuse": "that folder is protected"}]}),
        api_case("a planner's lookups with a run token, $refs written out", P,
                 [ok_tool(P, "memory_read", {"query": "dentist"}), {"sse": sse_events(P, '{"text": "D', 'r. Rao"}')}],
                 **sup(tool_names=["memory.read", "missing.tool"], run_token="tok", planner=True)),
        api_case("a cut-off reply is an error", P, [
            {"json": {"content": [], "stop_reason": "max_tokens"}} if P == "anthropic" else
            {"json": {"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}}} if P == "openai"
            else {"json": {"candidates": [{"finishReason": "MAX_TOKENS"}]}}]),
        api_case("a refused key is an account fact and its words never carry the key", P,
                 [{"status": 401, "json": {"error": {"message": f"Invalid key {KEY} given", "type": "auth"}}}]),
        api_case("a busy provider is retried with its retry-after, then answers", P,
                 [{"status": 429, "json": {"error": {"message": "slow down"}}, "headers": {"retry-after": "2"}},
                  {"status": 503, "json": {}}, ok_text(P, ANSWER)]),
        api_case("a rate limit that outlasts the retries says when to retry", P,
                 [{"status": 429, "json": {"error": {"message": "slow down"}}, "headers": {"retry-after": "1.5"}}] * 5),
        api_case("a refused typed format is resent stated and remembered", P,
                 [{"status": 400, "json": {"error": {"message": "The compiled grammar is too large",
                                                     "type": "invalid_request_error"}}},
                  ok_text(P, "```json\n" + ANSWER + "\n```")]),
        api_case("a planner whose form is stated answers through the answer tool", P,
                 [{"sse": answer_tool_events(P, ANSWER)}], **sup(planner=True, refused=True)),
        api_case("an answer outside the form is told back once", P,
                 [ok_text(P, '{"txt": 1}'), ok_text(P, ANSWER)], refused=True),
        api_case("an empty answer is asked once, then answers", P, [ok_text(P, ""), ok_text(P, ANSWER)]),
        api_case("an empty answer twice ends the run", P, [ok_text(P, "  "), ok_text(P, "")]),
        api_case("an owner's correction reaches the next call", P,
                 [ok_tool(P, "fs_list", {"path": "/"}), ok_text(P, ANSWER)],
                 ops=[{"op": "start"}, {"op": "events", "n": 2}, {"op": "steer", "text": "only pdfs"},
                      {"op": "events"}, {"op": "steer", "text": "too late"}]),
        api_case("a stream that fails after text is not retried", P,
                 [{"sse": sse_events(P, '{"text": "a', 'b"}')[:{"anthropic": 7, "openai": 2, "google": 1}[P]],
                   "broken": True}], **sup()),
        api_case("a transport failure before any answer is retried", P,
                 [{"transport_error": True}, ok_text(P, ANSWER)]),
        api_case("a reply that isn't JSON", P, [{"raw": "<html>oops</html>", "status": 200}]),
        api_case("a stream with invalid JSON", P, [{"sse_raw": "data: {nope\n\n"}], **sup()),
        api_case("a stream with an event that isn't an object", P, [{"sse_raw": "data: [1]\n\n"}], **sup()),
        api_case("a stream that ends early", P, [{"sse": sse_events(P, '{"text": "a', 'b"}')[:2]}], **sup()),
        api_case("cancel mid-stream", P, [{"sse": sse_events(P, '{"text": "a', 'b"}')[:2], "hang": True}],
                 ops=[{"op": "start"}, {"op": "events", "n": 1}, {"op": "cancel"}, {"op": "events"},
                      {"op": "steer", "text": "x"}], **sup()),
        api_case("a call that outlasts its limit", P, [{"hang_request": True}], limit_s=0.2, **sup()),
        api_case("no key, no run", P, [], ops=[{"op": "start"}], key=None),
        api_case("an owner's effort and images", P, [ok_text(P, ANSWER)], effort="high",
                 request={"system": "", "parts": ["a"], "cacheable": 1, "images": [
                     {"data_b64": base64.b64encode(b"pic").decode(), "ref": "r1", "mime": "image/png"}]}),
    ]
API_SCENARIOS += [
    api_case("an over-long prompt is an overflow", "anthropic", [{"status": 400, "json": {"error": {
        "type": "invalid_request_error", "message": "prompt is too long: 300000 tokens > 200000 maximum"}}}]),
    api_case("an image the API couldn't process", "anthropic", [{"status": 400, "json": {"error": {
        "type": "invalid_request_error", "message": "Could not process image"}}}]),
    api_case("early stops are errors", "anthropic", [{"json": {"content": [], "stop_reason": "pause_turn"}}]),
    api_case("a refusal stop is an error", "anthropic", [{"json": {"content": [], "stop_reason": "refusal"}}]),
    api_case("a window stop is an overflow", "anthropic",
             [{"json": {"content": [], "stop_reason": "model_context_window_exceeded"}}]),
    api_case("arguments that aren't JSON are the call's error", "anthropic",
             [ok_tool("anthropic", "fs_list", "{bad"), ok_text("anthropic", ANSWER)]),
    api_case("the window code is an overflow", "openai", [{"status": 400, "json": {"error": {
        "code": "context_length_exceeded", "message": "too long"}}}]),
    api_case("an image code is an image refusal", "openai", [{"status": 400, "json": {"error": {
        "code": "invalid_image", "message": "bad image"}}}]),
    api_case("a failed reply with an image code", "openai", [{"json": {"status": "failed", "error": {
        "code": "image_too_large", "message": "too big"}}}]),
    api_case("a refusal is an error", "openai", [{"json": {"status": "completed", "output": [
        {"type": "message", "content": [{"type": "refusal", "refusal": "no"}]}]}}]),
    api_case("empty or malformed arguments", "openai",
             [ok_tool("openai", "fs_list", "  "), ok_tool("openai", "fs_list", "[1]"), ok_text("openai", ANSWER)]),
    api_case("gpt-5.5 keeps the routing key without breakpoints", "openai", [ok_text("openai", ANSWER)],
             model="gpt-5.5"),
    api_case("a stop other than STOP is an error", "google", [{"json": {"candidates": [{"finishReason": "SAFETY"}]}}]),
    api_case("no candidates names the block reason", "google",
             [{"json": {"promptFeedback": {"blockReason": "OTHER"}}}]),
    api_case("calls without ids get FERB's own", "google",
             [ok_tool("google", "fs_list", {"path": "/"}, ident=""), ok_text("google", ANSWER)]),
    api_case("arguments that aren't an object", "google",
             [ok_tool("google", "fs_list", [1, 2]), ok_text("google", ANSWER)]),
]


class ApiBridge:
    def __init__(self, spec: dict[str, Any], log: list[Any]) -> None:
        self.spec, self.log = spec, log
        self.results = list(spec["results"])

    def tools(self, run_token: str) -> list[dict[str, Any]]:
        return self.spec["tools"]

    async def call(self, run_token: str, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        self.log.append(["call", run_token, name, arguments])
        result = self.results.pop(0) if self.results else RESULT
        if "refuse" in result:
            raise Refused(result["refuse"])
        return result


class Chunks(httpx.AsyncByteStream):
    def __init__(self, chunks: list[bytes], broken: bool, hang: bool) -> None:
        self.chunks, self.broken, self.hang = chunks, broken, hang

    async def __aiter__(self) -> Any:
        for c in self.chunks:
            yield c
        if self.broken:
            raise httpx.RemoteProtocolError(f"peer closed connection; {KEY}")
        if self.hang:
            await asyncio.Event().wait()

    async def aclose(self) -> None:
        pass


def sse_body(item: dict[str, Any]) -> list[bytes]:
    if "sse_raw" in item:
        return [item["sse_raw"].encode()]
    nl = "\r\n" if item.get("crlf") else "\n"
    text = "".join("data: " + json.dumps(e) + nl + nl for e in item["sse"]).encode()
    n = item.get("split")
    return [text[i:i + n] for i in range(0, len(text), n)] if n else [text]


async def run_api(sc: dict[str, Any]) -> dict[str, Any]:
    requests: list[Any] = []
    responses = list(sc["responses"])

    async def handle(request: httpx.Request) -> httpx.Response:
        headers = {k: (v.replace(KEY, "[key]")) for k, v in request.headers.items() if k in HEADERS}
        requests.append({"method": request.method, "path": request.url.raw_path.decode(), "headers": headers,
                         "body": request.content.decode()})
        item = responses.pop(0)
        if item.get("transport_error"):
            raise httpx.ConnectError("connection refused", request=request)
        if item.get("hang_request"):
            await asyncio.Event().wait()
        if "raw" in item:
            return httpx.Response(item["status"], content=item["raw"].encode())
        if "sse" in item or "sse_raw" in item:
            return httpx.Response(200, headers={"content-type": "text/event-stream"},
                                  stream=Chunks(sse_body(item), item.get("broken", False), item.get("hang", False)))
        return httpx.Response(item.get("status", 200), json=item["json"], headers=item.get("headers", {}))

    log: list[Any] = []
    key = sc.get("key", KEY)

    async def get_key(name: str) -> str | None:
        return key

    async def sleep(seconds: float) -> None:
        log.append(["sleep", seconds])

    ids = Counter()
    saved = api_mod.secrets.token_hex
    api_mod.secrets.token_hex = ids.hex                  # type: ignore[assignment]
    provider = {"anthropic": anthropic_api.AnthropicProvider, "openai": openai_api.OpenAIProvider,
                "google": google_api.GoogleProvider}[sc["provider"]](sc["model"])
    refusals = schema_shape.Refusals(lambda: 0.0)
    rt = api_mod.ApiRuntime(provider, mode=sc["mode"], role=sc["role"], tool_names=sc["tool_names"],
                            output_schema=sc["schema"], bridge=ApiBridge(sc["bridge"], log), key=get_key,
                            transport=httpx.MockTransport(handle), sleep=sleep, effort=sc["effort"],
                            limit_s=sc["limit_s"], monotonic=lambda: 0.0, refusals=refusals, planner=sc["planner"])
    if sc["refused"]:
        refusals.remember(rt.format_key)
    events: list[Any] = []
    run = None
    gen = None
    try:
        for op in sc["ops"]:
            kind = op["op"]
            if kind == "start":
                try:
                    run = await rt.start(TASK, request_of(sc["request"]), sc["run_token"])
                    events.append({"op": "start"})
                except Exception as e:
                    events.append({"op": "start", "raised": raised(e)})
            elif kind == "events":
                gen = gen or rt.events(run)
                count = 0
                while op.get("n") is None or count < op["n"]:
                    try:
                        event = await asyncio.wait_for(gen.__anext__(), 5)
                    except StopAsyncIteration:
                        events.append({"op": "end"})
                        break
                    except Exception as e:
                        events.append({"op": "raised", "raised": raised(e)})
                        break
                    events.append({"op": "event", "event": event_text(event)})
                    count += 1
            elif kind == "steer":
                try:
                    await rt.continue_(run, op["text"])
                    events.append({"op": "steer", "ok": True})
                except Exception as e:
                    events.append({"op": "steer", "raised": raised(e)})
            elif kind == "cancel":
                await rt.cancel(run)
                events.append({"op": "cancel"})
    finally:
        if gen is not None:
            await gen.aclose()
        api_mod.secrets.token_hex = saved                # type: ignore[assignment]
    return {"name": sc["name"], "requests": requests, "log": events, "calls": log, "retried": rt.retried,
            "remembered": refusals.refused(rt.format_key) if rt.format_key else None}


# ---------------------------------------------------------------- tables: usage, prices, limits, the runtime table

import ferb.runtimes.catalog as catalog_mod  # noqa: E402
import ferb.runtimes.tooltrace as tooltrace  # noqa: E402
from ferb.runtimes.prices import PRICES  # noqa: E402


class Cfg:
    def __init__(self, data: dict[str, dict[str, Any]]) -> None:
        self.data = data

    def get(self, section: str, key: str, default: Any = None) -> Any:
        return self.data.get(section, {}).get(key, default)


CONFIGS = [
    {},
    {"models": {"codex": "gpt-6.1-sol", "claude": ""}, "runtimes": {"claude_model": "opus"}},
    {"efforts": {"codex": " high ", "claude": 5, "openai": ""},
     "role_models": {"orchestrator": {"runtime": "codex", "model": " gpt-6-sol ", "effort": "medium"},
                     "checker": {"runtime": "claude", "model": "", "effort": "max"},
                     "worker": {"runtime": "openai", "model": "gpt-6-luna"}}},
    {"role_models": {"orchestrator": "not a table", "worker": {"runtime": "codex", "effort": "  "}},
     "efforts": {"codex": "xhigh"}},
]


def tables() -> dict[str, Any]:
    out: dict[str, Any] = {}
    costs = []
    for model in list(PRICES) + ["gpt-unknown"]:
        for inp, cached, write, outp, w1 in [(1000, 0, 0, 100, 0), (300000, 200000, 50000, 2000, 30000),
                                             (120000, 10, 120000, 5, 999999), (5, 10, 0, 0, 0)]:
            for day in ("2026-11-13", "2027-02-01"):
                costs.append([model, inp, cached, write, outp, day, w1,
                              usage_mod.cost_usd(model, inp, cached, write, outp, day, w1)])
    out["costs"] = costs
    usages = []
    for kind, model, data in [
        ("openai", "gpt-6.1-sol", {"input_tokens": 120, "output_tokens": 9, "input_tokens_details": {
            "cached_tokens": 100, "cache_write_tokens": 20}, "output_tokens_details": {"reasoning_tokens": 4}}),
        ("openai", "gpt-5.5", {"input_tokens": 300000, "output_tokens": 9, "input_tokens_details": None}),
        ("openai", "nope", {}),
        ("codex", "gpt-5.6-sol", {"input_tokens": 8, "cached_input_tokens": 2.7, "output_tokens": "4",
                                  "reasoning_output_tokens": None}),
        ("anthropic", "claude-opus-5-5", {"input_tokens": 5, "output_tokens": 3, "cache_read_input_tokens": 100,
                                          "cache_creation_input_tokens": 50,
                                          "cache_creation": {"ephemeral_1h_input_tokens": 40}}),
        ("anthropic", "claude-haiku-5-5", {"input_tokens": 99000, "output_tokens": 3,
                                           "cache_creation_input_tokens": 5000}),
        ("anthropic", "claude", {"input_tokens": 1}),
        ("gemini", "gemini-3.8-flash", {"promptTokenCount": 130, "cachedContentTokenCount": 100,
                                        "candidatesTokenCount": 7, "thoughtsTokenCount": 3,
                                        "toolUsePromptTokenCount": 11}),
    ]:
        fn = {"openai": lambda m, d: usage_mod.from_openai(m, d, "api"),
              "codex": usage_mod.from_codex,
              "anthropic": lambda m, d: usage_mod.from_anthropic(m, d, "api"),
              "gemini": lambda m, d: usage_mod.from_gemini(m, d, "api")}[kind]
        from dataclasses import asdict
        usages.append([kind, model, data, json.dumps(asdict(fn(model, data)))])
    usages.append(["anthropic_cost", "claude-opus-5-5", {"input_tokens": 1}, json.dumps(asdict(
        usage_mod.from_anthropic("claude-opus-5-5", {"input_tokens": 1}, "subscription", provider_cost=0.25)))])
    out["usages"] = usages
    home = ROOT / "tables" / "home"
    write_files(home, {"codex-home/auth.json": AUTH, "run/codex-bundled-0.166.0.json": json.dumps({"models": [
        {"slug": "gpt-5.6-sol", "context_window": 272000, "effective_context_window_percent": 95},
        {"slug": "gpt-x", "context_window": 100000, "effective_context_window_percent": 87.5},
        {"slug": "gpt-new", "context_window": 1, "effective_context_window_percent": 1,
         "minimal_client_version": [0, 170, 0]},
        {"slug": "gpt-broken", "context_window": 0, "effective_context_window_percent": 95},
        {"slug": "gpt-true", "context_window": True, "effective_context_window_percent": 95}]})})
    limits = []
    for name in ("codex", "claude", "anthropic", "openai", "google"):
        for model in (None, "", "gpt-5.6-sol", "gpt-x", "gpt-new", "gpt-broken", "gpt-true", "gpt-9",
                      "claude-haiku-4-5", "opus", "claude-new", "gemini-3.1-pro-preview", "gemini-9", "gpt-6-luna"):
            try:
                lim = catalog_mod.CATALOG[name].limits(model, home)
                limits.append([name, model, [lim.model, lim.context_window, lim.max_output, lim.source,
                                             lim.window_includes_output, lim.max_input_chars]])
            except Exception as e:
                limits.append([name, model, raised(e)])
    out["limits"] = [normalize(x, home) for x in limits]
    out["call_limit"] = [[m, p, catalog_mod.call_limit(m, p)] for m in ("supervisor", "worker") for p in (True, False)]
    efforts, models = [], []
    for i, cfg in enumerate(CONFIGS):
        c = Cfg(cfg)
        for name in ("codex", "claude", "openai", "nope"):
            models.append([i, name, catalog_mod.configured_model(c, name), catalog_mod.model_for(c, name),
                           [catalog_mod.role_model(c, name, r) for r in ("orchestrator", "worker", "checker:x")],
                           catalog_mod.configured_effort(c, name),
                           [catalog_mod.role_effort(c, name, r) for r in ("orchestrator", "worker", "checker")]])
            for role, mode, planner, asked in [("orchestrator", "supervisor", True, None),
                                               ("orchestrator", "supervisor", False, "high"),
                                               ("worker", "worker", False, None), ("checker:3", "worker", False, "high"),
                                               ("checker", "worker", False, None), ("worker", "worker", False, "")]:
                efforts.append([i, name, role, mode, planner, asked,
                                catalog_mod.run_effort(c, name, role, mode, planner, asked)])
    out["models"], out["efforts"] = models, efforts
    out["effort_problem"] = [[n, e, catalog_mod.effort_problem(n, e)] for n in ("codex", "claude")
                             for e in ("low", "Ultra", "x" * 32, "x" * 33, "-low", "a_b-c", "", "é", "high!")]
    out["sign_in_fix"] = [[n, catalog_mod.sign_in_fix(n)] for n in ("codex", "claude", "anthropic", "openai",
                                                                     "google", "other")]
    out["labels"] = [[n, catalog_mod.label(n)] for n in ("codex", "claude", "anthropic", "openai", "google", "x")]
    out["api_providers"] = list(catalog_mod.api_providers())
    out["catalog"] = [[r.name, r.default_model, r.sign_in, r.billing] for r in catalog_mod.CATALOG.values()]
    out["http_cause"] = [[st, api_mod.http_cause(st)] for st in (400, 401, 402, 403, 404, 408, 409, 413, 418, 429,
                                                                 500, 503, 599)]
    out["g"] = [[x, format(x, "g")] for x in (0.2, 1.5, 2.0, 60.0, 0.0001, 0.00001, 123456.0, 1234567.0, 900.0,
                                              0.1, 1e-07, 33.333333333)]
    trace_texts = [
        'TRACE {"tools":[{"type":"function","name":"a"},{"type":"web_search"},{"type":"x","name":""}]}',
        'x output=[{"type":"namespace","name":"ns","tools":[{"type":"function","name":"f"}]}] y',
        'x output=[{"type":"other","name":"no"}] "tools":[{"type":"function","name":"functions.b"}]',
        '"tools":[{"type":"function"]  "tools":[{"name":"ok"}]',
        '"tools":[{"name":"ns","tools":"str"},{"name":"ns2","tools":{"k":1}},{}]',
        'no markers here', '"tools":[{"type":"namespace","tools":[{"name":"x"}]}]',
    ]
    traces = []
    for t in trace_texts:
        try:
            traces.append([t, sorted(tooltrace.tools_in(t))])
        except Exception as e:
            traces.append([t, raised(e)])
    out["traces"] = traces
    out["inline_refs"] = [[sch, api_mod.inline_refs(sch)] for sch in [
        LOOKUP["input_schema"],
        {"$defs": {"A": {"type": "object", "properties": {"b": {"$ref": "#/$defs/B"}}},
                   "B": {"type": "object", "properties": {"a": {"$ref": "#/$defs/A"}}}},
         "type": "object", "properties": {"a": {"$ref": "#/$defs/A", "description": "an a"},
                                          "m": {"$ref": "#/$defs/Missing"}, "list": [{"$ref": "#/$defs/B"}]}},
        {"type": "string"}]]
    errors = []
    for e in [None, "x", {}, {"message": "boom"}, {"message": "", "code": 5}, {"message": "m", "code": True},
              {"message": "m", "codexErrorInfo": {"other": 1}, "data": {"input_error_code": "c", "actual_chars": 9}},
              {"message": "m", "data": {"input_error_code": "c", "actual_chars": "9"}},
              {"message": "refresh_token_reused here", "data": {"x": "Invalid_Grant"}},
              {"message": "m", "data": {"x": "invalid_granted"}}]:
        errors.append([e, codex_stream.codex_error("P", e, lambda x: str(x)), codex_stream._signed_out(e)])
    out["codex_errors"] = errors
    logins = ['{"tokens":{}}', '{"tokens":{},"OPENAI_API_KEY":""}', '{"tokens":{},"auth_mode":"APIKEY"}',
              '{"tokens":{},"auth_mode":5}', '{"tokens":null}', "", "﻿{}"]
    out["logins"] = [[t, codex_stream._chatgpt_login(t.encode())] for t in logins]
    out["cache_ids"] = [[r, m, sc, codex_stream.cache_thread_id(r, m, sc)] for r, m, sc in
                        [("planner", "gpt-5.6-sol", "dev-1"), ("planner", "x", ""), ("a:b", "c", "é")]]
    import ferb.runtimes.models as models_mod
    out["model_lists"] = [[p, d, models_mod.parse(p, d)] for p, d in [
        ("anthropic", {"data": [{"id": "claude-b"}, {"id": "claude-a"}, {"id": ""}, {"x": 1}, "s", {"id": "claude-a"}]}),
        ("openai", {"data": None}), ("openai", []),
        ("google", {"models": [{"name": "models/gemini-x", "supportedGenerationMethods": ["generateContent"]},
                               {"name": "models/embed", "supportedGenerationMethods": ["embedContent"]},
                               {"name": "plain", "supportedGenerationMethods": ["generateContent", "x"]}]})]]
    names = ["claude-opus-5-5", "claude-opus-5-5-20261101", "claude-opus-4-5", "claude-x", "gemini-3.8-flash",
             "gemini-3-pro-preview-001", "gpt-5.6-sol", "gpt-6-luna", "gpt-7"]
    codex_home = ROOT / "tables" / "home2"
    write_files(codex_home, {"codex-home/auth.json": AUTH, "codex-home/models_cache.json": json.dumps({"models": [
        {"slug": "gpt-a", "supported_reasoning_levels": [{"effort": "low"}, {"effort": "ultra"}, {"x": 1}],
         "default_reasoning_level": "low"}, {"slug": "gpt-b"}, {"nope": 1}]})})
    out["effort_levels"] = [[n, models_mod.effort_levels(n, codex_home, names)] for n in
                            ("codex", "claude", "anthropic", "google", "openai", "other")]
    out["known_models"] = [[n, models_mod.known_models(n, codex_home)] for n in ("codex", "claude", "google")]
    out["toml"] = [codex_stream._toml(codex_stream.provider_entry()), codex_stream._toml("q\"é"),
                   codex_stream._toml(True), codex_stream._toml(1.5)]
    return out


def write(name: str, data: Any) -> None:
    (HERE / name).write_text(json.dumps(data, indent=1, ensure_ascii=False, sort_keys=False) + "\n")


async def main() -> None:
    which = set(sys.argv[1:])
    if not which or "codex" in which:
        out = []
        for i, sc in enumerate(CODEX_SCENARIOS):
            out.append({"scenario": sc, "python": await run_codex(sc, i)})
        write("codex.json", {"base_env": BASE_ENV, "today": TODAY, "cases": out})
    if not which or "claude" in which:
        out = []
        for i, sc in enumerate(CLAUDE_SCENARIOS):
            out.append({"scenario": sc, "python": await run_claude(sc, i)})
        write("claude.json", {"base_env": BASE_ENV, "today": TODAY, "forms": FORMS, "cases": out})
    if not which or "mcp" in which:
        out = []
        for i, sc in enumerate(MCP_SCENARIOS):
            out.append({"scenario": sc, "python": await run_mcp(sc, i)})
        write("mcp.json", {"cases": out})
    if not which or "api" in which:
        out = []
        for sc in API_SCENARIOS:
            out.append({"scenario": sc, "python": await run_api(sc)})
        write("api.json", {"key": KEY, "today": TODAY, "forms": FORMS, "cases": out})
    if not which or "tables" in which:
        write("tables.json", tables())


if __name__ == "__main__":
    try:
        asyncio.run(main())
    finally:
        shutil.rmtree(ROOT, ignore_errors=True)
