"""Owns authenticated extension transport and task-owned browser references; must not choose source/account intent.

Hub binds authenticated installs to profiles. Bridge tracks generation-scoped command IDs, chunks large content,
and records disconnected actions as unknown without replay. Router binds explicit profiles and element references.
FerbIdentity adds origin-bound sealed fills and opaque mailbox candidates. Upload receipts retain local byte identity;
the browser is given FERB's own copy of the file (uploads.py), never the original.
A download caught in a task tab goes to `Hub.on_download` (downloads.py takes it); the drivers say whose tab it is.
WakeServer serves only the temporary local pairing page; ordinary wake launches no window or URL."""

import asyncio
import json
import os
import secrets
from dataclasses import dataclass, field
from pathlib import Path
from stat import S_ISREG
from typing import Any, Awaitable, Callable
from urllib.parse import urlsplit

from ferb.act.browser.ext_protocol import (MIN_EXTENSION_VERSION, PROTOCOL_VERSION, Event, Hello, Op, Result, TabEvent,
                                           command, parse, version_ok)
from ferb.act.browser.downloads import Downloads, TaskTab
from ferb.act.browser.uploads import StagingFailed, Uploads, source_path, tab_key
from ferb.act.browser.pairing import Binding, Pairings
from ferb.act.browser.profiles import BrowserProfile, open_known_profile, running as browser_running
from ferb.act.browser.resolver import Resolver
from ferb.act.browser.snapshot import element as page_element
from ferb.act.browser.tools import Secrets
from ferb.act.fs.files import opened_path
from ferb.act.fs.recovery import sha256_fd
from ferb.identity.mailbox import FEED_URL, GmailFeedMailbox
from ferb.identity.seal import seal, seal_key, unseal
from ferb.identity.secrets import KeychainSecretStore, SecretUnavailable, normalize_origin, origin_allowed
from ferb.core.errors import DriverError, Refused, StaleRef, Withheld
from ferb.core.types import (ActionResult, BrowserAction, DriverFeatures, ElementRef, Identity, PageElement,
                             PageSnapshot, SecretRef, SecretSpec, TabRef, ToolContext, WaitCondition)
from ferb.orchestrator.ledger import Ledger

Publish = Callable[[str, dict[str, Any]], None]
CMD_TIMEOUT_S = 30.0
WINDOWLESS_WAIT_S = 8.0          # a profile loaded with no window: its extension connects by then (spike 19)
KEEPALIVE_S = 15.0               # while a task tab is open: a ping keeps the MV3 worker awake (idle limit 30 s, 1.1)
TRUSTED_KINDS = frozenset({"click", "dblclick", "rightclick", "hover", "type", "press"})   # real input by default
BLANK = "about:blank"            # a task tab opens blank, then loads its URL (D35)
# a media element's state (L26): `playing` once its time moves (I5), `paused` as the page has it
MEDIA_FIELDS = ("playing", "paused", "current_time", "duration", "muted", "volume", "in_view")


class OutcomeUnknown(DriverError):
    """The connection dropped while a command was in flight: it may or may not have happened."""


@dataclass
class _Conn:
    writer: asyncio.StreamWriter
    install_id: str | None = None
    bridge: "Bridge | None" = None
    gen: int = 0
    unpaired_install: str | None = None
    browser_pid: int | None = None          # the browser process behind this native host (L26: hear its tab's app)


def _browser_pid(writer: Any) -> int | None:
    """The browser process behind a native-host connection: the native host's parent (the browser starts it, and
    FERB's launcher execs Python in place). Kernel facts (LOCAL_PEERPID, libproc), never a name or window lookup."""
    import ctypes
    import socket as socket_mod
    import struct
    try:
        sock = writer.get_extra_info("socket")
        if not isinstance(sock, socket_mod.socket):
            sock = getattr(sock, "_sock", sock)
        host = int(sock.getsockopt(0, 0x002))                 # SOL_LOCAL, LOCAL_PEERPID
        info = ctypes.create_string_buffer(136)               # struct proc_bsdinfo: pbi_ppid at offset 16
        if ctypes.CDLL("/usr/lib/libproc.dylib").proc_pidinfo(host, 3, ctypes.c_uint64(0), info, 136) != 136:
            return None
        parent = int(struct.unpack_from("I", info.raw, 16)[0])
        return parent if parent > 1 else None
    except Exception:
        return None


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
    detached: dict[int, str] = field(default_factory=dict)  # chrome tab id -> why the debugger left (Cancel, ...)
    holds: int = 0                                           # task tabs open on this bridge (keepalive while > 0)
    hellos: int = 0                                          # hellos attached so far (pairing waits for a new one)
    _keepalive: "asyncio.Task[None] | None" = None

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
        wire = json.dumps(command(cid, conn.gen, op, args, int(timeout * 1000)))
        try:
            if len(wire) > 500000:
                # Native messaging has a per-message limit, not a deliverable limit. Only the fully
                # assembled original command can perform an action; chunks have separate transport IDs.
                for offset in range(0, len(wire), 100000):
                    await self.call("command_part", {"id":cid,"offset":offset,"text":wire[offset:offset+100000],
                                                     "total_chars":len(wire)}, timeout=timeout)
            else:
                conn.writer.write(wire.encode() + b"\n")
            await conn.writer.drain()
            r = await asyncio.wait_for(fut, timeout + 5)
        except asyncio.CancelledError:
            self.lost.add(cid)
            if op == "pointer":
                await self._cancel_pointer(args, cid)
            raise
        except asyncio.TimeoutError:
            if op == "pointer":
                await self._cancel_pointer(args, cid)
            self.lost.add(cid)                            # the next hello's seen / owned ids settle it (never resent)
            raise OutcomeUnknown(f"{op} timed out in {self.name}") from None
        finally:
            self.pending.pop(cid, None)
        if r.status == "ok":
            if isinstance(r.value, dict) and "result_parts" in r.value:
                transfer, pieces, offset = r.value["result_parts"], [], 0
                while offset is not None:
                    part = await self.call("result_part", {"id": transfer["id"], "offset": offset}, timeout=timeout)
                    pieces.append(part["text"])
                    offset = part["next_offset"]
                return json.loads("".join(pieces))
            return r.value
        if r.status == "stale":
            raise StaleRef("the browser reconnected; take a new snapshot")
        if r.status == "duplicate":
            raise DriverError(f"{op} was already sent once; not repeating it")
        if r.outcome == "unknown" or r.error == "timeout" or str(r.error or "").startswith("detached:"):
            self.lost.add(cid)
            raise OutcomeUnknown(f"{op}: {r.error}; outcome unknown")
        raise DriverError(r.error or f"{op} failed")

    async def _cancel_pointer(self, args: dict[str, Any], cid: str) -> None:
        try:
            await asyncio.shield(self.call("pointer_cancel", {"tab_id": args["tab_id"], "command_id": cid}, timeout=2))
        except (Exception, asyncio.CancelledError):
            pass                                    # disconnected: the extension also cancels on disconnect

    def attach(self, conn: _Conn, hello: Hello) -> None:
        self.hellos += 1
        if conn is self.conn:                            # a second hello on the same connection (e.g. right after
            conn.bridge, self.version = self, hello.version   # pairing): same generation, in-flight commands stay
            self._reconcile(hello)
            return
        self.gen += 1
        conn.gen, conn.bridge = self.gen, self
        old, self.conn, self.version = self.conn, conn, hello.version
        self.detached.clear()
        if old is not None:
            self.drop(old)
        self._reconcile(hello)
        self.connected.set()
        self._keep()

    def drop(self, conn: _Conn) -> None:
        if self.conn is conn:
            self.conn = None
            self.connected.clear()
            if self._keepalive is not None:
                self._keepalive.cancel()
        for cid, fut in list(self.pending.items()):
            if not fut.done():
                self.lost.add(cid)
                fut.set_exception(OutcomeUnknown(f"{self.name} disconnected mid-command; outcome unknown"))
        self.pending.clear()

    def hold(self) -> None:
        """A task tab is open here: keep the extension's worker awake until the last one closes."""
        self.holds += 1
        self._keep()

    def unhold(self) -> None:
        self.holds = max(0, self.holds - 1)
        if self.holds == 0 and self._keepalive is not None:
            self._keepalive.cancel()

    def _keep(self) -> None:
        if self.holds > 0 and self.conn is not None and (self._keepalive is None or self._keepalive.done()):
            self._keepalive = asyncio.get_running_loop().create_task(self._keepalive_loop())

    async def _keepalive_loop(self) -> None:
        while self.holds > 0 and self.conn is not None:
            try:
                await self.call("ping", {}, timeout=5)
            except (DriverError, OSError):                # a dropped connection: the next hello restarts the loop
                pass
            await asyncio.sleep(KEEPALIVE_S)

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
            elif seen.get(cid) == "failed":
                self.ledger.reconcile(cid, "failed", [f"{self.name} reported failure after reconnecting"])
            else:
                continue  # a bounded/reloaded journal cannot prove that a missing command never ran
            self.lost.discard(cid)


