"""A lab Chrome for real browser tests: a throwaway `--user-data-dir`, driven over `--remote-debugging-pipe` only to
load the unpacked FERB extension, hand it a pairing token and look at windows. FERB itself never debugs a browser
this way; the owner's Chrome, Brave and profiles are never touched (ARCHITECTURE.md: browser lab).

Also here: the native-host wiring for the lab (a standalone copy of ferb/native_host/main.py in a folder Chrome can
read, pointed at the test daemon's socket) and a frontmost-app watcher (NSWorkspace polled every 50 ms, as in the
probes) that records every change, so a test can assert the owner was never moved.
"""

import itertools
import http.server
import json
import os
import queue
import shutil
import stat
import subprocess
import sys
import threading
import time
import tempfile
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Iterator
from urllib.parse import parse_qs, urlsplit

from ferb.cli.chat import render
from ferb.cli.client import Client
from tests.conftest import RunningDaemon

import pytest

CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
HOST_NAME = "com.ferb.native_host"


class CdpError(Exception):
    pass


class ChromeLab:
    def __init__(self, user_data_dir: Path, extra_args: list[str] | None = None) -> None:
        to_chrome_r, self._to_chrome = os.pipe()
        self._from_chrome, from_chrome_w = os.pipe()

        def child_fds() -> None:                       # between fork and exec: Chrome reads fd 3, writes fd 4
            os.dup2(to_chrome_r, 3)
            os.dup2(from_chrome_w, 4)
            # dup2(fd, fd) is a no-op: an original os.pipe descriptor may still be close-on-exec.
            os.set_inheritable(3, True)
            os.set_inheritable(4, True)

        args = [CHROME, "--remote-debugging-pipe", f"--user-data-dir={user_data_dir}", "--no-first-run",
                "--no-default-browser-check", "--no-startup-window", "--enable-unsafe-extension-debugging",
                *(extra_args or [])]
        self.proc = subprocess.Popen(args, preexec_fn=child_fds, close_fds=False, stdout=subprocess.DEVNULL,
                                     stderr=subprocess.DEVNULL)
        os.close(to_chrome_r)
        os.close(from_chrome_w)
        self._ids = itertools.count(1)
        self._pending: dict[int, queue.Queue[dict[str, Any]]] = {}
        self._lock = threading.Lock()
        self.events: queue.Queue[dict[str, Any]] = queue.Queue()
        threading.Thread(target=self._reader, daemon=True).start()

    @property
    def pid(self) -> int:
        return self.proc.pid

    def _reader(self) -> None:
        buf = b""
        while chunk := os.read(self._from_chrome, 65536):
            buf += chunk
            while b"\0" in buf:
                raw, buf = buf.split(b"\0", 1)
                msg = json.loads(raw)
                if "id" in msg:
                    with self._lock:
                        q = self._pending.pop(msg["id"], None)
                    if q:
                        q.put(msg)
                else:
                    self.events.put(msg)

    def send(self, method: str, params: dict[str, Any] | None = None, session: str | None = None,
             timeout: float = 30) -> dict[str, Any]:
        mid = next(self._ids)
        q: queue.Queue[dict[str, Any]] = queue.Queue()
        with self._lock:
            self._pending[mid] = q
        msg: dict[str, Any] = {"id": mid, "method": method, "params": params or {}}
        if session:
            msg["sessionId"] = session
        os.write(self._to_chrome, json.dumps(msg).encode() + b"\0")
        resp = q.get(timeout=timeout)
        if "error" in resp:
            raise CdpError(f"{method}: {resp['error'].get('message')}")
        return dict(resp.get("result", {}))

    def load_extension(self, path: Path) -> str:
        return str(self.send("Extensions.loadUnpacked", {"path": str(path)})["id"])

    def targets(self) -> list[dict[str, Any]]:
        return list(self.send("Target.getTargets")["targetInfos"])

    def worker_session(self, ext_id: str) -> str:
        """A flat session on the extension's service worker (lab only: FERB never evaluates in its own worker)."""
        for _ in range(100):
            for t in self.targets():
                if t["type"] == "service_worker" and t["url"].startswith(f"chrome-extension://{ext_id}/"):
                    return str(self.send("Target.attachToTarget", {"targetId": t["targetId"], "flatten": True})["sessionId"])
            time.sleep(0.1)
        raise RuntimeError("the extension's service worker never appeared")

    def evaluate(self, session: str, expression: str) -> Any:
        r = self.send("Runtime.evaluate", {"expression": expression, "returnByValue": True, "awaitPromise": True}, session)
        if "exceptionDetails" in r:
            raise CdpError(str(r["exceptionDetails"]))
        return r.get("result", {}).get("value")

    def owner_tab(self, url: str) -> int:
        """An owner-like tab in this disposable Chrome, never in an installed profile."""
        return int(self.evaluate(self.sw, "chrome.windows.create(" + json.dumps(
            {"url": url, "state": "minimized", "focused": False}) + ").then(w => w.tabs[0].id)"))

    def reconnect(self) -> None:
        """Disconnect this extension's relay only; other profiles and native hosts stay untouched."""
        self.evaluate(self.sw, "port.disconnect(); port = null; ensurePort();")

    def window_state(self, url_contains: str) -> str | None:
        """windowState of the window holding the page whose url contains the text (None: no such page). CDP says
        "normal" for a window that was never shown (probe F0b), so judge by the extension API and the screen too."""
        for t in self.targets():
            if t["type"] == "page" and url_contains in t["url"]:
                return str(self.send("Browser.getWindowForTarget", {"targetId": t["targetId"]})["bounds"].get("windowState"))
        return None

    def ext_window(self, session: str, window_id: int) -> dict[str, Any]:
        """What chrome.windows.get says about that window, asked in the extension's worker (lab only)."""
        return dict(self.evaluate(session, f"chrome.windows.get({int(window_id)}).then((w) => ({{state: w.state, focused: w.focused}}))"))

    def on_screen(self) -> list[tuple[str, int]]:
        """This Chrome's windows that macOS currently shows (name, layer); [] when nothing of it is on screen."""
        import Quartz
        rows = Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionOnScreenOnly, Quartz.kCGNullWindowID) or []
        return [(str(r.get("kCGWindowName", "")), int(r.get("kCGWindowLayer", 0))) for r in rows
                if int(r.get("kCGWindowOwnerPID", 0)) == self.pid]

    def close(self) -> None:
        try:
            self.send("Browser.close", timeout=5)
        except Exception:
            pass
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        os.close(self._to_chrome)


