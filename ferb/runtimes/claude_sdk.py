"""Owns the Claude SDK adapter and tool containment. Must not select task targets, authorize actions or expose tools outside the registered harness.

ClaudeRuntime over the Claude Agent SDK — supervisor and worker modes.

Lockdown (spike 2): tools=[] (no built-in tools), permission_mode="dontAsk" (anything not pre-allowed is denied),
setting_sources=[] (None would load ~/.claude user + project settings), strict MCP, no plugins/agents/skills.
Telemetry off. FERB's own subscription sign-in only (`claude_login`, CLAUDE_CONFIG_DIR=~/.ferb/claude-home): without it
no run starts (never the owner's own ~/.claude login and billing), and a run whose init reports an API key is refused (an
inherited ANTHROPIC_API_KEY or CLAUDE_CODE_OAUTH_TOKEN would otherwise silently take over; both are blanked). The run
token reaches FERB's MCP shim through a 0600 file, never the MCP config Claude Code receives on its command line. The
tool list the API actually receives is checked through the local tool-list proxy after every turn. Typed answers use
Anthropic's structured outputs (Claude Code's StructuredOutput tool), so the schema is reshaped for Anthropic's limits
(schema_shape.for_anthropic) and the answer mapped back. Claude Code sends it as the API's output_config.format, so the
API's undocumented grammar-size refusal reaches this runtime too: a run whose first API call is refused (the result's
`api_error_status` 400, before any output) starts again once with no output_format and the answer form stated after
FERB's instructions, sharing the API loop's memory of which formats don't fit (RUN-427, schema_shape.Refusals)."""

import asyncio
import json
import os
import secrets
import shutil
import tempfile
import time
import uuid
from dataclasses import asdict, replace
from pathlib import Path
from contextlib import suppress
from typing import Any, AsyncIterator, Callable, Literal

from ferb.cli.mcp import mcp_name
from ferb.core.errors import ModelLimitsUnknown, Unsupported
from ferb.core.structured import check_final
from ferb.core.timing import CallTimer, Steps
from ferb.core.types import ModelImage, ModelLimits, ModelRequest, RunHandle, RuntimeEvent, Task
from ferb.runtimes.proxy import ToolListProxy
from ferb.runtimes.schema_shape import (REFUSALS, Refusals, SchemaTooComplex, Shape, for_anthropic, repair_note, same,
                                        stated, unfenced)
from ferb.runtimes.usage import from_anthropic

SDK_VERSION = "0.2.174"
# The Claude Code the pinned SDK bundles (claude_agent_sdk/_cli_version.py); `runtimes.status` compares both
CLAUDE_CODE_VERSION = "2.1.310"
STRUCTURED_OUTPUT = "StructuredOutput"                # tool the SDK adds when output_format is set
# The CLI's result subtype when the model's answers never matched output_format (bundled CLI 2.1.283 and 2.1.310)
STRUCTURED_RETRIES = "error_max_structured_output_retries"


# P20: FERB's breakpoint on its stable prefix. One hour, because Claude Code puts its own breakpoints around FERB's text
# (its system prompt before, its environment message after) and the API refuses a longer TTL after a shorter one;
# CACHE_TTL_ENV pins Claude Code's own to the same hour (unset, it picks 5 minutes on a subscription past its usage
# limits). With FERB's, 3 of the API's 4 breakpoints (measured through 2.1.283, 2026-10-25; the same three in 2.1.310's
# request, checked offline 2026-11-01).
CACHE = {"type": "ephemeral", "ttl": "1h"}
CACHE_TTL_ENV = {"CLAUDE_CODE_PROMPT_CACHE_TTL": "1h"}
# Claude Code 2.1.304 turned tool search on by default behind a custom ANTHROPIC_BASE_URL (FERB's tool-list proxy; it
# was off there before). It then sends a `DeferredToolPlaceholder` tool, which FERB's tool check would stop as a tool
# outside the run's list, and may defer FERB's MCP tools behind a search. Off, the request carries exactly the run's
# tools, as with 2.1.283 (offline request check, 2026-11-01).
TOOL_SEARCH_ENV = {"ENABLE_TOOL_SEARCH": "false"}