class Hub:
    def __init__(self, pairings: Pairings, ledger: Ledger, publish: Publish, device_id: Callable[[], str]) -> None:
        self.pairings, self.ledger, self.publish, self.device_id = pairings, ledger, publish, device_id
        self.bridges: dict[tuple[str, str], Bridge] = {}
        self.refused: dict[str, str] = {}                    # install_id -> why (e.g. protocol mismatch)
        self.unpaired: dict[str, str] = {}                   # install_id -> browser: installed, connected, not paired
        # The activity trail: every tab event goes to on_tab; while paused, only tabs FERB owns are tracked.
        self.on_tab: Callable[["Bridge", TabEvent], None] = lambda br, e: None
        # A tab closed: FERB's own resources tied to it (upload copies) can go; nothing about the tab is recorded, so
        # this runs while sensing is paused too. A returned awaitable runs in the background.
        self.on_tab_closed: Callable[["Bridge", int], Awaitable[None] | None] = lambda br, tab_id: None
        self.paused: Callable[[], bool] = lambda: False
        self.owned: Callable[["Bridge", int], bool] = lambda br, tab_id: False
        # A download the extension holds (D35); the daemon sets downloads.Downloads.receive. Unset: back to the browser.
        self.on_download: Callable[["Bridge", Event], None] = _let_download_go
        self.drivers: list["_TabDriver"] = []               # the browser drivers (they register): whose tab is it
        self._jobs: set["asyncio.Task[Any]"] = set()

    def task_tab(self, br: "Bridge", chrome_id: int) -> TaskTab | None:
        """The live task that drives this tab now, in any driver; None: the extension must not hold its downloads."""
        return next((tt for d in self.drivers if (tt := d.task_tab(br, chrome_id)) is not None), None)

    def _spawn(self, coro: Awaitable[Any]) -> None:
        async def quiet() -> None:
            try:
                await coro
            except Exception:
                pass                                    # best effort: a dropped connection ends it anyway
        job = asyncio.get_running_loop().create_task(quiet())
        self._jobs.add(job)
        job.add_done_callback(self._jobs.discard)

    def _disarm_unheld(self, br: "Bridge", armed: list[int]) -> None:
        """After a reconnect or a daemon restart: let go of every tab the extension still holds downloads for that
        isn't a live task tab here (a failed release, a task that ended while disconnected). Never closes a tab."""
        for tab_id in armed:
            if self.task_tab(br, int(tab_id)) is None:
                self._spawn(br.call("disarm", {"tab_id": int(tab_id)}, timeout=10))

    def _on_open(self, br: "Bridge", ev: Event) -> None:
        self._spawn(self._open(br, ev))

    async def _open(self, br: "Bridge", ev: Event) -> None:
        """A task tab's link that opens a new tab. First the answer the extension waits for (open_seen, ~1 s): "expired"
        means the wait ran out and the browser already opened the link, so FERB opens nothing. Then FERB opens it itself
        (blank, armed, then loaded, inactive), as the same task's tab. No live task (a stale listener caught the owner's
        own click): the browser opens it as the owner would have had it, and the tab is let go of."""
        if ev.open_id:
            seen = await br.call("open_seen", {"open_id": ev.open_id}, timeout=10)
            if not (isinstance(seen, dict) and seen.get("seen") is True):
                return
        for d in self.drivers:
            if d.task_tab(br, ev.tab_id) is not None:
                await d.open_from(br, ev.tab_id, str(ev.url or ""), ev.window_id)
                return
        await _pass_through(br, ev.tab_id, str(ev.url or ""), active=not ev.background)

    def _on_popup(self, br: "Bridge", ev: Event) -> None:
        """A popup the extension claimed by its page-hook token for a live task tab: the same task's tab from now on
        (within the task's tab cap), then the page's hook loads its URL (popup_ready); FERB never navigates it. Else
        let go of it, and the hook loads the URL itself after 1.5 s."""
        for d in self.drivers:
            if ev.opener_tab_id is not None and d.task_tab(br, ev.opener_tab_id) is not None:
                if d.adopt(br, ev.opener_tab_id, ev.tab_id, str(ev.url or "")):
                    self._spawn(br.call("popup_ready", {"tab_id": ev.tab_id}, timeout=10))
                else:
                    self._spawn(br.call("disarm", {"tab_id": ev.tab_id}, timeout=10))
                return
        self._spawn(br.call("disarm", {"tab_id": ev.tab_id}, timeout=10))

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
        conn = _Conn(writer, browser_pid=_browser_pid(writer))
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
            if m.type == "hello" and v.reason in {"bad_mac", "replay"}:
                why = f"extension pairing failed ({v.reason}); run ferb browsers pair for this profile"
                self.refused[m.install_id] = why
                conn.writer.write(json.dumps({"type": "refused", "reason": why}).encode() + b"\n")
                await conn.writer.drain()
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
                conn.writer.write(json.dumps({"type": "pairing_required"}).encode() + b"\n")
                await conn.writer.drain()
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
            if hello.protocol != PROTOCOL_VERSION or not version_ok(hello.version):
                why = (f"extension {hello.version} (protocol {hello.protocol}), FERB needs {MIN_EXTENSION_VERSION} "
                       f"(protocol {PROTOCOL_VERSION}): reload the FERB extension")
                self.refused[m.install_id] = why
                conn.writer.write(json.dumps({"type": "refused", "reason": why,
                                              "required_protocol": PROTOCOL_VERSION,
                                              "minimum_version": MIN_EXTENSION_VERSION}).encode() + b"\n")
                await conn.writer.drain()
                return
            self.refused.pop(m.install_id, None)
            br = self.bridge(v.binding.browser, v.binding.folder)
            if br is None:
                return
            br.attach(conn, hello)
            self.pairings.db.conn.execute("UPDATE ext_installs SET ext_version = ? WHERE install_id = ?",
                                          (hello.version, m.install_id))
            # identity: in FERB's own profile every download is FERB's, so a hold the daemon never takes is failed, not
            # handed to the browser (D35).
            conn.writer.write(json.dumps({"type": "welcome", "gen": br.gen,
                                          "identity": br.binding.identity.value}).encode() + b"\n")
            await conn.writer.drain()
            self.publish("browser.connected", {"profile": br.name, "version": hello.version, "gen": br.gen})
            self._disarm_unheld(br, hello.armed)
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
        elif m.type == "event":
            try:
                ev = Event.model_validate(body)
            except ValueError:
                return
            if ev.event == "detached":
                br.detached[ev.tab_id] = ev.reason or "unknown"
                self.publish("browser.debugger_detached", {"profile": br.name, "tab_id": ev.tab_id,
                                                           "reason": ev.reason or "unknown",
                                                           "by_owner": ev.reason == "canceled_by_user"})
            elif ev.event == "file_chooser":
                self.publish("browser.file_chooser", {"profile": br.name, **ev.model_dump(exclude_none=True)})
            elif ev.event == "download":
                self.on_download(br, ev)                     # its parts come back on this connection: never awaited
            elif ev.event == "open":
                self._on_open(br, ev)
            elif ev.event == "popup":
                self._on_popup(br, ev)
            else:
                self.publish("browser.dialog", {"profile": br.name, "tab_id": ev.tab_id, "type": ev.type,
                                                "message": ev.message or "", "handled": ev.handled})
        elif m.type == "tab_event":
            try:
                e = TabEvent.model_validate(body)
            except ValueError:
                return
            if e.event == "removed" and (job := self.on_tab_closed(br, e.tab_id)) is not None:
                self._spawn(job)
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


_cancels: set["asyncio.Task[None]"] = set()


async def _pass_through(br: Bridge, tab_id: int, url: str, active: bool = True) -> None:
    """The owner's own new-tab click caught by a listener in a tab no task drives any more: opened as they asked (cmd/ctrl
    or middle click: in the background; with shift, or a plain target=_blank click: in front, as the browser would), the
    tab disarmed."""
    try:
        await br.call("open_plain", {"tab_id": tab_id, "url": url, "active": active}, timeout=10)
    finally:
        await br.call("disarm", {"tab_id": tab_id}, timeout=10)


def _let_download_go(br: Bridge, ev: Event) -> None:
    """No download handler: the held response goes back to the browser untouched."""
    if ev.uncaptured or not ev.download_id:
        return

    async def release() -> None:
        try:
            await br.call("download_part", {"download_id": ev.download_id, "release": True}, timeout=10)
        except Exception:
            pass
    job = asyncio.get_running_loop().create_task(release())
    _cancels.add(job)
    job.add_done_callback(_cancels.discard)


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

