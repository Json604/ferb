"""Transcripts of the tab driver's page operations (ferb/act/browser/ext_bridge.py `_TabDriver`, `Router`,
`FerbIdentity`), for make.py: ops.json.

The same harness as transcripts.py (the Python tests' scripted fake extension, imported, its transport replaced by
direct calls), with the daemon's seams recorded as {"seam": [...]} entries: the image library (painting a capture's
secret fields), Accessibility's and the window capture's answers, the downloads records, the upload copies. Upload
files live in a fixed temporary folder (UPLOADS) with fixed bytes and times, so the frames that name them are the same
on every run and on the Rust side; the owner's uid in a receipt is written "$UID".

A {"keepalive": 1} action lets the bridges' keep-awake loops take one more step (Python: their 15 s sleep returns;
Rust: tokio's clock moves 15 s).
"""

import asyncio
import base64
import dataclasses
import os
import shutil
import sys
import types
from pathlib import Path
from typing import Any

import transcripts as T
from transcripts import new_ext, pair, scenario

from ferb.act.browser import ext_bridge
from ferb.act.browser.profiles import BrowserProfile
from ferb.core.types import (ActionResult, BrowserAction, ElementRef, Identity, PageElement, SecretRef, TabRef,
                             TerminalGrant, TerminalInput, WaitCondition)
import tests.fake_extension as fe

UPLOADS = "/private/tmp/ferb-browser-ops-fixture"
MTIME_NS = 1_700_000_000_123_456_789
FILES = {"a.txt": b"hello upload\n", "notes.md": b"# notes\n" * 4, "big.bin": bytes(range(256)) * 1060,
         "secret.txt": b"closed by the lists\n", "empty.dat": b""}
FEED = ("<?xml version=\"1.0\" encoding=\"UTF-8\"?><feed xmlns=\"http://purl.org/atom/ns#\" version=\"0.3\">"
        "<title>Gmail - Inbox for ferb@example.test</title>"
        "<entry><title>Your code</title><summary>Your sign-in code is 482913. It expires soon.</summary>"
        "<id>tag:gmail.google.com,2004:111</id><issued>2026-01-01T09:00:00Z</issued>"
        "<author><name>Example</name><email>no-reply@example.test</email></author></entry>"
        "<entry><title>Two codes 1234 and 99887766</title><summary>see above</summary>"
        "<id>tag:gmail.google.com,2004:222</id><issued>2026-01-01T09:01:00Z</issued>"
        "<author><name>Other</name><email>other@example.test</email></author></entry></feed>")

# ---------- pages the fake extension doesn't have ----------

PAY = {"title": "Checkout", "text": "Total ₹248.75 incl. tax ₹12.00",
       "elements": [{"role": "button", "label": "Pay ₹248.75", "form": "pay", "backend_node": 77,
                     "submit": {"click": True, "enter": False, "action_host": "bank.example", "method": "post",
                                "kinds": ["card_number", "card_csc", "bogus"], "label": "Pay ₹248.75"},
                     "submitted": [{"kinds": ["card_number", "password", "nonsense"], "action_host": "bank.example",
                                    "method": "post"}]},
                    {"role": "textbox", "label": "Card number", "secret": True, "field_kind": "card_number",
                     "input_type": "text", "path": "html>body>form>input:nth-of-type(1)"},
                    {"role": "textbox", "label": "Name on card", "form": "pay", "path": "html>body>form>input:nth-of-type(2)",
                     "submit": {"click": False, "enter": True, "action_host": None, "method": "get",
                                "kinds": [], "label": "Continue"}},
                    {"role": "button", "label": "Pay ₹248.75", "in_view": False, "offscreen": "left",
                     "act_reply": {"ok": False, "detail": "outside_page: the element is outside the page"}},
                    {"role": "link", "label": "Terms", "href": "https://shop.example/terms", "frame": 2}]}
_page = fe.page


def page(url: str) -> dict[str, Any]:
    from urllib.parse import urlsplit
    if urlsplit(url).path == "/pay":
        return PAY
    return _page(url)


# ---------- the daemon's seams, recorded ----------

class Downloads:
    def __init__(self, h: Any) -> None:
        self.h = h

    def busy(self, br: Any, chrome_id: int) -> bool:
        return False

    async def settle(self, br: Any, chrome_id: int, timeout: float = 60) -> None:
        self.h.log.append({"seam": ["settle", br.name, chrome_id, timeout]})

    def unreported(self, br: Any, chrome_id: int) -> list[dict[str, Any]]:
        self.h.log.append({"seam": ["unreported", br.name, chrome_id]})
        return [{"name": "report.pdf", "path": "/ws/downloads/report.pdf"}] if chrome_id == 101 else []

    def facts_for(self, tasks: set[str], identity: str | None = None) -> list[dict[str, Any]]:
        self.h.log.append({"seam": ["facts_for", sorted(tasks), identity]})
        return [{"name": "report.pdf"}]

    def limits(self) -> Any:
        return types.SimpleNamespace(task_bytes=self.h.task_bytes, total_bytes=20 * 1000 ** 3)


class Staging:
    def __init__(self, h: Any) -> None:
        self.h = h

    def stage_all(self, tree: str, sources: Any, tab: str | None = None) -> list[str]:
        self.h.log.append({"seam": ["stage", tree, [[p, s] for _, p, s in sources], tab]})
        return [f"{UPLOADS}/staged/{Path(p).name}" for _, p, _ in sources]

    def closed(self, tab: str) -> bool:
        self.h.log.append({"seam": ["closed", tab]})
        return False


def seams(h: Any, *drivers: Any) -> None:
    async def secrets(pid: int, bounds: Any, title: str) -> Any:
        h.log.append({"seam": ["secrets", pid, list(bounds), title]})
        return None if title == "unreadable" else ([(100.0, 200.0, 50.0, 10.0)], (0.0, 80.0, 1280.0, 820.0), title != "partial")

    async def capture(pid: int, bounds: Any, title: str) -> Any:
        h.log.append({"seam": ["capture", pid, list(bounds), title]})
        return (None, "the window is off screen") if title == "gone" else (b"native-jpeg", "" if title == "plain" else "AX painted 1 field")

    async def place(rect: Any, viewport: Any, pid: int, bounds: Any, title: str) -> Any:
        h.log.append({"seam": ["place", list(rect), list(viewport), pid, list(bounds), title]})
        return (rect[0] + bounds[0], rect[1] + bounds[1] + 80.0, rect[2], rect[3])
    for d in drivers:
        d.native_secrets, d.native_capture, d.native_place = secrets, capture, place
        d.clipboard = lambda: (7, "echo pasted", False)
        d.downloads, d.uploads = Downloads(h), Staging(h)
        d.profiles = lambda: [BrowserProfile("chrome", "Profile 1", "Work", "work@example.test"),
                              BrowserProfile("chrome", "Profile 3", "Home", "")]


def images() -> Any:
    """mask.size / mask_secret_fields and Quartz recorded (the Rust side's Images seam answers the same)."""
    from ferb.act.evidence import mask

    def size(image: bytes) -> tuple[int, int]:
        if image.startswith(b"bad"):
            raise ValueError("not an image")
        return 2560, 1800

    def paint(image: bytes, boxes: list[Any]) -> bytes:
        H.log.append({"seam": ["mask", image.decode(), [list(b) for b in boxes]]})
        return b"painted:" + image
    quartz = types.ModuleType("Quartz")
    quartz.CGImageSourceCreateWithData = lambda data, opts: data                       # type: ignore[attr-defined]
    quartz.CGImageSourceCopyPropertiesAtIndex = lambda img, i, o: {"PixelWidth": 2560, "PixelHeight": 1800}  # type: ignore
    saved = (mask.size, mask.mask_secret_fields, sys.modules.get("Quartz"))
    mask.size, mask.mask_secret_fields = size, paint                                     # type: ignore[assignment]
    sys.modules["Quartz"] = quartz
    return saved


H: Any = None
GATES: list[asyncio.Event] = []
_sleep = asyncio.sleep


async def gated_sleep(delay: float, result: Any = None) -> Any:
    if delay == ext_bridge.KEEPALIVE_S:
        ev = asyncio.Event()
        GATES.append(ev)
        await ev.wait()
        return result
    return await _sleep(delay, result)


def keepalive() -> None:
    while GATES:
        GATES.pop(0).set()


# ---------- calls ----------

