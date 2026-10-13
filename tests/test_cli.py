"""CLI: `ferb trace` rendering (golden), `ferb logs` live stream with filters, daemon-not-running message,
and the daemon lifecycle (journal entries) through the real `ferb daemon run` process."""

import io
import os
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any

import pytest

from ferb.cli import main as cli_main
import json

from ferb.cli.admin import render_trace
from ferb.cli.client import Client
from ferb.cli.logs import run as logs_run
from tests.conftest import RunningDaemon

GOLDEN_TRACE = """\
task t1
09:00:00  task  demo  [5.0s]
  09:00:00  fs.read    [1.5s]
  09:00:01  shell    [0.0s]
      ✗ denied"""


def seed_trace(d: RunningDaemon) -> None:
    async def go() -> None:
        sp, clock = d.daemon.spans, d.daemon.clock
        assert sp is not None
        root = sp.start("task", task_id="t1", input={"goal": "demo"})
        a = sp.start("tool.call", task_id="t1", parent_id=root.id, input={"tool": "fs.read"})
        clock.advance(1.5)                                     # type: ignore[attr-defined]
        sp.finish(a, output={"bytes": 3})
        b = sp.start("tool.call", task_id="t1", parent_id=root.id, input={"tool": "shell"})
        sp.finish(b, error="denied")
        clock.advance(3.5)                                     # type: ignore[attr-defined]
        sp.finish(root, cost_usd=0.012)
    d.run(go())


def test_trace_renders_span_tree(daemon: RunningDaemon) -> None:
    seed_trace(daemon)
    c = Client(daemon.sock)
    assert "\n".join(render_trace(c.call("trace.get", {"task_id": "t1"}))) == GOLDEN_TRACE
    assert render_trace(c.call("trace.get", {"task_id": "nope"})) == ["no spans for task nope"]
    c.close()


def test_logs_streams_live_events_with_filters(daemon: RunningDaemon) -> None:
    out = io.StringIO()
    c = Client(daemon.sock)
    t = threading.Thread(target=logs_run, args=(c, ["tasks"], out, 2), daemon=True)
    t.start()
    time.sleep(0.3)                                            # let the subscription register
    daemon.call_soon(daemon.daemon.bus.publish, "trail.event", {"app": "Safari"})   # filtered out
    daemon.call_soon(daemon.daemon.bus.publish, "task.created", {"id": "t9"})
    daemon.call_soon(daemon.daemon.bus.publish, "task.finished", {"id": "t9"})
    t.join(5)
    lines = out.getvalue().splitlines()
    assert len(lines) == 2
    assert "task.created" in lines[0] and '"id":"t9"' in lines[0] and " ! " in lines[0]  # critical marker
    assert "task.finished" in lines[1]
    assert "trail.event" not in out.getvalue()
    c.close()


def test_cli_says_daemon_not_running(home: Path, capsys, monkeypatch) -> None:  # type: ignore[no-untyped-def]
    monkeypatch.setenv("FERB_HOME", str(home))
    assert cli_main(["trace", "t1"]) == 2
    assert "not running" in capsys.readouterr().err


