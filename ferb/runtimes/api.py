"""Owns FERB's own tool loop for direct provider APIs (D32). Must not choose targets, authorize actions or offer tools the
run token doesn't cover.

One loop for every provider: ask the model, run each tool call through FERB's gateway (GatewayBridge, the same path as the
MCP shim), send the results back, until the model answers. A small Provider class per API translates requests, responses,
cache markers, typed output (its `shape` of FERB's schema, ferb/runtimes/schema_shape.py) and usage. A call to a tool the
run wasn't offered gets a tool error back and nothing runs (no provider-hosted tool is ever enabled, so it can only be a
made-up name). No turn or token budget; stop, the stall watchdog and the call's own time limit (none for a worker) end a
run. Keys come from the Keychain at start and never appear in errors."""

import asyncio
import json
import secrets
import time
from dataclasses import asdict, dataclass, field
from typing import Any, AsyncIterator, Awaitable, Callable, Literal, Protocol

import httpx

from ferb.cli.mcp import mcp_name
from ferb.core.errors import FerbError, Unsupported
from ferb.core.timing import CallTimer
from ferb.core.types import ModelLimits, ModelRequest, RunHandle, RuntimeEvent, Task
from ferb.runtimes.schema_shape import Shape, same
from ferb.runtimes.usage import Usage

RETRY_STATUS = frozenset({408, 409, 429, 500, 502, 503, 504, 529})


class ProviderError(Exception):
    """A provider failure, worded without the key or the request. `overflow`: the provider's documented
    window-exceeded error (fix wave 6b round 1: a check may run once more without its optional recorded calls).
    `image`: the provider's own rejection of an image in the request (`Provider.image_refused`, ORCH-524: only then is
    the planner's turn run again without the owner's screenshot)."""

    def __init__(self, message: str, overflow: bool = False, account: str = '', image: bool = False) -> None:
        super().__init__(message)
        self.overflow, self.account, self.image = overflow, account, image


# HTTP statuses that are account facts no model can change (step 8b, ORCH-315): a key the provider refused,
# or a quota or rate limit still in force after the retries
ACCOUNT_STATUS = {401: 'signed_out', 403: 'signed_out', 402: 'usage_limit', 429: 'usage_limit'}
# The plain cause of a provider's HTTP refusal, from its status code alone (a fact), said before the provider's own
# technical words (kb area runtimes: owner-facing errors in app words, plain cause first)
HTTP_CAUSES = {400: "it refused the request FERB sent", 401: "it didn't accept FERB's API key",
               403: "it didn't accept FERB's API key", 402: "the account's usage limit is reached",
               404: ("it doesn't offer the model FERB asked for to this API key: pick another model or service "
                     "in Settings → AI"), 408: "it timed out",
               413: "the request was too large for it", 429: "the account's usage or rate limit is reached"}


def http_cause(status: int) -> str:
    return HTTP_CAUSES.get(status, "it had a problem on its side" if status >= 500 else "it refused the request")



@dataclass(frozen=True)
class ToolSpec:
    name: str                        # as the model sees it (dots become underscores)
    description: str
    schema: dict[str, Any]


@dataclass(frozen=True)
class ToolCall:
    id: str
    name: str
    arguments: dict[str, Any]
    arguments_error: str = ""        # set when the model's arguments weren't a JSON object


@dataclass(frozen=True)
class ToolOutcome:
    call: ToolCall
    text: str
    is_error: bool
    image: dict[str, str] | None = None     # {"data": base64, "mime": "image/png"}


@dataclass(frozen=True)
class Turn:
    calls: list[ToolCall]
    final: str | None
    usage: Usage
    stop: str = ""


