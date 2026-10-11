"""Owns the OpenAI Responses API translation for FERB's tool loop (D32). Must not choose targets or add tools.

Verified 2026-10-07 (https://developers.openai.com/api/reference/resources/responses/methods/create,
/api/docs/guides/prompt-caching, /structured-outputs, /reasoning, /function-calling):
- FERB's instructions go in a developer message (top-level `instructions` can't carry a cache breakpoint);
- cache: on GPT-5.6 and later (all gpt-6.x), explicit breakpoints on the instructions and on the last cacheable context
  part (P20), within the 4 writes allowed; on gpt-5.5 and earlier caching is implicit and prompt_cache_key steers routing,
  so the key stays the same for every call of one role (FERB measured per-process keys defeating Codex's cache);
- stateless (store false): every output item, reasoning included, is replayed on the next call;
- typed answers: text.format json_schema strict (FERB's schemas are strict-compatible); tools stay non-strict because
  FERB's tool schemas have optional fields;
- function_call arguments arrive as a JSON string; results go back as function_call_output, with images as input_image."""

import json
from typing import Any

from ferb.core.types import Capabilities, ModelRequest
from ferb.runtimes.api import ProviderError, ToolCall, ToolOutcome, ToolSpec, Turn
from ferb.runtimes.usage import from_openai

URL = "https://api.openai.com/v1/responses"
DEFAULT_MODEL = "gpt-6.1-sol"
EXPLICIT_CACHE = ("gpt-6", "gpt-5.6")
BREAKPOINT = {"mode": "explicit"}


def capabilities_for(model: str) -> Capabilities:
    explicit = model.startswith(EXPLICIT_CACHE)
    return Capabilities("direct", "markers" if explicit else "prefix", True, "native", True, True, True, True, "api")


class OpenAIProvider:
    name = "openai"

    def __init__(self, model: str = DEFAULT_MODEL) -> None:
        self.model = model
        self.capabilities = capabilities_for(model)
        self.explicit = model.startswith(EXPLICIT_CACHE)

    def _text(self, text: str, mark: bool) -> dict[str, Any]:
        block: dict[str, Any] = {"type": "input_text", "text": text}
        if mark and self.explicit:
            block["prompt_cache_breakpoint"] = BREAKPOINT
        return block

    def begin(self, request: ModelRequest, tools: list[ToolSpec], schema: dict[str, Any] | None, effort: str | None,
              cache_key: str) -> dict[str, Any]:
        items: list[dict[str, Any]] = []
        if request.system:
            items.append({"role": "developer", "content": [self._text(request.system, True)]})
        items.append({"role": "user", "content": [self._text(p, i == request.cacheable - 1)
                                                  for i, p in enumerate(request.parts)]})
        # store false: replayed reasoning items need their encrypted content, so ask for it on every call
        body: dict[str, Any] = {"model": self.model, "input": items, "store": False, "prompt_cache_key": cache_key,
                                "include": ["reasoning.encrypted_content"]}
        if tools:
            body["tools"] = [{"type": "function", "name": t.name, "description": t.description, "parameters": t.schema,
                              "strict": False} for t in tools]
        if schema:
            body["text"] = {"format": {"type": "json_schema", "name": "ferb_result", "schema": schema, "strict": True}}
        if effort:
            body["reasoning"] = {"effort": effort}
        return {"body": body}

    def http(self, state: dict[str, Any]) -> tuple[str, dict[str, str], dict[str, Any]]:
        return URL, {"content-type": "application/json"}, state["body"]

    def stream_http(self, state: dict[str, Any]) -> tuple[str, dict[str, str], dict[str, Any]]:
        return URL, {"content-type": "application/json"}, {**state['body'], 'stream': True}

    def stream_parser(self) -> "OpenAIStream":
        return OpenAIStream()

    def auth(self, key: str) -> dict[str, str]:
        return {"authorization": f"Bearer {key}"}

    def parse(self, state: dict[str, Any], data: dict[str, Any]) -> Turn:
        status = data.get("status")
        if status == "incomplete":
            reason = (data.get("incomplete_details") or {}).get("reason", "unknown")
            raise ProviderError(f"openai: the reply is incomplete ({reason}); it was cut off")
        if status == "failed":
            detail = str((data.get("error") or {}).get("message") or "")[:300]
            raise ProviderError("openai: the reply failed" + (f": {detail}" if detail else ""))
        if status != "completed":        # FERB never emits a partial answer (F2-R6)
            raise ProviderError(f"openai: the reply did not complete ({status})")
        output = data.get("output") or []
        state["body"]["input"].extend(output)            # replay every output item, reasoning included
        calls = []
        for item in output:
            if item.get("type") != "function_call":
                continue
            try:
                args = json.loads(item.get("arguments") or "{}")
                error = "" if isinstance(args, dict) else "not a JSON object"
            except ValueError:
                args, error = {}, "not a JSON object"
            calls.append(ToolCall(str(item["call_id"]), str(item["name"]), args if not error else {}, error))
        parts = [c for item in output if item.get("type") == "message" for c in item.get("content", [])]
        if any(c.get("type") == "refusal" for c in parts):
            raise ProviderError("openai: the model refused to answer")
        text = "".join(c.get("text", "") for c in parts if c.get("type") == "output_text")
        return Turn(calls, None if calls else text, from_openai(self.model, data.get("usage") or {}, billing="api"),
                    str(data.get("status") or ""))

    def add_message(self, state: dict[str, Any], message: str) -> None:
        state["body"]["input"].append({"role": "user", "content": [self._text(message, False)]})

    def add_results(self, state: dict[str, Any], turn: Turn, results: list[ToolOutcome]) -> None:
        for r in results:
            text = ("Tool error: " + r.text) if r.is_error else r.text
            out: Any = text
            if r.image:
                out = [{"type": "input_text", "text": text},
                       {"type": "input_image", "image_url": f"data:{r.image['mime']};base64,{r.image['data']}"}]
            state["body"]["input"].append({"type": "function_call_output", "call_id": r.call.id, "output": out})


class OpenAIStream:
    """Responses SSE carries text deltas and the authoritative complete response separately."""
    def __init__(self) -> None:
        self.response: dict[str, Any] | None = None

    def feed(self, event: dict[str, Any]) -> str:
        kind = event.get('type')
        if kind == 'response.output_text.delta':
            return str(event.get('delta') or '')
        if kind in ('response.completed', 'response.failed', 'response.incomplete'):
            self.response = event.get('response')
        elif kind == 'error':
            raise ProviderError('openai: the response stream failed')
        return ''

    def finish(self) -> dict[str, Any]:
        if not isinstance(self.response, dict):
            raise ProviderError('openai: the response stream ended before completion')
        return self.response
