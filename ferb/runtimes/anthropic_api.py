"""Owns the Anthropic Messages API translation for FERB's tool loop (D32). Must not choose targets or add tools.

Verified 2026-10-07 (https://platform.claude.com/docs/en/build-with-claude/prompt-caching, /structured-outputs,
/agents-and-tools/tool-use/define-tools, /build-with-claude/effort):
- cache: breakpoints on the system block and on the last cacheable context part (P20), plus the top-level automatic
  breakpoint that follows the growing tool loop: 3 of the 4 allowed;
- typed answers: output_config.format json_schema (GA, works with tools); tool_choice stays "auto" because "any"/"tool"
  return 400 on Opus 5.5, Sonnet 5.5 and Fable 5.1;
- effort: output_config.effort (not supported by Haiku 4.5); thinking stays adaptive (disabling it returns 400) and the
  assistant's content, thinking blocks included, goes back unchanged;
- max_tokens is required: MAX_TOKENS is a per-reply ceiling, and a reply cut off there is an error, never a partial answer.
- only end_turn, tool_use and stop_sequence are complete turns; refusal, pause_turn (FERB enables no server tools) and
  model_context_window_exceeded are errors.

Typed-answer limits (RUN-408, RUN-427). Counted, documented
(https://platform.claude.com/docs/en/build-with-claude/structured-outputs, "Schema complexity limits", checked
2026-10-30): at most 24 optional parameters and 16 parameters with union types across a request's strict schemas;
schema_shape.for_anthropic reshapes to fit them. The page also says (as quoted 2026-11-08, not re-read) that a request
takes at most 20 strict tools, unions have an exponential compilation cost, each optional parameter roughly doubles part
of the grammar's state space, and compiled grammars are cached 24 h. Not counted, not documented: the compiled
grammar's size. A 400 "The compiled grammar is too large, which would cause performance issues. Simplify your tool
schemas or reduce the number of strict tools." came back on every planner turn on 2026-11-08 (the owner's journal,
05:20, 06:00, 07:10) for SUPERVISOR_SCHEMA, within the counted limits after reshaping. Claude Code (2.1.310, bundled
in claude-agent-sdk 0.2.174) sends its typed output as the same output_config.format and has a fallback of its own only
for a model without the feature ("output_config.format: extra inputs are not permitted" or
"structured_outputs_unsupported": it latches that model and resends without the format). FERB's is the shared one in
api.py, through `without_format` below."""

import json
from typing import Any

from ferb.core.errors import ModelLimitsUnknown
from ferb.core.types import ModelLimits, ModelRequest
from ferb.runtimes.api import ProviderError, ToolCall, ToolOutcome, ToolSpec, Turn
from ferb.runtimes.schema_shape import Shape, for_anthropic
from ferb.runtimes.usage import from_anthropic

URL = "https://api.anthropic.com/v1/messages"
VERSION = "2023-06-01"
MAX_TOKENS = 32000
NO_EFFORT = ("claude-haiku-4-5",)
CACHE = {"type": "ephemeral"}
# L28: each model's documented limits (context window, max output on the synchronous Messages API), checked 2026-10-14
# on https://platform.claude.com/docs/en/about-claude/models/overview (Fable 5.1, Opus 5.5, Sonnet 5.5, Haiku 5.5) and
# https://platform.claude.com/docs/en/models/haiku-4-5/overview (Haiku 4.5, legacy). Facts, not FERB choices.
LIMITS: dict[str, tuple[int, int]] = {
    "claude-fable-5-1": (1_000_000, 128_000),
    "claude-opus-5-5": (1_000_000, 128_000),
    "claude-sonnet-5-5": (1_000_000, 128_000),
    "claude-haiku-5-5": (1_000_000, 128_000),
    "claude-haiku-4-5": (200_000, 64_000),
    "claude-haiku-4-5-20251001": (200_000, 64_000),
}
LIMITS_SOURCE = "https://platform.claude.com/docs/en/about-claude/models/overview (checked 2026-10-14)"
# The message prefixes of the API's over-window rejection (`window_exceeded`), lowercased
WINDOW_EXCEEDED = ("prompt is too long", "input length and `max_tokens` exceed context limit")
# The API's image rejections (`image_refused`) as Anthropic's own client classifies them (Claude Code 2.1.310, bundled in
# claude-agent-sdk 0.2.174): a 400 whose message holds "image exceeds" and "maximum" (image_too_large), "image
# dimensions exceed" and "many-image" (image_too_large), or "Could not process image" (image_unprocessable); lowercased
IMAGE_REFUSALS = (("image exceeds", "maximum"), ("image dimensions exceed", "many-image"), ("could not process image",))


