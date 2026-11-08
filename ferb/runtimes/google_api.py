"""Owns the Gemini API (generateContent) translation for FERB's tool loop (D32). Must not choose targets or add tools.

Verified 2026-10-07 (https://ai.google.dev/api/generate-content, /gemini-api/docs/generate-content/function-calling,
/structured-output, /thinking, /caching, /thought-signatures):
- function declarations use parametersJsonSchema (JSON Schema; mutually exclusive with the OpenAPI-subset `parameters`);
- typed answers: responseMimeType application/json + responseJsonSchema; together with tools this works on Gemini 3 only
  (gemini-3.8-flash, gemini-3.1-pro-preview), so the default model (ferb/runtimes/catalog.py) is a Gemini 3 model;
- function-call parts carry a thoughtSignature that must go back exactly: FERB appends the model's whole content unchanged;
- caching is implicit (Gemini 3: at least 4,096 tokens), helped by the stable-first order (P20); hits are reported in
  usageMetadata.cachedContentTokenCount;
- thinkingLevel "low" for planner-type calls ("minimal" is unsupported on 3.8-flash and 3.1-pro);
- only finishReason STOP (or none) is a normal end; every other reason is an error (F2-R6).
Google now also promotes a newer Interactions API; generateContent remains documented and is what this adapter uses.

Typed-answer limits (RUN-408, RUN-427): the structured-output page documents the JSON Schema keywords it takes
(schema_shape.GEMINI_KEYWORDS, checked 2026-10-30); FERB's repository records no size limit for it. FERB's runtime
notes expected a too-large schema's rejection as "too many states" (LIVE-411; the wording not verified). A 400 on a
request carrying responseJsonSchema goes through the shared fallback in api.py (`without_format` below)."""

from typing import Any

from ferb.core.types import ModelLimits, ModelRequest
from ferb.runtimes.api import ProviderError, ToolCall, ToolOutcome, ToolSpec, Turn
from ferb.runtimes.schema_shape import Shape, for_gemini
from ferb.runtimes.usage import from_gemini

BASE = "https://generativelanguage.googleapis.com/v1beta/models"
# A function call Gemini sent without an id gets FERB's own (this prefix, the call's place in the turn, its name), so
# parallel calls to one tool stay apart; only Gemini's own ids are sent back with the results
LOCAL_ID = "ferb-local:"
# L28: each model's documented input and output token limits, checked 2026-10-14 on
# https://ai.google.dev/gemini-api/docs/models/<model code>. Google documents the input limit itself, with the reply
# counted apart, so the whole input limit is FERB's room. Facts, not FERB choices.
LIMITS: dict[str, tuple[int, int]] = {
    "gemini-3.8-flash": (1_048_576, 65_536),
    "gemini-3.1-pro-preview": (1_048_576, 65_536),
}
LIMITS_SOURCE = "https://ai.google.dev/gemini-api/docs/models (checked 2026-10-14)"


def limits_for(model: str) -> ModelLimits:
    if model not in LIMITS:                         # RUN-428: a model the owner typed: the smallest limits FERB knows
        window, output = min(w for w, _ in LIMITS.values()), min(o for _, o in LIMITS.values())
        return ModelLimits(model, window, output, f"not documented to FERB for {model}: the smallest limits it knows "
                                                  f"for Google models ({LIMITS_SOURCE})", window_includes_output=False)
    window, output = LIMITS[model]
    return ModelLimits(model, window, output, LIMITS_SOURCE, window_includes_output=False)


class GoogleProvider:
    name = "google"

    def __init__(self, model: str | None = None) -> None:
        from ferb.runtimes.catalog import CATALOG
        self.model = model or CATALOG["google"].default_model

    def shape(self, schema: dict[str, Any]) -> Shape:
        """Gemini's documented schema form (schema_shape.for_gemini): the same values, nothing to map back."""
        return for_gemini(schema)

    @property
    def limits(self) -> ModelLimits:
        return limits_for(self.model)

    def begin(self, request: ModelRequest, tools: list[ToolSpec], schema: dict[str, Any] | None, effort: str | None,
              cache_key: str) -> dict[str, Any]:
        parts: list[dict[str, Any]] = [{"text": p} for p in request.parts]
        parts += [{"inlineData": {"mimeType": i.mime, "data": i.base64()}} for i in request.images]   # L12
        body: dict[str, Any] = {"contents": [{"role": "user", "parts": parts}]}
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
        calls: list[ToolCall] = []
        for p in parts:
            fc = p.get("functionCall")
            if not fc:
                continue
            args = fc.get("args")
            ok = isinstance(args, dict) or args is None
            ident = str(fc.get("id") or f"{LOCAL_ID}{len(calls)}:{fc['name']}")
            calls.append(ToolCall(ident, str(fc["name"]), (args or {}) if ok else {}, "" if ok else "not a JSON object"))
        text = "".join(p.get("text", "") for p in parts if "text" in p and not p.get("thought"))
        return Turn(calls, None if calls else text, from_gemini(self.model, data.get("usageMetadata") or {}, billing="api"),
                    finish)

    def add_message(self, state: dict[str, Any], message: str) -> None:
        state["body"]["contents"].append({"role": "user", "parts": [{"text": message}]})

    def without_format(self, state: dict[str, Any], statement: str) -> None:
        """RUN-427: no responseMimeType or responseJsonSchema; the answer form goes at the end of the system instruction
        (implicit caching keeps a stable prefix)."""
        body = state["body"]
        config = body.get("generationConfig") or {}
        config.pop("responseMimeType", None)
        config.pop("responseJsonSchema", None)
        if not config:
            body.pop("generationConfig", None)
        body.setdefault("systemInstruction", {"parts": []})["parts"].append({"text": statement})

    def window_exceeded(self, status: int, error: dict[str, Any]) -> bool:
        """Never marked: Google documents no error code or message for an input over a model's token limit (its
        troubleshooting page names none), and its own client (Gemini CLI 0.55.1) counts tokens before sending rather
        than reading the rejection, so FERB has no shape to cite (fix wave 6b round 2; an owner-acceptance residual)."""
        return False

    def image_refused(self, status: int, error: dict[str, Any]) -> bool:
        """Never marked: Google's API answers a bad image with its generic 400 INVALID_ARGUMENT and documents no code
        for it, so FERB has no shape to cite and the planner's turn is not rerun without the screenshot (ORCH-524)."""
        return False

    def add_results(self, state: dict[str, Any], turn: Turn, results: list[ToolOutcome]) -> None:
        parts = []
        for r in results:
            response: dict[str, Any] = {"name": r.call.name, "response": {"error" if r.is_error else "result": r.text}}
            if not r.call.id.startswith(LOCAL_ID):         # Gemini's own id; results otherwise pair by order
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
