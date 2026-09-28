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
from ferb.act.browser.resolver import Resolver
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
            if conn.bridge is not None:
                conn.bridge.drop(conn)
                self.publish("browser.disconnected", {"profile": conn.bridge.name})

    async def _handle(self, conn: _Conn, m: Any) -> None:
        v = self.pairings.handle(m)
        if not v.accepted:
            return                                           # unpaired / bad MAC / replay: dropped, nothing acts
        if v.reply is not None:                              # just paired: hand the key back on this connection
            conn.writer.write(json.dumps(v.reply).encode() + b"\n")
            await conn.writer.drain()
            if v.binding:
                self.publish("browser.profile_paired", {"browser": v.binding.browser, "folder": v.binding.folder,
                                                        "identity": v.binding.identity.value})
            return
        if v.binding is None:
            return                                           # unpaired hello: nothing to do until pairing
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
            await r.readline()
            body = b"<!doctype html><title>FERB</title>"
            w.write(b"HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: %d\r\nconnection: close\r\n\r\n"
                    % len(body) + body)
            await w.drain()
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
        """Wake a sleeping extension with a unique local page in that profile, then close exactly that page."""
        if br.conn is None and self.wake is not None:
            url = self.wake.url()
            kwargs = {"launch": self.launch} if self.launch else {}
            open_known_profile(br.binding.browser, br.binding.folder, url, self.profiles(), **kwargs)
            try:
                await asyncio.wait_for(br.connected.wait(), WAKE_TIMEOUT_S)
            except asyncio.TimeoutError:
                raise DriverError(f"{br.name} didn't connect (is the FERB extension enabled there?)") from None
            try:
                await br.call("wake_close", {"url": url}, timeout=5)
            except DriverError:
                pass
        if br.conn is None:
            raise DriverError(f"{br.name} isn't connected")
        return br

    async def _open_in(self, br: Bridge, url: str, owner: str | None) -> TabRef:
        v = await br.call("open", {"url": url}, timeout=30)
        self._n += 1
        ref = TabRef(br.device_id, br.binding.browser, br.binding.folder, br.gen, f"t{self._n}")
        self._tabs[ref.tab_id] = _Tab(ref, br, int(v["tab_id"]), owner, str(v.get("url") or url))
        return ref

    async def navigate(self, tab: TabRef, url: str) -> None:
        t = self._tab(tab)
        v = await t.bridge.call("navigate", {"tab_id": t.chrome_id, "url": url}, timeout=30)
        t.url = str(v.get("url") or url)

    async def tabs(self) -> list[TabRef]:
        return [t.ref for t in self._tabs.values()]

    async def close(self, tab: TabRef) -> None:
        t = self._tabs.pop(tab.tab_id, None)
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

    async def open_for(self, url: str, owner: str, profile: str | None = None) -> TabRef:
        choice = self.resolver.choose(urlsplit(url).hostname, profile)
        br = self.hub.bridge(choice.browser, choice.folder)
        if br is None:
            raise Refused(f"{choice.browser}/{choice.folder} isn't paired with FERB")
        if br.binding.identity is not Identity.OWNER:
            raise Refused("that profile belongs to FERB's identity, not the owner")
        return await self._open_in(await self._connected(br), url, owner)

    async def open(self, url: str, background: bool = True) -> TabRef:
        return await self.open_for(url, owner="")

    async def check_account(self, tab: TabRef) -> None:
        """Before a consequential action (9.3): the page's signed-in account must be the profile's."""
        t = self._tab(tab)
        host = urlsplit(t.url).hostname or ""
        p = next((p for p in self.profiles() if (p.browser, p.folder) == (tab.browser, tab.profile)), None)
        if p is None or accounts.unverified_key(host, tab.browser, tab.profile, p.email) is not None:
            return                                            # can't be checked: policy asked once instead
        result = await t.bridge.call("account", {"tab_id": t.chrome_id}, timeout=10)
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
        return await GmailFeedMailbox(self._read_feed, self.mailbox_address(), self.feed_url).signed_in()