def shape(v: Any) -> Any:
    from ferb.act.browser.ext_bridge import OwnerOnlyImage
    from ferb.act.browser.snapshot import for_model
    from ferb.core.types import ActionFacts, PageSnapshot
    if isinstance(v, PageSnapshot):
        return {"snapshot": for_model(v, full=True), "generation": v.generation}
    if isinstance(v, TabRef):
        return {"tab_ref": dataclasses.asdict(v)}
    if isinstance(v, ActionResult):
        return {"action": {"ok": v.ok, "detail": v.detail, "facts": v.facts,
                           "observed": dataclasses.asdict(v.observed) if v.observed is not None else None}}
    if isinstance(v, (PageElement, TerminalInput, ActionFacts)):
        return {type(v).__name__: dataclasses.asdict(v)}
    if isinstance(v, bytes):
        return {"bytes": base64.b64encode(v).decode()}
    if isinstance(v, OwnerOnlyImage):
        return {"owner_only": len(v.jpeg())}
    if isinstance(v, tuple):
        return [shape(x) for x in v]
    if isinstance(v, list):
        return [shape(x) for x in v]
    return v


def uid_free(v: Any) -> Any:
    """The owner's uid in a receipt as "$UID" (it differs from Mac to Mac)."""
    if isinstance(v, dict):
        return {k: ("$UID" if k == "owner_uid" else uid_free(x)) for k, x in v.items()}
    if isinstance(v, list):
        return [uid_free(x) for x in v]
    return v


def action(a: dict[str, Any]) -> BrowserAction:
    g = a.get("terminal")
    grant = TerminalGrant(TerminalInput(**g["input"]), g["confirmation_id"]) if g else None
    closed = a.get("closed")
    check = (lambda p, how: f"{p} is closed by the protection lists" if str(p) in closed else None) \
        if closed is not None else None
    return BrowserAction(a["kind"], a.get("text"), a.get("trusted", False), page_mode=a.get("page_mode", False),
                         expect_dialog=a.get("expect_dialog"), dialog_text=a.get("dialog_text"),
                         files=tuple(a.get("files") or ()), end_element=a.get("end_element"), files_closed=check,
                         terminal=grant, mode=a.get("mode"), html=a.get("html"),
                         modifiers=tuple(a.get("modifiers") or ()))


def ocall(h: Any, name: str, driver: Any, which: str, method: str, args: dict[str, Any]) -> None:
    """A driver call, logged with its arguments (a tab or element by alias, generation and id)."""
    h.log.append({"start": {"name": name, "driver": which, "method": method, "args": args}})

    def ref(a: dict[str, Any]) -> TabRef:
        t = driver._tabs[a["alias"]]
        return TabRef(t.ref.device_id, t.ref.browser, t.ref.profile, a.get("generation", t.ref.generation), a["alias"])

    async def run() -> None:
        try:
            a = dict(args)
            if str(a.get("candidate", "")).startswith("$candidate-"):
                found = getattr(h, "tokens", [])
                i = int(a["candidate"].split("-")[1])
                a["candidate"] = found[i] if i < len(found) else "none"
            tab = ref(a.pop("tab")) if "tab" in a else None
            el = None
            if "el" in a:
                e = a.pop("el")
                el = ElementRef(ref(e), "", e["snap"], e["id"])
            if method == "act":
                value: Any = await driver.act(el, action(a["action"]), a.get("intent", ""))
            elif method == "page_rect":
                value = driver.page_rect(el, action(a["action"]) if a.get("action") else None, a.get("visible", False))
            elif method == "screen_rect":
                value = await driver.screen_rect(el, action(a["action"]) if a.get("action") else None,
                                                 a.get("visible", False))
            elif method == "wait":
                value = await driver.wait(tab, WaitCondition(a["until"], a.get("value")), a["timeout"])
            elif method in ("otp_candidates", "fill_otp"):
                extra = [a["candidate"], a.get("intent", "")] if method == "fill_otp" else []
                value = await getattr(driver, method)(el, SecretRef(a["secret"]), *extra)
            elif method == "fill_secret":
                value = await driver.fill_secret(el, SecretRef(a["secret"]), a.get("intent", ""))
            elif method in ("act_facts", "element_identity", "terminal_input", "owned_by", "task_tabs", "not_connected",
                            "profile_candidates", "downloads_of", "tab_chain", "tab_url", "element_info"):
                value = getattr(driver, method)(**a)
            elif method in ("masking", "timing", "input_note", "ids_note", "download_only", "closes",
                            "typed_since_load", "http_status", "owner_view_supported", "opened_for", "closed_for"):
                value = getattr(driver, method)(tab)
            elif method == "pointable":
                value = driver.pointable(tab, a["shot"])
            elif tab is not None:
                value = await getattr(driver, method)(tab, **a)
            else:
                value = await getattr(driver, method)(**a)
            if method == "visual_snapshot":
                value = [value[0], {"bytes": base64.b64encode(value[1]).decode()}, value[2], value[3]]
            h.log.append({"ret": [name, {"ok": uid_free(shape(value))}]})
        except Exception as e:                                      # noqa: BLE001
            h.log.append({"ret": [name, {"error": type(e).__name__, "message": str(e),
                                         "facts": uid_free(getattr(e, "facts", None)), "timing": None}]})
    h.calls[name] = asyncio.get_running_loop().create_task(run())


def caller(h: Any, act: Any, driver: Any, which: str) -> Any:
    async def c(name: str, method: str, **args: Any) -> None:
        await act({"call": name}, lambda: ocall(h, name, driver, which, method, args), wait=name)
    return c


def reply(ext: Any, op: str, *bodies: dict[str, Any]) -> None:
    """The extension's next answers to `op` (status ok and that value, unless the body names its status)."""
    for b in bodies:
        ext.replies.setdefault(op, []).append(b if "status" in b else {"status": "ok", "value": b})


def snap(url: str, elements: list[dict[str, Any]], **top: Any) -> dict[str, Any]:
    """A snapshot reply with the facts the fake doesn't give (window, viewport, scroll, media, commerce...)."""
    frame = {"frame_id": 0, "gen": 1, "url": url, "title": top.pop("title", "Page"), "elements": elements,
             "text": top.pop("text", "page text"), "truncated": False, **top.pop("frame", {})}
    return {"frames": [frame] + top.pop("frames", []), "dialogs": [], **top}


def el(i: int, role: str, label: str, **kw: Any) -> dict[str, Any]:
    return {"id": i, "role": role, "label": label, "value": "", "rect": kw.pop("rect", [10, 20, 100, 30]),
            "in_view": kw.pop("in_view", True), "input_type": kw.pop("input_type", None), "href": None, "form": None,
            "secret": kw.pop("secret", False), "code_fill": False, **kw}


# ---------- scenarios ----------

