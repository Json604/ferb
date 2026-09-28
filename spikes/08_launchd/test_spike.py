"""Spike 8: launchd user agent start/stop/restart + the CLI reaching it over an owner-only Unix socket.

Installs com.ferb.spike8 into ~/Library/LaunchAgents for the duration of the tests and removes it at the end.
"""

import os
import signal
import socket
import stat
import time

import pytest

import agent as A


@pytest.fixture(scope="module", autouse=True)
def installed():
    A.uninstall()  # clean slate if a previous run was interrupted
    A.install()
    yield
    A.uninstall()
    assert not A.PLIST.exists() and not A.loaded()


@pytest.fixture
def running():
    if not A.loaded():
        A.start()
    return A.wait_ping()


def test_launchd_starts_daemon_and_cli_reaches_it(running):
    assert running["pid"] == A.launchd_pid()
    assert running["peer_uid"] == os.getuid()        # daemon sees who connected (CLI role = same user)
    assert running["executable"] == str(A.VENV_PY)   # the launchd-started process, not this test process
    assert running["pid"] != os.getpid()


def test_socket_is_owner_only(running):
    assert stat.S_IMODE(A.SOCK.parent.stat().st_mode) == 0o700
    assert stat.S_IMODE(A.SOCK.stat().st_mode) == 0o600
    assert A.SOCK.stat().st_uid == os.getuid()


def test_launchd_restarts_daemon_after_crash(running):
    old = running["pid"]
    os.kill(old, signal.SIGKILL)
    end = time.monotonic() + 15
    while True:
        try:
            new = A.call("ping")["pid"]
            if new != old:
                break
        except A.CliError:
            pass
        assert time.monotonic() < end, "launchd did not restart the daemon"
        time.sleep(0.2)
    assert new == A.launchd_pid()


def test_stop_unloads_and_cli_fails_cleanly(running):
    pid = running["pid"]
    A.stop()
    assert not A.loaded()
    with pytest.raises(A.CliError, match="daemon not running"):
        A.call("ping")
    with pytest.raises(ProcessLookupError):
        os.kill(pid, 0)


def test_stale_socket_file_does_not_block_start():
    A.stop()
    A.SOCK.unlink(missing_ok=True)
    stale = socket.socket(socket.AF_UNIX)
    stale.bind(str(A.SOCK))   # a socket file nobody listens on, as after a crash
    stale.close()
    assert A.SOCK.exists()
    A.start()
    assert A.wait_ping()["pid"] == A.launchd_pid()


def test_tcc_is_reported_for_the_launchd_process(running):
    """Grants belong to the responsible process: the launchd daemon has its own (not the terminal's)."""
    t = A.call("tcc", timeout=15)
    assert t["accessibility"] in ("granted", "denied")
    assert t["screen_recording"] in ("granted", "denied")
    assert t["automation_chrome"] in ("granted", "denied", "not_determined", "target_not_running")