def limits_for(model: str) -> ModelLimits:
    if model not in LIMITS:
        raise ModelLimitsUnknown(f"Anthropic documents no context window FERB knows for model {model!r}; "
                                 f"the ones it knows: {', '.join(sorted(LIMITS))}")
    window, output = LIMITS[model]
    return ModelLimits(model, window, output, LIMITS_SOURCE)


class AnthropicProvider:
    name = "anthropic"

    def __init__(self, model: str | None = None) -> None:
        from ferb.runtimes.catalog import CATALOG
        self.model = model or CATALOG["anthropic"].default_model

    def shape(self, schema: dict[str, Any]) -> Shape:
        """Within the API's union and optional-parameter limits for typed output (schema_shape.for_anthropic)."""
        return for_anthropic(schema)

    @property
    def limits(self) -> ModelLimits:
        return limits_for(self.model)

    def begin(self, request: ModelRequest, tools: list[ToolSpec], schema: dict[str, Any] | None, effort: str | None,
              cache_key: str) -> dict[str, Any]:
        content: list[dict[str, Any]] = [{"type": "text", "text": p} for p in request.parts]
        if 0 < request.cacheable <= len(content):
            content[request.cacheable - 1]["cache_control"] = CACHE
        content += [{"type": "image", "source": {"type": "base64", "media_type": i.mime, "data": i.base64()}}
                    for i in request.images]                       # after the text: the cached prefix stays (L12)
        body: dict[str, Any] = {"model": self.model, "max_tokens": MAX_TOKENS, "cache_control": CACHE,
                                "messages": [{"role": "user", "content": content}]}
        if request.system:
            body["system"] = [{"type": "text", "text": request.system, "cache_control": CACHE}]
        if tools:
            body["tools"] = [{"name": t.name, "description": t.description, "input_schema": t.schema} for t in tools]
            body["tool_choice"] = {"type": "auto"}
        output: dict[str, Any] = {}
        if schema:
            output["format"] = {"type": "json_schema", "schema": schema}
        if effort and not self.model.startswith(NO_EFFORT):
            output["effort"] = effort
        if output:
            body["output_config"] = output
        return {"body": body}

    def http(self, state: dict[str, Any]) -> tuple[str, dict[str, str], dict[str, Any]]:
        return URL, {"anthropic-version": VERSION, "content-type": "application/json"}, state["body"]

    def stream_http(self, state: dict[str, Any]) -> tuple[str, dict[str, str], dict[str, Any]]:
        url, headers, body = self.http(state)
        return url, headers, {**body, 'stream': True}

    def stream_parser(self) -> "AnthropicStream":
        return AnthropicStream()

    def auth(self, key: str) -> dict[str, str]:
        return {"x-api-key": key}

    def parse(self, state: dict[str, Any], data: dict[str, Any]) -> Turn:
        content = data.get("content") or []
        stop = str(data.get("stop_reason") or "")
        if stop == "max_tokens":
            raise ProviderError("anthropic: the reply was cut off at the output limit")
        if stop == "refusal":
            raise ProviderError("anthropic: the model refused to answer")
        if stop not in ("end_turn", "tool_use", "stop_sequence"):
            raise ProviderError(f"anthropic: the reply stopped early ({stop})",
                                overflow=stop == "model_context_window_exceeded")
        calls = [ToolCall(b["id"], b["name"], b["input"] if isinstance(b.get("input"), dict) else {},
                          "" if isinstance(b.get("input"), dict) else
                          "not valid JSON" if isinstance(b.get("input"), str) else "not a JSON object")
                 for b in content if b.get("type") == "tool_use"]
        # the turn goes back as it was, a call's unreadable arguments as {} (the API takes only an object there)
        replay = [{**b, "input": b["input"] if isinstance(b.get("input"), dict) else {}}
                  if b.get("type") == "tool_use" else b for b in content]
        state["body"]["messages"].append({"role": "assistant", "content": replay})
        text = "".join(b.get("text", "") for b in content if b.get("type") == "text")
        return Turn(calls, None if calls else text, from_anthropic(self.model, data.get("usage") or {}, billing="api"), stop)

    def add_message(self, state: dict[str, Any], message: str) -> None:
        state["body"]["messages"].append({"role": "user", "content": [{"type": "text", "text": message}]})

    def window_exceeded(self, status: int, error: dict[str, Any]) -> bool:
        """The Messages API's rejection of an input over the model's window: a 400 `invalid_request_error` whose message
        starts "prompt is too long" ("prompt is too long: N tokens > M maximum"), or "input length and `max_tokens`
        exceed context limit" when the input and the reply ceiling together exceed it. Anthropic's own client defines
        both shapes (Claude Code 2.1.283 and 2.1.310, bundled in claude-agent-sdk 0.2.160 and 0.2.174: `"Prompt is too
        long"` with `/prompt is too long[^0-9]*(\\d+)\\s*tokens?\\s*>\\s*(\\d+)/i`, and its `max_tokens_context_overflow`
        class);
        the API's errors page documents the status and type, not the message (fix wave 6b round 2)."""
        message = str(error.get("message") or "").lower()
        return status == 400 and error.get("type") == "invalid_request_error" and message.startswith(WINDOW_EXCEEDED)

    def image_refused(self, status: int, error: dict[str, Any]) -> bool:
        """The Messages API's rejection of an image (ORCH-524): a 400 in one of the shapes Anthropic's own client
        classifies as an image error (IMAGE_REFUSALS); the API's errors page documents the status and type only, and a
        schema or other request error has the same status and type, so the shape is what tells them apart."""
        message = str(error.get("message") or "").lower()
        return status == 400 and any(all(part in message for part in shape) for shape in IMAGE_REFUSALS)

    def without_format(self, state: dict[str, Any], statement: str) -> None:
        """RUN-427: no output_config.format; the answer form goes at the end of the system block, which keeps its cache
        breakpoint, so the stated form is cached with FERB's instructions."""
        body = state["body"]
        output = body.get("output_config") or {}
        output.pop("format", None)
        if not output:
            body.pop("output_config", None)
        system = body.get("system")
        if system:
            system[-1]["text"] += "\n\n" + statement
        else:
            body["system"] = [{"type": "text", "text": statement, "cache_control": CACHE}]

    def add_results(self, state: dict[str, Any], turn: Turn, results: list[ToolOutcome]) -> None:
        blocks = []
        for r in results:
            content: list[dict[str, Any]] = [{"type": "text", "text": r.text}]
            if r.image:
                content.append({"type": "image", "source": {"type": "base64", "media_type": r.image["mime"],
                                                            "data": r.image["data"]}})
            blocks.append({"type": "tool_result", "tool_use_id": r.call.id, "content": content, "is_error": r.is_error})
        state["body"]["messages"].append({"role": "user", "content": blocks})


