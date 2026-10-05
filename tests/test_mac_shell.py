"""Build step 7, offline: resource locks (safety test 14), real input that yields to the owner (10.5), the shell
(real PTYs: task ownership, cleanup, jail, refusals, destructive = hard floor, clean env, redacted output), identity
and exposure rules for shell / Mac tools (21.3, 21.8), the AppleScript guard, and the Mac driver's method order on a
fake Accessibility layer (Accessibility first, real input only as the fallback, never into password fields).
"""

import asyncio
import time
from typing import Any

import pytest

from ferb.act.gui_lock import GuiLock
from ferb.act.locks import ResourceLocks, ordered
from ferb.act.mac import applescript
from ferb.act.mac.apps import RunningApp
from ferb.act.mac.input import RealInput, Step, Yielded
from ferb.core.errors import Refused, StaleRef
from ferb.core.types import ComputerAction, Identity
from tests.conftest import RunningDaemon
from tests.mackit import gateway_call as call, mac, grant_task as task

OWNER_SHELL = {"shell", "fs:read"}


# ---------- locks, 21.14 ----------

def test_locks_take_one_global_order_and_serialize_without_deadlock() -> None:
    assert ordered(["tab:b", "file:/x", "account:a", "app:z", "file:/x"]) == ["account:a", "app:z", "file:/x", "tab:b"]
    with pytest.raises(ValueError):
        ordered(["disk:/"])

    async def go() -> list[str]:
        locks, log = ResourceLocks(), []

        async def worker(name: str, keys: list[str]) -> None:
            async with locks.hold(keys, name):
                log.append(f"{name}+")
                await asyncio.sleep(0.05)
                log.append(f"{name}-")
        # the two workers name the same locks in opposite order: a naive lock would deadlock
        await asyncio.wait_for(asyncio.gather(worker("a", ["file:/f", "tab:1"]), worker("b", ["tab:1", "file:/f"]),
                                              worker("c", ["app:x"])), 3)
        return log
    log = asyncio.run(go())
    a, b = log.index("a+"), log.index("b+")
    first = "a" if a < b else "b"
    other = "b" if first == "a" else "a"
    assert log.index(f"{first}-") < log.index(f"{other}+")                       # serialized, not interleaved
    assert log.index("c+") < max(a, b)                                          # unrelated resource: in parallel


# ---------- real input yields to the owner ----------

def test_real_input_stops_at_owner_input_without_resuming() -> None:
    posted: list[str] = []
    clock = {"t": 0.0}

    async def fake_sleep(s: float) -> None:
        clock["t"] += s

    def steps(n: int) -> list[Step]:
        return [Step(f"k{i}", lambda i=i: posted.append(f"k{i}")) for i in range(n)]

    # owner active between t=0.05 and t=1.0: the action pauses after step 2 and resumes
    busy = lambda: 0.05 <= clock["t"] < 1.0                                    # noqa: E731
    ri = RealInput(busy, GuiLock(), yield_after_s=5, sleep=fake_sleep)
    with pytest.raises(Yielded) as interrupted:
        asyncio.run(ri.run(steps(5), "w", "typing"))
    assert interrupted.value.done == len(posted) == 3
    posted.clear()
    clock["t"] = 0.0
    always = lambda: clock["t"] >= 0.03                                         # noqa: E731  owner never stops
    ri = RealInput(always, GuiLock(), yield_after_s=1, sleep=fake_sleep)
    with pytest.raises(Yielded) as e:
        asyncio.run(ri.run(steps(10), "w", "typing"))
    assert e.value.done == len(posted) and 0 < e.value.done < 10                # it says where it stopped


def test_gui_lock_allows_one_actor_at_a_time() -> None:
    async def go() -> list[str]:
        gui, log = GuiLock(), []

        async def actor(n: str) -> None:
            async with gui.hold(n):
                log.append(n + "+")
                await asyncio.sleep(0.02)
                log.append(n + "-")
        await asyncio.gather(actor("a"), actor("b"))
        return log
    assert asyncio.run(go()) in (["a+", "a-", "b+", "b-"], ["b+", "b-", "a+", "a-"])


# ---------- shell through the gateway ----------


