"""Which launch flag makes navigator.webdriver true? Reads it from a local page, independent of CDP."""

import http.server
import queue
import subprocess
import tempfile
import threading
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

from cdp_pipe import CHROME, ChromePipe

reports: queue.Queue = queue.Queue()


class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        u = urlsplit(self.path)
        if u.path == "/report":
            reports.put(parse_qs(u.query)["wd"][0])
        self.send_response(200)
        self.send_header("content-type", "text/html")
        self.end_headers()
        if u.path == "/probe":
            self.wfile.write(b"<script>fetch('/report?wd=' + navigator.webdriver)</script>")

    def log_message(self, *a):
        pass


srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
threading.Thread(target=srv.serve_forever, daemon=True).start()
PROBE = f"http://127.0.0.1:{srv.server_port}/probe"
BASE = ["--no-first-run", "--no-default-browser-check"]


def plain(extra: list[str]) -> str:
    with tempfile.TemporaryDirectory() as d:
        p = subprocess.Popen([CHROME, f"--user-data-dir={d}", *BASE, *extra, PROBE],
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            return reports.get(timeout=30)
        finally:
            p.terminate()
            p.wait(timeout=15)


def pipe(extra: list[str], attach: bool) -> str:
    with tempfile.TemporaryDirectory() as d:
        c = ChromePipe(Path(d), headless=False, start_url=PROBE, extra_args=extra)
        try:
            first = reports.get(timeout=30)
            if not attach:
                return first
            t = next(t for t in c.send("Target.getTargets")["targetInfos"] if t["url"].startswith(PROBE))
            s = c.send("Target.attachToTarget", {"targetId": t["targetId"], "flatten": True})["sessionId"]
            c.send("Page.reload", session=s)
            return f"{first} before attach, {reports.get(timeout=30)} after attach+reload"
        finally:
            c.close()


for name, fn in [
    ("no debugging flags", lambda: plain([])),
    ("--enable-automation (control)", lambda: plain(["--enable-automation"])),
    ("--remote-debugging-port=0 --remote-debugging-address=127.0.0.1", lambda: plain(
        ["--remote-debugging-port=0", "--remote-debugging-address=127.0.0.1"])),
    ("--remote-debugging-pipe, no CDP commands", lambda: pipe([], attach=False)),
    ("--remote-debugging-pipe, CDP attached", lambda: pipe([], attach=True)),
]:
    print(f"{name:66} webdriver={fn()}", flush=True)
srv.shutdown()