async def page_ops_tabs(h: Any, act: Any) -> None:
    router, ferb = T.drivers(h)
    seams(h, router, ferb)
    ext = new_ext(h)
    await pair(h, act, ext, "chrome", "Profile 1")
    c = caller(h, act, router, "router")
    await c("open", "open_for", url="https://a.example/form", owner="task-a")
    await c("snap", "snapshot", tab={"alias": "t1"})
    await c("type", "act", el={"alias": "t1", "snap": 1, "id": "s1e1"},
            action={"kind": "type", "text": "Ada", "mode": "insert"}, intent="i-type")
    await c("typed", "typed_since_load", tab={"alias": "t1"})
    await c("press", "act", el={"alias": "t1", "snap": 1, "id": "s1e1"}, action={"kind": "press", "text": "Enter"})
    await c("facts-enter", "act_facts", tab_id="t1", element_id="s1e1", kind="press", text="enter")
    await c("click-mods", "act", el={"alias": "t1", "snap": 1, "id": "s1e2"},
            action={"kind": "click", "modifiers": ["cmd", "shift", "cmd"], "expect_dialog": "accept",
                    "dialog_text": "yes"})
    await c("identity", "element_identity", tab_id="t1", element_id="s1e2")
    await c("identity-none", "element_identity", tab_id="t1", element_id="s1e9")
    await c("find", "find", tab={"alias": "t1"}, text="Submit", offset=0, max_chars=12000)
    await c("find-page", "find", tab={"alias": "t1"}, text="Submit", offset=5, max_chars=40)
    await c("find-unsaved", "find", tab={"alias": "t1"}, text="Nope", offset=3, max_chars=40)
    await c("html", "html", tab={"alias": "t1"}, element=None, selector=None, frame=None, scripts=False, offset=0,
            max_chars=60)
    await c("html-more", "html", tab={"alias": "t1"}, element=None, selector=None, frame=None, scripts=False,
            offset=60, max_chars=100000)
    await c("html-element", "html", tab={"alias": "t1"}, element="s1e2", selector=None, frame=None, scripts=True,
            offset=0, max_chars=12000)
    await c("html-selector", "html", tab={"alias": "t1"}, element=None, selector="button", frame=0, scripts=False,
            offset=0, max_chars=12000)
    await c("scroll", "scroll", tab={"alias": "t1"}, direction="down")
    await c("scroll-up", "scroll", tab={"alias": "t1"}, direction="up", screens=2.5, to="start")
    await c("scroll-no-container", "scroll", tab={"alias": "t1"}, direction="down", container="c9")
    await c("wait", "wait", tab={"alias": "t1"}, until="text", value="Form", timeout=5)
    await c("shows", "shows", tab={"alias": "t1"}, text="Submit")
    await c("show", "show_tab", tab={"alias": "t1"})
    await c("sound-on", "tab_sound", tab={"alias": "t1"}, on=True)
    await c("sound-off", "tab_sound", tab={"alias": "t1"}, on=False)
    await c("media-none", "media", tab={"alias": "t1"}, action="play", index=None, to=None)
    await c("downloads", "downloads_for", tab={"alias": "t1"}, wait=2.5)
    await c("downloads-of", "downloads_of", task="task-a")
    await c("click-link", "act", el={"alias": "t1", "snap": 1, "id": "s1e3"}, action={"kind": "click"})
    await c("stale-after-nav", "act", el={"alias": "t1", "snap": 1, "id": "s1e1"}, action={"kind": "click"})
    await c("find-after-nav", "find", tab={"alias": "t1"}, text="Home", offset=0, max_chars=12000)
    # scripted replies: a detached act, an unknown op, a stale element, a scroll of another surface
    reply(ext, "act", {"status": "error", "error": "detached:canceled_by_user"})
    await c("snap-2", "snapshot", tab={"alias": "t1"})
    await c("detached", "act", el={"alias": "t1", "snap": 2, "id": "s2e1"}, action={"kind": "click"})
    reply(ext, "act", {"ok": False, "detail": "stale"})
    await c("stale-reply", "act", el={"alias": "t1", "snap": 2, "id": "s2e1"}, action={"kind": "click"})
    reply(ext, "scroll", {"status": "error", "error": "unknown op: scroll", "outcome": "failed"})
    await c("scroll-unknown-op", "scroll", tab={"alias": "t1"}, direction="down")
    reply(ext, "scroll", {"container": "x", "y": 1})
    await c("scroll-elsewhere", "scroll", tab={"alias": "t1"}, direction="right")
    reply(ext, "scroll", {"y": 10.7, "height": "3000", "at_end": 1, "viewport_h": 900.2, "x": 40, "width": 0,
                          "viewport_w": 1280})
    await c("scroll-right", "scroll", tab={"alias": "t1"}, direction="right")
    reply(ext, "wait", {"status": "error", "error": "timeout", "outcome": "unknown"})
    await c("wait-unknown", "wait", tab={"alias": "t1"}, until="load", value=None, timeout=3)
    reply(ext, "find", {"status": "error", "error": "frame gone", "outcome": "failed"})
    await c("shows-unsearchable", "shows", tab={"alias": "t1"}, text="x")
    reply(ext, "mute_tab", {"status": "error", "error": "no tab", "outcome": "failed"}, {"changed": False, "reason": "owner"})
    await c("sound-error", "tab_sound", tab={"alias": "t1"}, on=True)
    await c("sound-unchanged", "tab_sound", tab={"alias": "t1"}, on=True)
    reply(ext, "html", {"nonce": "n1", "frames": [{"frame_id": 0, "error": "blocked"}, {"frame_id": 1, "error": "x" * 600}]})
    await c("html-errors", "html", tab={"alias": "t1"}, element=None, selector=None, frame=None, scripts=False,
            offset=0, max_chars=100)
    reply(ext, "html", {"nonce": "n²", "frames": [{"frame_id": "0", "url": "https://a.example/next", "truncated": 1,
                                                    "html": '<a ferb-el-n²="0">x</a><b ferb-el-n²="7">y</b>'
                                                            '<i ferb-el-n²="">z</i><i ferb-el-n²="1x">'},
                                                   {"frame_id": 2, "undrawn": True}]})
    await c("html-marks", "html", tab={"alias": "t1"}, element=None, selector=None, frame=None, scripts=False,
            offset=0, max_chars=1000)
    await c("closes", "closes", tab={"alias": "t1"})
    await c("shutdown", "shutdown")


async def secret_and_terminal_tabs(h: Any, act: Any) -> None:
    router, ferb = T.drivers(h)
    seams(h, router, ferb)
    ext = new_ext(h)
    await pair(h, act, ext, "chrome", "Profile 1")
    c = caller(h, act, router, "router")
    await c("open", "open_for", url="https://a.example/login", owner="task-a")
    await c("snap", "snapshot", tab={"alias": "t1"})
    pw = {"alias": "t1", "snap": 1, "id": "s1e2"}
    for n, a in [("type-secret", {"kind": "type", "text": "x"}), ("paste-secret", {"kind": "paste", "text": "x"}),
                 ("press-char", {"kind": "press", "text": "a"}), ("press-paste", {"kind": "press", "text": "cmd+v"}),
                 ("press-enter", {"kind": "press", "text": "Enter"}), ("press-tab", {"kind": "press", "text": "Tab"})]:
        await c(n, "act", el=pw, action=a)
    await c("open-term", "open_for", url="https://shell.example/terminal", owner="task-a")
    await c("snap-term", "snapshot", tab={"alias": "t2"})
    term = {"alias": "t2", "snap": 1, "id": "s1e1"}
    for n, kind, text in [("ti-type", "type", "ls -la\n"), ("ti-type-no-enter", "type", "ls"), ("ti-paste-key", "press", "cmd+v"),
                          ("ti-key", "press", "ctrl+c"), ("ti-return", "press", "Return"), ("ti-none", "type", None),
                          ("ti-select", "select", "x")]:
        await c(n, "terminal_input", tab_id="t2", element_id="s1e1", kind=kind, text=text)
    await c("ti-other", "terminal_input", tab_id="t2", element_id="s1e2", kind="type", text="docs")
    await c("ti-drag", "terminal_input", tab_id="t2", element_id="s1e2", kind="drag", text=None, end_element="s1e1")
    await c("term-unasked", "act", el=term, action={"kind": "type", "text": "ls"})
    base = {"app": "the page on shell.example", "bundle_id": "web:shell.example",
            "surface": "a terminal in the page (xterm.js)"}
    await c("term-type", "act", el=term, action={"kind": "type", "text": "ls",
                                                 "terminal": {"input": {**base, "text": "ls", "enter": True},
                                                              "confirmation_id": "q-1"}})
    await c("ti-return-armed", "terminal_input", tab_id="t2", element_id="s1e1", kind="press", text=" RETURN ")
    await c("term-return", "act", el=term, action={"kind": "press", "text": "Return",
                                                   "terminal": {"input": {**base, "keys": "return", "covered_by": "q-1"},
                                                                "confirmation_id": "q-1"}})
    await c("term-return-wrong", "act", el=term, action={"kind": "press", "text": "Return",
                                                         "terminal": {"input": {**base, "keys": "return", "covered_by": "q-1"},
                                                                      "confirmation_id": "q-2"}})
    await c("term-paste", "act", el=term, action={"kind": "press", "text": "cmd+v",
                                                  "terminal": {"input": {**base, "text": "echo pasted", "paste": True,
                                                                         "clipboard": 7}, "confirmation_id": "q-3"}})
    await c("term-paste-new-copy", "act", el=term, action={"kind": "press", "text": "meta-v",
                                                           "terminal": {"input": {**base, "paste": True, "clipboard": 6},
                                                                        "confirmation_id": "q-4"}})
    await c("term-key", "act", el=term, action={"kind": "press", "text": "ctrl+c",
                                                "terminal": {"input": {**base, "keys": "ctrl+c"}, "confirmation_id": "q-5"}})
    await c("term-paste-text", "act", el=term, action={"kind": "paste", "text": "pwd",
                                                       "terminal": {"input": {**base, "text": "pwd"}, "confirmation_id": "q-6"}})


