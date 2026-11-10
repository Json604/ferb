"""Owns which models each connection can offer the owner (RUN-429, owner 2026-11-08: "cant we give option to users to
change the model???"): an API key's live list from its provider's models endpoint, read with the stored key; for FERB's
Codex and Claude sign-ins, the models their pinned runtime knows. Must not choose a model, and never logs or returns a
key.

Endpoints (each provider's documented list call, read with the key as its runtime sends it):
- Anthropic: GET https://api.anthropic.com/v1/models (x-api-key, anthropic-version) -> data[].id
- OpenAI: GET https://api.openai.com/v1/models (Authorization: Bearer) -> data[].id
- Google: GET https://generativelanguage.googleapis.com/v1beta/models (x-goog-api-key) -> models[].name without
  "models/", those whose supportedGenerationMethods hold generateContent (the call FERB's runtime makes)."""

from pathlib import Path
from typing import Any

import httpx

TIMEOUT_S = 20.0
URLS = {"anthropic": "https://api.anthropic.com/v1/models?limit=1000",
        "openai": "https://api.openai.com/v1/models",
        "google": "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000"}


class ListFailed(Exception):
    """The provider didn't give its list: its own words (or the transport's)."""


def _headers(provider: str, key: str) -> dict[str, str]:
    if provider == "anthropic":
        return {"x-api-key": key, "anthropic-version": "2023-06-01"}
    if provider == "openai":
        return {"Authorization": f"Bearer {key}"}
    return {"x-goog-api-key": key}


def parse(provider: str, data: Any) -> list[str]:
    """The model ids in a provider's list answer, sorted."""
    if provider == "google":
        rows = data.get("models") if isinstance(data, dict) else None
        ids = [str(m["name"]).removeprefix("models/") for m in rows or [] if isinstance(m, dict) and m.get("name")
               and "generateContent" in (m.get("supportedGenerationMethods") or [])]
    else:
        rows = data.get("data") if isinstance(data, dict) else None
        ids = [str(m["id"]) for m in rows or [] if isinstance(m, dict) and m.get("id")]
    return sorted(set(ids))


async def provider_models(provider: str, key: str, transport: httpx.AsyncBaseTransport | None = None) -> list[str]:
    """The models the provider offers this key; ListFailed with the provider's words when it doesn't answer one."""
    try:
        async with httpx.AsyncClient(timeout=TIMEOUT_S, transport=transport) as client:
            resp = await client.get(URLS[provider], headers=_headers(provider, key))
    except httpx.HTTPError as e:
        raise ListFailed(f"{type(e).__name__}: {e}") from None
    if resp.status_code != 200:
        try:
            body = resp.json()
            error = body.get("error") if isinstance(body, dict) else None
            words = str(error.get("message") if isinstance(error, dict) else error or resp.text)
        except ValueError:
            words = resp.text
        raise ListFailed(f"HTTP {resp.status_code}: {words[:300]}")
    try:
        return parse(provider, resp.json())
    except ValueError:
        raise ListFailed("the answer wasn't a model list") from None


def known_models(name: str, home: Path) -> list[str]:
    """The models FERB's own sign-ins can run, from files only: Codex's catalog for FERB's login (else the one shipped
    with the pinned Codex); for Claude, the models FERB documents (anthropic_api.LIMITS)."""
    if name == "codex":
        from ferb.runtimes.codex_exec import _catalog, _models
        found = _catalog(home)
        rows = _models(found[0]) if found is not None else None
        return sorted({str(m["slug"]) for m in rows or [] if isinstance(m, dict) and m.get("slug")})
    if name == "claude":
        from ferb.runtimes.anthropic_api import LIMITS
        return sorted(LIMITS)
    return []


