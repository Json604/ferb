"""The direct-API runtimes join FERB: chosen per role, keys from the Keychain and redacted, offered only when usable."""
import json

from ferb.runtimes.api import ApiRuntime
from ferb.runtimes.catalog import CATALOG
from tests.conftest import RunningDaemon


def test_catalog_lists_all_five_runtimes() -> None:
    assert set(CATALOG) == {"codex", "claude", "anthropic", "openai", "google"}
    assert CATALOG["openai"].capabilities("gpt-5.5").cache == "prefix"
    assert CATALOG["openai"].capabilities("gpt-6.1-sol").cache == "markers"


def test_catalog_defaults_match_the_provider_modules() -> None:
    from ferb.runtimes import anthropic_api, google_api, openai_api
    assert CATALOG["anthropic"].default_model == anthropic_api.DEFAULT_MODEL
    assert CATALOG["openai"].default_model == openai_api.DEFAULT_MODEL
    assert CATALOG["google"].default_model == google_api.DEFAULT_MODEL


def test_api_runtime_is_built_for_its_role(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    d.config.set("runtimes", "orchestrator", "openai")
    d.config.set("models", "openai", "gpt-6-sol")
    rt = d._make_runtime("orchestrator", mode="supervisor", tool_names=[], schema={"type": "object"})
    assert isinstance(rt, ApiRuntime) and rt.provider.model == "gpt-6-sol" and rt.role == "orchestrator"


def test_worker_types_list_only_available_runtimes(daemon: RunningDaemon, monkeypatch) -> None:
    d = daemon.daemon
    monkeypatch.setattr(d, "_runtime_available", lambda name: name in ("codex", "anthropic"))
    assert d.supervisor.worker_types() == ["default", "codex", "anthropic"]


def test_a_key_read_for_a_call_is_redacted_everywhere(daemon: RunningDaemon, monkeypatch) -> None:
    import ferb.runtimes.keys as keys

    async def fake_key(provider):
        return "sk-live-abc123"
    monkeypatch.setattr(keys, "api_key", fake_key)
    d = daemon.daemon
    assert daemon.run(d._api_key("openai")) == "sk-live-abc123"
    assert "sk-live-abc123" not in json.dumps(d.redact({"text": "echo sk-live-abc123"}))