def _content(request: ModelRequest) -> list[dict[str, Any]]:
    """FERB's text as the one user message's blocks: the system text, then each context part, each its own text block,
    with the cache breakpoint on the last stable part (`request.cacheable`), or on the system text when no part is
    stable; then each image as a base64 block, after the text so the cached prefix stays (L12). The same placement as
    the Anthropic API runtime. Empty text is left out (the API refuses an empty block); a breakpoint that would fall on
    one moves to the text before it."""
    texts = [request.system, *request.parts]
    marked = request.cacheable if 0 < request.cacheable <= len(request.parts) else 0
    blocks: list[dict[str, Any]] = []
    for index, text in enumerate(texts):
        if text:
            blocks.append({"type": "text", "text": text})
        if index == marked and blocks:
            blocks[-1]["cache_control"] = CACHE
    return blocks + [{"type": "image", "source": {"type": "base64", "media_type": i.mime, "data": i.base64()}}
                     for i in request.images]


async def _frames(request: ModelRequest) -> AsyncIterator[dict[str, Any]]:
    """The SDK's streaming input: the form that carries content blocks."""
    yield {"type": "user", "parent_tool_use_id": None, "message": {"role": "user", "content": _content(request)}}


# L28: with ANTHROPIC_BASE_URL set (FERB's proxy), Claude Code gives recognized models the Anthropic API's window, and its
# aliases resolve on the Anthropic API to these models (https://code.claude.com/docs/en/model-config, checked
# 2026-10-14). The limits themselves are Anthropic's documented ones (anthropic_api.LIMITS). No model configured means
# the account's plan-dependent default, whose window FERB can't know: an explicit error, never a guess.
ALIASES = {"opus": "claude-opus-5-5", "sonnet": "claude-sonnet-5-5", "haiku": "claude-haiku-5-5",
           "fable": "claude-fable-5-1"}


def limits_for(model: str | None) -> ModelLimits:
    from ferb.runtimes.anthropic_api import limits_for as documented
    if not model:
        raise ModelLimitsUnknown("no model is chosen for Claude, so its context window is the account's "
                                 "plan-dependent default")
    resolved = documented(ALIASES.get(model, model))
    return ModelLimits(model, resolved.context_window, resolved.max_output,
                       resolved.source + "; aliases: https://code.claude.com/docs/en/model-config")


def claude_pin_status(installed_sdk: str) -> str:
    """The doctor's Claude line: the installed SDK and the Claude Code it bundles against the pins, or the problem (a
    mismatch on either, or the bundled copy missing, which every run refuses)."""
    from ferb.runtimes.claude_login import cli_binary
    bundled: str | None
    try:
        from claude_agent_sdk._cli_version import __cli_version__ as bundled
    except ImportError:
        bundled = None
    problems = []
    if installed_sdk != SDK_VERSION:
        problems.append(f"SDK {installed_sdk} != pinned {SDK_VERSION}")
    if bundled != CLAUDE_CODE_VERSION:
        problems.append(f"bundled Claude Code {bundled or 'unknown'} != pinned {CLAUDE_CODE_VERSION}")
    try:
        cli_binary()
    except Unsupported as e:
        problems.append(str(e))
    return "problem: " + "; ".join(problems) if problems else f"{installed_sdk} (Claude Code {bundled})"


def write_token_file(run_token: str) -> Path:
    """The run token in a fresh owner-only folder, as a 0600 file the shim reads (`FERB_RUN_TOKEN_FILE`)."""
    folder = Path(tempfile.mkdtemp(prefix="ferb-claude-run-"))          # 0700
    path = folder / "run-token"
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as file:
        file.write(run_token)
    return path



# The Agent SDK's AssistantMessageError kinds that are account facts no model can change (step 8b, ORCH-315)
ACCOUNT_ERRORS = {"authentication_failed": "signed_out", "billing_error": "usage_limit", "rate_limit": "usage_limit"}

