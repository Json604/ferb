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
