"""A lab Chrome for real browser tests: a throwaway `--user-data-dir`, driven over `--remote-debugging-pipe` only to
load the unpacked FERB extension, hand it a pairing token and look at windows. FERB itself never debugs a browser
this way; the owner's Chrome, Brave and profiles are never touched (PLATFORM_BROWSER.md 12: the lab method).

Also here: the native-host wiring for the lab (a standalone copy of ferb/native_host/main.py in a folder Chrome can
read, pointed at the test daemon's socket) and a frontmost-app watcher (NSWorkspace polled every 50 ms, as in the
probes) that records every change, so a test can assert the owner was never moved.
"""

import itertools
import json
import os
import queue
import shutil
import stat
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any

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
        return r.get("result", {}).get("value")

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
    """Every change of the frontmost app (bundle id, pid), polled every 50 ms like the probes in PLATFORM_BROWSER 12."""

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