# ---------- effort levels per model (owner 2026-11-10: "show the effort level along with model selection") ----------
# Each list is what the provider documents for that model, read 2026-11-10; a model none names offers only Default and a
# typed level (checked by one tiny call before it is saved). Matched by the longest documented id the model starts with.
#
# Anthropic Messages API, output_config.effort (https://platform.claude.com/docs/en/build-with-claude/effort, "Effort
# levels"): low, medium, high on every model with effort; xhigh and max only on the models listed there; the default is
# medium on Claude Opus 5.5 and Claude Haiku 5.5, high on the others. Claude Haiku 4.5 takes no effort (NO_EFFORT).
# Claude Code passes its --effort to the same API (claude-agent-sdk 0.2.174 EffortLevel: low, medium, high, xhigh, max).
_ALL5 = ("low", "medium", "high", "xhigh", "max")
_NO_XHIGH = ("low", "medium", "high", "max")
ANTHROPIC_LEVELS: dict[str, tuple[tuple[str, ...], str]] = {
    **{m: (_ALL5, "high") for m in ("claude-fable-5-2", "claude-mythos-5-2", "claude-fable-5-1", "claude-mythos-5-1",
                                    "claude-fable-5", "claude-mythos-5", "claude-opus-5", "claude-opus-4-8",
                                    "claude-opus-4-7", "claude-sonnet-5-5", "claude-sonnet-5")},
    "claude-opus-5-5": (_ALL5, "medium"), "claude-haiku-5-5": (_ALL5, "medium"),
    **{m: (_NO_XHIGH, "high") for m in ("claude-mythos-preview", "claude-opus-4-6", "claude-sonnet-4-6")},
    "claude-opus-4-5": (("low", "medium", "high"), "high"),
}
# Gemini API thinking level (https://ai.google.dev/gemini-api/docs/thinking, which now redirects to
# https://aistudio.google.com/docs/thinking: its table of models, read 2026-11-10). FERB sends the same level names as
# generationConfig.thinkingConfig.thinkingLevel (google_api.py).
GOOGLE_LEVELS: dict[str, tuple[tuple[str, ...], str]] = {
    "gemini-4-argon": (("low", "medium", "high"), "medium"),
    "gemini-3.8-flash": (("low", "medium", "high"), "medium"),
    "gemini-3.6-flash": (("minimal", "low", "medium", "high"), "medium"),
    "gemini-3.5-flash-lite": (("minimal", "low", "medium", "high"), "minimal"),
    "gemini-3.1-pro-preview": (("low", "medium", "high"), "high"),
    "gemini-3.1-flash-lite-image": (("minimal", "high"), "minimal"),
    "gemini-3-flash-preview": (("minimal", "low", "medium", "high"), "high"),
    "gemini-3-pro-preview": (("low", "high"), "high"),
}
# OpenAI Responses API reasoning.effort (https://developers.openai.com/api/docs/guides/reasoning, read 2026-11-10): the
# values are model-dependent and the guide lists no per-model set, so no list is offered (Default and a typed level);
# only the defaults it names are shown.
OPENAI_DEFAULTS = {"gpt-5.5": "medium", "gpt-5.6": "medium", "gpt-6.1-sol": "medium", "gpt-6-sol": "medium",
                   "gpt-6-luna": "medium"}
# Codex's own catalog lists levels per model; "ultra" hands work to sub-agents, which FERB keeps off (RUN-405)
CODEX_HIDDEN = {"ultra"}


def _documented(table: dict[str, Any], model: str) -> Any:
    keys = [k for k in table if model == k or model.startswith(k + "-")]
    return table[max(keys, key=len)] if keys else None


def effort_levels(name: str, home: Path, models: list[str]) -> tuple[dict[str, list[str]], dict[str, str]]:
    """Each model's effort levels the owner can pick (a model with none listed: only Default and a typed level), and
    each model's default level where its provider documents one. Files and tables only: no model call. Claude Code's
    own default isn't documented per model, so none is given for it."""
    levels: dict[str, list[str]] = {}
    defaults: dict[str, str] = {}
    if name == "codex":
        from ferb.runtimes.codex_exec import _catalog, _models
        found = _catalog(home)
        for row in (_models(found[0]) if found is not None else None) or []:
            if not isinstance(row, dict) or not row.get("slug"):
                continue
            slug = str(row["slug"])
            levels[slug] = [str(r["effort"]) for r in row.get("supported_reasoning_levels") or []
                            if isinstance(r, dict) and r.get("effort") and str(r["effort"]) not in CODEX_HIDDEN]
            if row.get("default_reasoning_level"):
                defaults[slug] = str(row["default_reasoning_level"])
        return levels, defaults
    for model in models:
        if name in ("claude", "anthropic"):
            documented = _documented(ANTHROPIC_LEVELS, model)
            if documented is not None:
                levels[model] = list(documented[0])
                if name == "anthropic":
                    defaults[model] = documented[1]
        elif name == "google":
            documented = _documented(GOOGLE_LEVELS, model)
            if documented is not None:
                levels[model], defaults[model] = list(documented[0]), documented[1]
        elif name == "openai" and (default := _documented(OPENAI_DEFAULTS, model)) is not None:
            defaults[model] = default
    return levels, defaults
