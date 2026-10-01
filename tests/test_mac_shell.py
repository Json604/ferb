"""Build step 7, offline: resource locks (safety test 14), real input that yields to the owner (10.5), the shell
(real PTYs: task ownership, cleanup, jail, refusals, destructive = hard floor, clean env, redacted output), identity
and exposure rules for shell / Mac tools (21.3, 21.8), the AppleScript guard, and the Mac driver's method order on a
fake Accessibility layer (Accessibility first, real input only as the fallback, never into password fields).
"""

import asyncio
import time
from dataclasses import dataclass
from datetime import timedelta
from typing import Any

import pytest

from ferb.act.gui_lock import GuiLock
from ferb.act.locks import ResourceLocks, ordered
from ferb.act.mac import applescript
from ferb.act.mac.accessibility import AxNode, AxTree
from ferb.act.mac.apps import RunningApp
from ferb.act.mac.driver import MacComputerDriver
from ferb.act.mac.input import RealInput, Step, Yielded
from ferb.core.errors import Refused, StaleRef
from ferb.core.types import ComputerAction, Identity, Task
from ferb.orchestrator.tree import TaskRecord
from ferb.policy.intent import RequestIntent
from tests.conftest import RunningDaemon

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

def test_real_input_waits_for_the_owner_then_resumes_or_yields_where_it_stopped() -> None:
    posted: list[str] = []
    clock = {"t": 0.0}

    async def fake_sleep(s: float) -> None:
        clock["t"] += s

    def steps(n: int) -> list[Step]:
        return [Step(f"k{i}", lambda i=i: posted.append(f"k{i}")) for i in range(n)]

    # owner active between t=0.05 and t=1.0: the action pauses after step 2 and resumes
    busy = lambda: 0.05 <= clock["t"] < 1.0                                    # noqa: E731
    ri = RealInput(busy, GuiLock(), yield_after_s=5, sleep=fake_sleep)
    assert asyncio.run(ri.run(steps(5), "w", "typing")) == 5 and len(posted) == 5
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

def task(d: RunningDaemon, tid: str, identity: Identity, scopes: set[str], verbs: set[str] | None,
         extra: set[str] | None = None) -> Any:
    t = Task(tid, None, "g", "dev", identity, scopes, "actor", f"req-{tid}", [],
             d.daemon.clock.now() + timedelta(minutes=10))

    async def go() -> Any:
        root = d.daemon.spans.start("task", task_id=t.id, input={})
        d.daemon.tasks.add(TaskRecord(t, d.daemon.clock.now(), root.id))
        if verbs is not None:
            d.daemon.engine.intents.record(RequestIntent.from_plan(f"req-{tid}", verbs, extra))
        return d.daemon.tokens.validate(d.daemon.tokens.issue(t.id, f"w-{tid}", scopes, identity, ttl_s=600))
    return d.run(go())


def call(d: RunningDaemon, grant: Any, name: str, timeout: float = 30, **args: Any) -> dict[str, Any]:
    return d.run(d.daemon.gateway.call(grant, name, args), timeout=timeout)    # type: ignore[no-any-return]


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


def test_shell_jail_and_refusals(daemon: RunningDaemon, tmp_path: Any) -> None:
    g = task(daemon, "j", Identity.OWNER, OWNER_SHELL, {"run_command"})
    for cmd, cwd, why in (("ls", "/etc", "working folder refused"),
                          ("cat ~/.ferb/config.toml", None, "FERB's own files"),
                          ("security find-generic-password -s ferb -w", None, "Keychain"),
                          ("cat ~/.ssh/id_ed25519", None, "protected location"),
                          ("osascript -e 'beep'", None, "AppleScript")):
        r = call(daemon, g, "shell.run", command=cmd, **({"cwd": cwd} if cwd else {}))
        assert r["is_error"] and why in r["content"]["error"], (cmd, r)
    r = call(daemon, g, "shell.run", command="env")
    assert "FERB_HOME" not in r["content"]["output"] and "FERB_RUN_TOKEN" not in r["content"]["output"]
    r = call(daemon, g, "shell.run", command="echo sk-ant-abcdefghijklmnopqrstuvwx")
    assert "sk-ant-abcdefghijklmnopqrstuvwx" not in str(r)                     # redacted before the worker sees it


