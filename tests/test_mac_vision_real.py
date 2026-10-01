"""Build step 9, real Mac (pytest -m real), on a throwaway probe window (tests/probe_window.py: a red window with a
word, a small blue window floating over its corner):
- the capture is that window only: its size, not the screen's, and none of the blue window on top;
- a click through the real driver (marked real input, the owner-input tap watching) lands where the model would
  aim, and a click at a point the blue window covers is refused before any event;
- for Codex and Claude: a worker reads the probe's word back from `mac_screenshot` alone (images reach the model
  through `ferb mcp`), and the frame is on no disk: FERB's home, Claude's and Codex's session folders.
"""

import asyncio
import base64
import os
import secrets
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from datetime import timedelta
from pathlib import Path
from typing import Any, Iterator

import pytest

from ferb.act.gui_lock import GuiLock
from ferb.act.mac import apps as appsm
from ferb.act.mac import screenshot as shotm
from ferb.act.mac.apps import RunningApp
from ferb.act.mac.driver import MacComputerDriver
from ferb.act.mac.input import RealInput
from ferb.clock.clock import SystemClock
from ferb.core.errors import StaleRef
from ferb.core.types import Identity, Task
from ferb.orchestrator.schemas import WORKER_SCHEMA, WorkerOutcome
from ferb.orchestrator.tree import TaskRecord
from ferb.policy.intent import RequestIntent
from ferb.sense.watchers.owner_input import OwnerInput
from tests.conftest import RunningDaemon
from tests.test_real_runtimes import run_direct

pytestmark = pytest.mark.real
PROBE = Path(__file__).with_name("probe_window.py")
BUNDLE = "local.ferb.probe"
WORDS = ["MANGO", "VIOLET", "PELICAN", "COPPER", "GLACIER", "HARBOR", "LANTERN", "ORCHID"]


class Probe:
    def __init__(self, word: str) -> None:
        self.word, self.clicks = word, []
        self.proc = subprocess.Popen([sys.executable, str(PROBE), word], stdout=subprocess.PIPE, text=True)
        assert self.proc.stdout is not None
        line = self.proc.stdout.readline()
        assert line.startswith("ready"), line
        self.pid = int(line.split()[1])
        threading.Thread(target=self._read, daemon=True).start()
        time.sleep(1.0)

    def _read(self) -> None:
        assert self.proc.stdout is not None
        for line in self.proc.stdout:
            self.clicks.append(line.split()[1:])

    def stop(self) -> None:
        self.proc.terminate()
        self.proc.wait(5)


class ProbeApps:
    """The real app layer, plus the probe (a script has no bundle id of its own)."""

    def __init__(self, probe: Probe) -> None:
        self.me = RunningApp(BUNDLE, "FERB Probe", probe.pid, False)

    def running(self) -> list[RunningApp]:
        return appsm.running() + [self.me]

    def find(self, q: str) -> RunningApp | None:
        return self.me if q.lower() in (BUNDLE, "ferb probe") else appsm.find(q)

    def frontmost(self) -> RunningApp | None:
        from AppKit import NSWorkspace
        f = NSWorkspace.sharedWorkspace().frontmostApplication()
        if f is not None and int(f.processIdentifier()) == self.me.pid:
            return self.me
        return appsm.frontmost()

    def activate(self, bundle_id: str) -> bool:
        if bundle_id != BUNDLE:
            return appsm.activate(bundle_id)
        from AppKit import NSApplicationActivateIgnoringOtherApps, NSRunningApplication
        app = NSRunningApplication.runningApplicationWithProcessIdentifier_(self.me.pid)
        return bool(app and app.activateWithOptions_(NSApplicationActivateIgnoringOtherApps))


@pytest.fixture
def probe() -> Iterator[Probe]:
    p = Probe(secrets.choice(WORDS) + str(secrets.randbelow(90) + 10))
    try:
        yield p
    finally:
        p.stop()


