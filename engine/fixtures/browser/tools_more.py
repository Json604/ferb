"""The tool runs entry 28 of rust/JOURNAL.md left unverified, for make.py: tools_more.json.

The same harness as driver_ops.py (the Python tests' scripted fake extension through the real Hub, Router and
FerbIdentity), with:

- browser.pdf on both routes: a tab, and an HTML file shown through the one-time local page (`_serve_once`, bound to
  PAGE_PORT so the signed frames that name its address are the same on the Rust side); the PDF is written for real
  under MORE (ferb/act/fs's `new_file`).
- browser.record with a recorder for the recordings manager and hearing (with and without sound).
- the success paths of browser.pair and browser.handoff (the daemon's `pair_profile` and `handoff`, recorded).
- the `point` of acting tools (L33): the driver's screen rect, then the display it is on (`display_of`, recorded).
- a navigation that became a download, through the real downloads.Downloads wired to the hub as the daemon wires it.

`serve_once` is the one-time page itself: raw requests and the exact bytes it answered.
"""

import asyncio
import base64
import os
import shutil
from pathlib import Path
from typing import Any

import driver_ops as O
import transcripts as T
from transcripts import new_ext, pair

MORE = "/private/tmp/ferb-browser-tools-more"
PAGE_PORT = 47613
HTML = b"<html><head><style>p{color:red}</style></head><body><p>Invented resume</p></body></html>"
DISPLAYS: list[Any] = []


def display_of(rect: Any) -> int | None:
    """The display a rect is on, recorded: display 1 for a rect left of x 300, display 2 up to 1000, else none."""
    O.H.log.append({"seam": ["display_of", list(rect)]})
    x = rect[0]
    return 1 if x < 300 else 2 if x < 1000 else None


class Hearing:
    def __init__(self, h: Any) -> None:
        self.h = h
        self.excluded: set[str] = set()
        self.bundle = "com.google.Chrome"

    def excluded_host(self, host: str) -> bool:
        self.h.log.append({"seam": ["excluded_host", host]})
        return host in self.excluded

    def resolve(self, target: str) -> Any:
        self.h.log.append({"seam": ["resolve_audio", target]})
        return (self.bundle, {"target": target, "pid": 4242})


class Recordings:
    def __init__(self, h: Any) -> None:
        self.h = h
        self.hearing: Hearing | None = Hearing(h)

    async def start(self, task: str, target: dict[str, Any], path: Any, capture: Any, fps: float, audio: Any = None,
                    language: str | None = None, sources: Any = (), closed: Any = None) -> dict[str, Any]:
        self.h.log.append({"seam": ["record_start", task, target, str(path), callable(capture), fps,
                                    list(audio) if audio is not None else None, language,
                                    [list(s) for s in sources], closed is not None]})
        return {"recording": "rec-1", "path": str(path), "fps": fps}


def tcall(h: Any, name: str, tools: dict[str, Any], args: dict[str, Any]) -> None:
    """driver_ops.tcall, plus `point` (the tool's L33 cursor point for this call)."""
    if args["what"] != "point":
        O.tcall(h, name, tools, args)
        return
    h.log.append({"start": {"name": name, "driver": args["family"], "method": "tool", "args": args}})

    async def run() -> None:
        from types import SimpleNamespace
        tool = tools[(args["family"], args["verb"])]
        try:
            p = await tool.point(tool.Input(**args["args"]), SimpleNamespace(task_id=args.get("task", "task-a")))
            value = None if p is None else {"kind": p.kind, "rect": list(p.rect), "display": p.display}
            h.log.append({"ret": [name, {"ok": value}]})
        except Exception as e:                                       # noqa: BLE001
            h.log.append({"ret": [name, {"error": type(e).__name__, "message": str(e), "facts": None,
                                         "timing": None}]})
    h.calls[name] = asyncio.get_running_loop().create_task(run())


