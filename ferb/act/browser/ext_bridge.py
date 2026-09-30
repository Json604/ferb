"""Driver for browser profiles through the FERB extension (sections 9.4, 9.6).

- `Hub` owns native-host connections: every message goes through pairing (`install_id` + HMAC + seq); unpaired
  installs may only say hello and pair. A paired `hello` must carry this protocol version, or the install is
  refused (a stale unpacked extension keeps running until it's reloaded).
- `Bridge` (one per paired profile) runs commands: request id (the ledger intent id for consequential actions),
  connection generation, timeout. A new connection bumps the generation; commands in flight on the old one fail
  with `OutcomeUnknown` (the gateway marks the ledger `unknown`) and are reconciled from the re-announced state —
  seen and done -> done, never seen -> failed, started -> stays unknown — never resent.
- `Router` is what the owner `browser.*` tools use: task-owned tabs with short aliases, generation-scoped element
  ids (a navigation or a newer snapshot makes older ids stale), and the pre-action account check (9.3).
- `WakeServer` serves the unique blank local page that wakes a sleeping extension (spike 13).
"""

import asyncio
import json
import secrets
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any, Callable
from urllib.parse import urlsplit

from ferb.act.browser import accounts
from ferb.act.browser.ext_protocol import PROTOCOL_VERSION, Hello, Op, Result, TabEvent, command, parse
from ferb.act.browser.pairing import Binding, Pairings
from ferb.act.browser.profiles import BrowserProfile, open_known_profile
from ferb.act.browser.resolver import NeedsOwner, Resolver
from ferb.act.browser.snapshot import element as page_element
from ferb.act.browser.tools import Secrets
from ferb.identity.mailbox import FEED_URL, REASONS, GmailFeedMailbox, wait_for_code
from ferb.identity.seal import seal, seal_key, unseal
from ferb.identity.secrets import KeychainSecretStore, SecretUnavailable, normalize_origin, origin_allowed
from ferb.core.errors import DriverError, Refused, StaleRef
from ferb.core.types import (ActionResult, BrowserAction, DriverFeatures, ElementRef, Identity, PageElement,
                             PageSnapshot, SecretRef, SecretSpec, TabRef, WaitCondition)
from ferb.orchestrator.ledger import Ledger

Publish = Callable[[str, dict[str, Any]], None]
CMD_TIMEOUT_S, WAKE_TIMEOUT_S = 30.0, 20.0


class OutcomeUnknown(DriverError):
    """The connection dropped while a command was in flight: it may or may not have happened."""


@dataclass
class _Conn:
    writer: asyncio.StreamWriter
    install_id: str | None = None
    bridge: "Bridge | None" = None
    gen: int = 0
    unpaired_install: str | None = None


@dataclass
class Bridge:
    binding: Binding
    ledger: Ledger
    publish: Publish
    device_id: str
    gen: int = 0
    conn: _Conn | None = None
    version: str = ""
    pending: dict[str, asyncio.Future[Result]] = field(default_factory=dict)
    lost: set[str] = field(default_factory=set)             # ledger ids in flight when a connection dropped
    wake_url: str | None = None                              # the wake page, closed once FERB's own tab is open
    tabs: dict[int, tuple[str, bool]] = field(default_factory=dict)   # chrome tab id -> (host, active)
    navs: dict[int, int] = field(default_factory=dict)      # chrome tab id -> loads seen (stale-ref detection)
    connected: asyncio.Event = field(default_factory=asyncio.Event)

    @property
    def name(self) -> str:
        return f"{self.binding.browser}/{self.binding.folder}"

    async def call(self, op: Op, args: dict[str, Any], timeout: float = CMD_TIMEOUT_S,
                   cid: str | None = None) -> Any:
        conn = self.conn
        if conn is None:
            raise DriverError(f"{self.name} isn't connected")
        cid = cid or "c-" + secrets.token_hex(8)
        fut: asyncio.Future[Result] = asyncio.get_running_loop().create_future()
        self.pending[cid] = fut
        conn.writer.write(json.dumps(command(cid, conn.gen, op, args, int(timeout * 1000))).encode() + b"\n")
        await conn.writer.drain()
        try:
            r = await asyncio.wait_for(fut, timeout + 5)
        except asyncio.TimeoutError:
            raise OutcomeUnknown(f"{op} timed out in {self.name}") from None
        finally:
            self.pending.pop(cid, None)
        if r.status == "ok":
            return r.value
        if r.status == "stale":
            raise StaleRef("the browser reconnected; take a new snapshot")
        if r.status == "duplicate":
            raise DriverError(f"{op} was already sent once; not repeating it")
        raise DriverError(r.error or f"{op} failed")

    def attach(self, conn: _Conn, hello: Hello) -> None:
        self.gen += 1
        conn.gen, conn.bridge = self.gen, self
        old, self.conn, self.version = self.conn, conn, hello.version
        if old is not None and old is not conn:
            self.drop(old)
        self._reconcile(hello)
        self.connected.set()

    def drop(self, conn: _Conn) -> None:
        if self.conn is conn:
            self.conn = None
            self.connected.clear()
        for cid, fut in list(self.pending.items()):
            if not fut.done():
                self.lost.add(cid)
                fut.set_exception(OutcomeUnknown(f"{self.name} disconnected mid-command; outcome unknown"))
        self.pending.clear()

    def _reconcile(self, hello: Hello) -> None:
        seen = {s.get("id"): s.get("status") for s in hello.seen}
        owned = {o.get("id") for o in hello.owned}
        for cid in list(self.lost):
            try:
                row = self.ledger.get(cid)
            except Exception:
                self.lost.discard(cid)
                continue
            if row.intent.state != "unknown":
                self.lost.discard(cid)
                continue
            if seen.get(cid) == "done" or cid in owned:
                self.ledger.reconcile(cid, "done", [f"{self.name} reported it done after reconnecting"])
            elif cid not in seen:
                self.ledger.reconcile(cid, "failed", [f"{self.name} never received it"])
            else:
                continue                                    # started, no result: stays unknown for the owner
            self.lost.discard(cid)