async def pay_tabs(h: Any, act: Any) -> None:
    router, ferb = T.drivers(h)
    seams(h, router, ferb)
    fe.page = page
    try:
        ext = new_ext(h)
        await pair(h, act, ext, "chrome", "Profile 1")
        c = caller(h, act, router, "router")
        await c("open", "open_for", url="https://shop.example/pay", owner="task-a")
        await c("snap", "snapshot", tab={"alias": "t1"})
        for n, eid, kind, text in [("f-click", "s1e1", "click", None), ("f-dbl", "s1e1", "dblclick", None),
                                   ("f-type-card", "s1e2", "type", "4111"), ("f-select", "s1e2", "select", "x"),
                                   ("f-enter", "s1e3", "press", " Return "), ("f-hover", "s1e1", "hover", None),
                                   ("f-frame", "s1e5", "click", None), ("f-none", "s1e9", "click", None)]:
            await c(n, "act_facts", tab_id="t1", element_id=eid, kind=kind, text=text)
        await c("pay", "act", el={"alias": "t1", "snap": 1, "id": "s1e1"}, action={"kind": "click"}, intent="i-pay")
        await c("outside", "act", el={"alias": "t1", "snap": 1, "id": "s1e4"}, action={"kind": "click"})
        reply(ext, "act", {"ok": True, "detail": "typed", "unpointed": "outside"})
        await c("unpointed", "act", el={"alias": "t1", "snap": 1, "id": "s1e4"}, action={"kind": "type", "text": "x"})
        # a scripted snapshot with the page's geometry (L33), commerce, media and a scroller
        window = {"id": 9, "minimized": False, "active": True, "bounds": [100, 50, 1280, 900], "title": "Checkout"}
        reply(ext, "snapshot", snap(
            "https://shop.example/pay",
            [el(0, "button", "Pay $5", rect=[100, 1000, 80, 20], backend_node=5),
             el(1, "button", "Fixed", rect=[10, 10, 50, 20], pinned="fixed"),
             el(2, "button", "Sticky", rect=[10, 10, 50, 20], pinned="sticky"),
             el(3, "textbox", "Boxed", rect=[10, 300, 50, 20], scroll_box=True, input_type="text"),
             el(4, "textbox", "Field", rect=[-20, 300, 200, 20], input_type="text"),
             el(5, "button", "Tiny", rect=[10, 10, 0, 20])],
            title="Checkout", text="Total $5.00 and USD 5",
            frame={"viewport": [1280, 800], "visual": [0, 100, 1280, 800, 1], "body_scrolls": False,
                   "scroll": {"y": 100, "height": 3000, "viewport_h": 800, "x": 0, "width": 1280, "at_end": False},
                   "commerce": {"amount": "$5.00", "merchant": "Shop Example"},
                   "media": [{"index": 0, "id": "m1-abcdef", "playing": False, "paused": True, "duration": 10.5},
                             {"index": 1, "id": "not-an-id"}],
                   "scrollers": [{"id": "c1", "label": "list", "axis": "both", "y": 0, "height": 900, "viewport_h": 300,
                                  "at_end": False, "x": 0, "width": 500, "viewport_w": 200}],
                   "content_type": "text/html"},
            window=window, steps={"script": 12, "ax": 30.5, "bad": "x"}))
        await c("snap-geo", "snapshot", tab={"alias": "t1"})
        await c("timing", "timing", tab={"alias": "t1"})
        await c("f-commerce", "act_facts", tab_id="t1", element_id="s2e2", kind="click", text=None)
        for n, eid, a, vis in [("r-click", "s2e1", {"kind": "click"}, False), ("r-fixed", "s2e2", None, False),
                               ("r-fixed-click", "s2e2", {"kind": "click"}, False), ("r-sticky", "s2e3", None, False),
                               ("r-boxed", "s2e4", {"kind": "type", "text": "x"}, False),
                               ("r-boxed-fill", "s2e4", None, False), ("r-visible", "s2e5", None, True),
                               ("r-type-page", "s2e5", {"kind": "type", "text": "x", "page_mode": True}, True),
                               ("r-tiny", "s2e6", None, False), ("r-drag", "s2e1", {"kind": "drag"}, False)]:
            await c(n, "page_rect", el={"alias": "t1", "snap": 2, "id": eid}, action=a, visible=vis)
        await c("s-click", "screen_rect", el={"alias": "t1", "snap": 2, "id": "s2e1"}, action={"kind": "click"})
        await c("s-fixed", "screen_rect", el={"alias": "t1", "snap": 2, "id": "s2e2"}, action=None, visible=True)
        reply(ext, "media", {"ok": True, "playing": False, "paused": False, "current_time": 0, "muted": False,
                             "volume": 1, "in_view": True, "extra": 1},
              {"ok": True, "muted": False, "error": "NotAllowedError: play() needs a click"},
              {"status": "error", "error": "unknown op: media", "outcome": "failed"})
        await c("media-index", "media", tab={"alias": "t1"}, action="play", index=0, to=None)
        await c("media-id-unmute", "media", tab={"alias": "t1"}, action="unmute", index=None, to=None, media_id="m1-abcdef")
        await c("media-many", "media", tab={"alias": "t1"}, action="pause", index=None, to=None)
        await c("media-no-id", "media", tab={"alias": "t1"}, action="pause", index=None, to=None, media_id="m9-000000")
        await c("media-no-index", "media", tab={"alias": "t1"}, action="seek", index=7, to=3.5)
        await c("media-old", "media", tab={"alias": "t1"}, action="pause", index=1, to=None)
        await c("scroll-container", "scroll", tab={"alias": "t1"}, direction="down", container="c1")
        await c("moved-rect", "page_rect", el={"alias": "t1", "snap": 2, "id": "s2e2"}, action=None, visible=False)
        await c("moved-rect-2", "page_rect", el={"alias": "t1", "snap": 2, "id": "s2e1"}, action=None, visible=False)
    finally:
        fe.page = _page


