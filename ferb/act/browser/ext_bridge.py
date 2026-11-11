"""Owns authenticated extension transport and task-owned browser references; must not choose source/account intent.

Hub binds authenticated installs to profiles. Bridge tracks generation-scoped command IDs, chunks large content,
and records disconnected actions as unknown without replay. Router binds explicit profiles and element references.
FerbIdentity adds origin-bound sealed fills and opaque mailbox candidates. Upload receipts retain local byte identity;
the browser is given FERB's own copy of the file (uploads.py), never the original.
A download caught in a task tab goes to `Hub.on_download` (downloads.py takes it); the drivers say whose tab it is.
WakeServer serves only the temporary local pairing page; ordinary wake launches no window or URL.

Protocol 18 (step 5c): the daemon signs what the extension acts on (`daemon_envelope`); a connection is taken only from
a browser app FERB knows (`peer_refusal`); a task's tabs live in one collapsed tab group (HAND-201) and are
known by browser tab id, so a reconnect or a daemon restart re-adopts them from the extension's records or closes those
of an ended task (`_TabDriver.readopt`); the owner closing or taking a FERB tab is published for the supervisor
(`browser.owner_stopped`, `browser.owner_took`); a read that fails is a known failure, never "outcome unknown"."""

import asyncio
import base64
import dataclasses
import hashlib
import json
import math
import mimetypes
import os
import re
import secrets
from dataclasses import dataclass, field
from pathlib import Path
from stat import S_ISREG
from typing import Any, Awaitable, Callable
from urllib.parse import urlsplit

from ferb.act.browser.ext_protocol import (MIN_EXTENSION_VERSION, PROTOCOL_VERSION, READS, UPLOAD_BYTES_EXTENSION_VERSION,
                                           UPLOAD_PART, Event, Hello, Op, Result, TabEvent, command, daemon_envelope,
                                           parse, version_ok)
from ferb.act.browser.downloads import Downloads, TaskTab
from ferb.act.browser.uploads import StagingFailed, Uploads, source_path, tab_key
from ferb.act.browser.pairing import Binding, Pairings
from ferb.act.browser.profiles import (BROWSERS, BrowserProfile, named as named_profile, open_known_profile,
                                      running as browser_running, shown as profile_shown, with_paired)
from ferb.act.browser.resolver import Resolver
from ferb.act.browser.snapshot import FIELD_KINDS, element as page_element
from ferb.act.browser.tools import Secrets
from ferb.act.fs.files import opened_path
from ferb.act.fs.recovery import sha256_fd
from ferb.identity.mailbox import FEED_URL, GmailFeedMailbox
from ferb.identity.seal import seal, seal_key, unseal
from ferb.identity.secrets import KeychainSecretStore, SecretUnavailable, normalize_origin, origin_allowed
from ferb.core.errors import DriverError, FerbError, Refused, StaleRef, Withheld
from ferb.core.types import (ActionFacts, ActionResult, BrowserAction, DriverFeatures, ElementRef, Identity, PageElement,
                             PageSnapshot, SecretRef, SecretSpec, TabRef, TerminalInput, ToolContext, WaitCondition)
from ferb.policy import terminal as terminals
from ferb.orchestrator.ledger import Ledger
from ferb.sanitize.redact import Redactor

Publish = Callable[[str, dict[str, Any]], None]
CMD_TIMEOUT_S = 30.0
WINDOWLESS_WAIT_S = 35.0         # a profile loaded with no window: a fresh one connects in about a second (tested
                                 # 2026-10-26); an already loaded one whose connection dropped (FERB restarted)
                                 # reconnects on the extension's 30 s alarm
WINDOW_WAIT_S = 15.0             # then the profile's window: a profile it loads connects in about a second
KEEPALIVE_S = 15.0               # while a task tab is open: a ping keeps the MV3 worker awake (idle limit 30 s, 1.1)
TRUSTED_KINDS = frozenset({"click", "dblclick", "rightclick", "hover", "type", "press"})   # real input by default
BLANK = "about:blank"            # a task tab opens blank, then loads its URL (D35)
# a media element's state (L26): `playing` once its time moves (I5), `paused` as the page has it
MEDIA_FIELDS = ("playing", "paused", "current_time", "duration", "muted", "volume", "in_view")
# a media element's own id in its frame (extension 0.15.8, L31): the content script's `m<n>-<page token>`, no dots, so
# a probe's dot path reaches it (`media_by_id.m12-3fa91c.playing`); the token differs in each page the tab loads
MEDIA_ID = re.compile(r"m[0-9]{1,9}-[0-9a-f]{6}")


# What each step of a read is, for the worker's "the page isn't responding" (extension 0.19.6 names the step)
READ_STEPS = {"wake": "keeping the tab awake", "attach": "preparing the tab", "inject": "loading FERB's reader into "
              "the page", "script": "reading the page", "ax": "reading the page's accessibility tree",
              "unbind": "tidying up after the read", "arm": "preparing the tab", "ready": "preparing the tab",
              "look": "finding the page's secret fields", "capture": "taking the picture"}


def _steps(raw: Any) -> dict[str, Any] | None:
    """The extension's step times (0.19.6) as numbers only, for the call's span; None from an older extension."""
    if not isinstance(raw, dict):
        return None
    return {str(k)[:40]: v for k, v in list(raw.items())[:30] if isinstance(v, (int, float)) and not isinstance(v, bool)}


def not_responding(op: str, timeout: float, step: str | None, steps: Any = None) -> DriverError:
    """A read of a tab that timed out (HAND-435): the page stopped answering. Says where (the extension's step, 0.19.6)
    and what helps, as the worker's rule for a page that doesn't work says it (PRM-341, HAND-441): refresh once, then
    the same address in a new tab, then report."""
    where = f" (it stopped at: {READ_STEPS.get(step, step)})" if step else ""
    e = DriverError(f"the page isn't responding: FERB's {op} got no answer from it within {timeout:g} s{where}. "
                    "Reading this tab again usually hangs the same way: refresh it once (navigate it to its address "
                    "again); if it still doesn't answer, open the same address in a new tab and read that; if that "
                    "fails too, report it",
                    facts={"reason": "page_not_responding", **({"step": step} if step else {})})
    e.timing = {"read": _steps(steps)} if _steps(steps) else None      # type: ignore[attr-defined]
    return e


def frame_urls(frames: list[dict[str, Any]]) -> list[str]:
    """The addresses of a snapshot's embedded frames, in order, once each: each child frame's own address (its content
    script reports it) and each frame element's `src` (a frame no script reaches, such as one in a closed shadow root,
    still names it). Structural facts, read like a field's kind (HAND-208)."""
    found: list[str] = []
    for f in frames:
        urls = [str(i.get("src") or "") for i in f.get("iframes") or [] if isinstance(i, dict)]
        if int(f.get("frame_id") or 0) != 0:
            urls.append(str(f.get("url") or ""))
        for u in urls:
            if u.startswith(("http://", "https://")) and u not in found:
                found.append(u)
    return found

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
    app: str | None = None                  # that process's app bundle id (peer_refusal)
    nonce: str = ""                         # protocol 18: the hello's signing nonce for this connection
    key: bytes | None = field(default=None, repr=False)   # the pairing key the daemon signs with (daemon-internal)
    dseq: int = 0                           # the last counter the daemon signed on this connection


def _app_bundle(pid: int | None) -> str | None:
    """The bundle id of the app whose executable this process runs (its path through libproc, then the enclosing
    .app's Info.plist): kernel and file facts, never a window or a name the process claims. Blocking (a file read).
    Chrome may run from its code-signing clone ("…/code_sign_clone.XXXX/Google Chrome.app.bundle/Contents/MacOS/…", a
    copy of its own bundle made at launch): that bundle's Info.plist is read the same way (HAND-438; lab 2026-11-09:
    5 of 12 lab Chromes ran from one, and every connection of theirs was refused as "couldn't tell which app")."""
    import ctypes
    import plistlib
    if not isinstance(pid, int) or pid <= 1:
        return None
    try:
        buf = ctypes.create_string_buffer(4096)
        if ctypes.CDLL("/usr/lib/libproc.dylib").proc_pidpath(pid, buf, 4096) <= 0:
            return None
        path = buf.value.decode(errors="replace")
        found = [(at, len(suffix)) for suffix in (".app/Contents/", ".app.bundle/Contents/")
                 if (at := path.find(suffix)) >= 0]
        if not found:
            return None
        at, size = min(found)                                # the outermost bundle, as before
        with open(path[:at + size - len("/Contents/")] + "/Contents/Info.plist", "rb") as f:
            bundle = plistlib.load(f).get("CFBundleIdentifier")
    except (OSError, ValueError, plistlib.InvalidFileException):
        return None
    return bundle if isinstance(bundle, str) and bundle else None


def too_old(version: str) -> str:
    """The plain reason a profile's FERB extension is below the minimum (HAND-406): what the owner does about it."""
    return (f"the FERB extension in this profile is too old (version {version}; FERB needs {MIN_EXTENSION_VERSION} or "
            "later): update it")


def version_refusal(version: str, protocol: int) -> tuple[str, str]:
    """(reason code, plain why) for a hello whose version or protocol FERB can't use: an extension older than FERB
    needs is updated; one newer than this FERB means FERB itself is updated."""
    if protocol > PROTOCOL_VERSION:
        return "extension_newer", (f"the FERB extension in this profile (version {version}, protocol {protocol}) is "
                                   f"newer than this FERB (protocol {PROTOCOL_VERSION}): update FERB")
    return "extension_too_old", too_old(version) + f" (its protocol {protocol}, FERB's {PROTOCOL_VERSION})"


def peer_refusal(app: str | None, browser: str | None = None) -> str | None:
    """Why a native-host connection isn't taken, None when it is (protocol 18; rule 5, a where-boundary by bundle id).
    The extension's origin is only what the native host was told, so the daemon also checks who started that host:
    the browser launches it, and its process's app (`app`, `_app_bundle` of the host's parent) must be a web browser FERB
    knows; for a paired profile (`browser`, its BROWSERS key) exactly that browser (or one of its helper apps). Limit:
    any process of the owner's own user can start that browser's binary, or read a profile's pairing key from its
    extension storage, so same-uid processes remain trusted (as for the CLI socket)."""
    from ferb.act.mac import browsers as mac_browsers
    if app is None:
        return ("FERB couldn't tell which app started this browser connection, so it didn't take it (the FERB "
                "extension reconnects by itself)")
    if not mac_browsers.known(app):
        return f"this browser connection was started by {app}, not a web browser FERB knows, so FERB didn't take it"
    if browser is not None and browser in BROWSERS:
        want, got = BROWSERS[browser].bundle_id.casefold(), app.casefold()
        if got != want and not got.startswith(want + ".helper"):
            return (f"this profile was paired in {BROWSERS[browser].label}, but the connection comes from {app}, so FERB "
                    "didn't take it")
    return None


_TEXT_MODS = frozenset({"cmd", "command", "meta", "ctrl", "control", "alt", "opt", "option"})


def enters_text(spec: str | None) -> bool:
    """Whether a key press puts text into the focused field (protocol.js `entersText`): a character or Space without
    ⌘/⌃/⌥ (Shift is a capital), or a paste (⌘V)."""
    if spec == " ":
        return True
    parts = [p.strip() for p in str(spec or "").strip().split("+")]
    if not parts or not parts[-1]:
        return False
    key, mods = parts[-1], {p.casefold() for p in parts[:-1]}
    if mods & _TEXT_MODS:
        return key.casefold() == "v" and bool(mods & {"cmd", "command", "meta"})
    if key.casefold() in ("space", "spacebar"):
        return True
    return len(key) == 1                          # a named key (Enter, Tab, an arrow, F5...) puts nothing in


def group_title(text: str) -> str:
    """A task's tab-group label (HAND-201): "FERB · <short task>", the task's first words, at most about 30
    characters; "FERB" when it has none."""
    words = " ".join(str(text or "").split())
    if len(words) > 30:
        cut = words[:30].rsplit(" ", 1)[0] if " " in words[:30] else words[:30]
        words = cut.rstrip(",.;:") + "…"
    return f"FERB · {words}" if words else "FERB"


# An amount as a page writes it (a fact's shape, never its meaning): a currency symbol or an ISO 4217 code of a common
# currency before or after a number. Used only on a control's own label and the page's structured data.
_CURRENCY = r"(?:[$€£¥₹₩₽₺₪₫฿₱₦₴]|Rs\.?|(?:USD|EUR|GBP|INR|JPY|CNY|AUD|CAD|CHF|SGD|HKD|NZD|SEK|NOK|DKK|PLN|BRL|MXN|ZAR|AED|SAR|KRW|TRY|THB|IDR|MYR|PHP|VND|ILS|RUB)\b)"
_AMOUNT = re.compile(rf"{_CURRENCY}\s?\d[\d,]*(?:\.\d{{1,2}})?|\b\d[\d,]*(?:\.\d{{1,2}})?\s?{_CURRENCY}")


def label_amount(label: str | None) -> str | None:
    """The one amount a control's label shows ("Pay ₹248.75" -> "₹248.75"), None when it shows none or several."""
    found = {m.group(0).strip() for m in _AMOUNT.finditer(str(label or "")[:300])}
    return found.pop() if len(found) == 1 else None


PAGE_AMOUNTS = 200              # GATE-578: at most this many distinct amounts kept from one snapshot