class Provider(Protocol):
    name: str
    model: str
    # FERB's output schema in the form this provider accepts, and its answers back (optional: as is when absent)
    # def shape(self, schema: dict[str, Any]) -> Shape: ...
    def begin(self, request: ModelRequest, tools: list[ToolSpec], schema: dict[str, Any] | None, effort: str | None,
              cache_key: str) -> dict[str, Any]: ...
    def http(self, state: dict[str, Any]) -> tuple[str, dict[str, str], dict[str, Any]]: ...
    def auth(self, key: str) -> dict[str, str]: ...
    def parse(self, state: dict[str, Any], data: dict[str, Any]) -> Turn: ...
    def add_results(self, state: dict[str, Any], turn: Turn, results: list[ToolOutcome]) -> None: ...
    # Whether an error response (its status and `error` object) is this provider's own rejection of an input over the
    # model's window, by the shape the provider defines (fix wave 6b rounds 1-2: a check reruns without its recorded
    # calls); a provider with none to cite answers False
    def window_exceeded(self, status: int, error: dict[str, Any]) -> bool: ...
    # Whether an error response is this provider's own rejection of an image in the request, by the shape the provider
    # (or its own client) defines (ORCH-524); a provider with none to cite answers False
    def image_refused(self, status: int, error: dict[str, Any]) -> bool: ...
    def add_message(self, state: dict[str, Any], message: str) -> None: ...


class Bridge(Protocol):
    def tools(self, run_token: str) -> list[dict[str, Any]]: ...
    async def call(self, run_token: str, name: str, arguments: dict[str, Any]) -> dict[str, Any]: ...