def write_native_host(user_data_dir: Path, sock: Path, ext_id: str, python: str | None = None) -> Path:
    """The lab's native host: ferb/native_host/main.py copied standalone (its only import, the socket path, is
    replaced by an env var) into a folder Chrome can read, run by the interpreter the venv resolves to (outside any
    TCC-protected folder), and registered in <user-data-dir>/NativeMessagingHosts (where a custom data dir looks)."""
    host_dir = user_data_dir / "host"
    host_dir.mkdir(parents=True, exist_ok=True)
    src = (Path(__file__).parents[1] / "ferb" / "native_host" / "main.py").read_text()
    src = src.replace("from ferb.paths import socket_path\n", "")
    src = src.replace("sys.exit(relay(origin, str(socket_path()), sys.stdin.buffer, sys.stdout.buffer))",
                      "sys.exit(relay(origin, os.environ['FERB_SOCK'], sys.stdin.buffer, sys.stdout.buffer))")
    assert "FERB_SOCK" in src
    (host_dir / "ferb_host.py").write_text(src)
    launcher = host_dir / "ferb-native-host"
    launcher.write_text(f'#!/bin/sh\nFERB_SOCK="{sock}" exec "{python or os.path.realpath(sys.executable)}" '
                        f'"{host_dir / "ferb_host.py"}" "$@"\n')
    launcher.chmod(stat.S_IRWXU)
    hosts = user_data_dir / "NativeMessagingHosts"
    hosts.mkdir(parents=True, exist_ok=True)
    (hosts / f"{HOST_NAME}.json").write_text(json.dumps({
        "name": HOST_NAME, "description": "FERB lab native host", "path": str(launcher), "type": "stdio",
        "allowed_origins": [f"chrome-extension://{ext_id}/"]}))
    return launcher


class Frontmost:
    """Every change of the frontmost app (bundle id, pid), polled every 50 ms like the browser probes."""

    def __init__(self) -> None:
        from AppKit import NSWorkspace
        self._ws = NSWorkspace.sharedWorkspace()
        self.changes: list[tuple[float, str, int]] = []
        self.start = self._now()
        self._stop = threading.Event()
        self._t0 = time.monotonic()
        threading.Thread(target=self._poll, daemon=True).start()

    def _now(self) -> tuple[str, int]:
        app = self._ws.frontmostApplication()
        return (str(app.bundleIdentifier()) if app else "", int(app.processIdentifier()) if app else 0)

    def _poll(self) -> None:
        last = self.start
        while not self._stop.is_set():
            cur = self._now()
            if cur != last:
                self.changes.append((round(time.monotonic() - self._t0, 2), *cur))
                last = cur
            time.sleep(0.05)

    def stop(self) -> list[tuple[float, str, int]]:
        self._stop.set()
        return self.changes

    def took_focus(self, pid: int) -> bool:
        return any(p == pid for _, _, p in self.changes)


