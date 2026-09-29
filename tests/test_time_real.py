"""Build step 14, real (pytest -m real): safety test 19 against real daemon processes (as launchd runs FERB): a
reminder claimed when the daemon "crashed" is delivered exactly once after it restarts, and not again after another
restart. It shows one real macOS notification ("FERB reminder: test 19")."""

import json
import os
import signal
import sqlite3
import subprocess
import sys
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

pytestmark = pytest.mark.real


def _start(home: Path) -> subprocess.Popen[bytes]:
    p = subprocess.Popen([str(Path(sys.executable).parent / "ferb"), "daemon", "run"],
                         env={**os.environ, "FERB_HOME": str(home)}, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    end = time.monotonic() + 15
    while not (home / "run" / "ferb.sock").exists():
        assert p.poll() is None and time.monotonic() < end, p.stderr.read().decode()[-500:] if p.stderr else ""
        time.sleep(0.1)
    return p


def _stop(p: subprocess.Popen[bytes]) -> None:
    p.send_signal(signal.SIGTERM)
    assert p.wait(15) == 0


def _deliveries(home: Path) -> list[str]:
    c = sqlite3.connect(home / "ferb.db")
    try:
        return [json.loads(r[0])["timer_id"] for r in c.execute("SELECT data_json FROM journal WHERE kind = "
                                                                 "'timer.delivered'")]
    finally:
        c.close()


def test_19_real_daemon_crash_then_restart_delivers_exactly_once() -> None:
    import shutil
    import tempfile
    base = Path(tempfile.mkdtemp(prefix="f19-", dir="/tmp"))           # a unix socket path must stay short
    home = base / "home"
    home.mkdir(mode=0o700)
    try:
        _run19(home)
    finally:
        shutil.rmtree(base, ignore_errors=True)


def _run19(home: Path) -> None:
    _stop(_start(home))                                            # creates and migrates the database
    c = sqlite3.connect(home / "ferb.db")
    due = (datetime.now(timezone.utc) - timedelta(seconds=5)).isoformat()
    c.execute("INSERT INTO timers (id, kind, fire_at, message, state, claim_token, claimed_at, created_at) VALUES "
              "('tm-19', 'reminder', ?, 'test 19', 'claimed', 'crashed', ?, ?)", (due, due, due))   # claimed, then crash
    c.commit()
    c.close()
    p = _start(home)
    end = time.monotonic() + 40
    while not _deliveries(home) and time.monotonic() < end:
        time.sleep(0.5)
    _stop(p)
    assert _deliveries(home) == ["tm-19"]
    p = _start(home)                                               # another restart: nothing again
    time.sleep(3)
    _stop(p)
    assert _deliveries(home) == ["tm-19"]
    c = sqlite3.connect(home / "ferb.db")
    assert c.execute("SELECT state FROM timers WHERE id = 'tm-19'").fetchone()[0] == "delivered"
    c.close()
