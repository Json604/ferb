"""The direct-API runtimes join FERB: chosen per role, keys from the Keychain and redacted, offered only when usable."""
import asyncio
import json

import pytest

from ferb.core.types import ModelLimits
from ferb.runtimes.api import ApiRuntime
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


def test_api_runtime_is_built_for_its_role(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    d.config.set("runtimes", "orchestrator", "openai")
    d.config.set("models", "openai", "gpt-6-sol")
    rt = d._make_runtime("orchestrator", mode="supervisor", tool_names=[], schema={"type": "object"})
    assert isinstance(rt, ApiRuntime) and rt.provider.model == "gpt-6-sol" and rt.role == "orchestrator"


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


def test_runtimes_check_reports_a_non_json_answer(daemon: RunningDaemon, monkeypatch) -> None:
    from types import SimpleNamespace as NS
    d = daemon.daemon

    def fake(text):
        class RT:
            name = "openai"
            limits = ModelLimits("fake-model", 1_000_000, 100_000, "test double")

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


def test_runtimes_check_fails_for_a_model_without_documented_limits(daemon: RunningDaemon, monkeypatch) -> None:
    """A Claude with no model (or any model FERB knows no window for) fails the check before any call (L28), instead
    of passing the check and failing every real call."""
    from ferb.runtimes.claude_login import mark
    d = daemon.daemon
    mark(d.home)
    started = []
    monkeypatch.setattr("ferb.runtimes.claude_sdk.ClaudeRuntime.start", lambda *a, **k: started.append(a))
    out = daemon.run(d._runtimes_check(None, {"runtime": "claude"}))
    assert out["ok"] is False and "no model is chosen" in out["detail"] and not started
    d.config.set("models", "claude", "claude-sonnet-9")
    out = daemon.run(d._runtimes_check(None, {"runtime": "claude"}))
    assert out["ok"] is False and "context window" in out["detail"] and not started


def test_offline_daemons_never_ask_the_real_keychain(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    assert d._worker_types() == ["default", "codex", "claude"]


# decision: RUN-1, RUN-404
def test_codex_without_ferbs_own_login_is_refused_with_the_fix(daemon: RunningDaemon) -> None:
    import pytest
    from ferb.core.errors import Unsupported
    d = daemon.daemon
    assert not (d.home / "codex-home" / "auth.json").exists()
    d.config.set("runtimes", "orchestrator", "codex")
    with pytest.raises(Unsupported, match="sign in to ChatGPT in Settings → AI"):
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
        daemon.daemon._record_learning_usage("distill")("fake", {"input_tokens": 1, "cost_usd": 0.5})
        return await rows("SELECT cost_usd FROM spans WHERE input_json LIKE '%distill%'")
    assert daemon.run(distill())[0]["cost_usd"] == 0.5


def test_the_codex_runtime_redacts_its_errors_with_the_daemons_redactor(daemon: RunningDaemon, monkeypatch) -> None:
    """Codex's error text is redacted with the daemon's own redactor, which knows the values the owner stored (rule 14)."""
    d = daemon.daemon
    (d.home / 'codex-home').mkdir(exist_ok=True)
    (d.home / 'codex-home' / 'auth.json').write_text('{}')
    monkeypatch.setattr('ferb.runtimes.codex_exec.codex_binary', lambda *_: d.home / 'fake-codex')
    rt = d._make_runtime('orchestrator', mode='supervisor', tool_names=[], schema={'type': 'object'})
    assert rt.redact is d.redact


def test_a_retried_runtime_error_is_recorded_and_the_task_goes_on(daemon: RunningDaemon, monkeypatch) -> None:
    """An error the runtime retries itself (Codex willRetry) is a `model.retry` span under the run, with its message;
    the worker's run continues to its answer (L28, D32)."""
    from ferb.core.types import RuntimeEvent
    from tests.conftest import FakeRuntime, chat, done, install, outcome, sup, task
    original = FakeRuntime.events

    async def events(self, run):
        if run.state["key"] == "worker":
            yield RuntimeEvent("retrying", {"message": "Codex is retrying after an error: stream disconnected"})
        async for event in original(self, run):
            yield event
    monkeypatch.setattr(FakeRuntime, "events", events)
    install(daemon, [sup("task", "Working", task())], {"worker": lambda _: done("ok")})
    result = outcome(chat(daemon, "Create the requested file"))
    assert result["status"] != "failed"

    async def retries():
        return [(r[0], r[1]) for r in daemon.daemon.db.conn.execute(
            "SELECT s.error, p.kind FROM spans s JOIN spans p ON p.id = s.parent_id WHERE s.kind = 'model.retry'")]
    assert daemon.run(retries()) == [("Codex is retrying after an error: stream disconnected", "worker")]


RECONNECTING = "Codex is retrying after an error: Reconnecting... 3/∞ (waiting for network)"


def _retrying_worker(daemon: RunningDaemon, monkeypatch, then_progress: bool) -> dict:
    """A worker run whose runtime only retries (Codex 0.157.1 retries a lost connection without end: its default-on
    unbounded_connection_retries), or retries and then makes progress. Real seconds, a 0.3 s stall for test speed."""
    import threading
    import time
    from ferb.core.types import RuntimeEvent
    from tests.conftest import FakeRuntime, chat, done, install, sup, task
    s = daemon.daemon.supervisor
    s.activity.now = time.monotonic
    s.stall_s, s.heartbeat_poll_s = 0.3, 0.05
    original = FakeRuntime.events

    async def events(self, run):
        if run.state["key"] != "worker":
            async for event in original(self, run):
                yield event
            return
        if then_progress:
            yield RuntimeEvent("retrying", {"message": RECONNECTING})
            for _ in range(3):
                await asyncio.sleep(0.1)
                yield RuntimeEvent("message", {"text": "reading the notes"})
            async for event in original(self, run):
                yield event
            return
        while not run.state["cancel"]:
            yield RuntimeEvent("retrying", {"message": RECONNECTING})
            await asyncio.sleep(0.05)
    monkeypatch.setattr(FakeRuntime, "events", events)
    install(daemon, [sup("task", "Working", task("tidy the notes"))], {"worker": lambda _: done("ok")})
    turn: dict = {}
    chatting = threading.Thread(target=lambda: turn.update(events=chat(daemon, "tidy the notes")), daemon=True)
    chatting.start()
    chatting.join(8)                                   # chat streams have no socket timeout: bound the wait here
    return turn


def test_a_worker_that_only_retries_stalls_and_says_why(daemon: RunningDaemon, monkeypatch) -> None:
    """Retries are not a sign of life (L28, D32): during an outage Codex retries without end, and FERB's stall
    watchdog (STALL_S) bounds the run, reporting the last retry as the reason."""
    from tests.conftest import outcome
    turn = _retrying_worker(daemon, monkeypatch, then_progress=False)
    assert "events" in turn, "a worker that only retried was never stopped"
    out = outcome(turn["events"])
    summary = out["subtasks"]["worker"]["summary"]
    assert out["status"] == "failed" and "Reconnecting... 3/∞ (waiting for network)" in summary, out
    assert ".." not in summary.replace("Reconnecting...", "")


def test_a_worker_that_retries_and_then_makes_progress_continues(daemon: RunningDaemon, monkeypatch) -> None:
    from tests.conftest import outcome
    turn = _retrying_worker(daemon, monkeypatch, then_progress=True)
    assert "events" in turn
    out = outcome(turn["events"])
    assert out["status"] != "failed" and "Reconnecting" not in json.dumps(out), out


def test_a_worker_that_only_retries_while_the_mac_is_offline_says_so(daemon: RunningDaemon, monkeypatch) -> None:
    import ferb.sense.network as network
    from tests.conftest import outcome
    monkeypatch.setattr(network, "has_route", lambda: False)
    out = outcome(_retrying_worker(daemon, monkeypatch, then_progress=False)["events"])
    assert "seems to be offline" in out["subtasks"]["worker"]["summary"], out


@pytest.mark.parametrize("offline", [False, True])
def test_a_failed_next_attempt_choice_reads_as_one_sentence(daemon: RunningDaemon, monkeypatch, offline) -> None:
    """The dispatch failure's sentence ended in a doubled period (the service's or the offline sentence's own period
    before FERB's). Step 8b: the checker chooses next; when its run fails after a failed attempt, the attempt's own
    reason is said, as one sentence."""
    import ferb.sense.network as network
    from ferb.orchestrator.worker import ServiceError
    from tests.conftest import chat, install, outcome, sup, task
    monkeypatch.setattr(network, "has_route", lambda: not offline)

    async def failing(*_a, **_k):
        raise ServiceError("You've hit your usage limit.", "codex", network=True)
    install(daemon, [sup("task", "Working", task("tidy the notes"))], {"worker": json.dumps(
        {"status": "failed", "summary": "The notes app didn't open.", "artifacts": [], "evidence": []})})
    monkeypatch.setattr(daemon.daemon.supervisor, "verify", failing)
    out = outcome(chat(daemon, "tidy the notes"))
    assert out["summary"] == "I couldn't finish this task. The notes app didn't open.", out
    assert ".." not in out["summary"]