async def tools_more_tabs(h: Any, act: Any) -> None:
    from ferb.act.browser import downloads as dl
    from ferb.act.browser import tools as bt
    import tests.fake_extension as fe
    router, ferb = T.drivers(h)
    O.seams(h, router, ferb)
    # the real downloads, as the daemon wires them (areas/browsers.py)
    d = dl.Downloads(Path(MORE) / "downloads", h.db, h.clock.now, lambda section, key, default: default,
                     lambda text: text.replace("SECRET", "[redacted]"),
                     lambda topic, data: h.log.append({"pub": [topic, data]}))
    d.task_tab = h.hub.task_tab
    d.task_root = lambda task: task.split(".", 1)[0]
    d.free_bytes = lambda: 10 ** 15
    d.monotonic = h.clock.monotonic                      # the stall clock: the harness's fake clock, not real time
    from types import SimpleNamespace
    dl.time = SimpleNamespace(monotonic=h.clock.monotonic)              # type: ignore[assignment]
    h.hub.on_download = d.receive
    router.downloads = ferb.downloads = d
    recordings = Recordings(h)
    tools: dict[tuple[str, str], Any] = {}
    for cls in [c for c in vars(bt).values() if isinstance(c, type) and issubclass(c, bt._Base) and c is not bt._Base
                and c.verb]:
        if cls is bt.BrowserRecord:
            tools[("router", cls.verb)] = cls(router, bt.OWNER_BROWSER, recordings)
            continue
        tools[("router", cls.verb)] = cls(router, bt.OWNER_BROWSER)
        tools[("ferb", cls.verb)] = cls(ferb, bt.FERB_BROWSER)

    async def pair_profile(browser: str, folder: str) -> dict[str, Any]:
        h.log.append({"seam": ["pair", browser, folder]})
        if folder == "Profile 8":
            return {"paired": False, "reason": "the owner said No"}
        return {"paired": True, "profile": f"{browser}/{folder}", "name": "Home"}

    async def handoff(ref: Any, message: str, ctx: Any) -> dict[str, Any]:
        import dataclasses
        h.log.append({"seam": ["handoff", dataclasses.asdict(ref), message, ctx.task_id]})
        if "puzzle" in message:
            return {"passed": False, "reason": "the page still shows it", "waited_s": 600}
        return {"passed": True, "waited_s": 12}
    router.pair_profile = pair_profile                                   # type: ignore[attr-defined]
    router.handoff = handoff                                             # type: ignore[attr-defined]
    ext = new_ext(h)

    def navigate(a: dict[str, Any], cid: str) -> dict[str, Any]:
        """The extension holds a download a navigation became and answers at once (the real one answers once FERB
        took it; the harness runs the fake inline, so it can't wait)."""
        if a["url"] in ext.download_urls:
            name, data = ext.download_urls[a["url"]]
            ext.start_download(a["tab_id"], name, data, a["url"])
            old = ext.tabs[a["tab_id"]]
            return {"tab_id": a["tab_id"], "url": old, "title": fe.page(old)["title"]}
        return fe.FakeExtension.op_navigate(ext, a, cid)
    ext.op_navigate = navigate                                           # type: ignore[method-assign]
    await pair(h, act, ext, "chrome", "Profile 1")

    async def t(name: str, family: str, verb: str, what: str, args: dict[str, Any], **kw: Any) -> None:
        a = {"family": family, "verb": verb, "what": what, "args": args, **kw}
        await act({"call": name}, lambda: tcall(h, name, tools, a), wait=name)
    R = "router"
    await t("open", R, "open", "run", {"url": "https://a.example/form"})
    # browser.pdf, a tab
    pdf = base64.b64encode(b"%PDF-1.7 invented tab").decode()
    O.reply(ext, "pdf", {"data": pdf}, {"parts": 2}, {"withheld": "a password field is on the page"})
    O.reply(ext, "pdf_part", {"data": pdf[:8]}, {"data": pdf[8:]})
    await t("pdf-tab", R, "pdf", "run", {"path": f"{MORE}/tab.pdf", "tab": "t1"})
    await t("pdf-tab-parts", R, "pdf", "run", {"path": f"{MORE}/tab-parts.pdf", "tab": "t1", "landscape": True})
    await t("pdf-tab-withheld", R, "pdf", "run", {"path": f"{MORE}/withheld.pdf", "tab": "t1"})
    await t("pdf-other-task", R, "pdf", "run", {"path": f"{MORE}/other.pdf", "tab": "t1"}, ctx={"task": "task-b"})
    # browser.pdf, an HTML file through the one-time local page
    O.reply(ext, "pdf", {"data": base64.b64encode(b"%PDF-1.7 invented html").decode()},
            {"data": base64.b64encode(b"not a pdf").decode()})
    await t("pdf-html", R, "pdf", "run", {"path": f"{MORE}/resume.pdf", "html_path": f"{MORE}/resume.html"})
    await t("pdf-html-not-pdf", R, "pdf", "run", {"path": f"{MORE}/resume-2.pdf", "html_path": f"{MORE}/resume.html"})
    await t("pdf-html-missing", R, "pdf", "run", {"path": f"{MORE}/x.pdf", "html_path": f"{MORE}/missing.html"})
    await t("pdf-html-too-big", R, "pdf", "run", {"path": f"{MORE}/big.pdf", "html_path": f"{MORE}/big.html"})
    # browser.record
    await t("record", R, "record", "run", {"tab": "t1", "path": f"{MORE}/rec.mp4", "fps": 3})
    await t("record-audio-no-scope", R, "record", "run", {"tab": "t1", "path": f"{MORE}/rec2.mp4", "with_audio": True})
    assert recordings.hearing is not None
    recordings.hearing.excluded.add("a.example")
    await t("record-audio-excluded", R, "record", "run", {"tab": "t1", "path": f"{MORE}/rec3.mp4", "with_audio": True},
            ctx={"scopes": ["audio", "browser:read"]})
    recordings.hearing.excluded.clear()
    recordings.hearing.bundle = "com.brave.Browser"
    await t("record-audio-other-app", R, "record", "run", {"tab": "t1", "path": f"{MORE}/rec4.mp4", "with_audio": True},
            ctx={"scopes": ["audio"]})
    recordings.hearing.bundle = "com.google.Chrome"
    await t("record-audio", R, "record", "run", {"tab": "t1", "path": f"{MORE}/rec5.mp4", "with_audio": True,
                                                 "language": "en-US", "fps": 0.5}, ctx={"scopes": ["audio"]})
    recordings.hearing = None
    await t("record-audio-no-hearing", R, "record", "run", {"tab": "t1", "path": f"{MORE}/rec6.mp4",
                                                            "with_audio": True}, ctx={"scopes": ["audio"]})
    # pair and handoff, their success paths and a No
    await t("pair", R, "pair", "run", {"profile": "chrome/Profile 9"})
    await t("pair-no", R, "pair", "run", {"profile": "chrome/Profile 8"})
    await t("pair-unnamed", R, "pair", "run", {"profile": "chrome"})
    await t("handoff", R, "handoff", "run", {"tab": "t1", "message": "Verify you are human"})
    await t("handoff-still", R, "handoff", "run", {"tab": "t1", "message": "Solve the puzzle"})
    # the point of acting tools (L33): a snapshot with the page's geometry and its window
    window = {"id": 9, "minimized": False, "active": True, "bounds": [100, 50, 1280, 900], "title": "Checkout"}
    O.reply(ext, "snapshot", O.snap(
        "https://a.example/form",
        [O.el(0, "button", "Pay", rect=[100, 300, 80, 20], backend_node=5),
         O.el(1, "textbox", "Name", rect=[20, 400, 200, 20], input_type="text"),
         O.el(2, "button", "Far", rect=[500, 300, 80, 20]),
         O.el(3, "button", "Fixed", rect=[10, 10, 50, 20], pinned="fixed"),
         O.el(4, "textbox", "Password", rect=[20, 500, 200, 20], input_type="password", secret=True),
         O.el(5, "button", "Edge", rect=[1000, 300, 80, 20])],
        title="Form", frame={"viewport": [1280, 800], "visual": [0, 0, 1280, 800, 1], "body_scrolls": False,
                             "scroll": {"y": 0, "height": 3000, "viewport_h": 800, "x": 0, "width": 1280,
                                        "at_end": False}},
        window=window))
    await t("snap-geo", R, "snapshot", "run", {"tab": "t1"})
    for n, verb, a in [("point-click", "act", {"element": "s1e1", "action": "click"}),
                       ("point-type", "act", {"element": "s1e2", "action": "type", "text": "Ada"}),
                       ("point-far", "act", {"element": "s1e3", "action": "click"}),
                       ("point-fixed", "act", {"element": "s1e4", "action": "hover"}),
                       ("point-upload", "act", {"element": "s1e2", "action": "upload", "files": ["/x/a.txt"]}),
                       ("point-drag", "act", {"element": "s1e1", "action": "drag", "end_element": "s1e3"}),
                       ("point-press", "act", {"element": "s1e2", "action": "press", "text": "Enter"}),
                       ("point-code", "login_code", {"element": "s1e2"}),
                       ("point-secret", "secret", {"element": "s1e5"}),
                       ("point-edge", "act", {"element": "s1e6", "action": "dblclick"}),
                       ("point-stale", "act", {"element": "s2e1", "action": "click"}),
                       ("point-gone", "act", {"element": "s1e9", "action": "click"})]:
        await t(n, R, verb, "point", {"tab": "t1", **a})
    await t("point-other-task", R, "act", "point", {"tab": "t1", "element": "s1e1", "action": "click"}, task="task-b")
    # a navigation that became a download (D35): the page stays, the result names the file
    url = "https://docs.example/document/d/1/export?format=txt&token=SECRET9"
    ext.download_urls[url] = ("Doc.txt", b"exported text\n" * 50)
    await t("nav-download", R, "navigate", "run", {"tab": "t1", "url": url, "leave_page": "leave"})
    await t("snap-after", R, "snapshot", "run", {"tab": "t1"})
    await t("downloads", R, "downloads", "run", {})


