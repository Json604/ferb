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
  model_context_window_exceeded are errors."""

import json
from typing import Any

from ferb.core.errors import ModelLimitsUnknown
from ferb.core.types import Capabilities, ModelLimits, ModelRequest
from ferb.runtimes.api import ProviderError, ToolCall, ToolOutcome, ToolSpec, Turn
from ferb.runtimes.usage import from_anthropic

URL = "https://api.anthropic.com/v1/messages"
VERSION = "2023-06-01"
DEFAULT_MODEL = "claude-sonnet-5-5"
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


def limits_for(model: str) -> ModelLimits:
    if model not in LIMITS:
        raise ModelLimitsUnknown(f"Anthropic documents no context window FERB knows for model {model!r}; "
                                 f"choose one of {sorted(LIMITS)} in config.toml [models]")
    window, output = LIMITS[model]
    return ModelLimits(model, window, output, LIMITS_SOURCE)


class AnthropicProvider:
    name = "anthropic"
    capabilities = Capabilities("direct", "markers", False, "native", True, True, True, True, "api")

    def __init__(self, model: str = DEFAULT_MODEL) -> None:
        self.model = model

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
            raise ProviderError(f"anthropic: the reply stopped early ({stop})")
        state["body"]["messages"].append({"role": "assistant", "content": content})
        calls = [ToolCall(b["id"], b["name"], b["input"] if isinstance(b.get("input"), dict) else {},
                          "" if isinstance(b.get("input"), dict) else "not a JSON object")
                 for b in content if b.get("type") == "tool_use"]
        text = "".join(b.get("text", "") for b in content if b.get("type") == "text")
        return Turn(calls, None if calls else text, from_anthropic(self.model, data.get("usage") or {}, billing="api"), stop)

    def add_message(self, state: dict[str, Any], message: str) -> None:
        state["body"]["messages"].append({"role": "user", "content": [{"type": "text", "text": message}]})

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
                try:
                    self.blocks[index]['input'] = json.loads(self.arguments[index])
                except ValueError:
                    raise ProviderError('anthropic: streamed tool arguments were not JSON') from None
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