class Hub:
    def __init__(self, pairings: Pairings, ledger: Ledger, publish: Publish, device_id: Callable[[], str]) -> None:
        self.pairings, self.ledger, self.publish, self.device_id = pairings, ledger, publish, device_id
        self.bridges: dict[tuple[str, str], Bridge] = {}
        self.refused: dict[str, str] = {}                    # install_id -> why (e.g. protocol mismatch)
        self.unpaired: dict[str, str] = {}                   # install_id -> browser: installed, connected, not paired
        # The trail (build step 10): every tab event goes to on_tab; while paused, only tabs FERB owns are tracked.
        self.on_tab: Callable[["Bridge", TabEvent], None] = lambda br, e: None
        self.paused: Callable[[], bool] = lambda: False
        self.owned: Callable[["Bridge", int], bool] = lambda br, tab_id: False

    def forget_owner_tabs(self) -> None:
        """On pause: drop what's known about tabs FERB doesn't own (only FERB's own stay tracked)."""
        for br in self.bridges.values():
            for tab_id in [t for t in br.tabs if not self.owned(br, t)]:
                br.tabs.pop(tab_id, None)

    def bridge(self, browser: str, folder: str) -> Bridge | None:
        b = self.pairings.for_profile(browser, folder)
        if b is None:
            return None
        br = self.bridges.get((browser, folder))
        if br is None or br.binding != b:
            br = Bridge(b, self.ledger, self.publish, self.device_id())
            self.bridges[(browser, folder)] = br
        return br

    async def serve(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        """One native-host connection, after its auth.hello: newline-delimited extension messages both ways."""
        conn = _Conn(writer)
        try:
            while line := await reader.readline():
                try:
                    raw = json.loads(line)
                except ValueError:
                    continue
                m = parse(raw) if isinstance(raw, dict) else None
                if m is not None:
                    await self._handle(conn, m)
        except (ConnectionError, asyncio.LimitOverrunError, ValueError):
            pass
        finally:
            if conn.unpaired_install:
                self.unpaired.pop(conn.unpaired_install, None)
            if conn.bridge is not None:
                conn.bridge.drop(conn)
                self.publish("browser.disconnected", {"profile": conn.bridge.name})

    async def _handle(self, conn: _Conn, m: Any) -> None:
        v = self.pairings.handle(m)
        if not v.accepted:
            return                                           # unpaired / bad MAC / replay: dropped, nothing acts
        if v.reply is not None:                              # just paired: hand the key back on this connection
            self.unpaired.pop(m.install_id, None)
            conn.writer.write(json.dumps(v.reply).encode() + b"\n")
            await conn.writer.drain()
            if v.binding:
                self.publish("browser.profile_paired", {"browser": v.binding.browser, "folder": v.binding.folder,
                                                        "identity": v.binding.identity.value})
            return
        if v.binding is None:                                # unpaired hello: nothing acts; FERB can tell the owner
            if m.type == "hello":
                try:
                    b = str(m.body().get("browser") or "")
                except ValueError:
                    b = ""
                if b in ("chrome", "brave"):
                    self.unpaired[m.install_id] = b
                    conn.unpaired_install = m.install_id
            return
        conn.install_id = m.install_id
        try:
            body = m.body()
        except ValueError:
            return
        if m.type == "hello":
            try:
                hello = Hello.model_validate(body)
            except ValueError:
                return
            if hello.protocol != PROTOCOL_VERSION:
                why = f"extension protocol {hello.protocol}, FERB {PROTOCOL_VERSION}: reload the FERB extension"
                self.refused[m.install_id] = why
                conn.writer.write(json.dumps({"type": "refused", "reason": why}).encode() + b"\n")
                await conn.writer.drain()
                return
            self.refused.pop(m.install_id, None)
            br = self.bridge(v.binding.browser, v.binding.folder)
            if br is None:
                return
            br.attach(conn, hello)
            self.pairings.db.conn.execute("UPDATE ext_installs SET ext_version = ? WHERE install_id = ?",
                                          (hello.version, m.install_id))
            conn.writer.write(json.dumps({"type": "welcome", "gen": br.gen}).encode() + b"\n")
            await conn.writer.drain()
            self.publish("browser.connected", {"profile": br.name, "version": hello.version, "gen": br.gen})
            return
        br = conn.bridge
        if br is None or br.conn is not conn:
            return                                           # results from an old connection are ignored
        if m.type == "result":
            try:
                r = Result.model_validate(body)
            except ValueError:
                return
            fut = br.pending.get(r.id)
            if fut is not None and not fut.done() and r.gen == conn.gen:
                fut.set_result(r)
        elif m.type == "tab_event":
            try:
                e = TabEvent.model_validate(body)
            except ValueError:
                return
            if self.paused() and not self.owned(br, e.tab_id):
                return                                       # paused: the owner's tabs aren't watched at all
            self.on_tab(br, e)
            if e.event == "removed":
                br.tabs.pop(e.tab_id, None)
            else:
                host = urlsplit(e.url).hostname or "" if e.url else br.tabs.get(e.tab_id, ("", False))[0]
                br.tabs[e.tab_id] = (host, bool(e.active))
                if e.event == "updated":
                    br.navs[e.tab_id] = br.navs.get(e.tab_id, 0) + 1


class WakeServer:
    """A unique blank local page per wake (not about:blank: its fragment is dropped, so it couldn't be told apart
    from the owner's tabs)."""

    def __init__(self) -> None:
        self._server: asyncio.base_events.Server | None = None
        self.port = 0

    async def start(self) -> None:
        async def handle(r: asyncio.StreamReader, w: asyncio.StreamWriter) -> None:
            try:
                await asyncio.wait_for(r.readline(), 5)          # a browser may open a connection and send nothing
                body = b"<!doctype html><title>FERB</title>"
                w.write(b"HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: %d\r\nconnection: close"
                        b"\r\n\r\n" % len(body) + body)
                await w.drain()
            except (asyncio.TimeoutError, ConnectionError):
                pass
            finally:
                w.close()
        self._server = await asyncio.start_server(handle, "127.0.0.1", 0)
        self.port = self._server.sockets[0].getsockname()[1]

    def url(self) -> str:
        return f"http://127.0.0.1:{self.port}/wake/{secrets.token_hex(8)}"

    async def close(self) -> None:
        if self._server:
            self._server.close()


# ---------- what the browser tools drive ----------

class _NoSecrets:
    """Owner profiles never get FERB's secrets."""

    def specs(self) -> list[SecretSpec]:
        return []

    def spec(self, ref: SecretRef) -> SecretSpec:
        raise KeyError(ref.key)

    def has(self, ref: SecretRef) -> bool:
        return False


@dataclass
class _Tab:
    ref: TabRef
    bridge: Bridge
    chrome_id: int
    owner: str | None
    url: str
    snap_gen: int = 0
    snap_nav: int = -1
    attached: bool = False          # the owner's own tab, taken over for a task: forgotten after, never closed
    elements: dict[str, tuple[int, int, int, PageElement]] = field(default_factory=dict)  # id -> frame, snap, local


class _TabDriver:
    """Task-owned tabs with short aliases and generation-scoped element ids, over one or more bridges."""
    name = "ext"
    features = DriverFeatures(trusted_input_background=True, parallel_actions=True, network_events=False,
                              dom_snapshot=True, screenshot_background=True)

    def __init__(self, hub: Hub, profiles: Callable[[], list[BrowserProfile]], wake: WakeServer | None = None,
                 launch: Callable[[list[str]], None] | None = None) -> None:
        self.hub, self.profiles, self.wake, self.launch = hub, profiles, wake, launch
        self._tabs: dict[str, _Tab] = {}
        self._n = 0

    async def _connected(self, br: Bridge) -> Bridge:
        """Wake a sleeping extension with a unique local page in that profile. The page stays open until FERB's own
        tab is open there (`_open_in`): if it were the profile's only window, closing it first would unload the
        profile and drop the connection before any work started."""
        if br.conn is None and self.wake is not None:
            url = self.wake.url()
            kwargs = {"launch": self.launch} if self.launch else {}
            open_known_profile(br.binding.browser, br.binding.folder, url, self.profiles(), **kwargs)
            try:
                await asyncio.wait_for(br.connected.wait(), WAKE_TIMEOUT_S)
            except asyncio.TimeoutError:
                raise DriverError(f"{br.name} didn't connect (is the FERB extension enabled there?)") from None
            br.wake_url = url
        if br.conn is None:
            raise DriverError(f"{br.name} isn't connected")
        return br

    async def _open_in(self, br: Bridge, url: str, owner: str | None) -> TabRef:
        v = await br.call("open", {"url": url}, timeout=30)
        if br.wake_url is not None:                           # FERB's own tab is open now: the wake page can go
            wake, br.wake_url = br.wake_url, None
            try:
                await br.call("wake_close", {"url": wake}, timeout=5)
            except DriverError:
                pass
        self._n += 1
        ref = TabRef(br.device_id, br.binding.browser, br.binding.folder, br.gen, f"t{self._n}")
        self._tabs[ref.tab_id] = _Tab(ref, br, int(v["tab_id"]), owner, str(v.get("url") or url))
        return ref

    def owner_of(self, br: Bridge, chrome_id: int) -> str | None:
        """The task that opened this tab, if FERB did."""
        return next((t.owner for t in self._tabs.values() if t.bridge is br and t.chrome_id == chrome_id), None)

    async def navigate(self, tab: TabRef, url: str) -> None:
        t = self._tab(tab)
        v = await t.bridge.call("navigate", {"tab_id": t.chrome_id, "url": url}, timeout=30)
        t.url = str(v.get("url") or url)

    async def tabs(self) -> list[TabRef]:
        return [t.ref for t in self._tabs.values()]

    async def close(self, tab: TabRef) -> None:
        t = self._tabs.pop(tab.tab_id, None)
        if t is not None and t.attached:
            return                                            # the owner's tab: FERB lets go of it, never closes it
        if t is not None and t.bridge.conn is not None:
            try:
                await t.bridge.call("close", {"tab_id": t.chrome_id}, timeout=10)
            except DriverError:
                pass

    async def snapshot(self, tab: TabRef) -> PageSnapshot:
        t = self._tab(tab)
        nav = t.bridge.navs.get(t.chrome_id, 0)
        gen = t.snap_gen + 1
        v = await t.bridge.call("snapshot", {"tab_id": t.chrome_id, "snap": gen}, timeout=30)
        elements: dict[str, tuple[int, int, int, PageElement]] = {}
        text, title, url, truncated = "", "", t.url, False
        for f in v["frames"]:
            for raw in f.get("elements", []):
                eid = f"s{gen}e{len(elements) + 1}"
                elements[eid] = (int(f["frame_id"]), gen, int(raw["id"]), page_element(eid, str(f["frame_id"]), raw))
            if f["frame_id"] == 0:
                text, title, url = f.get("text", ""), f.get("title", ""), f.get("url", url)
                truncated = bool(f.get("truncated"))
        t.snap_gen, t.snap_nav, t.elements, t.url = gen, nav, elements, url
        return PageSnapshot(tab, gen, url, title, [e for *_, e in elements.values()], text, truncated)

    async def act(self, el: ElementRef, action: BrowserAction, intent_id: str) -> ActionResult:
        t, frame, snap, local, info = self._resolve(el)
        if action.kind == "type" and info.secret:
            raise Refused("that's a password/code field; FERB never types into it")
        v = await t.bridge.call("act", {"tab_id": t.chrome_id, "frame_id": frame, "snap": snap, "el": local,
                                        "kind": action.kind, "text": action.text, "trusted": action.trusted},
                                timeout=30, cid=intent_id or None)
        if v.get("detail") == "stale":
            raise StaleRef(f"element {el.element_id} is gone; take a new snapshot")
        return ActionResult(bool(v.get("ok")), str(v.get("detail", "")))

    async def find(self, tab: TabRef, text: str) -> dict[str, Any]:
        """Lines on the whole page (not just the snapshot's first screens) containing `text`: a count and a few."""
        t = self._tab(tab)
        try:
            v = await t.bridge.call("find", {"tab_id": t.chrome_id, "text": text}, timeout=15)
        except DriverError as e:
            if "unknown op" in str(e):
                raise Refused("the FERB extension in that browser is older than FERB: reload it (0.6.5 or later)") \
                    from None
            raise
        return {"matches": int(v.get("count", 0)), "lines_on_page": int(v.get("lines", 0)),
                "examples": [str(x)[:160] for x in v.get("examples", [])][:20]}

    async def scroll(self, tab: TabRef, direction: str) -> dict[str, Any]:
        """Scroll the whole page about one screen; where it is now and whether it reached the end."""
        t = self._tab(tab)
        try:
            v = await t.bridge.call("scroll", {"tab_id": t.chrome_id, "direction": direction}, timeout=10)
        except DriverError as e:
            if "unknown op" in str(e):
                raise Refused("the FERB extension in that browser is older than FERB: reload it (0.6.4 or later)") \
                    from None
            raise
        return {"y": int(v.get("y", 0)), "page_height": int(v.get("height", 0)), "at_end": bool(v.get("at_end"))}

    async def wait(self, tab: TabRef, until: WaitCondition, timeout: float) -> bool:
        t = self._tab(tab)
        try:
            v = await t.bridge.call("wait", {"tab_id": t.chrome_id, "until": until.kind, "value": until.value},
                                    timeout=timeout)
        except OutcomeUnknown:
            return False
        return bool(v.get("met"))

    async def screenshot(self, tab: TabRef) -> bytes:
        import base64
        t = self._tab(tab)
        return base64.b64decode((await t.bridge.call("screenshot", {"tab_id": t.chrome_id}))["data"])

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
        head = element_id[1:].split("e")[0] if element_id.startswith("s") else ""
        return ElementRef(tab, "", int(head) if head.isdigit() else -1, element_id)

    def _tab(self, ref: TabRef) -> _Tab:
        t = self._tabs.get(ref.tab_id)
        if t is None:
            raise StaleRef(f"tab {ref.tab_id} is gone")
        if t.bridge.gen != ref.generation or t.bridge.conn is None:
            raise StaleRef(f"tab {ref.tab_id} belongs to an older browser connection")
        return t

    def _resolve(self, el: ElementRef) -> tuple[_Tab, int, int, int, PageElement]:
        t = self._tab(el.tab)
        hit = t.elements.get(el.element_id)
        if hit is None or el.snapshot_gen != t.snap_gen:
            raise StaleRef(f"element {el.element_id} is from an older snapshot; take a new one")
        if t.bridge.navs.get(t.chrome_id, 0) != t.snap_nav:
            raise StaleRef("the page changed since the snapshot; take a new one")
        frame, snap, local, info = hit
        return t, frame, snap, local, info

    def urls_of(self, owner: str) -> list[str]:
        """Last known URLs of a task's tabs (verification, section 7 step 8)."""
        return [t.url for t in self._tabs.values() if t.owner == owner]

    async def account_of(self, owner: str, site: str) -> str | None:
        """The signed-in account on a task's tab for that site, from the site's account extractor (or None)."""
        for t in self._tabs.values():
            if t.owner == owner and (urlsplit(t.url).hostname or "").endswith(site):
                v = await t.bridge.call("account", {"tab_id": t.chrome_id}, timeout=10)
                acct = v.get("account") if v.get("supported") else None
                return str(acct) if acct and acct != "signed_in_unknown" else None
        return None

    async def release(self, owner: str) -> None:
        for alias in [a for a, t in self._tabs.items() if t.owner == owner]:
            await self.close(self._tabs[alias].ref)

    async def shutdown(self) -> None:
        for alias in list(self._tabs):
            await self.close(self._tabs[alias].ref)


class Router(_TabDriver):
    """The owner's paired profiles (`browser.*`): resolver-chosen profile, account check before consequential
    actions (9.3). Never FERB's identity profile, never FERB's secrets."""
    identity = Identity.OWNER

    def __init__(self, hub: Hub, resolver: Resolver, profiles: Callable[[], list[BrowserProfile]],
                 wake: WakeServer | None = None, launch: Callable[[list[str]], None] | None = None) -> None:
        super().__init__(hub, profiles, wake, launch)
        self.resolver = resolver
        self.secrets: Secrets = _NoSecrets()

    jev: Any = None                                    # browser.profile_match (build step 13): a suggestion only

    async def open_for(self, url: str, owner: str, profile: str | None = None) -> TabRef:
        try:
            choice = self.resolver.choose(urlsplit(url).hostname, profile)
        except NeedsOwner as e:
            hint = await self._profile_hint(profile) if profile and self.jev is not None else None
            if hint is None:
                raise
            raise NeedsOwner(f"{e} Did you mean “{hint}”? (a suggestion — the owner confirms)") from None
        br = self.hub.bridge(choice.browser, choice.folder)
        if br is None:
            raise Refused(f"{choice.browser}/{choice.folder} isn't paired with FERB")
        if br.binding.identity is not Identity.OWNER:
            raise Refused("that profile belongs to FERB's identity, not the owner")
        return await self._open_in(await self._connected(br), url, owner)

    async def open(self, url: str, background: bool = True) -> TabRef:
        return await self.open_for(url, owner="")

    def _owner_bridges(self, profile: str | None) -> list[Bridge]:
        """Connected owner profiles (FERB's identity profile never), optionally only the one the owner named."""
        names = {(p.browser, p.folder): p.display_name for p in self.profiles()}
        out = []
        for br in self.hub.bridges.values():
            if br.conn is None or br.binding.identity is not Identity.OWNER:
                continue
            shown = names.get((br.binding.browser, br.binding.folder), br.binding.folder)
            if profile and profile.lower() not in (shown.lower(), br.binding.folder.lower(), br.binding.browser.lower()):
                continue
            out.append(br)
        return out

    async def open_tabs(self, profile: str | None = None, task: str | None = None) -> list[dict[str, Any]]:
        """The owner's open tabs in their connected profiles: a reference, the site and whether it's the active one.
        A tab this task already took over is listed with its tab id; FERB's own tabs and other tasks' aren't."""
        names = {(p.browser, p.folder): p.display_name for p in self.profiles()}
        out = []
        for br in self._owner_bridges(profile):
            v = await br.call("list_tabs", {}, timeout=10)
            for t in v.get("tabs", []):
                mine = next((x for x in self._tabs.values() if x.bridge is br and x.chrome_id == int(t["tab_id"])
                             and x.attached and task is not None and x.owner == task), None)
                if mine is None and self.owner_of(br, int(t["tab_id"])) is not None:
                    continue                                  # FERB's own tabs, or another task's
                entry = {"tab_ref": f"{br.binding.browser}:{br.binding.folder}:{t['tab_id']}",
                            "profile": f"{br.binding.browser.capitalize()} “"
                                       f"{names.get((br.binding.browser, br.binding.folder), br.binding.folder)}”",
                            "site": str(t.get("host") or ""), "active": bool(t.get("active"))}
                if mine is not None:
                    entry["taken_over_as"] = mine.ref.tab_id      # already this task's: use this tab id
                out.append(entry)
        return out

    async def attach(self, tab_ref: str, owner: str) -> TabRef:
        """Take over one of the owner's open tabs for this task (their words pointed at their browser)."""
        try:
            browser, folder, chrome_id = tab_ref.rsplit(":", 2)
            cid = int(chrome_id)
        except ValueError:
            raise Refused(f"{tab_ref!r} isn't a tab reference from browser.tabs") from None
        br = self.hub.bridge(browser, folder)
        if br is None or br.conn is None or br.binding.identity is not Identity.OWNER:
            raise Refused("that profile isn't one of the owner's connected profiles")
        existing = next((t for t in self._tabs.values() if t.bridge is br and t.chrome_id == cid), None)
        if existing is not None:
            if existing.owner != owner:
                raise Refused("another task is using that tab")
            return existing.ref
        v = await br.call("list_tabs", {}, timeout=10)
        tab = next((t for t in v.get("tabs", []) if int(t["tab_id"]) == cid), None)
        if tab is None:
            raise Refused("that tab isn't open any more")
        self._n += 1
        ref = TabRef(br.device_id, br.binding.browser, br.binding.folder, br.gen, f"t{self._n}")
        self._tabs[ref.tab_id] = _Tab(ref, br, cid, owner, f"https://{tab.get('host') or ''}/", attached=True)
        return ref

    async def _profile_hint(self, named: str) -> str | None:
        from ferb.decide.decider import Ask
        names = sorted({p.display_name for p in self.profiles()} - {""})[:50]
        if not names:
            return None
        crit = {n: f"the profile “{n}”" for n in names} | {"none": "none of these"}
        try:
            got = await self.jev.ask({"owner_said": named, "profiles": names},
                                     [Ask("browser.profile_match", "none", f"profile:{named}", criteria=crit)])
        except Exception:
            return None
        v = got.get("browser.profile_match")
        return v if v in names else None

    async def check_account(self, tab: TabRef) -> None:
        """Before a consequential action (9.3): the page's signed-in account must be the profile's."""
        t = self._tab(tab)
        host = urlsplit(t.url).hostname or ""
        p = next((p for p in self.profiles() if (p.browser, p.folder) == (tab.browser, tab.profile)), None)
        if p is None or accounts.unverified_key(host, tab.browser, tab.profile, p.email) is not None:
            return                                            # can't be checked: policy asked once instead
        result = await t.bridge.call("account", {"tab_id": t.chrome_id}, timeout=10)
        if result.get("account") == "signed_in_unknown" and host in accounts.ACCOUNT_PAGES:
            helper = await self._open_in(t.bridge, accounts.ACCOUNT_PAGES[host], owner=None)
            try:
                h = self._tab(helper)
                await t.bridge.call("wait", {"tab_id": h.chrome_id, "until": "text", "value": "Signed in as"},
                                    timeout=10)
                result = await t.bridge.call("account", {"tab_id": h.chrome_id}, timeout=10)
            except DriverError:
                pass                                          # still "unknown": refused below, never guessed
            finally:
                await self.close(helper)
        accounts.check(result, host, p.email, f"{tab.browser}/{p.display_name}")

    def tab_profile(self, tab_id: str) -> tuple[str, str, str] | None:
        """(browser, folder, the profile's Chrome account) for the account-check decision in describe()."""
        t = self._tabs.get(tab_id)
        if t is None:
            return None
        p = next((p for p in self.profiles() if (p.browser, p.folder) == (t.ref.browser, t.ref.profile)), None)
        return t.ref.browser, t.ref.profile, p.email if p else ""

    async def fill_secret(self, el: ElementRef, ref: SecretRef, intent_id: str = "") -> ActionResult:
        raise Refused("FERB's secrets are never used in the owner's profiles")

    async def fill_otp(self, el: ElementRef, ref: SecretRef, since: datetime, sender_contains: str | None,
                       intent_id: str = "") -> ActionResult:
        raise Refused("FERB's inbox codes are never used in the owner's profiles")


class FerbIdentity(_TabDriver):
    """FERB's own profile (`ferb_browser.*`, identity FERB), bound by pairing. Secrets and one-time codes reach a
    field only sealed (10.1): the frame's origin is checked against the secret's allowed origins BEFORE the Keychain
    is read, the value is sealed to this fill command and that origin, and the extension re-checks the origin in the
    same in-page call that sets it on the exact element."""
    identity = Identity.FERB

    def __init__(self, hub: Hub, profile: Callable[[], tuple[str, str] | None], secrets: KeychainSecretStore,
                 add_secret_value: Callable[[str], None], mailbox_address: Callable[[], str],
                 pairing_key: Callable[[str], bytes | None], profiles: Callable[[], list[BrowserProfile]],
                 wake: WakeServer | None = None, launch: Callable[[list[str]], None] | None = None,
                 feed_url: str = FEED_URL) -> None:
        super().__init__(hub, profiles, wake, launch)
        self.profile, self.add_secret_value = profile, add_secret_value
        self.secrets: Secrets = secrets                       # names and specs only (tool descriptions, checks)
        self._store = secrets                                 # the Keychain: read only inside _sealed fills
        self.mailbox_address, self.pairing_key, self.feed_url = mailbox_address, pairing_key, feed_url
        self.used_otps: set[str] = set()

    async def _bridge(self) -> Bridge:
        prof = self.profile()
        br = self.hub.bridge(*prof) if prof else None
        if br is None or br.binding.identity is not Identity.FERB:
            raise Refused("FERB's identity profile isn't paired: run `ferb setup --identity`")
        return await self._connected(br)

    async def open_for(self, url: str, owner: str, profile: str | None = None) -> TabRef:
        return await self._open_in(await self._bridge(), url, owner)

    async def open(self, url: str, background: bool = True) -> TabRef:
        return await self.open_for(url, owner="")

    async def check_account(self, tab: TabRef) -> None:
        return None                                           # FERB's own profile: FERB's own accounts

    async def _in_sent(self, tab: TabRef, subject: str, poll_s: float, tries: int) -> bool:
        """The message is in FERB's Sent folder (searched by its exact subject): a real confirmation, not a toast."""
        from urllib.parse import quote
        url = "https://mail.google.com/mail/u/0/#search/" + quote(f'in:sent subject:"{subject}"', safe="")
        for i in range(tries):
            if i % 6 == 0:
                await self.navigate(tab, url)                  # Gmail refreshes search results on navigation
                await asyncio.sleep(1.0)
            snap = await self.snapshot(tab)
            if subject in snap.text and "No messages matched" not in snap.text:
                return True
            await asyncio.sleep(poll_s)
        return False

    async def send_mail(self, to: list[str], subject: str, body: str, owner: str, intent_id: str = "",
                        poll_s: float = 0.5, tries: int = 30) -> str:
        """Send from FERB's own Gmail (build step 14): its compose page in FERB's profile, a real click on Send, and
        Gmail's "Message sent" as the confirmation. No confirmation after the click: outcome unknown (the ledger
        reconciles from FERB's Sent folder; never sent twice)."""
        from urllib.parse import urlencode
        url = "https://mail.google.com/mail/?view=cm&fs=1&" + urlencode({"to": ",".join(to), "su": subject,
                                                                          "body": body})
        tab = await self.open_for(url, owner)
        try:
            send = None
            for _ in range(tries):
                snap = await self.snapshot(tab)
                send = next((e for e in snap.elements if e.role == "button" and e.label.startswith("Send")), None)
                if send is not None:
                    break
                await asyncio.sleep(poll_s)
            if send is None:
                raise Refused("Gmail's compose page didn't load in FERB's profile (is FERB's inbox signed in?)")
            await self.act(self.element_ref(tab, send.id), BrowserAction("click", trusted=True), intent_id)
            for _ in range(tries):                            # Gmail's own toast, when the tab shows it
                snap = await self.snapshot(tab)
                if "Message sent" in snap.text:
                    return "sent"
                await asyncio.sleep(poll_s)
            if await self._in_sent(tab, subject, poll_s, tries * 3):   # a background tab may never show the toast
                return "sent"
            raise OutcomeUnknown("clicked Send but Gmail didn't confirm; check FERB's Sent folder")
        finally:
            await self.close(tab)

    def tab_profile(self, tab_id: str) -> tuple[str, str, str] | None:
        return None

    async def fill_secret(self, el: ElementRef, ref: SecretRef, intent_id: str = "") -> ActionResult:
        if not self.secrets.has(ref):
            raise Refused(f"no secret {ref.key} is set up")
        t, frame, snap, local, _ = self._resolve(el)
        origin = await self._checked_origin(t, frame, ref)
        try:
            value = await self._store._reveal(ref)
        except SecretUnavailable:
            raise Refused(f"secret {ref.key} isn't in the Keychain") from None
        self.add_secret_value(value)                          # scrub it if a page or error ever echoes it back
        r = await self._sealed_fill(t, frame, snap, local, origin, value, intent_id)
        del value
        return ActionResult(r.ok, f"{r.detail} ({ref.key})")

    async def fill_otp(self, el: ElementRef, ref: SecretRef, since: datetime, sender_contains: str | None,
                       intent_id: str = "", timeout: float = 120.0, poll: float = 5.0) -> ActionResult:
        if not self.secrets.has(ref):
            raise Refused(f"no account {ref.key} is set up")
        t, frame, snap, local, _ = self._resolve(el)
        await self._checked_origin(t, frame, ref)
        mailbox = GmailFeedMailbox(self._read_feed, self.mailbox_address(), self.feed_url)
        mailbox.used = self.used_otps
        found = await wait_for_code(mailbox, since, sender_contains, timeout, poll)
        if not found.code:
            return ActionResult(False, REASONS[found.reason])
        self.add_secret_value(found.code)
        t, frame, snap, local, _ = self._resolve(el)          # the page may have moved while we waited
        origin = await self._checked_origin(t, frame, ref)
        r = await self._sealed_fill(t, frame, snap, local, origin, found.code, intent_id)
        if r.ok and found.message_id:
            self.used_otps.add(found.message_id)
        return ActionResult(r.ok, f"{r.detail} (code from FERB's inbox)")

    async def _checked_origin(self, t: _Tab, frame: int, ref: SecretRef) -> str:
        v = await t.bridge.call("frame_origin", {"tab_id": t.chrome_id, "frame_id": frame}, timeout=10)
        origin = str(v.get("origin") or "")
        if not origin_allowed(origin, self.secrets.spec(ref)):
            raise Refused(f"refused: this field's page ({origin or 'unknown origin'}) isn't allowed for {ref.key}")
        return normalize_origin(origin)

    async def _sealed_fill(self, t: _Tab, frame: int, snap: int, local: int, origin: str, value: str,
                           intent_id: str) -> ActionResult:
        key = self.pairing_key(t.bridge.binding.install_id)
        if key is None:
            raise Refused("FERB's identity profile isn't paired")
        cid = intent_id or "f-" + secrets.token_hex(8)
        sealed = seal(seal_key(key), value, cid, origin)
        v = await t.bridge.call("fill", {"tab_id": t.chrome_id, "frame_id": frame, "snap": snap, "el": local,
                                         "sealed": sealed, "origin": origin}, timeout=15, cid=cid)
        if v.get("detail") == "stale":
            raise StaleRef("the element is gone; take a new snapshot")
        return ActionResult(bool(v.get("ok")), str(v.get("detail", "")))

    async def _read_feed(self, url: str) -> str:
        """FERB's inbox feed in a private background tab of FERB's profile (no task owns it)."""
        br = await self._bridge()
        key = self.pairing_key(br.binding.install_id)
        if key is None:
            raise Refused("FERB's identity profile isn't paired")
        ref = await self._open_in(br, url, owner=None)
        try:
            t = self._tab(ref)
            cid = "r-" + secrets.token_hex(8)
            v = await br.call("read_source", {"tab_id": t.chrome_id}, timeout=20, cid=cid)
            return unseal(seal_key(key), v["sealed"], cid, url)       # the feed travels sealed too (test 21)
        finally:
            await self.close(ref)

    async def session_ok(self) -> bool:
        """Signed in as FERB? Can't reach FERB's profile -> raises (that isn't "signed out")."""
        await self._bridge()
        return await GmailFeedMailbox(self._read_feed, self.mailbox_address(), self.feed_url).signed_in()