async def serve_cases(reset_random: Any) -> dict[str, Any]:
    """The one-time page itself: each raw request and the exact bytes `_serve_once` answered."""
    from ferb.act.browser import tools as bt
    reset_random()
    url, server = await bt._serve_once(HTML)
    port = int(url.split(":")[2].split("/")[0])
    path = "/" + url.split("/", 3)[3]
    cases = []
    for req in [f"GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: */*\r\n\r\n", "GET /nope.html HTTP/1.1\r\n\r\n",
                f"POST {path} HTTP/1.1\r\n\r\n", f"GET\x1c{path} HTTP/1.1\r\n\r\n", f"GET\t{path}\n\n", "",
                f"GET {path}", "\r\n", f"get {path} HTTP/1.1\r\n\r\n", f"GET {path}?x=1 HTTP/1.1\r\n\r\n",
                f"GET  {path}  HTTP/1.0\r\nX: \xe9\r\n\r\n"]:
        reader, writer = await asyncio.open_connection("127.0.0.1", port)
        writer.write(req.encode("latin-1"))
        await writer.drain()
        writer.write_eof()
        got = await reader.read()
        writer.close()
        cases.append({"request": req, "response": got.decode("latin-1")})
    server.close()
    return {"path": path, "html": HTML.decode(), "cases": cases}