def painted(jpeg: bytes, reply: dict[str, Any]) -> bytes:
    """A tab capture with every secret field the extension reported painted over (L13). Its `mask` is the page's mask
    state before and after the capture (mask.page_masks merges them), in visual-viewport CSS px, scaled onto the
    image (device pixels; page and pinch zoom included). Withheld when the extension couldn't read the page (its
    `withheld` says why, in its own words, never page content), didn't say, a field's place in the picture is
    unknown, or the image can't be painted."""
    from ferb.act.evidence import mask
    if reply.get("withheld"):
        raise Withheld(f"pixels withheld: {reply['withheld']}")
    m = reply.get("mask")
    before, after = (mask.PageState.parse(m.get("before")), mask.PageState.parse(m.get("after"))) \
        if isinstance(m, dict) else (None, None)
    if before is None or after is None:
        raise Withheld("pixels withheld: the extension didn't say where the page's secret fields are")
    rects, viewport = mask.page_masks(before, after)
    if not rects:
        return jpeg
    try:
        boxes = mask.to_image_boxes(rects, viewport, mask.size(jpeg))
        return mask.mask_secret_fields(jpeg, boxes) if boxes else jpeg
    except (ValueError, RuntimeError):
        raise Withheld("pixels withheld: the capture couldn't be painted over") from None


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
    snapshot_json: str = ""
    searches: dict[str, tuple[str, int, int]] = field(default_factory=dict)
    snap_gen: int = 0
    visual: tuple[str, int, int, float, int, int] | None = None
    points: dict[str, str] = field(default_factory=dict)
    snap_nav: int = -1
    attached: bool = False          # the owner's own tab, taken over for a task: forgotten after, never closed
    keep_open: bool = False         # explicit handoff requested; uncertain ACK must not cause a later close
    opened: list[dict[str, Any]] = field(default_factory=list)   # new tabs this tab started, not yet reported
    containers: dict[str, tuple[int, str]] = field(default_factory=dict)
    elements: dict[str, tuple[int, int, int, PageElement]] = field(default_factory=dict)  # id -> frame, snap, local
    media: dict[int, tuple[int, int]] = field(default_factory=dict)   # snapshot media index -> frame, local index
    masking: str = ""               # how the latest screenshot's secret fields were found, when worth saying (L13)


