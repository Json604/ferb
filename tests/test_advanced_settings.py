"""L34 Phase E, Settings → Advanced and AI: the diagnostic file is written on this Mac with every part redacted and is
sent nowhere; `ferb setup --cli` links FERB.app's command; `ferb setup --codex-logout` signs out FERB's own Codex only."""

import io
import json
import os
import re
import stat
import subprocess
import uuid
from pathlib import Path
from typing import Any

import pytest

from ferb.cli import setup as setup_cmd
from ferb.cli.client import Client, RpcError
from ferb.daemon import auth
from ferb.daemon.reports import DIAGNOSTIC_SECTIONS
from ferb.daemon.server import INTERNAL_ERROR
from ferb.device.install import BUNDLE_CLI, Installer
from tests.conftest import RunningDaemon
from tests.support.app_bundle import make_ferb_app

SEALED = "Morvane-Tessel-2290"
API_KEY = "sk-ant-api03-" + "Qx4" * 12


def _on_loop(daemon: RunningDaemon, fn: Any) -> Any:
    async def go() -> Any:
        return fn()
    return daemon.run(go())


def _fake_health(daemon: RunningDaemon, note: str = "fine") -> None:
    async def health(session: Any, params: dict[str, Any]) -> dict[str, Any]:
        assert params == {"selftest": False, "hearing": False}
        return {"permissions": {"accessibility": "granted"}, "note": note, "version": "test"}

    async def recording(session: Any, params: dict[str, Any]) -> dict[str, Any]:
        return {"paused": False, "watchers": ["app_focus"]}
    daemon.daemon._doctor_check = health                 # type: ignore[method-assign]
    daemon.daemon._sense_status = recording              # type: ignore[method-assign]


def _export(daemon: RunningDaemon, **params: Any) -> dict[str, Any]:
    c = Client(daemon.sock, timeout=30)
    try:
        return c.call("reports.diagnostic", params)
    finally:
        c.close()


def test_the_diagnostic_file_has_its_sections_and_is_owner_only(daemon: RunningDaemon) -> None:
    _fake_health(daemon)
    r = _export(daemon, app_version="0.28.0 (202611010900)")
    path = Path(r["path"])
    assert path.parent == daemon.daemon.home / "reports"
    assert re.fullmatch(r"FERB-diagnostic-\d{4}-\d{2}-\d{2}-\d{6}\.txt", path.name)
    assert stat.S_IMODE(path.stat().st_mode) == 0o600 and stat.S_IMODE(path.parent.stat().st_mode) == 0o700
    text = path.read_text()
    heads = [text.index(f"\n## {name}\n") for name in DIAGNOSTIC_SECTIONS]
    assert heads == sorted(heads)
    assert "App version: 0.28.0 (202611010900)" in text and '"accessibility": "granted"' in text
    assert '"app_focus"' in text
    built = _on_loop(daemon, lambda: daemon.daemon.journal.of_kind("diagnostic.built"))
    assert built and built[-1].data["file"] == str(path)


def test_every_part_of_the_diagnostic_file_is_redacted(daemon: RunningDaemon) -> None:
    _fake_health(daemon, note=f"{SEALED} {API_KEY}")
    _on_loop(daemon, lambda: daemon.daemon.redact.remember_hidden_secret(SEALED))     # type: ignore[attr-defined]
    _on_loop(daemon, lambda: daemon.daemon.db.conn.execute(                            # type: ignore[union-attr]
        "INSERT INTO journal (id, at, kind, data_json) VALUES (?, ?, ?, ?)",
        (uuid.uuid4().hex, daemon.daemon.clock.now().isoformat(), INTERNAL_ERROR,
         json.dumps({"method": "x", "error": "KeyError", "message": SEALED, "frames": []}))))
    text = Path(_export(daemon)["path"]).read_text()
    assert SEALED not in text and API_KEY not in text and "sk-ant" not in text and "[redacted]" in text


def test_old_diagnostic_files_go_and_failure_reports_stay(daemon: RunningDaemon) -> None:
    _fake_health(daemon)
    folder = daemon.daemon.home / "reports"
    folder.mkdir(parents=True, exist_ok=True)
    old_diag = folder / "FERB-diagnostic-2025-01-01-120000.txt"
    old_report = folder / "FERB-report-2025-01-01-t-old.txt"
    past = daemon.daemon.clock.now().timestamp() - 40 * 86400
    for f in (old_diag, old_report):
        f.write_text("old")
        os.utime(f, (past, past))
    _export(daemon)
    assert not old_diag.exists() and old_report.exists()