async def capture_tabs(h: Any, act: Any) -> None:
    router, ferb = T.drivers(h)
    seams(h, router, ferb)
    ext = new_ext(h)
    ext.shot_jpeg = b"jpeg-1"
    await pair(h, act, ext, "chrome", "Profile 1")
    c = caller(h, act, router, "router")
    await c("open", "open_for", url="https://a.example/login", owner="task-a")
    await c("shot-masked", "screenshot", tab={"alias": "t1"})
    await c("masking", "masking", tab={"alias": "t1"})
    await c("visual", "visual_snapshot", tab={"alias": "t1"})
    await c("visual-2", "visual_snapshot", tab={"alias": "t1"})
    await c("ids-note", "ids_note", tab={"alias": "t1"})
    shot = ""
    for entry in reversed(h.log):
        if "ret" in entry and entry["ret"][0] == "visual-2":
            shot = entry["ret"][1]["ok"][0]
            break
    reply(ext, "point", {"element": {"id": 2, "role": "button", "label": "Sign in", "rect": [5, 6, 7, 8],
                                     "in_view": True, "pinned": "fixed"}, "frame_id": 0, "pointer": "p-1"},
          {"element": {"id": 0, "role": "textbox", "label": "User", "rect": [1, 2, 3, 4], "in_view": True},
           "frame_id": 0, "pointer": "p-2"},
          {"error": "nothing there"})
    await c("point", "point", tab={"alias": "t1"}, shot=shot, x=10.5, y=20.0)
    await c("point-2", "point", tab={"alias": "t1"}, shot=shot, x=100.0, y=200.0)
    await c("point-outside", "point", tab={"alias": "t1"}, shot=shot, x=2560.0, y=1.0)
    await c("point-other-shot", "point", tab={"alias": "t1"}, shot="bs-0000", x=1.0, y=1.0)
    await c("point-nothing", "point", tab={"alias": "t1"}, shot=shot, x=1.0, y=1.0)
    await c("pointable", "pointable", tab={"alias": "t1"}, shot=shot)
    reply(ext, "pointer", {"ok": True, "detail": "dragged", "moved": 3})
    await c("drag-one-point", "act", el={"alias": "t1", "snap": 2, "id": "s2e4"}, action={"kind": "drag"})
    await c("drag-page-mode", "act", el={"alias": "t1", "snap": 2, "id": "s2e4"},
            action={"kind": "drag", "end_element": "s2e5", "page_mode": True})
    await c("rect-point", "page_rect", el={"alias": "t1", "snap": 2, "id": "s2e4"}, action={"kind": "click"})
    await c("drag", "act", el={"alias": "t1", "snap": 2, "id": "s2e4"}, action={"kind": "drag", "end_element": "s2e5"},
            intent="i-drag")
    await c("drag-again", "act", el={"alias": "t1", "snap": 2, "id": "s2e4"},
            action={"kind": "drag", "end_element": "s2e5"})
    # scripted pictures: withheld, in parts, unread with and without Accessibility, a debugger-proof tab
    window = {"minimized": False, "active": True, "bounds": [0, 0, 1280, 900], "title": "Login"}
    before = {"top": "doc", "viewport": [0, 0, 1280, 900], "items": [
        {"key": "field|doc|0", "rect": [10, 10, 20, 10], "paint": True},
        {"key": "frame|doc|1", "rect": [0, 0, 5, 5], "paint": False}]}
    data = base64.b64encode(b"jpeg-parts").decode()
    reply(ext, "screenshot",
          {"withheld": "the page is in the middle of loading"},
          {"parts": 2, "size": len(data), "format": "jpeg", "steps": {"capture": 40},
           "mask": {"before": {"top": "d", "viewport": [0, 0, 10, 10], "items": []},
                    "after": {"top": "d", "viewport": [0, 0, 10, 10], "items": []}}},
          {"data": base64.b64encode(b"jpeg-unread").decode(), "unread": "a frame blocked the reader",
           "mask": {"before": before}, "window": window, "input_note": "y" * 400},
          {"data": base64.b64encode(b"jpeg-unread").decode(), "unread": True, "window": {**window, "title": "unreadable"}},
          {"data": base64.b64encode(b"jpeg-unread").decode(), "unread": "x", "window": {**window, "minimized": True}},
          {"data": base64.b64encode(b"jpeg-unread").decode(), "unread": "x", "window": {**window, "emulated": True}},
          {"data": base64.b64encode(b"jpeg-unread").decode(), "unread": "x", "window": {**window, "title": "partial"}},
          {"data": base64.b64encode(b"jpeg-unread").decode(), "unread": "x", "mask": {"before": "nope"}},
          {"data": base64.b64encode(b"bad-unread").decode(), "unread": "x"},
          {"undebuggable": True, "withheld": "the Web Store's pages can't be captured", "window": window},
          {"undebuggable": True, "window": {**window, "title": "plain"}},
          {"undebuggable": True, "window": {**window, "title": "gone"}},
          {"undebuggable": True, "window": {**window, "active": False}},
          {"undebuggable": True, "window": {"minimized": False, "active": True, "title": "nob"}})
    reply(ext, "screenshot_part", {"data": data[:7]}, {"data": data[7:]})
    for i in range(14):
        await c(f"shot-{i}", "screenshot", tab={"alias": "t1"}, looks=i % 2 == 0)
        await c(f"masking-{i}", "masking", tab={"alias": "t1"})
    await c("input-note", "input_note", tab={"alias": "t1"})
    await c("timing", "timing", tab={"alias": "t1"})
    pdf = base64.b64encode(b"%PDF-1.7 invented").decode()
    reply(ext, "pdf", {"data": pdf}, {"withheld": "a password field is on the page"},
          {"parts": 2}, {"data": base64.b64encode(b"<html>").decode()})
    reply(ext, "pdf_part", {"data": pdf[:4]}, {"data": pdf[4:]})
    for n, land in [("pdf", False), ("pdf-withheld", True), ("pdf-parts", False), ("pdf-not-pdf", False)]:
        await c(n, "pdf", tab={"alias": "t1"}, landscape=land)
    await c("owner-view", "owner_view", tab={"alias": "t1"})
    await c("owner-view-supported", "owner_view_supported", tab={"alias": "t1"})
    reply(ext, "owner_view", {"unavailable": "the tab is a Chrome page"}, {"status": "error", "error": "frame gone",
                                                                            "outcome": "failed"}, {})
    for n in ("owner-view-unavailable", "owner-view-error", "owner-view-empty"):
        await c(n, "owner_view", tab={"alias": "t1"})
    # an unreadable snapshot (it timed out): the picture is taken without the page's looks, and no point resolves
    reply(ext, "snapshot", {"status": "error", "error": "timeout", "outcome": "failed", "step": "ax"})
    reply(ext, "screenshot", {"data": base64.b64encode(b"jpeg-stuck").decode(), "unread": "x", "window": window})
    await c("visual-stuck", "visual_snapshot", tab={"alias": "t1"})
    reply(ext, "snapshot", {"status": "error", "error": "frame gone", "outcome": "failed"})
    await c("visual-failed", "visual_snapshot", tab={"alias": "t1"})


def tokens(h: Any) -> None:
    """The candidate tokens of the latest inventory, for the calls that name them ("$candidate-<n>")."""
    for entry in reversed(h.log):
        if "ret" in entry and "ok" in entry["ret"][1] and isinstance(entry["ret"][1]["ok"], dict) \
                and "candidates" in entry["ret"][1]["ok"]:
            h.tokens = [x["candidate"] for x in entry["ret"][1]["ok"]["candidates"]]
            return


async def old_tabs(h: Any, act: Any) -> None:
    """An extension older than paste, held keys, sideways scrolls, markup, PDFs, the owner's view and uploads
    through the page (0.19.2): each is refused with the version, and an upload keeps the debugger's route."""
    router, ferb = T.drivers(h)
    seams(h, router, ferb)
    upload_files()
    try:
        await act({"staging": None}, lambda: setattr(router, "uploads", None))
        ext = new_ext(h, version="0.19.2")
        await pair(h, act, ext, "chrome", "Profile 1")
        c = caller(h, act, router, "router")
        await c("open", "open_for", url="https://a.example/upload", owner="task-a")
        await c("snap", "snapshot", tab={"alias": "t1"})
        e1 = {"alias": "t1", "snap": 1, "id": "s1e1"}
        await c("paste", "act", el=e1, action={"kind": "paste", "text": "x"})
        await c("mods", "act", el=e1, action={"kind": "click", "modifiers": ["shift"]})
        await c("sideways", "scroll", tab={"alias": "t1"}, direction="left")
        await c("screens", "scroll", tab={"alias": "t1"}, direction="down", screens=1.0)
        await c("html", "html", tab={"alias": "t1"}, element=None, selector=None, frame=None, scripts=False, offset=0,
                max_chars=10)
        await c("pdf", "pdf", tab={"alias": "t1"}, landscape=False)
        await c("owner-view", "owner_view_supported", tab={"alias": "t1"})
        await c("upload", "act", el=e1, action={"kind": "upload", "files": [f"{UPLOADS}/a.txt"]}, intent="i-old")
        reply(ext, "upload", {"status": "error", "error": "the browser refused to set the file (no file access)",
                              "outcome": "failed"})
        await c("upload-refused", "act", el=e1, action={"kind": "upload", "files": [f"{UPLOADS}/a.txt"]})
    finally:
        shutil.rmtree(UPLOADS, ignore_errors=True)


def upload_files() -> None:
    shutil.rmtree(UPLOADS, ignore_errors=True)
    os.makedirs(UPLOADS)
    for name, data in FILES.items():
        p = Path(UPLOADS) / name
        p.write_bytes(data)
        os.utime(p, ns=(MTIME_NS, MTIME_NS))
    os.symlink(Path(UPLOADS) / "a.txt", Path(UPLOADS) / "link.txt")


