"""Real launchd (ported from spike 8): a uniquely-labelled agent with a temporary FERB_HOME runs the daemon; the CLI
reaches it; doctor.check answers from inside it; uninstall leaves nothing behind. Uses the dev environment's
Python (not a dedicated interpreter) so the test stays fast; the real install is exercised by `ferb setup`."""

import secrets
import shutil
import sys
import tempfile
import time
from pathlib import Path

import pytest

pytestmark = pytest.mark.real  # Native installation/restart requires an explicit owner-run live check.

from ferb.cli.client import Client
from ferb.core.errors import NotRunning
from ferb.device.install import MARKER, Installer
from ferb.paths import socket_path


class DevVenvInstaller(Installer):
    """The dev environment (a real venv with FERB installed) stands in for ~/.ferb/venv."""

    @property
    def venv(self) -> Path:
        return Path(sys.prefix)


@pytest.fixture
def agent() -> Installer:
    home = Path(tempfile.mkdtemp(prefix="ferbl-")) / "home"
    inst = DevVenvInstaller(home, label=f"com.ferb.test.{secrets.token_hex(4)}")
    inst.write_marker()
    yield inst
    inst.uninstall()
    shutil.rmtree(home.parent, ignore_errors=True)


def ping(home: Path, timeout: float = 20) -> dict:
    end = time.monotonic() + timeout
    while True:
        try:
            c = Client(socket_path(home))
            try:
                return c.call("ping")
            finally:
                c.close()
        except NotRunning:
            if time.monotonic() > end:
                raise
            time.sleep(0.2)


def test_launchd_started_daemon_answers_and_uninstall_is_clean(agent: Installer) -> None:
    agent.write_plist()
    agent.load()
    assert agent.loaded()
    r = ping(agent.home)
    assert r["pid"] == agent.agent_pid()
    c = Client(socket_path(agent.home))
    report = c.call("doctor.check", {"selftest": False})
    c.close()
    assert set(report["permissions"]) == {"accessibility", "screen_recording", "input_monitoring",
                                          "full_disk_access"}
    assert report["home"] == str(agent.home)
    assert Path(report["executable"]) == Path(sys.executable).resolve()   # the launchd process, not this one
    agent.restart()                                                        # `ferb setup --update` path
    assert ping(agent.home)["pid"] != r["pid"]
    removed = agent.uninstall()
    assert not agent.loaded() and not agent.plist_path.exists() and len(removed) == 2
    assert (agent.home / MARKER).exists()                                  # home kept without --purge
    with pytest.raises(NotRunning):
        Client(socket_path(agent.home))
