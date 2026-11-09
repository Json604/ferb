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
- function_call arguments arrive as a JSON string; results go back as function_call_output, with images as input_image.

Typed-answer limits (RUN-427): OpenAI documents its strict-schema limits on the structured-outputs page above; FERB's
repository records no checked figures or refusal wording for them (not verified for this change), so none are enforced
here and no wording is matched. A 400 on a request carrying `text.format` goes through the
shared fallback in api.py (`without_format` below), which tells a format refusal from any other 400 by resending
without the format."""

import json
from typing import Any

from ferb.core.types import ModelLimits, ModelRequest
from ferb.runtimes.api import ProviderError, ToolCall, ToolOutcome, ToolSpec, Turn
from ferb.runtimes.usage import from_openai

URL = "https://api.openai.com/v1/responses"
EXPLICIT_CACHE = ("gpt-6", "gpt-5.6")
BREAKPOINT = {"mode": "explicit"}
# L28: each model's documented limits (context window, max output tokens), checked 2026-10-14 on
# https://developers.openai.com/api/docs/models/<model id> (gpt-5.5, gpt-5.6-sol: "Maximum input is 922,000 tokens",
# gpt-6-astra) and the model list at https://developers.openai.com/api/docs/models (gpt-6.1-sol, gpt-6-luna). Facts, not FERB choices; a model missing
# here has no documented limits FERB knows of, and using it is an explicit error.
LIMITS: dict[str, tuple[int, int]] = {
    "gpt-5.5": (1_050_000, 128_000),
    "gpt-5.6-sol": (1_050_000, 128_000),
    # https://developers.openai.com/api/docs/models/gpt-5.6-luna (checked 2026-11-08): 1,050,000 context window,
    # 128,000 max output tokens
    "gpt-5.6-luna": (1_050_000, 128_000),
    "gpt-6-astra": (1_050_000, 128_000),
    "gpt-6.1-sol": (1_050_000, 128_000),
    "gpt-6-luna": (1_050_000, 128_000),
}
LIMITS_SOURCE = "https://developers.openai.com/api/docs/models (checked 2026-10-14)"
# The image error codes of OpenAI's Responses API (openai-python src/openai/types/responses/response_error.py, `code`,
# checked 2026-11-04): the ones naming an image the request carried
IMAGE_CODES = frozenset({"invalid_image", "invalid_image_format", "invalid_base64_image", "invalid_image_url",
                         "image_too_large", "image_too_small", "image_parse_error", "image_content_policy_violation",
                         "invalid_image_mode", "image_file_too_large", "unsupported_image_media_type",
                         "empty_image_file", "failed_to_download_image", "image_file_not_found"})


def limits_for(model: str) -> ModelLimits:
    if model not in LIMITS:                         # RUN-428: a model the owner typed: the smallest limits FERB knows
        window, output = min(w for w, _ in LIMITS.values()), min(o for _, o in LIMITS.values())
        return ModelLimits(model, window, output, f"not documented to FERB for {model}: the smallest limits it knows "
                                                  f"for OpenAI models ({LIMITS_SOURCE})")
    window, output = LIMITS[model]
    return ModelLimits(model, window, output, LIMITS_SOURCE)


class OpenAIProvider:
    name = "openai"

    def __init__(self, model: str | None = None) -> None:
        from ferb.runtimes.catalog import CATALOG
        self.model = model or CATALOG["openai"].default_model
        self.explicit = self.model.startswith(EXPLICIT_CACHE)    # FERB marks the cacheable prefix; else implicit

    @property
    def limits(self) -> ModelLimits:
        return limits_for(self.model)

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
                                                  for i, p in enumerate(request.parts)] +
                      [{"type": "input_image", "image_url": i.data_url()} for i in request.images]})   # L12
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

    def stream_parser(self, answer: str = "") -> "OpenAIStream":
        return OpenAIStream(answer)

    def auth(self, key: str) -> dict[str, str]:
        return {"authorization": f"Bearer {key}"}

    def parse(self, state: dict[str, Any], data: dict[str, Any]) -> Turn:
        status = data.get("status")
        if status == "incomplete":
            reason = (data.get("incomplete_details") or {}).get("reason", "unknown")
            raise ProviderError(f"openai: the reply is incomplete ({reason}); it was cut off")
        if status == "failed":
            error = data.get("error")
            failure: dict[str, Any] = error if isinstance(error, dict) else {}
            detail = str(failure.get("message") or "")[:300]
            raise ProviderError("openai: the reply failed" + (f": {detail}" if detail else ""),
                                image=self.image_refused(0, failure))
        if status != "completed":        # FERB never emits a partial answer (F2-R6)
            raise ProviderError(f"openai: the reply did not complete ({status})")
        output = data.get("output") or []
        state["body"]["input"].extend(output)            # replay every output item, reasoning included
        calls = []
        for item in output:
            if item.get("type") != "function_call":
                continue
            try:
                args = json.loads(str(item.get("arguments") or "").strip() or "{}")   # none: no arguments
                error = "" if isinstance(args, dict) else "not a JSON object"
            except ValueError:
                args, error = {}, "not a JSON object"
            calls.append(ToolCall(str(item["call_id"]), str(item["name"]), args if not error else {}, error))
        parts = [c for item in output if item.get("type") == "message" for c in item.get("content", [])]
        if any(c.get("type") == "refusal" for c in parts):
            raise ProviderError("openai: the model refused to answer")
        text = "".join(c.get("text", "") for c in parts if c.get("type") == "output_text")
        return Turn(calls, None if calls else text, from_openai(self.model, data.get("usage") or {}, billing="api"),
                    str(data.get("status") or ""), tuple(str(item.get("type")) for item in output))

    def add_message(self, state: dict[str, Any], message: str) -> None:
        state["body"]["input"].append({"role": "user", "content": [self._text(message, False)]})

    def without_format(self, state: dict[str, Any], statement: str, tool: ToolSpec | None = None) -> None:
        """RUN-427: no text.format; the answer form goes at the end of FERB's developer message, inside its cache
        breakpoint (explicit models) or its implicitly cached prefix. RUN-447: `tool`, the answer tool, follows FERB's
        tools as a non-strict function, as theirs are."""
        body = state["body"]
        body.pop("text", None)
        if tool is not None:
            body.setdefault("tools", []).append({"type": "function", "name": tool.name, "description": tool.description,
                                                 "parameters": tool.schema, "strict": False})
        items = body["input"]
        if items and items[0].get("role") == "developer":
            items[0]["content"][0]["text"] += "\n\n" + statement
        else:
            items.insert(0, {"role": "developer", "content": [self._text(statement, True)]})

    def window_exceeded(self, status: int, error: dict[str, Any]) -> bool:
        """The `error.code` OpenAI's API returns for an input longer than the model's context window, read from that
        code field, never from the message (fix wave 6b round 1)."""
        return error.get("code") == "context_length_exceeded"

    def image_refused(self, status: int, error: dict[str, Any]) -> bool:
        """An `error.code` of OpenAI's for an image it couldn't take (IMAGE_CODES), read from that code field, never
        from the message (ORCH-524)."""
        return error.get("code") in IMAGE_CODES

    def add_results(self, state: dict[str, Any], turn: Turn, results: list[ToolOutcome]) -> None:
        for r in results:
            text = ("Tool error: " + r.text) if r.is_error else r.text
            out: Any = text
            if r.image:
                out = [{"type": "input_text", "text": text},
                       {"type": "input_image", "image_url": f"data:{r.image['mime']};base64,{r.image['data']}"}]
            state["body"]["input"].append({"type": "function_call_output", "call_id": r.call.id, "output": out})


class OpenAIStream:
    """Responses SSE carries text deltas and the authoritative complete response separately. The answer tool's
    (`answer`, RUN-447) argument deltas are the answer's text from its call's start (`answer_began`); text after it
    isn't."""
    def __init__(self, answer: str = "") -> None:
        self.response: dict[str, Any] | None = None
        self.answer = answer
        self.answer_item: str | None = None        # the answer call's item id, once it starts
        self.answer_began = False                  # set when it starts; the loop reads and clears it

    def feed(self, event: dict[str, Any]) -> str:
        kind = event.get('type')
        if kind == 'response.output_item.added' and self.answer and self.answer_item is None:
            item = event.get('item') or {}
            if item.get('type') == 'function_call' and item.get('name') == self.answer:
                self.answer_item, self.answer_began = str(item.get('id') or ''), True
            return ''
        if kind == 'response.function_call_arguments.delta':
            return (str(event.get('delta') or '') if self.answer_item is not None
                    and str(event.get('item_id') or '') == self.answer_item else '')
        if kind == 'response.output_text.delta':
            return str(event.get('delta') or '') if self.answer_item is None else ''
        if kind in ('response.completed', 'response.failed', 'response.incomplete'):
            self.response = event.get('response')
        elif kind == 'error':
            raise ProviderError('openai: the response stream failed')
        return ''

    def finish(self) -> dict[str, Any]:
        if not isinstance(self.response, dict):
            raise ProviderError('openai: the response stream ended before completion')
        return self.response
