# tests/support/runtime_fakes.py
"""Test doubles for the direct-API runtimes: a scripted HTTP transport, FERB's gateway as a bridge, a minimal provider."""
import asyncio
import json
from typing import Any

import httpx

from ferb.core.types import Capabilities
from ferb.runtimes.api import ProviderError, ToolCall, Turn
from ferb.runtimes.usage import Usage

FS_LIST = {"name": "fs.list", "description": "List a folder.",
           "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}


class FakeBridge:
    def __init__(self, tools: list[dict[str, Any]] | None = None, result: dict[str, Any] | None = None) -> None:
        self._tools = tools if tools is not None else [FS_LIST]
        self.result = result or {"content": {"entries": ["a.txt"]}, "footer": "", "is_error": False, "image": None}
        self.calls: list[tuple[str, dict[str, Any]]] = []

    def tools(self, run_token: str) -> list[dict[str, Any]]:
        return self._tools

    async def call(self, run_token: str, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        self.calls.append((name, arguments))
        return self.result


class Scripted:
    """Replays (status, json body) responses in order; records each request's headers and JSON body.
    A response of None blocks until the test cancels the run."""
    def __init__(self, responses: list[tuple[int, Any] | None]) -> None:
        self.responses = list(responses)
        self.requests: list[tuple[dict[str, str], dict[str, Any]]] = []

    async def _handle(self, request: httpx.Request) -> httpx.Response:
        self.requests.append((dict(request.headers), json.loads(request.content)))
        item = self.responses.pop(0)
        if item is None:
            await asyncio.Event().wait()
        status, body = item  # type: ignore[misc]
        return httpx.Response(status, json=body)

    def transport(self) -> httpx.MockTransport:
        return httpx.MockTransport(self._handle)


class FakeProvider:
    """A provider whose wire format is FERB-made: {"calls": [[id, name, args]], "text": ..., "usage": n}."""
    name = "fake"
    model = "fake-model"
    capabilities = Capabilities("direct", "prefix", True, "native", True, True, False, False, "api")

    def begin(self, request, tools, schema, effort, cache_key) -> dict[str, Any]:
        return {"body": {"system": request.system, "parts": list(request.parts), "cacheable": request.cacheable,
                         "tools": [t.name for t in tools], "schema": schema, "effort": effort, "cache_key": cache_key,
                         "results": []}}

    def http(self, state):
        return "https://provider.test/v1", {"content-type": "application/json"}, state["body"]

    def auth(self, key: str) -> dict[str, str]:
        return {"x-key": key}

    def parse(self, state, data) -> Turn:
        if data.get("cut"):
            raise ProviderError("fake: the reply was cut off at the output limit")
        calls = [ToolCall(c[0], c[1], c[2] if isinstance(c[2], dict) else {},
                          "" if isinstance(c[2], dict) else "not a JSON object") for c in data.get("calls", [])]
        return Turn(calls, None if calls else data.get("text"),
                    Usage(self.model, data.get("usage", 1), 0, 0, 1, 0, None, "unknown", "api"))

    def add_results(self, state, turn, results) -> None:
        state["body"]["results"].append([[r.call.id, r.text, r.is_error, r.image] for r in results])

    def add_message(self, state, message) -> None:
        state["body"].setdefault("updates", []).append(message)