class GatewayBridge:
    """FERB's gateway in-process: the tools a run token covers, and each call validated and traced by the gateway."""
    def __init__(self, gateway: Any, tokens: Any) -> None:
        self.gateway, self.tokens = gateway, tokens

    def tools(self, run_token: str) -> list[dict[str, Any]]:
        return self.gateway.tools_for(self.tokens.validate(run_token))  # type: ignore[no-any-return]

    async def call(self, run_token: str, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        return await self.gateway.call(self.tokens.validate(run_token), name, arguments)  # type: ignore[no-any-return]


def inline_refs(schema: dict[str, Any]) -> dict[str, Any]:
    """The schema with local #/$defs references written out (not every provider accepts $defs). A reference to a
    definition already being expanded becomes a plain object, so recursive models can't loop."""
    defs = schema.get("$defs", {})

    def walk(node: Any, stack: tuple[str, ...]) -> Any:
        if isinstance(node, dict):
            ref = node.get("$ref")
            if isinstance(ref, str) and ref.startswith("#/$defs/"):
                name = ref.removeprefix("#/$defs/")
                if name in stack or name not in defs:
                    return {"type": "object"}
                expanded = walk(defs[name], stack + (name,))
                siblings = {k: walk(v, stack) for k, v in node.items() if k not in ("$ref", "$defs")}
                return {**expanded, **siblings} if isinstance(expanded, dict) else expanded
            return {k: walk(v, stack) for k, v in node.items() if k != "$defs"}
        if isinstance(node, list):
            return [walk(v, stack) for v in node]
        return node
    return walk(schema, ())  # type: ignore[no-any-return]


@dataclass
class _Run:
    state: dict[str, Any]
    key: str = field(repr=False)
    token: str
    names: dict[str, str]                       # model-visible tool name -> capability name
    client: httpx.AsyncClient
    cancelled: bool = False
    closed: bool = False
    updates: list[str] = field(default_factory=list)
    inflight: "asyncio.Future[Any] | None" = None
    deadline: float | None = None               # the call's one overall limit (loop time), None for none
    timer: CallTimer | None = None              # each model call's timing on its usage report (RUN-419)


class ApiRuntime:
    def __init__(self, provider: Provider, *, mode: Literal["supervisor", "worker"], role: str,
                 tool_names: list[str] | None, output_schema: dict[str, Any] | None, bridge: Bridge | None,
                 key: Callable[[str], Awaitable[str | None]], transport: httpx.AsyncBaseTransport | None = None,
                 sleep: Callable[[float], Awaitable[None]] = asyncio.sleep, retries: int = 4, backoff_s: float = 1.0,
                 timeout_s: float = 600.0, effort: str | None = None, limit_s: float | None = None,
                 monotonic: Callable[[], float] = time.monotonic) -> None:
        self.provider, self.name = provider, provider.name
        self.monotonic = monotonic       # the clock each model call's timing reads (RUN-419); tests give a fake
        self.effort = effort             # L39: a worker run's effort when asked for ("high"); supervisor runs stay low
        self.mode, self.role, self.tool_names, self.output_schema = mode, role, list(tool_names or []), output_schema
        reshape = getattr(provider, "shape", None)
        self.shape: Shape | None = ((reshape(output_schema) if reshape is not None else same(output_schema))
                                    if output_schema else None)
        self.limit_s = limit_s           # one overall limit on this call (catalog.call_limit); None for a worker
        self.bridge, self.key, self.transport, self.sleep = bridge, key, transport, sleep
        self.retries, self.backoff_s, self.timeout_s = retries, backoff_s, timeout_s

    @property
    def limits(self) -> ModelLimits:
        """The provider's documented limits for its model (L28); ModelLimitsUnknown when it documents none."""
        from ferb.core.errors import ModelLimitsUnknown
        limits = getattr(self.provider, "limits", None)
        if limits is None:
            raise ModelLimitsUnknown(f"the {self.name} adapter reports no context window for {self.provider.model!r}")
        return limits  # type: ignore[no-any-return]

    async def start(self, task: Task, request: ModelRequest, run_token: str) -> RunHandle:
        key = await self.key(self.name)
        if not key:
            raise Unsupported(f"FERB has no {self.name} API key: add one in Settings → AI")
        tools: list[ToolSpec] = []
        names: dict[str, str] = {}
        # RUN-415: FERB's tools come with a run token (a worker's, or a planner's lookups)
        if (self.mode == "worker" or run_token) and self.bridge is not None and self.tool_names:
            offered = {t["name"]: t for t in self.bridge.tools(run_token)}
            for capability in self.tool_names:
                if capability in offered:
                    t = offered[capability]
                    tools.append(ToolSpec(mcp_name(capability), str(t["description"]), inline_refs(t["input_schema"])))
                    names[mcp_name(capability)] = capability
        state = self.provider.begin(request, tools, self.shape.wire if self.shape else None,
                                    "low" if self.mode == "supervisor" else self.effort,
                                    f"ferb:{self.role.partition(':')[0]}:{self.mode}")
        client = httpx.AsyncClient(timeout=self.timeout_s, transport=self.transport)
        deadline = asyncio.get_running_loop().time() + self.limit_s if self.limit_s is not None else None
        return RunHandle("run-" + secrets.token_hex(6), self.name, task.id,
                         {"run": _Run(state, key, run_token, names, client, deadline=deadline,
                                      timer=CallTimer(self.monotonic))})

    async def events(self, run: RunHandle) -> AsyncIterator[RuntimeEvent]:
        r: _Run = run.state["run"]
        inner = self._events(run)
        try:
            async with asyncio.timeout_at(r.deadline):
                async for event in inner:
                    yield event
        except TimeoutError:
            if not r.cancelled:
                yield RuntimeEvent("error", {"message": f"{self.name}: the call took longer than its "
                                                        f"{self.limit_s:g} s limit"})
        finally:
            await inner.aclose()              # its own clean-up runs now, not when it is garbage-collected

    async def _events(self, run: RunHandle) -> AsyncIterator[RuntimeEvent]:
        r: _Run = run.state["run"]
        try:
            while True:
                if r.cancelled:
                    return
                self._apply_updates(r)
                fragments: asyncio.Queue[str | None] = asyncio.Queue()

                async def read_turn() -> Turn:
                    try:
                        return await self._call(r, fragments.put_nowait)
                    finally:
                        fragments.put_nowait(None)

                r.inflight = asyncio.ensure_future(read_turn())
                try:
                    while (fragment := await fragments.get()) is not None:
                        if r.cancelled:
                            return
                        # "" is a streamed event with no answer text (thinking, a block starting): a sign of life for
                        # the silence guard (ORCH-528)
                        yield (RuntimeEvent('text_delta', {'text': fragment}) if fragment
                               else RuntimeEvent('progress', {}))
                    turn = await r.inflight
                except asyncio.CancelledError:
                    if r.cancelled and not _consumer_cancelled():
                        return
                    raise
                except ProviderError as e:
                    r.closed = True
                    yield RuntimeEvent("error", {"message": str(e).replace(r.key, '[key]')} |
                                       ({"overflow": True} if e.overflow else {}) |
                                       ({"account": e.account} if e.account else {}) |
                                       ({"image": True} if e.image else {}))
                    return
                # RUN-419: the call's timing, from its request going out to its last streamed event
                yield RuntimeEvent("usage", asdict(turn.usage) | ({"timing": r.timer.done()} if r.timer else {}))
                if not turn.calls:
                    if r.updates:
                        continue  # The answer predates an accepted owner correction.
                    if not turn.final:
                        r.closed = True
                        yield RuntimeEvent("error", {"message": f"{self.name}: the model ended without an answer"})
                        return
                    r.closed = True  # No update may be accepted while the final event is handled.
                    yield RuntimeEvent("final", {"text": self.shape.answer_text(turn.final) if self.shape
                                                 else turn.final})
                    return
                results = []
                for call in turn.calls:
                    if call.name not in r.names:
                        # a name this run was never offered: nothing runs, and the model is told so and goes on
                        results.append(ToolOutcome(call, f"FERB refused: no tool named {call.name!r} was offered to "
                                                         "this run; use one of the tools listed for it.", True))
                        continue
                    if not r.updates:
                        yield RuntimeEvent("tool_call", {"tool": call.name, "arguments": call.arguments})
                    if r.cancelled:
                        return
                    if r.updates:
                        # Keep every call paired with a result, but do not begin stale actions.
                        results.append(ToolOutcome(call, "Tool not executed: the owner updated the task; "
                                                   "reconsider this action using the new instructions.", True))
                        continue
                    outcome = await self._execute(r, r.names[call.name], call)
                    if outcome is None or r.cancelled:
                        return
                    results.append(outcome)
                    yield RuntimeEvent("progress", {})       # the tool's result is back: a sign of life (ORCH-528)
                self.provider.add_results(r.state, turn, results)
        finally:
            r.closed = True
            if r.inflight is not None:
                if not r.inflight.done():
                    r.inflight.cancel()
                # Closing at a yielded fragment can leave an already-failed HTTP task unobserved.
                await asyncio.gather(r.inflight, return_exceptions=True)
            await r.client.aclose()

    async def _execute(self, r: _Run, capability: str, call: ToolCall) -> ToolOutcome | None:
        """None means the run was cancelled. The gateway call runs as its own task, so the gateway cancelling its job
        (an owner stop) ends that tool call, not this whole worker."""
        if call.arguments_error:
            return ToolOutcome(call, f"FERB refused: the tool arguments were not a JSON object ({call.arguments_error})",
                               True)
        assert self.bridge is not None
        r.inflight = asyncio.ensure_future(self.bridge.call(r.token, capability, call.arguments))
        try:
            out = await r.inflight
        except FerbError as e:
            return ToolOutcome(call, f"FERB refused: {e}", True)
        except asyncio.CancelledError:
            if _consumer_cancelled():
                raise
            if r.cancelled:
                return None
            return ToolOutcome(call, "FERB stopped this tool call", True)
        text = json.dumps(out["content"], ensure_ascii=False) + "\n" + str(out.get("footer", ""))
        return ToolOutcome(call, text, bool(out.get("is_error")), out.get("image"))

    async def _call(self, r: _Run, on_delta: Callable[[str], None]) -> Turn:
        streaming = (self.mode == 'supervisor' and self.output_schema is not None
                     and hasattr(self.provider, 'stream_http'))
        request = getattr(self.provider, 'stream_http') if streaming else self.provider.http
        url, headers, body = request(r.state)
        headers = {**headers, **self.provider.auth(r.key)}
        delay = self.backoff_s
        timer = r.timer
        if timer:
            timer.begin()                    # this model call begins (RUN-421); a retry's wait counts before its send
        for attempt in range(self.retries + 1):
            emitted = False
            try:
                if timer:
                    timer.sent()
                async with r.client.stream('POST', url, headers=headers, json=body) as resp:
                    if resp.status_code in RETRY_STATUS and attempt < self.retries:
                        await resp.aread()
                        await self.sleep(_retry_after(resp) or delay)
                        delay *= 2
                        continue
                    if resp.status_code != 200:
                        await resp.aread()
                        error = _error_of(resp)
                        shape = error if isinstance(error, dict) else {}
                        raise ProviderError(self._error_text(resp, r.key), overflow=self.provider.window_exceeded(
                            resp.status_code, shape), account=ACCOUNT_STATUS.get(resp.status_code, ''),
                            image=self.provider.image_refused(resp.status_code, shape))
                    if streaming and resp.headers.get('content-type', '').split(';', 1)[0].strip().lower() == 'text/event-stream':
                        parser = getattr(self.provider, 'stream_parser')()
                        async for event in _sse_events(resp):
                            if timer:
                                timer.output()       # each streamed event is the model's output arriving
                            try:
                                text = parser.feed(event)
                            except (KeyError, TypeError, ValueError):
                                raise ProviderError(f'{self.name}: malformed response stream event') from None
                            if text:
                                emitted = True
                            on_delta(text or "")      # "" still shows the model working (ORCH-528)
                        data = parser.finish()
                    else:
                        await resp.aread()
                        if timer:
                            timer.output()           # not streamed: the whole answer arrives at once
                        try:
                            data = resp.json()
                        except ValueError:
                            raise ProviderError(f"{self.name}: the reply was not JSON") from None
                    return self.provider.parse(r.state, data)
            except httpx.TransportError as e:
                # Once a draft has been exposed, silently restarting would concatenate different answers.
                if not emitted and attempt < self.retries:
                    await self.sleep(delay)
                    delay *= 2
                    continue
                raise ProviderError(f"{self.name}: network error ({type(e).__name__})") from None
        raise ProviderError(f"{self.name}: still failing after {self.retries} retries")

    def _error_text(self, resp: httpx.Response, key: str) -> str:
        err = _error_of(resp)
        detail = (err.get("message") or err.get("type") or err.get("status") or "") if isinstance(err, dict) else str(err)
        detail = str(detail)
        if key:
            detail = detail.replace(key, "[key]")      # before truncating: a key at the cut would keep its prefix
        technical = f"HTTP {resp.status_code}" + (f": {detail[:300]}" if detail else "")
        return f"{http_cause(resp.status_code)} ({technical})"

    def _apply_updates(self, r: _Run) -> None:
        for message in r.updates:
            self.provider.add_message(r.state, message)
        r.updates.clear()

    async def continue_(self, run: RunHandle, message: str) -> None:
        r: _Run = run.state["run"]
        if r.closed or r.cancelled:
            raise Unsupported("the direct-API run has already ended")
        # An HTTP request or started tool cannot be rewritten safely. The existing loop
        # consumes these in order before its next model decision, retaining tool outcomes.
        r.updates.append(message)

    async def cancel(self, run: RunHandle) -> None:
        r: _Run = run.state["run"]
        r.cancelled = True
        if r.inflight is not None and not r.inflight.done():
            r.inflight.cancel()
        await r.client.aclose()


def _consumer_cancelled() -> bool:
    task = asyncio.current_task()
    return task is not None and task.cancelling() > 0


async def _sse_events(response: httpx.Response) -> AsyncIterator[dict[str, Any]]:
    """Decode SSE framing, including multiline data and transport splits, without logging raw provider bytes."""
    data: list[str] = []

    def decode() -> dict[str, Any] | None:
        text = '\n'.join(data)
        data.clear()
        if not text or text == '[DONE]':
            return None
        try:
            event = json.loads(text)
        except ValueError:
            raise ProviderError('The response stream contained invalid JSON') from None
        if not isinstance(event, dict):
            raise ProviderError('The response stream contained a non-object event')
        return event

    async for line in response.aiter_lines():
        if not line:
            if (event := decode()) is not None:
                yield event
        elif line.startswith('data:'):
            data.append(line[5:].removeprefix(' '))
    if (event := decode()) is not None:
        yield event


def _retry_after(resp: httpx.Response) -> float | None:
    try:
        return min(float(resp.headers.get("retry-after", "")), 60.0)
    except ValueError:
        return None


def _error_of(resp: httpx.Response) -> Any:
    """An error response's `error` value, as the provider sent it ({} when the body has none)."""
    try:
        return resp.json().get("error") or {}
    except (ValueError, AttributeError):
        return {}