def page_amounts(texts: list[str]) -> tuple[str, ...]:
    """GATE-578: every money amount the page shows (a currency sign or code beside a number), as written, in page
    order, each once. Only a shape: which one is the total is never decided from the words around it."""
    found: dict[str, None] = {}
    for text in texts:
        for m in _AMOUNT.finditer(str(text or "")):
            found.setdefault(" ".join(m.group(0).split()), None)
            if len(found) >= PAGE_AMOUNTS:
                return tuple(found)
    return tuple(found)

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
    file_access: bool | None = None                          # 0.19.9 (HAND-500): the hello's, None from an older one
    pending: dict[str, asyncio.Future[Result]] = field(default_factory=dict)
    lost: set[str] = field(default_factory=set)             # ledger ids in flight when a connection dropped
    tabs: dict[int, tuple[str, bool]] = field(default_factory=dict)   # chrome tab id -> (host, active)
    navs: dict[int, int] = field(default_factory=dict)      # chrome tab id -> loads seen (stale-ref detection)
    connected: asyncio.Event = field(default_factory=asyncio.Event)
    detached: dict[int, str] = field(default_factory=dict)  # chrome tab id -> why the debugger left (Cancel, ...)
    holds: int = 0                                           # task tabs open on this bridge (keepalive while > 0)
    hellos: int = 0                                          # hellos attached so far (pairing waits for a new one)
    window: int | None = None                                # the window FERB chose for a tab here last (remembered)
    _keepalive: "asyncio.Task[None] | None" = None

    @property
    def name(self) -> str:
        return f"{self.binding.browser}/{self.binding.folder}"

    def envelope(self, conn: _Conn, mtype: Any, body: dict[str, Any]) -> dict[str, Any]:
        """A signed daemon message for this connection (protocol 18): the next counter, the hello's nonce, the key."""
        conn.dseq += 1
        return daemon_envelope(conn.key or b"", conn.nonce, conn.dseq, mtype, body)

    async def call(self, op: Op, args: dict[str, Any], timeout: float = CMD_TIMEOUT_S,
                   cid: str | None = None) -> Any:
        """One command. A read (READS, or `dialog` without a decision) that fails, times out or loses its connection is
        a known failure (DriverError): it changed nothing, so it is never "outcome unknown"."""
        conn = self.conn
        if conn is None:
            raise DriverError(f"{self.name} isn't connected")
        if conn.key is None or not conn.nonce:
            raise DriverError(f"{self.name} can't be sent commands: its connection has no signing key yet")
        read = op in READS or (op == "dialog" and not args.get("decision"))
        cid = cid or "c-" + secrets.token_hex(8)
        fut: asyncio.Future[Result] = asyncio.get_running_loop().create_future()
        self.pending[cid] = fut
        wire = json.dumps(self.envelope(conn, "cmd", command(cid, conn.gen, op, args, int(timeout * 1000))))
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
            if not read:
                self.lost.add(cid)
            if op == "pointer":
                await self._cancel_pointer(args, cid)
            raise
        except asyncio.TimeoutError:
            if op == "pointer":
                await self._cancel_pointer(args, cid)
            if read and "tab_id" in args:
                raise not_responding(op, timeout, None) from None
            if read:
                raise DriverError(f"{op} timed out in {self.name}") from None
            self.lost.add(cid)                            # the next hello's seen / owned ids settle it (never resent)
            raise OutcomeUnknown(f"{op} timed out in {self.name}") from None
        except OutcomeUnknown as e:                      # the connection dropped mid-command (drop)
            if read:
                self.lost.discard(cid)
                raise DriverError(f"{op}: {self.name} disconnected; nothing changed, try it again") from e
            raise
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
        if not read and (r.outcome == "unknown" or r.error == "timeout" or str(r.error or "").startswith("detached:")):
            self.lost.add(cid)
            raise OutcomeUnknown(f"{op}: {r.error}; outcome unknown")
        if str(r.error or "").startswith("debugger_refused:"):   # the browser refused the debugger: nothing was sent
            raise DriverError(str(r.error).removeprefix("debugger_refused:").strip(), facts={"reason": "debugger_refused"})
        if read and r.error == "timeout" and "tab_id" in args:
            raise not_responding(op, timeout, r.step, r.steps)
        if str(r.error or "").startswith("dialog_pending:"):   # 0.19.7 (HAND-442): nothing was sent; the dialog's type
            raise DriverError(str(r.error).removeprefix("dialog_pending:").strip(),
                              facts={"reason": "dialog_pending", **({"dialog": r.dialog} if r.dialog else {})})
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
            self.file_access = hello.file_access
            self._reconcile(hello)
            return
        self.gen += 1
        conn.gen, conn.bridge = self.gen, self
        old, self.conn, self.version, self.file_access = self.conn, conn, hello.version, hello.file_access
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
    def unpaired_copies(self) -> dict[str, str]:
        """APP-892: each browser with an unpaired FERB copy connected now, and that copy's version ("" when it said
        none, or when there are several there and they differ)."""
        out: dict[str, str] = {}
        for install, browser in self.unpaired.items():
            version = self.unpaired_version.get(install, "")
            out[browser] = version if out.get(browser, version) == version else ""
        return out

    def __init__(self, pairings: Pairings, ledger: Ledger, publish: Publish, device_id: Callable[[], str]) -> None:
        self.pairings, self.ledger, self.publish, self.device_id = pairings, ledger, publish, device_id
        self.bridges: dict[tuple[str, str], Bridge] = {}
        self.refused: dict[str, str] = {}                    # install_id -> why (e.g. protocol mismatch)
        self.unpaired: dict[str, str] = {}                   # install_id -> browser: installed, connected, not paired
        self.unpaired_version: dict[str, str] = {}           # install_id -> the version its hello said (APP-892)
        self.on_unpaired: Callable[[], None] = lambda: None  # an unpaired copy said hello: FERB can pair it (L25)
        # The activity trail: every tab event goes to on_tab; while paused, only tabs FERB owns are tracked.
        self.on_tab: Callable[["Bridge", TabEvent], None] = lambda br, e: None
        # A tab closed: FERB's own resources tied to it (upload copies) can go; nothing about the tab is recorded, so
        # this runs while sensing is paused too. A returned awaitable runs in the background.
        self.on_tab_closed: Callable[["Bridge", int], Awaitable[None] | None] = lambda br, tab_id: None
        self.paused: Callable[[], bool] = lambda: False
        self.owned: Callable[["Bridge", int], bool] = lambda br, tab_id: False
        # HAND-207 (protocol 19): a tab's sound changed; the daemon records one in a tab that isn't FERB's as a
        # disturbance fact (never in the activity trail)
        self.on_sound: Callable[["Bridge", TabEvent], None] = lambda br, e: None
        # A download the extension holds (D35); the daemon sets downloads.Downloads.receive. Unset: back to the browser.
        self.on_download: Callable[["Bridge", Event], None] = _let_download_go
        self.drivers: list["_TabDriver"] = []               # the browser drivers (they register): whose tab is it
        self._jobs: set["asyncio.Task[Any]"] = set()
        # HAND-406: every connection or hello FERB doesn't take, with its reason (reason code, plain why, the profile
        # or the app when known; never a message's contents). The daemon journals it, bounded per reason and profile.
        self.record_refusal: Callable[[dict[str, Any]], None] = lambda facts: None

    def note_refusal(self, reason: str, why: str, install_id: str | None = None, app: str | None = None,
                     **facts: Any) -> None:
        """Record one refused connection or message (HAND-406). A paired install's refusal is also kept for this run
        (`refused`), so the planner's facts, the tool errors and Settings say why its profile isn't connected."""
        binding = self.pairings.binding(install_id) if install_id and self.pairings is not None else None
        if binding is not None:
            self.refused[binding.install_id] = why
        out = {"reason": reason, "why": why, **facts}
        if binding is not None:
            out["profile"] = f"{binding.browser}/{binding.folder}"
        if app:
            out["app"] = app
        try:
            self.record_refusal(out)
        except Exception:
            pass                                             # a recording never changes the connection

    def problem(self, binding: Binding) -> str | None:
        """Why this paired profile's extension can't be used, as far as FERB knows: its refusal in this run, else the
        version it last reported when FERB needs a newer one (kept in the pairing record, so a restart, or an
        extension too old to get as far as a hello, doesn't hide it). None: nothing known against it."""
        if (why := self.refused.get(binding.install_id)) is not None:
            return why
        version = self.pairings.version(binding.install_id) if self.pairings is not None else ""
        return too_old(version) if version and not version_ok(version) else None

    def task_tab(self, br: "Bridge", chrome_id: int) -> TaskTab | None:
        """The live task that drives this tab now, in any driver; None: the extension must not hold its downloads."""
        return next((tt for d in self.drivers if (tt := d.task_tab(br, chrome_id)) is not None), None)

    def holds(self, br: "Bridge", chrome_id: int) -> bool:
        """A running task still holds this tab (a loop's, resting between rounds, included): it isn't let go of here."""
        return any(d.holds(br, chrome_id) for d in self.drivers)

    def tab_removed(self, br: "Bridge", chrome_id: int) -> None:
        """A tab closed in the browser: a task tab is marked closed in its driver (re-review 2, R5)."""
        for d in self.drivers:
            d.tab_closed(br, chrome_id)

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
            if self.task_tab(br, int(tab_id)) is None and not self.holds(br, int(tab_id)):
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
        # a loop's held tab between rounds: the owner's link opens as theirs, and the loop keeps holding the tab
        await _pass_through(br, ev.tab_id, str(ev.url or ""), active=not ev.background,
                            disarm=not self.holds(br, ev.tab_id))

    def _on_popup(self, br: "Bridge", ev: Event) -> None:
        """A popup the extension claimed by its page-hook token for a live task tab: the same task's tab from now on
        (within the task's tab cap), then the page's hook loads its URL (popup_ready); FERB never navigates it. Past
        the cap the extension closes it while it is still blank (`popup_refused`). Else let go of it, and the hook
        loads the URL itself after 1.5 s."""
        for d in self.drivers:
            if ev.opener_tab_id is not None and d.task_tab(br, ev.opener_tab_id) is not None:
                record = d.adopt(br, ev.opener_tab_id, ev.tab_id, str(ev.url or ""))
                if record is not None and "refused" in record:   # past the task's tab cap: closed, never handed over
                    self._spawn(br.call("popup_refused", {"tab_id": ev.tab_id}, timeout=10))
                elif record is not None:
                    async def ready(record: dict[str, Any] = record) -> None:
                        try:                             # recorded (and grouped) first: a reconnect re-adopts it
                            await br.call("adopt_tab", record, timeout=10)
                        finally:
                            await br.call("popup_ready", {"tab_id": ev.tab_id}, timeout=10)
                    self._spawn(ready())
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
        conn.app = await asyncio.to_thread(_app_bundle, conn.browser_pid)
        if (why := peer_refusal(conn.app)) is not None:   # only a browser FERB knows starts the extension's host
            self.publish("browser.connection_refused", {"why": why})
            self.note_refusal("peer", why, app=conn.app)
            try:
                writer.write(json.dumps({"type": "refused", "reason": why}).encode() + b"\n")
                await writer.drain()
            except ConnectionError:
                pass
            return
        try:
            while line := await reader.readline():
                try:
                    raw = json.loads(line)
                except ValueError:
                    raw = None
                m = parse(raw) if isinstance(raw, dict) else None
                if m is not None:
                    await self._handle(conn, m)
                else:                                        # dropped, nothing acts; recorded (never its contents)
                    self.note_refusal("malformed", "a message from the FERB extension wasn't in FERB's format, so "
                                      "FERB dropped it", conn.install_id, conn.app)
        except (ConnectionError, asyncio.LimitOverrunError, ValueError):
            pass
        finally:
            if conn.unpaired_install:
                self.unpaired.pop(conn.unpaired_install, None)
                self.unpaired_version.pop(conn.unpaired_install, None)
            if conn.bridge is not None:
                conn.bridge.drop(conn)
                self.publish("browser.disconnected", {"profile": conn.bridge.name})

    async def _handle(self, conn: _Conn, m: Any) -> None:
        v = self.pairings.handle(m)
        if not v.accepted:
            if m.type == "hello" and v.reason in {"bad_mac", "replay"}:
                why = f"extension pairing failed ({v.reason}); pair this profile again (browser_pair, or Settings → Browsers)"
                self.note_refusal(str(v.reason), why, m.install_id, conn.app)
                conn.writer.write(json.dumps({"type": "refused", "reason": why}).encode() + b"\n")
                await conn.writer.drain()
            elif m.type in ("hello", "pair"):                # a pairing that didn't take (unknown or expired token)
                self.note_refusal(str(v.reason), f"the FERB extension's {m.type} was refused ({v.reason})",
                                  m.install_id, conn.app)
            return                                           # unpaired / bad MAC / replay: dropped, nothing acts
        if v.reply is not None:                              # just paired: hand the key back on this connection
            self.unpaired.pop(m.install_id, None)
            self.unpaired_version.pop(m.install_id, None)
            conn.writer.write(json.dumps(v.reply).encode() + b"\n")
            await conn.writer.drain()
            if v.binding:
                self.publish("browser.profile_paired", {"browser": v.binding.browser, "folder": v.binding.folder,
                                                        "identity": v.binding.identity.value})
            return
        if v.binding is None:                                # unpaired hello: nothing acts; FERB can tell the owner
            if m.type == "hello":
                try:
                    said = m.body()
                except ValueError:
                    said = {}
                b = str(said.get("browser") or "")
                if b in ("chrome", "brave"):
                    self.unpaired[m.install_id] = b
                    self.unpaired_version[m.install_id] = str(said.get("version") or "")[:32]
                    conn.unpaired_install = m.install_id
                    self.on_unpaired()
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
            self.pairings.record_version(m.install_id, hello.version)   # authenticated: kept, accepted or not
            if hello.protocol != PROTOCOL_VERSION or not version_ok(hello.version):
                reason, why = version_refusal(hello.version, hello.protocol)
                self.note_refusal(reason, why, m.install_id, conn.app, version=hello.version[:32],
                                  protocol=hello.protocol)
                conn.writer.write(json.dumps({"type": "refused", "reason": why,
                                              "required_protocol": PROTOCOL_VERSION,
                                              "minimum_version": MIN_EXTENSION_VERSION}).encode() + b"\n")
                await conn.writer.drain()
                return
            key = self.pairings.key(m.install_id)
            refusal = ("the FERB extension sent no signing nonce: reload the FERB extension"
                       if not hello.nonce or key is None else peer_refusal(conn.app, v.binding.browser))
            if refusal is not None:
                self.note_refusal("no_nonce" if not hello.nonce or key is None else "peer", refusal, m.install_id,
                                  conn.app)
                conn.writer.write(json.dumps({"type": "refused", "reason": refusal}).encode() + b"\n")
                await conn.writer.drain()
                return
            self.refused.pop(m.install_id, None)
            br = self.bridge(v.binding.browser, v.binding.folder)
            if br is None:
                return
            conn.nonce, conn.key, conn.dseq = hello.nonce, key, 0     # this connection's signing state (protocol 18)
            br.attach(conn, hello)
            # identity: in FERB's own profile every download is FERB's, so a hold the daemon never takes is failed, not
            # handed to the browser (D35).
            conn.writer.write(json.dumps(br.envelope(conn, "welcome", {
                "gen": br.gen, "identity": br.binding.identity.value})).encode() + b"\n")
            await conn.writer.drain()
            self.publish("browser.connected", {"profile": br.name, "version": hello.version, "gen": br.gen})
            for d in self.drivers:                           # FERB's tabs here, by browser tab id (HAND-201)
                if getattr(d, "identity", None) is br.binding.identity:
                    for settle in d.readopt(br, hello.tabs):
                        self._spawn(settle)
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
            elif ev.event == "taken":                        # the owner moved a FERB tab out of its group: theirs now
                for d in self.drivers:
                    d.tab_taken(br, ev.tab_id, ev.reason or "")
            else:
                self.publish("browser.dialog", {"profile": br.name, "tab_id": ev.tab_id, "type": ev.type,
                                                "message": ev.message or "", "handled": ev.handled})
        elif m.type == "tab_event":
            try:
                e = TabEvent.model_validate(body)
            except ValueError:
                return
            if e.event == "removed":
                self.tab_removed(br, e.tab_id)
                if (job := self.on_tab_closed(br, e.tab_id)) is not None:
                    self._spawn(job)
            if self.paused() and not self.owned(br, e.tab_id):
                return                                       # paused: the owner's tabs aren't watched at all
            if e.event == "sound":
                self.on_sound(br, e)
                return
            self.on_tab(br, e)
            if e.event == "removed":
                br.tabs.pop(e.tab_id, None)
            else:
                host = urlsplit(e.url).hostname or "" if e.url else br.tabs.get(e.tab_id, ("", False))[0]
                br.tabs[e.tab_id] = (host, bool(e.active))
                if e.event == "updated" and e.same_document is not True:   # HAND-492: a route change keeps ids
                    br.navs[e.tab_id] = br.navs.get(e.tab_id, 0) + 1


_cancels: set["asyncio.Task[None]"] = set()


async def _pass_through(br: Bridge, tab_id: int, url: str, active: bool = True, disarm: bool = True) -> None:
    """The owner's own new-tab click caught by a listener in a tab no task drives now: opened as they asked (cmd/ctrl
    or middle click: in the background; with shift, or a plain target=_blank click: in front, as the browser would), the
    tab disarmed, unless a task still holds it (a loop between rounds, L31)."""
    try:
        await br.call("open_plain", {"tab_id": tab_id, "url": url, "active": active}, timeout=10)
    finally:
        if disarm:
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


def _hidden(window: Any) -> str:
    """Why a tab's window doesn't show it on screen ("" when it does): minimized (FERB's own window always is), or
    another tab is the one it shows (round 4, item 2)."""
    if not isinstance(window, dict):
        return "the extension didn't say where the tab's window is"
    if window.get("minimized"):
        return "its window is minimized"
    return "" if window.get("active") else "the tab isn't the one its window shows"


def _window_bounds(window: dict[str, Any]) -> tuple[float, float, float, float] | None:
    """A tab window's screen bounds as the extension reported them (x, y, w, h), or None when malformed."""
    b = window.get("bounds")
    if not (isinstance(b, list) and len(b) == 4 and all(isinstance(v, (int, float)) and not isinstance(v, bool)
                                                        for v in b)):
        return None
    return float(b[0]), float(b[1]), float(b[2]), float(b[3])


Rect = tuple[float, float, float, float]
# Acts that scroll their element to the viewport's centre before acting on it (content.js `act` for a click or hover,
# and `rect` for real input: scrollIntoView, block and inline "center", instant)
CENTRING = frozenset({"click", "dblclick", "rightclick", "hover"})
CENTRING_REAL = frozenset({"type", "press"})
POINTER_KINDS = frozenset({"click", "dblclick", "rightclick", "hover", "drag"})   # what a screenshot point takes
STILL = frozenset({"select", "upload"})          # acts that never scroll the page (no focus, no scroll into view)
# HAND-447: acts that put input into the page (what it may hold unsaved): leaving the tab after one needs leave_page
WRITES = frozenset({"type", "select", "press", "upload"})


def _numbers(v: Any, n: int) -> tuple[float, ...] | None:
    if not (isinstance(v, list) and len(v) == n and all(isinstance(x, (int, float)) and not isinstance(x, bool)
                                                        and math.isfinite(x) for x in v)):
        return None
    return tuple(float(x) for x in v)


@dataclass(frozen=True)
class _Page:
    """Where the latest snapshot saw the tab (L33, FERB's cursor; extension 0.15.9, optional fields): its window as the
    extension reported it, and the top frame's layout viewport (w, h), visual viewport (x, y, w, h and its pinch zoom:
    what the screen shows of the page), own scroll (top, height, viewport height, left, width) and whether its body
    scrolls by itself, CSS px. Geometry only; None for a fact an older extension doesn't give."""
    window: dict[str, Any] | None
    layout: tuple[float, ...] | None
    visual: tuple[float, ...] | None
    scroll: tuple[float, ...] | None
    body_scrolls: bool = False

    @classmethod
    def of(cls, window: Any, top: dict[str, Any]) -> "_Page":
        s = top.get("scroll")
        scroll = _numbers([s.get(k) for k in ("y", "height", "viewport_h", "x", "width")], 5) \
            if isinstance(s, dict) else None
        layout, visual = _numbers(top.get("viewport"), 2), _numbers(top.get("visual"), 5)
        return cls(window if isinstance(window, dict) else None,
                   layout if layout is not None and layout[0] > 0 and layout[1] > 0 else None,
                   visual if visual is not None and visual[2] > 0 and visual[3] > 0 and visual[4] > 0 else None,
                   scroll, top.get("body_scrolls") is True)

    def mapping(self) -> Rect | None:
        """The page's part the screen shows, in CSS px, as placement maps it onto the window's page area: the layout
        viewport (the page area holds it whole, a classic scrollbar included, which the visual viewport leaves out),
        shifted and shrunk by a pinch zoom."""
        if self.layout is None or self.visual is None:
            return None
        (lw, lh), (ox, oy, _, _, scale) = self.layout, self.visual
        return ox, oy, lw / scale, lh / scale

    def centred(self, rect: Rect) -> Rect | None:
        """Where an act that centres its element (CENTRING) will have scrolled it: the page's own vertical scroll, from
        the snapshot's scroll facts, as far as the page can scroll. Its place in the page doesn't change when the page
        scrolls, so this holds after FERB's own scrolls too. None when FERB can't tell: the page is pinch-zoomed,
        scrolls sideways or has its body scroll instead, the facts are missing, or the element is beside the viewport.
        The element's own boxes (fixed, sticky, a scroll box) are the caller's to check."""
        if self.body_scrolls or self.layout is None or self.visual is None or self.scroll is None:
            return None
        (lw, _), scale = self.layout, self.visual[4]
        top, height, view, _, width = self.scroll
        if abs(scale - 1) > 1e-3 or width > lw + 1 or not 0 <= rect[0] + rect[2] / 2 <= lw:
            return None
        to = min(max(top + rect[1] + rect[3] / 2 - view / 2, 0.0), max(height - view, 0.0))
        return rect[0], rect[1] - (to - top), rect[2], rect[3]