async def uploads_tabs(h: Any, act: Any) -> None:
    router, ferb = T.drivers(h)
    seams(h, router, ferb)
    h.task_bytes = 50 * 1000 ** 3
    upload_files()
    try:
        ext = new_ext(h)
        await pair(h, act, ext, "chrome", "Profile 1")
        c = caller(h, act, router, "router")
        await c("open", "open_for", url="https://a.example/uploads", owner="task-a")
        await c("snap", "snapshot", tab={"alias": "t1"})
        f = {"alias": "t1", "snap": 1, "id": "s1e4"}
        up = lambda *names, **kw: {"kind": "upload", "files": [f"{UPLOADS}/{n}" for n in names], **kw}
        await c("two-files", "act", el=f, action=up("a.txt", "big.bin"), intent="i-up")
        await c("drop", "act", el={"alias": "t1", "snap": 1, "id": "s1e1"}, action=up("notes.md", "empty.dat"))
        await c("ignored-drop", "act", el={"alias": "t1", "snap": 1, "id": "s1e3"}, action=up("a.txt"))
        await c("single-field", "act", el={"alias": "t1", "snap": 1, "id": "s1e2"}, action=up("a.txt", "notes.md"))
        await c("relative", "act", el=f, action={"kind": "upload", "files": ["a.txt"]})
        await c("none", "act", el=f, action={"kind": "upload", "files": []})
        await c("missing", "act", el=f, action=up("nope.txt"))
        await c("link", "act", el=f, action=up("link.txt"))
        await c("closed", "act", el=f, action=up("a.txt", "secret.txt", closed=[f"{UPLOADS}/secret.txt"]))
        await act({"budget": 100}, lambda: setattr(h, "task_bytes", 100))
        await c("over-limit", "act", el=f, action=up("big.bin"))
        await act({"budget": 50 * 1000 ** 3}, lambda: setattr(h, "task_bytes", 50 * 1000 ** 3))
        reply(ext, "upload", {"ok": True, "selected_files": [{"sha256": "00", "size": 13}]})
        await c("mismatch", "act", el=f, action=up("a.txt"))
        reply(ext, "upload", {"ok": True, "detail": "", "route": "field", "files_set": "2"})
        await c("no-proof", "act", el=f, action=up("a.txt"))
        reply(ext, "upload", {"ok": False, "detail": "stale"})
        await c("stale", "act", el=f, action=up("a.txt"))
        reply(ext, "upload_part", {"status": "error", "error": "upload part refused in the page", "outcome": "failed"})
        await c("part-refused", "act", el=f, action=up("a.txt"))
        # the owner allowed file URLs: the browser sets FERB's own copy (rule 17); the switch then goes off
        ext.file_access = True
        await act({"ext": "hello"}, ext.hello)
        await c("snap-2", "snapshot", tab={"alias": "t1"})
        one = {"alias": "t1", "snap": 2, "id": "s2e2"}
        await c("with-access", "act", el=one, action=up("a.txt"), intent="i-acc")
        await c("drop-with-access", "act", el={"alias": "t1", "snap": 2, "id": "s2e1"}, action=up("a.txt"))
        ext.file_access = False
        await c("access-off", "act", el=one, action=up("a.txt"))
        await c("after-off", "act", el=one, action=up("a.txt"))
    finally:
        shutil.rmtree(UPLOADS, ignore_errors=True)


async def otp_tabs(h: Any, act: Any) -> None:
    router, ferb = T.drivers(h)
    seams(h, router, ferb)
    ext = new_ext(h)
    ext.feed_xml = FEED
    await pair(h, act, ext, "chrome", "Profile 2", identity="ferb")
    c = caller(h, act, ferb, "ferb")
    await c("open", "open_for", url="https://a.example/login", owner="task-a")
    await c("snap", "snapshot", tab={"alias": "t1"})
    user = {"alias": "t1", "snap": 1, "id": "s1e1"}
    await c("fill-first", "fill_otp", el=user, secret=T.SECRET_KEY, candidate="nope")
    await c("unknown-account", "otp_candidates", el=user, secret="ferb/other")
    await c("candidates", "otp_candidates", el=user, secret=T.SECRET_KEY)
    await act({"tokens": 1}, lambda: tokens(h))
    await c("fill-bad", "fill_otp", el=user, secret=T.SECRET_KEY, candidate="nope")
    await c("fill", "fill_otp", el=user, secret=T.SECRET_KEY, candidate="$candidate-0", intent="i-otp")
    await c("fill-again", "fill_otp", el=user, secret=T.SECRET_KEY, candidate="$candidate-0")
    await c("candidates-2", "otp_candidates", el=user, secret=T.SECRET_KEY)
    await act({"tokens": 2}, lambda: tokens(h))
    await c("session", "session_ok")
    ext.feed_xml = FEED.replace("ferb@example.test", "someone@example.test")
    await c("session-other", "session_ok")
    await c("release", "release", owner="task-a")


async def rest_and_tabs_tabs(h: Any, act: Any) -> None:
    router, ferb = T.drivers(h)
    seams(h, router, ferb)
    from ferb.act.browser import profiles
    saved = profiles.has_app
    profiles.has_app = lambda bundle_id: True
    try:
        ext = new_ext(h)
        await pair(h, act, ext, "chrome", "Profile 1")
        c = caller(h, act, router, "router")
        await c("tabs-none", "open_tabs", profile=None, task="task-a")
        for t, url in [(102, "https://news.example/today"), (201, "https://video.example/watch?v=1")]:
            ext.tabs[t], ext.tab_windows[t] = url, 7                 # the owner's own window
        ext.tab_active[201] = True
        ext.audible.add(201)
        await c("attach", "attach", tab_ref="chrome:Profile 1:102", owner="task-a")
        await c("rest", "rest", owner="task-a", resting=True)
        await c("unrest", "rest", owner="task-a", resting=False)
        reply(ext, "rest", {"status": "error", "error": "no", "outcome": "failed"}, {"held": True})
        await c("unrest-unconfirmed", "rest", owner="task-a", resting=False)
        await c("open", "open_for", url="https://a.example/form", owner="task-a")
        await c("tabs", "open_tabs", profile=None, task="task-a")
        await c("tabs-named", "open_tabs", profile="Work", task="task-b")
        await c("candidates", "profile_candidates")
        await c("not-connected", "not_connected")
        # the tab cap counts every driver's tabs (HAND-234)
        ferb_ext = new_ext(h)
        await pair(h, act, ferb_ext, "chrome", "Profile 2", identity="ferb")
        await act({"max_tabs": 2}, lambda: [setattr(d, "max_tabs", lambda: 2) for d in (router, ferb)])
        await act({"call": "ferb-open"}, lambda: ocall(h, "ferb-open", ferb, "ferb", "open_for",
                                                       {"url": "https://b.example/", "owner": "task-a"}), wait="ferb-open")
        await c("over-cap", "open_for", url="https://c.example/", owner="task-a")
        # a link and a popup from a task tab (HAND-443, HAND-511), past the cap and within it
        await act({"ext": "link"}, lambda: ext.link_to_new_tab(103, "https://a.example/next"))
        await act({"ext": "popup"}, lambda: ext.popup(103, "https://a.example/popup"))
        await c("opened", "opened_for", tab={"alias": "t2"})
        await act({"max_tabs": 50}, lambda: [setattr(d, "max_tabs", lambda: 50) for d in (router, ferb)])
        await act({"ext": "link-2"}, lambda: ext.link_to_new_tab(103, "https://a.example/next", background=True))
        await act({"ext": "popup-2"}, lambda: ext.popup(103, "https://a.example/popup"))
        await c("opened-2", "opened_for", tab={"alias": "t2"})
        await c("task-tabs", "task_tabs", owner="task-a")
        # a task tab whose page became a download closes once it is in (HAND-511)
        await act({"ext": "download"}, lambda: ext.send("event", {"event": "download", "tab_id": 106, "download_id": "d-9",
                                                                  "name": "x.zip", "blank_tab": True}))
        await c("closed-news", "closed_for", tab={"alias": "t2"})
        # a cleanup that fails twice names the tab by browser, profile, account and title (HAND-419)
        reply(ext, "release_tab", {"status": "error", "error": "boom", "outcome": "failed"},
              {"status": "error", "error": "boom", "outcome": "failed"})
        await c("release", "release", owner="task-a")
    finally:
        profiles.has_app = saved


async def keepalive_tabs(h: Any, act: Any) -> None:
    router, ferb = T.drivers(h)
    seams(h, router, ferb)
    ext = new_ext(h)
    await pair(h, act, ext, "chrome", "Profile 1")
    c = caller(h, act, router, "router")
    await c("open", "open_for", url="https://a.example/form", owner="task-a")
    await act({"keepalive": 1}, keepalive)
    reply(ext, "ping", {"status": "error", "error": "busy", "outcome": "failed"})
    await act({"keepalive": 1}, keepalive)
    await act({"keepalive": 1}, keepalive)
    await c("close", "close", tab={"alias": "t1"})
    await act({"keepalive": 1}, keepalive)
    await act({"keepalive": 1}, keepalive)


async def odd_version_tabs(h: Any, act: Any) -> None:
    """A paired hello whose version Python's int() refuses ("0.19.²"): Python's ValueError ends the connection; the
    Rust hub refuses it as too old and keeps the connection (fixed against Python, named in the Rust test)."""
    ext = new_ext(h)
    await pair(h, act, ext, "chrome", "Profile 1")
    hello = {"version": "0.19.\u00b2", "protocol": 21, "browser": "chrome", "paired": True, "owned": [], "seen": [],
             "armed": [], "tabs": [], "nonce": "n-odd"}
    await act({"ext": "hello-odd-version"}, lambda: ext.send("hello", hello))
    await act({"state": "chrome/Profile 1"}, lambda: h.log.append({"state": T.state(h, "chrome/Profile 1")}))