def _pixels(jpeg: bytes) -> tuple[int, int, int, int]:
    """(width, height, red pixels, blue pixels) of the decoded frame."""
    import Quartz
    from Foundation import NSData
    src = Quartz.CGImageSourceCreateWithData(NSData.dataWithBytes_length_(jpeg, len(jpeg)), None)
    img = Quartz.CGImageSourceCreateImageAtIndex(src, 0, None)
    w, h = Quartz.CGImageGetWidth(img), Quartz.CGImageGetHeight(img)
    ctx = Quartz.CGBitmapContextCreate(None, w, h, 8, w * 4, Quartz.CGColorSpaceCreateDeviceRGB(),
                                       Quartz.kCGImageAlphaPremultipliedLast)
    Quartz.CGContextDrawImage(ctx, Quartz.CGRectMake(0, 0, w, h), img)
    raw = bytes(Quartz.CGBitmapContextGetData(ctx).as_buffer(w * h * 4))
    red = blue = 0
    for i in range(0, len(raw), 16):                       # every 4th pixel is plenty
        r, g, b = raw[i], raw[i + 1], raw[i + 2]
        red += r > 180 and g < 90 and b < 90
        blue += b > 180 and r < 90 and g < 90
    return w, h, red, blue


def test_capture_is_the_window_only_and_clicks_land_or_refuse_under_a_covering_window(probe: Probe) -> None:
    oi = OwnerInput(quiet_s=0.5)
    if not oi.start():
        pytest.skip("no event tap (this process needs Accessibility or Input Monitoring)")
    try:
        d = MacComputerDriver("dev", RealInput(oi.active, GuiLock(), yield_after_s=10), excluded=frozenset)
        d.apps = ProbeApps(probe)
        _, shot, jpeg = asyncio.run(d.screenshot("FERB Probe"))
        w, h, red, blue = _pixels(jpeg)
        assert (w, h) == shot.size and abs(w / h - 600 / 400) < 0.01          # the window's shape, not the screen's
        assert shot.bounds == (200.0, 200.0, 600.0, 400.0)
        assert red > 0.6 * (w * h / 4) and blue == 0, (red, blue)            # the blue window on top isn't in it
        k = w / 600
        r = asyncio.run(d.click_at("FERB Probe", shot.id, 450 * k, 100 * k, holder="t"))
        time.sleep(0.5)
        assert r.ok and probe.clicks and probe.clicks[-1][0] == "main", probe.clicks
        x, y = map(float, probe.clicks[-1][1:])
        assert abs(x - 450) <= 2 and abs(y - 100) <= 2, probe.clicks
        n = len(probe.clicks)
        with pytest.raises(StaleRef, match="another window"):
            asyncio.run(d.click_at("FERB Probe", shot.id, 40 * k, 40 * k, holder="t"))   # under the blue window
        time.sleep(0.5)
        assert len(probe.clicks) == n                                          # no event reached either window
    finally:
        oi.stop()


@pytest.fixture(params=["codex", pytest.param("claude", marks=pytest.mark.real_claude)])
def rt_daemon(request: pytest.FixtureRequest) -> Iterator[tuple[str, RunningDaemon, Path]]:
    base = Path(tempfile.mkdtemp(prefix="ferbr-"))
    d = RunningDaemon(base / "home", clock=SystemClock())
    d.daemon.config.set("runtimes", "worker", request.param)
    yield request.param, d, base
    d.stop()
    shutil.rmtree(base, ignore_errors=True)