class _TabDriver:
    seal_value = staticmethod(seal)
    """Task-owned tabs with short aliases and generation-scoped element ids, over one or more bridges."""
    name = "ext"
    identity: Identity
    features = DriverFeatures(trusted_input_background=True, parallel_actions=True, network_events=False,
                              dom_snapshot=True, screenshot_background=True)

    def __init__(self, hub: Hub, profiles: Callable[[], list[BrowserProfile]], wake: WakeServer | None = None,
                 launch: Callable[[list[str]], None] | None = None) -> None:
        self.hub, self.profiles, self.wake, self.launch = hub, profiles, wake, launch
        self.is_running: Callable[[str], bool] = browser_running  # is that browser running (tests replace it)
        self._tabs: dict[str, _Tab] = {}
        self.inspection_parent: Callable[[str], str | None] = lambda task: None  # registered checker, daemon supplied
        self.downloads: Downloads | None = None         # D35: shared by both drivers, daemon supplied
        self.uploads: Uploads | None = None             # rule 17: daemon supplied; the browser reads FERB's copy
        self.task_running: Callable[[str], bool] = lambda task: True   # daemon supplied: the task hasn't ended
        self.task_root: Callable[[str], str] = lambda task: task      # daemon supplied: the task tree (tab cap)
        self.max_tabs: Callable[[], int] = lambda: 50                 # daemon supplied: browser.task_max_tabs
        self._tabs_opened: dict[str, int] = {}                        # task tree -> tabs FERB opened from its pages
        self._released: set[str] = set()                              # tasks that let go of their tabs
        # daemon supplied (B6): (browser pid, window bounds) -> the secret fields Accessibility finds in that window's
        # one page area, that area, and whether every field it found has a box; None: it can't tell
        self.native_secrets: Callable[[int, tuple[float, float, float, float]],
                                      Awaitable[tuple[list[tuple[float, float, float, float]],
                                                      tuple[float, float, float, float], bool] | None]] | None = None
        self._n = 0
        if isinstance(getattr(hub, "drivers", None), list):
            hub.drivers.append(self)

    async def _connected(self, br: Bridge) -> Bridge:
        """Try loading the profile without a window; never create a visible wake tab to force a connection."""
        if br.conn is None and self.wake is not None:
            kwargs: dict[str, Any] = {"launch": self.launch} if self.launch else {}
            kwargs["is_running"] = self.is_running
            # Never restore a previously focused app after this wait: the owner can switch apps meanwhile.
            open_known_profile(br.binding.browser, br.binding.folder, None, self.profiles(), **kwargs)
            try:
                await asyncio.wait_for(br.connected.wait(), WINDOWLESS_WAIT_S)
            except asyncio.TimeoutError:
                reason = self.hub.refused.get(br.binding.install_id) or "reload the FERB extension in that profile"
                raise DriverError(f"{br.name} didn't connect without opening a window; {reason}. "
                                  "No visible wake tab was opened.") from None
        if br.conn is None:
            raise DriverError(f"{br.name} isn't connected")
        return br

    async def _open_in(self, br: Bridge, url: str, owner: str | None, *,
                       window_id: int | None = None, allow_foreground: bool = False,
                       registered: Callable[[TabRef], None] | None = None, attached: bool = False,
                       abandon: Callable[[], bool] | None = None) -> TabRef:
        # The extension opens an inactive tab in the selected existing window or returns window choices. App focus belongs to the owner,
        # including while this command is pending; a newly foreground browser is not proof of focus theft.
        # D35: the tab opens blank (the extension prepares it to hold a download) and becomes this task's tab before the
        # URL loads, so a URL that is a download is already the task's download, never the browser's.
        args: dict[str, Any] = {"url": BLANK}
        if window_id is not None:
            args["window_id"] = window_id
        if allow_foreground:
            args["allow_foreground"] = True
        v = await br.call("open", args, timeout=30)
        if v.get("opened") is False:
            raise DriverError(str(v["detail"]), facts=v)
        self._n += 1
        ref = TabRef(br.device_id, br.binding.browser, br.binding.folder, br.gen, f"t{self._n}")
        t = self._tabs[ref.tab_id] = _Tab(ref, br, int(v["tab_id"]), owner, BLANK, attached=attached)
        if not attached:
            br.hold()
        if registered is not None:
            registered(ref)
        if abandon is not None and abandon():            # the task let go of its tabs meanwhile: never load the URL
            await self._orphaned(ref)
            raise DriverError("the task had already let go of its tabs; the new tab was closed before it loaded")
        try:
            loaded = await br.call("navigate", {"tab_id": t.chrome_id, "url": url}, timeout=30)
            if loaded.get("navigated") is False:
                raise DriverError(str(loaded.get("detail") or "the tab didn't load"), facts=loaded)
        except BaseException as e:                       # never a blank task tab the worker doesn't know about
            try:
                await asyncio.shield(self.close(ref))
            except (DriverError, OSError) as cleanup:
                if isinstance(e, DriverError):
                    e.facts.update(tab=ref.tab_id, tab_left_open=f"the blank tab couldn't be closed: {cleanup}")
            raise
        t.url = str(loaded.get("url") or url)
        return ref

    def owner_of(self, br: Bridge, chrome_id: int) -> str | None:
        """The task that opened this tab, if FERB did."""
        return next((t.owner for t in self._tabs.values() if t.bridge is br and t.chrome_id == chrome_id), None)

    def _live(self, br: Bridge, chrome_id: int) -> _Tab | None:
        """This driver's tab with that browser id, driven by a task that is still running (opened or borrowed)."""
        return next((t for t in self._tabs.values() if t.bridge is br and t.chrome_id == chrome_id and t.owner
                     and self.task_running(t.owner)), None)

    def task_tab(self, br: Bridge, chrome_id: int) -> TaskTab | None:
        """The live task that drives this tab now, for a download caught in it. A tab whose task ended, or whose
        release failed, is no task's: its downloads go back to the browser (D35, rule 17)."""
        t = self._live(br, chrome_id)
        return TaskTab(str(t.owner), t.ref.tab_id, br.binding.identity.value) if t else None

    async def downloads_for(self, tab: TabRef, wait: float = 0.0) -> list[dict[str, Any]]:
        """Downloads this tab started that the worker hasn't seen yet; `wait` bounds how long to let ones in progress
        finish (a navigation or open that became a download)."""
        t = self._tab(tab)
        if self.downloads is None:
            return []
        if wait:
            await self.downloads.settle(t.bridge, t.chrome_id, wait)
        return self.downloads.unreported(t.bridge, t.chrome_id)

    def opened_for(self, tab: TabRef) -> list[dict[str, Any]]:
        """New tabs this tab started (a link to a new tab, a page's window.open), now this task's, not yet reported."""
        t = self._tabs.get(tab.tab_id)
        if t is None:
            return []
        out, t.opened = t.opened, []
        return out

    def _count_tab(self, owner: str) -> bool:
        """One more tab FERB opens for this task tree (routed links and adopted popups), within browser.task_max_tabs."""
        root = self.task_root(owner)
        if self._tabs_opened.get(root, 0) >= self.max_tabs():
            return False
        self._tabs_opened[root] = self._tabs_opened.get(root, 0) + 1
        return True

    async def _orphaned(self, ref: TabRef) -> None:
        """A tab registered after its task let go of its tabs: let go of it too."""
        try:
            await self.close(ref)
        except DriverError:
            pass

    async def open_from(self, br: Bridge, opener_id: int, url: str, window_id: int | None) -> None:
        """A task tab's link to a new tab: opened by FERB (blank, armed, loaded, inactive) in the opener's window, as
        that task's tab; the opener's next result names it. Opened from the owner's borrowed tab, it is released (never
        closed) like its opener. Past the task's tab cap the browser opens it as an ordinary tab, and the opener's next
        result says so."""
        opener = self._live(br, opener_id)
        if opener is None or opener.owner is None:
            return
        owner = opener.owner
        from ferb.act.browser.tools import check_url
        entry: dict[str, Any] = {"url": url, "via": "link"}
        opener.opened.append(entry)                      # named at once: its load may become a long download
        if not self._count_tab(owner):
            entry.update(error=f"this task already opened {self.max_tabs()} tabs (browser.task_max_tabs): the "
                               "browser opened this link as an ordinary tab, not the task's")
            try:
                await br.call("open_plain", {"tab_id": opener_id, "url": url, "active": False}, timeout=10)
            except DriverError as e:
                entry["error"] += f" (it couldn't: {e})"
            return
        try:
            check_url(url)
            await self._open_in(br, url, owner, window_id=window_id, attached=opener.attached,
                                registered=lambda ref: entry.update(tab=ref.tab_id),
                                # let go of (release) or ended without one (its record gone): never load it as the task's
                                abandon=lambda: owner in self._released or not self.task_running(owner))
        except (Refused, DriverError) as e:
            entry.update(error=f"FERB couldn't open the link's new tab: {e}")

    def adopt(self, br: Bridge, opener_id: int, chrome_id: int, url: str) -> bool:
        """A popup the page hook of a live task tab opened (claimed by its token): that task's tab, released (never
        closed) when the opener was the owner's borrowed tab. False: not the task's (past the tab cap, or the task let
        go of its tabs)."""
        opener = self._live(br, opener_id)
        if opener is None or opener.owner is None or opener.owner in self._released:
            return False
        if any(t.bridge is br and t.chrome_id == chrome_id for t in self._tabs.values()):
            return True
        if not self._count_tab(opener.owner):
            opener.opened.append({"url": url, "error": f"this task already opened {self.max_tabs()} tabs "
                                  "(browser.task_max_tabs): the page's new tab was left to the browser"})
            return False
        self._n += 1
        ref = TabRef(br.device_id, br.binding.browser, br.binding.folder, br.gen, f"t{self._n}")
        self._tabs[ref.tab_id] = _Tab(ref, br, chrome_id, opener.owner, url, attached=opener.attached)
        if not opener.attached:
            br.hold()
        opener.opened.append({"tab": ref.tab_id, "url": url, "via": "the page opened it"})
        return True

    def downloads_of(self, task: str) -> list[dict[str, Any]]:
        """Every download this task's tabs in this identity's profiles started; a registered checker sees its
        worker's."""
        if self.downloads is None:
            return []
        tasks = {task} | ({p} if (p := self.inspection_parent(task)) else set())
        return self.downloads.facts_for(tasks, self.identity.value)

    async def navigate(self, tab: TabRef, url: str) -> None:
        t = self._tab(tab)
        v = await t.bridge.call("navigate", {"tab_id": t.chrome_id, "url": url}, timeout=30)
        if v.get("navigated") is False:                  # e.g. download_capture_unavailable: the URL never loaded
            raise DriverError(str(v.get("detail") or "the tab didn't navigate"), facts=v)
        t.url = str(v.get("url") or url)

    async def tabs(self) -> list[TabRef]:
        return [t.ref for t in self._tabs.values()]

    def _lifecycle(self, t: _Tab, result: dict[str, Any]) -> dict[str, Any]:
        return {"tab": t.ref.tab_id, "tab_ref": f"{t.ref.browser}:{t.ref.profile}:{t.chrome_id}",
                "profile": f"{t.ref.browser}/{t.ref.profile}", "url": result.get("url") or t.url,
                "borrowed": t.attached, "closed": bool(result.get("closed")),
                "kept_open": bool(result.get("kept_open")), "released": bool(result.get("released") or result.get("closed"))}

    async def keep_open(self, tab: TabRef) -> dict[str, Any]:
        t = self._tab(tab)
        t.keep_open = True
        v = await t.bridge.call("release_tab", {"tab_id": t.chrome_id, "keep_open": True}, timeout=10)
        if not v.get("released") or not v.get("kept_open"):
            raise DriverError("browser did not confirm keeping the tab open and releasing control")
        return self._lifecycle(t, v)

    async def close(self, tab: TabRef) -> dict[str, Any]:
        t = self._tabs.get(tab.tab_id)
        if t is None:
            return {"tab": tab.tab_id, "released": True, "closed": False, "already_released": True}
        if t.bridge.conn is None:
            raise DriverError(f"cannot confirm cleanup of {tab.tab_id}: {t.bridge.name} is disconnected")
        if t.attached or t.keep_open:
            v = await t.bridge.call("release_tab", {"tab_id": t.chrome_id, "keep_open": t.keep_open}, timeout=10)
            if not v.get("released") or (t.keep_open and not v.get("kept_open")):
                raise DriverError(f"browser did not confirm releasing {tab.tab_id}")
        else:
            # An old connection's tab reference must not close a different browser session's tab.
            if t.bridge.gen != tab.generation:
                raise DriverError(f"cannot confirm cleanup of {tab.tab_id}: browser connection changed")
            v = await t.bridge.call("close", {"tab_id": t.chrome_id}, timeout=10)
            if not v.get("closed") and not (v.get("kept_open") and v.get("released")):
                raise DriverError(f"browser did not confirm closing {tab.tab_id}")
        self._tabs.pop(tab.tab_id, None)
        if not t.attached:
            t.bridge.unhold()
        if self.uploads is not None and v.get("closed"):
            self.uploads.closed(tab_key(t.bridge.name, t.chrome_id))   # rule 17: its upload copies go with its tree
        return self._lifecycle(t, v)

    async def snapshot(self, tab: TabRef) -> PageSnapshot:
        t = self._tab(tab)
        nav = t.bridge.navs.get(t.chrome_id, 0)
        gen = t.snap_gen + 1
        v = await t.bridge.call("snapshot", {"tab_id": t.chrome_id, "snap": gen}, timeout=30)
        elements: dict[str, tuple[int, int, int, PageElement]] = {}
        containers: dict[str, tuple[int, str]] = {}
        text, title, url, truncated = "", "", t.url, False
        scroll: dict[str, Any] | None = None
        scrollers: list[dict[str, Any]] = []
        media: list[dict[str, Any]] = []
        media_refs: dict[int, tuple[int, int]] = {}
        for f in v["frames"]:
            fid = int(f["frame_id"])
            for m in f.get("media", []):               # L26: one index across frames, as browser.media takes it
                media_refs[len(media)] = (fid, int(m["index"]))
                media.append({k: m[k] for k in MEDIA_FIELDS if k in m} | {"index": len(media)}
                             | ({"frame": fid} if fid else {}))
            for sc in f.get("scrollers", []):
                local = str(sc['id'])
                alias = local if fid == 0 else f"f{fid}:{local}"
                containers[alias] = (fid, local)
                scrollers.append({k: sc[k] for k in ("label", "y", "height", "viewport_h", "at_end") if k in sc}
                                 | {"id": alias, "frame": fid})
            truncated = truncated or bool(f.get("truncated"))
            for raw in f.get("elements", []):
                eid = f"s{gen}e{len(elements) + 1}"
                if fid and raw.get("container"):
                    raw = dict(raw, container=f"f{fid}:{raw['container']}")
                elements[eid] = (fid, gen, int(raw["id"]), page_element(eid, str(fid), raw))
            if fid == 0:
                text, title, url = f.get("text", ""), f.get("title", ""), f.get("url", url)
                scroll = f.get("scroll") or None
        child_text = [str(f.get("text", "")) for f in v["frames"] if int(f["frame_id"]) != 0 and f.get("text")]
        if child_text:
            text = text + "\n[Embedded frames]\n" + "\n".join(child_text)
        dialogs = [f"{d.get('type')}: {str(d.get('message') or '')} ({d.get('handled')})"
                   for d in v.get("dialogs", []) if isinstance(d, dict)]
        t.visual = None
        t.points.clear()
        t.snap_gen, t.snap_nav, t.elements, t.url = gen, nav, elements, url
        t.containers, t.media = containers, media_refs
        result = PageSnapshot(tab, gen, url, title, [e for *_, e in elements.values()], text, truncated, dialogs,
                              scroll, scrollers, media)
        from ferb.act.browser.snapshot import for_model
        full = for_model(result, full=True)
        full["frames"] = [{k: f[k] for k in ("frame_id", "url", "text", "truncated") if k in f} for f in v["frames"]]
        t.snapshot_json = json.dumps(full, ensure_ascii=False)
        return result

    async def read(self, tab: TabRef, offset: int = 0, max_chars: int = 12000) -> dict[str, Any]:
        t = self._tab(tab)
        if not t.snapshot_json:
            raise Refused("take a browser snapshot before reading its full evidence")
        end = min(offset + max_chars, len(t.snapshot_json))
        return {"tab": tab.tab_id, "snapshot": t.snap_gen, "text": t.snapshot_json[offset:end], "offset": offset,
                "next_offset": end if end < len(t.snapshot_json) else None, "total_chars": len(t.snapshot_json)}

    async def dialog(self, tab: TabRef, decision: str | None = None, text: str | None = None) -> dict[str, Any]:
        t = self._tab(tab)
        return dict(await t.bridge.call("dialog", {"tab_id":t.chrome_id, "decision":decision,
                                                   **({"text":text} if text is not None else {})}, timeout=10))

    async def act(self, el: ElementRef, action: BrowserAction, intent_id: str) -> ActionResult:
        t, frame, snap, local, info = self._resolve(el)
        if el.element_id in t.points or action.kind == "drag":
            return await self._pointer(t, el, action, intent_id)
        t.visual = None
        if action.kind == "type" and info.secret:
            raise Refused("that's a password/code field; FERB never types into it")
        if action.kind == "upload":
            return await self._upload(t, frame, snap, local, action.files, intent_id, action.files_closed)
        args: dict[str, Any] = {"tab_id": t.chrome_id, "frame_id": frame, "snap": snap, "el": local,
                                "kind": action.kind, "text": action.text,
                                # real (debugger-dispatched) input is the default; page events on request
                                "page_mode": bool(action.page_mode and not action.trusted and action.kind in TRUSTED_KINDS)}
        if action.expect_dialog:
            args["expect_dialog"] = action.expect_dialog
            if action.dialog_text is not None:
                args["dialog_text"] = action.dialog_text
        try:
            v = await t.bridge.call("act", args, timeout=30, cid=intent_id or None)
        except DriverError as e:
            if str(e).startswith("detached:"):
                raise OutcomeUnknown(self._detached_msg(str(e)[9:])) from None
            raise
        if v.get("detail") == "stale":
            raise StaleRef(f"element {el.element_id} is gone; take a new snapshot")
        return ActionResult(bool(v.get("ok")), str(v.get("detail", "")),
                            {k:value for k,value in v.items() if k not in {"ok","detail"}})

    async def _pointer(self, t: _Tab, el: ElementRef, action: BrowserAction, intent_id: str) -> ActionResult:
        if action.page_mode or action.kind not in {"click", "dblclick", "rightclick", "hover", "drag"}:
            raise Refused("visual points support trusted pointer actions; take a semantic snapshot for typing")
        v = t.visual
        if not v or v[1] != t.snap_gen or v[2] != t.bridge.navs.get(t.chrome_id, 0) or \
                asyncio.get_running_loop().time() - v[3] > 30:
            raise StaleRef("the visual target is stale; take a fresh screenshot and resolve its points")
        start = t.points.get(el.element_id)
        end = t.points.get(action.end_element or "")
        if not start or (action.kind == "drag" and not end):
            raise Refused("drag needs two points resolved from the same screenshot")
        t.visual = None
        result = await t.bridge.call("pointer", {"tab_id": t.chrome_id, "point": start, "end": end,
                                                "kind": action.kind}, timeout=10, cid=intent_id or None)
        return ActionResult(bool(result.get("ok")), str(result.get("detail", "")))

    @staticmethod
    def _detached_msg(reason: str) -> str:
        if reason == "canceled_by_user":
            return "the owner cancelled FERB's debugger on that tab mid-action; outcome unknown"
        return f"the debugger left the tab mid-action ({reason}); outcome unknown"

    async def _upload(self, t: _Tab, frame: int, snap: int, local: int, files: tuple[str, ...],
                      intent_id: str, closed: Callable[[Path, str], str | None] | None = None) -> ActionResult:
        relative = [f for f in files if not os.path.isabs(os.path.expanduser(f))]
        if relative:                    # BrowserAct makes them the workspace's: a relative one here is never guessed
            raise Refused(f"upload paths must be absolute ({relative[0]}); nothing was given to the browser")
        paths = [source_path(f) for f in files]
        if not paths:
            raise Refused("upload needs at least one file path")
        staging = None
        if self.uploads is not None:
            staging = (self.uploads, self.task_root(str(t.owner or "")), tab_key(t.bridge.name, t.chrome_id))
        identities, given = await asyncio.to_thread(self._sources, paths, staging, closed)
        try:
            v = await t.bridge.call("upload", {"tab_id": t.chrome_id, "frame_id": frame, "snap": snap, "el": local,
                                               "paths": given}, timeout=30, cid=intent_id or None)
        except DriverError as e:
            if str(e).startswith("detached:"):
                raise OutcomeUnknown(self._detached_msg(str(e)[9:])) from None
            raise
        if v.get("detail") == "stale":
            raise StaleRef("the file field is gone; take a new snapshot")
        facts = {k: value for k, value in v.items() if k not in {"ok", "detail"}}
        facts.update(source="local", files=identities, transferred=bool(v.get("ok")))
        if given is not paths:
            facts["staged_copies"] = given          # what the browser read: FERB's copies; the originals are unchanged
        selected = v.get("selected_files")
        facts["bytes_match"] = (len(selected) == len(identities) and all(
            a.get("sha256") == b["sha256"] and a.get("size") == b["size"]
            for a,b in zip(selected,identities))) if isinstance(selected,list) else None
        changed = []
        for item in identities:
            try:
                stat = os.stat(item["path"])
                if (stat.st_size, stat.st_mtime_ns) != (item["size"], item["modified_ns"]):
                    changed.append(item["path"])
            except OSError:
                changed.append(item["path"])
        if changed:
            facts.update(reason="source_changed_during_upload", changed=changed, transferred=None)
            return ActionResult(False, "source changed during upload; attached bytes are unverified", facts)
        if v.get("ok") and facts["bytes_match"] is False:
            facts["reason"] = "uploaded_bytes_mismatch"
            return ActionResult(False, "file input contains different bytes; inspect before any submission", facts)
        return ActionResult(bool(v.get("ok")), str(v.get("detail", "")), facts)

    @staticmethod
    def _sources(paths: list[str], staging: tuple[Uploads, str, str] | None,
                 closed: Callable[[Path, str], str | None] | None = None) -> tuple[list[dict[str, Any]], list[str]]:
        """Each file's identity, read through one handle that never follows a link at its last step, and the paths the
        browser is given. Rule 17: with `staging` (uploads, task tree, tab), FERB's own copy made from that same handle,
        so the browser reads the copy, never the original in a folder macOS protects (a permission dialog in front of
        the owner), and the copy is checked against the identity just read. Rule 10: `closed` checks each handle as
        macOS opened it (F_GETPATH) before a byte is read, so a folder swapped for a link after the gateway's check
        can't hand over FERB's own files or a never-read store. Blocking: runs in a worker thread."""
        handles: list[int] = []
        try:
            identities: list[dict[str, Any]] = []
            for path in paths:
                try:
                    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
                except FileNotFoundError:
                    raise Refused(f"no such file to upload: {path}") from None
                except OSError as e:
                    raise Refused(f"FERB can't read the file to upload ({path}: {e.strerror or e}); nothing was given "
                                  "to the browser") from None
                handles.append(fd)
                if closed is not None:
                    try:
                        refusal = closed(opened_path(fd), "read")
                    except OSError as e:
                        refusal = (f"FERB couldn't tell which file it opened to upload ({path}: {e.strerror or e}); "
                                   "nothing was given to the browser")
                    if refusal:
                        raise Refused(refusal)
                before = os.fstat(fd)
                if not S_ISREG(before.st_mode):
                    raise Refused(f"no such file to upload: {path}")
                try:
                    digest = sha256_fd(fd)
                except OSError as e:
                    raise Refused(f"FERB can't read the file to upload ({path}: {e.strerror or e}); nothing was given "
                                  "to the browser") from None
                after = os.fstat(fd)
                if (before.st_size, before.st_mtime_ns) != (after.st_size, after.st_mtime_ns):
                    raise Refused("upload source changed while reading identity; inspect the file again")
                identities.append({"path": path, "size": before.st_size, "modified_ns": before.st_mtime_ns,
                                   "sha256": digest, "owner_uid": before.st_uid})
            if staging is None:
                return identities, paths
            uploads, tree, tab = staging
            try:
                return identities, uploads.stage_all(tree, [(fd, path, str(item["sha256"])) for fd, path, item
                                                            in zip(handles, paths, identities)], tab)
            except (OSError, StagingFailed) as e:
                raise Refused(f"FERB couldn't make its own copy of the file to upload ({e}); nothing was given to "
                              "the browser") from None
        finally:
            for fd in handles:
                os.close(fd)

    async def find(self, tab: TabRef, text: str, offset: int = 0, max_chars: int = 12000) -> dict[str, Any]:
        """Search all accessible frames; page full hit text without selecting a candidate."""
        t = self._tab(tab)
        if offset == 0:
            value = await t.bridge.call("find", {"tab_id": t.chrome_id, "text": text}, timeout=30)
            hits = value.get("hits", [{"frame_id": 0, "text": x} for x in value.get("examples", [])])
            t.searches[text] = (json.dumps(hits, ensure_ascii=False), int(value.get("count", 0)),
                                int(value.get("lines", 0)))
        if text not in t.searches:
            raise Refused("start this search at offset=0 before fetching its saved result pages")
        payload, count, lines = t.searches[text]
        end = min(offset + max_chars, len(payload))
        return {"matches": count, "lines_on_page": lines,
                "text": payload[offset:end], "offset": offset, "next_offset": end if end < len(payload) else None,
                "total_chars": len(payload), "examples": [h["text"] for h in json.loads(payload)] if len(payload) <= max_chars else [],
                "note": "Saved all-frame search inventory. offset=0 refreshes it; later pages keep the same evidence."}

    async def scroll(self, tab: TabRef, direction: str, container: str | None = None) -> dict[str, Any]:
        """Scroll the page, or one scroll container from the latest snapshot, about one screen (a real wheel event
        at the scroller's centre); where that scroller is now and whether it reached the end."""
        t = self._tab(tab)
        if container and container not in t.containers:
            raise StaleRef(f"no container {container} in the latest snapshot")
        frame, local = t.containers[container] if container else (0, None)
        args: dict[str, Any] = {"tab_id": t.chrome_id, "direction": direction, "frame_id": frame}
        if container:
            args["container"] = local
        try:
            v = await t.bridge.call("scroll", args, timeout=15)
        except DriverError as e:
            if str(e).startswith("detached:"):
                raise OutcomeUnknown(self._detached_msg(str(e)[9:])) from None
            if "unknown op" in str(e):                    # belt and braces: hello already refuses old extensions
                raise Refused(f"the FERB extension in that browser is older than FERB: reload it ({MIN_EXTENSION_VERSION} or later)") \
                    from None
            raise
        if v.get("container") != local or int(v.get("frame_id", 0)) != frame:
            raise DriverError("browser scrolled a different surface; inspect it before continuing")
        out: dict[str, Any] = {"y": int(v.get("y", 0)), "page_height": int(v.get("height", 0)),
                               "at_end": bool(v.get("at_end"))}
        if v.get("viewport_h") is not None:
            out["viewport_h"] = int(v["viewport_h"])
        if container:
            out["container"] = container
            out["frame"] = frame
        return out

    async def media(self, tab: TabRef, action: str, index: int | None, to: float | None) -> dict[str, Any]:
        """L26: play/pause/mute/unmute/seek one media element of the latest snapshot, by its index there; returns its
        state afterwards (and the page's error when it refused, e.g. play() without a real click)."""
        t = self._tab(tab)
        if not t.snapshot_json:
            raise Refused("take a snapshot first: browser.media uses an index from its `media` list")
        if index is None:
            if len(t.media) != 1:
                raise Refused(f"this page has {len(t.media)} media elements in the latest snapshot; choose one by its "
                              "index" if t.media else "the latest snapshot lists no video or audio on this page")
            index = next(iter(t.media))
        if index not in t.media:
            raise Refused(f"the latest snapshot has no media element {index}")
        frame, local = t.media[index]
        try:
            v = await t.bridge.call("media", {"tab_id": t.chrome_id, "frame_id": frame, "snap": t.snap_gen,
                                              "index": local, "action": action, "to": to})
        except DriverError as e:
            if "unknown op" in str(e):
                raise Refused(f"the FERB extension in that browser is older than FERB: reload it "
                              f"({MIN_EXTENSION_VERSION} or later)") from None
            raise
        return {k: v[k] for k in (*MEDIA_FIELDS, "error") if k in v} | {"index": index} | ({"frame": frame} if frame else {})

    async def wait(self, tab: TabRef, until: WaitCondition, timeout: float) -> bool:
        t = self._tab(tab)
        try:
            v = await t.bridge.call("wait", {"tab_id": t.chrome_id, "until": until.kind, "value": until.value},
                                    timeout=timeout)
        except OutcomeUnknown:
            return False
        return bool(v.get("met"))

    async def screenshot(self, tab: TabRef) -> bytes:
        """A jpeg of the tab (works minimized / in the background, 2.7), fetched in parts when one message would
        come near the 1 MB native-messaging line, with every secret field the extension reports painted over before
        anything else sees it (`painted`, L13). Never refused for what the page shows; withheld (Withheld) only when
        the masking can't be done."""
        import base64
        t = self._tab(tab)
        t.masking = ""
        v = await t.bridge.call("screenshot", {"tab_id": t.chrome_id, "snap": t.snap_gen})
        if v.get("withheld"):
            return painted(b"", v)                       # raises: nothing was captured
        if "data" in v:
            jpeg = base64.b64decode(v["data"])
        else:
            parts = []
            for i in range(int(v.get("parts", 0))):
                parts.append(str((await t.bridge.call("screenshot_part", {"tab_id": t.chrome_id, "part": i}))["data"]))
            jpeg = base64.b64decode("".join(parts))
        if v.get("unread"):                              # protocol 15: a page it couldn't read, sent anyway (B6)
            jpeg, t.masking = await self._unread(t, jpeg, v)
            return jpeg
        return painted(jpeg, v)

    def masking(self, tab: TabRef) -> str:
        """How the latest screenshot of this tab had its secret fields found, when worth saying ("" otherwise)."""
        return self._tab(tab).masking

    async def _unread(self, t: _Tab, jpeg: bytes, reply: dict[str, Any]) -> tuple[bytes, str]:
        """A capture of a page the extension couldn't read (protocol 15, B6; owner: no screenshot refused): painted
        where it can be, the rest left to the gateway's text recognition, and a note saying which. Painted: the look
        before the capture's fields when only the look after failed, and, when the tab is the one its window shows,
        the secret fields Accessibility finds in that window's page area, read just after the capture, scaled from the
        area onto the picture. Withheld only when the picture can't be painted."""
        from ferb.act.evidence import mask
        why = str(reply.get("unread"))[:200]
        try:
            size = mask.size(jpeg)
        except ValueError:
            raise Withheld("pixels withheld: the capture couldn't be painted over") from None
        boxes: list[mask.Box] = []
        m = reply.get("mask")
        before = mask.PageState.parse(m.get("before")) if isinstance(m, dict) else None
        if before is not None:
            boxes += mask.to_image_boxes([i.rect for i in before.items if i.paint], before.viewport, size)
        ax_boxes, ax_why = await self._native_boxes(t, reply.get("window"), size)
        boxes += ax_boxes
        if boxes:
            try:
                jpeg = mask.mask_secret_fields(jpeg, boxes)
            except (ValueError, RuntimeError):
                raise Withheld("pixels withheld: the capture couldn't be painted over") from None
        if ax_why is None:
            return jpeg, (f"the browser extension couldn't locate the page's secret fields ({why}): masked from "
                          "Accessibility, read just after the capture, and by text recognition (a secret neither can "
                          "find could show)")
        return jpeg, (f"the page's secret fields couldn't be located (the browser extension couldn't read it: {why}; "
                      f"{ax_why}); masked by text recognition only (a secret it can't read as text could show)")

    async def _native_boxes(self, t: _Tab, window: Any, size: tuple[int, int]) -> tuple[list[Any], str | None]:
        """Accessibility's secret fields for an unread tab, as image boxes, and why there are none (None: it read the
        page; a field it found with no box is reported as a reason too)."""
        from ferb.act.evidence import mask
        if not isinstance(window, dict) or window.get("minimized") or not window.get("active"):
            return [], "the tab isn't the one its window shows, so Accessibility can't see it"
        pid = getattr(t.bridge.conn, "browser_pid", None) if t.bridge.conn is not None else None
        if self.native_secrets is None or not isinstance(pid, int):
            return [], "Accessibility can't be asked about this browser"
        b = window.get("bounds")
        if not (isinstance(b, list) and len(b) == 4 and all(isinstance(v, (int, float)) for v in b)):
            return [], "the extension didn't say where the tab's window is"
        found = await self.native_secrets(pid, (float(b[0]), float(b[1]), float(b[2]), float(b[3])))
        if found is None:
            return [], "Accessibility couldn't read the page in its window"
        rects, area, whole = found
        boxes = mask.to_image_boxes(rects, area, size)
        return boxes, None if whole else "a secret field Accessibility found has no place to paint"

    def browser_pid(self, tab: TabRef) -> int | None:
        """The process of the browser showing this tab (its native host's parent), so hearing the tab (L26) captures
        that exact app process even when another instance shares its bundle id."""
        conn = self._tab(tab).bridge.conn
        return conn.browser_pid if conn is not None else None

    def recording_source(self, tab: TabRef, owner: str) -> tuple[dict[str, Any], Any]:
        t = self._tab(tab)

        async def capture() -> bytes | tuple[bytes, str]:    # with a note when the page couldn't be read (B6)
            import base64
            self.owned_by(tab.tab_id, owner)
            if self._tab(tab) is not t:
                raise Refused('recording tab identity changed')
            result = await t.bridge.call('record_frame', {'tab_id': t.chrome_id})
            if result.get('withheld'):
                return painted(b'', result)              # raises: this frame only (the recording goes on)
            if 'data' in result:
                frame = base64.b64decode(result['data'])
            else:
                parts = []
                for i in range(int(result['parts'])):
                    part = await t.bridge.call('record_frame', {'tab_id': t.chrome_id, 'part': i,
                                                               'capture_id': result['capture_id']})
                    parts.append(part['data'])
                frame = base64.b64decode(''.join(parts))
            if result.get('unread'):                     # protocol 15 (B6): masked where it can be, said with it
                return await self._unread(t, frame, result)
            return painted(frame, result)
        return {'kind': 'tab', 'browser': tab.browser, 'profile': tab.profile, 'tab_id': t.chrome_id,
                'tab': tab.tab_id}, capture

    async def visual_snapshot(self, tab: TabRef) -> tuple[str, bytes, int, int]:
        import Quartz
        await self.snapshot(tab)            # the generation the shot's points resolve against
        jpeg = await self.screenshot(tab)   # secret fields painted from the extension's own rects, not this list (L13)
        image = Quartz.CGImageSourceCreateWithData(jpeg, None)
        props = Quartz.CGImageSourceCopyPropertiesAtIndex(image, 0, None)
        width, height = int(props["PixelWidth"]), int(props["PixelHeight"])
        t = self._tab(tab)
        shot = "bs-" + secrets.token_hex(8)
        t.visual = (shot, t.snap_gen, t.snap_nav, asyncio.get_running_loop().time(), width, height)
        return shot, jpeg, width, height

    async def point(self, tab: TabRef, shot: str, x: float, y: float) -> PageElement:
        t = self._tab(tab)
        v = t.visual
        if not v or v[0] != shot or v[1] != t.snap_gen or v[2] != t.bridge.navs.get(t.chrome_id, 0) \
                or asyncio.get_running_loop().time() - v[3] > 30:
            raise StaleRef("the screenshot is stale; take another browser screenshot")
        if not (0 <= x < v[4] and 0 <= y < v[5]):
            raise Refused("point outside the screenshot")
        raw = await t.bridge.call("point", {"tab_id": t.chrome_id, "snap": t.snap_gen, "x": x, "y": y,
                                            "image_width": v[4], "image_height": v[5]})
        if raw.get("error"):
            raise StaleRef(str(raw["error"]))
        local, frame = int(raw["element"]["id"]), int(raw["frame_id"])
        eid = f"s{t.snap_gen}e{len(t.elements) + 1}"
        info = page_element(eid, str(frame), raw["element"])
        t.elements[eid] = (frame, t.snap_gen, local, info)
        t.points[eid] = str(raw["pointer"])       # two points can share a shot; the gesture consumes it
        return info

    def claim(self, tab: TabRef, owner: str) -> None:
        self._tab(tab).owner = owner

    def owned_by(self, tab_id: str, owner: str, borrow: bool = True) -> TabRef:
        """`owner`'s tab; with `borrow`, a registered checker reaches its worker's tabs too (`inspection_parent`)."""
        t = self._tabs.get(tab_id)
        if t is None or not (self._accessible(t, owner) if borrow else t.owner == owner):
            raise Refused(f"no tab {tab_id} open for this task")
        return t.ref

    def _accessible(self, tab: _Tab, task: str) -> bool:
        return tab.owner is not None and (tab.owner == task or tab.owner == self.inspection_parent(task))

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
        """Last known URLs of the task-owned tabs, exposed as verification facts."""
        return [t.url for t in self._tabs.values() if t.owner == owner]

    def tabs_of(self, owner: str) -> list[str]:
        """Existing aliases for verification; ownership stays with the original worker."""
        return [t.ref.tab_id for t in self._tabs.values() if t.owner == owner]

    async def release(self, owner: str) -> list[dict[str, Any]]:
        """Task end: close or release its tabs, each retried once. A tab that still can't be let go of becomes no task's:
        its downloads go back to the browser, and the extension's next hello lists it armed, so the daemon disarms it
        then (the release, retried on reconnect)."""
        # bounded: tasks that ended are no longer anyone's to race with; trees with no tab left start their count over
        self._released = {o for o in self._released if self.task_running(o)} | {owner}
        results, errors = [], []
        for alias in [a for a, t in self._tabs.items() if t.owner == owner]:
            for attempt in (1, 2):
                if alias not in self._tabs:
                    break
                try:
                    results.append(await self.close(self._tabs[alias].ref))
                    break
                except DriverError as e:
                    if attempt == 2:
                        self._tabs[alias].owner = None
                        errors.append(f"{alias}: {e}")
        live_roots = {self.task_root(t.owner) for t in self._tabs.values() if t.owner}
        self._tabs_opened = {r: n for r, n in self._tabs_opened.items() if r in live_roots}
        if errors:
            raise DriverError("browser cleanup incomplete: " + "; ".join(errors))
        return results

    async def shutdown(self) -> None:
        for alias in list(self._tabs):
            try:
                await self.close(self._tabs[alias].ref)
            except DriverError as e:
                self.hub.publish("browser.cleanup_failed", {"tab": alias, "error": str(e)})