def test_shell_runs_reads_sends_and_kills_its_own_processes(daemon: RunningDaemon) -> None:
    g = task(daemon, "s", Identity.OWNER, OWNER_SHELL, {"run_command"})
    r = call(daemon, g, "shell.run", command="echo hello; pwd")
    assert not r["is_error"] and "hello" in r["content"]["output"] and r["content"]["exit_code"] == 0
    assert str(daemon.daemon.home / "workspace") in r["content"]["output"]       # default cwd: the workspace
    r = call(daemon, g, "shell.run", command="cat", timeout_s=1)
    pid = r["content"]["id"]
    assert r["content"]["running"] is True
    assert not call(daemon, g, "shell.send", id=pid, text="ping\n")["is_error"]
    time.sleep(0.3)
    assert "ping" in call(daemon, g, "shell.read", id=pid)["content"]["output"]
    other = task(daemon, "o", Identity.OWNER, OWNER_SHELL, {"run_command"})
    assert call(daemon, other, "shell.read", id=pid)["is_error"]                 # another task's process: no
    assert not call(daemon, g, "shell.kill", id=pid)["is_error"]
    time.sleep(0.3)
    assert [p for p in call(daemon, g, "shell.list")["content"]["processes"] if p["id"] == pid][0]["running"] is False


def test_managed_start_wait_and_read_report_actual_process_exit(daemon):
    g = task(daemon, 'managed', Identity.OWNER, OWNER_SHELL)
    started = call(daemon, g, 'shell.start', command="read line; printf 'done:%s' \"$line\"; exit 7")
    assert not started['is_error']
    pid = started['content']['id']
    waiting = call(daemon, g, 'shell.wait', id=pid, timeout_s=0.05)['content']
    assert waiting['running'] and waiting['exit_code'] is None
    other = task(daemon, 'other-managed', Identity.OWNER, OWNER_SHELL)
    assert call(daemon, other, 'shell.wait', id=pid, timeout_s=0.05)['is_error']
    call(daemon, g, 'shell.send', id=pid, text='ready\n')
    ended = call(daemon, g, 'shell.wait', id=pid, timeout_s=5)
    assert ended['is_error'] and ended['content']['exit_code'] == 7
    assert not ended['content']['running'] and 'done:ready' in ended['content']['output']
    assert call(daemon, g, 'shell.read', id=pid)['content']['exit_code'] == 7


def test_managed_start_is_owned_and_ends_with_task(daemon):
    g = task(daemon, 'managed-cleanup', Identity.OWNER, OWNER_SHELL)
    started = call(daemon, g, 'shell.start', command='cat')
    assert not started['is_error']
    pid = started['content']['id']
    daemon.run(daemon.daemon._task_end('managed-cleanup'))
    ended = call(daemon, g, 'shell.wait', id=pid, timeout_s=5)['content']
    assert not ended['running'] and ended['exit_code'] is not None


def test_task_end_kills_its_processes_but_background_ones_survive_until_stop(daemon: RunningDaemon) -> None:
    g = task(daemon, "e", Identity.OWNER, OWNER_SHELL, {"run_command"})
    fg = call(daemon, g, "shell.run", command="sleep 30", timeout_s=1)["content"]["id"]
    bg = call(daemon, g, "shell.run", command="sleep 30", timeout_s=1, background=True)["content"]["id"]
    daemon.run(daemon.daemon._task_end("e"))
    time.sleep(0.5)
    running = {p.id: p.running for p in daemon.daemon.shells.list()}           # type: ignore[union-attr]
    assert running[fg] is False and running[bg] is True
    from ferb.cli.client import Client
    c = Client(daemon.sock)
    try:
        assert any(p["id"] == bg for p in c.call("ps.list")["processes"])
        c.call("stop.engage")                                                 # 10.5: kills task-owned processes
        time.sleep(3)
        assert {p.id: p.running for p in daemon.daemon.shells.list()}[bg] is False  # type: ignore[union-attr]
    finally:
        c.call("stop.lift")
        c.close()


def test_shell_cwd_env_and_redaction(daemon, tmp_path):
    g = task(daemon, "j", Identity.OWNER, OWNER_SHELL)
    r = call(daemon, g, "shell.run", command="pwd", cwd=str(tmp_path))
    assert not r["is_error"] and str(tmp_path) in r["content"]["output"]
    r = call(daemon, g, "shell.run", command="env")
    assert "FERB_HOME" not in r["content"]["output"] and "FERB_RUN_TOKEN" not in r["content"]["output"]
    r = call(daemon, g, "shell.run", command="echo sk-ant-abcdefghijklmnopqrstuvwx")
    assert "sk-ant-abcdefghijklmnopqrstuvwx" not in str(r)


