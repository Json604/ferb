"""The owner's effort setting (owner 2026-11-10: "show the effort level along with model selection"): who sends what,
what each adapter's request carries, checking a level before it is saved, the levels offered per model, an older
config, and Settings → AI's facts (each role's connection, model and effort; a runtime's last refusal)."""
import json
from pathlib import Path
from typing import Any

import pytest

import ferb.runtimes.codex_exec as cx
from ferb.core.errors import FerbError
from ferb.core.types import Identity, ModelRequest, Task
from ferb.runtimes import models as models_
from ferb.runtimes.catalog import role_effort, run_effort
from tests.conftest import RunningDaemon
from tests.support.runtime_fakes import FakeBridge, Scripted

TASK = Task("t", None, "reply", "d", Identity.OWNER, set(), "mixed", "r", [], None)
REQ = ModelRequest("sys", ("ctx",))


class Cfg:
    def __init__(self, data: dict[str, dict[str, Any]]) -> None:
        self.data = data

    def get(self, section: str, key: str, default: Any = None) -> Any:
        return self.data.get(section, {}).get(key, default)


# decision: RUN-460, RUN-461, RUN-462
def test_who_sends_which_effort() -> None:
    """Planner: its own or the connection's, else low (Claude Code too). Other one-turn calls: low, whatever is set.
    Worker: its own or the connection's, else nothing (the runtime's default). Checker: its own or the connection's,
    else high for a step that changed files, else nothing. A role's own applies only while it runs there."""
    none = Cfg({})
    for name in ("codex", "claude", "anthropic", "openai", "google"):
        assert run_effort(none, name, "orchestrator", "supervisor", True, None) == "low"
        assert run_effort(none, name, "orchestrator", "supervisor", False, None) == "low"
        assert run_effort(none, name, "worker:default", "worker", False, None) is None
        assert run_effort(none, name, "checker", "worker", False, "high") == "high"
        assert run_effort(none, name, "checker", "worker", False, None) is None
    cfg = Cfg({"efforts": {"claude": "medium"},
               "role_models": {"orchestrator": {"runtime": "claude", "model": "", "effort": "high"},
                               "checker": {"runtime": "claude", "model": "", "effort": "low"},
                               "worker": {"runtime": "codex", "model": "gpt-x", "effort": "xhigh"}}})
    assert run_effort(cfg, "claude", "orchestrator", "supervisor", True, None) == "high"     # the owner's setting wins
    assert run_effort(cfg, "claude", "orchestrator", "supervisor", False, None) == "low"     # other one-turn calls
    assert run_effort(cfg, "claude", "checker:claude", "worker", False, "high") == "low"     # set: exactly that
    assert run_effort(cfg, "claude", "worker:default", "worker", False, None) == "medium"    # the connection's
    assert run_effort(cfg, "codex", "worker:codex", "worker", False, None) == "xhigh"        # its own, on its runtime
    assert role_effort(cfg, "openai", "worker") == ""                                        # chosen for codex only