class Router(_TabDriver):
    """Owner profiles chosen explicitly by the agent. Never FERB's identity profile or secrets."""
    identity = Identity.OWNER

    def profile_candidates(self) -> list[dict[str, Any]]:
        out = []
        for p in self.profiles():
            br = self.hub.bridge(p.browser, p.folder)
            if br is not None and br.binding.identity is Identity.FERB:
                continue
            out.append({"profile":f"{p.browser}/{p.folder}", "name":p.display_name, "account":p.email,
                        "paired":br is not None, "connected":bool(br is not None and br.conn is not None)})
        return out

    def __init__(self, hub: Hub, resolver: Resolver, profiles: Callable[[], list[BrowserProfile]],
                 wake: WakeServer | None = None, launch: Callable[[list[str]], None] | None = None) -> None:
        super().__init__(hub, profiles, wake, launch)
        self.resolver = resolver
        self.ask_code: Callable[[ToolContext, str], Awaitable[str | None]] | None = None
        self.add_secret_value: Callable[[str], None] = lambda value: None
        self.secrets: Secrets = _NoSecrets()

    async def open_for(self, url: str, owner: str, profile: str | None = None, *,
                       window_id: int | None = None, allow_foreground: bool = False) -> TabRef:
        choice = self.resolver.choose(profile)
        br = self.hub.bridge(choice.browser, choice.folder)
        if br is None:
            raise Refused(f"{choice.browser}/{choice.folder} isn't paired with FERB")
        if br.binding.identity is not Identity.OWNER:
            raise Refused("that profile belongs to FERB's identity, not the owner")
        return await self._open_in(await self._connected(br), url, owner, window_id=window_id,
                                   allow_foreground=allow_foreground)

    async def open(self, url: str, background: bool = True) -> TabRef:
        return await self.open_for(url, owner="")

    def _owner_bridges(self, profile: str | None, *, connected_only: bool = True) -> list[Bridge]:
        """Connected owner profiles (FERB's identity profile never), optionally only the one the owner named."""
        choice = self.resolver.choose(profile) if profile else None
        out = []
        for br in self.hub.bridges.values():
            if (connected_only and br.conn is None) or br.binding.identity is not Identity.OWNER:
                continue
            if choice and (choice.browser, choice.folder) != (br.binding.browser, br.binding.folder):
                continue
            out.append(br)
        return out

    async def open_tabs(self, profile: str | None = None, task: str | None = None) -> list[dict[str, Any]]:
        """The owner's open tabs in their connected profiles: a reference, the site and whether it's the active one.
        A tab this task already took over is listed with its tab id; FERB's own tabs and other tasks' aren't."""
        names = {(p.browser, p.folder): p.display_name for p in self.profiles()}
        out = []
        selected = self._owner_bridges(profile, connected_only=False)
        missing = [br for br in selected if br.conn is None]
        if missing:
            details = "; ".join(f"{br.name}: " + (self.hub.refused.get(br.binding.install_id) or
                               "paired but not connected; reload the FERB extension in that profile, then retry")
                               for br in missing)
            raise Refused(details + ". No tab inventory is available for these profiles; this is a connection "
                          "problem, not an empty tab list. Preserve the requested profile. browser.open with this "
                          "profile loads it without a window; when the profile has no window, opening a tab creates "
                          "one minimized, unfocused window, which may switch Spaces and so needs the owner's "
                          "permission once per task (allow_foreground). Native background controls may be an "
                          "alternative if the exact requested window can be identified.")
        for br in self._owner_bridges(profile):
            v = await br.call("list_tabs", {}, timeout=10)
            for t in v.get("tabs", []):
                if t.get("ferb"):
                    continue                                  # FERB's own minimized window: never the owner's
                mine = next((x for x in self._tabs.values() if x.bridge is br and x.chrome_id == int(t["tab_id"])
                             and x.attached and task is not None and self._accessible(x, task)), None)
                if mine is None and self.owner_of(br, int(t["tab_id"])) is not None:
                    continue                                  # FERB's own tabs, or another task's
                entry = {"tab_ref": f"{br.binding.browser}:{br.binding.folder}:{t['tab_id']}",
                            "profile": f"{br.binding.browser.capitalize()} “"
                                       f"{names.get((br.binding.browser, br.binding.folder), br.binding.folder)}”",
                            "profile_id": f"{br.binding.browser}/{br.binding.folder}",
                            "url":str(t.get("url") or ""), "title":str(t.get("title") or ""),
                            "site": str(t.get("host") or ""), "active": bool(t.get("active")),
                            "window_id": t.get("window_id")}
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
            if not self._accessible(existing, owner):
                raise Refused("another task is using that tab")
            return existing.ref
        v = await br.call("list_tabs", {}, timeout=10)
        tab = next((t for t in v.get("tabs", []) if int(t["tab_id"]) == cid), None)
        if tab is None:
            raise Refused("that tab isn't open any more")
        self._n += 1
        ref = TabRef(br.device_id, br.binding.browser, br.binding.folder, br.gen, f"t{self._n}")
        self._tabs[ref.tab_id] = _Tab(ref, br, cid, owner, str(tab.get("url") or ""), attached=True)
        return ref

    async def owner_code(self, el: ElementRef, ctx: ToolContext) -> ActionResult:
        t, frame, snap, local, info = self._resolve(el)
        self.owned_by(el.tab.tab_id, ctx.task_id)
        if not info.code_fill:      # the extension's own fill rule, known before anyone is asked (rule 9)
            raise Refused("that field takes no login code: select a visible one-time-code field, not a password or "
                          "an ordinary text field")
        if self.ask_code is None:
            raise Refused("hidden login-code input is unavailable")
        v = await t.bridge.call("frame_origin", {"tab_id": t.chrome_id, "frame_id": frame}, timeout=10)
        origin = normalize_origin(str(v.get("origin") or ""))
        if not origin.startswith("https://"):
            raise Refused("login codes require an HTTPS destination")
        value = await self.ask_code(ctx, origin)
        if not value:
            return ActionResult(False, "login code declined")
        try:
            self._resolve(el)                     # same generation, element and tab after waiting for the owner
            self.add_secret_value(value)
            key = self.hub.pairings.key(t.bridge.binding.install_id)
            if key is None:
                raise Refused("the browser is no longer paired")
            cid = ctx.intent_id or "f-" + secrets.token_hex(8)
            sealed = self.seal_value(seal_key(key), value, cid, origin)
            result = await t.bridge.call("fill", {"tab_id": t.chrome_id, "frame_id": frame, "snap": snap,
                                                 "el": local, "sealed": sealed, "origin": origin, "code_only": True}, timeout=15, cid=cid)
            return ActionResult(bool(result.get("ok")), str(result.get("detail", "")))
        finally:
            del value

    async def owner_secret(self, el: ElementRef, ctx: ToolContext, *, handle: str | None = None,
                           remember: str | None = None, split_elements: list[str] | None = None) -> ActionResult:
        t, frame, snap, local, info = self._resolve(el)
        self.owned_by(el.tab.tab_id, ctx.task_id)
        if info.role not in {"textbox", "searchbox", "combobox"} and not info.input_type:
            raise Refused("select an exact text field from a fresh snapshot")
        refs = [el]
        locals_: list[int] | None = None
        if split_elements is not None:
            if len(split_elements) < 2 or len(set(split_elements)) != len(split_elements) or split_elements[0] != el.element_id:
                raise Refused("select every distinct split-code textbox in order, with element first")
            refs = [self.element_ref(el.tab, ident) for ident in split_elements]
            locals_ = []
            for ref in refs:
                other, other_frame, other_snap, other_local, field = self._resolve(ref)
                if other is not t or (other_frame, other_snap) != (frame, snap):
                    raise Refused("split-code fields must share one tab, frame and snapshot")
                if field.role != "textbox" or field.input_type not in {None, "", "text", "tel", "number"}:
                    raise Refused("split-code fields must all be textboxes, not passwords or other controls")
                locals_.append(other_local)
        ask = getattr(self, "ask_secret", None)
        if ask is None:
            raise Refused("hidden secret input is unavailable")
        v = await t.bridge.call("frame_origin", {"tab_id": t.chrome_id, "frame_id": frame}, timeout=10)
        origin = normalize_origin(str(v.get("origin") or ""))
        if not origin.startswith("https://"):
            raise Refused("secret input requires an HTTPS destination")
        value = await ask(ctx, origin, handle=handle, remember=remember)
        if not value:
            return ActionResult(False, "secret input declined")
        try:
            for ref in refs:
                self._resolve(ref)
            self.add_secret_value(value)
            key = self.hub.pairings.key(t.bridge.binding.install_id)
            if key is None:
                raise Refused("the browser is no longer paired")
            cid = ctx.intent_id or "f-" + secrets.token_hex(8)
            result = await t.bridge.call("fill", {"tab_id": t.chrome_id, "frame_id": frame, "snap": snap,
                "el": local, "sealed": self.seal_value(seal_key(key), value, cid, origin), "origin": origin,
                **({"split_els": locals_} if locals_ is not None else {})}, timeout=15, cid=cid)
            return ActionResult(bool(result.get("ok")), str(result.get("detail", "")))
        finally:
            self.release_secret(value)

    async def fill_secret(self, el: ElementRef, ref: SecretRef, intent_id: str = "") -> ActionResult:
        raise Refused("FERB's secrets are never used in the owner's profiles")

    async def fill_otp(self, el: ElementRef, ref: SecretRef, candidate: str,
                       intent_id: str = "") -> ActionResult:
        raise Refused("FERB's inbox codes are never used in the owner's profiles")