# ---------- the tool layer (ferb/act/browser/tools.py) ----------

POLICY: dict[str, Any] = {"parse": {}, "typed": {}}


def policy_recorded() -> Any:
    """urls.parse and typed_destination as the tools call them, recorded (ferb-policy's, a seam on the Rust side)."""
    from ferb.act.browser import tools as bt
    from ferb.policy import urls
    real_parse, real_typed = urls.parse, bt.typed_destination

    def parse(url: str) -> Any:
        u = real_parse(url)
        POLICY["parse"][url] = None if u is None else [u.scheme, u.host]
        return u

    def typed(text: str) -> Any:
        found = real_typed(text)
        POLICY["typed"][text] = found
        return found
    urls.parse, bt.typed_destination = parse, typed                       # type: ignore[assignment]
    return real_parse, real_typed


def ctx_of(a: dict[str, Any], identity: Identity) -> Any:
    from ferb.core.types import ToolContext
    return ToolContext(a.get("task", "task-a"), "w", "r", identity, frozenset(a.get("scopes", ())), "dev-1", None,
                       "/tmp/ferb-ws", (), (), intent_id=a.get("intent"), observe_only=a.get("observe_only", False),
                       probe=a.get("probe", False))


def tcall(h: Any, name: str, tools: dict[str, Any], args: dict[str, Any]) -> None:
    """A tool's method: {"family", "verb", "what": run | describe | locks | file_paths | ledger_args |
    terminal_input | preview_target | validate, "args", "ctx"}."""
    h.log.append({"start": {"name": name, "driver": args["family"], "method": "tool", "args": args}})

    async def run() -> None:
        tool = tools[(args["family"], args["verb"])]
        fam = Identity.OWNER if args["family"] == "router" else Identity.FERB
        try:
            if args["what"] == "validate":
                try:
                    tool.Input(**args["args"])
                    value: Any = None
                except Exception as e:                       # noqa: BLE001
                    value = [str(x["ctx"]["error"]) for x in e.errors() if "ctx" in x]   # type: ignore[attr-defined]
                h.log.append({"ret": [name, {"ok": value}]})
                return
            model = tool.Input(**args["args"])
            what = args["what"]
            if what == "run":
                r = await tool.run(model, ctx_of(args.get("ctx", {}), fam))
                value = {"content": uid_free(r.content), "is_error": r.is_error,
                         "read_sources": [list(x) for x in r.read_sources],
                         "image": base64.b64encode(r.image).decode() if r.image is not None else None,
                         "facts": dataclasses.asdict(r.facts) if r.facts is not None else None, "timing": r.timing}
            elif what == "describe":
                value = dataclasses.asdict(tool.describe(model))
            elif what == "locks":
                value = tool.locks(model)
            elif what == "file_paths":
                value = [list(x) for x in tool.file_paths(model)]
            elif what == "ledger_args":
                value = tool.ledger_args(model, args["hashed"])
            elif what == "terminal_input":
                found = await tool.terminal_input(model, "task-a")
                value = dataclasses.asdict(found) if found is not None else None
            else:
                t = tool.preview_target(model, "task-a", args.get("observed"))
                value = None if t is None else {"scope": t.scope, "label": t.label}
            h.log.append({"ret": [name, {"ok": value}]})
        except Exception as e:                                       # noqa: BLE001
            h.log.append({"ret": [name, {"error": type(e).__name__, "message": str(e),
                                         "facts": uid_free(getattr(e, "facts", None)), "timing": None}]})
    h.calls[name] = asyncio.get_running_loop().create_task(run())


async def tools_tabs(h: Any, act: Any) -> None:
    from ferb.act.browser import tools as bt
    router, ferb = T.drivers(h)
    seams(h, router, ferb)
    from ferb.core.types import SecretSpec
    ferb.secrets.specs = lambda: [SecretSpec(SecretRef(T.SECRET_KEY), ["https://a.example"], "ferb@example.test")]
    tools: dict[tuple[str, str], Any] = {}
    for cls in [c for c in vars(bt).values() if isinstance(c, type) and issubclass(c, bt._Base) and c is not bt._Base
                and c.verb]:
        if cls is bt.BrowserRecord:
            continue
        tools[("router", cls.verb)] = cls(router, bt.OWNER_BROWSER)
        tools[("ferb", cls.verb)] = cls(ferb, bt.FERB_BROWSER)
    ext = new_ext(h)
    ext.shot_jpeg = b"jpeg-1"
    await pair(h, act, ext, "chrome", "Profile 1")
    fx = new_ext(h)
    fx.feed_xml = FEED
    await pair(h, act, fx, "chrome", "Profile 2", identity="ferb")

    async def t(name: str, family: str, verb: str, what: str, args: dict[str, Any], **kw: Any) -> None:
        a = {"family": family, "verb": verb, "what": what, "args": args, **kw}
        await act({"call": name}, lambda: tcall(h, name, tools, a), wait=name)
    R, F = "router", "ferb"
    await t("open-bad", R, "open", "run", {"url": "file:///etc/passwd"})
    await t("open-nohost", R, "open", "run", {"url": "http:///x"})
    await t("open-describe", R, "open", "describe", {"url": "https://Shop.Example/pay?x=1"})
    await t("open", R, "open", "run", {"url": "https://a.example/form", "allow_foreground": True},
            ctx={"observe_only": True})
    await t("ferb-open-profile", F, "open", "run", {"url": "https://a.example/", "profile": "Work"})
    await t("ferb-open", F, "open", "run", {"url": "https://a.example/login"})
    await t("snap", R, "snapshot", "run", {"tab": "t2"})
    await t("snap-probe", R, "snapshot", "run", {"tab": "t2"}, ctx={"probe": True})
    await t("snap-other-task", R, "snapshot", "run", {"tab": "t2"}, ctx={"task": "task-b"})
    await t("snap-describe", R, "snapshot", "describe", {"tab": "t2"})
    await t("snap-locks", R, "snapshot", "locks", {"tab": "t2"})
    await t("snap-preview", R, "snapshot", "preview_target", {"tab": "t2"})
    await t("snap-preview-observed", R, "snapshot", "preview_target", {"tab": "t2"}, observed={"x": 1})
    await t("read", R, "read", "run", {"tab": "t2", "offset": 10, "max_chars": 50})
    await t("html", R, "html", "run", {"tab": "t2", "max_chars": 40})
    await t("type", R, "act", "run", {"tab": "t2", "element": "s2e1", "action": "type", "text": "ada@example.test"},
            ctx={"intent": "i-1"})
    await t("type-describe", R, "act", "describe", {"tab": "t2", "element": "s2e1", "action": "type",
                                                     "text": "ada@example.test"})
    await t("paste-describe", R, "act", "describe", {"tab": "t2", "element": "s2e1", "action": "paste",
                                                      "text": "hi", "html": "<b>hi</b>"})
    await t("click-describe", R, "act", "describe", {"tab": "t2", "element": "s2e2", "action": "click"})
    await t("drag-describe", R, "act", "describe", {"tab": "t2", "element": "s2e2", "action": "drag",
                                                     "end_element": "s2e3"})
    await t("click-ledger", R, "act", "ledger_args", {"tab": "t2", "element": "s2e2", "action": "click"},
            hashed={"tab": "t2", "element": "s2e2", "action": "click", "text": None})
    await t("files", R, "act", "file_paths", {"tab": "t2", "element": "s2e2", "action": "upload",
                                               "files": ["/x/a.txt", "rel.txt"]})
    await t("ti-none", R, "act", "terminal_input", {"tab": "t2", "element": "s2e1", "action": "type", "text": "x"})
    for n, a in [("v-type", {"action": "type"}), ("v-mode", {"action": "click", "mode": "insert"}),
                 ("v-html", {"action": "type", "text": "x", "html": "<b>"}),
                 ("v-mods", {"action": "hover", "modifiers": ["cmd"]}), ("v-press", {"action": "press", "text": ""}),
                 ("v-ok", {"action": "click"})]:
        await t(n, R, "act", "validate", {"tab": "t2", "element": "s2e1", **a})
    for n, a in [("v-pdf", {"path": "a.pdf"}), ("v-pdf-ext", {"path": "a.png", "tab": "t2"}),
                 ("v-pdf-ok", {"path": "a.PDF", "tab": "t2"})]:
        await t(n, R, "pdf", "validate", a)
    for n, a in [("v-seek", {"tab": "t2", "action": "seek"}),
                 ("v-both", {"tab": "t2", "action": "play", "index": 0, "media_id": "m1-aaaaaa"})]:
        await t(n, R, "media", "validate", a)
    await t("leave-unsaid", R, "navigate", "run", {"tab": "t2", "url": "https://a.example/next"})
    await t("close-unsaid", R, "close", "run", {"tab": "t2"})
    await t("navigate", R, "navigate", "run", {"tab": "t2", "url": "https://a.example/next", "leave_page": "leave"})
    await t("navigate-observe", R, "navigate", "run", {"tab": "t2", "url": "https://a.example/next"},
            ctx={"observe_only": True, "task": "task-b"})
    await t("snap-2", R, "snapshot", "run", {"tab": "t2"})
    await t("find", R, "find", "run", {"tab": "t2", "text": "Home"})
    await t("scroll", R, "scroll", "run", {"tab": "t2", "direction": "down"})
    await t("wait", R, "wait", "run", {"tab": "t2", "until": "load"})
    await t("dialog", R, "dialog", "run", {"tab": "t2"})
    await t("dialog-describe", R, "dialog", "describe", {"tab": "t2", "decision": "accept"})
    await t("media", R, "media", "run", {"tab": "t2", "action": "play"})
    await t("media-describe", R, "media", "describe", {"tab": "t2", "action": "seek", "to": 3})
    await t("downloads", R, "downloads", "run", {})
    await t("profiles", R, "profiles", "run", {})
    await t("tabs", R, "tabs", "run", {}, ctx={"probe": True})
    await t("use-tab", R, "use_tab", "run", {"tab_ref": "chrome:Profile 1:999"})
    await t("screenshot", R, "screenshot", "run", {"tab": "t2"})
    await t("click", R, "act", "run", {"tab": "t2", "element": "s3e1", "action": "click"})
    await t("keep", R, "keep_open", "run", {"tab": "t2"})
    await t("pair-describe", R, "pair", "describe", {"profile": "chrome/Profile 9"})
    await t("handoff", R, "handoff", "run", {"tab": "t2", "message": "Verify you are human"})
    await t("pair", R, "pair", "run", {"profile": "chrome/Profile 9"})
    await t("pdf-files", R, "pdf", "file_paths", {"path": "/x/a.pdf", "html_path": "/x/a.html"})
    await t("pdf-locks", R, "pdf", "locks", {"path": "/x/a.pdf", "html_path": "/x/a.html"})
    await t("pdf-describe", R, "pdf", "describe", {"path": "/x/a.pdf", "tab": "t2"})
    # FERB's own profile: the sealed fills
    await t("f-snap", F, "snapshot", "run", {"tab": "t1"})
    await t("fill-describe", F, "fill_secret", "describe", {"tab": "t1", "element": "s1e2", "secret": T.SECRET_KEY})
    await t("fill", F, "fill_secret", "run", {"tab": "t1", "element": "s1e2", "secret": T.SECRET_KEY},
            ctx={"intent": "i-fill"})
    await t("fill-unknown", F, "fill_secret", "run", {"tab": "t1", "element": "s1e2", "secret": "ferb/other"})
    await t("otp", F, "otp_candidates", "run", {"tab": "t1", "element": "s1e1", "secret": T.SECRET_KEY})
    await t("otp-describe", F, "otp_candidates", "describe", {"tab": "t1", "element": "s1e1", "secret": T.SECRET_KEY})
    await t("fill-otp-bad", F, "fill_otp", "run", {"tab": "t1", "element": "s1e1", "secret": T.SECRET_KEY,
                                                    "candidate": "nope"})
    # the owner's hidden input (browser.secret, browser.login_code)
    await t("open-login", R, "open", "run", {"url": "https://a.example/login"})
    await t("snap-login", R, "snapshot", "run", {"tab": "t3"})
    await t("secret-describe", R, "secret", "describe", {"tab": "t3", "element": "s1e2"})
    await t("secret", R, "secret", "run", {"tab": "t3", "element": "s1e2", "remember": "site password"},
            ctx={"intent": "i-sec"})
    await t("code", R, "login_code", "run", {"tab": "t3", "element": "s1e2"})
    await t("type-secret", R, "act", "run", {"tab": "t3", "element": "s1e2", "action": "type", "text": "x"})
    await t("upload-relative", R, "act", "run", {"tab": "t3", "element": "s1e1", "action": "upload",
                                                  "files": ["/nonexistent-dir/a.txt"]})