def test_daemon_process_lifecycle_writes_journal(home: Path) -> None:
    """Through the installed `ferb` command, as launchd will run it: start, answer, SIGTERM, clean exit."""
    env = {**os.environ, "FERB_HOME": str(home)}
    # Exercise the CLI/lifecycle in a separate interpreter while excluding owner-native capture and models.
    bootstrap = ("import ferb.daemon.main as dm; from ferb.cli import main; Real=dm.Daemon; "
                 "dm.Daemon=lambda: Real(watchers=False, watch_owner_input=False, distill=False, "
                 "retention=False, notifications=False, snapshots=False); main(['daemon','run'])")
    p = subprocess.Popen([sys.executable, "-c", bootstrap], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    sock = home / "run" / "ferb.sock"
    end = time.monotonic() + 10
    while not sock.exists():
        if p.poll() is not None or time.monotonic() > end:
            p.kill()
            raise AssertionError(f"daemon didn't start: {p.stderr.read().decode()[-500:]}")
        time.sleep(0.05)
    c = Client(sock)
    assert c.call("ping")["pid"] == p.pid
    c.close()
    p.send_signal(signal.SIGTERM)
    assert p.wait(10) == 0, p.stderr.read().decode()[-500:]
    assert not sock.exists()                                   # socket removed on clean shutdown
    from ferb.clock.clock import SystemClock
    from ferb.core.ids import RandomIds
    from ferb.memory.journal import Journal
    from ferb.store.db import Database
    db = Database(home / "ferb.db")
    entries = Journal(db, SystemClock(), RandomIds(), lambda d: d).entries()
    # A fresh home records the owner's limits decision once (nothing to change: the defaults are the decided values),
    # so a value the owner sets later is never replaced; then the lifecycle.
    # A home without FERB's Codex login can't report the model's window: said at start, once per role (L28)
    assert [e.kind for e in entries] == ["config.upgraded", "daemon.started", *["runtime.limits_unknown"] * 3,
                                         "daemon.stopped"]
    assert entries[0].data["changes"] == []


def test_ferbs_own_profile_is_described_plainly() -> None:
    """The owner asked why FERB's profile never says "connected": it's opened only when a task needs it."""
    import io as _io
    from ferb.cli.admin import browsers

    def listing(connected: bool, paired: bool = True) -> str:
        class C:
            def call(self, method: str, params: object = None) -> dict:
                return {"profiles": [{"browser": "chrome", "name": "FERB", "folder": "Profile 22", "email": "",
                                      "ferb_identity": True, "paired": paired, "identity": "ferb",
                                      "connected": connected, "version": "0.6.5" if connected else "",
                                      "refused": None}]}
        out = _io.StringIO()
        browsers(C(), None, None, None, out)                                 # type: ignore[arg-type]
        return out.getvalue()
    assert "FERB's own profile, paired; not open right now (FERB opens it when a task needs it)" in listing(False)
    assert "FERB's own profile, connected, extension 0.6.5" in listing(True)
    assert "not paired yet (`ferb setup --identity`)" in listing(False, paired=False)
    assert "build step" not in listing(False) + listing(True)


def test_trace_shows_every_step_and_what_came_back(daemon: RunningDaemon) -> None:
    """Owner's request (2026-10-01): see the whole trail — steps run as their own tasks (<id>.<step>) and were
    missing; terminal output (Claude Code) is shown as plain text."""
    async def go() -> None:
        sp = daemon.daemon.spans
        assert sp is not None
        root = sp.start("task", task_id="t2", input={"goal": "do the assignment"})
        w = sp.start("worker", task_id="t2.run_claude", parent_id=root.id, input={"goal": "drive Claude"})
        t = sp.start("tool.call", task_id="t2.run_claude", parent_id=w.id,
                     input={"tool": "shell.read", "args": {"id": "p-1"}})
        sp.finish(t, output={"output": "\x1b[2K\x1b[3GI couldn't get the brief\r\n\x1b[?25l✻ Working…\r\n"})
        sp.finish(w, output={"result": json.dumps({"status": "failed", "summary": "brief needs sign-in"})})
        sp.finish(root)
    daemon.run(go())
    c = Client(daemon.sock)
    text = "\n".join(render_trace(c.call("trace.get", {"task_id": "t2"})))
    c.close()
    assert "worker  drive Claude" in text and "shell.read  id=p-1" in text
    assert "→ I couldn't get the brief" in text and "\x1b" not in text
    assert "→ failed: brief needs sign-in" in text


def test_a_slow_tool_call_isnt_reported_as_the_daemon_going_away(tmp_path: Path) -> None:
    """Owner's report (2026-10-01): waking a browser profile took 16 s, the worker's link gave up at 10 s and said
    "FERB daemon went away". Workers' calls now wait as long as FERB's longest legitimate wait; a timeout says so."""
    import socket as so
    import threading

    from ferb.cli.client import Client
    from ferb.core.errors import NotRunning
    # Workers' calls (the MCP shim) have no cutoff at all: tests/test_questions_persistence.py
    import tempfile
    path = tempfile.mkdtemp(prefix="ferb", dir="/tmp") + "/s.sock"            # macOS: socket paths stay short
    srv = so.socket(so.AF_UNIX, so.SOCK_STREAM)
    srv.bind(path)
    srv.listen(1)

    def serve() -> None:
        conn, _ = srv.accept()
        f = conn.makefile("r")
        f.readline()
        conn.sendall(b'{"jsonrpc":"2.0","id":1,"result":{"role":"cli"}}\n')     # hello, then silence
        f.readline()
        threading.Event().wait(2)
        conn.close()
    threading.Thread(target=serve, daemon=True).start()
    c = Client(Path(path), timeout=0.5)
    with pytest.raises(NotRunning, match="didn't answer within"):
        c.call("ping")
    c.close()
    srv.close()


def test_the_timeline_shows_each_upgraded_setting_in_full_and_an_empty_upgrade_plainly() -> None:
    from ferb.cli.admin import timeline
    changes = [{"key": "browser.download_max_mb", "from": 100, "to": 512},
               {"key": "browser.download_task_max_files", "from": 20, "to": 50},
               {"key": "browser.download_task_max_mb", "from": 500, "to": 4096},
               {"key": "browser.download_total_mb", "from": 2048, "to": 20480},
               {"key": "browser.task_max_tabs", "from": 20, "to": 50},
               {"key": "retention.download_days", "from": 30, "to": None}]

    class C:
        def call(self, method: str, params: Any = None) -> Any:
            return {"entries": [
                {"at": "2026-10-09T10:00:00", "kind": "config.upgraded",
                 "data": {"upgrade": "download-limits-2026-10-09", "changes": changes}},
                {"at": "2026-10-09T10:01:00", "kind": "config.upgraded",
                 "data": {"upgrade": "download-limits-2026-10-09", "changes": []}}]}
    out = io.StringIO()
    assert timeline(C(), None, out) == 0                                   # type: ignore[arg-type]
    first, second = out.getvalue().splitlines()
    for ch in changes[:-1]:
        assert f"{ch['key']} {ch['from']} → {ch['to']}" in first
    assert "retention.download_days 30 → removed" in first             # the last one too: nothing cut off
    assert "download-limits-2026-10-09" in second and "settings already current" in second
