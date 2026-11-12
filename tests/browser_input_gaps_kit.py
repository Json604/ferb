"""Kit for the input-gap lab (tests/test_browser_input_gaps_real.py): the lab pages' server (two 127.0.0.1 ports, so a
frame can be cross-origin) and a worker-shaped driver that calls FERB's real browser tools (BrowserSnapshot's view,
BrowserAct, BrowserScroll, BrowserFind, BrowserScreenshot, BrowserPoint) the way a worker does, plus a ground-truth read
of the page's own state (window.labState in the page's main world, through the lab's own debugging pipe, never
FERB's). Every call is timed and kept in `log`, so a test can print exactly what a worker would have seen."""

import http.server
import json
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, urlsplit

from ferb.act.browser.snapshot import for_model
from ferb.act.browser.tools import (
    OWNER_BROWSER,
    BrowserAct,
    BrowserFind,
    BrowserHtml,
    BrowserPoint,
    BrowserScreenshot,
    BrowserScroll,
)
from ferb.core.types import Identity, ToolContext

PAGES = Path(__file__).parent / "browser_input_lab"


class LabServer:
    """Serves tests/browser_input_lab/* on two ports (`a` and `b`: different origins) and records hits on /seen."""

    def __init__(self) -> None:
        self.seen: list[str] = []
        self.servers = [self._start() for _ in range(2)]
        self.a, self.b = (f"http://127.0.0.1:{s.server_port}" for s in self.servers)

    def _start(self) -> http.server.ThreadingHTTPServer:
        lab = self

        class H(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                u = urlsplit(self.path)
                if u.path == "/seen":
                    lab.seen.append(parse_qs(u.query).get("v", [""])[0])
                    body, kind = b"ok", "text/plain"
                elif u.path == "/auth":                        # HTTP basic auth: the browser's own sign-in sheet
                    if self.headers.get("Authorization"):
                        body, kind = b"<title>in</title><p>signed in</p>", "text/html"
                    else:
                        self.send_response(401)
                        self.send_header("WWW-Authenticate", 'Basic realm="lab"')
                        self.end_headers()
                        return
                elif u.path == "/doc.pdf":
                    body, kind = _pdf(), "application/pdf"
                else:
                    f = PAGES / (u.path.strip("/") or "editors.html")
                    if not f.is_file() or PAGES not in f.resolve().parents:
                        self.send_response(404)
                        self.end_headers()
                        return
                    body, kind = f.read_bytes(), "text/html; charset=utf-8"
                self.send_response(200)
                self.send_header("content-type", kind)
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *a: Any) -> None:
                pass
        srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        return srv

    def close(self) -> None:
        for s in self.servers:
            s.shutdown()


def _pdf() -> bytes:
    """A one-page PDF saying "Lab PDF invoice 4242" (hand-built, no library)."""
    stream = b"BT /F1 24 Tf 72 700 Td (Lab PDF invoice 4242) Tj ET"
    objs = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
            b"<< /Length %d >>\nstream\n%s\nendstream" % (len(stream), stream),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    out, offs = b"%PDF-1.4\n", []
    for i, o in enumerate(objs, 1):
        offs.append(len(out))
        out += b"%d 0 obj\n%s\nendobj\n" % (i, o)
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1) + b"".join(b"%010d 00000 n \n" % o for o in offs)
    return out + b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref)


