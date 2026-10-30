"""Build step 9, real Mac (pytest -m real), on a throwaway probe window (tests/probe_window.py: a red window with a
word, a small blue window floating over its corner):
- the capture is that window only: its size, not the screen's, and none of the blue window on top;
- a click through the real driver (marked real input, the owner-input tap watching) lands where the model would
  aim, and a click at a point the blue window covers is refused before any event;
- for Codex and Claude: a worker reads the probe's word back from `mac_screenshot` alone (images reach the model
  through `ferb mcp`), and the frame is on no disk but FERB's evidence store (V1, ~/.ferb/evidence): not elsewhere
  in FERB's home, nor Claude's or Codex's session folders.
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
from ferb.orchestrator.schemas import WORKER_SCHEMA, WorkerReport
from ferb.orchestrator.tree import TaskRecord
from ferb.policy.targets import Request
from ferb.sense.watchers.owner_input import OwnerInput
from tests.conftest import RunningDaemon
from tests.conftest import run_direct

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


def test_a_worker_reads_the_window_from_the_image_and_the_frame_is_only_in_the_evidence_store(
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
    task = Task("t-v", None, "look", "dev", Identity.OWNER, {"screen"}, "worker", "r-v", [],
                SystemClock().now() + timedelta(minutes=10))

    async def setup() -> str:
        root = d.daemon.spans.start("task", task_id=task.id, input={})
        d.daemon.tasks.add(TaskRecord(task, d.daemon.clock.now(), root.id))
        d.daemon.state.requests.record(Request(task.request_id, "look at the FERB Probe app"))
        return d.daemon.tokens.issue(task.id, "w-v", task.scopes, task.identity, ttl_s=600)
    token = d.run(setup())
    prompt = ("Take a screenshot of the app named 'FERB Probe' with the mac_screenshot tool. Its window shows one "
              "word in large black letters on a red background. Put exactly that word (letters and digits) in your "
              "answer and report status done.")
    out = d.run(run_direct(d, "worker", prompt, ["mac.screenshot"], WORKER_SCHEMA, token, task), timeout=600)
    assert "violation" not in out, out.get("violation")
    assert frames, ("the worker never took a screenshot", out.get("final"), out.get("seen"))
    summary = WorkerReport.model_validate_json(out["final"]).answer
    assert probe.word in summary.upper().replace(" ", ""), summary
    marker = base64.b64encode(frames[0][200:260]).decode()[4:40]           # a slice of the frame's base64
    raw = frames[0][200:260]
    kept = [p for p in (d.daemon.home / "evidence").rglob("*") if p.is_file()]
    assert any(raw in p.read_bytes() for p in kept), "the frame wasn't kept as evidence"
    for root in (d.daemon.home, Path.home() / ".claude" / "projects", Path.home() / ".codex" / "sessions"):
        for p in root.rglob("*") if root.exists() else []:
            try:
                if not p.is_file() or p.stat().st_mtime < started or p in kept:
                    continue
                data = p.read_bytes()
            except OSError:
                continue
            assert raw not in data and marker.encode() not in data, f"the frame is on disk: {p}"



# ---------- browser password privacy, in the disposable lab process only ----------

from tests.chromelab import lab, wait_for  # noqa: E402,F401


def test_the_extension_reports_a_web_password_field_and_the_frame_is_masked(lab: Any) -> None:
    """The lab Chrome's own window is captured without activating any installed browser window; a page with a
    password field is captured with that field's rect painted over (L13), never dropped."""
    import http.server
    import json
    from ferb.act.mac.driver import WebSecrets

    rd, chrome, _ = lab
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
    br = d.hub.bridges[("chrome", "Default")]

    class LabApps:
        def find(self, name: str) -> RunningApp | None:
            return RunningApp("com.google.Chrome", "Chrome Lab", chrome.pid, False) if name == "Chrome Lab" else None

    answers: list[WebSecrets] = []

    async def secrets(browser: str, bounds: Any, title: str = "") -> WebSecrets | None:
        result = await br.call("window_secrets", {"bounds": list(bounds), "title": title}, timeout=4)
        answers.append(WebSecrets.from_reply(result))
        return answers[-1] if result.get("matched") else None

    driver = MacComputerDriver(d.mac.device_id, d.mac.input, apps=LabApps(), web_secrets=secrets)
    created: list[int] = []
    try:
        for path, secret in (("/login", True), ("/plain", False)):
            url = f"http://127.0.0.1:{srv.server_port}{path}"
            window = chrome.evaluate(chrome.sw, "chrome.windows.create(" + json.dumps(
                {"url": url, "focused": False, "state": "normal"}) + ")")
            created.append(window["id"])
            title = "Login probe" if secret else "Plain probe"
            wait_for(lambda: any(title in w["kCGWindowName"] for w in shotm._info_windows(True)
                                if int(w.get("kCGWindowOwnerPID", 0)) == chrome.pid), 10, "lab window did not render")
            answers.clear()
            _, _, jpeg = rd.run(driver.screenshot("Chrome Lab"), timeout=30)
            assert jpeg.startswith(b"\xff\xd8") and answers and all(a.complete for a in answers)
            assert all(len([i for i in a.state.items if i.key.startswith("field|")]) == (1 if secret else 0)
                       for a in answers if a.state is not None), answers
            chrome.evaluate(chrome.sw, f"chrome.windows.remove({created.pop()})")
    finally:
        for window_id in created:
            chrome.evaluate(chrome.sw, f"chrome.windows.remove({window_id})")
        srv.shutdown()

    async def tab_rows() -> list[Any]:
        return [json.loads(r["data_json"]) | {"actor": r["actor"]} for r in d.db.conn.execute(
            "SELECT * FROM trail WHERE source = 'browser_tab'").fetchall()]
    loads = [r for r in rd.run(tab_rows()) if r["host"] == "127.0.0.1"]
    assert loads and all(r["actor"] == "owner" and r["profile"] == "Default" and r["browser"] == "chrome"
                         for r in loads), loads
