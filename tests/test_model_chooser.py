"""The owner's model chooser (owner 2026-11-08: "cant we give option to users to change the model???"): a model per
connection from the provider's own list or one typed, checked before it is saved; a role's own model; Claude's
sign-in working out of the box; any typed model sized safely."""
import json
from typing import Any

import httpx
import pytest

from ferb.runtimes import models as models_
from ferb.runtimes.catalog import CATALOG, role_model
from tests.conftest import RunningDaemon


# decision: RUN-429
def test_provider_lists_are_read_as_each_provider_answers() -> None:
    assert models_.parse("anthropic", {"data": [{"id": "claude-opus-5-5"}, {"id": "claude-sonnet-5-5"}]}) == [
        "claude-opus-5-5", "claude-sonnet-5-5"]
    assert models_.parse("google", {"models": [
        {"name": "models/gemini-3.8-flash", "supportedGenerationMethods": ["generateContent"]},
        {"name": "models/text-embedding-9", "supportedGenerationMethods": ["embedContent"]}]}) == ["gemini-3.8-flash"]

    def answer(request: httpx.Request) -> httpx.Response:
        assert request.headers["x-api-key"] == "k" and "anthropic-version" in request.headers
        return httpx.Response(401, json={"error": {"type": "authentication_error", "message": "invalid x-api-key"}})
    import asyncio
    with pytest.raises(models_.ListFailed, match="HTTP 401: invalid x-api-key"):
        asyncio.run(models_.provider_models("anthropic", "k", httpx.MockTransport(answer)))


# decision: RUN-429, RUN-432, RUN-433
def test_claude_works_out_of_the_box_and_a_role_can_have_its_own_model(tmp_path: Any) -> None:
    from ferb.config import Config
    assert CATALOG["claude"].default_model == "claude-opus-5-5"
    cfg = Config(tmp_path)
    assert role_model(cfg, "claude", "worker") == "claude-opus-5-5"
    cfg.set("role_models", "worker", {"runtime": "claude", "model": "claude-sonnet-5-5"})
    assert role_model(cfg, "claude", "worker") == "claude-sonnet-5-5"
    assert role_model(cfg, "claude", "worker:claude") == "claude-sonnet-5-5"
    assert role_model(cfg, "claude", "orchestrator") == "claude-opus-5-5"
    assert role_model(cfg, "anthropic", "worker") == CATALOG["anthropic"].default_model   # chosen for another service


# decision: RUN-428
def test_a_typed_model_is_sized_with_the_smallest_limits_ferb_knows() -> None:
    from ferb.runtimes.anthropic_api import LIMITS, limits_for
    known, typed = limits_for("claude-opus-5-5"), limits_for("claude-opus-6-0")
    assert known.context_window == LIMITS["claude-opus-5-5"][0]
    assert typed.context_window == min(w for w, _ in LIMITS.values()) and "not documented" in typed.source


def _rpc(daemon: RunningDaemon, name: str, params: dict[str, Any]) -> Any:
    return daemon.run(getattr(daemon.daemon, name)(None, params))


# decision: RUN-429, RUN-431, DMN-580
def test_a_model_is_checked_before_it_is_saved(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    """In the provider's list for this key: saved. Not in it: not saved, the previous model keeps running. No list
    (the provider didn't answer one): one tiny call decides. A role's own model is saved apart and cleared by empty."""
    d = daemon.daemon

    async def key(provider: str) -> str:
        return "k"
    offered = ["claude-opus-5-5", "claude-sonnet-5-5"]

    async def listed(provider: str, k: str) -> list[str]:
        if offered is None:
            raise models_.ListFailed("HTTP 503: overloaded")
        return offered
    checked: list[tuple[str, str]] = []

    async def check(name: str, model: str = "") -> dict[str, Any]:
        checked.append((name, model))
        return {"ok": model != "claude-bad", "detail": "answered" if model != "claude-bad" else "model: claude-bad not found"}
    monkeypatch.setattr(d, "_api_key", key)
    monkeypatch.setattr(models_, "provider_models", listed)
    monkeypatch.setattr(d, "_check_runtime", check)

    listing = _rpc(daemon, "_runtimes_models", {"runtime": "anthropic"})
    assert listing["models"] == offered and listing["source"] == "provider"
    assert _rpc(daemon, "_runtimes_model", {"runtime": "anthropic", "model": "claude-opus-5-5"})["ok"]
    assert d.config.get("models", "anthropic") == "claude-opus-5-5" and checked == []
    refused = _rpc(daemon, "_runtimes_model", {"runtime": "anthropic", "model": "claude-opus-9"})
    assert not refused["ok"] and "isn't in the models" in refused["detail"]
    assert d.config.get("models", "anthropic") == "claude-opus-5-5"          # the working one stays
    offered = None                                                           # no list: one tiny call decides
    assert not _rpc(daemon, "_runtimes_model", {"runtime": "anthropic", "model": "claude-bad"})["ok"]
    assert _rpc(daemon, "_runtimes_model", {"runtime": "anthropic", "model": "claude-new-1"})["ok"]
    assert checked == [("anthropic", "claude-bad"), ("anthropic", "claude-new-1")]
    assert d.config.get("models", "anthropic") == "claude-new-1"
    assert _rpc(daemon, "_runtimes_model", {"runtime": "claude", "model": "claude-sonnet-5-5", "role": "worker"})["ok"]
    assert d.config.get("role_models", "worker") == {"runtime": "claude", "model": "claude-sonnet-5-5"}
    assert _rpc(daemon, "_runtimes_model", {"runtime": "claude", "model": "", "role": "worker"})["ok"]
    assert d.config.get("role_models", "worker") is None
    kinds = [e.kind for e in d.journal.recent(50)] if hasattr(d.journal, "recent") else []
    assert not kinds or "runtimes.model" in kinds
    assert json.dumps(listing).count("\"k\"") == 0                         # the key never comes back