@dataclass
class Worker:
    """One task's view of the lab browser, through FERB's tools as a worker calls them."""
    d: Any                       # tests.conftest.RunningDaemon
    chrome: Any                  # tests.chromelab.ChromeLab
    task: str
    workspace: str
    log: list[dict[str, Any]] = field(default_factory=list)
    tab: str = ""
    snap: Any = None

    @property
    def router(self) -> Any:
        return self.d.daemon.router

    def ctx(self) -> ToolContext:
        return ToolContext(task_id=self.task, worker_id="lab", request_id="lab", identity=Identity.OWNER,
                           scopes=frozenset({"browser:read", "browser:act"}), device_id="lab", span_id=None,
                           workspace=self.workspace, owner_roots=(), untrusted_roots=())

    def run(self, coro: Any, timeout: float = 90) -> Any:
        return self.d.run(coro, timeout=timeout)

    def _note(self, tool: str, args: dict[str, Any], out: Any, t0: float) -> Any:
        self.log.append({"tool": tool, "args": args, "s": round(time.monotonic() - t0, 2),
                         "out": out if isinstance(out, (dict, list, str, int, float, bool)) or out is None else str(out)})
        return out

    def open(self, url: str, wait_ready: float = 0) -> str:
        t0 = time.monotonic()
        ref = self.run(self.router.open_for(url, self.task, "Lab"))
        self.tab = ref.tab_id
        self._note("open", {"url": url}, {"tab": self.tab}, t0)
        if wait_ready:
            end = time.monotonic() + wait_ready
            while time.monotonic() < end:
                st = self.truth()
                if isinstance(st, dict) and st.get("ready"):
                    break
                time.sleep(0.3)
        return self.tab

    def snapshot(self) -> dict[str, Any]:
        """BrowserSnapshot's own result (for_model), and the full element list kept for lookups."""
        t0 = time.monotonic()
        ref = self.router.owned_by(self.tab, self.task)
        self.snap = self.run(self.router.snapshot(ref))
        view = for_model(self.snap)
        self._note("snapshot", {}, {"elements": len(self.snap.elements), "truncated": view.get("truncated"),
                                    "omitted_elements": view.get("omitted_elements")}, t0)
        return view

    def el(self, label: str | None = None, role: str | None = None, contains: bool = False) -> Any:
        """The first element of the latest snapshot with this label (exact, or contained) and role."""
        for e in self.snap.elements:
            if role and e.role != role:
                continue
            if label is None or (label in e.label if contains else e.label == label):
                return e
        return None

    def elements(self, role: str | None = None) -> list[Any]:
        return [e for e in self.snap.elements if role is None or e.role == role]

    def tool(self, cls: Any, **args: Any) -> dict[str, Any]:
        t0 = time.monotonic()
        tool = cls(self.router, OWNER_BROWSER)
        res = self.run(tool.run(tool.Input(tab=self.tab, **args), self.ctx()))
        out = dict(res.content) | ({"is_error": True} if res.is_error else {})
        return self._note(tool.name, args, out, t0)

    def act(self, element: str, action: str, **args: Any) -> dict[str, Any]:
        return self.tool(BrowserAct, element=element, action=action, **args)

    def scroll(self, direction: str = "down", container: str | None = None, **args: Any) -> dict[str, Any]:
        return self.tool(BrowserScroll, direction=direction, container=container, **args)

    def find(self, text: str) -> dict[str, Any]:
        return self.tool(BrowserFind, text=text)

    def html(self, **args: Any) -> dict[str, Any]:
        return self.tool(BrowserHtml, **args)

    def screenshot(self) -> dict[str, Any]:
        t0 = time.monotonic()
        tool = BrowserScreenshot(self.router, OWNER_BROWSER)
        res = self.run(tool.run(tool.Input(tab=self.tab), self.ctx()))
        return self._note("browser.screenshot", {}, dict(res.content), t0)

    def point(self, shot: str, x: float, y: float) -> dict[str, Any]:
        return self.tool(BrowserPoint, shot=shot, x=x, y=y)

    def chrome_id(self) -> int:
        return int(self.router._tabs[self.tab].chrome_id)

    def evaluate(self, js_func: str, all_frames: bool = False) -> Any:
        """Ground truth only: run `js_func` (a function's source) in the page's main world through the lab's pipe."""
        expr = (f"chrome.scripting.executeScript({{target: {{tabId: {self.chrome_id()}, allFrames: {str(all_frames).lower()}}},"
                f" world: 'MAIN', func: {js_func}}}).then(r => r.map(x => x.result))")
        out = self.chrome.evaluate(self.chrome.sw, expr)
        return out if all_frames else (out[0] if out else None)

    def truth(self) -> Any:
        return self.evaluate("() => window.labState ? window.labState() : null")

    def rect_of(self, selector: str) -> list[float]:
        return self.evaluate(f"() => {{ const r = document.querySelector({json.dumps(selector)}).getBoundingClientRect();"
                             " return [r.x + r.width / 2, r.y + r.height / 2, innerWidth, innerHeight]; }")

    def point_at(self, x: float, y: float) -> dict[str, Any]:
        """A worker's look-and-point: a screenshot, then a point at viewport (x, y) scaled to the image."""
        shot = self.screenshot()
        vw, vh = self.evaluate("() => [innerWidth, innerHeight]")
        return self.point(shot["shot"], x * shot["width"] / vw, y * shot["height"] / vh)

    def act_at(self, x: float, y: float, action: str, **args: Any) -> dict[str, Any]:
        """Look, point at viewport (x, y), act on the point's id (a point that resolves nothing is returned as is)."""
        p = self.point_at(x, y)
        return self.act(p["id"], action, **args) if "id" in p else p

    def close(self) -> None:
        if self.tab:
            try:
                self.run(self.router.close(self.router.owned_by(self.tab, self.task), leave_page="leave"))
            except Exception as e:                       # noqa: BLE001 - the lab's own teardown; the task closes it too
                self.log.append({"tool": "close", "args": {}, "s": 0, "out": repr(e)[:200]})
            self.tab = ""


def report(name: str, w: Worker, extra: dict[str, Any] | None = None) -> dict[str, Any]:
    """One probe's record: the worker's calls and what the page's own state was at the end."""
    out = {"probe": name, "calls": w.log, **(extra or {})}
    lines = [f"\n=== {name}"]
    for c in w.log:
        o = c["out"]
        if isinstance(o, dict):
            o = {k: v for k, v in o.items() if k not in ("next",)}
        lines.append(f"  {c['s']:5.2f}s {c['tool']:18s} {short(c['args'], 90)} -> {short(o, 330)}")
    for k, v in (extra or {}).items():
        if k != "results":
            lines.append(f"  {k}: {short(v, 600)}")
    print("\n".join(lines))
    return out


def short(s: Any, n: int = 200) -> str:
    return (s if isinstance(s, str) else json.dumps(s, default=str))[:n]
