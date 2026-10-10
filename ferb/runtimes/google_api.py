"""Owns the Gemini API (generateContent) translation for FERB's tool loop (D32). Must not choose targets or add tools.

Verified 2026-10-07 (https://ai.google.dev/api/generate-content, /gemini-api/docs/generate-content/function-calling,
/structured-output, /thinking, /caching, /thought-signatures):
- function declarations use parametersJsonSchema (JSON Schema; mutually exclusive with the OpenAPI-subset `parameters`);
- typed answers: responseMimeType application/json + responseJsonSchema; together with tools this works on Gemini 3 only
  (gemini-3.8-flash, gemini-3.1-pro-preview), so DEFAULT_MODEL is a Gemini 3 model;
- function-call parts carry a thoughtSignature that must go back exactly: FERB appends the model's whole content unchanged;
- caching is implicit (Gemini 3: at least 4,096 tokens), helped by the stable-first order (P20); hits are reported in
  usageMetadata.cachedContentTokenCount;
- thinkingLevel "low" for planner-type calls ("minimal" is unsupported on 3.8-flash and 3.1-pro);
- only finishReason STOP (or none) is a normal end; every other reason is an error (F2-R6).
Google now also promotes a newer Interactions API; generateContent remains documented and is what this adapter uses."""

from typing import Any

from ferb.core.types import Capabilities, ModelRequest
from ferb.runtimes.api import ProviderError, ToolCall, ToolOutcome, ToolSpec, Turn
from ferb.runtimes.usage import from_gemini

BASE = "https://generativelanguage.googleapis.com/v1beta/models"
DEFAULT_MODEL = "gemini-3.8-flash"


class GoogleProvider:
    name = "google"
    capabilities = Capabilities("direct", "prefix", False, "native", True, True, False, False, "api")

    def __init__(self, model: str = DEFAULT_MODEL) -> None:
        self.model = model

    def begin(self, request: ModelRequest, tools: list[ToolSpec], schema: dict[str, Any] | None, effort: str | None,
              cache_key: str) -> dict[str, Any]:
        body: dict[str, Any] = {"contents": [{"role": "user", "parts": [{"text": p} for p in request.parts]}]}
        if request.system:
            body["systemInstruction"] = {"parts": [{"text": request.system}]}
        if tools:
            body["tools"] = [{"functionDeclarations": [{"name": t.name, "description": t.description,
                                                        "parametersJsonSchema": t.schema} for t in tools]}]
        config: dict[str, Any] = {}
        if schema:
            config["responseMimeType"] = "application/json"
            config["responseJsonSchema"] = schema
        if effort:
            config["thinkingConfig"] = {"thinkingLevel": effort}
        if config:
            body["generationConfig"] = config
        return {"body": body}

    def http(self, state: dict[str, Any]) -> tuple[str, dict[str, str], dict[str, Any]]:
        return f"{BASE}/{self.model}:generateContent", {"content-type": "application/json"}, state["body"]

    def stream_http(self, state: dict[str, Any]) -> tuple[str, dict[str, str], dict[str, Any]]:
        return f"{BASE}/{self.model}:streamGenerateContent?alt=sse", {"content-type": "application/json"}, state['body']

    def stream_parser(self) -> "GoogleStream":
        return GoogleStream()

    def auth(self, key: str) -> dict[str, str]:
        return {"x-goog-api-key": key}

    def parse(self, state: dict[str, Any], data: dict[str, Any]) -> Turn:
        candidates = data.get("candidates") or []
        if not candidates:
            reason = (data.get("promptFeedback") or {}).get("blockReason", "no candidates")
            raise ProviderError(f"google: no answer ({reason})")
        first = candidates[0]
        finish = str(first.get("finishReason") or "")
        if finish == "MAX_TOKENS":
            raise ProviderError("google: the reply was cut off at the output limit")
        if finish not in ("", "STOP"):
            raise ProviderError(f"google: the model stopped ({finish})")
        content = first.get("content") or {"role": "model", "parts": []}
        state["body"]["contents"].append(content)        # thought signatures go back exactly as received
        parts = content.get("parts") or []
        calls = []
        for p in parts:
            fc = p.get("functionCall")
            if not fc:
                continue
            args = fc.get("args")
            ok = isinstance(args, dict) or args is None
            calls.append(ToolCall(str(fc.get("id") or fc["name"]), str(fc["name"]), (args or {}) if ok else {},
                                  "" if ok else "not a JSON object"))
        text = "".join(p.get("text", "") for p in parts if "text" in p and not p.get("thought"))
        return Turn(calls, None if calls else text, from_gemini(self.model, data.get("usageMetadata") or {}, billing="api"),
                    finish)

    def add_results(self, state: dict[str, Any], turn: Turn, results: list[ToolOutcome]) -> None:
        parts = []
        for r in results:
            response: dict[str, Any] = {"name": r.call.name, "response": {"error" if r.is_error else "result": r.text}}
            if r.call.id != r.call.name:
                response["id"] = r.call.id
            if r.image:
                response["parts"] = [{"inlineData": {"mimeType": r.image["mime"], "data": r.image["data"]}}]
            parts.append({"functionResponse": response})
        state["body"]["contents"].append({"role": "user", "parts": parts})


class GoogleStream:
    """Gemini streams complete candidate chunks; retain signatures and omit thought parts from deltas."""
    def __init__(self) -> None:
        self.parts: list[dict[str, Any]] = []
        self.candidate: dict[str, Any] = {}
        self.metadata: dict[str, Any] = {}

    def feed(self, event: dict[str, Any]) -> str:
        if event.get('error'):
            raise ProviderError('google: the response stream failed')
        for field in ('usageMetadata', 'promptFeedback'):
            if field in event:
                self.metadata[field] = event[field]
        candidates = event.get('candidates') or []
        if not candidates:
            return ''
        candidate = candidates[0]
        if candidate.get('index', 0) != 0:
            return ''
        self.candidate.update(candidate)
        parts = (candidate.get('content') or {}).get('parts') or []
        self.parts.extend(parts)
        return ''.join(str(part.get('text') or '') for part in parts if not part.get('thought'))

    def finish(self) -> dict[str, Any]:
        if not self.candidate:
            return self.metadata
        if not self.candidate.get('finishReason'):
            raise ProviderError('google: the response stream ended before completion')
        content = {**(self.candidate.get('content') or {}), 'parts': self.parts}
        return {**self.metadata, 'candidates': [{**self.candidate, 'content': content}]}