SCENARIOS = [("page_ops_tabs", page_ops_tabs), ("secret_and_terminal_tabs", secret_and_terminal_tabs),
             ("pay_tabs", pay_tabs), ("capture_tabs", capture_tabs), ("uploads_tabs", uploads_tabs),
             ("otp_tabs", otp_tabs), ("rest_and_tabs_tabs", rest_and_tabs_tabs), ("keepalive_tabs", keepalive_tabs),
             ("old_tabs", old_tabs), ("odd_version_tabs", odd_version_tabs), ("tools_tabs", tools_tabs)]


_seals = [0]


def ext_seal(*args: Any) -> Any:
    """The fake extension's own seal (the inbox feed it sends): its nonce from its own counter, never the daemon's, so
    the daemon's random draws stay the same as on the Rust side, where the extension is replayed."""
    import hashlib
    from ferb.identity import seal as seal_mod
    _seals[0] += 1
    saved = os.urandom
    os.urandom = lambda n: hashlib.sha256(f"ferb-extension-seal-{_seals[0]}".encode()).digest()[:n]   # type: ignore
    try:
        return REAL_SEAL(*args)
    finally:
        os.urandom = saved                                                # type: ignore[assignment]


REAL_SEAL: Any = None


def make(reset_random: Any) -> dict[str, Any]:
    import uuid
    from ferb.identity import seal as seal_mod
    global H, REAL_SEAL
    REAL_SEAL = seal_mod.seal
    seal_mod.seal = ext_seal                                     # type: ignore[assignment]
    saved_uuid = uuid.uuid4
    uuid.uuid4 = T.ext_uuid
    saved = images()
    asyncio.sleep = gated_sleep                                  # type: ignore[assignment]
    real_harness = T.Harness

    class Harness(real_harness):                                 # type: ignore[misc,valid-type]
        def __init__(self, home: Path) -> None:
            global H
            super().__init__(home)
            H = self
            self.task_bytes = 50 * 1000 ** 3
    T.Harness = Harness                                          # type: ignore[misc]
    real_policy = policy_recorded()
    try:
        out = []
        for name, script in SCENARIOS:
            reset_random()
            T._ext_n[0] = 0
            _seals[0] = 0
            GATES.clear()
            out.append(asyncio.run(scenario(name, script)))
    finally:
        from ferb.act.evidence import mask
        mask.size, mask.mask_secret_fields = saved[0], saved[1]  # type: ignore[assignment]
        if saved[2] is not None:
            sys.modules["Quartz"] = saved[2]
        else:
            sys.modules.pop("Quartz", None)
        asyncio.sleep = _sleep                                   # type: ignore[assignment]
        T.Harness = real_harness                                 # type: ignore[misc]
        from ferb.act.browser import tools as bt
        from ferb.policy import urls
        urls.parse, bt.typed_destination = real_policy               # type: ignore[assignment]
        uuid.uuid4 = saved_uuid
        seal_mod.seal = REAL_SEAL                                # type: ignore[assignment]
    from ferb.act.browser.profiles import BROWSERS
    from ferb.act.mac import browsers as mac_browsers
    apps = ["com.google.Chrome", "com.google.Chrome.helper", "com.brave.Browser", "com.example.notabrowser"]
    return {"note": __doc__, "device_id": "dev-1", "known": {a: mac_browsers.known(a) for a in apps},
            "browsers": {k: {"bundle_id": v.bundle_id, "label": v.label} for k, v in BROWSERS.items()},
            "clock_start": "2026-01-01T09:00:00+00:00", "monotonic_start": 1000.0, "uploads": UPLOADS,
            "files": {k: base64.b64encode(v).decode() for k, v in FILES.items()}, "mtime_ns": MTIME_NS,
            "policy": {"parse": dict(sorted(POLICY["parse"].items())), "typed": dict(sorted(POLICY["typed"].items()))},
            "scenarios": out}
