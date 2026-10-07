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

from typing import Any

from ferb.core.types import Capabilities, ModelRequest
from ferb.runtimes.api import ProviderError, ToolCall, ToolOutcome, ToolSpec, Turn
from ferb.runtimes.usage import from_anthropic

URL = "https://api.anthropic.com/v1/messages"
VERSION = "2023-06-01"
DEFAULT_MODEL = "claude-sonnet-5-5"
MAX_TOKENS = 32000
NO_EFFORT = ("claude-haiku-4-5",)
CACHE = {"type": "ephemeral"}


class AnthropicProvider:
    name = "anthropic"
    capabilities = Capabilities("direct", "markers", False, "native", True, True, False, False, "api")

    def __init__(self, model: str = DEFAULT_MODEL) -> None:
        self.model = model

    def begin(self, request: ModelRequest, tools: list[ToolSpec], schema: dict[str, Any] | None, effort: str | None,
              cache_key: str) -> dict[str, Any]:
        content: list[dict[str, Any]] = [{"type": "text", "text": p} for p in request.parts]
        if 0 < request.cacheable <= len(content):
            content[request.cacheable - 1]["cache_control"] = CACHE
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

    def add_results(self, state: dict[str, Any], turn: Turn, results: list[ToolOutcome]) -> None:
        blocks = []
        for r in results:
            content: list[dict[str, Any]] = [{"type": "text", "text": r.text}]
            if r.image:
                content.append({"type": "image", "source": {"type": "base64", "media_type": r.image["mime"],
                                                            "data": r.image["data"]}})
            blocks.append({"type": "tool_result", "tool_use_id": r.call.id, "content": content, "is_error": r.is_error})
        state["body"]["messages"].append({"role": "user", "content": blocks})