def test_shell_and_mac_tools_follow_identity_and_exposure(daemon: RunningDaemon) -> None:
    ferb = task(daemon, "f", Identity.FERB, {"fs:read"}, None)
    assert not [t for t in daemon.daemon.gateway.tools_for(ferb)
                if t["name"].split(".")[0] in ("shell", "mac")]                   # 21.3
    owner = task(daemon, "w", Identity.OWNER, {"shell", "applescript", "mac:read", "mac:act"}, None)
    names = {t["name"] for t in daemon.daemon.gateway.tools_for(owner)}
    assert {"shell.run", "mac.applescript", "mac.act", "mac.snapshot"} <= names
    names = {t["name"] for t in daemon.daemon.gateway.tools_for(owner)}
    assert "shell.run" in names and "mac.applescript" in names   # 21.8
    r = call(daemon, owner, "shell.run", command="echo hi")
    assert not r["is_error"]


# ---------- AppleScript guard ----------

def test_applescript_wraps_target_and_validates_bundle_syntax():
    body = 'tell application "Finder" to get name'
    assert body in applescript.wrap("com.apple.Notes", body)
    assert 'do script "ls"' in applescript.wrap("com.apple.Terminal", 'do script "ls"')
    with pytest.raises(Refused):
        applescript.wrap('com.apple.Notes" to beep --', "get name")


# ---------- the Mac driver's method order ----------