def test_destructive_commands_are_permanent_deletion_blocked_or_always_asked(daemon: RunningDaemon, tmp_path: Any) -> None:
    g = task(daemon, "x", Identity.OWNER, OWNER_SHELL, {"run_command"})
    r = call(daemon, g, "shell.run", command="rm -rf build")
    assert r["is_error"] and "'delete_permanent' was not requested" in r["content"]["error"]
    h = task(daemon, "y", Identity.OWNER, OWNER_SHELL, {"run_command"}, {"delete_permanent"})
    from tests.conftest import Answers
    no = Answers(daemon, granted=False)
    try:
        r = call(daemon, h, "shell.run", command="rm -rf build")
        assert r["is_error"] and "declined" in r["content"]["error"]
        assert "can't be undone" in no.asked[0].reason                        # asked, even when requested
    finally:
        no.stop()


def test_shell_and_mac_tools_follow_identity_and_exposure(daemon: RunningDaemon) -> None:
    ferb = task(daemon, "f", Identity.FERB, {"fs:read"}, None)
    assert not [t for t in daemon.daemon.gateway.tools_for(ferb)
                if t["name"].split(".")[0] in ("shell", "mac")]                   # 21.3
    owner = task(daemon, "w", Identity.OWNER, {"shell", "applescript", "mac:read", "mac:act"}, None)
    names = {t["name"] for t in daemon.daemon.gateway.tools_for(owner)}
    assert {"shell.run", "mac.applescript", "mac.act", "mac.snapshot"} <= names
    daemon.run(_expose(daemon, "w-w"))
    names = {t["name"] for t in daemon.daemon.gateway.tools_for(owner)}
    assert not {n for n in names if n.startswith("shell.")} and "mac.applescript" not in names   # 21.8
    r = call(daemon, owner, "shell.run", command="echo hi")
    assert r["is_error"]


async def _expose(d: RunningDaemon, worker: str) -> None:
    d.daemon.engine.exposure.read(worker, "web", "https://evil.example/")


# ---------- AppleScript guard ----------

def test_applescript_stays_inside_one_app() -> None:
    assert applescript.wrap("com.apple.Notes", "get name of every note").startswith('tell application id "com.apple.Notes"')
    for body in ('do shell script "rm -rf ~"', 'tell application "Finder" to delete every file',
                 'tell application "System Events" to keystroke "x"', 'run script "beep"',
                 "«event aevtodoc»", "using terms from application \"Mail\"\nend using terms from"):
        with pytest.raises(Refused):
            applescript.wrap("com.apple.Notes", body)
    with pytest.raises(Refused):
        applescript.wrap('com.apple.Notes" to beep --', "get name")


# ---------- the Mac driver's method order ----------

@dataclass
class FakeAx:
    tree: AxTree
    press_ok: bool = True
    set_ok: bool = True

    def __post_init__(self) -> None:
        self.calls: list[str] = []

    def read_tree(self, pid: int, browser: bool = False) -> AxTree:
        return self.tree

    def press(self, el: Any) -> bool:
        self.calls.append("ax.press")
        return self.press_ok

    def set_value(self, el: Any, text: str) -> bool:
        self.calls.append("ax.set_value")
        return self.set_ok

    def focus(self, el: Any) -> bool:
        self.calls.append("ax.focus")
        return True

    def center(self, n: AxNode) -> tuple[float, float]:
        return 10.0, 10.0


class FakeApps:
    def __init__(self) -> None:
        self.apps = [RunningApp("com.apple.Notes", "Notes", 11, False), RunningApp("com.brave.Browser", "Brave", 12, True),
                     RunningApp("com.secret.Vault", "Vault", 13, False)]
        self.activated: list[str] = []

    def find(self, q: str) -> RunningApp | None:
        return next((a for a in self.apps if q.lower() in (a.bundle_id.lower(), a.name.lower())), None)

    def running(self) -> list[RunningApp]:
        return self.apps

    def frontmost(self) -> RunningApp | None:
        return next(a for a in self.apps if a.active)

    def activate(self, bid: str) -> bool:
        self.activated.append(bid)
        return True


def mac(mp: pytest.MonkeyPatch, press_ok: bool = True) -> tuple[MacComputerDriver, FakeAx, FakeApps, list[str]]:
    nodes = [AxNode("btn", "button", "Save", None, (0, 0, 20, 20), False, True),
             AxNode("pw", "textbox", "Password", "filled", (0, 30, 20, 20), True, True),
             AxNode("title", "textbox", "Title", "draft", (0, 60, 20, 20), False, True)]
    ax, apps, posted = FakeAx(AxTree(nodes, "note text", "My note"), press_ok=press_ok), FakeApps(), []

    async def no_sleep(s: float) -> None:
        return None
    ri = RealInput(lambda: False, GuiLock(), sleep=no_sleep)
    d = MacComputerDriver("dev", ri, excluded=lambda: {"com.secret.Vault"}, ax=ax, apps=apps)
    import ferb.act.mac.driver as drv
    mp.setattr(drv, "click_steps", lambda x, y: [Step("click", lambda: posted.append(f"click {x},{y}"))])
    mp.setattr(drv, "type_steps", lambda t: [Step("type", lambda: posted.append(f"type {t}"))])
    return d, ax, apps, posted