def make(reset_random: Any) -> dict[str, Any]:
    from ferb.act.browser import tools as bt
    shutil.rmtree(MORE, ignore_errors=True)
    os.makedirs(MORE)
    Path(MORE, "resume.html").write_bytes(HTML)
    Path(MORE, "big.html").write_bytes(b"x" * (bt.PDF_HTML_MAX + 1))
    real_server, real_display = asyncio.start_server, bt.display_of

    async def start_server(cb: Any, host: Any = None, port: Any = None, **kw: Any) -> Any:
        return await real_server(cb, host, PAGE_PORT if host == "127.0.0.1" and port == 0 else port, **kw)
    saved = O.SCENARIOS
    O.POLICY["parse"].clear()                                            # only what these runs asked
    O.POLICY["typed"].clear()
    O.SCENARIOS = [("tools_more_tabs", tools_more_tabs)]
    from ferb.act.browser import downloads as dl
    real_time = dl.time
    asyncio.start_server = start_server                                  # type: ignore[assignment]
    bt.display_of = display_of                                           # type: ignore[assignment]
    try:
        out = O.make(reset_random)
        written = {name: base64.b64encode(Path(MORE, name).read_bytes()).decode()
                   for name in sorted(os.listdir(MORE)) if name.endswith(".pdf") or name.startswith("dl-")}
        downloads = sorted(os.listdir(Path(MORE) / "downloads")) if (Path(MORE) / "downloads").exists() else []
    finally:
        O.SCENARIOS = saved
        dl.time = real_time                                              # type: ignore[assignment]
        asyncio.start_server = real_server                               # type: ignore[assignment]
        bt.display_of = real_display                                     # type: ignore[assignment]
    out["note"] = __doc__
    out.update({"more": MORE, "page_port": PAGE_PORT, "html": HTML.decode(), "written": written,
                "downloads": downloads, "serve_once": asyncio.run(serve_cases(reset_random))})
    shutil.rmtree(MORE, ignore_errors=True)
    return out