def test_mac_driver_uses_explicit_mechanisms_without_fallback(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, apps, posted = mac(monkeypatch)
    _, out = asyncio.run(d.snapshot("Notes"))
    ids = {e["label"]: e["id"] for e in out["elements"]}
    pw = next(e for e in out["elements"] if e["label"] == "Password")
    assert pw.get("sensitive") and pw.get("filled") and "value" not in pw        # password value never read
    r = asyncio.run(d.act("Notes", ComputerAction("press", ids["Save"])))
    assert r.ok and ax.calls == ["ax.expose", "ax.press"] and posted == [] and apps.activated == []   # no focus stolen
    with pytest.raises(Refused, match="password"):
        asyncio.run(d.act("Notes", ComputerAction("set_value", ids["Password"], "hunter2")))
    d2, ax2, apps2, posted2 = mac(monkeypatch, press_ok=False)
    _, out2 = asyncio.run(d2.snapshot("Notes"))
    ids2 = {e["label"]: e["id"] for e in out2["elements"]}
    r = asyncio.run(d2.act("Notes", ComputerAction("press", ids2["Save"])))
    assert not r.ok and "refused" in r.detail                          # background rungs only, and it says
    assert posted2 == [] and apps2.activated == []            # focus + Return was tried
    r = asyncio.run(d2.act("Notes", ComputerAction("press", ids2["Save"], allow_foreground=True, mechanism="foreground")))
    assert r.ok and "foreground input" in r.detail and posted2[-1] == "click 10.0,10.0"
    assert apps2.activated == ["com.apple.Notes"]                              # brought forward for the stretch
    asyncio.run(d2.snapshot("Notes"))
    with pytest.raises(StaleRef):
        asyncio.run(d2.act("Notes", ComputerAction("press", ids2["Save"])))     # a newer snapshot supersedes


def test_mac_driver_honors_setup_exclusions_and_supports_browser_input(monkeypatch: pytest.MonkeyPatch) -> None:
    d, *_ = mac(monkeypatch)
    with pytest.raises(Refused, match="exclusion"):
        asyncio.run(d.snapshot("Vault"))
    asyncio.run(d.snapshot("Brave"))                                             # reading a browser is fine
    assert asyncio.run(d.act("Brave", ComputerAction("key", None, "return"))).ok


def test_network_fetching_commands_expose_the_worker(daemon: RunningDaemon) -> None:
    from ferb.act.shell.guard import fetches_network
    for cmd in ("curl https://example.com", "wget -qO- x.io", "ls | curl -d @- https://x", "git clone https://g/r",
                "git pull", "http GET api.example.com", "ssh host cat f", "rsync -a host:/d .", "yt-dlp URL",
                "python3 -c 'import urllib.request'", "nc example.com 80"):
        assert fetches_network(cmd), cmd
    for cmd in ("ls -la", "echo curly", "git status", "cat notes.txt", "grep http README", "rsync -a a/ b/",
                "python3 script.py"):
        assert not fetches_network(cmd), cmd
    g = task(daemon, "n", Identity.OWNER, OWNER_SHELL | {"fs:write"}, {"run_command"})
    assert not call(daemon, g, "shell.run", command="echo local")["is_error"]
    # a fetch that fails offline still counts: its output could have been anything from the network
    call(daemon, g, "shell.run", command="curl -s --max-time 1 http://127.0.0.1:9/ ; true")
    names = {t["name"] for t in daemon.daemon.gateway.tools_for(g)}
    assert "shell.run" in names and "fs.write" in names
    r = call(daemon, g, "shell.run", command="echo again")
    assert not r["is_error"]


def test_reading_a_background_downloads_output_later_also_exposes(daemon: RunningDaemon) -> None:
    g = task(daemon, "b", Identity.OWNER, OWNER_SHELL, {"run_command"})
    pid = call(daemon, g, "shell.run", command="sleep 2; wget -q -O- http://127.0.0.1:9/ ; true",
               timeout_s=1, background=True)["content"]["id"]
    h = task(daemon, "c", Identity.OWNER, OWNER_SHELL, {"run_command"})
    assert call(daemon, h, "shell.read", id=pid)["is_error"]                     # not their process


def test_foreground_input_always_yields_to_the_owner(daemon: RunningDaemon) -> None:
    """Owner decision (2026-10-01): whenever an app is brought forward, the owner's input wins — the 2026-09-30
    `[mac] yield_to_owner = false` switch is retired (background input never meets their typing)."""
    d = daemon.daemon
    real_input = d.mac.input                                                  # type: ignore[union-attr]
    d.owner_input.active = lambda: True                                      # type: ignore[method-assign]
    d.config.set("mac", "yield_to_owner", False)
    assert real_input.owner_active() is True


def test_opening_an_app_does_not_restore_a_stale_owner_app(daemon: RunningDaemon, monkeypatch: Any) -> None:
    """The owner may choose the launched app during startup; never reactivate an old foreground app."""
    from tests.mackit import _fake_mac
    _fake_mac(daemon, monkeypatch)
    apps = daemon.daemon.mac.apps                                           # type: ignore[union-attr]
    apps.apps = [RunningApp("com.brave.Browser", "Brave", 12, True)]       # the owner is in Brave

    def launch(bid: str) -> bool:                                           # macOS brings the new app forward
        apps.apps = [RunningApp(a.bundle_id, a.name, a.pid, False) for a in apps.apps] + \
                    [RunningApp(bid, "Notes", 11, True)]
        return True
    apps.launch = launch
    real_activate = apps.activate

    def activate(bid: str) -> bool:
        apps.apps = [RunningApp(a.bundle_id, a.name, a.pid, a.bundle_id == bid) for a in apps.apps]
        return real_activate(bid)
    apps.activate = activate
    g = task(daemon, "o", Identity.OWNER, {"mac:read"}, {"use_app"})          # reading may open apps too
    r = call(daemon, g, "mac.open", bundle_id="com.apple.Notes")
    assert not r["is_error"], r
    assert apps.activated == [] and apps.frontmost().bundle_id == "com.apple.Notes"


def test_shell_run_returns_a_bounded_tail_and_preserves_the_first_page(daemon, monkeypatch) -> None:
    manager = daemon.daemon.shells
    original = manager.read

    async def bounded_read(handle, since=0, limit=None):
        assert limit is not None, 'tool output must not load the whole persisted log'
        return await original(handle, since, limit)

    monkeypatch.setattr(manager, 'read', bounded_read)
    grant = task(daemon, 'long-output', Identity.OWNER, OWNER_SHELL)
    result = call(daemon, grant, 'shell.run', command="i=0; while [ $i -lt 40000 ]; do echo line$i; i=$((i+1)); done")
    assert not result['is_error']
    content = result['content']
    assert content['output_truncated'] and content['output'].rstrip().endswith('line39999')
    assert len(content['output']) <= 8003 and content['total'] > 300000
    first = call(daemon, grant, 'shell.read', id=content['id'], since=0)['content']
    assert first['output'].startswith('line0\n') and first['more']
