"""Transcripts of the real ext_bridge.Hub and Bridge (ferb/act/browser/ext_bridge.py), for make.py.

The extension is the Python tests' scripted fake (tests/fake_extension.py, imported), with its native-host transport
replaced by direct calls: what the daemon writes goes straight to the fake's `_on`, and what the fake posts is fed to
the daemon's reader as one line. Commands run synchronously (no threads), so the order of every frame is fixed.

Each scenario is a list of script actions (connect, an extension frame, a pairing token, a bridge call, closing the
connection...). After each action the harness lets the event loop settle and records, in order:

    {"d": text}       a line the daemon wrote          {"e": text}    a line the extension sent
    {"pub": [t, d]}   a published event                {"ref": facts} a refusal recorded (HAND-406)
    {"hook": [...]}   a hook the hub called            {"rec": [...]} a ledger reconciliation
    {"ret": [name, outcome]}                           a bridge call's result or error

The Rust test replays the extension's lines after the daemon lines before them and must write the same daemon lines,
publish the same events and return the same results.
"""

import asyncio
import hashlib
import json
import uuid
from pathlib import Path
from tempfile import TemporaryDirectory
from typing import Any

from ferb.act.browser import ext_bridge
from ferb.act.browser.ext_bridge import Hub
from ferb.act.browser.pairing import Pairings
from ferb.clock.clock import FakeClock
from ferb.core.types import Identity
from ferb.store.db import Database
from tests.fake_extension import FakeExtension

_ext_n = [0]


def ext_uuid() -> uuid.UUID:
    """The extension side's own counter (never the daemon's): install ids and hello nonces."""
    _ext_n[0] += 1
    return uuid.UUID(bytes=hashlib.sha256(f"ferb-extension-{_ext_n[0]}".encode()).digest()[:16], version=4)


class Ledger:
    """The ledger seam: intents in state "unknown" until reconciled; every reconcile is logged."""

    def __init__(self, log: list[Any], states: dict[str, str]) -> None:
        self.log, self.states = log, states

    def get(self, cid: str) -> Any:
        if cid not in self.states:
            raise KeyError(cid)
        from types import SimpleNamespace
        return SimpleNamespace(intent=SimpleNamespace(state=self.states[cid]))

    def reconcile(self, cid: str, observed: str, evidence: list[str]) -> None:
        self.states[cid] = observed
        self.log.append({"rec": [cid, observed, evidence]})


