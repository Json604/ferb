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


def test_a_key_stays_redacted_in_journal_and_spans_after_a_config_reload(daemon: RunningDaemon, monkeypatch) -> None:
    import ferb.runtimes.keys as keys
    secret = "plain-words-key-9f8e7d6c"                      # matches no regex pattern

    async def fake_key(provider):
        return secret
    monkeypatch.setattr(keys, "api_key", fake_key)
    d = daemon.daemon
    d._load_config()                                          # what a config or voice reload runs
    assert daemon.run(d._api_key("google")) == secret

    async def write_and_read() -> list[dict]:
        d.journal.append("note", {"text": f"echo {secret}"})
        span = d.spans.start("probe", input={"text": f"echo {secret}"})
        d.spans.finish(span, output={"text": secret}, error=f"bad {secret}")
        return [dict(r) for t in ("journal", "spans") for r in d.db.conn.execute(f"SELECT * FROM {t}")]
    rows = daemon.run(write_and_read())
    assert secret not in json.dumps(rows, default=str)


def test_google_api_keys_are_redacted_by_pattern() -> None:
    from ferb.sanitize.redact import Redactor
    key = "AIza" + "Sy" + "a" * 33
    assert key not in Redactor().text(f"key={key} end")


def test_runtimes_check_reports_a_non_json_answer(daemon: RunningDaemon, monkeypatch) -> None:
    from types import SimpleNamespace as NS
    d = daemon.daemon

    def fake(text):
        class RT:
            async def start(self, *a):
                return "run"

            async def events(self, run):
                yield NS(kind="final", data={"text": text})

            async def cancel(self, run):
                pass
        return RT()
    for text in ("not json", "[1, 2]", "true"):
        monkeypatch.setattr(d, "_build_runtime", lambda *a, _t=text, **k: fake(_t))
        out = daemon.run(d._runtimes_check(None, {"runtime": "openai"}))
        assert out == {"ok": False, "detail": 'the answer was not {"ok": ...}'}
    monkeypatch.setattr(d, "_build_runtime", lambda *a, **k: fake('{"ok": true}'))
    assert daemon.run(d._runtimes_check(None, {"runtime": "openai"}))["ok"] is True


def test_offline_daemons_never_ask_the_real_keychain(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    assert [n for n in d.registry.runtimes if d._runtime_available(n)] == ["codex", "claude"]


def test_codex_without_ferbs_own_login_is_refused_with_the_fix(daemon: RunningDaemon) -> None:
    import pytest
    from ferb.core.errors import Unsupported
    d = daemon.daemon
    assert not (d.home / "codex-home" / "auth.json").exists()
    d.config.set("runtimes", "orchestrator", "codex")
    with pytest.raises(Unsupported, match="ferb setup --codex-login"):
        d._make_runtime("orchestrator", mode="supervisor", tool_names=[], schema={"type": "object"})


def test_a_usage_events_cost_lands_in_the_span_cost_column(daemon: RunningDaemon) -> None:
    from tests.conftest import chat, done, install, outcome, sup, task
    install(daemon, [sup("task", "Working", task())], {"worker": lambda _: done("ok")})
    outcome(chat(daemon, "Create the requested file"))
    async def rows(sql):
        return [dict(r) for r in daemon.daemon.db.conn.execute(sql).fetchall()]
    usage = daemon.run(rows("SELECT cost_usd FROM spans WHERE kind = 'model.usage'"))
    assert usage and any(r["cost_usd"] == 0.001 for r in usage)

    async def distill():
        daemon.daemon._record_distill_usage("fake", {"input_tokens": 1, "cost_usd": 0.5})
        return await rows("SELECT cost_usd FROM spans WHERE input_json LIKE '%distill%'")
    assert daemon.run(distill())[0]["cost_usd"] == 0.5