def test_a_redactor_without_a_text_path_writes_no_diagnostic_file(daemon: RunningDaemon) -> None:
    _fake_health(daemon)
    daemon.daemon.redact = lambda value: value          # type: ignore[assignment]
    with pytest.raises(RpcError, match="couldn't redact"):
        _export(daemon)


def test_only_the_owners_connection_may_export_a_diagnostic_file() -> None:
    assert auth.allowed(auth.Role.CLI, "reports.diagnostic")
    for role in (auth.Role.MCP_SHIM, auth.Role.NATIVE_HOST):
        assert not auth.allowed(role, "reports.diagnostic")


def _env(tmp_path: Path, bundle: bool = True) -> tuple[setup_cmd.Env, io.StringIO]:
    app = make_ferb_app(tmp_path / "Applications") if bundle else None
    inst = Installer(tmp_path / ".ferb", launch_agents=tmp_path / "LaunchAgents", bundle=app, cli_dir=tmp_path / "bin")
    out = io.StringIO()
    return setup_cmd.Env(inst, lambda: None, lambda prompt: "", out), out     # type: ignore[arg-type,return-value]


def test_install_command_line_tool_links_ferb_apps_command(tmp_path: Path) -> None:
    env, out = _env(tmp_path)
    assert setup_cmd.cli_setup(env) == 0
    link = tmp_path / "bin" / "ferb"
    assert link.is_symlink() and link.resolve() == (env.inst.bundle / BUNDLE_CLI).resolve()   # type: ignore[operator]
    assert "`ferb` command: " in out.getvalue() and ".zprofile" in out.getvalue()


def test_install_command_line_tool_leaves_another_ferb_alone(tmp_path: Path) -> None:
    env, out = _env(tmp_path)
    (tmp_path / "bin").mkdir()
    (tmp_path / "bin" / "ferb").write_text("#!/bin/sh\necho mine\n")
    assert setup_cmd.cli_setup(env) == 1
    assert (tmp_path / "bin" / "ferb").read_text() == "#!/bin/sh\necho mine\n"
    assert "left" in out.getvalue()


def test_install_command_line_tool_is_for_ferb_app_only(tmp_path: Path) -> None:
    env, out = _env(tmp_path, bundle=False)
    assert setup_cmd.cli_setup(env) == 1 and "FERB.app" in out.getvalue()


def test_sign_out_of_chatgpt_signs_out_ferbs_own_codex_only(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    env, out = _env(tmp_path)
    codex_home = env.inst.home / "codex-home"
    codex_home.mkdir(parents=True)
    (codex_home / "auth.json").write_text("{}")
    monkeypatch.setattr("ferb.runtimes.codex_exec.codex_binary", lambda home, *a: tmp_path / "codex")
    seen: list[tuple[list[str], str]] = []

    def run(argv: list[str], env: dict[str, str]) -> subprocess.CompletedProcess[str]:
        seen.append((argv, env["CODEX_HOME"]))
        (codex_home / "auth.json").unlink()
        return subprocess.CompletedProcess(argv, 0, "", "")
    assert setup_cmd.codex_logout(env, run=run) == 0
    assert seen == [([str(tmp_path / "codex"), "logout"], str(codex_home))]
    assert "Signed out" in out.getvalue()


def test_sign_out_without_a_sign_in_changes_nothing(tmp_path: Path) -> None:
    env, out = _env(tmp_path)
    assert setup_cmd.codex_logout(env, run=lambda *a, **k: pytest.fail("ran codex")) == 0
    assert "isn't signed in" in out.getvalue()


def test_send_diagnostics_names_the_developer_address_and_shows_setup_events(daemon: RunningDaemon) -> None:
    """Owner 2026-10-27: Settings → Send diagnostics to developer, emailed like a failure report (a new email in the
    owner's own Mail app; FERB sends nothing), with the recent setup and pairing events a first install hits."""
    _fake_health(daemon)
    _on_loop(daemon, lambda: daemon.daemon._journal().append("browsers.pair_failed", {
        "browser": "chrome", "folder": "Profile 3", "why": "the pairing page wasn't answered"}))
    _on_loop(daemon, lambda: daemon.daemon._journal().append("task.created", {"goal": "not a setup event"}))
    r = _export(daemon, send=True, app_version="0.28.0")
    assert r["address"] == "ktky.attri@gmail.com" and r["subject"].startswith("FERB diagnostics (FERB ")
    text = Path(r["path"]).read_text()
    events = text[text.index("\n## Recent setup, pairing and watcher events\n"):]
    assert "browsers.pair_failed" in events and "Profile 3" in events and "not a setup event" not in events
    assert "address" not in _export(daemon)                            # Export alone names no one
