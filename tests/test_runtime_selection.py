"""One runtime table; the orchestrator, worker and checker each pick their runtime and model (D32)."""
import io
from pathlib import Path

from ferb.cli import runtimes as runtimes_cli
from ferb.config import Config
from ferb.runtimes.catalog import CATALOG, ROLES
from tests.conftest import RunningDaemon


def test_every_runtime_in_the_table_has_a_factory_limits_and_a_sign_in() -> None:
    for name, entry in CATALOG.items():
        assert entry.name == name and callable(entry.build) and entry.sign_in in ("codex", "claude", "key"), name
        assert entry.billing in ("subscription", "api"), name


def test_codex_adapter_takes_its_default_model_from_the_table() -> None:
    from ferb.runtimes.codex_exec import CodexRuntime
    rt = CodexRuntime(Path("/fake/codex"), "supervisor", Path("/nonexistent"))
    assert rt.model == CATALOG["codex"].default_model
    import ferb.runtimes.codex_exec as cx
    assert not hasattr(cx, "MODEL")                           # one place for the default: the runtime table


def test_checker_role_falls_back_to_the_worker_runtime(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    d.config.set("runtimes", "worker", "claude")
    assert d._runtime_for("checker") == "claude"
    d.config.set("runtimes", "checker", "codex")
    assert d._runtime_for("checker") == "codex" and d._runtime_for("worker:default") == "claude"
    assert d._runtime_for("worker:codex") == "codex"            # a planner-chosen worker type still wins


def test_model_comes_from_config(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    assert d._model_for("codex") is None
    d.config.set("models", "codex", "gpt-6-sol")
    assert d._model_for("codex") == "gpt-6-sol"


def test_verifier_runs_ask_for_the_checker_role(daemon: RunningDaemon) -> None:
    from tests.conftest import chat, done, install, sup, task
    roles: list[str] = []
    install(daemon, [sup("task", "Checking.", task("check a file"))], {"worker": done("checked")})
    inner = daemon.daemon.supervisor.make_runtime

    def spy(role, **kw):
        roles.append(role)
        return inner(role, **kw)
    daemon.daemon.supervisor.make_runtime = spy
    chat(daemon, "check a file")
    assert "checker" in roles and "worker:default" in roles


def test_runtimes_use_writes_config_and_show_lists_roles(tmp_path: Path) -> None:
    home = tmp_path / "home"
    out = io.StringIO()
    assert runtimes_cli.use(home, "orchestrator", "claude", None, out) == 0
    assert Config(home).get("runtimes", "orchestrator") == "claude"
    assert runtimes_cli.use(home, "planner", "claude", None, out) == 1         # unknown role
    assert runtimes_cli.use(home, "worker", "nope", None, out) == 1           # unknown runtime
    out = io.StringIO()
    assert runtimes_cli.show(home, out) == 0
    text = out.getvalue()
    assert all(role in text for role in ROLES) and "codex" in text and "claude" in text


def test_use_warns_when_the_runtime_is_not_usable_yet(tmp_path: Path, monkeypatch) -> None:
    import ferb.runtimes.keys as keys
    monkeypatch.setattr(keys, "has_api_key", lambda provider: False)
    out = io.StringIO()
    assert runtimes_cli.use(tmp_path / "home", "orchestrator", "openai", None, out) == 0      # configuring ahead is allowed
    assert Config(tmp_path / "home").get("runtimes", "orchestrator") == "openai"
    assert "openai isn't usable on this Mac yet: run `ferb setup --key openai`. The orchestrator will fail" in out.getvalue()
    monkeypatch.setattr(keys, "has_api_key", lambda provider: True)
    out = io.StringIO()
    runtimes_cli.use(tmp_path / "home", "orchestrator", "openai", None, out)
    assert "isn't usable" not in out.getvalue()


def test_show_lists_each_runtimes_model_billing_sign_in_and_window(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setattr("ferb.runtimes.keys.has_api_key", lambda provider: False)
    out = io.StringIO()
    runtimes_cli.show(tmp_path / "home", out)
    lines = out.getvalue().splitlines()
    assert lines[0].split() == ["runtime", "usable", "model", "billing", "sign-in", "window"]
    anthropic = next(line for line in lines if line.startswith("anthropic "))
    assert "claude-sonnet-5-5" in anthropic and "api" in anthropic and "1,000,000" in anthropic
    claude = next(line for line in lines if line.startswith("claude "))
    assert "(none)" in claude and "unknown" in claude and "subscription" in claude


def test_live_tests_use_ferbs_own_codex_login() -> None:
    """Live tests run daemons in temporary homes; they borrow FERB's own Codex login, never the owner's ~/.codex."""
    import ferb.runtimes.codex_exec as cx
    from tests.conftest import real_codex_login
    login = real_codex_login()
    assert login is None or login == Path.home() / ".ferb" / "codex-home"
    assert cx.codex_home(Path("/nonexistent-ferb-home")) is None    # offline tests are untouched

# ---------- L34 C: Choose your AI ----------

import pytest

from ferb.cli.client import Client, RpcError


def test_status_lists_each_runtime_and_what_each_role_uses(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("ferb.runtimes.keys.has_api_key", lambda provider: provider == "openai")
    monkeypatch.setattr("ferb.runtimes.codex_exec.codex_binary", lambda *a, **k: Path("/x/codex"))
    out = runtimes_cli.status(tmp_path)
    rows = {r["name"]: r for r in out["runtimes"]}
    assert out["roles"] == {"orchestrator": "codex", "worker": "codex", "checker": "codex"}
    assert set(rows) == set(CATALOG)
    assert rows["codex"]["usable"] is False and "codex-login" in rows["codex"]["fix"]   # FERB's own Codex not signed in
    assert rows["openai"]["usable"] is True and rows["anthropic"]["usable"] is False
    assert rows["codex"]["model"] == CATALOG["codex"].default_model
    # Claude: only with FERB's own sign-in AND a model (no default, no fallback to the owner's ~/.claude)
    assert rows["claude"]["usable"] is False and "claude-login" in rows["claude"]["fix"]
    (tmp_path / "codex-home").mkdir()
    (tmp_path / "codex-home" / "auth.json").write_text("{}")
    from ferb.runtimes.claude_login import mark
    mark(tmp_path)
    rows = {r["name"]: r for r in runtimes_cli.status(tmp_path)["runtimes"]}
    assert rows["codex"]["usable"] is True
    assert rows["claude"]["usable"] is False and rows["claude"]["signed_in"] is True and "--model" in rows["claude"]["fix"]
    Config(tmp_path).set("models", "claude", "opus")
    assert {r["name"]: r for r in runtimes_cli.status(tmp_path)["runtimes"]}["claude"]["usable"] is True


def test_assign_points_the_roles_at_one_runtime_or_names_what_is_unknown(tmp_path: Path) -> None:
    assert runtimes_cli.assign(tmp_path, "claude") == {"orchestrator": "claude", "worker": "claude", "checker": "claude"}
    assert runtimes_cli.assign(tmp_path, "codex", ["worker"]) == {"orchestrator": "claude", "worker": "codex",
                                                                   "checker": "claude"}
    with pytest.raises(ValueError, match="Unknown runtime"):
        runtimes_cli.assign(tmp_path, "gpt-9")
    with pytest.raises(ValueError, match="Unknown role"):
        runtimes_cli.assign(tmp_path, "codex", ["boss"])
    with pytest.raises(ValueError, match="at least one role"):
        runtimes_cli.assign(tmp_path, "codex", [])


def test_ferb_runtimes_use_still_says_what_it_did(tmp_path: Path) -> None:
    out = io.StringIO()
    assert runtimes_cli.use(tmp_path, "worker", "claude", None, out) == 0
    assert "The worker now uses claude from its next call." in out.getvalue()
    bad = io.StringIO()
    assert runtimes_cli.use(tmp_path, "boss", "claude", None, bad) == 1 and "Unknown role 'boss'" in bad.getvalue()


def test_choose_your_ai_switches_every_role_and_journals_it(daemon: RunningDaemon,
                                                            monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("ferb.runtimes.keys.has_api_key", lambda provider: False)
    client = Client(daemon.sock)
    try:
        reply = client.call("runtimes.use", {"runtime": "anthropic"})
        status = client.call("runtimes.status")
        with pytest.raises(RpcError, match="Unknown runtime"):
            client.call("runtimes.use", {"runtime": "nope"})
        with pytest.raises(RpcError, match="roles must be a list"):
            client.call("runtimes.use", {"runtime": "codex", "roles": "worker"})
    finally:
        client.close()
    assert reply == {"roles": {"orchestrator": "anthropic", "worker": "anthropic", "checker": "anthropic"},
                     "usable": False, "fix": "run `ferb setup --key anthropic`"}
    assert status["roles"]["worker"] == "anthropic"
    assert daemon.daemon._runtime_for("orchestrator") == "anthropic"


def test_a_runtime_checks_failure_detail_is_redacted(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    daemon.call_soon(d._add_secret_value, "sk-proj-secretvalue0123456789")

    def boom(*args: object, **kwargs: object) -> object:
        raise RuntimeError("Incorrect API key provided: sk-proj-secretvalue0123456789")
    d._build_runtime = boom                                    # type: ignore[method-assign]
    client = Client(daemon.sock)
    try:
        reply = client.call("runtimes.check", {"runtime": "openai"})
    finally:
        client.close()
    assert reply["ok"] is False and "sk-proj-secretvalue0123456789" not in reply["detail"]