def _motion(raw: dict[str, Any]) -> tuple[str | None, bool]:
    """An element's scroll facts from the extension (L33): fixed or sticky (itself or an ancestor as the page is drawn),
    or "unknown" (fixed inside a box that holds fixed descendants: it scrolls with the page after all), and whether a box
    other than the page scrolls it (its scroll container, a carousel, hidden overflow)."""
    pinned = raw.get("pinned")
    return (pinned if pinned in ("fixed", "sticky", "unknown") else None), \
        bool(raw.get("scroll_box") or raw.get("container"))


class CaptureUnavailable(DriverError):
    """HAND-497: the tab couldn't be captured for the owner's card; its words are plain ("couldn't capture the page")."""


class OwnerOnlyImage:
    """HAND-497: an unmasked picture of a page for the owner's eyes only (the payment card), from
    `_TabDriver.owner_view`. It never enters a tool result, a model's context, a trace, the journal or a store: it holds
    its bytes behind `jpeg()`, shows only its size as text, and refuses JSON (no __dict__), pickling and copying into
    anything that serializes."""
    __slots__ = ("_jpeg",)

    def __init__(self, jpeg: bytes) -> None:
        self._jpeg = jpeg

    def jpeg(self) -> bytes:
        return self._jpeg

    def __repr__(self) -> str:
        return f"<owner-only page picture, {len(self._jpeg)} bytes>"

    __str__ = __repr__

    def __reduce__(self) -> Any:
        raise TypeError("an owner-only page picture is never stored or copied")


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
    searches: dict[str, tuple[str, int, int, int, bool]] = field(default_factory=dict)   # find: payload, counts, current
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
    media_ids: dict[str, int] = field(default_factory=dict)  # a media element's own id (L31) -> its snapshot index
    masking: str = ""               # how the latest screenshot's secret fields were found, when worth saying (L13)
    unread_shot: str | None = None  # a screenshot of a page the extension can't read: no point resolves on it (I1)
    native_shot: bool = False       # the latest screenshot is the whole browser window (a debugger-proof tab)
    input_note: str = ""            # the latest capture went on although the tab's input readiness failed (round 5)
    timing: dict[str, Any] | None = None   # the extension's step times for the latest read of it (0.19.6)
    shot_timing: dict[str, Any] | None = None   # and for its latest screenshot
    stuck: bool = False             # its latest snapshot timed out: the page isn't answering (HAND-435)
    # the page an owner's tab held when the task took it over: the tab is known by its id, wherever it navigates
    taken_over_from: dict[str, str] = field(default_factory=dict)
    closed: bool = False            # closed in the browser (the owner): listed as closed, never driven again
    left: str = ""                  # why FERB no longer drives it (closed, taken by the owner, lost on a reconnect)
    closing: bool = False           # FERB itself is closing it: its removal is no owner's stop
    held: bool = False              # it counts in its bridge's keepalive holds
    frame_urls: dict[int, str] = field(default_factory=dict)   # the latest snapshot's frames' URLs (masked)
    commerce: dict[str, Any] = field(default_factory=dict)     # the page's structured amount/merchant (protocol 18)
    amounts: tuple[str, ...] = ()   # GATE-578: every money amount its latest snapshot shows (text and labels)
    muted: bool = False             # HAND-207: FERB muted it (the browser's tab mute); its task may unmute it
    typed_at: int | None = None     # HAND-447: the tab's load count when FERB last typed, filled or uploaded in it
    page: _Page | None = None       # where the latest snapshot saw the tab, for FERB's cursor (L33)
    motion: dict[str, tuple[str | None, bool]] = field(default_factory=dict)   # element id -> its scroll facts (L33)
    moved: bool = False             # FERB scrolled the page since the snapshot: its rects aren't where they were (L33)
    # D31: the web terminal element FERB just typed approved text into, and that Yes: it covers one Return there
    armed: tuple[str, str] | None = None
    opener: str | None = None       # the task tab that opened this one (a link's new tab, an adopted popup)
    title: str = ""                 # the page title it last showed FERB (redacted): names it in a cleanup leftover
    # HAND-488: earlier ids a screenshot's snapshot found the same node for (they keep working), and what it said
    carried: set[str] = field(default_factory=set)
    ids_note: dict[str, Any] = field(default_factory=dict)
    http_status: int | None = None  # HAND-494: the last load's document status, as the extension's debugger saw it
    markup: tuple[str, str, list[int]] = ("", "", [])   # HAND-496: the last html read (its key, its text, skipped frames)


def _status(v: dict[str, Any]) -> int | None:
    """HAND-494: a reply's `http_status` (extension 0.19.8), only as a plain HTTP status number."""
    n = v.get("http_status")
    return n if isinstance(n, int) and not isinstance(n, bool) and 100 <= n <= 599 else None


def _read_clipboard() -> tuple[int, str | None, bool]:
    from ferb.act.mac.clipboard import read_general
    return read_general()


SECRET_TERMINAL = ("that's a terminal in the page: FERB puts no password or code into a terminal (D31); it would run "
                   "as the owner's input there")