class AnthropicStream:
    """Assemble indexed Messages blocks without forwarding thinking or tool arguments as answer text."""
    def __init__(self) -> None:
        self.message: dict[str, Any] | None = None
        self.blocks: dict[int, dict[str, Any]] = {}
        self.arguments: dict[int, str] = {}
        self.stopped = False

    def feed(self, event: dict[str, Any]) -> str:
        kind = event.get('type')
        if kind == 'message_start':
            self.message = dict(event['message'])
        elif kind == 'content_block_start':
            started = dict(event['content_block'])
            self.blocks[event['index']] = started
            return str(started.get('text') or '') if started.get('type') == 'text' else ''
        elif kind == 'content_block_delta':
            index, delta = event['index'], event['delta']
            block = self.blocks.get(index)
            if block is None:
                raise ProviderError('anthropic: a response fragment had no content block')
            delta_type = delta.get('type')
            if delta_type == 'text_delta' and block.get('type') == 'text':
                text = str(delta.get('text') or '')
                block['text'] = block.get('text', '') + text
                return text
            if delta_type == 'thinking_delta':
                block['thinking'] = block.get('thinking', '') + str(delta.get('thinking') or '')
            elif delta_type == 'signature_delta':
                block['signature'] = block.get('signature', '') + str(delta.get('signature') or '')
            elif delta_type == 'input_json_delta':
                self.arguments[index] = self.arguments.get(index, '') + str(delta.get('partial_json') or '')
            elif delta_type == 'citations_delta':
                block.setdefault('citations', []).append(delta.get('citation'))
        elif kind == 'content_block_stop':
            index = event['index']
            if index in self.arguments:
                raw = self.arguments[index]
                if not raw.strip():          # a tool called with no arguments: its block's own input, else {}
                    started = self.blocks[index].get('input')
                    self.blocks[index]['input'] = started if isinstance(started, dict) else {}
                else:
                    try:
                        self.blocks[index]['input'] = json.loads(raw)
                    except ValueError:       # told back to the model as the call's error (its one repair), not fatal
                        self.blocks[index]['input'] = raw
        elif kind == 'message_delta' and self.message is not None:
            self.message.update(event.get('delta') or {})
            self.message.setdefault('usage', {}).update(event.get('usage') or {})
        elif kind == 'message_stop':
            self.stopped = True
        elif kind == 'error':
            raise ProviderError('anthropic: the response stream failed')
        return ''

    def finish(self) -> dict[str, Any]:
        if self.message is None or not self.stopped:
            raise ProviderError('anthropic: the response stream ended before completion')
        return {**self.message, 'content': [self.blocks[index] for index in sorted(self.blocks)]}