def chrome_available() -> bool:
    return Path(CHROME).exists() and shutil.which("node") is not None


def wait_for(cond: Any, timeout: float, what: str) -> None:
    end = time.monotonic() + timeout
    while not cond():
        if time.monotonic() > end:
            raise AssertionError(what)
        time.sleep(0.1)


@contextmanager
def browser_lab(*, ferb_identity: bool = False):
    """One isolated daemon, native host, extension, profile and Chrome process per lab."""
    from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID
    from ferb.clock.clock import SystemClock
    from ferb.core.types import Identity
    from tests.conftest import RunningDaemon, without_owner_mac

    if not chrome_available():
        pytest.skip("Google Chrome (and node) are needed for the lab")
    with tempfile.TemporaryDirectory(prefix="ferbl-") as folder:
        base = Path(folder)  # Keep the daemon's Unix socket below sun_path's limit.
        udd = base / "chrome"
        udd.mkdir()
        (udd / "Local State").write_text(json.dumps({"profile": {"info_cache": {
            "Default": {"name": "Lab", "user_name": ""}}, "last_used": "Default"}}))
        d = RunningDaemon(base / "home", clock=SystemClock())
        without_owner_mac(d)
        dm = d.daemon
        dm.browser_data_dirs = {"chrome": udd}
        launched: list[list[str]] = []
        dm.browser_launch = launched.append
        assert dm.router is not None
        dm.router.launch, dm.router.wake = launched.append, None
        dm.router.is_running = lambda b: True
        if ferb_identity:
            dm.config.set("identity", "browser", "chrome")
            dm.config.set("identity", "profile", "Default")
        write_native_host(udd, d.sock, FERB_EXTENSION_ID)
        front, chrome = Frontmost(), None
        try:
            chrome = ChromeLab(udd)
            assert chrome.load_extension(Path(__file__).parents[1] / "extensions" / "chromium") == FERB_EXTENSION_ID
            wait_for(lambda: "chrome" in dm.hub.unpaired.values(), 30, "the extension never said hello")

            async def issue() -> str:
                return dm.pairings.issue_token("chrome", "Default", identity=Identity.FERB if ferb_identity else Identity.OWNER)

            token = d.run(issue())
            chrome.sw = chrome.worker_session(FERB_EXTENSION_ID)
            chrome.evaluate(chrome.sw, f"send('pair', {{token: {json.dumps(token)}}})")
            wait_for(lambda: (br := dm.hub.bridges.get(("chrome", "Default"))) is not None and br.conn is not None,
                     30, "pairing never completed")
            yield d, chrome, front
        finally:
            if chrome:
                chrome.close()
            d.stop()
            front.stop()


@pytest.fixture(scope="module")
def lab():
    with browser_lab() as isolated:
        yield isolated


# Offline extension transport fixtures share the same isolated browser-test kit.

EXT = Path(__file__).parents[1] / "extensions" / "chromium"
SHOP = "https://shop.example"


def _ready(d: Any, launched: Any, name: str = "Work", verbs: set[str] | None = None, tid: str = "x") -> tuple[Any, Any]:
    from tests.browserkit import OWNER, pair, task
    ext = pair(d, launched, name)
    ext.wait_for(lambda: ext.gen is not None)
    g = task(d, tid, OWNER, verbs if verbs is not None else {"fill_form"}, sites=frozenset({"shop.example"}))
    return ext, g



def _node() -> str:
    node = shutil.which("node")
    if node is None:
        pytest.skip("node not installed")
    return node



def worker(script: str) -> dict:
    harness = r'''
const vm = require("vm"), fs = require("fs"), path = require("path");
const noop = new Proxy(function(){}, {get: () => noop});
const state = {s: {seen: {}, order: [], owned: [], windowId: 9}};
const chrome = new Proxy({
  storage: {local: {get: async () => ({ferb: {key: null}}), set: async () => {}},
            session: {get: async () => structuredClone(state), set: async x => Object.assign(state, x)}},
  runtime: new Proxy({getManifest: () => ({version: "test"}), connectNative: () => ({
    postMessage() {}, disconnect() {}, onMessage: noop, onDisconnect: noop})}, {get: (o,k) => o[k] || noop}),
  tabs: new Proxy({get: async () => ({windowId: 9})}, {get: (o,k) => o[k] || noop}),
}, {get: (o,k) => o[k] || noop});
const ctx = vm.createContext({chrome, crypto: globalThis.crypto, btoa, atob, TextEncoder, URL,
  setTimeout, clearTimeout, console, navigator: {}, structuredClone});
ctx.importScripts = f => vm.runInContext(fs.readFileSync(path.join(process.argv[1], f), "utf8"), ctx);
vm.runInContext(fs.readFileSync(path.join(process.argv[1], "background.js"), "utf8").replace(/ensurePort\(\);\s*$/, ''), ctx);
vm.runInContext(process.argv[2], ctx).then(x => { console.log(JSON.stringify(x)); process.exit(0); })
 .catch(e => { console.error(e); process.exit(1); });
'''
    result = subprocess.run([_node(), "-e", harness, str(EXT), script], capture_output=True, text=True, timeout=20)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)