def _no_secret_into_terminal(info: PageElement) -> None:
    """D31 round 2, O3: a secret or a login code never goes into a web terminal (mac.secret asks for a native one)."""
    if info.terminal:
        raise Refused(SECRET_TERMINAL)


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
        # daemon supplied (L31): a loop holds this task's tabs between its rounds; the owner's own tab is then theirs
        # again for downloads and new tabs (owner, 2026-10-25: "give them back between rounds")
        self.task_resting: Callable[[str], bool] = lambda task: False
        self.task_root: Callable[[str], str] = lambda task: task      # daemon supplied: the task tree (tab cap)
        self.max_tabs: Callable[[], int] = lambda: 50                 # daemon supplied: browser.task_max_tabs
        # D31 round 2, O2: the Mac clipboard (change count, text, concealed) for a paste key into a web terminal
        self.clipboard: Callable[[], tuple[int, str | None, bool]] = _read_clipboard
        self.redact_text: Callable[[str], str] = Redactor().text      # daemon supplied: its redactor (rule 14)
        self._opening: dict[str, int] = {}                            # task tree -> tabs being opened now (tab cap)
        self._released: set[str] = set()                              # tasks that let go of their tabs
        # HAND-201 (tab groups), daemon supplied: a task tree's words for its group's label ("FERB · <short
        # task>", group_title); whether the task is paused (its group stays: nothing closes at its end); and the task
        # trees whose owner asked to keep the tabs (`keep_tabs_open`: at the end they leave the group, open).
        self.task_title: Callable[[str], str] = lambda root: ""
        self.task_paused: Callable[[str], bool] = lambda task: False
        self._keep_roots: set[str] = set()
        # daemon supplied (B6): (browser pid, window bounds, tab title) -> the secret fields Accessibility finds in that
        # window's one page area, that area, and whether every field it found has a box; None: it can't tell
        self.native_secrets: Callable[[int, tuple[float, float, float, float], str],
                                      Awaitable[tuple[list[tuple[float, float, float, float]],
                                                      tuple[float, float, float, float], bool] | None]] | None = None
        # daemon supplied (round 3, I1): (browser pid, window bounds, tab title) -> a native capture of that on-screen
        # browser window (JPEG, its masking note), for a tab the debugger can't attach to; (None, why) when it can't
        self.native_capture: Callable[[int, tuple[float, float, float, float], str],
                                      Awaitable[tuple[bytes | None, str]]] | None = None
        # daemon supplied (L33): (element rect, visual viewport, browser pid, window bounds, tab title) -> that rect in
        # screen points on the page area of the on-screen window showing the tab (MacComputerDriver.place_on_web_area);
        # None when it can't be placed
        self.native_place: Callable[[Rect, Rect, int, Rect, str], Awaitable[Rect | None]] | None = None
        self._n = 0
        if isinstance(getattr(hub, "drivers", None), list):
            hub.drivers.append(self)

    async def _connected(self, br: Bridge) -> Bridge:
        """The one check every browser tool's profile goes through, the owner's rule (2026-10-26): "see if the browser
        is closed or open; if open then keep using through the extension; if not open, once that causes focus shift -
        ok - but then after opened continue using through the extension". Connected: used as is. Not connected: the
        profile starts with no window first (spike 19: nothing moves, and it connects in about a second); if its
        extension still hasn't come, the profile's own window opens (the accepted focus shift) and FERB carries on
        through the extension. An unpaired or refused extension stops it: only the owner can fix those. Never a
        visible wake page."""
        if br.conn is None and self.wake is not None:
            kwargs: dict[str, Any] = {"launch": self.launch} if self.launch else {}
            # Never restore a previously focused app after this wait: the owner can switch apps meanwhile.
            open_known_profile(br.binding.browser, br.binding.folder, None, self.known(),
                               is_running=self.is_running, **kwargs)
            if not await self._came(br, WINDOWLESS_WAIT_S):
                reason = self.hub.refused.get(br.binding.install_id)
                if reason is None and br.binding.browser in self.hub.unpaired.values():
                    # Live 2026-10-26: a reinstalled extension connects with a new identity and no pairing, so FERB
                    # can't tell it is this profile's. Only the owner can pair it, while they are in its window.
                    raise DriverError(
                        f"{br.name}'s FERB extension is connected but not paired with FERB (a reinstall gives it a new "
                        f"identity): pair it with browser_pair profile={br.binding.browser}/{br.binding.folder} (FERB "
                        "asks the owner once). Never work around this through Mac tools (the browser's menus, address "
                        "bar or clicks): that takes over the owner's screen.")
                if reason is not None:
                    raise DriverError(f"{br.name}'s FERB extension was refused: {reason}.")
                open_known_profile(br.binding.browser, br.binding.folder, None, self.known(), window=True, **kwargs)
                if not await self._came(br, WINDOW_WAIT_S):
                    if (known := self.hub.problem(br.binding)) is not None:   # e.g. too old to reach a hello (HAND-406)
                        raise DriverError(f"{br.name} didn't connect, even with its window open: {known}. Tell the "
                                          "owner; only they can fix this.")
                    raise DriverError(f"{br.name} didn't connect, even with its window open: its FERB extension may "
                                      "be turned off there; ask the owner to reload the FERB extension in that "
                                      "profile.")
        if br.conn is None:
            raise DriverError(f"{br.name} isn't connected")
        return br

    def known(self) -> list[BrowserProfile]:
        """The profiles FERB may start: those discovered, plus the paired ones of a browser discovery found none in
        (Full Disk Access off, its Local State unreadable), from FERB's own pairing records (HAND-407)."""
        pairings = getattr(self.hub, "pairings", None)
        return with_paired(self.profiles(), [(b.browser, b.folder) for b in pairings.all()] if pairings else [])

    @staticmethod
    async def _came(br: Bridge, wait: float) -> bool:
        """Whether that profile's extension connected within `wait` seconds."""
        try:
            await asyncio.wait_for(br.connected.wait(), wait)
        except asyncio.TimeoutError:
            return br.conn is not None
        return True

    async def _open_in(self, br: Bridge, url: str, owner: str | None, *,
                       window_id: int | None = None, allow_foreground: bool = False,
                       registered: Callable[[TabRef], None] | None = None, attached: bool = False,
                       abandon: Callable[[], bool] | None = None) -> TabRef:
        # The extension opens an inactive tab in the selected existing window or returns window choices. App focus belongs to the owner,
        # including while this command is pending; a newly foreground browser is not proof of focus theft.
        # D35: the tab opens blank (the extension prepares it to hold a download) and becomes this task's tab before the
        # URL loads, so a URL that is a download is already the task's download, never the browser's.
        # within the task's tab cap, or refused (a fact); counted as "being opened" until the tab is registered
        reserved = [self._reserve_tab(owner)] if owner else []
        try:
            return await self._open_reserved(br, url, owner, reserved, window_id=window_id,
                                             allow_foreground=allow_foreground, registered=registered,
                                             attached=attached, abandon=abandon)
        finally:
            self._unreserve(reserved)

    def _unreserve(self, reserved: list[str]) -> None:
        while reserved:
            root = reserved.pop()
            self._opening[root] = self._opening.get(root, 1) - 1
            if self._opening[root] <= 0:
                self._opening.pop(root, None)

    async def _open_reserved(self, br: Bridge, url: str, owner: str | None, reserved: list[str], *,
                             window_id: int | None = None, allow_foreground: bool = False,
                             registered: Callable[[TabRef], None] | None = None, attached: bool = False,
                             abandon: Callable[[], bool] | None = None) -> TabRef:
        args: dict[str, Any] = {"url": BLANK}
        if window_id is not None:
            args["window_id"] = window_id
        if allow_foreground:
            args["allow_foreground"] = True
        if owner and not attached:                       # the task's collapsed group (HAND-201)
            args["group"] = self._group(owner)
        if not attached:                                 # HAND-207: a tab FERB opens is muted (protocol 19)
            args["muted"] = True
        v = await br.call("open", args, timeout=30)
        if (v.get("opened") is False and v.get("reason") == "window_selection_required" and window_id is None
                and not allow_foreground):
            # No FERB window in this profile (a cold browser, or one woken with no startup window): FERB makes its own
            # minimized, unfocused window once and works in it through the extension. It stays (a blank tab keeps it),
            # so later tasks reuse it; a possible Space shift once per browser session is the owner's accepted cost
            # (owner 2026-10-26: "if not open once that causes focus shift - ok - but then after opened continue using
            # through extension"; replaces the 2026-10-06 refusal, 46c0be7).
            # An open window in the profile takes a background tab first (no new window, no Space change); only a
            # profile with no window at all gets FERB's minimized window.
            # The window chosen last time in this profile, while it is still there, else an unfocused one first.
            windows = [w for w in v.get("windows") or [] if w.get("type", "normal") == "normal" and not w.get("incognito")]
            windows.sort(key=lambda w: (w.get("id") != br.window, bool(w.get("focused"))))
            v = await br.call("open", {**args, "window_id": windows[0]["id"]} if windows
                              else {**args, "allow_foreground": True}, timeout=30)
        if v.get("opened") is False:
            raise DriverError(str(v["detail"]), facts=v)
        if isinstance(v.get("window_id"), int):
            br.window = int(v["window_id"])               # remembered for this profile (protocol 18 fixes)
        self._n += 1
        ref = TabRef(br.device_id, br.binding.browser, br.binding.folder, br.gen, f"t{self._n}")
        t = self._tabs[ref.tab_id] = _Tab(ref, br, int(v["tab_id"]), owner, BLANK, attached=attached,
                                          muted=v.get("muted") is True)
        self._unreserve(reserved)                        # it counts as an open tab of the task from here
        self._hold(t)
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
        t.http_status = _status(loaded)
        return ref

    def _group(self, owner: str) -> dict[str, Any]:
        """The extension's group record for a task's tab: its tree (one group per task), the task, and the label."""
        root = self.task_root(owner)
        return {"root": root, "task": owner, "title": self.redact_text(group_title(self.task_title(root)))}

    def _hold(self, t: _Tab) -> None:
        if not t.attached and not t.held:
            t.held = True
            t.bridge.hold()

    def _unhold(self, t: _Tab) -> None:
        if t.held:
            t.held = False
            t.bridge.unhold()

    def keep_tabs_open(self, task: str) -> None:
        """The owner asked to keep this task's tabs (HAND-201): at its end they leave the group and stay open,
        each handed over like a result (`keep_open`). Task-level: the whole tree."""
        self._keep_roots.add(self.task_root(task))

    def _drop(self, t: _Tab, why: str) -> None:
        """FERB no longer drives this tab (closed, taken, lost): listed so, never driven, never closed by FERB."""
        t.closed, t.left = True, t.left or why
        self._unhold(t)

    def readopt(self, br: Bridge, listed: list[dict[str, Any]]) -> list[Awaitable[Any]]:
        """After a hello (an extension reconnect, a worker or extension restart, or this daemon's own restart): FERB's
        tabs are known by browser tab id, which holds for the browser session, not by connection (HAND-201).
        Each tab of this profile the daemon holds is re-adopted when the extension's records still list it (its element
        ids go: take a new snapshot) and dropped otherwise (closed or the owner's meanwhile, never closed by FERB). A
        listed tab the daemon doesn't hold (it restarted) becomes its task's again while the task runs; any tab whose
        task ended is closed (FERB opened it) or let go of (the owner's): the returned jobs, run after the welcome. No
        orphan is left to block the extension's self-reload."""
        seen = {int(r["tab_id"]): r for r in listed if isinstance(r, dict) and isinstance(r.get("tab_id"), int)}
        jobs: list[Awaitable[Any]] = []
        for t in [t for t in self._tabs.values() if t.bridge is br and not t.closed]:
            r = seen.pop(t.chrome_id, None)
            if r is None or bool(r.get("borrowed")) != t.attached:
                self._drop(t, "it was closed or taken while FERB's connection to the browser was down")
                continue
            t.ref = dataclasses.replace(t.ref, generation=br.gen)
            t.elements, t.points, t.visual, t.snapshot_json = {}, {}, None, ""
            if not (t.owner and self.task_running(t.owner)) and not (t.owner and self.task_paused(t.owner)):
                jobs.append(self._settle(t.ref))
        for chrome_id, r in seen.items():
            task, borrowed = r.get("task"), r.get("borrowed") is True
            if isinstance(task, str) and task and (self.task_running(task) or self.task_paused(task)):
                self._n += 1
                ref = TabRef(br.device_id, br.binding.browser, br.binding.folder, br.gen, f"t{self._n}")
                t = self._tabs[ref.tab_id] = _Tab(ref, br, chrome_id, task, str(r.get("url") or ""), attached=borrowed)
                self._hold(t)
            elif borrowed:
                jobs.append(br.call("release_tab", {"tab_id": chrome_id}, timeout=10))
            else:
                jobs.append(br.call("close", {"tab_id": chrome_id}, timeout=10))
        return jobs

    async def _settle(self, ref: TabRef) -> None:
        """A re-adopted tab whose task ended while the connection was down: closed or let go of, as at the task end."""
        try:
            await self.close(ref)
        except DriverError:
            pass                                         # the next hello lists it again

    def tab_taken(self, br: Bridge, chrome_id: int, how: str = "") -> None:
        """The owner moved a tab FERB opened out of its task's group (HAND-201): it is theirs now. FERB stops
        using it (the extension already let go of it) and never closes it; the supervisor hears of it."""
        for alias, t in self._tabs.items():
            if t.bridge is br and t.chrome_id == chrome_id and not t.closed:
                self._drop(t, "the owner moved it out of FERB's tab group, so it is theirs now")
                if t.owner and self.task_running(t.owner):
                    self.hub.publish("browser.owner_took", {"task_id": t.owner, "tab": alias, "profile": br.name,
                                                            "how": how})

    def owner_of(self, br: Bridge, chrome_id: int) -> str | None:
        """The task that opened this tab, if FERB did."""
        return next((t.owner for t in self._tabs.values() if t.bridge is br and t.chrome_id == chrome_id), None)

    def _live(self, br: Bridge, chrome_id: int) -> _Tab | None:
        """This driver's tab with that browser id, driven by a task that is still running (opened or borrowed). The
        owner's own tab that a loop holds between its rounds isn't driven (L31, owner 2026-10-25: "give them back
        between rounds"): a download there goes back to the browser and a new-tab link opens as the owner's own tab;
        the hold that keeps the page running (the debugger, its focus emulation) stays. A round's own actions, and
        its check, happen while the tab is driven, so what they start is FERB's (D35)."""
        return next((t for t in self._tabs.values() if t.bridge is br and t.chrome_id == chrome_id and t.owner
                     and not t.closed and self.task_running(t.owner)
                     and not (t.attached and self.task_resting(t.owner))), None)

    async def rest(self, owner: str, resting: bool) -> None:
        """A loop's task ends a round (`resting`) or starts one: the owner's own tabs it holds give their file picker
        back between rounds, and a round takes the interception again (L31, owner 2026-10-25: "give them back between
        rounds"; the debugger and focus emulation, which keep the page running, stay). Every extension FERB accepts
        knows the command (the minimum is past 0.15.10). Resting is best effort: a failure leaves the picker
        intercepted, the safe side. Taking the tab back for a round is confirmed (the reply says the picker is intercepted, or the
        extension holds no session, so its next attach intercepts), retried once, and otherwise raised for the
        supervisor's record; the extension also takes the picker back on the round's first acting command."""
        unconfirmed = []
        for t in [t for t in self._tabs.values() if t.owner == owner and t.attached and not t.closed]:
            if t.bridge.conn is None:
                continue
            for attempt in (1, 2) if not resting else (1,):
                try:
                    reply = await t.bridge.call("rest", {"tab_id": t.chrome_id, "resting": resting}, timeout=10)
                except (DriverError, OSError):
                    reply = None
                if resting or (isinstance(reply, dict) and (reply.get("file_chooser_intercepted") is True
                                                           or reply.get("held") is False)):
                    break
            else:
                unconfirmed.append(t.ref.tab_id)
        if unconfirmed:
            raise DriverError(f"the browser didn't confirm taking the file picker back for {', '.join(unconfirmed)}")

    def holds(self, br: Bridge, chrome_id: int) -> bool:
        """A running task still holds this tab, driven or resting between a loop's rounds: never let go of it from
        outside the task (a link's pass-through, a reconnect's sweep)."""
        return any(t.bridge is br and t.chrome_id == chrome_id and t.owner and not t.closed
                   and self.task_running(t.owner) for t in self._tabs.values())

    def tab_closed(self, br: Bridge, chrome_id: int) -> None:
        """The browser closed this tab: the task's inventory says so; a reopened tab is a new one. Not FERB's own close,
        while its task runs: the owner stopped that part of the task (HAND-201), published for the supervisor
        (`browser.owner_stopped`; step 8 asks the owner whether to continue). `part` is "group" when no other tab FERB
        opened for that task tree is left on this profile (the owner closed the group), else "tab"."""
        for alias, t in self._tabs.items():
            if t.bridge is not br or t.chrome_id != chrome_id or t.closed:
                continue
            self._drop(t, "the owner closed this tab")
            if t.closing or not t.owner or not self.task_running(t.owner):
                continue
            root = self.task_root(t.owner)
            left = any(o is not t and o.bridge is br and not o.closed and not o.attached and o.owner
                       and self.task_root(o.owner) == root for o in self._tabs.values())
            self.hub.publish("browser.owner_stopped", {"task_id": t.owner, "tab": alias, "profile": br.name,
                                                       "borrowed": t.attached,
                                                       "part": "tab" if left or t.attached else "group"})

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

    @staticmethod
    def _note_input(t: _Tab) -> None:
        """HAND-447: FERB typed, filled or uploaded in this tab: until its next load the page may hold unsaved input,
        so leaving it (navigate, close) needs the worker's leave_page."""
        t.typed_at = t.bridge.navs.get(t.chrome_id, 0)

    def typed_since_load(self, tab: TabRef) -> bool:
        """Whether FERB typed, filled or uploaded in this task tab since the page last loaded (a load seen as the
        browser's `updated` tab event). A fact about FERB's own input, never about the page's content."""
        t = self._tabs.get(tab.tab_id)
        return t is not None and t.typed_at is not None and t.typed_at == t.bridge.navs.get(t.chrome_id, 0)

    def closes(self, tab: TabRef) -> bool:
        """Whether `close` would close this tab (one FERB opened, not handed over), rather than only let go of it."""
        t = self._tabs.get(tab.tab_id)
        return t is not None and not t.attached and not t.keep_open and not t.closed

    def opened_for(self, tab: TabRef) -> list[dict[str, Any]]:
        """New tabs this tab started (a link to a new tab, a page's window.open), now this task's, not yet reported."""
        t = self._tabs.get(tab.tab_id)
        if t is None:
            return []
        out, t.opened = t.opened, []
        return out

    def _open_tabs(self, root: str) -> int:
        """Tabs FERB opened for this task tree that are still open, in every browser driver, and those being opened now
        (ruling, step 12e): its own tabs (opened by the worker, from a link, or a page's popup) and new tabs opened
        from the owner's borrowed tab; never the owner's borrowed tab itself, nor a tab already handed over (kept
        open) or closed."""
        listed = getattr(self.hub, "drivers", None)
        drivers: list[_TabDriver] = listed if isinstance(listed, list) and self in listed else [self]
        return sum(1 for d in drivers for t in d._tabs.values()
                   if t.owner and not t.closed and not t.keep_open and (not t.attached or t.opener is not None)
                   and d.task_root(t.owner) == root) + sum(d._opening.get(root, 0) for d in drivers)

    def _tab_cap(self, owner: str) -> str | None:
        """Why this task tree may not open another tab now (browser.task_max_tabs), or None."""
        cap = self.max_tabs()
        if self._open_tabs(self.task_root(owner)) >= cap:
            return (f"this task already has {cap} tabs open (browser.task_max_tabs), so FERB didn't open another; close "
                    "tabs it no longer needs (close) and open it again")
        return None

    def _reserve_tab(self, owner: str) -> str:
        """Count one tab being opened for this task tree, within its cap; refused (a fact) past it."""
        if (why := self._tab_cap(owner)) is not None:
            raise Refused(why)
        root = self.task_root(owner)
        self._opening[root] = self._opening.get(root, 0) + 1
        return root

    async def _orphaned(self, ref: TabRef) -> None:
        """A tab registered after its task let go of its tabs: let go of it too."""
        try:
            await self.close(ref)
        except DriverError:
            pass

    async def open_from(self, br: Bridge, opener_id: int, url: str, window_id: int | None) -> None:
        """A task tab's link to a new tab: opened by FERB (blank, armed, loaded, inactive) in the opener's window, as
        that task's tab; the opener's next result names it. Opened from the owner's borrowed tab, it is released (never
        closed) like its opener. Past the task's tab cap it isn't opened at all (never handed to the browser as an
        ordinary tab, whose downloads would skip every limit), and the opener's next result says so."""
        opener = self._live(br, opener_id)
        if opener is None or opener.owner is None:
            return
        owner = opener.owner
        from ferb.act.browser.tools import check_url
        entry: dict[str, Any] = {"url": url, "via": "link"}
        opener.opened.append(entry)                      # named at once: its load may become a long download
        try:
            check_url(url)
            await self._open_in(br, url, owner, window_id=window_id, attached=opener.attached,
                                registered=lambda ref: (entry.update(tab=ref.tab_id),
                                                        self._opened_by(ref.tab_id, opener.ref.tab_id)),
                                # let go of (release) or ended without one (its record gone): never load it as the task's
                                abandon=lambda: owner in self._released or not self.task_running(owner))
        except (Refused, DriverError) as e:
            entry.update(error=f"FERB didn't open the link's new tab: {e}")

    def adopt(self, br: Bridge, opener_id: int, chrome_id: int, url: str) -> dict[str, Any] | None:
        """A popup the page hook of a live task tab opened (claimed by its token): that task's tab, released (never
        closed) when the opener was the owner's borrowed tab. Returns the extension's record for it (`adopt_tab`: it
        joins the task's group unless borrowed); None: not the task's (the task let go of its tabs). Past the tab cap it
        is refused: `{"refused": why}`, and the extension closes the blank popup before its page loads anything."""
        opener = self._live(br, opener_id)
        if opener is None or opener.owner is None or opener.owner in self._released:
            return None
        record = {"tab_id": chrome_id, "task": opener.owner, "root": self.task_root(opener.owner),
                  "borrowed": opener.attached, "title": self._group(opener.owner)["title"],
                  "muted": not opener.attached}         # a popup of a FERB tab is FERB's: muted (HAND-207)
        if any(t.bridge is br and t.chrome_id == chrome_id for t in self._tabs.values()):
            return record
        if (why := self._tab_cap(opener.owner)) is not None:
            opener.opened.append({"url": url, "error": f"the page's new tab was closed before it loaded: {why}"})
            return {"refused": why}
        self._n += 1
        ref = TabRef(br.device_id, br.binding.browser, br.binding.folder, br.gen, f"t{self._n}")
        self._hold(self._tabs.setdefault(ref.tab_id, _Tab(ref, br, chrome_id, opener.owner, url,
                                                          attached=opener.attached, opener=opener.ref.tab_id,
                                                          muted=not opener.attached)))
        opener.opened.append({"tab": ref.tab_id, "url": url, "via": "the page opened it"})
        return record

    def downloads_of(self, task: str) -> list[dict[str, Any]]:
        """Every download this task's tabs in this identity's profiles started; a registered checker sees its
        worker's."""
        if self.downloads is None:
            return []
        tasks = {task} | ({p} if (p := self.inspection_parent(task)) else set())
        return self.downloads.facts_for(tasks, self.identity.value)

    async def navigate(self, tab: TabRef, url: str, leave_page: str | None = None) -> None:
        """`leave_page` (HAND-442): the worker's choice for a page that asks before it is left, carried out by the
        extension; without it such a page ends the call with its dialog (`reason: dialog_pending`)."""
        t = self._tab(tab)
        v = await t.bridge.call("navigate", {"tab_id": t.chrome_id, "url": url,
                                             **({"leave_page": leave_page} if leave_page else {})}, timeout=30)
        if v.get("navigated") is False:                  # e.g. download_capture_unavailable: the URL never loaded
            raise DriverError(str(v.get("detail") or "the tab didn't navigate"), facts=v)
        t.url = str(v.get("url") or url)
        t.http_status = _status(v)

    def http_status(self, tab: TabRef) -> int | None:
        """HAND-494: the HTTP status of the page the tab's last open or navigate loaded, when the extension saw it."""
        t = self._tabs.get(tab.tab_id)
        return t.http_status if t is not None else None

    async def tabs(self) -> list[TabRef]:
        return [t.ref for t in self._tabs.values()]

    def _lifecycle(self, t: _Tab, result: dict[str, Any]) -> dict[str, Any]:
        return {"tab": t.ref.tab_id, "tab_ref": f"{t.ref.browser}:{t.ref.profile}:{t.chrome_id}",
                "profile": f"{t.ref.browser}/{t.ref.profile}", "url": result.get("url") or t.url,
                "borrowed": t.attached, "closed": bool(result.get("closed")),
                "kept_open": bool(result.get("kept_open")), "released": bool(result.get("released") or result.get("closed"))}

    async def keep_open(self, tab: TabRef) -> dict[str, Any]:
        """Keep the tab open after the task. The owner's borrowed tab is only marked kept and stays held until its task
        (a loop's: until the loop) ends, when `close` lets go of it, kept open: letting go detaches the debugger, which
        turns focus emulation off, and a site that pauses on blur paused the very playback the owner asked to keep
        (L31, live cc5debe4, 2026-10-25: Instagram 0.3 s after each round's keep_open). Such a tab is never closed, so
        nothing is lost holding it; the browser is asked only whether it still has the tab. A tab FERB opened is
        handed over at once: the extension records it kept before it detaches, so a lost answer can't let a later
        cleanup close it."""
        t = self._tab(tab)
        if t.attached:
            # the browser confirms the tab is open now; letting go of it, kept open, is confirmed at the task's end
            listed = await t.bridge.call("list_tabs", {}, timeout=10)
            if not any(int(x.get("tab_id", -1)) == t.chrome_id for x in listed.get("tabs", [])):
                raise DriverError(f"tab {tab.tab_id} isn't open in the browser any more, so it can't be kept open")
            t.keep_open = True
            return self._lifecycle(t, {"kept_open": True}) | {
                "held_until_task_end": "the browser has the tab open now; FERB keeps holding it while the task runs "
                                       "and lets go of it, open, when the task ends"}
        t.keep_open = True
        v = await t.bridge.call("release_tab", {"tab_id": t.chrome_id, "keep_open": True}, timeout=10)
        if not v.get("released") or not v.get("kept_open"):
            raise DriverError("browser did not confirm keeping the tab open and releasing control")
        return self._lifecycle(t, v)

    async def close(self, tab: TabRef, leave_page: str | None = None) -> dict[str, Any]:
        t = self._tabs.get(tab.tab_id)
        if t is None:
            return {"tab": tab.tab_id, "released": True, "closed": False, "already_released": True}
        if t.closed:                                   # the browser already closed it: nothing to let go of
            v: dict[str, Any] = {"closed": True, "released": True}
        elif t.bridge.conn is None:
            raise DriverError(f"cannot confirm cleanup of {tab.tab_id}: {t.bridge.name} is disconnected")
        elif t.attached or t.keep_open:
            v = await t.bridge.call("release_tab", {"tab_id": t.chrome_id, "keep_open": t.keep_open}, timeout=10)
            if not v.get("released") or (t.keep_open and not v.get("kept_open")):
                raise DriverError(f"browser did not confirm releasing {tab.tab_id}")
        else:
            # A tab FERB holds by its browser id once a hello re-adopted it (readopt): until then a reconnect left it
            # unconfirmed, and it is not closed blind.
            if t.bridge.gen != t.ref.generation:
                raise DriverError(f"cannot confirm cleanup of {tab.tab_id}: browser connection changed")
            t.closing = True                             # its removal is FERB's own, no owner's stop
            v = await t.bridge.call("close", {"tab_id": t.chrome_id,
                                              **({"leave_page": leave_page} if leave_page else {})}, timeout=10)
            if v.get("reason") in ("dialog_pending", "stayed"):   # HAND-442: the page asked; the tab stays the task's
                t.closing = False
                raise DriverError(str(v.get("detail") or "the page asked before closing"), facts=v)
            if v.get("reason") == "not_ferbs":           # the extension never closes a tab it has no record of opening
                v = {**v, "released": True, "left_open": True}
            elif not v.get("closed") and not (v.get("kept_open") and v.get("released")):
                t.closing = False
                raise DriverError(f"browser did not confirm closing {tab.tab_id}")
        self._tabs.pop(tab.tab_id, None)
        self._unhold(t)
        if self.uploads is not None and v.get("closed"):
            self.uploads.closed(tab_key(t.bridge.name, t.chrome_id))   # rule 17: its upload copies go with its tree
        return self._lifecycle(t, v)

    async def snapshot(self, tab: TabRef) -> PageSnapshot:
        t = self._tab(tab)
        nav = t.bridge.navs.get(t.chrome_id, 0)
        gen = t.snap_gen + 1
        try:
            v = await t.bridge.call("snapshot", {"tab_id": t.chrome_id, "snap": gen}, timeout=30)
        except DriverError as e:
            t.stuck, t.timing, t.shot_timing = e.facts.get("reason") == "page_not_responding", None, None
            raise
        t.stuck, t.timing, t.shot_timing = False, _steps(v.get("steps")), None
        elements: dict[str, tuple[int, int, int, PageElement]] = {}
        containers: dict[str, tuple[int, str]] = {}
        text, title, url, truncated = "", "", t.url, False
        scroll: dict[str, Any] | None = None
        top: dict[str, Any] = {}
        scrollers: list[dict[str, Any]] = []
        media: list[dict[str, Any]] = []
        media_refs: dict[int, tuple[int, int]] = {}
        media_ids: dict[str, int] = {}
        motion: dict[str, tuple[str | None, bool]] = {}
        for f in v["frames"]:
            fid = int(f["frame_id"])
            for m in f.get("media", []):               # L26: one index across frames, as browser.media takes it
                media_refs[len(media)] = (fid, int(m["index"]))
                entry: dict[str, Any] = {"index": len(media)}
                if isinstance(m.get("id"), str) and MEDIA_ID.fullmatch(m["id"]):
                    # the element's own id for the page's life (L31); a child frame numbers its own, as scrollers do
                    entry["id"] = m["id"] if fid == 0 else f"f{fid}:{m['id']}"
                    media_ids[entry["id"]] = len(media)
                media.append(entry | {k: m[k] for k in MEDIA_FIELDS if k in m} | ({"frame": fid} if fid else {}))
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
                if any(_motion(raw)):
                    motion[eid] = _motion(raw)
            if fid == 0:
                text, title, url = f.get("text", ""), f.get("title", ""), f.get("url", url)
                # the page's sideways scroll is FERB's cursor's fact (L33), not the worker's
                scroll = {k: n for k, n in (f.get("scroll") or {}).items() if k not in ("x", "width")} or None
                top = f
        child_text = [str(f.get("text", "")) for f in v["frames"] if int(f["frame_id"]) != 0 and f.get("text")]
        if child_text:
            text = text + "\n[Embedded frames]\n" + "\n".join(child_text)
        dialogs = [f"{d.get('type')}: {str(d.get('message') or '')} ({d.get('handled')})"
                   for d in v.get("dialogs", []) if isinstance(d, dict)]
        t.visual = None
        t.points.clear()
        t.snap_gen, t.snap_nav, t.elements, t.url = gen, nav, elements, url
        t.carried, t.ids_note = set(), {}
        t.title = self.redact_text(title) if title else t.title
        t.frame_urls = {int(f["frame_id"]): str(f.get("url") or "") for f in v["frames"]}
        t.commerce = next((dict(f["commerce"]) for f in v["frames"] if int(f["frame_id"]) == 0
                           and isinstance(f.get("commerce"), dict)), {})
        t.amounts = page_amounts([text, *(e.label or "" for *_, e in elements.values())])
        t.containers, t.media, t.media_ids = containers, media_refs, media_ids
        t.page, t.motion, t.moved = _Page.of(v.get("window"), top), motion, False
        result = PageSnapshot(tab, gen, url, title, [e for *_, e in elements.values()], text, truncated, dialogs,
                              scroll, scrollers, media, frame_urls(v["frames"]), loading=v.get("loading") is True)
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

    async def html(self, tab: TabRef, element: str | None = None, selector: str | None = None, frame: int | None = None,
                   scripts: bool = False, offset: int = 0, max_chars: int = 12000) -> dict[str, Any]:
        """HAND-496: the page's real markup (extension 0.19.8's `html`), read only, paged like `read`: the whole page
        (every frame it draws), one element of the latest snapshot, or a CSS selector's matches. Each element of the latest
        snapshot carries ferb-element="<its id>", a frame element ferb-frame="<frame id>"; the extension masks what the
        snapshot masks (HAND-496's rules in content.js `markup`). offset 0 reads afresh; later pages keep that read."""
        from ferb.act.browser.ext_protocol import HTML_EXTENSION_VERSION, version_ok
        t = self._tab(tab)
        key = json.dumps([element, selector, frame, scripts])
        if offset == 0 or t.markup[0] != key:
            running = str(getattr(t.bridge, "version", "") or "")
            if not version_ok(running, HTML_EXTENSION_VERSION):
                raise Refused(f"the FERB extension in this profile is {running or 'an older version'}; reading a page's "
                              f"markup needs {HTML_EXTENSION_VERSION} or later. Use the snapshot, find and read meanwhile. "
                              "Tell the owner: their browser updates the extension by itself, or they can update it on "
                              "its extensions page")
            args: dict[str, Any] = {"tab_id": t.chrome_id, "snap": t.snap_gen, "scripts": bool(scripts)}
            if element:
                fid, snap, local, _ = self._resolve(self.element_ref(tab, element))[1:]
                args |= {"frame_id": fid, "snap": snap, "el": local}
            elif frame is not None:
                args["frame_id"] = int(frame)
            if selector:
                args["selector"] = selector
            v = await t.bridge.call("html", args, timeout=30)
            nonce = str(v.get("nonce") or "")
            ids = {(f, local): eid for eid, (f, gen, local, _) in t.elements.items() if gen == t.snap_gen}
            mark = re.compile(rf'ferb-el-{re.escape(nonce)}="(\d+)"') if nonce else None
            parts, skipped, errors = [], [], []
            for fr in v.get("frames") or []:
                fid = int(fr.get("frame_id", 0))
                if fr.get("undrawn"):
                    skipped.append(fid)
                    continue
                if fr.get("error"):
                    errors.append(f"frame {fid}: {fr['error']}")
                    continue
                text = str(fr.get("html") or "")
                if mark is not None:
                    text = mark.sub(lambda m, f=fid: (f'ferb-element="{ids[(f, int(m.group(1)))]}"'
                                                      if (f, int(m.group(1))) in ids else ""), text)
                parts.append(f"<!-- frame {fid}: {fr.get('url') or ''}"
                             f"{' (cut at its size limit)' if fr.get('truncated') else ''} -->\n{text}")
            if errors and not parts:
                raise Refused("; ".join(errors)[:500])
            t.markup = (key, "\n".join(parts), skipped)
        _, text, skipped = t.markup
        end = min(offset + max_chars, len(text))
        return {"tab": tab.tab_id, "snapshot": t.snap_gen, "text": text[offset:end], "offset": offset,
                "next_offset": end if end < len(text) else None, "total_chars": len(text),
                **({"undrawn_frames_skipped": skipped} if skipped else {}),
                "note": "the page's markup, read only. ferb-element=\"sNeM\" is that element's id in the latest snapshot "
                        "(act on it with browser.act); ferb-frame is a frame's id. Values of password, code and card "
                        "fields are never shown; scripts' and styles' contents are left out."}

    async def dialog(self, tab: TabRef, decision: str | None = None, text: str | None = None) -> dict[str, Any]:
        t = self._tab(tab)
        return dict(await t.bridge.call("dialog", {"tab_id":t.chrome_id, "decision":decision,
                                                   **({"text":text} if text is not None else {})}, timeout=10))

    async def act(self, el: ElementRef, action: BrowserAction, intent_id: str) -> ActionResult:
        t, frame, snap, local, info = self._resolve(el)
        if action.kind == "drag":                        # a drop onto a web terminal (D31 round 2, finding 4)
            end = t.elements.get(action.end_element or "")
            if end is not None and end[3].terminal and (refusal := self._terminal_refusal(t, el.element_id, action)):
                raise Refused(refusal)
        if el.element_id in t.points or action.kind == "drag":
            return await self._pointer(t, el, action, intent_id)
        t.visual = None
        if info.secret and (action.kind == "type" or (action.kind == "press" and enters_text(action.text))):
            raise Refused(f"that's a secret field ({info.field_kind or 'password or code'}); FERB never types into it "
                          "or presses a key that puts text there: fill it through browser.secret or a saved card")
        into = action.kind in ("type", "press") and info.terminal                # D31: a web terminal
        if into and (refusal := self._terminal_refusal(t, el.element_id, action)):
            raise Refused(refusal)
        if action.kind in WRITES:
            self._note_input(t)                         # HAND-447: the page may now hold unsaved input
        if action.kind == "upload":
            return await self._upload(t, frame, snap, local, action.files, intent_id, action.files_closed,
                                      file_input=info.input_type == "file")
        args: dict[str, Any] = {"tab_id": t.chrome_id, "frame_id": frame, "snap": snap, "el": local,
                                "kind": action.kind, "text": action.text,
                                # real (debugger-dispatched) input is the default; page events on request
                                "page_mode": bool(action.page_mode and not action.trusted and action.kind in TRUSTED_KINDS)}
        if action.expect_dialog:
            args["expect_dialog"] = action.expect_dialog
            if action.dialog_text is not None:
                args["dialog_text"] = action.dialog_text
        if action.kind not in STILL:
            t.moved = True                              # it may scroll the page (L33: the snapshot's rects go stale)
        try:
            v = await t.bridge.call("act", args, timeout=30, cid=intent_id or None)
        except DriverError as e:
            if str(e).startswith("detached:"):
                raise OutcomeUnknown(self._detached_msg(str(e)[9:])) from None
            raise
        if v.get("detail") == "stale":
            raise StaleRef(f"element {el.element_id} is gone; take a new snapshot")
        if str(v.get("detail") or "").startswith("outside_page:"):   # HAND-482: a copy parked off the page
            twin = next((eid for eid, (*_, e) in t.elements.items() if eid != el.element_id and e.in_view
                         and e.role == info.role and e.label == info.label), None)
            if twin:
                v = {**v, "detail": f"{v['detail']}. The same {info.role} \"{info.label}\" is on screen as {twin}",
                     "on_screen": twin}
        if into:                                       # one Return stays covered after approved text, nothing else
            g = action.terminal
            t.armed = ((el.element_id, g.confirmation_id) if g is not None and action.kind == "type" and g.input.enter
                       and v.get("ok") else None)
        facts = self.act_facts(t.ref.tab_id, el.element_id, action.kind, action.text)
        submitted = [s for s in v.get("submitted") or [] if isinstance(s, dict)] if isinstance(v.get("submitted"), list) \
            else []
        if submitted:                                  # the page's own submit event, seen during this act
            s = submitted[0]
            kinds = tuple(dict.fromkeys(k for x in submitted for k in x.get("kinds") or () if k in FIELD_KINDS))
            facts = dataclasses.replace(facts or ActionFacts(), submits_form=True,
                                        form_action_host=s.get("action_host") if isinstance(s.get("action_host"), str)
                                        else None,
                                        form_method=s.get("method") if isinstance(s.get("method"), str) else None,
                                        field_kinds=tuple(dict.fromkeys((*(facts.field_kinds if facts else ()),
                                                                         *kinds))))
        return ActionResult(bool(v.get("ok")), str(v.get("detail", "")),
                            {k:value for k,value in v.items() if k not in {"ok","detail"}}, observed=facts)

    def act_facts(self, tab_id: str, element_id: str, kind: str, text: str | None) -> ActionFacts | None:
        """What code reads, from the latest snapshot, about an act on this element (GATE-202, protocol 18):
        whether it submits a form (a click on a submit control, Enter in a field whose form submits) with the form's
        action host and method; the kinds of the secret fields it fills or the submitted form holds; an amount the
        control's own label shows, else the one the page states in structured form; the page's merchant; the target's
        role and secrecy, the page URL, and the frame's host when it isn't the page's. None: no such element."""
        t = self._tabs.get(tab_id)
        hit = t.elements.get(element_id) if t else None
        if t is None or hit is None:
            return None
        frame, info = hit[0], hit[3]
        sub = info.submit
        enter = kind == "press" and str(text or "").strip().casefold() in ("enter", "return")
        submits = sub is not None and ((kind in ("click", "dblclick") and sub.on_click) or (enter and sub.on_enter))
        kinds = tuple(dict.fromkeys(([info.field_kind] if info.field_kind and kind in ("type", "select") else [])
                                    + (list(sub.field_kinds) if submits and sub else [])))
        amount = label_amount(info.label if kind in ("click", "dblclick") else None) or             (label_amount(sub.label) if submits and sub else None)
        page = urlsplit(t.url or "").hostname or None
        here = urlsplit(t.frame_urls.get(frame, "")).hostname or None if frame else None
        commerce = t.commerce or {}
        return ActionFacts(submits_form=True if submits else None,
                           form_action_host=sub.action_host if submits and sub else None,
                           form_method=sub.method if submits and sub else None, field_kinds=kinds,  # type: ignore[arg-type]
                           page_amount=amount or (commerce.get("amount") if isinstance(commerce.get("amount"), str)
                                                  else None),
                           page_merchant=commerce.get("merchant") if isinstance(commerce.get("merchant"), str) else None,
                           page_amounts=getattr(t, "amounts", ()),
                           secure_field=info.secret or None, terminal_target=info.terminal or None,
                           target_role=info.role or None, page_url=t.url or None,
                           frame_host=here if here and here != page else None, tab_chain=self.tab_chain(tab_id))

    def element_identity(self, tab_id: str, element_id: str) -> dict[str, Any] | None:
        """P19's identity of an element of the latest snapshot (kb area gateway): the page (origin and path,
        never the query), the frame's when it is a child frame's, its role, and the node itself: the debugger's node id
        while it lives (a fresh snapshot of the same page finds the same node), else its DOM path. Never the
        snapshot-scoped id or the tab alias, so a new look after an unknown outcome doesn't make the same act new.
        None: the id isn't one of the latest snapshot's."""
        t = self._tabs.get(tab_id)
        hit = t.elements.get(element_id) if t else None
        if t is None or hit is None:
            return None
        frame, info = hit[0], hit[3]

        def where(url: str) -> str:
            u = urlsplit(url or "")
            return f"{u.scheme}://{u.netloc}{u.path}" if u.scheme else ""
        node = f"b:{info.backend_node}" if info.backend_node is not None else f"p:{info.path}" if info.path else None
        if node is None:
            return None
        return {"page": where(t.url), **({"frame": where(t.frame_urls.get(frame, ""))} if frame else {}),
                "role": info.role, "node": node}

    async def _pointer(self, t: _Tab, el: ElementRef, action: BrowserAction, intent_id: str) -> ActionResult:
        if action.page_mode or action.kind not in POINTER_KINDS:     # HAND-493: what a point takes, and what instead
            raise Refused("a screenshot point takes click, dblclick, rightclick, hover or drag, as real input: drop "
                          "page_mode; to type, select or press a key, act on an element id from browser.snapshot")
        if not self._visual_fresh(t):
            raise StaleRef("the visual target is stale; take a fresh screenshot and resolve its points")
        start = t.points.get(el.element_id)
        end = t.points.get(action.end_element or "")
        if not start or (action.kind == "drag" and not end):
            raise Refused("drag needs two points resolved from the same screenshot")
        t.visual = None
        t.moved = True                                  # a gesture may scroll the page (L33)
        result = await t.bridge.call("pointer", {"tab_id": t.chrome_id, "point": start, "end": end,
                                                "kind": action.kind}, timeout=10, cid=intent_id or None)
        return ActionResult(bool(result.get("ok")), str(result.get("detail", "")),
                            {k: v for k, v in result.items() if k not in {"ok", "detail"}})

    @staticmethod
    def _visual_fresh(t: _Tab) -> bool:
        """The tab's latest screenshot still resolves points: of this snapshot and page, under 30 seconds old, unused."""
        v = t.visual
        return v is not None and v[1] == t.snap_gen and v[2] == t.bridge.navs.get(t.chrome_id, 0) and \
            asyncio.get_running_loop().time() - v[3] <= 30

    @staticmethod
    def _detached_msg(reason: str) -> str:
        if reason == "canceled_by_user":
            return "the owner cancelled FERB's debugger on that tab mid-action; outcome unknown"
        return f"the debugger left the tab mid-action ({reason}); outcome unknown"

    async def _upload(self, t: _Tab, frame: int, snap: int, local: int, files: tuple[str, ...],
                      intent_id: str, closed: Callable[[Path, str], str | None] | None = None, *,
                      file_input: bool = True) -> ActionResult:
        """HAND-500: the route. The browser sets files on a field through FERB's debugger only when the owner allowed
        FERB access to file URLs (the hello's `file_access`; FERB never turns it on); otherwise, and for an element
        that isn't a file field (a drop zone), an extension from UPLOAD_BYTES_EXTENSION_VERSION gets the file's bytes
        through the page (_upload_bytes). An older extension keeps the debugger's route, refusal and all."""
        relative = [f for f in files if not os.path.isabs(os.path.expanduser(f))]
        if relative:                    # BrowserAct makes them the workspace's: a relative one here is never guessed
            raise Refused(f"upload paths must be absolute ({relative[0]}); nothing was given to the browser")
        paths = [source_path(f) for f in files]
        if not paths:
            raise Refused("upload needs at least one file path")
        version = str(getattr(t.bridge, "version", "") or "")
        if not version_ok(version, UPLOAD_BYTES_EXTENSION_VERSION):
            try:
                return await self._upload_paths(t, frame, snap, local, paths, intent_id, closed)
            except DriverError as e:
                if "refused to set the file" not in str(e):
                    raise
                raise DriverError(f"{e}. FERB's browser extension here is {version or 'an older one'}, which can only "
                                  "ask the browser to set the file, and the browser refuses that without access to "
                                  f"file URLs; extension {UPLOAD_BYTES_EXTENSION_VERSION} sends the file through the "
                                  "page instead, and the browser updates it by itself from the Chrome Web Store",
                                  facts={**e.facts, "reason": "upload_refused_by_browser"}) from None
        if file_input and getattr(t.bridge, "file_access", None) is True:
            r = await self._upload_paths(t, frame, snap, local, paths, intent_id, closed)
            if r.facts.get("reason") == "file_access_off":       # the switch went off since the hello: nothing was
                t.bridge.file_access = False                     # tried, and the next upload sends the bytes (the
                r = ActionResult(False, f"{r.detail}; try the upload again: FERB now sends the file through the page",
                                 r.facts)                         # act's id is spent, so it isn't sent again here)
            return r
        return await self._upload_bytes(t, frame, snap, local, paths, intent_id, closed)

    async def _upload_paths(self, t: _Tab, frame: int, snap: int, local: int, paths: list[str], intent_id: str,
                            closed: Callable[[Path, str], str | None] | None) -> ActionResult:
        """The debugger's route: the browser reads FERB's own copy of each file (rule 17) and sets it on the field."""
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
        facts.setdefault("route", "debugger")
        if given is not paths:
            facts["staged_copies"] = given          # what the browser read: FERB's copies; the originals are unchanged
        return self._receipt(v, facts, identities)

    @staticmethod
    def _receipt(v: dict[str, Any], facts: dict[str, Any], identities: list[dict[str, Any]]) -> ActionResult:
        """What an upload's reply proves: the page's files compared with the ones read (`bytes_match`; None when the
        page couldn't hash them), and whether a source changed meanwhile."""
        selected = v.get("selected_files")
        facts["bytes_match"] = None if not isinstance(selected, list) or any(
            not isinstance(a, dict) or not a.get("sha256") for a in selected) else (
            len(selected) == len(identities) and all(a.get("sha256") == b["sha256"] and a.get("size") == b["size"]
                                                    for a, b in zip(selected, identities)))
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

    def _upload_limit(self) -> int:
        """HAND-502: the most bytes one upload hands through the page. No upload limit is decided yet (an owner
        decision, rule 18); the task's download budget (browser.download_task_max_gb) stands in."""
        if self.downloads is not None:
            return self.downloads.limits().task_bytes
        from ferb.act.browser.downloads import DEFAULTS
        return int(DEFAULTS["download_task_max_gb"]) * 1000 ** 3

    async def _upload_bytes(self, t: _Tab, frame: int, snap: int, local: int, paths: list[str], intent_id: str,
                            closed: Callable[[Path, str], str | None] | None) -> ActionResult:
        """HAND-501: FERB reads each file through the handle the protection lists checked (_identify) and sends its
        bytes in UPLOAD_PART parts to the target frame of this task's tab only (upload_part, a known failure on error:
        nothing reaches the page); then `upload` with the transfer gives them to the element. The bytes go nowhere
        else: no copy on disk, nothing logged or journaled. A file that changes while it is read sets nothing."""
        identities, handles = await asyncio.to_thread(self._identify, paths, closed)
        try:
            total, limit = sum(int(i["size"]) for i in identities), self._upload_limit()
            if total > limit:
                raise Refused(f"the files come to {total} bytes, more than the {limit} bytes one upload may carry through "
                              "the page (browser.download_task_max_gb, the per-task download budget, stands in until "
                              "the owner sets an upload limit); nothing was given to the browser")
            transfer, meta = secrets.token_hex(16), []
            for index, (fd, item) in enumerate(zip(handles, identities)):
                size, offset, digest = int(item["size"]), 0, hashlib.sha256()
                while True:
                    chunk = await asyncio.to_thread(os.pread, fd, min(UPLOAD_PART, size - offset), offset)
                    digest.update(chunk)
                    await t.bridge.call("upload_part", {"tab_id": t.chrome_id, "frame_id": frame, "transfer": transfer,
                                                        "index": index, "offset": offset,
                                                        "data": base64.b64encode(chunk).decode()}, timeout=30)
                    offset += len(chunk)
                    if offset >= size or not chunk:
                        break
                if offset != size or digest.hexdigest() != item["sha256"]:
                    return ActionResult(False, "the file changed while FERB sent it to the page; nothing was set on "
                                               "the page", {"source": "local", "files": identities, "route": "page",
                                                            "reason": "source_changed_during_upload",
                                                            "transferred": False})
                name = os.path.basename(str(item["path"]))
                meta.append({"name": name, "type": mimetypes.guess_type(name)[0] or "", "size": size,
                             "last_modified": int(item["modified_ns"]) // 1_000_000})
        finally:
            for fd in handles:
                os.close(fd)
        try:
            v = await t.bridge.call("upload", {"tab_id": t.chrome_id, "frame_id": frame, "snap": snap, "el": local,
                                               "transfer": transfer, "files": meta}, timeout=30, cid=intent_id or None)
        except DriverError as e:
            if str(e).startswith("detached:"):
                raise OutcomeUnknown(self._detached_msg(str(e)[9:])) from None
            raise
        if v.get("detail") == "stale":
            raise StaleRef("the element is gone; take a new snapshot")
        facts = {k: value for k, value in v.items() if k not in {"ok", "detail"}}
        facts.update(source="local", files=identities, transferred=bool(v.get("ok")))
        route = facts.get("route")
        detail = str(v.get("detail") or "")
        if v.get("ok") and not detail:
            detail = (f"set {facts.get('files_set', len(meta))} file(s) on the field through the page"
                      if route == "field" else f"dropped {len(meta)} file(s) on the element and the page took the drop")
            detail += "; whether the site accepted the upload is what the page shows next"
        return self._receipt({**v, "detail": detail}, facts, identities)

    @staticmethod
    def _identify(paths: list[str], closed: Callable[[Path, str], str | None] | None = None
                  ) -> tuple[list[dict[str, Any]], list[int]]:
        """Each file's identity, read through one handle that never follows a link at its last step, and those open
        handles (the caller closes them). Rule 10: `closed` checks each handle as macOS opened it (F_GETPATH) before a
        byte is read, so a folder swapped for a link after the gateway's check can't hand over FERB's own files or a
        never-read store. Blocking: runs in a worker thread."""
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
            return identities, handles
        except BaseException:
            for fd in handles:
                os.close(fd)
            raise

    @staticmethod
    def _sources(paths: list[str], staging: tuple[Uploads, str, str] | None,
                 closed: Callable[[Path, str], str | None] | None = None) -> tuple[list[dict[str, Any]], list[str]]:
        """Each file's identity (_identify) and the paths the browser is given. Rule 17: with `staging` (uploads, task
        tree, tab), FERB's own copy made from that same handle, so the browser reads the copy, never the original in a
        folder macOS protects (a permission dialog in front of the owner), and the copy is checked against the
        identity just read. Blocking: runs in a worker thread."""
        identities, handles = _TabDriver._identify(paths, closed)
        try:
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
        """Search all accessible frames; page full hit text without selecting a candidate. HAND-484: the latest
        snapshot's elements whose label, value (never a secret field's) or role hold the words come first, by id, so a
        button's words give the button to act on; then the page's text lines."""
        t = self._tab(tab)
        if offset == 0:
            value = await t.bridge.call("find", {"tab_id": t.chrome_id, "text": text}, timeout=30)
            hits = value.get("hits", [{"frame_id": 0, "text": x} for x in value.get("examples", [])])
            current = t.snap_nav == getattr(t.bridge, "navs", {}).get(t.chrome_id, 0)   # ids before a navigation: stale
            found = self._element_hits(t, text) if current else []
            if current:                                # a text line that is exactly a control's label: that control
                labels: dict[str, str] = {}
                for eid, (*_, e) in t.elements.items():
                    labels.setdefault(" ".join(e.label.split()).casefold(), eid)
                hits = [h | ({"element": labels[k]} if (k := " ".join(str(h.get("text", "")).split()).casefold())
                             in labels else {}) for h in hits]
            t.searches[text] = (json.dumps(found + hits, ensure_ascii=False), int(value.get("count", 0)),
                                int(value.get("lines", 0)), len(found), current)
        if text not in t.searches:
            raise Refused("start this search at offset=0 before fetching its saved result pages")
        payload, count, lines, elements, current = t.searches[text]
        end = min(offset + max_chars, len(payload))
        return {"matches": count, "element_matches": elements, "lines_on_page": lines,
                "text": payload[offset:end], "offset": offset, "next_offset": end if end < len(payload) else None,
                "total_chars": len(payload),
                "examples": [h["text"] for h in json.loads(payload) if "text" in h] if len(payload) <= max_chars else [],
                "note": "Saved all-frame search inventory: the latest snapshot's matching elements first (act on them "
                        "by `element`), then the page's text lines (with `element` when a line is a control's whole "
                        "label). A line with no element id isn't a control of the latest snapshot: if it is one, it "
                        "appeared after that snapshot, so take a new one. Frames the page doesn't draw are skipped. "
                        "offset=0 refreshes it; later pages keep the same evidence." +
                        ("" if current or not t.elements else
                         " The page navigated since the last snapshot: take one for element ids.")}

    @staticmethod
    def _element_hits(t: _Tab, text: str) -> list[dict[str, Any]]:
        """The latest snapshot's elements whose label, value, link or role hold `text` (case-insensitive), in its
        order."""
        needle = text.casefold()
        out: list[dict[str, Any]] = []
        for eid, (_, _, _, e) in t.elements.items():
            value = None if e.secret else e.value
            if not any(needle in s.casefold() for s in (e.label, value or "", e.href or "", e.role)):
                continue
            hit: dict[str, Any] = {"element": eid, "role": e.role, "label": e.label}
            if value:
                hit["value"] = value
            if e.href and needle in e.href.casefold():
                hit["href"] = e.href
            if not e.in_view:
                hit["in_view"] = False
                if e.offscreen:
                    hit["offscreen"] = e.offscreen
            if e.state:
                hit["state"] = list(e.state)
            out.append(hit)
        return out

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
        t.moved = True                                  # L33: the snapshot's rects go stale
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

    async def media(self, tab: TabRef, action: str, index: int | None, to: float | None,
                    media_id: str | None = None) -> dict[str, Any]:
        """L26: play/pause/mute/unmute/seek one media element of the latest snapshot, by its index there or its own id
        (`media_id`, L31); returns its state afterwards (and the page's error when it refused, e.g. play() without a
        real click)."""
        t = self._tab(tab)
        if not t.snapshot_json:
            raise Refused("take a snapshot first: browser.media uses an index from its `media` list")
        if media_id is not None:
            if media_id not in t.media_ids:
                raise Refused(f"the latest snapshot has no media element {media_id}: take a new snapshot")
            index = t.media_ids[media_id]
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
        own = next((qid for qid, i in t.media_ids.items() if i == index), None)
        sound: dict[str, Any] = {}
        if action in ("mute", "unmute") and not t.attached and not v.get("error"):
            # HAND-207: in a tab FERB opened, the task's own unmute lets the tab be heard (it is muted
            # otherwise), and its mute silences it again; the owner's tabs keep their own sound setting
            sound = await self.tab_sound(tab, action == "unmute")
        return ({k: v[k] for k in (*MEDIA_FIELDS, "error") if k in v} | {"index": index}
                | ({"id": own} if own else {}) | ({"frame": frame} if frame else {}) | sound)

    async def tab_sound(self, tab: TabRef, on: bool) -> dict[str, Any]:
        """HAND-207: let a tab FERB opened be heard (`on`: a task that listens to it, records it with sound, or
        unmutes its media) or mute it again, through the browser's tab mute (`mute_tab`, protocol 19). The owner's own
        tabs are never touched. Returns `tab_muted` as the browser reports it, or why it couldn't change ({} for the
        owner's tab). A failure never fails the caller: the fact is said instead."""
        t = self._tab(tab)
        if t.attached:
            return {}
        try:                                             # always asked: a tab re-adopted after a restart keeps its mute
            v = await t.bridge.call("mute_tab", {"tab_id": t.chrome_id, "muted": not on}, timeout=10)
        except DriverError as e:
            return {"tab_muted": t.muted, "tab_sound_error": str(e)}
        if v.get("changed") is True:
            t.muted = v.get("muted") is True
        return {"tab_muted": t.muted} | ({"tab_sound_error": str(v.get("reason"))} if v.get("changed") is False else {})

    async def show_tab(self, tab: TabRef) -> bool:
        """The owner clicked FERB's human-check notification: this tab comes to the front in its own window. False when
        its profile isn't connected or the tab isn't FERB's to show any more (the caller brings the browser forward)."""
        t = self._tabs.get(tab.tab_id)
        if t is None or t.closed or t.bridge.conn is None:
            return False
        await t.bridge.call("show_tab", {"tab_id": t.chrome_id}, timeout=10)
        return True

    def tab_profile(self, tab: TabRef) -> tuple[str, str]:
        """(browser, folder) of the profile this tab is in."""
        b = self._tab(tab).bridge.binding
        return b.browser, b.folder

    async def shows(self, tab: TabRef, text: str) -> dict[str, Any]:
        """Facts for a human check's wait: whether the tab is still `open`, its `url` and `title`, and whether its
        page `shows` the check's own words (None while the page can't be searched, e.g. between documents)."""
        t = self._tab(tab)
        listed = await t.bridge.call("list_tabs", {}, timeout=10)
        row = next((r for r in listed.get("tabs", []) if int(r.get("tab_id", -1)) == t.chrome_id), None)
        if row is None:
            return {"open": False}
        try:
            found = await t.bridge.call("find", {"tab_id": t.chrome_id, "text": text}, timeout=15)
            shows: bool | None = int(found.get("count", 0)) > 0
        except DriverError:
            shows = None
        return {"open": True, "shows": shows, "url": str(row.get("url") or ""), "title": str(row.get("title") or "")}

    async def wait(self, tab: TabRef, until: WaitCondition, timeout: float) -> bool:
        t = self._tab(tab)
        try:
            v = await t.bridge.call("wait", {"tab_id": t.chrome_id, "until": until.kind, "value": until.value},
                                    timeout=timeout)
        except OutcomeUnknown:
            return False
        return bool(v.get("met"))

    async def screenshot(self, tab: TabRef, looks: bool = True) -> bytes:
        """A jpeg of the tab (works minimized / in the background, 2.7), fetched in parts when one message would
        come near the 1 MB native-messaging line, with every secret field the extension reports painted over before
        anything else sees it (`painted`, L13). Never refused for what the page shows; withheld (Withheld) only when
        the masking can't be done. `looks=False` (its read just timed out, HAND-435): captured without the page's own
        looks, which would hang the same way, and masked as a page the extension can't read (extension 0.19.6; an
        older one looks anyway)."""
        import base64
        t = self._tab(tab)
        t.masking, t.native_shot, t.shot_timing = "", False, None
        v = await t.bridge.call("screenshot", {"tab_id": t.chrome_id, "snap": t.snap_gen}
                                | ({} if looks else {"looks": False}))
        t.shot_timing = _steps(v.get("steps"))
        t.input_note = str(v.get("input_note") or "")[:300]                # protocol 17 (round 5)
        if v.get("undebuggable"):                        # protocol 16: no tab capture at all; maybe its window (I1)
            jpeg, t.masking = await self._native_window(t, v)
            t.native_shot = True
            return jpeg
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

    def owner_view_supported(self, tab: TabRef) -> bool:
        """GATE-579: whether the extension connected for this tab has `owner_view` (OWNER_VIEW_EXTENSION_VERSION), read
        as browser.html and the PDF tool read theirs; an older one keeps the payment card's masked capture."""
        from ferb.act.browser.ext_protocol import OWNER_VIEW_EXTENSION_VERSION, version_ok
        return version_ok(str(getattr(self._tab(tab).bridge, "version", "") or ""), OWNER_VIEW_EXTENSION_VERSION)

    async def owner_view(self, tab: TabRef) -> "OwnerOnlyImage":
        """HAND-497: one unmasked picture of the tab for the owner's own payment card and nothing else (the owner's
        exception: shown once, to them only, never to a model). Not a tool and called by no tool: only the payment
        card's builder calls it. It returns an OwnerOnlyImage, which no JSON, pickle, string or tool result can carry,
        and nothing here records it. CaptureUnavailable ("couldn't capture the page") when the tab can't be captured.
        Every model-visible capture (screenshot above) keeps its masking exactly (GATE-206)."""
        import base64
        t = self._tab(tab)
        try:
            v = await t.bridge.call("owner_view", {"tab_id": t.chrome_id}, timeout=15)
        except DriverError as e:
            raise CaptureUnavailable(f"couldn't capture the page ({str(e)[:200]})") from None
        if not isinstance(v.get("data"), str) or not v["data"]:
            why = str(v.get("unavailable") or "")[:200]
            raise CaptureUnavailable("couldn't capture the page" + (f" ({why})" if why else ""))
        return OwnerOnlyImage(base64.b64decode(v["data"]))

    async def pdf(self, tab: TabRef, landscape: bool = False) -> bytes:
        """T-1065, HAND-427: the tab as Chrome prints it (extension 0.19.3's `pdf`), fetched in parts like a
        screenshot. Nothing paints over a PDF, so the extension withholds a page with a password, code or card field
        anywhere in it: Refused, saying why."""
        import base64
        from ferb.act.browser.ext_protocol import PDF_EXTENSION_VERSION, version_ok
        t = self._tab(tab)
        running = str(getattr(t.bridge, "version", "") or "")
        if not version_ok(running, PDF_EXTENSION_VERSION):     # HAND-428: only a PDF needs the newer extension
            raise Refused(f"the FERB extension in this profile is {running or 'an older version'}; making a PDF needs "
                          f"{PDF_EXTENSION_VERSION} or later. Don't start a browser or app from the shell instead (it can't "
                          "run there and crashes on the owner's screen). Tell the owner: "
                          "their browser updates the extension by itself, or they can update it now on its extensions "
                          "page (Developer mode, Update); then ask again")
        v = await t.bridge.call("pdf", {"tab_id": t.chrome_id, "landscape": bool(landscape)})
        if v.get("withheld"):
            raise Refused(f"no PDF of this page: {str(v['withheld'])[:300]}")
        if "data" in v:
            data = str(v["data"])
        else:
            parts = []
            for i in range(int(v.get("parts", 0))):
                parts.append(str((await t.bridge.call("pdf_part", {"tab_id": t.chrome_id, "part": i}))["data"]))
            data = "".join(parts)
        pdf = base64.b64decode(data)
        if not pdf.startswith(b"%PDF-"):
            raise Refused("the browser didn't return a PDF for this page")
        return pdf

    def masking(self, tab: TabRef) -> str:
        """How the latest screenshot of this tab had its secret fields found, when worth saying ("" otherwise)."""
        return self._tab(tab).masking

    def timing(self, tab: TabRef) -> dict[str, Any] | None:
        """The extension's step times (ms) for this tab's latest read and screenshot (0.19.6), for the call's span."""
        t = self._tab(tab)
        out = {k: v for k, v in (("read", t.timing), ("shot", t.shot_timing)) if v}
        return out or None

    def input_note(self, tab: TabRef) -> str:
        """Why clicks on this tab may not land, when its latest capture went on although input readiness failed."""
        return self._tab(tab).input_note

    def pointable(self, tab: TabRef, shot: str) -> bool:
        """Whether browser_point can resolve elements on this screenshot (not one of a page the extension can't read,
        nor a picture of the whole browser window)."""
        return shot != self._tab(tab).unread_shot

    async def _unread(self, t: _Tab, jpeg: bytes, reply: dict[str, Any]) -> tuple[bytes, str]:
        """A capture of a page the extension couldn't read (protocol 15, B6; owner: no screenshot refused): painted
        where it can be, the rest left to the gateway's text recognition, and a note saying what was painted. Painted:
        the look before the capture's fields when only the look after failed, and, when the tab is the one its window
        shows, the secret fields Accessibility finds in that window's page area, read just after the capture, scaled
        from the area onto the picture. Withheld when the picture can't be painted, or the look before is malformed
        (as the normal path, round 3 m5)."""
        from ferb.act.evidence import mask
        why = str(reply.get("unread"))[:200]
        try:
            size = mask.size(jpeg)
        except ValueError:
            raise Withheld("pixels withheld: the capture couldn't be painted over") from None
        m = reply.get("mask")
        before_boxes: list[mask.Box] = []
        if m is not None:
            before = mask.PageState.parse(m.get("before")) if isinstance(m, dict) else None
            if before is None:
                raise Withheld("pixels withheld: the extension didn't say where the page's secret fields are")
            before_boxes = mask.to_image_boxes([i.rect for i in before.items if i.paint], before.viewport, size)
        ax_boxes, read, whole, ax_why = await self._native_boxes(t, reply.get("window"), size)
        boxes = before_boxes + ax_boxes
        if boxes:
            try:
                jpeg = mask.mask_secret_fields(jpeg, boxes)
            except (ValueError, RuntimeError):
                raise Withheld("pixels withheld: the capture couldn't be painted over") from None
        done = []                                        # the note says what was painted (round 3, m7)
        if read:
            done.append("masked from Accessibility (the secret fields it finds in the tab's window, read just after "
                        "the capture" + ("; one it found has no box to paint" if not whole else "") + ")")
        if before_boxes:
            done.append("the fields the look before the capture found are painted")
        ext = f"the browser extension couldn't read the page ({why})"
        if done:
            return jpeg, (f"{ext}: " + "; ".join(done) + ("" if read else f" ({ax_why})") +
                          "; anything else is left to text recognition (a secret neither can find could show)")
        return jpeg, (f"the page's secret fields couldn't be located ({ext}; {ax_why}); masked by text recognition "
                      "only (a secret it can't read as text could show)")

    async def _native_boxes(self, t: _Tab, window: Any,
                            size: tuple[int, int]) -> tuple[list[Any], bool, bool, str | None]:
        """Accessibility's secret fields for an unread tab, as image boxes; whether it read the tab's page, whether
        every field it found has a box, and why it couldn't read it (None when it did)."""
        from ferb.act.evidence import mask
        hidden = _hidden(window)
        if hidden or not isinstance(window, dict):
            return [], False, False, f"{hidden}, so Accessibility can't see it"
        if window.get("emulated"):                       # FERB's own window: its viewport isn't the window's layout
            return [], False, False, ("the tab is laid out at FERB's own size, so Accessibility's places in its window "
                                      "don't match the picture")
        pid = getattr(t.bridge.conn, "browser_pid", None) if t.bridge.conn is not None else None
        if self.native_secrets is None or not isinstance(pid, int):
            return [], False, False, "Accessibility can't be asked about this browser"
        bounds = _window_bounds(window)
        if bounds is None:
            return [], False, False, "the extension didn't say where the tab's window is"
        found = await self.native_secrets(pid, bounds, str(window.get("title") or ""))   # its window by title (m6)
        if found is None:
            return [], False, False, "Accessibility couldn't read the page in its window"
        rects, area, whole = found
        return mask.to_image_boxes(rects, area, size), True, whole, None

    async def _native_window(self, t: _Tab, reply: dict[str, Any]) -> tuple[bytes, str]:
        """A tab the debugger can't attach to (the Web Store, chrome:// and other extensions' pages: no tab capture
        is possible, protocol 16): when it is the one its window shows, that browser window captured natively like a
        mac.screenshot (AX and the extension where they can read it, its note; the gateway's text recognition), said
        with the picture (round 3, I1). Otherwise withheld with the honest reason: the only no-picture case."""
        reason = str(reply.get("withheld") or "the debugger can't attach to this page")[:300]
        window = reply.get("window")
        hidden = _hidden(window)
        if hidden or not isinstance(window, dict):
            raise Withheld(f"pixels withheld: {reason}, and {hidden}, so its window can't be captured in its place")
        pid = getattr(t.bridge.conn, "browser_pid", None) if t.bridge.conn is not None else None
        bounds = _window_bounds(window)
        missing = ("the browser's process isn't known" if not isinstance(pid, int) else
                   "the extension didn't say where the tab's window is" if bounds is None else
                   "no native window capture is connected" if self.native_capture is None else "")
        if missing or bounds is None or not isinstance(pid, int) or self.native_capture is None:   # (round 5)
            raise Withheld(f"pixels withheld: {reason}, and its window can't be captured in its place: {missing}")
        jpeg, note = await self.native_capture(pid, bounds, str(window.get("title") or ""))
        if jpeg is None:                                 # the actual reason (round 4, item 3)
            raise Withheld(f"pixels withheld: {reason}, and its window couldn't be captured in its place: {note}")
        return jpeg, (f"{reason}: this is a picture of the whole browser window showing it, masked as a mac.screenshot "
                      f"({note or 'the secret fields Accessibility and the browser extension found are painted'}) "
                      "and by text recognition")

    def browser_pid(self, tab: TabRef) -> int | None:
        """The process of the browser showing this tab (its native host's parent), so hearing the tab (L26) captures
        that exact app process even when another instance shares its bundle id."""
        conn = self._tab(tab).bridge.conn
        return conn.browser_pid if conn is not None else None

    def page_rect(self, el: ElementRef, action: BrowserAction | None = None, visible: bool = False) -> Rect | None:
        """L33, FERB's cursor: where the element `action` touches will be when it does, in the top frame's layout
        viewport (CSS px), from the latest snapshot; with `visible`, only its part the visual viewport shows (a typing
        pointer aims at the field's leading edge, which must be on the page). An act that scrolls its element to the
        viewport's centre first (CENTRING; real typing and key presses too, as `act` sends them) is pointed at where
        the scroll takes it; a screenshot point is pressed where it is, and a fill (no `action`) scrolls nothing.
        Publish nothing rather than a wrong spot, so None when: the element is stale or in a child frame (its rect is
        its frame's); `act` would refuse the act (a drag or a pointer act without a fresh screenshot's points, typing
        into a secret field); a centring act's element is fixed, sticky or in a box that scrolls too, or the page
        can't say where the scroll ends (`_Page.centred`); FERB scrolled the page since the snapshot and the element
        isn't fixed (fixed in a box that scrolls doesn't count); it is sticky or its pin is unknown; or the spot is
        off the page's visible part."""
        try:
            t, frame, _, _, info = self._resolve(el)
        except FerbError:
            return None
        page = t.page
        if frame != 0 or page is None or page.visual is None or self._refused(t, el, info, action):
            return None
        pinned, boxed = t.motion.get(el.element_id, (None, False))
        centres = action is not None and el.element_id not in t.points and (
            action.kind in CENTRING or action.kind in CENTRING_REAL and not (action.page_mode and not action.trusted))
        if centres:
            rect = None if pinned or boxed else page.centred(info.rect)
        else:
            rect = None if pinned in ("sticky", "unknown") or (t.moved and (pinned != "fixed" or boxed)) \
                else info.rect
        vx, vy, vw, vh, _ = page.visual
        if rect is None or rect[2] <= 0 or rect[3] <= 0 \
                or not (vx <= rect[0] + rect[2] / 2 <= vx + vw and vy <= rect[1] + rect[3] / 2 <= vy + vh):
            return None
        if visible:                                  # its centre is on the page, so some of it is
            left, top = max(rect[0], vx), max(rect[1], vy)
            rect = (left, top, min(rect[0] + rect[2], vx + vw) - left, min(rect[1] + rect[3], vy + vh) - top)
        return rect

    def _refused(self, t: _Tab, el: ElementRef, info: PageElement, action: BrowserAction | None) -> bool:
        """Whether `act` would refuse this act before doing anything (its own checks, in its order)."""
        if action is None:
            return False
        if el.element_id in t.points or action.kind == "drag":
            return bool(action.page_mode or action.kind not in POINTER_KINDS or not self._visual_fresh(t)
                        or el.element_id not in t.points
                        or (action.kind == "drag" and (action.end_element or "") not in t.points))
        return info.secret and (action.kind == "type" or (action.kind == "press" and enters_text(action.text)))

    async def screen_rect(self, el: ElementRef, action: BrowserAction | None = None,
                          visible: bool = False) -> Rect | None:
        """L33, FERB's cursor: `page_rect` (with `visible`, the part shown) in global screen points, placed on the page
        area of the tab's window (`native_place`). None besides when FERB's own minimized window or a tab its window
        doesn't show holds it (decided from the snapshot, with no Accessibility read), the tab is laid out at FERB's
        own size, an older extension's snapshot lacks the facts, or the window can't be matched or isn't on screen."""
        rect = self.page_rect(el, action, visible)
        t = self._tabs.get(el.tab.tab_id)
        page = t.page if t is not None else None
        window = page.window if page is not None else None
        pid = t.bridge.conn.browser_pid if t is not None and t.bridge.conn is not None else None
        if rect is None or page is None or window is None or _hidden(window) or window.get("emulated") \
                or self.native_place is None or not isinstance(pid, int):
            return None
        bounds, viewport = _window_bounds(window), page.mapping()
        if bounds is None or viewport is None:
            return None
        return await self.native_place(rect, viewport, pid, bounds, str(window.get("title") or ""))

    def recording_source(self, tab: TabRef, owner: str) -> tuple[dict[str, Any], Any]:
        t = self._tab(tab)

        async def capture() -> bytes | tuple[bytes, str]:    # with a note when the page couldn't be read (B6)
            import base64
            from ferb.act.point import preview_left, preview_phase
            self.owned_by(tab.tab_id, owner)
            if self._tab(tab) is not t:
                raise Refused('recording tab identity changed')
            # a live preview's frame gives the extension only the time the preview has left (DMN-406), so a look it
            # gave up on frees the tab then, not 30 s later; a tab whose read just timed out is captured without the
            # page's own looks (HAND-435), masked as a page the extension can't read
            left = preview_left()
            timeout = CMD_TIMEOUT_S if left is None else max(0.5, left)
            with preview_phase('extension'):
                result = await t.bridge.call('record_frame', {'tab_id': t.chrome_id}
                                             | ({'looks': False} if t.stuck else {}), timeout=timeout)
            if result.get('undebuggable'):               # as a screenshot: its window natively, or withheld with the
                return await self._native_window(t, result)   # honest reason (round 4, item 5)
            if result.get('withheld'):
                return painted(b'', result)              # raises: this frame only (the recording goes on)
            if 'data' in result:
                frame = base64.b64decode(result['data'])
            else:
                parts = []
                for i in range(int(result['parts'])):
                    left = preview_left()
                    part = await t.bridge.call('record_frame', {'tab_id': t.chrome_id, 'part': i,
                                                               'capture_id': result['capture_id']},
                                               timeout=CMD_TIMEOUT_S if left is None else max(0.5, left))
                    parts.append(part['data'])
                frame = base64.b64decode(''.join(parts))
            if result.get('unread'):                     # protocol 15 (B6): masked where it can be, said with it
                return await self._unread(t, frame, result)
            return painted(frame, result)
        return {'kind': 'tab', 'browser': tab.browser, 'profile': tab.profile, 'tab_id': t.chrome_id,
                'tab': tab.tab_id}, capture

    async def visual_snapshot(self, tab: TabRef) -> tuple[str, bytes, int, int]:
        """browser.screenshot: the snapshot generation the shot's points resolve against, then the picture. On a page
        the extension can't script, the snapshot fails first; the picture is asked for anyway (round 3, I1): when the
        page couldn't be read (its note says so) it comes back as a shot no point resolves on; otherwise the snapshot's
        failure stands."""
        import Quartz
        failed: FerbError | None = None
        before = getattr(self, "_tabs", {}).get(tab.tab_id)       # a stand-in driver may keep no tabs
        earlier = (before.snap_gen, before.snap_nav, dict(before.elements)) if before is not None else None
        try:
            await self.snapshot(tab)
            if earlier is not None:
                self._carry(self._tab(tab), *earlier)
        except FerbError as e:
            failed = e
        # secret fields painted from the extension's own rects, not this list (L13); after a read that timed out, no
        # second wait on the same page's looks (HAND-435): masked as a page the extension can't read
        stuck = isinstance(failed, DriverError) and failed.facts.get("reason") == "page_not_responding"
        jpeg = await self.screenshot(tab, looks=not stuck)
        t = self._tab(tab)
        if failed is not None and not t.masking:
            raise failed
        image = Quartz.CGImageSourceCreateWithData(jpeg, None)
        props = Quartz.CGImageSourceCopyPropertiesAtIndex(image, 0, None)
        width, height = int(props["PixelWidth"]), int(props["PixelHeight"])
        shot = "bs-" + secrets.token_hex(8)
        if failed is not None or t.native_shot:   # no element resolves on a page FERB can't read, nor on the whole
                                                  # browser window's picture (round 4, item 4)
            t.visual, t.unread_shot = None, shot
        else:
            t.visual, t.unread_shot = (shot, t.snap_gen, t.snap_nav, asyncio.get_running_loop().time(), width,
                                       height), None
        return shot, jpeg, width, height

    IN_VIEW_LIST_CHARS = 8000       # HAND-488: the in-view elements a screenshot lists when earlier ids went, at most

    @staticmethod
    def _carry(t: _Tab, gen: int, nav: int, old: dict[str, tuple[int, int, int, PageElement]]) -> None:
        """HAND-488: after a screenshot's own snapshot, each id of the snapshot before it whose node the new one found
        again (same frame and the debugger's node id, else the same DOM path, role and label), on the same page,
        keeps working for that node; the rest went. `ids_note` says which, with the new snapshot's in-view elements when
        any earlier id went."""
        if gen == t.snap_gen or not old:
            return
        def key(frame: int, e: PageElement) -> tuple[Any, ...]:
            return (frame, "b", e.backend_node) if e.backend_node is not None else (frame, "p", e.path, e.role, e.label)
        new = {key(f, e): eid for eid, (f, _, _, e) in t.elements.items()}
        gone, found = 0, set()
        if nav == t.snap_nav:
            for eid, (f, _, _, e) in old.items():
                now = new.get(key(f, e)) if (e.backend_node is not None or e.path) else None
                if now is None or eid in t.elements:
                    gone += eid not in t.elements
                    continue
                t.elements[eid] = t.elements[now]
                t.carried.add(eid)
                found.add(now)
        else:
            gone = len(old)
        if not gone:
            t.ids_note = {"ids": f"the last snapshot's element ids still work (snapshot s{t.snap_gen} found the same "
                                  "elements)"}
            return
        shown, used, more = [], 0, 0
        for eid, (*_, e) in t.elements.items():
            if eid in t.carried or eid in found or not e.in_view:   # an element an earlier id still names: known
                continue
            item = {"id": eid, "role": e.role, "label": e.label[:80]}
            size = len(json.dumps(item, ensure_ascii=False))
            if used + size > _TabDriver.IN_VIEW_LIST_CHARS:
                more += 1
                continue
            shown.append(item)
            used += size
        t.ids_note = {"ids": f"element ids are now from snapshot s{t.snap_gen}: {len(t.carried)} earlier ids still work "
                             f"for the same elements, {gone} went (the page changed)",
                      "in_view_elements": shown, **({"more_in_view": more} if more else {})}

    def ids_note(self, tab: TabRef) -> dict[str, Any]:
        """HAND-488: what the last screenshot's snapshot did to the earlier element ids ({} when there were none)."""
        t = self._tabs.get(tab.tab_id)
        return dict(t.ids_note) if t is not None else {}

    async def point(self, tab: TabRef, shot: str, x: float, y: float) -> PageElement:
        t = self._tab(tab)
        if shot == t.unread_shot:
            raise Refused("a point on that screenshot can't be resolved to an element: it shows a page the browser "
                          "extension can't read, or the whole browser window around it")
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
        if any(_motion(raw["element"])):
            t.motion[eid] = _motion(raw["element"])
        t.points[eid] = str(raw["pointer"])       # two points can share a shot; the gesture consumes it
        return info

    def claim(self, tab: TabRef, owner: str) -> None:
        self._tab(tab).owner = owner

    def owned_by(self, tab_id: str, owner: str, borrow: bool = True) -> TabRef:
        """`owner`'s tab; with `borrow`, a registered checker reaches its worker's tabs too (`inspection_parent`)."""
        t = self._tabs.get(tab_id)
        if t is None or not (self._accessible(t, owner) if borrow else t.owner == owner):
            open_ = [x.ref.tab_id for x in self._tabs.values() if not x.closed and (self._accessible(x, owner) if borrow
                                                                                   else x.owner == owner)]
            raise Refused(f"no tab {tab_id} open for this task; its open tabs: {', '.join(open_) or 'none'} "
                          "(browser.tabs lists them)")   # HAND-493: never a guess at ids
        return t.ref

    def _accessible(self, tab: _Tab, task: str) -> bool:
        return tab.owner is not None and (tab.owner == task or tab.owner == self.inspection_parent(task))

    def tab_url(self, tab_id: str) -> str | None:
        t = self._tabs.get(tab_id)
        return t.url if t else None

    def _opened_by(self, tab_id: str, opener: str) -> None:
        t = self._tabs.get(tab_id)
        if t is not None:
            t.opener = opener

    def tab_chain(self, tab_id: str) -> str:
        """The first task tab of the chain this tab was opened from (itself when it opened on its own): a payment's
        checkout and the bank page it opens share it (GATE-204)."""
        seen, at = {tab_id}, tab_id
        while (t := self._tabs.get(at)) is not None and (opener := getattr(t, "opener", None)) and opener not in seen:
            seen.add(opener)
            at = opener
        return at

    def element_info(self, tab_id: str, element_id: str) -> PageElement | None:
        t = self._tabs.get(tab_id)
        hit = t.elements.get(element_id) if t else None
        return hit[3] if hit else None

    @staticmethod
    def _pastes(text: str | None) -> bool:
        """A paste key the extension turns into the page's paste command (⌘V, as Meta+V or cmd+v)."""
        parts = [p.strip().casefold() for p in (text or "").replace("-", "+").split("+")]
        return len(parts) > 1 and parts[-1] == "v" and bool({"cmd", "meta", "command"} & set(parts[:-1]))

    def terminal_input(self, tab_id: str, element_id: str, kind: str, text: str | None,
                       end_element: str | None = None) -> TerminalInput | None:
        """D31: what this act would put into a web terminal (the snapshot marked the element inside xterm.js's own
        nodes), for the gateway's question, else None: typed text, a key, a paste key (the clipboard's text, O2) or
        a drag dropped onto it (round 2, finding 4). A Return on the element FERB just typed approved text into is
        covered by that Yes (`covered_by`)."""
        t = self._tabs.get(tab_id)
        if t is None:
            return None
        host = urlsplit(t.url or "").hostname or ""
        app, bid = (f"the page on {host}" if host else "a web page"), f"web:{host}"
        why = "a terminal in the page (xterm.js)"
        if kind == "drag":
            end = self.element_info(tab_id, end_element or "")
            return TerminalInput(app, bid, why, drag=True) if end is not None and end.terminal else None
        info = self.element_info(tab_id, element_id)
        if info is None or not info.terminal or kind not in ("type", "press") or text is None:
            return None
        if kind == "type":
            return TerminalInput(app, bid, why, text=text, enter=not terminals.newline(text))
        if self._pastes(text):
            count, pasted, concealed = self.clipboard()
            return TerminalInput(app, bid, why, text=pasted, paste=True, concealed=concealed, clipboard=count,
                                 enter=pasted is not None and not terminals.newline(pasted))
        if text.strip().casefold() in terminals.RETURN_KEYS and t.armed is not None and t.armed[0] == element_id:
            return TerminalInput(app, bid, why, keys="return", covered_by=t.armed[1])
        return TerminalInput(app, bid, why, keys=text)

    def _terminal_refusal(self, t: _Tab, element_id: str, action: BrowserAction) -> str | None:
        """Right before input is sent into a web terminal: None when the owner's Yes names exactly this input (a Return
        only on the element the approved text went into, a paste only while the clipboard is the one shown, a drop
        only with a Yes to it), else the refusal; nothing is sent then."""
        g = action.terminal
        if g is not None and action.kind == "drag" and g.input.drag:
            return None
        if g is not None and action.kind == "type" and g.input.text == action.text and not g.input.keys:
            return None
        if action.kind == "press" and self._pastes(action.text):
            if g is not None and g.input.paste and self.clipboard()[0] == g.input.clipboard:
                return None
        elif g is not None and action.kind == "press" and action.text is not None:
            if g.input.covered_by is not None:
                if (action.text.strip().casefold() in terminals.RETURN_KEYS and g.input.covered_by == g.confirmation_id
                        and t.armed == (element_id, g.confirmation_id)):
                    return None
            elif g.input.keys == action.text and g.input.text is None:
                return None
        host = urlsplit(t.url or "").hostname or ""
        return terminals.UNASKED.format(why="a terminal in the page (xterm.js)", app=host or "a web page")

    def element_ref(self, tab: TabRef, element_id: str) -> ElementRef:
        head = element_id[1:].split("e")[0] if element_id.startswith("s") else ""
        return ElementRef(tab, "", int(head) if head.isdigit() else -1, element_id)

    def _tab(self, ref: TabRef) -> _Tab:
        t = self._tabs.get(ref.tab_id)
        if t is None:
            raise StaleRef(f"tab {ref.tab_id} is gone")
        if t.closed:                                   # closed, or taken / lost (HAND-201): never driven again
            raise StaleRef(f"tab {ref.tab_id} is no longer FERB's: {t.left or 'it was closed in the browser'}")
        if t.bridge.gen != ref.generation or t.bridge.conn is None:
            raise StaleRef(f"tab {ref.tab_id} belongs to an older browser connection")
        return t

    def _resolve(self, el: ElementRef) -> tuple[_Tab, int, int, int, PageElement]:
        t = self._tab(el.tab)
        hit = t.elements.get(el.element_id)
        if hit is None or (el.snapshot_gen != t.snap_gen and el.element_id not in t.carried):   # HAND-488
            raise StaleRef(f"element {el.element_id} is from an older snapshot; take a new one")
        if t.bridge.navs.get(t.chrome_id, 0) != t.snap_nav:
            raise StaleRef("the page changed since the snapshot; take a new one")
        frame, snap, local, info = hit
        return t, frame, snap, local, info

    def urls_of(self, owner: str) -> list[str]:
        """Last known URLs of the task-owned tabs, exposed as verification facts."""
        return [t.url for t in self._tabs.values() if t.owner == owner]

    def task_tabs(self, owner: str) -> list[dict[str, Any]]:
        """The task's tabs by tab id (owner live use 2026-10-18: a navigated tab counted as missing), each with the
        page it held when the task took it over, when it was the owner's, and then its tab_ref (what browser.tabs and
        a loop's probe name it by, L31)."""
        return [{"tab": t.ref.tab_id, **({"closed": t.left or "the owner closed this tab"} if t.closed else {}),
                 **({"tab_ref": f"{t.ref.browser}:{t.ref.profile}:{t.chrome_id}",
                     "taken_over_from": dict(t.taken_over_from)} if t.taken_over_from else {})}
                for t in self._tabs.values() if t.owner == owner]

    async def release(self, owner: str) -> list[dict[str, Any]]:
        """Task end (HAND-201): a borrowed owner tab is let go of where the work finished; a tab FERB opened
        that holds the result (`keep_open`), or every one when the owner asked to keep the task's tabs
        (`keep_tabs_open`), leaves the group and stays open; the others close with the group. A paused task keeps its
        group: nothing closes. Each retried once; a tab that still can't be let go of becomes no task's: its downloads
        go back to the browser, and the extension's next hello lists it, so the daemon closes or releases it then."""
        if self.task_paused(owner):
            return []
        keep_all = self.task_root(owner) in self._keep_roots
        # bounded: tasks that ended are no longer anyone's to race with; trees with no tab left start their count over
        self._released = {o for o in self._released if self.task_running(o)} | {owner}
        results, errors, left = [], [], []
        for alias in [a for a, t in self._tabs.items() if t.owner == owner]:
            if keep_all and not self._tabs[alias].attached and not self._tabs[alias].closed:
                self._tabs[alias].keep_open = True
            for attempt in (1, 2):
                if alias not in self._tabs:
                    break
                try:
                    results.append(await self.close(self._tabs[alias].ref))
                    break
                except DriverError as e:
                    # HAND-448: a page that asked before closing keeps its dialog (tabs.remove doesn't bring the tab
                    # forward, measured) and isn't asked again: code never answers it, and the owner is told
                    if attempt == 2 or e.facts.get("reason") == "dialog_pending":
                        self._tabs[alias].owner = None
                        errors.append(f"{alias}: {e}")
                        left.append(self._leftover(self._tabs[alias], str(e)))
                        break
        live_roots = {self.task_root(t.owner) for t in self._tabs.values() if t.owner}
        self._keep_roots = {r for r in self._keep_roots if r in live_roots or self.task_running(r)}
        if errors:                      # the raw errors stay in the trace; `tabs` names each one for the owner (HAND-419)
            raise DriverError("browser cleanup incomplete: " + "; ".join(errors), facts={"tabs": left})
        return results

    def _leftover(self, t: _Tab, error: str) -> dict[str, Any]:
        """A tab the task's cleanup couldn't close or let go of, as the owner knows it: the browser and profile by
        name, the page title it last showed (redacted), whether it was the owner's own tab, and the raw error."""
        try:
            known = self.known()
        except Exception:                                # a name is a courtesy: the folder still says which profile
            known = []
        p = named_profile(known, t.ref.browser, t.ref.profile)
        browser = BROWSERS[t.ref.browser].label if t.ref.browser in BROWSERS else t.ref.browser.capitalize()
        host = (urlsplit(t.url).hostname or "") if t.url.startswith(("http://", "https://")) else ""
        return {"tab": t.ref.tab_id, "browser": browser,
                "profile": p.display_name, "account": p.email,        # its signed-in account, read now (HAND-424)
                "title": t.title or self.redact_text(host), "borrowed": t.attached, "kept_open": t.keep_open,
                "error": error}

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
        for p in self.known():
            br = self.hub.bridge(p.browser, p.folder)
            if br is not None and br.binding.identity is Identity.FERB:
                continue
            connected = bool(br is not None and br.conn is not None)
            row = {"profile":f"{p.browser}/{p.folder}", "name":p.display_name, "account":p.email,
                   "paired":br is not None, "connected":connected}
            if br is not None and not connected and (problem := self.hub.problem(br.binding)) is not None:
                row["refused"] = problem                    # HAND-406: why it can't connect, as FERB knows it
            out.append(row)
        return out

    def profiles_note(self) -> str | None:
        """Why discovery found no profiles, when FERB knows (HAND-407: Full Disk Access is off); None otherwise."""
        return self.resolver.unseen() if not self.profiles() else None

    def __init__(self, hub: Hub, resolver: Resolver, profiles: Callable[[], list[BrowserProfile]],
                 wake: WakeServer | None = None, launch: Callable[[list[str]], None] | None = None) -> None:
        super().__init__(hub, profiles, wake, launch)
        self.resolver = resolver
        self.ask_code: Callable[[ToolContext, str], Awaitable[str | None]] | None = None
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

    def not_connected(self) -> list[dict[str, str]]:
        """The paired owner profiles a listing of every profile skipped: not connected, usually just not running."""
        return [{"profile": br.name, "why": self.hub.problem(br.binding) or
                 "not connected (usually the profile just isn't running): browser_open with this profile starts it in "
                 "the background"} for br in self._owner_bridges(None, connected_only=False) if br.conn is None]

    async def open_tabs(self, profile: str | None = None, task: str | None = None) -> list[dict[str, Any]]:
        """The owner's open tabs in their connected profiles: a reference, the site and whether it's the active one.
        A tab this task already took over is listed with its tab id; FERB's own tabs and other tasks' aren't."""
        known = self.known()
        out = []
        selected = self._owner_bridges(profile, connected_only=False)
        if profile and self.wake is not None:
            # A named profile that isn't running starts with no window first, as for browser.open (live 2026-10-26,
            # task 7c1eca3a: the listing never started the named profile and said "reload the FERB extension")
            for br in selected:
                await self._connected(br)
        missing = [br for br in selected if br.conn is None]
        if missing and profile:
            details = "; ".join(f"{br.name}: " + (self.hub.problem(br.binding) or
                               "paired but not connected (usually the profile just isn't running)")
                               for br in missing)
            raise Refused(details + ". No tab inventory is available for these profiles; this is a connection "
                          "problem, not an empty tab list. Preserve the requested profile. browser.open with this "
                          "profile loads it without a window and opens the tab itself: in one of its open windows "
                          "or, when it has none, in FERB's own minimized window, made once; nothing to ask the "
                          "owner.")
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
                            # with the account it is signed in to (HAND-424)
                            "profile": profile_shown(named_profile(known, br.binding.browser, br.binding.folder),
                                                     br.binding.browser.capitalize()),
                            "profile_id": f"{br.binding.browser}/{br.binding.folder}",
                            "url":str(t.get("url") or ""), "title":str(t.get("title") or ""),
                            "site": str(t.get("host") or ""), "active": bool(t.get("active")),
                            "window_id": t.get("window_id")}
                # whether it plays sound, and is muted (extension 0.15.8, L31); absent from an older one, never guessed
                entry |= {k: bool(t[k]) for k in ("audible", "muted") if k in t}
                if mine is not None:
                    entry["taken_over_as"] = mine.ref.tab_id      # already this task's: use this tab id
                    if mine.taken_over_from:                      # what it held then: navigating never loses it
                        entry["taken_over_from"] = dict(mine.taken_over_from)
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
        if br is None or br.binding.identity is not Identity.OWNER:
            raise Refused("that profile isn't one of the owner's paired profiles")
        br = await self._connected(br)                     # a profile whose connection dropped reconnects first
        existing = next((t for t in self._tabs.values() if t.bridge is br and t.chrome_id == cid and not t.closed), None)
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
        self._tabs[ref.tab_id] = _Tab(ref, br, cid, owner, str(tab.get("url") or ""), attached=True,
                                      title=self.redact_text(str(tab.get("title") or "")),
                                      # stored redacted, so every later reader is safe (re-review 2, I4)
                                      taken_over_from={"url": self.redact_text(str(tab.get("url") or "")),
                                                       "title": self.redact_text(str(tab.get("title") or ""))})
        try:                         # recorded (never grouped) so a reconnect re-adopts it; best effort
            await br.call("adopt_tab", {"tab_id": cid, "task": owner, "root": self.task_root(owner), "borrowed": True},
                          timeout=10)
        except DriverError:
            pass
        return ref

    async def owner_code(self, el: ElementRef, ctx: ToolContext) -> ActionResult:
        t, frame, snap, local, info = self._resolve(el)
        self.owned_by(el.tab.tab_id, ctx.task_id)
        _no_secret_into_terminal(info)
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
            key = self.hub.pairings.key(t.bridge.binding.install_id)
            if key is None:
                raise Refused("the browser is no longer paired")
            cid = ctx.intent_id or "f-" + secrets.token_hex(8)
            sealed = self.seal_value(seal_key(key), value, cid, origin)
            self._note_input(t)
            result = await t.bridge.call("fill", {"tab_id": t.chrome_id, "frame_id": frame, "snap": snap,
                                                 "el": local, "sealed": sealed, "origin": origin, "code_only": True}, timeout=15, cid=cid)
            return ActionResult(bool(result.get("ok")), str(result.get("detail", "")))
        finally:
            del value

    async def owner_secret(self, el: ElementRef, ctx: ToolContext, *, handle: str | None = None,
                           remember: str | None = None, split_elements: list[str] | None = None) -> ActionResult:
        t, frame, snap, local, info = self._resolve(el)
        self.owned_by(el.tab.tab_id, ctx.task_id)
        _no_secret_into_terminal(info)
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
                _no_secret_into_terminal(field)  # every split field, not just the first (round 3, O3)
                locals_.append(other_local)
        ask = getattr(self, "ask_secret", None)
        if ask is None:
            raise Refused("hidden secret input is unavailable")
        v = await t.bridge.call("frame_origin", {"tab_id": t.chrome_id, "frame_id": frame}, timeout=10)
        origin = normalize_origin(str(v.get("origin") or ""))
        if not origin.startswith("https://"):
            raise Refused("secret input requires an HTTPS destination")
        value = await ask(ctx, origin, handle=handle, remember=remember, field=getattr(info, "label", None) or None,
                          kind=getattr(info, "field_kind", None) or None)
        if not value:
            return ActionResult(False, "secret input declined")
        try:
            for ref in refs:
                _no_secret_into_terminal(self._resolve(ref)[4])
            key = self.hub.pairings.key(t.bridge.binding.install_id)
            if key is None:
                raise Refused("the browser is no longer paired")
            cid = ctx.intent_id or "f-" + secrets.token_hex(8)
            self._note_input(t)
            result = await t.bridge.call("fill", {"tab_id": t.chrome_id, "frame_id": frame, "snap": snap,
                "el": local, "sealed": self.seal_value(seal_key(key), value, cid, origin), "origin": origin,
                **({"split_els": locals_} if locals_ is not None else {})}, timeout=15, cid=cid)
            return ActionResult(bool(result.get("ok")), str(result.get("detail", "")))
        finally:
            del value

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
                 mailbox_address: Callable[[], str],
                 pairing_key: Callable[[str], bytes | None], profiles: Callable[[], list[BrowserProfile]],
                 wake: WakeServer | None = None, launch: Callable[[list[str]], None] | None = None,
                 feed_url: str = FEED_URL) -> None:
        super().__init__(hub, profiles, wake, launch)
        self.profile = profile
        self.secrets: Secrets = secrets                       # names and specs only (tool descriptions, checks)
        self._store = secrets                                 # the Keychain: read only inside _sealed fills
        self.mailbox_address, self.pairing_key, self.feed_url = mailbox_address, pairing_key, feed_url
        self.used_otps: set[str] = set()
        self._otp_inventory: dict[tuple[str | None, str, str], GmailFeedMailbox] = {}

    async def _bridge(self) -> Bridge:
        prof = self.profile()
        br = self.hub.bridge(*prof) if prof else None
        if br is None or br.binding.identity is not Identity.FERB:
            raise Refused("FERB's identity profile isn't paired yet")
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
        t, frame, snap, local, info = self._resolve(el)
        _no_secret_into_terminal(info)
        origin = await self._checked_origin(t, frame, ref)
        try:
            value = await self._store._reveal(ref)
        except SecretUnavailable as e:                         # HAND-410: why, when the helper said
            raise Refused(f"secret {ref.key} couldn't be read from the Keychain: {e.reason}" if e.reason
                          else f"secret {ref.key} isn't in the Keychain") from None
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
        t, frame, snap, local, info = self._resolve(el)
        _no_secret_into_terminal(info)
        origin = await self._checked_origin(t, frame, ref)
        mailbox = self._otp_inventory.get((t.owner, ref.key, origin))
        if mailbox is None:
            raise Refused("inspect OTP candidates for this account and destination first")
        try:
            found = mailbox.take(candidate)
        except ValueError as e:
            raise Refused(str(e)) from None
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
        self._note_input(t)
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