class FerbIdentity(_TabDriver):
    """FERB's own profile (`ferb_browser.*`, identity FERB), bound by pairing. Secrets and one-time codes reach a
    field only sealed: the frame's origin is checked against the secret's allowed origins BEFORE the Keychain
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
        self._otp_inventory: dict[tuple[str | None, str, str], GmailFeedMailbox] = {}

    async def _bridge(self) -> Bridge:
        prof = self.profile()
        br = self.hub.bridge(*prof) if prof else None
        if br is None or br.binding.identity is not Identity.FERB:
            raise Refused("FERB's identity profile isn't paired: run `ferb setup --identity`")
        return await self._connected(br)

    async def open_for(self, url: str, owner: str, profile: str | None = None, *,
                       window_id: int | None = None, allow_foreground: bool = False) -> TabRef:
        return await self._open_in(await self._bridge(), url, owner, window_id=window_id,
                                   allow_foreground=allow_foreground)

    async def open(self, url: str, background: bool = True) -> TabRef:
        return await self.open_for(url, owner="")

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

    async def otp_candidates(self, el: ElementRef, ref: SecretRef) -> dict[str, Any]:
        if not self.secrets.has(ref):
            raise Refused(f"no account {ref.key} is set up")
        t, frame, _, _, _ = self._resolve(el)
        origin = await self._checked_origin(t, frame, ref)
        mailbox = GmailFeedMailbox(self._read_feed, self.mailbox_address(), self.feed_url)
        mailbox.used = self.used_otps
        result = await mailbox.candidates()
        self._otp_inventory[(t.owner, ref.key, origin)] = mailbox
        return {**result, "destination_origin":origin}

    async def fill_otp(self, el: ElementRef, ref: SecretRef, candidate: str,
                       intent_id: str = "") -> ActionResult:
        if not self.secrets.has(ref):
            raise Refused(f"no account {ref.key} is set up")
        t, frame, snap, local, _ = self._resolve(el)
        origin = await self._checked_origin(t, frame, ref)
        mailbox = self._otp_inventory.get((t.owner, ref.key, origin))
        if mailbox is None:
            raise Refused("inspect OTP candidates for this account and destination first")
        try:
            found = mailbox.take(candidate)
        except ValueError as e:
            raise Refused(str(e)) from None
        self.add_secret_value(found.code)
        r = await self._sealed_fill(t, frame, snap, local, origin, found.code, intent_id)
        if r.ok:
            self.used_otps.add(found.message_id)
        return ActionResult(r.ok, f"{r.detail} (explicitly selected inbox candidate)",
                            {"message_id":found.message_id,"source":self.feed_url,"destination_origin":origin})

    async def release(self, owner: str) -> list[dict[str, Any]]:
        try:
            return await super().release(owner)
        finally:
            self._otp_inventory = {k:v for k,v in self._otp_inventory.items() if k[0] != owner}

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
        sealed = self.seal_value(seal_key(key), value, cid, origin)
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
            return unseal(seal_key(key), v["sealed"], cid, url)       # the feed travels sealed too
        finally:
            await self.close(ref)

    async def session_ok(self) -> bool:
        """Signed in as FERB? Can't reach FERB's profile -> raises (that isn't "signed out")."""
        await self._bridge()
        return await GmailFeedMailbox(self._read_feed, self.mailbox_address(), self.feed_url).signed_in()
