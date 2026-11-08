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