def test_a_worker_reads_the_window_from_the_image_and_the_frame_is_on_no_disk(
        rt_daemon: tuple[str, RunningDaemon, Path], probe: Probe) -> None:
    runtime, d, base = rt_daemon
    mac = d.daemon.mac
    assert mac is not None
    mac.apps = ProbeApps(probe)
    frames: list[bytes] = []
    shoot = mac.screenshot

    async def recording(name: str) -> Any:
        a, shot, jpeg = await shoot(name)
        frames.append(jpeg)
        return a, shot, jpeg
    mac.screenshot = recording                                             # type: ignore[method-assign]
    started = time.time()
    task = Task("t-v", None, "look", "dev", Identity.OWNER, {"screen"}, "reader", "r-v", [],
                SystemClock().now() + timedelta(minutes=10))

    async def setup() -> str:
        root = d.daemon.spans.start("task", task_id=task.id, input={})
        d.daemon.tasks.add(TaskRecord(task, d.daemon.clock.now(), root.id))
        d.daemon.engine.intents.record(RequestIntent.from_plan(task.request_id, {"use_app"}))
        return d.daemon.tokens.issue(task.id, "w-v", task.scopes, task.identity, ttl_s=600)
    token = d.run(setup())
    prompt = ("Take a screenshot of the app named 'FERB Probe' with the mac_screenshot tool. Its window shows one "
              "word in large black letters on a red background. Put exactly that word (letters and digits) in your "
              "summary and report status done.")
    out = d.run(run_direct(d, "worker", prompt, ["mac.screenshot"], WORKER_SCHEMA, token, task), timeout=600)
    assert "violation" not in out, out.get("violation")
    assert frames, ("the worker never took a screenshot", out.get("final"), out.get("seen"))
    summary = WorkerOutcome.model_validate_json(out["final"]).summary
    assert probe.word in summary.upper().replace(" ", ""), summary
    marker = base64.b64encode(frames[0][200:260]).decode()[4:40]           # a slice of the frame's base64
    raw = frames[0][200:260]
    for root in (d.daemon.home, Path.home() / ".claude" / "projects", Path.home() / ".codex" / "sessions"):
        for p in root.rglob("*") if root.exists() else []:
            try:
                if not p.is_file() or p.stat().st_mtime < started:
                    continue
                data = p.read_bytes()
            except OSError:
                continue
            assert raw not in data and marker.encode() not in data, f"the frame is on disk: {p}"



# ---------- the extension's password check, in the spare profile ----------

from tests.test_ext_real import rd, target  # noqa: E402,F401  (module fixtures: test daemon on the launcher)


def test_the_extension_reports_a_web_password_field_and_the_frame_is_dropped(rd: RunningDaemon,  # noqa: F811
                                                                            target: tuple[str, str]) -> None:  # noqa: F811
    """Chrome exposes no web content to Accessibility, so the check is the extension's (window_password): a local login
    page in the spare profile's front window is refused; a page without a password field is captured."""
    import http.server

    from ferb.act.browser.profiles import discover_all, open_known_profile
    from ferb.cli.client import Client
    from ferb.core.errors import Refused
    browser, name = target
    if browser != "chrome":
        pytest.skip("Chrome only")
    pages = {"/login": b"<!doctype html><title>Login probe</title><input placeholder=User><input type=password>",
             "/plain": b"<!doctype html><title>Plain probe</title><h1>nothing secret here</h1>"}

    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.end_headers()
            self.wfile.write(pages.get(self.path, b""))

        def log_message(self, *a: Any) -> None:
            pass
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    d = rd.daemon
    known = discover_all()
    p = next(p for p in known if p.browser == browser and p.display_name == name)
    c = Client(rd.sock, timeout=90)
    assert c.call("browsers.pair", {"profile": name, "browser": browser, "timeout_s": 60})["paired"]
    c.close()
    assert d.mac is not None and d.hub is not None
    br = d.hub.bridges[(browser, p.folder)]

    async def close_probe_tabs() -> None:
        from ferb.core.errors import FerbError
        for t in (await br.call("list_tabs", {}))["tabs"]:
            if t["host"] == "127.0.0.1":
                try:
                    await br.call("close", {"tab_id": t["tab_id"]})
                except FerbError:
                    return                  # the last tab closed its window and unloaded the profile: done
    try:
        for path, refused in (("/login", True), ("/plain", False)):
            open_known_profile(browser, p.folder, f"http://127.0.0.1:{srv.server_port}{path}", known)
            time.sleep(2.5)
            appsm.activate("com.google.Chrome")
            _raise_titled("com.google.Chrome", "Login probe" if refused else "Plain probe")
            time.sleep(0.5)
            if refused:
                with pytest.raises(Refused, match="password"):
                    rd.run(d.mac.screenshot("com.google.Chrome"), timeout=30)
            else:
                rd.run(d.mac.screenshot("com.google.Chrome"), timeout=30)
    finally:
        srv.shutdown()

    async def tab_rows() -> list[Any]:                                  # the same tabs, in the trail (build step 10)
        import json as _j
        return [_j.loads(r["data_json"]) | {"actor": r["actor"]} for r in d.db.conn.execute(  # type: ignore[union-attr]
            "SELECT * FROM trail WHERE source = 'browser_tab'").fetchall()]
    loads = [r for r in rd.run(tab_rows()) if r["host"] == "127.0.0.1"]
    rd.run(close_probe_tabs(), timeout=30)
    assert loads and all(r["actor"] == "owner" and r["profile"] == p.folder and r["browser"] == "chrome"
                         for r in loads), loads



