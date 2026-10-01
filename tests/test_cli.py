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

from ferb.cli import main as cli_main
from ferb.cli.admin import render_trace
from ferb.cli.client import Client
from ferb.cli.logs import run as logs_run
from tests.conftest import RunningDaemon

GOLDEN_TRACE = """\
task t1
└─ task [id-000002] 5.000s {"goal":"demo"}
   ├─ tool.call [id-000003] 1.500s {"tool":"fs.read"}
   └─ tool.call [id-000004] 0.000s ERROR denied {"tool":"shell"}"""   # id-000001: daemon.started entry


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
    ferb = Path(sys.executable).parent / "ferb"
    p = subprocess.Popen([str(ferb), "daemon", "run"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
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
    kinds = [e.kind for e in Journal(db, SystemClock(), RandomIds(), lambda d: d).entries()]
    assert kinds == ["daemon.started", "daemon.stopped"]


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
