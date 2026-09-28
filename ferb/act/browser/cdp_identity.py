"""CDP driver for FERB's own Chrome profile (sections 2, 9.1, 10.1): identity FERB only.

Chrome runs on `~/.ferb/chrome/identity` with `--remote-debugging-pipe` (no TCP port): commands on fd 3, replies
and events on fd 4, NUL-delimited JSON. Payloads are never logged (they can carry secrets).

- Elements are pinned by Chrome's backend node id, held here, never by anything the page can edit.
- Element ids carry their snapshot generation; a navigation or a newer snapshot makes older ids stale.
- `fill_secret` / `fill_otp`: the frame's origin is checked against the secret's `allowed_origins` before the value
  is fetched, and again inside the same in-page call that sets it (isolated world). The value is set on that exact
  element, never typed into whatever has focus. Results carry only `ref.key`.
- Only http(s) URLs open; loopback / private hosts only when allowed (tests, local tools).
- Default is a real (headed) Chrome that never comes to the front: headless Chrome gets its Google session signed
  out (step 5). Headless stays available for tests.
"""

import asyncio
import fcntl
import ipaddress
import json
import os
import subprocess
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Callable
from urllib.parse import urlsplit

from ferb.act.browser import snapshot as snap
from ferb.core.types import (ActionResult, BrowserAction, DriverFeatures, ElementRef, Identity, PageElement,
                             PageSnapshot, SecretRef, TabRef, WaitCondition)
from ferb.identity.mailbox import GmailFeedMailbox
from ferb.identity.secrets import KeychainSecretStore, SecretUnavailable

CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
WORLD = "ferb"
LOAD_TIMEOUT_S, CMD_TIMEOUT_S = 20.0, 30.0
KEYS = {"Enter": (13, "\r"), "Tab": (9, None), "Escape": (27, None), "Backspace": (8, None), "Space": (32, " "),
        "ArrowDown": (40, None), "ArrowUp": (38, None), "ArrowLeft": (37, None), "ArrowRight": (39, None)}

_SET_IF_ORIGIN = """function(expectedOrigin, value) {
  if (location.origin !== expectedOrigin) return "origin_changed";
  if (!(this instanceof HTMLInputElement) && !(this instanceof HTMLTextAreaElement)) return "not_a_text_field";
  const proto = this instanceof HTMLInputElement ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
  Object.getOwnPropertyDescriptor(proto, "value").set.call(this, value);
  this.dispatchEvent(new Event("input", {bubbles: true}));
  this.dispatchEvent(new Event("change", {bubbles: true}));
  return "ok";
}"""
_SELECT = """function(choice) {
  if (!(this instanceof HTMLSelectElement)) return "not_a_select";
  const opt = Array.from(this.options).find(o => o.value === choice || o.text.trim() === choice);
  if (!opt) return "no_such_option";
  this.value = opt.value;
  this.dispatchEvent(new Event("input", {bubbles: true}));
  this.dispatchEvent(new Event("change", {bubbles: true}));
  return "ok";
}"""
_FOCUS_AND_SELECT = """function() {
  this.focus();
  if (typeof this.select === "function") this.select();
  return document.activeElement === this || this.contains(document.activeElement);
}"""


class CdpError(Exception):
    pass


class Refused(Exception):
    """A safety check said no. The message is safe to show (never contains a secret)."""


class StaleRef(Refused):
    pass


def normalize_origin(origin: str) -> str:
    u = urlsplit(origin)
    default = {"http": 80, "https": 443}.get(u.scheme)
    port = u.port
    return f"{u.scheme}://{u.hostname or ''}" + (f":{port}" if port and port != default else "")


def check_url(url: str, allow_local: bool) -> None:
    u = urlsplit(url)
    if u.scheme not in ("http", "https") or not u.hostname:
        raise Refused(f"only http(s) URLs can be opened, not {u.scheme or 'this'}")
    if allow_local:
        return
    host = u.hostname
    if host == "localhost" or host.endswith(".localhost") or host.endswith(".local"):
        raise Refused("FERB's browser doesn't open local addresses")
    try:
        ip = ipaddress.ip_address(host)
    except ValueError:
        return
    if ip.is_private or ip.is_loopback or ip.is_link_local or ip.is_reserved or ip.is_unspecified:
        raise Refused("FERB's browser doesn't open local or private addresses")


