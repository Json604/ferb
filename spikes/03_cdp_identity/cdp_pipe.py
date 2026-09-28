"""Chrome launched with --remote-debugging-pipe, and a minimal CDP client over that pipe.

Chrome reads commands on fd 3 and writes responses/events on fd 4, NUL-delimited JSON. No TCP port.
The client never logs payloads (they can carry secrets); `log` records method names only.
"""

import itertools
import json
import os
import queue
import subprocess
import threading
from pathlib import Path

CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"


class CdpError(Exception):
    pass


class ChromePipe:
    def __init__(self, user_data_dir: Path, headless: bool = True, start_url: str = "about:blank",
                 extra_args: list[str] | None = None) -> None:
        to_chrome_r, self._to_chrome = os.pipe()
        self._from_chrome, from_chrome_w = os.pipe()

        def child_fds() -> None:  # runs in the child between fork and exec
            os.dup2(to_chrome_r, 3)
            os.dup2(from_chrome_w, 4)

        args = [CHROME, "--remote-debugging-pipe", f"--user-data-dir={user_data_dir}",
                "--no-first-run", "--no-default-browser-check", *(extra_args or []), start_url]
        if headless:
            args.insert(1, "--headless=new")
        self.proc = subprocess.Popen(args, preexec_fn=child_fds, pass_fds=(3, 4),  # preexec runs before fd cleanup: keep the dup2 targets
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        os.close(to_chrome_r)
        os.close(from_chrome_w)

        self.log: list[str] = []
        self._ids = itertools.count(1)
        self._pending: dict[int, queue.Queue] = {}
        self._lock = threading.Lock()
        self.events: queue.Queue = queue.Queue()
        self.attached: list[str] = []  # flat sessions auto-attached (e.g. cross-origin iframes)
        threading.Thread(target=self._reader, daemon=True).start()

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
                    if msg.get("method") == "Target.attachedToTarget":
                        self.attached.append(msg["params"]["sessionId"])
                    self.events.put(msg)

    def send(self, method: str, params: dict | None = None, session: str | None = None,
             timeout: float = 30) -> dict:
        mid = next(self._ids)
        q: queue.Queue = queue.Queue()
        with self._lock:
            self._pending[mid] = q
        msg: dict = {"id": mid, "method": method, "params": params or {}}
        if session:
            msg["sessionId"] = session
        self.log.append(method)
        os.write(self._to_chrome, json.dumps(msg).encode() + b"\0")
        resp = q.get(timeout=timeout)
        if "error" in resp:
            raise CdpError(f"{method}: {resp['error'].get('message')}")
        return resp.get("result", {})

    def wait_event(self, method: str, session: str | None = None, timeout: float = 30) -> dict:
        while True:
            ev = self.events.get(timeout=timeout)
            if ev.get("method") == method and (session is None or ev.get("sessionId") == session):
                return ev.get("params", {})

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


class Page:
    """One tab, attached with a flat session; cross-origin iframes attach as child sessions."""

    def __init__(self, chrome: ChromePipe) -> None:
        self.chrome = chrome
        target = chrome.send("Target.createTarget", {"url": "about:blank"})["targetId"]
        self.session = chrome.send("Target.attachToTarget", {"targetId": target, "flatten": True})["sessionId"]
        self.send("Page.enable")
        self.send("Target.setAutoAttach", {"autoAttach": True, "waitForDebuggerOnStart": False, "flatten": True})

    def send(self, method: str, params: dict | None = None, session: str | None = None) -> dict:
        return self.chrome.send(method, params, session or self.session)

    def goto(self, url: str) -> None:
        self.send("Page.navigate", {"url": url})
        self.chrome.wait_event("Page.loadEventFired", self.session)

    def frames(self) -> dict[str, tuple[str, dict]]:
        """frame id -> (owning session, frame info incl. securityOrigin) across all attached sessions."""
        out: dict[str, tuple[str, dict]] = {}

        def walk(session: str, node: dict) -> None:
            out[node["frame"]["id"]] = (session, node["frame"])
            for child in node.get("childFrames", []):
                walk(session, child)

        for s in [self.session, *self.chrome.attached]:
            try:
                walk(s, self.send("Page.getFrameTree", session=s)["frameTree"])
            except CdpError:
                pass  # detached child session
        return out

    def eval(self, expr: str, session: str | None = None):
        r = self.send("Runtime.evaluate", {"expression": expr, "returnByValue": True, "awaitPromise": True},
                      session)
        return r.get("result", {}).get("value")