def _codex_signed_in(home: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    (home / "codex-home").mkdir(exist_ok=True)
    (home / "codex-home" / "auth.json").write_text('{"tokens":{"refresh_token":"test-only-fixture"}}')
    monkeypatch.setattr(cx, "codex_binary", lambda ferb_home, repo=None: Path("/fake/codex"))


# decision: RUN-460, RUN-462
def test_the_daemon_builds_each_run_with_its_effort(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.orchestrator.schemas import QUESTION_SCHEMA, SUPERVISOR_SCHEMA, WORKER_SCHEMA
    from ferb.runtimes.claude_login import mark
    d = daemon.daemon
    _codex_signed_in(d.home, monkeypatch)
    mark(d.home)
    build = d._build_runtime
    assert build("claude", "orchestrator", mode="supervisor", tool_names=[], schema=SUPERVISOR_SCHEMA).effort == "low"
    assert build("claude", "worker:claude", mode="worker", tool_names=[], schema=WORKER_SCHEMA).effort is None
    d.config.set("role_models", "orchestrator", {"runtime": "codex", "model": "", "effort": "medium"})
    d.config.set("efforts", "codex", "high")
    planner = build("codex", "orchestrator", mode="supervisor", tool_names=[], schema=SUPERVISOR_SCHEMA)
    other = build("codex", "orchestrator", mode="supervisor", tool_names=[], schema=QUESTION_SCHEMA)
    checker = build("codex", "checker", mode="worker", tool_names=[], schema=WORKER_SCHEMA, effort="high")
    worker = build("codex", "worker:default", mode="worker", tool_names=[], schema=WORKER_SCHEMA)
    assert (planner.effort, other.effort, checker.effort, worker.effort) == ("medium", "low", "high", "high")
    assert build("codex", "orchestrator", mode="supervisor", tool_names=[], schema=QUESTION_SCHEMA,
                 fixed_effort="max").effort == "max"          # a level being checked is sent exactly


# decision: RUN-462
def test_codex_sends_the_effort_on_its_flags_and_turn(tmp_path: Path) -> None:
    from ferb.runtimes.codex_stream import _command, _turn_params, write_template
    sup = cx.CodexRuntime(Path("/x/codex"), "supervisor", tmp_path, effort="high")
    assert 'model_reasoning_effort="high"' in _command(sup, None)
    assert _turn_params(sup, REQ, "th", False)["effort"] == "high"
    plain = cx.CodexRuntime(Path("/x/codex"), "supervisor", tmp_path)
    assert 'model_reasoning_effort="low"' in _command(plain, None)     # a supervisor run built without one: low
    work = cx.CodexRuntime(Path("/x/codex"), "worker", tmp_path, shim_cmd=["ferb", "mcp"], effort="xhigh")
    assert _turn_params(work, REQ, "th", False)["effort"] == "xhigh"
    assert "effort" not in _turn_params(cx.CodexRuntime(Path("/x/codex"), "worker", tmp_path), REQ, "th", False)
    template = write_template(tmp_path, tmp_path, "th", "m", "FERB instructions", "medium")
    assert json.loads(template.read_text().splitlines()[1])["payload"]["effort"] == "medium"


def _api(provider: Any, effort: str | None, mode: str = "worker") -> tuple[Any, Scripted]:
    from ferb.runtimes.api import ApiRuntime
    script = Scripted([(500, {"error": {"message": "stop here"}})] * 6)

    async def key(provider: str) -> str:
        return "k"

    async def sleep(s: float) -> None:
        pass
    return ApiRuntime(provider, mode=mode, role="worker", tool_names=[], output_schema=None, bridge=FakeBridge(),
                      key=key, transport=script.transport(), sleep=sleep, effort=effort), script


# decision: RUN-462
@pytest.mark.anyio
async def test_each_api_request_carries_the_effort() -> None:
    from ferb.runtimes.anthropic_api import AnthropicProvider
    from ferb.runtimes.google_api import GoogleProvider
    from ferb.runtimes.openai_api import OpenAIProvider
    cases = [(AnthropicProvider("claude-sonnet-5-5"), lambda b: b["output_config"]["effort"]),
             (OpenAIProvider("gpt-6.1-sol"), lambda b: b["reasoning"]["effort"]),
             (GoogleProvider("gemini-3.8-flash"), lambda b: b["generationConfig"]["thinkingConfig"]["thinkingLevel"])]
    for provider, read in cases:
        for effort, mode, sent in (("high", "worker", "high"), (None, "supervisor", "low"),
                                   ("max", "supervisor", "max")):
            rt, script = _api(provider, effort, mode)
            run = await rt.start(TASK, REQ, "tok")
            [e async for e in rt.events(run)]
            assert read(script.requests[0][1]) == sent, (provider.name, effort, mode)
    rt, script = _api(AnthropicProvider("claude-sonnet-5-5"), None)          # a worker without one sends none
    [e async for e in rt.events(await rt.start(TASK, REQ, "tok"))]
    assert "effort" not in script.requests[0][1].get("output_config", {})


# decision: RUN-460, RUN-462
def test_claude_code_gets_the_effort_low_for_the_planner(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.runtimes import claude_login as CL
    from ferb.runtimes.claude_sdk import ClaudeRuntime
    CL.mark(tmp_path)
    monkeypatch.setattr(CL, "cli_binary", lambda: Path("/bundled/claude"))
    proxy = type("P", (), {"base_url": lambda self, run_id: "http://unused"})()
    rt = ClaudeRuntime("supervisor", proxy, str(tmp_path), effort="low")             # type: ignore[arg-type]
    assert rt._options("run", None).effort == "low"


def _rpc(daemon: RunningDaemon, name: str, params: dict[str, Any]) -> Any:
    return daemon.run(getattr(daemon.daemon, name)(None, params))


# decision: RUN-463, DMN-580
def test_an_effort_is_checked_before_it_is_saved(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    d = daemon.daemon
    calls: list[tuple[str, str, str]] = []

    async def check(name: str, model: str = "", effort: str = "") -> dict[str, Any]:
        calls.append((name, model, effort))
        return {"ok": effort != "turbo", "detail": "answered" if effort != "turbo" else "unknown effort: turbo"}
    monkeypatch.setattr(d, "_check_runtime", check)
    ok = _rpc(daemon, "_runtimes_model", {"runtime": "claude", "model": "claude-opus-5-5", "effort": "high"})
    assert ok["ok"] and d.config.get("efforts", "claude") == "high"
    assert calls == [("claude", "claude-opus-5-5", "high")]            # the model's check is the same one call
    refused = _rpc(daemon, "_runtimes_model", {"runtime": "claude", "model": "claude-opus-5-5", "effort": "turbo"})
    assert not refused["ok"] and "turbo" in refused["detail"]
    assert d.config.get("efforts", "claude") == "high"                 # the working one stays
    with pytest.raises(FerbError, match="one short word"):
        _rpc(daemon, "_runtimes_model", {"runtime": "claude", "model": "claude-opus-5-5", "effort": "very high!"})
    with pytest.raises(FerbError, match="ultra"):
        _rpc(daemon, "_runtimes_model", {"runtime": "codex", "model": "gpt-5.6-sol", "effort": "ultra"})
    assert _rpc(daemon, "_runtimes_model", {"runtime": "claude", "model": "claude-opus-5-5", "effort": ""})["ok"]
    assert d.config.get("efforts", "claude") is None                   # "" clears it
    # a role's effort with the connection's model: saved on the role's row, and an empty row goes
    assert _rpc(daemon, "_runtimes_model", {"runtime": "claude", "model": "", "role": "orchestrator",
                                            "effort": "medium"})["ok"]
    assert d.config.get("role_models", "orchestrator") == {"runtime": "claude", "model": "", "effort": "medium"}
    assert _rpc(daemon, "_runtimes_model", {"runtime": "claude", "model": "", "role": "orchestrator",
                                            "effort": ""})["ok"]
    assert d.config.get("role_models", "orchestrator") is None


# decision: RUN-464, RUN-463
def test_each_model_lists_its_levels(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    catalog = {"models": [{"slug": "gpt-6.1-sol", "default_reasoning_level": "low", "supported_reasoning_levels": [
        {"effort": e} for e in ("low", "medium", "high", "xhigh", "max", "ultra")]}]}
    monkeypatch.setattr(cx, "_catalog", lambda home: (catalog, "test"))
    levels, defaults = models_.effort_levels("codex", tmp_path, [])
    assert levels == {"gpt-6.1-sol": ["low", "medium", "high", "xhigh", "max"]} and defaults == {"gpt-6.1-sol": "low"}
    levels, defaults = models_.effort_levels("anthropic", tmp_path, ["claude-opus-5-5", "claude-opus-4-6", "claude-x"])
    assert levels["claude-opus-5-5"][-2:] == ["xhigh", "max"] and "xhigh" not in levels["claude-opus-4-6"]
    assert defaults["claude-opus-5-5"] == "medium" and "claude-x" not in levels    # undocumented: Default + typed
    levels, defaults = models_.effort_levels("claude", tmp_path, ["claude-sonnet-5-5"])
    assert levels["claude-sonnet-5-5"] and defaults == {}                          # Claude Code's own default
    levels, defaults = models_.effort_levels("openai", tmp_path, ["gpt-6.1-sol"])
    assert levels == {} and defaults == {"gpt-6.1-sol": "medium"}
    google = models_.effort_levels("google", tmp_path, ["gemini-3-pro-preview"])[0]
    assert google == {"gemini-3-pro-preview": ["low", "high"]}


# decision: RUN-462, STORE-403
def test_an_older_config_runs_as_before(tmp_path: Path) -> None:
    """A config an older FERB wrote (no [efforts], role rows without effort) sends exactly what it sent then."""
    from ferb.config import Config
    (tmp_path / "config.toml").write_text('[runtimes]\nworker = "claude"\n\n[role_models]\n'
                                          'worker = {runtime = "claude", model = "claude-sonnet-5-5"}\n')
    (tmp_path / "config.toml").chmod(0o600)
    cfg = Config(tmp_path)
    assert run_effort(cfg, "claude", "worker:default", "worker", False, None) is None
    assert run_effort(cfg, "claude", "checker", "worker", False, "high") == "high"
    assert run_effort(cfg, "codex", "orchestrator", "supervisor", True, None) == "low"


# decision: DMN-580
def test_settings_read_each_roles_ai_and_a_refusal_the_presence_check_keeps(daemon: RunningDaemon,
                                                                           monkeypatch: pytest.MonkeyPatch) -> None:
    d = daemon.daemon
    monkeypatch.setattr("ferb.runtimes.keys.has_api_key", lambda provider: False)
    published: list[tuple[str, dict[str, Any]]] = []
    publish = d._publish
    d._publish = lambda topic, data: (published.append((topic, data)), publish(topic, data))[1]   # type: ignore
    d.config.set("efforts", "codex", "medium")

    async def on_loop(step: Any) -> Any:
        return step()
    daemon.run(on_loop(lambda: d._runtime_account("claude", "usage_limit")))
    status = _rpc(daemon, "_runtimes_status", {})
    claude = next(r for r in status["runtimes"] if r["name"] == "claude")
    assert claude["refused"]["account"] == "usage_limit" and claude["refused"]["at"]
    assert status["role_details"]["orchestrator"]["runtime"] == "codex"
    assert status["role_details"]["orchestrator"]["effort"] == "medium"
    assert ("runtimes.changed", {"runtime": "claude", "why": "refused"}) in published
    d._refusal_facts = None                                            # read again from the journal, as after a restart
    assert daemon.run(on_loop(d._refusals))["claude"]["account"] == "usage_limit"
    daemon.run(on_loop(lambda: d._runtime_account("claude", "")))     # a later call it answered clears it
    status = _rpc(daemon, "_runtimes_status", {})
    assert "refused" not in next(r for r in status["runtimes"] if r["name"] == "claude")
    d._refusal_facts = None
    assert "claude" not in daemon.run(on_loop(d._refusals))


# decision: DMN-580
def test_each_browser_profile_row_says_its_version_and_whether_it_is_too_old(tmp_path: Path) -> None:
    from ferb.act.browser import autoinstall
    from ferb.device.extensions import browser_rows
    old = autoinstall.ProfileStatus("chrome", "Default", "Me", "id", True, False, True, version="0.1.0")
    new = autoinstall.ProfileStatus("chrome", "Profile 1", "Work", "id", True, False, True, version="99.0.0")
    unknown = autoinstall.ProfileStatus("chrome", "Profile 2", "Other", "", False, False, None)
    rows = browser_rows([autoinstall.BrowserState("chrome", None, (old, new, unknown))], "", tmp_path, None,
                        {("chrome", "Profile 1")})["browsers"][0]["profiles"]
    assert [(r["version"], r["too_old"], r["connected"]) for r in rows] == [
        ("0.1.0", True, False), ("99.0.0", False, True), ("", False, False)]