# ---------- CDP over the pipe ----------

class Pipe:
    def __init__(self, proc: subprocess.Popen[bytes], on_event: Callable[[dict[str, Any]], None]) -> None:
        self.proc, self.on_event = proc, on_event
        self._next = 0
        self._pending: dict[int, asyncio.Future[dict[str, Any]]] = {}
        self._writer: asyncio.WriteTransport | None = None
        self._reader_task: asyncio.Task[None] | None = None
        self.closed = asyncio.Event()

    @classmethod
    async def launch(cls, args: list[str], on_event: Callable[[dict[str, Any]], None]) -> "Pipe":
        to_r, to_w = os.pipe()
        from_r, from_w = os.pipe()

        def child() -> None:                             # between fork and exec: only async-signal-safe calls
            a = fcntl.fcntl(to_r, fcntl.F_DUPFD, 10)     # lift both above 4 first, so neither dup2 clobbers the other
            b = fcntl.fcntl(from_w, fcntl.F_DUPFD, 10)
            os.dup2(a, 3)
            os.dup2(b, 4)

        # Not pass_fds=(3, 4): subprocess marks those inheritable in the child *before* preexec, failing with EBADF
        # when the parent has no fd 3/4 open. Python opens every fd non-inheritable (close-on-exec, PEP 446), so
        # with close_fds=False only stdio and the two dup2 targets reach Chrome.
        proc = subprocess.Popen(args, preexec_fn=child, close_fds=False, stdin=subprocess.DEVNULL,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        os.close(to_r)
        os.close(from_w)
        self = cls(proc, on_event)
        loop = asyncio.get_running_loop()
        reader = asyncio.StreamReader(limit=64 * 1024 * 1024)
        await loop.connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), os.fdopen(from_r, "rb", 0))
        self._writer, _ = await loop.connect_write_pipe(asyncio.Protocol, os.fdopen(to_w, "wb", 0))
        self._reader_task = asyncio.create_task(self._read(reader))
        return self

    async def _read(self, reader: asyncio.StreamReader) -> None:
        try:
            while True:
                raw = await reader.readuntil(b"\0")
                msg = json.loads(raw[:-1])
                if "id" in msg:
                    fut = self._pending.pop(msg["id"], None)
                    if fut and not fut.done():
                        fut.set_result(msg)
                else:
                    try:
                        self.on_event(msg)
                    except Exception:
                        pass                             # one bad event never kills the connection
        except (asyncio.IncompleteReadError, asyncio.LimitOverrunError, ConnectionError, ValueError):
            pass
        finally:
            self.closed.set()
            for fut in self._pending.values():
                if not fut.done():
                    fut.set_exception(CdpError("FERB's browser exited"))
            self._pending.clear()

    async def send(self, method: str, params: dict[str, Any] | None = None, session: str | None = None,
                   timeout: float = CMD_TIMEOUT_S) -> dict[str, Any]:
        if self.closed.is_set() or self._writer is None:
            raise CdpError("FERB's browser is not running")
        self._next += 1
        mid = self._next
        fut: asyncio.Future[dict[str, Any]] = asyncio.get_running_loop().create_future()
        self._pending[mid] = fut
        msg: dict[str, Any] = {"id": mid, "method": method, "params": params or {}}
        if session:
            msg["sessionId"] = session
        self._writer.write(json.dumps(msg).encode() + b"\0")
        try:
            resp = await asyncio.wait_for(fut, timeout)
        finally:
            self._pending.pop(mid, None)
        if "error" in resp:
            raise CdpError(f"{method}: {resp['error'].get('message', 'error')}")   # never our params
        result: dict[str, Any] = resp.get("result", {})
        return result

    async def close(self) -> None:
        try:
            await self.send("Browser.close", timeout=5)
        except (CdpError, asyncio.TimeoutError):
            pass
        try:
            await asyncio.wait_for(asyncio.to_thread(self.proc.wait), 10)
        except asyncio.TimeoutError:
            self.proc.kill()
        if self._writer:
            self._writer.close()
        if self._reader_task:
            await asyncio.wait([self._reader_task], timeout=2)