# Model-driven local contacts/current-tab scenarios.

LINK: list[tuple[str, str, str]] = []
PEOPLE = {"one": [("Rahul Verma", "+91 98200 22222", "Gym"), ("Priya Nair", "+91 98300 33333", "Family")],
          "two": [("Rahul Sharma", "+91 98100 11111", "Work"), ("Rahul Verma", "+91 98200 22222", "Gym")]}
PAGE = """<!doctype html><meta charset=utf-8><title>Chat</title>
<h1>Chats</h1><div id=list>%s</div>
<div id=chat hidden><h2 id=who></h2><p id=detail></p>
<button type=button id=call onclick="fetch('/called?who=' + encodeURIComponent(current));
  document.getElementById('status').textContent = 'Calling ' + current">Video call</button>
<p id=status></p></div>
<script>let current = '';
function open_(n, d) { current = n; document.getElementById('chat').hidden = false;
  document.getElementById('who').textContent = n; document.getElementById('detail').textContent = d; }</script>"""
COMMENTS = ("<!doctype html><meta charset=utf-8><title>Video</title><h1>A video</h1><h2>Comments</h2>" +
            "".join(f"<p style='margin:40px 0'>Comment {i}: {'great video' if i < 60 else 'the last one'}</p>"
                    for i in range(1, 61)) +
            "<script>let t=0;addEventListener('scroll',()=>{clearTimeout(t);t=setTimeout(()=>"
            "fetch('/scrolled?y='+Math.round(scrollY)),200)})</script>")
ROW = """<button type=button onclick="open_('{n}', '{d}')">{n} · {d} · {s}</button><br>"""


@pytest.fixture(scope="module")
def site() -> Iterator[tuple[str, list[str]]]:
    called: list[str] = []

    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            u = urlsplit(self.path)
            if u.path == "/called":
                called.append(parse_qs(u.query).get("who", [""])[0])
                body = b"ok"
            elif u.path == "/scrolled":                             # test_tabs_real: how far the owner's tab scrolled
                called.append("scrolled:" + parse_qs(u.query).get("y", ["0"])[0])
                body = b"ok"
            elif u.path == "/comments":
                body = COMMENTS.encode()
            else:
                rows = "".join(ROW.format(n=n, d=d, s=s) for n, d, s in PEOPLE.get(u.path.strip("/"), []))
                body = (PAGE % rows).encode()
            self.send_response(200)
            self.send_header("content-type", "text/html; charset=utf-8")
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a: Any) -> None:
            pass
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{srv.server_port}", called
    srv.shutdown()


@pytest.fixture(scope="module")
def rd(lab: Any) -> Iterator[RunningDaemon]:
    d, _, _ = lab
    d.daemon.config.set("runtimes", "orchestrator", "codex")
    d.daemon.config.set("runtimes", "worker", "codex")
    d.run(_reload(d))
    real_pub = d.daemon.hub.publish

    def pub(topic: str, data: Any) -> None:
        if topic.startswith("browser."):
            LINK.append((time.strftime("%H:%M:%S"), topic, str(data)[:80]))
        real_pub(topic, data)
    d.daemon.hub.publish = pub
    for br in d.daemon.hub.bridges.values():
        br.publish = pub
    d.daemon.supervisor.apps = lambda: [("com.google.Chrome", "Google Chrome")]
    yield d


async def _reload(d: RunningDaemon) -> None:
    d.daemon._load_config()


def chat(d: RunningDaemon, text: str) -> list[dict[str, Any]]:
    c = Client(d.sock, timeout=900)
    try:
        return list(c.stream("chat.send", {"session_id": "s", "text": text}))
    finally:
        c.close()


def problems(d: RunningDaemon, since: str = "") -> list[Any]:
    async def go() -> list[Any]:
        out: list[Any] = [e.data for e in d.daemon._journal().entries() if e.kind in ("task.repair", "chat.failed")]
        out += [(r["input_json"][:300], (r["output_json"] or "")[:200], r["error"]) for r in d.daemon.db.conn.execute(
            "SELECT input_json, output_json, error FROM spans WHERE kind = 'tool.call' AND started_at >= ? ORDER BY started_at",
            (since,))]
        return out + [("links", LINK[-12:])]
    return d.run(go())


def shown(evs: list[dict[str, Any]]) -> str:
    return "\n".join(line for e in evs if (line := render(e)))
