"""L34 Phase F: `ferb uninstall` removes FERB.app's login LaunchAgent (the fallback when macOS refuses the app's login
item), and only a file FERB wrote."""

import plistlib
import subprocess
from pathlib import Path

from ferb.device.install import APP_LOGIN_LABEL, Installer


def _quiet(cmd: list[str]) -> "subprocess.CompletedProcess[str]":
    """launchctl answers 'not loaded'; nothing else runs."""
    return subprocess.CompletedProcess(cmd, 113, "", "")


def _inst(tmp_path: Path) -> Installer:
    return Installer(tmp_path / ".ferb", run=_quiet, launch_agents=tmp_path / "LaunchAgents")


def test_the_label_matches_the_apps_plan() -> None:
    source = (Path(__file__).resolve().parent.parent / "ui" / "mac" / "FERBNotch" / "Sources" / "NotchCore"
              / "LoginItemPlan.swift").read_text()
    assert f'public static let label = "{APP_LOGIN_LABEL}"' in source


def test_uninstall_removes_the_apps_login_agent(tmp_path: Path) -> None:
    inst = _inst(tmp_path)
    login = tmp_path / "LaunchAgents" / f"{APP_LOGIN_LABEL}.plist"
    login.parent.mkdir(parents=True)
    login.write_bytes(plistlib.dumps({"Label": APP_LOGIN_LABEL, "ProgramArguments": ["/usr/bin/open", "-g", "-a", "x"]}))
    removed = inst.uninstall()
    assert not login.exists() and str(login) in removed


def test_uninstall_leaves_a_file_with_another_label(tmp_path: Path) -> None:
    inst = _inst(tmp_path)
    login = tmp_path / "LaunchAgents" / f"{APP_LOGIN_LABEL}.plist"
    login.parent.mkdir(parents=True)
    login.write_bytes(plistlib.dumps({"Label": "com.example.mine"}))
    inst.uninstall()
    assert login.exists()
    login.write_text("not a plist")
    inst.uninstall()
    assert login.exists()