def _raise_titled(bundle: str, title: str) -> None:
    """Chrome's front window may belong to another profile: raise the test page's own window."""
    from ferb.act.mac import accessibility as ax
    app = appsm.find(bundle)
    assert app is not None
    w = next((w for w in (shotm._window(d) for d in shotm._info_windows(True))
              if w.pid == app.pid and title in w.title), None)
    assert w is not None, f"no {title} window"
    ax.raise_window(app.pid, w.bounds)


def _rgb(jpeg: bytes) -> tuple[int, int, bytes]:
    import Quartz
    from Foundation import NSData
    src = Quartz.CGImageSourceCreateWithData(NSData.dataWithBytes_length_(jpeg, len(jpeg)), None)
    img = Quartz.CGImageSourceCreateImageAtIndex(src, 0, None)
    w, h = Quartz.CGImageGetWidth(img), Quartz.CGImageGetHeight(img)
    ctx = Quartz.CGBitmapContextCreate(None, w, h, 8, w * 4, Quartz.CGColorSpaceCreateDeviceRGB(),
                                       Quartz.kCGImageAlphaPremultipliedLast)
    Quartz.CGContextDrawImage(ctx, Quartz.CGRectMake(0, 0, w, h), img)
    return w, h, bytes(Quartz.CGBitmapContextGetData(ctx).as_buffer(w * h * 4))


def test_a_mark_is_drawn_where_its_control_is(probe: Probe) -> None:
    """The image with the tree (owner request, 2026-10-01): a control's number sits on its box in the real capture."""
    d = MacComputerDriver("dev", RealInput(lambda: False, GuiLock()), excluded=frozenset)
    d.apps = ProbeApps(probe)
    _, shot, plain = asyncio.run(d.screenshot("FERB Probe"))
    _, _, marked = asyncio.run(d.screenshot("FERB Probe", [("7", (450.0, 400.0, 120.0, 60.0))]))   # screen points
    w, h, a = _rgb(plain)
    _, _, b = _rgb(marked)
    k = w / 600

    def changed(x0: float, y0: float, x1: float, y1: float) -> int:
        n = 0
        for y in range(int(y0 * k), int(y1 * k)):
            for x in range(int(x0 * k), int(x1 * k)):
                i = (y * w + x) * 4
                n += sum(abs(a[i + c] - b[i + c]) for c in range(3)) > 60
        return n
    near = changed(245, 195, 375, 265)                     # the box: window coords (450-200, 400-200) ± a few points
    far = changed(420, 300, 560, 380)                      # well away from it
    assert near > 50 and far < 5, (near, far)