class Ext(FakeExtension):
    """The fake extension with its transport replaced (no native host process, no threads)."""

    def __init__(self, h: "Harness", **kw: Any) -> None:
        super().__init__(h.home, **kw)
        self.h = h
        self.replies: dict[str, list[dict[str, Any]]] = {}    # op -> scripted result bodies (status, value, error...)
        self.reader: asyncio.StreamReader | None = None

    def connect(self, hello: bool = True) -> None:
        raise AssertionError("the harness connects")

    def post(self, msg: dict[str, Any]) -> None:
        data = json.dumps(msg)
        self.wire.append(data)
        self.h.log.append({"e": data, "conn": self.h.index(self.reader)})
        if self.reader is not None:
            self.reader.feed_data(data.encode() + b"\n")

    def raw(self, text: str) -> None:
        """A line that isn't the extension's format (malformed)."""
        self.h.log.append({"e": text, "conn": self.h.index(self.reader)})
        assert self.reader is not None
        self.reader.feed_data(text.encode() + b"\n")

    def _on(self, m: dict[str, Any]) -> None:
        if m.get("type") == "cmd":
            c = self.verified(m)
            if c is not None:
                self._cmd(c)
            return
        super()._on(m)

    def _run(self, m: dict[str, Any]) -> None:
        script = self.replies.get(m["op"])
        if script:
            body = {"id": m["id"], "gen": self.gen, **script.pop(0)}
            self.seen[m["id"]] = "done"
            self.send("result", body)
            return
        super()._run(m)

    def op_result_part(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        text = self.transfers[a["id"]]
        end = min(a["offset"] + 7, len(text))
        return {"text": text[a["offset"]:end], "next_offset": end if end < len(text) else None}

    def op_command_part(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        self.parts.setdefault(a["id"], []).append(a["text"])
        got = "".join(self.parts[a["id"]])
        if len(got) == a["total_chars"]:
            env = json.loads(got)
            body = json.loads(env["body"])           # the assembled envelope (its mac checked by the real extension)
            self.assembled.append({"seq": env["seq"], "id": body["id"], "op": body["op"]})
            self._cmd(body)
        return {"received": a["offset"] + len(a["text"]), "total_chars": a["total_chars"]}

    def op_media(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        return {"ok": True, "played": len(str(a.get("action")))}


class Writer:
    """The daemon's side of the connection."""

    def __init__(self, h: "Harness", conn: "Conn") -> None:
        self.h, self.conn = h, conn

    def write(self, data: bytes) -> None:
        for line in data.decode().split("\n"):
            if line:
                self.h.log.append({"d": line, "conn": self.h.conns.index(self.conn)})
                if self.conn.ext is not None and not self.conn.closed:
                    self.conn.ext._on(json.loads(line))

    async def drain(self) -> None:
        return None

    def get_extra_info(self, name: str) -> Any:
        return None


class Conn:
    def __init__(self, h: "Harness", ext: Ext | None, app: str | None) -> None:
        self.reader = asyncio.StreamReader(limit=ext_bridge.MAX_MESSAGE if hasattr(ext_bridge, "MAX_MESSAGE")
                                           else 1024 * 1024)
        self.writer = Writer(h, self)
        self.ext, self.app, self.closed = ext, app, False
        self.task: asyncio.Task[None] | None = None


class Harness:
    def __init__(self, home: Path) -> None:
        self.home = home
        self.log: list[Any] = []
        self.states: dict[str, str] = {}
        self.db = Database(home / "ferb.db")
        self.db.migrate()
        self.clock = FakeClock()
        self.pairings = Pairings(self.db, self.clock)
        self.hub = Hub(self.pairings, Ledger(self.log, self.states),        # type: ignore[arg-type]
                       lambda topic, data: self.log.append({"pub": [topic, data]}), lambda: "dev-1")
        self.hub.record_refusal = lambda facts: self.log.append({"ref": facts})
        self.hub.on_unpaired = lambda: self.log.append({"hook": ["unpaired"]})
        self.hub.on_tab = lambda br, e: self.log.append({"hook": ["tab", br.name, e.model_dump()]})
        self.hub.on_sound = lambda br, e: self.log.append({"hook": ["sound", br.name, e.model_dump()]})
        self.hub.on_tab_closed = lambda br, tab_id: self.log.append({"hook": ["tab_closed", br.name, tab_id]})
        self.paused = False
        self.hub.paused = lambda: self.paused
        self.conns: list[Conn] = []
        self.calls: dict[str, asyncio.Task[Any]] = {}
        self.app_of: dict[int, str | None] = {}

    def index(self, reader: Any) -> int | None:
        """Which connection a frame went on (None: its connection is closed, nothing reads it)."""
        return next((i for i, c in enumerate(self.conns) if c.reader is reader and not c.closed), None)

    async def settle(self) -> None:
        for _ in range(60):
            await asyncio.sleep(0)

    async def connect(self, ext: Ext | None, app: str | None) -> Conn:
        conn = Conn(self, ext, app)
        if ext is not None:
            ext.reader = conn.reader
        self.conns.append(conn)
        pid = 1000 + len(self.conns)
        self.app_of[pid] = app
        ext_bridge._browser_pid = lambda writer, pid=pid: pid     # type: ignore[assignment]
        conn.task = asyncio.get_running_loop().create_task(self.hub.serve(conn.reader, conn.writer))  # type: ignore
        await self.settle()
        return conn

    def close(self, conn: Conn) -> None:
        conn.closed = True
        if conn.ext is not None and conn.ext.reader is conn.reader:
            conn.ext.reader = None
        conn.reader.feed_eof()

    def call(self, name: str, profile: str, op: str, args: dict[str, Any], timeout: float = 30.0,
             cid: str | None = None) -> None:
        br = self.hub.bridges[tuple(profile.split("/"))]               # type: ignore[index]
        self.log.append({"start": {"name": name, "profile": profile, "op": op, "args": small(args), "timeout": timeout,
                                   "cid": cid}})

        async def run() -> None:
            try:
                value = await br.call(op, args, timeout=timeout, cid=cid)
                self.log.append({"ret": [name, {"ok": value}]})
            except Exception as e:                                     # noqa: BLE001
                self.log.append({"ret": [name, {"error": type(e).__name__, "message": str(e),
                                                "facts": getattr(e, "facts", None),
                                                "timing": getattr(e, "timing", None)}]})
        self.calls[name] = asyncio.get_running_loop().create_task(run())


def small(v: Any) -> Any:
    """A long run of one character as {"$repeat": char, "n": count} (the Rust test expands it back)."""
    if isinstance(v, str) and len(v) > 1000 and len(set(v)) == 1:
        return {"$repeat": v[0], "n": len(v)}
    if isinstance(v, dict):
        return {k: small(x) for k, x in v.items()}
    if isinstance(v, list):
        return [small(x) for x in v]
    return v


def new_ext(h: Harness, **kw: Any) -> Ext:
    ext = Ext(h, **kw)
    ext.transfers, ext.parts, ext.assembled = {}, {}, []        # type: ignore[attr-defined]
    return ext


async def scenario(name: str, script: Any) -> dict[str, Any]:
    with TemporaryDirectory() as tmp:
        h = Harness(Path(tmp))
        ext_bridge._app_bundle = lambda pid: h.app_of.get(pid)      # type: ignore[assignment]
        actions: list[dict[str, Any]] = []

        async def act(action: dict[str, Any], fn: Any = None, wait: str | None = None) -> Any:
            start = len(h.log)
            got = fn() if fn is not None else None
            if asyncio.iscoroutine(got):
                got = await got
            await h.settle()
            if wait is not None:
                await h.calls[wait]
            await h.settle()
            actions.append({"action": action, "log": h.log[start:]})
            return got

        await script(h, act)
        for conn in h.conns:
            if not conn.closed:
                h.close(conn)
        await h.settle()
        for t in h.calls.values():
            t.cancel()
        h.db.conn.close()
        return {"name": name, "actions": actions}


def state(h: Harness, profile: str) -> dict[str, Any]:
    br = h.hub.bridges.get(tuple(profile.split("/")))                  # type: ignore[arg-type]
    if br is None:
        return {}
    return {"gen": br.gen, "connected": br.conn is not None, "version": br.version, "file_access": br.file_access,
            "lost": sorted(br.lost), "tabs": {str(k): list(v) for k, v in sorted(br.tabs.items())},
            "navs": {str(k): v for k, v in sorted(br.navs.items())},
            "detached": {str(k): v for k, v in sorted(br.detached.items())}, "hellos": br.hellos,
            "refused": dict(h.hub.refused), "unpaired": dict(h.hub.unpaired),
            "unpaired_copies": h.hub.unpaired_copies()}


async def pair(h: Harness, act: Any, ext: Ext, browser: str, folder: str, identity: str = "owner",
               app: str = "com.google.Chrome") -> Any:
    """Connect, say hello unpaired, get a token, pair; the fake then says hello signed and gets its welcome."""
    conn = await act({"connect": {"app": app, "ext": ext.install_id}}, lambda: h.connect(ext, app))
    await act({"ext": "hello"}, ext.hello)
    token = await act({"issue_token": [browser, folder, identity]},
                      lambda: h.pairings.issue_token(browser, folder, Identity(identity)))
    await act({"ext": "pair"}, lambda: ext.pair(token))
    return conn


# ---------- scenarios ----------

async def pairing_and_commands(h: Harness, act: Any) -> None:
    ext = new_ext(h)
    stranger = new_ext(h)
    await act({"connect": {"app": "com.google.Chrome", "ext": stranger.install_id}},
              lambda: h.connect(stranger, "com.google.Chrome"))
    await act({"ext": "hello"}, stranger.hello)
    await act({"ext": "tab_event"}, lambda: stranger.tab_event("created", 1, "https://evil.example/"))
    await act({"state": "chrome/Profile 1"}, lambda: h.log.append({"state": state(h, "chrome/Profile 1"),
                                                                    "copies": h.hub.unpaired_copies()}))
    await pair(h, act, ext, "chrome", "Profile 1")
    await act({"state": "chrome/Profile 1"}, lambda: h.log.append({"state": state(h, "chrome/Profile 1"),
                                                                    "copies": h.hub.unpaired_copies()}))
    await act({"call": "ping"}, lambda: h.call("ping", "chrome/Profile 1", "ping", {}, 5), wait="ping")
    await act({"call": "tabs"}, lambda: h.call("tabs", "chrome/Profile 1", "list_tabs", {}, 10), wait="tabs")
    await act({"call": "open"}, lambda: h.call("open", "chrome/Profile 1", "open",
                                                {"url": "about:blank", "group": {"root": "r", "task": "t",
                                                                                 "title": "FERB · x"}, "muted": True},
                                                30), wait="open")
    await act({"call": "nav"}, lambda: h.call("nav", "chrome/Profile 1", "navigate",
                                               {"tab_id": 103, "url": "https://a.example/form"}, 30), wait="nav")
    await act({"call": "custom-cid"}, lambda: h.call("custom-cid", "chrome/Profile 1", "media",
                                                      {"tab_id": 103, "action": "play"}, 10, "intent-1"),
              wait="custom-cid")
    await act({"call": "custom-cid-again"}, lambda: h.call("custom-cid-again", "chrome/Profile 1", "media",
                                                            {"tab_id": 103, "action": "play"}, 10, "intent-1"),
              wait="custom-cid-again")
    await act({"state": "chrome/Profile 1"}, lambda: h.log.append({"state": state(h, "chrome/Profile 1")}))


async def results(h: Harness, act: Any) -> None:
    ext = new_ext(h)
    await pair(h, act, ext, "chrome", "Profile 1")
    replies = [
        ("read-error", "snapshot", {"tab_id": 4}, {"status": "error", "error": "frame gone", "outcome": "failed"}),
        ("read-timeout-tab", "snapshot", {"tab_id": 4},
         {"status": "error", "error": "timeout", "outcome": "failed", "step": "ax", "steps": {"ax_ms": 30001}}),
        ("read-timeout-notab", "list_tabs", {}, {"status": "error", "error": "timeout", "outcome": "failed"}),
        ("act-timeout", "act", {"tab_id": 4}, {"status": "error", "error": "timeout", "outcome": "unknown"}),
        ("act-unknown", "act", {"tab_id": 4}, {"status": "error", "error": "page went away", "outcome": "unknown"}),
        ("act-detached", "act", {"tab_id": 4}, {"status": "error", "error": "detached:canceled_by_user"}),
        ("act-failed", "act", {"tab_id": 4}, {"status": "error", "error": "no such element", "outcome": "failed"}),
        ("act-failed-noerror", "act", {"tab_id": 4}, {"status": "error", "outcome": "failed"}),
        ("debugger-refused", "act", {"tab_id": 4},
         {"status": "error", "error": "debugger_refused: Cannot access a chrome-extension:// URL", "outcome": "failed"}),
        ("dialog-pending", "navigate", {"tab_id": 4, "url": "https://a.example/"},
         {"status": "error", "error": "dialog_pending: a confirm dialog is open", "outcome": "failed",
          "dialog": {"type": "confirm"}}),
        ("dialog-pending-read", "find", {"tab_id": 4, "text": "x"},
         {"status": "error", "error": "dialog_pending: an alert is open", "outcome": "failed"}),
        ("stale", "act", {"tab_id": 4}, {"status": "stale"}),
        ("duplicate", "act", {"tab_id": 4}, {"status": "duplicate"}),
        ("dialog-read", "dialog", {"tab_id": 4, "decision": None},
         {"status": "error", "error": "timeout", "outcome": "failed"}),
        ("dialog-decide", "dialog", {"tab_id": 4, "decision": "accept"},
         {"status": "error", "error": "timeout", "outcome": "unknown"}),
        ("ok-null", "wait", {"tab_id": 4}, {"status": "ok"}),
    ]
    for name, op, args, reply in replies:
        ext.replies.setdefault(op, []).append(reply)
        await act({"call": name}, lambda name=name, op=op, args=args: h.call(name, "chrome/Profile 1", op, args, 30),
                  wait=name)
    # a reply too big for one message comes back in parts (result_parts / result_part)
    value = {"text": "é ☃ " * 5, "n": [1, 2.5, None, True]}
    serialized = json.dumps(value)
    ext.transfers["big-1"] = serialized                                         # type: ignore[attr-defined]
    ext.replies.setdefault("snapshot", []).append({"status": "ok", "value": {"result_parts": {
        "id": "big-1", "total_chars": len(serialized)}}})
    await act({"call": "parts"}, lambda: h.call("parts", "chrome/Profile 1", "snapshot", {"tab_id": 4}, 30),
              wait="parts")
    # a command too big for one message goes in command parts (command_part), then runs as one
    await act({"call": "big-command"}, lambda: h.call("big-command", "chrome/Profile 1", "media",
                                                       {"tab_id": 4, "action": "x" * 520000}, 30), wait="big-command")
    await act({"assembled": True}, lambda: h.log.append({"assembled": ext.assembled}))   # type: ignore[attr-defined]
    await act({"state": "chrome/Profile 1"}, lambda: h.log.append({"state": state(h, "chrome/Profile 1")}))


async def refusals(h: Harness, act: Any) -> None:
    ext = new_ext(h)
    conn = await pair(h, act, ext, "chrome", "Profile 1")
    await act({"close": h.conns.index(conn)}, lambda: h.close(conn))
    # a connection whose host no browser FERB knows started
    nobody = new_ext(h)
    await act({"connect": {"app": None, "ext": nobody.install_id}}, lambda: h.connect(nobody, None))
    other = new_ext(h)
    await act({"connect": {"app": "com.example.notabrowser", "ext": other.install_id}},
              lambda: h.connect(other, "com.example.notabrowser"))
    # the paired chrome profile's hello from Brave
    c2 = await act({"connect": {"app": "com.brave.Browser", "ext": ext.install_id}},
                   lambda: h.connect(ext, "com.brave.Browser"))
    await act({"ext": "hello"}, ext.hello)
    await act({"close": h.conns.index(c2)}, lambda: h.close(c2))
    # malformed lines, and messages that aren't the format
    c3 = await act({"connect": {"app": "com.google.Chrome.helper", "ext": ext.install_id}},
                   lambda: h.connect(ext, "com.google.Chrome.helper"))
    for text in ["not json", "[]", "{}", '{"install_id": "short", "type": "hello", "body_json": "{}"}', ""]:
        await act({"raw": text}, lambda text=text: ext.raw(text))
    # no nonce, a protocol FERB doesn't speak, a version below the minimum, one newer than FERB
    hello = {"version": ext.version, "protocol": 21, "browser": "chrome", "paired": True, "owned": [], "seen": [],
             "armed": [], "tabs": []}
    await act({"ext": "hello-no-nonce"}, lambda: ext.send("hello", {**hello, "nonce": ""}))
    await act({"ext": "hello-protocol-20"}, lambda: ext.send("hello", {**hello, "protocol": 20, "nonce": "n1"}))
    await act({"ext": "hello-too-old"}, lambda: ext.send("hello", {**hello, "version": "0.18.2", "nonce": "n2"}))
    await act({"ext": "hello-newer"}, lambda: ext.send("hello", {**hello, "version": "0.30.0", "protocol": 22,
                                                                 "nonce": "n3"}))
    await act({"version": 1}, lambda: h.log.append({"version": h.pairings.version(ext.install_id),
                                                    "problem": h.hub.problem(h.pairings.binding(ext.install_id)),
                                                    "refused": dict(h.hub.refused)}))
    await act({"ext": "hello-invalid"}, lambda: ext.send("hello", {"protocol": 21}))
    # a wrong key, a replayed counter, an unknown and an expired token
    good_key = ext.key
    await act({"ext": "bad-mac"}, lambda: ext.send("hello", {**hello, "nonce": "n4"}, key=b"x" * 32))
    await act({"ext": "replay"}, lambda: ext.send("hello", {**hello, "nonce": "n5"}, seq=1))
    await act({"ext": "tab-event-bad-mac"}, lambda: ext.send("tab_event", {"event": "created", "tab_id": 9},
                                                             key=b"x" * 32))
    ext.key = good_key
    await act({"ext": "pair-unknown"}, lambda: ext.send("pair", {"token": "never-issued"}))
    tok = await act({"issue_token": ["chrome", "Profile 2", "owner"]},
                    lambda: h.pairings.issue_token("chrome", "Profile 2", Identity.OWNER))
    await act({"advance": 301}, lambda: h.clock.advance(301))
    late = new_ext(h)
    late.reader = c3.reader
    await act({"ext": "pair-expired"}, lambda: late.pair(tok))
    await act({"ext": "unpaired-hello-brave"}, lambda: late.send("hello", {"version": "0.19.9", "protocol": 21,
                                                                            "browser": "brave"}))
    await act({"ext": "unpaired-hello-other"}, lambda: late.send("hello", {"version": 19, "browser": "arc"}))
    await act({"state": "chrome/Profile 1"}, lambda: h.log.append({"state": state(h, "chrome/Profile 1")}))
    await act({"close": h.conns.index(c3)}, lambda: h.close(c3))
    await act({"state": "chrome/Profile 1"}, lambda: h.log.append({"state": state(h, "chrome/Profile 1")}))
    # finally a good hello again: the refusal of this run is cleared
    await act({"connect": {"app": "com.google.Chrome", "ext": ext.install_id}},
              lambda: h.connect(ext, "com.google.Chrome"))
    await act({"ext": "hello"}, ext.hello)
    await act({"state": "chrome/Profile 1"}, lambda: h.log.append({"state": state(h, "chrome/Profile 1")}))


async def drops_and_reconnects(h: Harness, act: Any) -> None:
    ext = new_ext(h)
    conn = await pair(h, act, ext, "chrome", "Profile 1")
    await act({"ledger": {"intent-act": "unknown", "intent-fail": "unknown", "intent-keep": "unknown"}},
              lambda: h.states.update({"intent-act": "unknown", "intent-fail": "unknown", "intent-keep": "unknown"}))
    ext.hold = True
    await act({"call": "held-act"}, lambda: h.call("held-act", "chrome/Profile 1", "act", {"tab_id": 4}, 30,
                                                   "intent-act"))
    await act({"call": "held-fail"}, lambda: h.call("held-fail", "chrome/Profile 1", "act", {"tab_id": 4}, 30,
                                                    "intent-fail"))
    await act({"call": "held-keep"}, lambda: h.call("held-keep", "chrome/Profile 1", "act", {"tab_id": 4}, 30,
                                                    "intent-keep"))
    await act({"call": "held-read"}, lambda: h.call("held-read", "chrome/Profile 1", "snapshot", {"tab_id": 4}, 30))
    await act({"ext": "results-after-drop-ignored"}, lambda: None)
    await act({"close": h.conns.index(conn)}, lambda: h.close(conn))
    await act({"state": "chrome/Profile 1"}, lambda: h.log.append({"state": state(h, "chrome/Profile 1")}))
    await act({"call": "while-down"}, lambda: h.call("while-down", "chrome/Profile 1", "ping", {}, 5),
              wait="while-down")
    # the extension comes back: it says which of the lost commands it finished
    ext.hold = False
    ext.seen = {"intent-act": "done", "intent-fail": "failed", "intent-keep": "started"}
    ext.held = []
    ext.armed = {7}
    conn2 = await act({"connect": {"app": "com.google.Chrome", "ext": ext.install_id}},
                      lambda: h.connect(ext, "com.google.Chrome"))
    await act({"ext": "hello"}, ext.hello)
    await act({"state": "chrome/Profile 1"}, lambda: h.log.append({"state": state(h, "chrome/Profile 1")}))
    # a second hello on the same connection keeps the generation
    await act({"ext": "hello-again"}, ext.hello)
    # a result for an old generation, and one for no command, change nothing
    await act({"ext": "stray-results"}, lambda: (ext.send("result", {"id": "c-none", "gen": 1, "status": "ok"}),
                                                 ext.send("result", {"id": "intent-keep", "gen": 1, "status": "ok"})))
    # a new connection replaces this one: the old one's frames are ignored from then on
    other = new_ext(h)
    other.install_id, other.key, other.seq = ext.install_id, ext.key, ext.seq
    ext.hold = True
    await act({"call": "held-across"}, lambda: h.call("held-across", "chrome/Profile 1", "act", {"tab_id": 4}, 30))
    await act({"connect": {"app": "com.google.Chrome", "ext": "second"}}, lambda: h.connect(other, "com.google.Chrome"))
    await act({"ext": "hello-second"}, other.hello)
    ext.seq = other.seq
    await act({"ext": "old-connection-event"}, lambda: ext.send("tab_event", {"event": "created", "tab_id": 77,
                                                                              "url": "https://old.example/"}))
    other.seq = ext.seq                                   # the two share the install's counter
    await act({"state": "chrome/Profile 1"}, lambda: h.log.append({"state": state(h, "chrome/Profile 1")}))
    other.hold = True
    await act({"call": "held-on-new"}, lambda: h.call("held-on-new", "chrome/Profile 1", "act", {"tab_id": 4}, 30))
    # the old connection's end fails every command in flight on the bridge, the new connection's too (Python)
    await act({"close": h.conns.index(conn2)}, lambda: h.close(conn2))
    await act({"release": "second"}, other.release_held)
    await act({"call": "on-second"}, lambda: h.call("on-second", "chrome/Profile 1", "ping", {}, 5), wait="on-second")


async def timeouts(h: Harness, act: Any) -> None:
    ext = new_ext(h)
    await pair(h, act, ext, "chrome", "Profile 1")
    ext.hold = True
    for name, op, args in [("t-act", "act", {"tab_id": 4}), ("t-read-tab", "snapshot", {"tab_id": 4}),
                           ("t-read", "list_tabs", {})]:
        await act({"call": name}, lambda name=name, op=op, args=args: h.call(name, "chrome/Profile 1", op, args, 0.05),
                  wait=name)
    await act({"state": "chrome/Profile 1"}, lambda: h.log.append({"state": state(h, "chrome/Profile 1")}))


async def events(h: Harness, act: Any) -> None:
    ext = new_ext(h)
    await pair(h, act, ext, "chrome", "Profile 1")
    send = ext.send
    for name, kind, body in [
        ("detached", "event", {"event": "detached", "tab_id": 4, "reason": "canceled_by_user"}),
        ("detached-other", "event", {"event": "detached", "tab_id": 5}),
        ("file-chooser", "event", {"event": "file_chooser", "tab_id": 73, "frame_id": "child", "session_id": "s",
                                   "backend_node_id": 71, "mode": "selectSingle", "native_picker_shown": False,
                                   "transferred": False, "direct_upload_available": True, "detail": "No source"}),
        ("dialog", "event", {"event": "dialog", "tab_id": 4, "type": "confirm", "message": "Sure?",
                             "handled": "dismissed"}),
        ("dialog-no-message", "event", {"event": "dialog", "tab_id": 4, "type": "alert"}),
        ("download", "event", {"event": "download", "tab_id": 4, "download_id": "d-1", "name": "a.pdf", "size": 3,
                               "source": "attachment"}),
        ("download-uncaptured", "event", {"event": "download", "tab_id": 4, "uncaptured": True, "name": "b.zip"}),
        ("download-dropped", "event", {"event": "download", "tab_id": 4, "uncaptured": True, "detail": "dropped",
                                       "count": 3}),
        ("taken", "event", {"event": "taken", "tab_id": 4, "reason": "moved_out_of_group"}),
        ("popup-no-task", "event", {"event": "popup", "tab_id": 9, "opener_tab_id": 4, "url": "https://p.example/"}),
        ("bad-event", "event", {"event": "exploded", "tab_id": 4}),
        ("created", "tab_event", {"event": "created", "tab_id": 4, "url": "https://A.Example:8443/x", "active": True,
                                  "title": "A"}),
        ("updated", "tab_event", {"event": "updated", "tab_id": 4, "url": "https://b.example/y"}),
        ("updated-same-doc", "tab_event", {"event": "updated", "tab_id": 4, "url": "https://b.example/z",
                                           "same_document": True}),
        ("updated-no-url", "tab_event", {"event": "updated", "tab_id": 4, "active": True}),
        ("activated-new", "tab_event", {"event": "activated", "tab_id": 6}),
        ("sound", "tab_event", {"event": "sound", "tab_id": 4, "audible": True, "muted": False, "ferb": False,
                                "changed": ["audible"], "muted_reason": "user"}),
        ("removed", "tab_event", {"event": "removed", "tab_id": 4, "closed_by_page": True}),
        ("bad-tab-event", "tab_event", {"event": "moved", "tab_id": 4}),
        ("error-message", "error", {"anything": 1}),
        ("bad-body", "tab_event", None),
    ]:
        if body is None:
            await act({"ext": name}, lambda: ext.send("tab_event", {}, ) or None)
            continue
        await act({"ext": name}, lambda kind=kind, body=body: send(kind, body))
    await act({"state": "chrome/Profile 1"}, lambda: h.log.append({"state": state(h, "chrome/Profile 1")}))
    await act({"paused": True}, lambda: setattr(h, "paused", True))
    await act({"ext": "paused-tab"}, lambda: send("tab_event", {"event": "created", "tab_id": 8,
                                                                "url": "https://c.example/"}))
    await act({"ext": "paused-removed"}, lambda: send("tab_event", {"event": "removed", "tab_id": 6}))
    await act({"forget": 1}, h.hub.forget_owner_tabs)
    await act({"state": "chrome/Profile 1"}, lambda: h.log.append({"state": state(h, "chrome/Profile 1")}))



# ---------- the tab drivers (Router: browser.*, FerbIdentity: ferb_browser.*) ----------

SECRET_KEY = "ferb/site/password"
SECRET_VALUE = "invented-Pa55word"


def shape(v: Any) -> Any:
    """A driver call's return, as the Rust test compares it."""
    import dataclasses
    from ferb.act.browser.snapshot import for_model
    from ferb.core.types import ActionResult, PageSnapshot, TabRef
    if isinstance(v, PageSnapshot):
        return {"snapshot": for_model(v, full=True), "generation": v.generation}
    if isinstance(v, TabRef):
        return {"tab_ref": dataclasses.asdict(v)}
    if isinstance(v, ActionResult):
        return {"action": {"ok": v.ok, "detail": v.detail, "facts": v.facts}}
    return v


class Secrets:
    """FERB's secrets seam: one configured secret; `reveal_error` makes the Keychain read fail."""

    def __init__(self) -> None:
        self.reveal_error: str | None = None
        self.revealed = 0

    def specs(self) -> list[Any]:
        return []

    def has(self, ref: Any) -> bool:
        return ref.key == SECRET_KEY

    def spec(self, ref: Any) -> Any:
        from ferb.core.types import SecretSpec
        return SecretSpec(ref, ["https://a.example"], "ferb@example.test")

    async def _reveal(self, ref: Any) -> str:
        from ferb.identity.secrets import SecretUnavailable
        self.revealed += 1
        if self.reveal_error is not None:
            raise SecretUnavailable(ref.key, self.reveal_error)
        return SECRET_VALUE


def drivers(h: Harness) -> tuple[Any, Any]:
    from types import SimpleNamespace
    from ferb.act.browser.ext_bridge import FerbIdentity, Router
    from ferb.core.errors import Refused
    h.running = {"task-a", "task-b"}                       # type: ignore[attr-defined]

    class Resolver:
        def choose(self, profile: str | None) -> Any:
            folders = {None: "Profile 1", "Work": "Profile 1", "FERB": "Profile 2"}
            if profile not in folders:
                raise Refused(f"no profile called {profile!r}")
            return SimpleNamespace(browser="chrome", folder=folders[profile])

        def unseen(self) -> None:
            return None
    router = Router(h.hub, Resolver(), lambda: [])        # type: ignore[arg-type]
    secrets = Secrets()
    ferb = FerbIdentity(h.hub, lambda: ("chrome", "Profile 2"), secrets, lambda: "ferb@example.test",  # type: ignore
                        h.pairings.key, lambda: [])
    for d in (router, ferb):
        d.task_running = lambda t: t in h.running         # type: ignore[attr-defined]
        d.task_title = lambda root: "Book a table for four"
        d.redact_text = lambda text: text.replace("SECRET", "[redacted]")

    async def ask_secret(ctx: Any, origin: str, **kw: Any) -> str | None:
        h.log.append({"asked": ["secret", origin, {k: v for k, v in kw.items()}]})
        return "owner-invented-pw" if "decline" not in str(kw.get("handle")) else None

    async def ask_code(ctx: Any, origin: str) -> str | None:
        h.log.append({"asked": ["code", origin]})
        return "424242"
    router.ask_secret = ask_secret                        # type: ignore[attr-defined]
    router.ask_code = ask_code
    h.secrets = secrets                                   # type: ignore[attr-defined]
    return router, ferb


def dcall(h: Harness, name: str, driver: Any, which: str, method: str, args: dict[str, Any]) -> None:
    """A driver call, logged with its arguments; a TabRef or ElementRef argument is given by tab alias, generation
    and element id."""
    from ferb.core.types import ElementRef, SecretRef, TabRef, ToolContext
    h.log.append({"start": {"name": name, "driver": which, "method": method, "args": args}})

    def ref(a: dict[str, Any]) -> TabRef:
        t = driver._tabs[a["alias"]]
        return TabRef(t.ref.device_id, t.ref.browser, t.ref.profile, a.get("generation", t.ref.generation), a["alias"])

    async def run() -> None:
        try:
            a = dict(args)
            if "tab" in a:
                a["tab"] = ref(a["tab"])
            if "el" in a:
                e = a.pop("el")
                a["el"] = ElementRef(ref(e), "", e["snap"], e["id"])
            if method == "fill_secret":
                value = await driver.fill_secret(a["el"], SecretRef(a["secret"]), a.get("intent", ""))
            elif method in ("owner_secret", "owner_code"):
                ctx = ToolContext("task-a", "w", "r", Identity.OWNER, frozenset(), "dev-1", None, "/tmp/none", (), (),
                                  intent_id=a.get("intent", ""))
                kw = {k: a[k] for k in ("handle", "split_elements") if k in a}
                value = await getattr(driver, method)(a["el"], ctx, **kw)
            elif method == "read":
                value = await driver.read(a["tab"], a["offset"], a["max_chars"])
            elif method == "owned_by":
                value = driver.owned_by(a["tab_id"], a["owner"])
            elif method == "release":
                value = await driver.release(a["owner"])
            elif method == "task_tabs":
                value = driver.task_tabs(a["owner"])
            elif method == "tab_chain":
                value = driver.tab_chain(a["tab_id"])
            elif method == "keep_tabs_open":
                value = driver.keep_tabs_open(a["owner"])
            else:
                value = await getattr(driver, method)(**a)
            h.log.append({"ret": [name, {"ok": shape(value)}]})
        except Exception as e:                                     # noqa: BLE001
            h.log.append({"ret": [name, {"error": type(e).__name__, "message": str(e),
                                         "facts": getattr(e, "facts", None), "timing": None}]})
    h.calls[name] = asyncio.get_running_loop().create_task(run())


async def owner_tabs(h: Harness, act: Any) -> None:
    router, ferb = drivers(h)
    ext = new_ext(h)
    await pair(h, act, ext, "chrome", "Profile 1")
    R = "router"

    async def c(name: str, method: str, **args: Any) -> None:
        await act({"call": name}, lambda: dcall(h, name, router, R, method, args), wait=name)
    await c("open", "open_for", url="https://a.example/login", owner="task-a")
    await c("snap", "snapshot", tab={"alias": "t1"})
    await c("read-1", "read", tab={"alias": "t1"}, offset=0, max_chars=300)
    await c("read-2", "read", tab={"alias": "t1"}, offset=300, max_chars=100000)
    await c("code-wrong-field", "owner_code", el={"alias": "t1", "snap": 1, "id": "s1e1"})
    await c("secret", "owner_secret", el={"alias": "t1", "snap": 1, "id": "s1e2"}, handle="site", intent="i-1")
    await c("secret-declined", "owner_secret", el={"alias": "t1", "snap": 1, "id": "s1e2"}, handle="decline")
    await c("secret-old-snap", "owner_secret", el={"alias": "t1", "snap": 0, "id": "s1e2"})
    await c("secret-no-such", "owner_secret", el={"alias": "t1", "snap": 1, "id": "s1e9"})
    await c("split-bad", "owner_secret", el={"alias": "t1", "snap": 1, "id": "s1e1"}, split_elements=["s1e1"])
    await c("nav", "navigate", tab={"alias": "t1"}, url="https://a.example/form")
    await c("secret-after-nav", "owner_secret", el={"alias": "t1", "snap": 1, "id": "s1e2"})
    await c("snap-2", "snapshot", tab={"alias": "t1"})
    await c("dialog", "dialog", tab={"alias": "t1"})
    await c("owned-other", "owned_by", tab_id="t1", owner="task-b")
    await c("owned", "owned_by", tab_id="t1", owner="task-a")
    await c("not-paired", "open_for", url="https://a.example/", owner="task-a", profile="FERB")
    await c("no-such-profile", "open_for", url="https://a.example/", owner="task-a", profile="Nope")
    # no FERB window in the profile: the extension lists the windows, FERB picks one
    ext.ferb_window = None
    await c("open-2", "open_for", url="https://b.example/", owner="task-a")
    await c("open-b", "open_for", url="https://b.example/x", owner="task-b")
    await c("chain", "tab_chain", tab_id="t3")
    await c("keep", "keep_open", tab={"alias": "t2"})
    await c("attach", "attach", tab_ref="chrome:Profile 1:102", owner="task-a")
    await c("attach-again", "attach", tab_ref="chrome:Profile 1:102", owner="task-b")
    await c("attach-bad", "attach", tab_ref="nonsense", owner="task-a")
    await c("tabs", "task_tabs", owner="task-a")
    # the owner closes task-b's tab; then moves task-a's first tab out of its group
    await act({"ext": "removed"}, lambda: ext.tab_event("removed", router._tabs["t3"].chrome_id))
    await act({"ext": "taken"}, lambda: ext.take_out_of_group(router._tabs["t1"].chrome_id))
    await c("tabs-after", "task_tabs", owner="task-a")
    await c("closed-tab", "snapshot", tab={"alias": "t3"})
    await c("release", "release", owner="task-a")
    await c("tabs-released", "task_tabs", owner="task-a")


async def ferb_tabs(h: Harness, act: Any) -> None:
    router, ferb = drivers(h)
    ext = new_ext(h)
    await pair(h, act, ext, "chrome", "Profile 2", identity="ferb")
    F = "ferb"

    async def c(name: str, method: str, **args: Any) -> None:
        await act({"call": name}, lambda: dcall(h, name, ferb, F, method, args), wait=name)
    await c("open", "open_for", url="https://a.example/login", owner="task-a")
    await c("snap", "snapshot", tab={"alias": "t1"})
    await c("fill", "fill_secret", el={"alias": "t1", "snap": 1, "id": "s1e2"}, secret=SECRET_KEY, intent="intent-9")
    await c("fill-random-id", "fill_secret", el={"alias": "t1", "snap": 1, "id": "s1e2"}, secret=SECRET_KEY)
    await c("fill-unknown", "fill_secret", el={"alias": "t1", "snap": 1, "id": "s1e2"}, secret="ferb/other")
    await act({"keychain": "fails"}, lambda: setattr(h.secrets, "reveal_error", "the Keychain is locked"))
    await c("fill-keychain", "fill_secret", el={"alias": "t1", "snap": 1, "id": "s1e2"}, secret=SECRET_KEY)
    await act({"keychain": "fails-silently"}, lambda: setattr(h.secrets, "reveal_error", ""))
    await c("fill-keychain-2", "fill_secret", el={"alias": "t1", "snap": 1, "id": "s1e2"}, secret=SECRET_KEY)
    await act({"keychain": "works"}, lambda: setattr(h.secrets, "reveal_error", None))
    await c("open-framed", "open_for", url="https://a.example/framed", owner="task-a")
    await c("snap-framed", "snapshot", tab={"alias": "t2"})
    await c("fill-framed", "fill_secret", el={"alias": "t2", "snap": 1, "id": "s1e1"}, secret=SECRET_KEY)
    await c("nav", "navigate", tab={"alias": "t1"}, url="https://a.example/login?again=1")
    await c("fill-stale", "fill_secret", el={"alias": "t1", "snap": 1, "id": "s1e2"}, secret=SECRET_KEY)
    await act({"revealed": True}, lambda: h.log.append({"revealed": h.secrets.revealed}))
    # the connection drops and comes back: FERB's tabs are re-adopted by browser tab id
    conn = h.conns[-1]
    await act({"close": h.conns.index(conn)}, lambda: h.close(conn))
    await act({"connect": {"app": "com.google.Chrome", "ext": ext.install_id}}, lambda: h.connect(ext, "com.google.Chrome"))
    await act({"ext": "hello"}, ext.hello)
    await c("old-ref", "snapshot", tab={"alias": "t1", "generation": 1})
    await c("snap-new", "snapshot", tab={"alias": "t1", "generation": 2})
    await c("fill-new", "fill_secret", el={"alias": "t1", "snap": 2, "id": "s2e2"}, secret=SECRET_KEY, intent="i-2")
    h.running.discard("task-a")                                   # type: ignore[attr-defined]
    await act({"running": ["task-b"]}, lambda: None)
    await c("release", "release", owner="task-a")


SCENARIOS = [("pairing_and_commands", pairing_and_commands), ("results", results), ("refusals", refusals),
             ("drops_and_reconnects", drops_and_reconnects), ("timeouts", timeouts), ("events", events),
             ("owner_tabs", owner_tabs), ("ferb_tabs", ferb_tabs)]


def make(reset_random: Any) -> dict[str, Any]:
    uuid.uuid4 = ext_uuid                                     # the extension's own randomness, apart from the daemon's
    out = []
    for name, script in SCENARIOS:
        reset_random()
        _ext_n[0] = 0
        out.append(asyncio.run(scenario(name, script)))
    from ferb.act.browser.profiles import BROWSERS
    from ferb.act.mac import browsers as mac_browsers
    apps = ["com.google.Chrome", "com.google.Chrome.helper", "com.brave.Browser", "com.example.notabrowser"]
    return {"note": __doc__, "device_id": "dev-1", "known": {a: mac_browsers.known(a) for a in apps},
            "browsers": {k: {"bundle_id": v.bundle_id, "label": v.label} for k, v in BROWSERS.items()}, "clock_start": "2026-01-01T09:00:00+00:00",
            "monotonic_start": 1000.0, "scenarios": out}
