"""A capability table per runtime; the orchestrator, worker and checker each pick their runtime and model (D32)."""
import io
from pathlib import Path

from ferb.cli import runtimes as runtimes_cli
from ferb.config import Config
from ferb.core.types import Capabilities
from ferb.runtimes.catalog import CATALOG, ROLES
from tests.conftest import RunningDaemon


def test_every_catalog_runtime_declares_its_capabilities() -> None:
    for name, info in CATALOG.items():
        caps = info.capabilities(info.default_model)
        assert isinstance(caps, Capabilities) and caps.native_tools_contained, name


def test_codex_catalog_default_matches_the_adapter() -> None:
    from ferb.runtimes.codex_exec import MODEL
    assert CATALOG["codex"].default_model == MODEL


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


def test_show_names_the_cache_routing_column(tmp_path: Path) -> None:
    out = io.StringIO()
    runtimes_cli.show(tmp_path / "home", out)
    header = out.getvalue().splitlines()[0]
    assert "cache key" in header and " key " not in header.replace("cache key", "")


def test_codex_receives_image_tool_results() -> None:
    """Verified live 2026-10-08: an MCP tool returning a PNG through FERB's shim format reached Codex 0.157.1 (it named
    the picture's colour), so screenshots taken by FERB's tools are visible to Codex workers."""
    from ferb.runtimes.codex_exec import CodexRuntime
    assert CodexRuntime.capabilities.image_input is True


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
    assert rows["claude"]["usable"] is True and rows["codex"]["model"] == CATALOG["codex"].default_model
    (tmp_path / "codex-home").mkdir()
    (tmp_path / "codex-home" / "auth.json").write_text("{}")
    assert {r["name"]: r for r in runtimes_cli.status(tmp_path)["runtimes"]}["codex"]["usable"] is True


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