# ---------- tab state ----------

@dataclass
class _Frame:
    session: str
    parent_session: str | None                           # the session that owns this frame's <iframe> (OOPIF)


@dataclass
class _Tab:
    ref: TabRef
    target_id: str
    session: str
    owner: str | None = None                             # task id: only that task's workers may use the tab
    url: str = "about:blank"
    nav: int = 0                                         # bumps on any frame navigation -> older ids stale
    snap_gen: int = 0
    snap_nav: int = -1
    elements: dict[str, tuple[str, str, int, PageElement]] = field(default_factory=dict)  # id -> frame, session,
    #                                                                                     node, info
    roots: dict[str, str] = field(default_factory=dict)                              # session -> its root frame
    children: dict[str, str | None] = field(default_factory=dict)                    # child session -> parent
    dialogs: list[str] = field(default_factory=list)
    loaded: asyncio.Event = field(default_factory=asyncio.Event)
    worlds: dict[tuple[str, str], int] = field(default_factory=dict)                 # (session, frame) -> context


class CdpIdentityDriver:
    name = "cdp_identity"
    identity = Identity.FERB
    features = DriverFeatures(trusted_input_background=True, parallel_actions=True, network_events=True,
                              dom_snapshot=True, screenshot_background=True)

    def __init__(self, user_data_dir: Path, device_id: str, secrets: KeychainSecretStore,
                 add_secret_value: Callable[[str], None], chrome: str = CHROME, headless: bool = False,
                 allow_local: bool = False, mailbox_address: Callable[[], str] = lambda: "",
                 feed_url: str | None = None) -> None:
        self.user_data_dir, self.device_id, self.secrets = user_data_dir, device_id, secrets
        self.add_secret_value, self.chrome, self.headless, self.allow_local = add_secret_value, chrome, headless, \
            allow_local
        self.mailbox_address, self.feed_url = mailbox_address, feed_url
        self.pipe: Pipe | None = None
        self.generation = 0
        self._tabs: dict[str, _Tab] = {}
        self._by_session: dict[str, _Tab] = {}
        self._launch_lock = asyncio.Lock()
        self._n = 0
        self.used_otps: set[str] = set()                 # message ids already filled (memory only)

    # ----- lifecycle -----

    @property
    def running(self) -> bool:
        return self.pipe is not None and not self.pipe.closed.is_set()

    async def _ensure(self) -> Pipe:
        async with self._launch_lock:
            if self.running:
                assert self.pipe
                return self.pipe
            if not (self.user_data_dir / "Default").is_dir():
                raise Refused("FERB's browser profile isn't set up: run `ferb setup --seed-identity`")
            self._reset()
            args = [self.chrome, "--remote-debugging-pipe", f"--user-data-dir={self.user_data_dir}",
                    "--no-first-run", "--no-default-browser-check", "--window-size=1280,900"]
            # Default: a real window, never brought to the front (Google signs out headless sessions). No startup
            # window; each tab opens in its own background window (a background tab in a shared window would be
            # hidden and throttled by the page).
            args += ["--headless=new", "about:blank"] if self.headless else ["--no-startup-window"]
            try:
                self.pipe = await Pipe.launch(args, self._event)
                await self.pipe.send("Browser.getVersion", timeout=15)
            except (OSError, CdpError, asyncio.TimeoutError) as e:
                if self.pipe:
                    await self.pipe.close()
                self.pipe = None
                raise CdpError("FERB's browser didn't start (is another copy using its profile?)") from e
            try:
                await self.pipe.send("Browser.setDownloadBehavior", {"behavior": "deny"})
            except CdpError:
                pass
            return self.pipe

    def _reset(self) -> None:
        self.generation += 1                             # every ref from the old browser is now stale
        self._tabs.clear()
        self._by_session.clear()

    async def shutdown(self) -> None:
        async with self._launch_lock:
            if self.pipe:
                await self.pipe.close()
            self.pipe = None
            self._reset()

    async def release(self, owner: str) -> None:
        """Task ended: close its tabs; stop Chrome when nothing is left open."""
        for tab in [t for t in self._tabs.values() if t.owner == owner]:
            await self.close(tab.ref)
        if not self._tabs and self.running:
            await self.shutdown()

    # ----- events -----

    def _event(self, msg: dict[str, Any]) -> None:
        method, params, session = msg.get("method"), msg.get("params", {}), msg.get("sessionId")
        if method == "Target.attachedToTarget":
            tab = self._by_session.get(session or "")
            if tab is not None and params.get("targetInfo", {}).get("type") == "iframe":
                child = params["sessionId"]
                tab.children[child] = session
                self._by_session[child] = tab
                asyncio.create_task(self._init_child(child))
            return
        if method == "Target.detachedFromTarget":
            gone = params.get("sessionId", "")
            tab = self._by_session.pop(gone, None)
            if tab is not None:
                tab.children.pop(gone, None)
                if gone == tab.session:
                    self._tabs.pop(tab.ref.tab_id, None)
            return
        tab = self._by_session.get(session or "")
        if tab is None:
            return
        if method == "Page.frameNavigated":
            tab.nav += 1
            tab.worlds = {k: v for k, v in tab.worlds.items() if k[1] != params["frame"]["id"]}
            if session == tab.session and not params["frame"].get("parentId"):
                tab.url = params["frame"].get("url", tab.url)
                tab.loaded.clear()
        elif method == "Page.loadEventFired" and session == tab.session:
            tab.loaded.set()
        elif method == "Runtime.executionContextsCleared":
            tab.worlds = {k: v for k, v in tab.worlds.items() if k[0] != session}
        elif method == "Page.javascriptDialogOpening":
            tab.dialogs.append(f"{params.get('type')}: {str(params.get('message', ''))[:200]}")
            asyncio.create_task(self._dismiss(session or "", params.get("type") == "beforeunload"))

    async def _dismiss(self, session: str, accept: bool) -> None:
        try:
            assert self.pipe
            await self.pipe.send("Page.handleJavaScriptDialog", {"accept": accept}, session)
        except (CdpError, AssertionError):
            pass

    async def _init_child(self, session: str) -> None:
        try:
            assert self.pipe
            await self.pipe.send("Page.enable", session=session)
            await self.pipe.send("Target.setAutoAttach", {"autoAttach": True, "waitForDebuggerOnStart": False,
                                                          "flatten": True}, session)
        except (CdpError, AssertionError):
            pass

    # ----- BrowserDriver -----

    async def tabs(self) -> list[TabRef]:
        return [t.ref for t in self._tabs.values()]

    async def open(self, url: str, background: bool = True) -> TabRef:
        check_url(url, self.allow_local)
        pipe = await self._ensure()
        target = (await pipe.send("Target.createTarget", {"url": "about:blank", "background": background,
                                                          "newWindow": not self.headless}))["targetId"]
        session = (await pipe.send("Target.attachToTarget", {"targetId": target, "flatten": True}))["sessionId"]
        self._n += 1
        ref = TabRef(self.device_id, "chrome", "identity", self.generation, f"t{self._n}")
        tab = _Tab(ref, target, session)
        self._tabs[ref.tab_id] = tab
        self._by_session[session] = tab
        await pipe.send("Page.enable", session=session)
        await pipe.send("Target.setAutoAttach", {"autoAttach": True, "waitForDebuggerOnStart": False,
                                                 "flatten": True}, session)
        try:
            await self._navigate(tab, url)
        except BaseException:
            await self.close(ref)                         # a failed load leaves nothing open behind
            raise
        return ref

    async def navigate(self, ref: TabRef, url: str) -> None:
        check_url(url, self.allow_local)
        await self._navigate(self._tab(ref), url)

    async def _navigate(self, tab: _Tab, url: str) -> None:
        assert self.pipe
        tab.loaded.clear()
        r = await self.pipe.send("Page.navigate", {"url": url}, tab.session)
        if r.get("errorText"):
            raise Refused(f"couldn't load the page ({r['errorText']})")
        tab.url = url
        try:
            await asyncio.wait_for(tab.loaded.wait(), LOAD_TIMEOUT_S)
        except asyncio.TimeoutError:
            pass                                          # still loading: the snapshot shows what's there

    async def close(self, tab: TabRef) -> None:
        t = self._tabs.pop(tab.tab_id, None)
        if t is None:
            return
        for s in [t.session, *t.children]:
            self._by_session.pop(s, None)
        if self.running:
            assert self.pipe
            try:
                await self.pipe.send("Target.closeTarget", {"targetId": t.target_id})
            except CdpError:
                pass

    async def snapshot(self, tab: TabRef) -> PageSnapshot:
        t = self._tab(tab)
        assert self.pipe
        nav_at_start = t.nav
        elements: dict[str, tuple[str, str, int, PageElement]] = {}
        text, title, url, truncated = "", "", t.url, False
        gen = t.snap_gen + 1
        for frame_id, session, is_main in await self._frames(t):
            try:
                ctx = await self._world(t, session, frame_id)
                arr = await self.pipe.send("Runtime.evaluate", {"expression": snap.WALKER, "contextId": ctx,
                                                                "objectGroup": "ferb-snap"}, session)
                oid = arr["result"]["objectId"]
                meta = (await self.pipe.send("Runtime.callFunctionOn", {
                    "functionDeclaration": snap.DESCRIBE, "objectId": oid, "returnByValue": True,
                    "arguments": [{"value": snap.MAX_TEXT}, {"value": snap.MAX_LABEL}, {"value": snap.MAX_VALUE}]},
                    session))["result"]["value"]
                props = (await self.pipe.send("Runtime.getProperties", {"objectId": oid, "ownProperties": True},
                                              session))["result"]
                handles = {int(p["name"]): p["value"]["objectId"] for p in props
                           if p["name"].isdigit() and "objectId" in p.get("value", {})}
                for i, raw in enumerate(meta["els"]):
                    if len(elements) >= snap.MAX_ELEMENTS:
                        truncated = True
                        break
                    if i not in handles:
                        continue
                    node = (await self.pipe.send("DOM.describeNode", {"objectId": handles[i]}, session))["node"]
                    eid = f"s{gen}e{len(elements) + 1}"
                    elements[eid] = (frame_id, session, node["backendNodeId"], snap.element(eid, frame_id, raw))
                if is_main:
                    text, title, url = meta["text"], meta["title"], meta["url"]
                    truncated = truncated or len(meta["text"]) >= snap.MAX_TEXT
                await self.pipe.send("Runtime.releaseObjectGroup", {"objectGroup": "ferb-snap"}, session)
            except (CdpError, KeyError, TypeError):
                continue                                  # a frame that went away mid-snapshot is skipped
        if t.nav != nav_at_start:
            raise StaleRef("the page navigated during the snapshot; take it again")
        t.snap_gen, t.snap_nav, t.elements = gen, t.nav, elements
        dialogs, t.dialogs = t.dialogs, []
        return PageSnapshot(tab, gen, url, title, [e for *_, e in elements.values()], text, truncated, dialogs)

    async def act(self, el: ElementRef, action: BrowserAction, intent_id: str) -> ActionResult:
        t, session, node, info = self._resolve(el)
        assert self.pipe
        p = self.pipe
        if action.kind == "click":
            x, y = await self._center(t, session, el.frame_id, node)
            for kind in ("mouseMoved", "mousePressed", "mouseReleased"):
                await p.send("Input.dispatchMouseEvent", {"type": kind, "x": x, "y": y, "button": "left",
                                                          "clickCount": 1}, t.session)
            return ActionResult(True, "clicked")
        if action.kind == "type":
            if info.secret:
                raise Refused("that's a password/code field: use fill_secret or fill_otp")
            if not await self._call(t, session, el.frame_id, node, _FOCUS_AND_SELECT):
                raise Refused("couldn't focus the element (something else took focus)")
            await p.send("Input.insertText", {"text": action.text or ""}, t.session)
            return ActionResult(True, "typed")
        if action.kind == "select":
            status = await self._call(t, session, el.frame_id, node, _SELECT, action.text or "")
            return ActionResult(status == "ok", str(status))
        if action.kind == "press":
            key = action.text or "Enter"
            if key not in KEYS:
                raise Refused(f"unsupported key {key!r}; use one of {', '.join(KEYS)}")
            await self._call(t, session, el.frame_id, node, "function() { this.focus(); return true; }")
            code, text = KEYS[key]
            base = {"key": key, "code": key if len(key) > 1 else f"Key{key.upper()}", "windowsVirtualKeyCode": code}
            await p.send("Input.dispatchKeyEvent", {"type": "keyDown", **base, **({"text": text} if text else {})},
                         t.session)
            await p.send("Input.dispatchKeyEvent", {"type": "keyUp", **base}, t.session)
            return ActionResult(True, f"pressed {key}")
        if action.kind == "scroll":
            x, y = await self._center(t, session, el.frame_id, node)
            await p.send("Input.dispatchMouseEvent", {"type": "mouseWheel", "x": x, "y": y, "deltaX": 0,
                                                      "deltaY": -600 if action.text == "up" else 600}, t.session)
            return ActionResult(True, "scrolled")
        raise Refused(f"unknown action {action.kind}")

    async def wait(self, tab: TabRef, until: WaitCondition, timeout: float) -> bool:
        t = self._tab(tab)
        loop = asyncio.get_running_loop()
        end = loop.time() + timeout
        while loop.time() < end:
            if until.kind == "load" and t.loaded.is_set():
                return True
            if until.kind in ("text", "url"):
                probe = "document.body ? document.body.innerText : ''" if until.kind == "text" else "location.href"
                try:
                    value = await self._eval_main(t, probe)
                except CdpError:
                    value = ""
                if (until.value or "") in str(value or ""):
                    return True
            await asyncio.sleep(0.25)
        return False

    async def screenshot(self, tab: TabRef) -> bytes:
        import base64
        t = self._tab(tab)
        assert self.pipe
        return base64.b64decode((await self.pipe.send("Page.captureScreenshot", {"format": "png"}, t.session))["data"])

    # ----- ownership + lookups (sync: capabilities' describe() uses them) -----

    def claim(self, tab: TabRef, owner: str) -> None:
        self._tab(tab).owner = owner

    def owned_by(self, tab_id: str, owner: str) -> TabRef:
        t = self._tabs.get(tab_id)
        if t is None or t.owner != owner:
            raise Refused(f"no tab {tab_id} open for this task")
        return t.ref

    def tab_url(self, tab_id: str) -> str | None:
        t = self._tabs.get(tab_id)
        return t.url if t else None

    def element_info(self, tab_id: str, element_id: str) -> PageElement | None:
        t = self._tabs.get(tab_id)
        hit = t.elements.get(element_id) if t else None
        return hit[3] if hit else None

    def element_ref(self, tab: TabRef, element_id: str) -> ElementRef:
        t = self._tab(tab)
        hit = t.elements.get(element_id)
        gen = int(element_id[1:].split("e")[0]) if element_id.startswith("s") and "e" in element_id[1:] \
            and element_id[1:].split("e")[0].isdigit() else -1
        return ElementRef(tab, hit[0] if hit else "", gen, element_id)

    def _tab(self, ref: TabRef) -> _Tab:
        t = self._tabs.get(ref.tab_id)
        if t is None or ref.generation != self.generation:
            raise StaleRef(f"tab {ref.tab_id} is gone (closed or the browser restarted)")
        return t

    def _resolve(self, el: ElementRef) -> tuple[_Tab, str, int, PageElement]:
        t = self._tab(el.tab)
        hit = t.elements.get(el.element_id)
        if hit is None or el.snapshot_gen != t.snap_gen:
            raise StaleRef(f"element {el.element_id} is from an older snapshot; take a new one")
        if t.nav != t.snap_nav:
            raise StaleRef("the page changed since the snapshot; take a new one")
        _, session, node, info = hit
        return t, session, node, info

    # ----- helpers -----

    async def _frames(self, t: _Tab) -> list[tuple[str, str, bool]]:
        """(frame id, owning session, is main frame) across the tab and its out-of-process iframes."""
        assert self.pipe
        found: dict[str, tuple[str, bool]] = {}

        def walk(session: str, node: dict[str, Any], main: bool) -> None:
            found[node["frame"]["id"]] = (session, main)
            if not node["frame"].get("parentId") or session not in t.roots:
                t.roots.setdefault(session, node["frame"]["id"])
            for child in node.get("childFrames", []):
                walk(session, child, False)

        for i, s in enumerate([t.session, *t.children]):
            try:
                walk(s, (await self.pipe.send("Page.getFrameTree", session=s))["frameTree"], i == 0)
            except CdpError:
                pass
        return [(fid, s, main) for fid, (s, main) in found.items()]

    async def _world(self, t: _Tab, session: str, frame_id: str) -> int:
        key = (session, frame_id)
        if key not in t.worlds:
            assert self.pipe
            t.worlds[key] = (await self.pipe.send("Page.createIsolatedWorld", {"frameId": frame_id,
                                                                               "worldName": WORLD}, session))[
                "executionContextId"]
        return t.worlds[key]

    async def _object(self, t: _Tab, session: str, frame_id: str, node: int) -> str:
        assert self.pipe
        ctx = await self._world(t, session, frame_id)
        obj = await self.pipe.send("DOM.resolveNode", {"backendNodeId": node, "executionContextId": ctx}, session)
        return str(obj["object"]["objectId"])

    async def _call(self, t: _Tab, session: str, frame_id: str, node: int, fn: str, *args: Any) -> Any:
        assert self.pipe
        oid = await self._object(t, session, frame_id, node)
        r = await self.pipe.send("Runtime.callFunctionOn", {"functionDeclaration": fn, "objectId": oid,
                                                            "arguments": [{"value": a} for a in args],
                                                            "returnByValue": True}, session)
        return r.get("result", {}).get("value")

    async def _eval_main(self, t: _Tab, expr: str) -> Any:
        assert self.pipe
        frames = await self._frames(t)
        main = next((fid for fid, s, is_main in frames if is_main), None)
        if main is None:
            raise CdpError("no main frame")
        ctx = await self._world(t, t.session, main)
        r = await self.pipe.send("Runtime.evaluate", {"expression": expr, "contextId": ctx, "returnByValue": True},
                                 t.session)
        return r.get("result", {}).get("value")

    async def _center(self, t: _Tab, session: str, frame_id: str, node: int) -> tuple[float, float]:
        assert self.pipe
        try:
            await self.pipe.send("DOM.scrollIntoViewIfNeeded", {"backendNodeId": node}, session)
        except CdpError:
            pass
        quads = (await self.pipe.send("DOM.getContentQuads", {"backendNodeId": node}, session))["quads"]
        if not quads:
            raise Refused("the element isn't on screen")
        q = quads[0]
        x, y = sum(q[0::2]) / 4, sum(q[1::2]) / 4
        ox, oy = await self._frame_offset(t, session, frame_id)
        return x + ox, y + oy

    async def _frame_offset(self, t: _Tab, session: str, frame_id: str) -> tuple[float, float]:
        """An out-of-process iframe reports coordinates in its own viewport: add its <iframe>'s position."""
        if session == t.session:
            return 0.0, 0.0
        assert self.pipe
        parent = t.children.get(session) or t.session
        owner = await self.pipe.send("DOM.getFrameOwner", {"frameId": frame_id}, parent)
        quads = (await self.pipe.send("DOM.getContentQuads", {"backendNodeId": owner["backendNodeId"]}, parent))["quads"]
        px, py = (min(quads[0][0::2]), min(quads[0][1::2])) if quads else (0.0, 0.0)
        gx, gy = await self._frame_offset(t, parent, t.roots.get(parent, "")) if parent != t.session else (0.0, 0.0)
        return px + gx, py + gy

    async def _frame_origin(self, t: _Tab, session: str, frame_id: str) -> str:
        assert self.pipe

        def find(node: dict[str, Any]) -> dict[str, Any] | None:
            if node["frame"]["id"] == frame_id:
                frame: dict[str, Any] = node["frame"]
                return frame
            for c in node.get("childFrames", []):
                hit = find(c)
                if hit:
                    return hit
            return None

        frame = find((await self.pipe.send("Page.getFrameTree", session=session))["frameTree"])
        return normalize_origin(frame.get("securityOrigin", "")) if frame else ""

    # ----- FERB identity: secrets and one-time codes (section 10.1) -----

    async def fill_secret(self, el: ElementRef, ref: SecretRef) -> ActionResult:
        if not self.secrets.has(ref):
            raise Refused(f"no secret {ref.key} is set up")
        origin, t, session, node = await self._checked_target(el, ref)
        try:
            value = await self.secrets._reveal(ref)
        except SecretUnavailable:
            raise Refused(f"secret {ref.key} isn't in the Keychain")
        self.add_secret_value(value)                    # scrub it if a page or error ever echoes it back
        status = await self._set_value(t, session, el.frame_id, node, origin, value)
        del value
        return ActionResult(status == "ok", f"{status} ({ref.key})")

    async def fill_otp(self, el: ElementRef, ref: SecretRef, since: datetime, sender_contains: str | None,
                       timeout: float = 120.0, poll: float = 5.0) -> ActionResult:
        """The newest one-time code emailed to FERB since `since`, set into a field allowed for `ref`'s account."""
        if not self.secrets.has(ref):
            raise Refused(f"no account {ref.key} is set up")
        origin, t, session, node = await self._checked_target(el, ref)
        mailbox = GmailFeedMailbox(self._read_url, self.mailbox_address(), **({"feed_url": self.feed_url}
                                                                              if self.feed_url else {}))
        mailbox.used = self.used_otps
        loop = asyncio.get_running_loop()
        end = loop.time() + timeout
        while True:
            found = await mailbox.find_code(since, sender_contains)
            if found.code or found.reason in ("ambiguous", "not_signed_in") or loop.time() >= end:
                break
            await asyncio.sleep(poll)
        if not found.code:
            return ActionResult(False, {"no_new_message": "no new code arrived in FERB's inbox",
                                        "no_code": "the newest email has no code in it",
                                        "ambiguous": "the newest email has more than one code; refusing to guess",
                                        "not_signed_in": "FERB's inbox is signed out (re-seed)"}[found.reason])
        self.add_secret_value(found.code)
        origin_now, _, _, _ = await self._checked_target(el, ref)   # the page may have moved while we waited
        status = await self._set_value(t, session, el.frame_id, node, origin_now, found.code)
        if status == "ok" and found.message_id:
            self.used_otps.add(found.message_id)
        return ActionResult(status == "ok", f"{status} (code from FERB's inbox)")

    async def _checked_target(self, el: ElementRef, ref: SecretRef) -> tuple[str, _Tab, str, int]:
        t, session, node, info = self._resolve(el)
        allowed = {normalize_origin(o) for o in self.secrets.spec(ref).allowed_origins}
        origin = await self._frame_origin(t, session, el.frame_id)
        if origin not in allowed:
            raise Refused(f"refused: this field's page ({origin or 'unknown origin'}) isn't allowed for {ref.key}")
        return origin, t, session, node

    async def _set_value(self, t: _Tab, session: str, frame_id: str, node: int, origin: str, value: str) -> str:
        try:
            return str(await self._call(t, session, frame_id, node, _SET_IF_ORIGIN, origin, value))
        except CdpError:
            return "fill_failed"                          # CdpError text never includes our arguments

    async def _read_url(self, url: str) -> str:
        """Load a URL in a private tab (no task owns it) and return the document's source. Mailbox use only."""
        check_url(url, self.allow_local)
        ref = await self.open(url)
        try:
            return str(await self._eval_main(self._tab(ref), "document.documentElement.outerHTML") or "")
        finally:
            await self.close(ref)

    async def session_ok(self) -> bool:
        """Doctor / setup: is the seeded session still signed in as FERB's address?"""
        mailbox = GmailFeedMailbox(self._read_url, self.mailbox_address(), **({"feed_url": self.feed_url}
                                                                              if self.feed_url else {}))
        try:
            return await mailbox.signed_in()
        finally:
            if not self._tabs:
                await self.shutdown()