def test_mac_driver_prefers_accessibility_and_falls_back_to_real_input(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, apps, posted = mac(monkeypatch)
    _, out = asyncio.run(d.snapshot("Notes"))
    ids = {e["label"]: e["id"] for e in out["elements"]}
    pw = next(e for e in out["elements"] if e["label"] == "Password")
    assert pw.get("sensitive") and pw.get("filled") and "value" not in pw        # password value never read
    r = asyncio.run(d.act("Notes", ComputerAction("press", ids["Save"])))
    assert r.ok and ax.calls == ["ax.press"] and posted == [] and apps.activated == []   # no focus stolen
    with pytest.raises(Refused, match="password"):
        asyncio.run(d.act("Notes", ComputerAction("set_value", ids["Password"], "hunter2")))
    d2, ax2, apps2, posted2 = mac(monkeypatch, press_ok=False)
    _, out2 = asyncio.run(d2.snapshot("Notes"))
    ids2 = {e["label"]: e["id"] for e in out2["elements"]}
    r = asyncio.run(d2.act("Notes", ComputerAction("press", ids2["Save"])))
    assert r.ok and "real input" in r.detail and posted2 == ["click 10.0,10.0"]
    assert apps2.activated == ["com.apple.Notes", "com.brave.Browser"]         # brought forward, then put back
    asyncio.run(d2.snapshot("Notes"))
    with pytest.raises(StaleRef):
        asyncio.run(d2.act("Notes", ComputerAction("press", ids2["Save"])))     # a newer snapshot supersedes


def test_mac_driver_refuses_excluded_apps_and_acting_in_browsers(monkeypatch: pytest.MonkeyPatch) -> None:
    d, *_ = mac(monkeypatch)
    with pytest.raises(Refused, match="exclusion"):
        asyncio.run(d.snapshot("Vault"))
    asyncio.run(d.snapshot("Brave"))                                             # reading a browser is fine
    with pytest.raises(Refused, match="only through its extension"):
        asyncio.run(d.act("Brave", ComputerAction("key", None, "return")))


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
    assert not daemon.daemon.engine.exposure.state("w-n").exposed                # local commands don't expose
    # a fetch that fails offline still counts: its output could have been anything from the network
    call(daemon, g, "shell.run", command="curl -s --max-time 1 http://127.0.0.1:9/ ; true")
    assert daemon.daemon.engine.exposure.state("w-n").exposed
    names = {t["name"] for t in daemon.daemon.gateway.tools_for(g)}
    assert not {n for n in names if n.startswith("shell.")} and "fs.write" not in names
    r = call(daemon, g, "shell.run", command="echo again")
    assert r["is_error"] and "not available" in r["content"]["error"]


def test_reading_a_background_downloads_output_later_also_exposes(daemon: RunningDaemon) -> None:
    g = task(daemon, "b", Identity.OWNER, OWNER_SHELL, {"run_command"})
    pid = call(daemon, g, "shell.run", command="sleep 2; wget -q -O- http://127.0.0.1:9/ ; true",
               timeout_s=1, background=True)["content"]["id"]
    assert daemon.daemon.engine.exposure.state("w-b").exposed                   # exposed on the first result
    h = task(daemon, "c", Identity.OWNER, OWNER_SHELL, {"run_command"})
    assert call(daemon, h, "shell.read", id=pid)["is_error"]                     # not their process


def test_yielding_to_the_owner_can_be_switched_off(daemon: RunningDaemon) -> None:
    """Owner request (2026-09-30): `[mac] yield_to_owner = false` keeps FERB's real input going while they type."""
    d = daemon.daemon
    real_input = d.mac.input                                                  # type: ignore[union-attr]
    d.owner_input.active = lambda: True                                      # type: ignore[method-assign]
    assert real_input.owner_active() is True
    d.config.set("mac", "yield_to_owner", False)
    assert real_input.owner_active() is False


def test_opening_an_app_takes_the_owner_back_to_where_they_were(daemon: RunningDaemon, monkeypatch: Any) -> None:
    """Owner's report (2026-10-01): opening an app moved them to its desktop. FERB now puts their app back."""
    from tests.test_mac_vision import _fake_mac
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
    g = task(daemon, "o", Identity.OWNER, {"mac:act"}, {"use_app"})
    r = call(daemon, g, "mac.open", bundle_id="com.apple.Notes")
    assert not r["is_error"], r
    assert apps.activated[-1] == "com.brave.Browser" and apps.frontmost().bundle_id == "com.brave.Browser"
