"""The direct-API runtimes join FERB: chosen per role, keys from the Keychain and redacted, offered only when usable."""
import json

import pytest

from ferb.runtimes.catalog import CATALOG
from tests.conftest import RunningDaemon


# decision: RUN-400, RUN-404
def test_catalog_lists_all_five_runtimes() -> None:
    assert set(CATALOG) == {"codex", "claude", "anthropic", "openai", "google"}
    from ferb.runtimes.openai_api import OpenAIProvider
    assert OpenAIProvider("gpt-5.5").explicit is False and OpenAIProvider("gpt-6.1-sol").explicit is True


# decision: RUN-411
def test_the_pin_report_compares_pins_with_the_registries_and_only_reads() -> None:
    """`ferb runtimes pins` (developer toolbox): the registry replies come through `get`; nothing is written."""
    from datetime import datetime, timezone
    from ferb.cli import runtimes as runtimes_cli
    from ferb.runtimes.claude_sdk import SDK_VERSION
    from ferb.runtimes.codex_exec import CODEX_VERSION
    replies = {runtimes_cli.NPM_TAGS: {"latest": CODEX_VERSION, "alpha": "9.0.0-alpha.1"},
               runtimes_cli.NPM_SEARCH: {"objects": [{"package": {"name": "@openai/codex", "version": CODEX_VERSION,
                                                                  "date": "2026-10-29T19:56:19Z"}}]},
               runtimes_cli.PYPI: {"info": {"version": "9.9.9"}, "releases": {
                   SDK_VERSION: [{}], "9.9.8": [{}], "9.9.9": [{"upload_time_iso_8601": "2026-10-31T01:45:36Z"}]}}}
    codex, claude = runtimes_cli.pin_report(replies.__getitem__, datetime(2026, 11, 1, tzinfo=timezone.utc))
    assert f"pinned {CODEX_VERSION}" in codex and "up to date" in codex and "behind" not in codex
    assert f"pinned {SDK_VERSION}, newest 9.9.9" in claude and "behind" in claude and "2 newer releases" in claude


# decision: RUN-404
def test_provider_defaults_come_from_the_table() -> None:
    from ferb.runtimes.anthropic_api import AnthropicProvider
    from ferb.runtimes.google_api import GoogleProvider
    from ferb.runtimes.openai_api import OpenAIProvider
    assert AnthropicProvider().model == CATALOG["anthropic"].default_model
    assert OpenAIProvider().model == CATALOG["openai"].default_model
    assert GoogleProvider().model == CATALOG["google"].default_model


# decision: RUN-403, RUN-404
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


# decision: RUN-1, RUN-404
def test_codex_without_ferbs_own_login_is_refused_with_the_fix(daemon: RunningDaemon) -> None:
    from ferb.core.errors import Unsupported
    d = daemon.daemon
    assert not (d.home / "codex-home" / "auth.json").exists()
    d.config.set("runtimes", "orchestrator", "codex")
    with pytest.raises(Unsupported, match="sign in to ChatGPT in Settings → AI"):
        d._make_runtime("orchestrator", mode="supervisor", tool_names=[], schema={"type": "object"})


def test_the_codex_runtime_redacts_its_errors_with_the_daemons_redactor(daemon: RunningDaemon, monkeypatch) -> None:
    """Codex's error text is redacted with the daemon's own redactor, which knows the values the owner stored (rule 14)."""
    d = daemon.daemon
    (d.home / 'codex-home').mkdir(exist_ok=True)
    (d.home / 'codex-home' / 'auth.json').write_text('{}')
    monkeypatch.setattr('ferb.runtimes.codex_exec.codex_binary', lambda *_: d.home / 'fake-codex')
    rt = d._make_runtime('orchestrator', mode='supervisor', tool_names=[], schema={'type': 'object'})
    assert rt.redact is d.redact


# decision: RUN-424, RUN-404, ORCH-315
def test_availability_is_read_once_and_refreshed_only_by_events_that_change_it(daemon: RunningDaemon,
                                                                               monkeypatch) -> None:
    """Owner 2026-11-04: no sign-in check on every message. Each runtime's availability is read once and kept; a
    sign-in failure from a call itself still says so (ORCH-315) and makes that runtime unavailable; a runtime change
    or the app's AI status read refreshes it. No real Keychain or login is read."""
    from ferb.core.types import RuntimeEvent
    from tests.conftest import FakeRuntime, chat, install
    d = daemon.daemon
    del d._runtime_available                        # the fixture's stand-in: back to the daemon's own
    signed = {name: name in ("codex", "anthropic") for name in CATALOG}
    reads: list[str] = []

    def available(entry, home, model=None):
        reads.append(entry.name)
        return (signed[entry.name], "" if signed[entry.name] else "sign in")
    monkeypatch.setattr(type(CATALOG["codex"]), "available", available)
    monkeypatch.setattr(type(CATALOG["codex"]), "signed_in", lambda entry, home: (signed[entry.name], ""))
    d._available_cache.clear()
    assert d.supervisor.worker_types() == ["default", "codex", "anthropic"]
    assert d.supervisor.worker_types() == ["default", "codex", "anthropic"]
    assert sorted(reads) == sorted(CATALOG)         # one read per runtime, not one per turn

    class Codex(FakeRuntime):
        name = "codex"
    script = install(daemon, [RuntimeEvent("error", {"message": "Codex runtime reported an error: unauthorized",
                                                    "account": "signed_out"})])
    d.supervisor.make_runtime = lambda role, *, mode, tool_names, schema, effort=None: Codex(script, mode, tool_names)
    reads.clear()
    [reply] = [e for e in chat(daemon, "what's the weather like?") if e["type"] == "reply"]
    assert reply["text"].startswith("Codex isn't signed in for FERB") and reply["runtime_switch"]["runtime"] == "codex"
    assert reply["runtime_switch"]["choices"] == ["anthropic"]
    assert d.supervisor.worker_types() == ["default", "anthropic"] and reads == []   # the call's own word, no probe

    daemon.run(d._runtimes_status(None, {}))       # the app reads its AI status after a sign-in: read afresh
    assert d.supervisor.worker_types() == ["default", "codex", "anthropic"]
    signed["openai"] = True
    daemon.run(d._runtimes_use(None, {"runtime": "openai", "roles": ["worker"]}))
    assert "openai" in d.supervisor.worker_types()   # a runtime change reads that runtime again
