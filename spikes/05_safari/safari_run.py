"""Spike 5 harness: drive the Safari extension (spike-only HTTP long-poll channel) and the AX + real-input fallback.

The extension runs only in the spare Safari profile where the owner enabled it. The AX fallback acts only after
proving the focused Safari window shows the exact tab the extension opened (URL with a one-time token);
otherwise it refuses (ARCHITECTURE 9.3: ax:* stops if it cannot establish what it is looking at).
"""

import http.server
import itertools
import json
import queue
import threading
import time

import ApplicationServices as AX
import Quartz
from AppKit import NSRunningApplication, NSWorkspace

PORT = 47811  # fixed: baked into ext/background.js
SAFARI = "com.apple.Safari"


class ExtError(Exception):
    pass


class Harness:
    """Commands go out on the extension's next poll; results come back on the poll after."""

    def __init__(self) -> None:
        self.hello: dict | None = None
        self._commands: queue.Queue = queue.Queue()
        self._pending: dict[int, queue.Queue] = {}
        self._ids = itertools.count(1)
        self._hello_seen = threading.Event()
        harness = self

        class H(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                msg = json.loads(self.rfile.read(int(self.headers["content-length"])))
                if msg.get("type") == "hello":
                    harness.hello = msg
                    harness._hello_seen.set()
                elif msg.get("type") in ("result", "error") and (q := harness._pending.pop(msg["id"], None)):
                    q.put(msg)
                try:
                    cmd = harness._commands.get(timeout=20)  # long-poll
                except queue.Empty:
                    cmd = {"type": "none"}
                body = json.dumps(cmd).encode()
                self.send_response(200)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *a):
                pass

        self._srv = http.server.ThreadingHTTPServer(("127.0.0.1", PORT), H)
        threading.Thread(target=self._srv.serve_forever, daemon=True).start()

    def wait_hello(self, timeout: float = 60) -> dict:
        if not self._hello_seen.wait(timeout):
            raise ExtError("extension never connected (Safari open in the spare profile, extension enabled?)")
        return self.hello

    def cmd(self, type_: str, timeout: float = 60, **args) -> dict:
        mid = next(self._ids)
        q: queue.Queue = queue.Queue()
        self._pending[mid] = q
        self._commands.put({"id": mid, "type": type_, "args": args})
        msg = q.get(timeout=timeout)
        if msg["type"] == "error":
            raise ExtError(msg["error"])
        return msg["result"]

    def close(self) -> None:
        self._srv.shutdown()
        self._srv.server_close()


# ---------- Accessibility + real input fallback ----------

def ax_trusted() -> bool:
    return bool(AX.AXIsProcessTrusted())


def _attr(el, name):
    err, val = AX.AXUIElementCopyAttributeValue(el, name, None)
    return val if err == 0 else None


def _walk(el, depth=0, max_depth=40):
    yield el
    if depth < max_depth:
        for child in _attr(el, "AXChildren") or []:
            yield from _walk(child, depth + 1, max_depth)


def safari_pid() -> int:
    apps = NSRunningApplication.runningApplicationsWithBundleIdentifier_(SAFARI)
    if not apps:
        raise ExtError("Safari not running")
    return apps[0].processIdentifier()


def focused_web_area():
    app = AX.AXUIElementCreateApplication(safari_pid())
    win = _attr(app, "AXFocusedWindow")
    if win is None:
        return None
    return next((e for e in _walk(win) if _attr(e, "AXRole") == "AXWebArea"), None)


def web_area_url(area) -> str:
    url = _attr(area, "AXURL")
    return str(url.absoluteString()) if url is not None else ""


def find_text_field(area, description: str):
    for e in _walk(area):
        if _attr(e, "AXRole") in ("AXTextField", "AXComboBox", "AXSearchField"):
            label = " ".join(str(_attr(e, a) or "") for a in ("AXDescription", "AXTitle", "AXPlaceholderValue"))
            if description.lower() in label.lower():
                return e
    return None


def frontmost_is_safari() -> bool:
    app = NSWorkspace.sharedWorkspace().frontmostApplication()
    return app is not None and app.bundleIdentifier() == SAFARI


def type_real(text: str, delay: float = 0.03) -> None:
    """Real keyboard events via CGEvent (goes to whatever has focus: caller must have verified focus)."""
    for ch in text:
        for down in (True, False):
            ev = Quartz.CGEventCreateKeyboardEvent(None, 0, down)
            Quartz.CGEventKeyboardSetUnicodeString(ev, len(ch), ch)
            Quartz.CGEventPost(Quartz.kCGHIDEventTap, ev)
        time.sleep(delay)


def press_return() -> None:
    for down in (True, False):
        Quartz.CGEventPost(Quartz.kCGHIDEventTap, Quartz.CGEventCreateKeyboardEvent(None, 36, down))