class ClaudeRuntime:
    name = "claude"

    def __init__(self, mode: Literal["supervisor", "worker"], proxy: ToolListProxy, ferb_home: str,
                 shim_cmd: list[str] | None = None, tool_names: list[str] | None = None,
                 output_schema: dict[str, Any] | None = None, model: str | None = None,
                 effort: str | None = None, limit_s: float | None = None,
                 monotonic: Callable[[], float] = time.monotonic, refusals: Refusals | None = None) -> None:
        self.mode, self.proxy, self.ferb_home, self.shim_cmd = mode, proxy, ferb_home, shim_cmd
        # the clock the run's timing reads (RUN-419, RUN-420): its start steps and its turn's; tests give a fake
        self.monotonic = monotonic
        self.output_schema, self.model = output_schema, model
        # what Claude Code receives, and how its answers come back to FERB's form (schema_shape)
        self.shape: Shape | None = None
        self.stated_only = False         # over Anthropic's counted limits: the answer form stated from the start
        if output_schema:
            try:
                self.shape = for_anthropic(output_schema)
            except SchemaTooComplex:
                self.shape, self.stated_only = same(output_schema), True
        # RUN-427: the same fact as the Anthropic API runtime's (one feature, one grammar), keyed by the resolved model
        self.refusals = refusals if refusals is not None else REFUSALS
        self.format_key = (Refusals.key("anthropic", ALIASES.get(model or "", model), self.shape.wire)
                           if self.shape is not None else "")
        self.effort = effort             # L39: the SDK's own effort option when asked for ("high"); None: its default
        self.limit_s = limit_s           # one overall limit on this call (catalog.call_limit); None for a worker
        self.allowed = {f"mcp__ferb__{mcp_name(n)}" for n in (tool_names or [])}
        if output_schema:
            self.allowed.add(STRUCTURED_OUTPUT)        # the SDK's carrier for typed output (inert)

    @property
    def limits(self) -> ModelLimits:
        return limits_for(self.model)

    def _options(self, run_id: str, token_file: Path | None, stated_form: bool = False) -> Any:
        from claude_agent_sdk import ClaudeAgentOptions
        from ferb.runtimes.claude_login import claude_home, cli_binary
        own = claude_home(Path(self.ferb_home))       # L34 C: FERB's own Claude sign-in, once it finished
        if own is None:                               # never the owner's own ~/.claude login and billing
            raise Unsupported("Claude isn't signed in for FERB: sign in to Claude in Settings → AI")
        # the pinned Claude Code the SDK bundles, by path: without one the SDK would fall back to any `claude` on PATH
        # (subprocess_cli._find_cli); missing, the run is refused as the sign-in is
        cli = cli_binary()
        # CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT=0: Claude Code otherwise aborts a tool call that is silent for 30 minutes;
        # an owner question waits as long as the owner takes (§8.1). "0 disables" is Claude Code's own wording.
        env = {"ANTHROPIC_BASE_URL": self.proxy.base_url(run_id), "DISABLE_TELEMETRY": "1",
               "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1", "DISABLE_ERROR_REPORTING": "1",
               "ANTHROPIC_API_KEY": "", "ANTHROPIC_AUTH_TOKEN": "", "CLAUDE_CODE_OAUTH_TOKEN": "",
               "CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT": "0", "CLAUDE_CONFIG_DIR": str(own), **CACHE_TTL_ENV,
               **TOOL_SEARCH_ENV}
        mcp: dict[str, Any] = {}
        if self.mode == "worker" or token_file is not None:     # RUN-415: FERB's tools come with a run token
            if not self.shim_cmd:
                raise ValueError("worker mode needs the ferb mcp shim")
            if token_file is None:
                raise ValueError("worker mode needs a run token file")
            # the token itself never goes into the MCP config, which Claude Code receives on its command line (visible
            # in `ps` to every local process): the shim reads it from a file only the owner can read
            mcp = {"ferb": {"type": "stdio", "command": self.shim_cmd[0], "args": self.shim_cmd[1:],
                            "env": {"FERB_RUN_TOKEN_FILE": str(token_file), "FERB_HOME": self.ferb_home}}}
        return ClaudeAgentOptions(
            tools=[], allowed_tools=sorted(self.allowed - {STRUCTURED_OUTPUT}), permission_mode="dontAsk",
            setting_sources=[],
            strict_mcp_config=True, plugins=[], agents=None, skills=None, mcp_servers=mcp, env=env,
            model=self.model, effort=self.effort, cli_path=cli,   # type: ignore[arg-type]
            include_partial_messages=self.mode == 'supervisor' and self.output_schema is not None,
            extra_args={"no-session-persistence": None, "replay-user-messages": None},   # no transcript (tool results, screenshots) on disk
            output_format=({"type": "json_schema", "schema": self.shape.wire} if self.shape and not stated_form
                           else None))

    def _request(self, request: ModelRequest, typed: str) -> ModelRequest:
        """The request as sent: with the answer form stated after FERB's instructions when the typed format isn't
        used (RUN-427), inside the same cached prefix."""
        if not typed.startswith("stated") or self.output_schema is None:
            return request
        return replace(request, system="\n\n".join(filter(None, [request.system, stated(self.output_schema)])))

    async def start(self, task: Task, request: ModelRequest, run_token: str) -> RunHandle:
        from claude_agent_sdk import ClaudeSDKClient
        run_id = "run-" + secrets.token_hex(6)
        deadline = asyncio.get_running_loop().time() + self.limit_s if self.limit_s is not None else None
        steps, timer = Steps(self.monotonic), CallTimer(self.monotonic)     # RUN-420, RUN-419
        token_file = write_token_file(run_token) if self.mode == "worker" or run_token else None
        typed = ("stated" if self.stated_only or self.refusals.refused(self.format_key) else "strict") if self.shape \
            else ""
        client = None
        try:
            client = ClaudeSDKClient(options=self._options(run_id, token_file, typed == "stated"))
            steps.mark("options")
            async with asyncio.timeout_at(deadline):
                await client.connect()
                steps.mark("connect")                  # Claude Code's process started and ready
                timer.begin()
                await client.query(_frames(self._request(request, typed)))
                steps.mark("query")
        except BaseException:
            if client is not None:
                with suppress(Exception):
                    await client.disconnect()
            if self.proxy is not None:
                self.proxy.forget(run_id)
            _remove_token(token_file)
            raise
        return RunHandle(run_id, self.name, task.id, {"client": client, "checked_auth": False,
                                                      "token_file": token_file, "deadline": deadline,
                                                      "start_steps": steps.steps, "timer": timer, "typed": typed,
                                                      "request": request})

    async def events(self, run: RunHandle) -> AsyncIterator[RuntimeEvent]:
        if run.state.get("client") is None or run.state.get("closed"):
            return
        try:
            async with asyncio.timeout_at(run.state.get("deadline")):
                while True:
                    inner = self._events(run, run.state["client"])
                    try:
                        async for event in inner:
                            yield event
                    finally:
                        await inner.aclose()
                    if not run.state.pop("restart", False):
                        break
                    await self._restart(run)
        except TimeoutError:
            yield RuntimeEvent("error", {"message": "Claude runtime timed out"})
        finally:
            run.state["closed"] = True

    async def _restart(self, run: RunHandle) -> None:
        """RUN-427: the API refused the typed format before any output; the same run starts again in a fresh Claude Code
        with the answer form stated instead (same run id, proxy route and token file)."""
        from claude_agent_sdk import ClaudeSDKClient
        old = run.state.get("client")
        if old is not None:
            with suppress(Exception):
                await old.disconnect()
        run.state.update(typed="stated_after_refusal", checked_auth=False, client=None)
        client = ClaudeSDKClient(options=self._options(run.id, run.state.get("token_file"), True))
        run.state["client"] = client
        await client.connect()
        if run.state.get("timer") is not None:
            run.state["timer"].begin()                 # the stated call is a call of its own (RUN-421)
        await client.query(_frames(self._request(run.state["request"], "stated_after_refusal")))

    async def _events(self, run: RunHandle, client: Any) -> AsyncIterator[RuntimeEvent]:
        from claude_agent_sdk import (AssistantMessage, ResultMessage, SystemMessage, ToolResultBlock, ToolUseBlock,
                                      UserMessage)
        from claude_agent_sdk.types import StreamEvent
        structured_blocks: set[int] = set()
        # RUN-421: the turn's timing from the messages' arrival: Claude Code's streamed events and assistant messages
        # are the model's output, a tool result goes back, the result ends it. One result covers the whole turn (its
        # API calls and the tools between them), as its usage does.
        timer: CallTimer | None = run.state.get("timer")
        # receive_response stops at the first ResultMessage, even with queued input.
        typed = str(run.state.get("typed") or "")
        async for m in client.receive_messages():
            if timer is not None and isinstance(m, (StreamEvent, AssistantMessage)):
                timer.output()
            if isinstance(m, UserMessage):
                if isinstance(m.content, list) and any(isinstance(b, ToolResultBlock) for b in m.content):
                    if timer is not None:
                        timer.handed_back()
                    yield RuntimeEvent("progress", {})     # a tool's result went back: a sign of life (ORCH-528)
                run.state.setdefault("pending_updates", set()).discard(m.uuid)
                continue
            if isinstance(m, SystemMessage) and m.subtype == "init":
                if m.data.get("apiKeySource", "none") != "none":
                    await self.cancel(run)
                    yield RuntimeEvent("violation", {"auth": f"API key source {m.data.get('apiKeySource')!r}; "
                                                             "only the subscription login is allowed"})
                    return
                run.state["checked_auth"] = True
            elif isinstance(m, StreamEvent):
                # The SDK's typed result is a StructuredOutput tool payload, not ordinary assistant commentary.
                if not run.state.get('checked_auth') or m.parent_tool_use_id is not None:
                    continue
                event = m.event
                kind = event.get('type')
                if kind == 'message_start':
                    structured_blocks.clear()
                elif kind == 'content_block_start':
                    block = event.get('content_block') or {}
                    if ((block.get('type') == 'tool_use' and block.get('name') == STRUCTURED_OUTPUT)
                            or (block.get('type') == 'text' and typed.startswith('stated'))):   # RUN-427: the answer
                        structured_blocks.add(event['index'])
                elif kind == 'content_block_delta' and event.get('index') in structured_blocks:
                    delta = event.get('delta') or {}
                    piece = (delta.get('partial_json') if delta.get('type') == 'input_json_delta'
                             else delta.get('text') if delta.get('type') == 'text_delta' else None)
                    if piece:
                        run.state["emitted"] = True
                        yield RuntimeEvent('text_delta', {'text': piece})
                        continue
                elif kind == 'content_block_stop':
                    structured_blocks.discard(event.get('index'))
                # ORCH-528: every other stream event (thinking, a block starting or ending) is the model working
                yield RuntimeEvent('progress', {})
            elif isinstance(m, AssistantMessage):
                if account := ACCOUNT_ERRORS.get(str(getattr(m, "error", None) or "")):
                    run.state["account"] = account   # the SDK's own error kind (AssistantMessageError), step 8b
                for b in m.content:
                    if isinstance(b, ToolUseBlock):
                        run.state["emitted"] = True
                        if b.name == STRUCTURED_OUTPUT:
                            run.state["structured"] = b.input
                        else:
                            yield RuntimeEvent("tool_call", {"tool": b.name, "arguments": b.input})
                    elif hasattr(b, "text") and b.text:
                        run.state["emitted"] = True
                        yield RuntimeEvent("message", {"text": b.text})
            elif isinstance(m, ResultMessage):
                # Claude Code's total_cost_usd is cumulative across the turns of one streaming-input session ("each
                # result carries the running total so far": its own field description, Claude Code 2.1.283 and 2.1.310),
                # while usage is per turn: a steered run's later results report only the cost added since the last one
                cost = m.total_cost_usd
                if cost is not None:
                    cost, run.state["cost_so_far"] = max(cost - run.state.get("cost_so_far", 0.0), 0.0), cost
                yield RuntimeEvent("usage", asdict(from_anthropic(self.model or "claude", dict(m.usage or {}),
                                                                  billing="subscription", provider_cost=cost))
                                   | ({"timing": timer.done()} if timer is not None else {})
                                   | ({"typed": typed} if typed else {}) | self._repair_mark(m, run, typed))
                if getattr(m, "is_error", False):
                    if (typed == "strict" and getattr(m, "api_error_status", None) == 400
                            and not run.state.get("emitted") and m.subtype != STRUCTURED_RETRIES):
                        run.state["restart"] = True    # RUN-427: once more without the typed format (`events`)
                        return
                    run.state["closed"] = True
                    detail = m.result or str(getattr(m, "errors", None)) or m.subtype
                    # the model answered, but never within the schema: format repair, not a failed call
                    yield RuntimeEvent("invalid_answer" if m.subtype == STRUCTURED_RETRIES else "error",
                                       {"message": detail} | ({"account": run.state["account"]}
                                                              if run.state.get("account") else {}))
                    return
                extra = self.proxy.seen(run.id) - self.allowed
                if extra:
                    await self.cancel(run)
                    yield RuntimeEvent("violation", {"unexpected_tools": sorted(extra)})
                    return
                if run.state.get("pending_updates"):
                    # A late result from the previous input cannot finish accepted corrections.
                    run.state.pop("structured", None)
                    continue
                structured = getattr(m, "structured_output", None) or run.state.get("structured")
                text = m.result or ""
                if structured is not None:
                    text = json.dumps(self.shape.value(structured) if self.shape else structured)
                elif self.shape is not None:
                    text = self.shape.answer_text(unfenced(text) if typed.startswith("stated") else text)
                if typed == "stated_after_refusal" and not run.state.get("remembered"):
                    run.state["remembered"] = True     # the same request without the format was accepted
                    self.refusals.remember(self.format_key)
                if typed.startswith("stated") and not run.state.get("repaired"):
                    try:
                        check_final(text, self.output_schema)
                    except ValueError as e:
                        # RUN-427: nothing enforced the form, so one answer outside it is told back once
                        run.state["repaired"] = True
                        await self.continue_(run, repair_note(str(e)))
                        continue
                run.state["closed"] = True
                yield RuntimeEvent("final", {"text": text, "tools_seen": sorted(self.proxy.seen(run.id))})
                return

    def _repair_mark(self, m: Any, run: RunHandle, typed: str) -> dict[str, str]:
        """RUN-440: a stated answer's in-run repair as its usage report says it: "asked" (this answer was outside the
        schema and is told back), "fixed" or "failed" (the answer after it); {} for every other result."""
        if not typed.startswith("stated") or getattr(m, "is_error", False) or run.state.get("pending_updates"):
            return {}
        structured = getattr(m, "structured_output", None) or run.state.get("structured")
        if structured is not None:
            text = json.dumps(self.shape.value(structured) if self.shape else structured)
        else:
            text = m.result or ""
            text = self.shape.answer_text(unfenced(text)) if self.shape is not None else text
        try:
            check_final(text, self.output_schema)
            fits = True
        except ValueError:
            fits = False
        if run.state.get("repaired"):
            return {"repair": "fixed" if fits else "failed"}
        return {} if fits else {"repair": "asked"}

    async def continue_(self, run: RunHandle, message: str) -> None:
        async with run.state.setdefault("input_lock", asyncio.Lock()):
            client = run.state.get("client")
            if client is None or run.state.get("closed"):
                raise Unsupported("the Claude run has already ended")
            ident = str(uuid.uuid4())
            pending = run.state.setdefault("pending_updates", set())
            pending.add(ident)

            async def frames() -> AsyncIterator[dict[str, Any]]:
                yield {"type": "user", "message": {"role": "user", "content": message},
                       "parent_tool_use_id": None, "uuid": ident}

            try:
                # The SDK writes to the same live CLI. UUID replay acknowledgements also
                # cover multiple queued inputs merged into a single model turn by Claude.
                await client.query(frames())
            except BaseException:
                pending.discard(ident)
                with suppress(Exception):
                    await self.cancel(run)
                raise

    async def cancel(self, run: RunHandle) -> None:
        run.state["closed"] = True
        client = run.state.get("client")
        try:
            if client is not None:
                try:
                    await client.disconnect()
                finally:
                    self.proxy.forget(run.id)
                    run.state["client"] = None
        finally:
            _remove_token(run.state.pop("token_file", None))


def _remove_token(path: Path | None) -> None:
    if path is not None:
        shutil.rmtree(path.parent, ignore_errors=True)
