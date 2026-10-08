"""A fake FERB extension speaking the real protocol (extensions/chromium, ARCHITECTURE.md 9.4/9.6) through the
real `ferb-native-host` process to a real daemon: install id, pairing, HMAC + seq, generations, seen / owned ids,
tab events. Pages are in-memory. `hold` makes commands hang (in flight) so tests can drop the connection mid-command.

It models the background-by-construction extension (ARCHITECTURE.md §6): one minimized FERB window per
profile with a blank placeholder tab, task tabs created inactive in it, a debugger attached on a tab's first
action and kept until the tab closes (`cancel_debugger` is the owner pressing Cancel on the infobar), dialogs
answered by the extension, keepalive pings, chunked screenshots, uploads.
"""

import base64
import hashlib
import hmac
import json
import os
import struct
import subprocess
import sys
import threading
import time
import uuid
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID, PROTOCOL_VERSION

ORIGIN = f"chrome-extension://{FERB_EXTENSION_ID}/"
EXTENSION_VERSION = json.loads((Path(__file__).parents[1] / "extensions" / "chromium" / "manifest.json").read_text())["version"]
SHOT_PART = 600000
DOWNLOAD_PART = 72000                                     # raw bytes per download_part, as protocol.js DOWNLOAD_PART


def page(url: str) -> dict[str, Any]:
    u = urlsplit(url)
    if u.path == "/login":
        return {"title": "Login", "text": "Sign in",
                "elements": [{"role": "textbox", "label": "User", "form": "login"},
                             {"role": "textbox", "label": "Password", "form": "login", "secret": True,
                              "input_type": "password"},
                             {"role": "button", "label": "Sign in", "form": "login"}]}
    if u.path == "/framed":                               # the password field lives in a cross-origin iframe
        return {"title": "Framed", "text": "",
                "elements": [{"role": "textbox", "label": "Password", "secret": True, "input_type": "password",
                              "frame": 1, "origin": "https://evil.example"}]}
    if u.hostname == "mail.google.com":
        return {"title": "Inbox - me@gmail.com - Gmail", "text": "Inbox",
                "elements": [{"role": "button", "label": "Send", "form": "other"},
                             {"role": "textbox", "label": "To", "form": "other"}]}
    if u.hostname == "www.youtube.com":
        return {"title": "YouTube", "text": "video", "elements": [{"role": "button", "label": "Subscribe", "form": None}]}
    if u.path == "/form":
        return {"title": "Form", "text": "a form",
                "elements": [{"role": "textbox", "label": "Name", "form": "other"},
                             {"role": "button", "label": "Submit", "form": "other"},
                             {"role": "link", "label": "Next", "href": f"{u.scheme}://{u.netloc}/next"}]}
    if u.path == "/dialog":                               # a button whose click opens confirm("Sure?")
        return {"title": "Dialog", "text": "a dialog page",
                "elements": [{"role": "button", "label": "Confirm", "form": None, "dialog": "confirm"},
                             {"role": "button", "label": "Ok", "form": None}]}
    if u.path == "/chat":                                 # a long list inside an overflow container
        return {"title": "Chat", "text": "messages",
                "elements": [{"role": "listitem", "label": "message 1", "form": None, "container": "c1"},
                             {"role": "listitem", "label": "message 2", "form": None, "container": "c1",
                              "in_view": False, "offscreen": "below"},
                             {"role": "textbox", "label": "Type a message", "form": None}]}
    if u.path == "/upload":
        return {"title": "Upload", "text": "pick a file",
                "elements": [{"role": "button", "label": "Choose file", "form": "other", "input_type": "file"}]}
    return {"title": "Page", "text": f"page {u.path}", "elements": [{"role": "link", "label": "Home",
                                                                    "href": f"{u.scheme}://{u.netloc}/"}]}


class FakeExtension:
    def __init__(self, home: Path, browser: str = "chrome", origin: str = ORIGIN,
                 protocol: int = PROTOCOL_VERSION, version: str = EXTENSION_VERSION) -> None:
        self.home, self.browser, self.origin, self.protocol, self.version = home, browser, origin, protocol, version
        self.install_id = str(uuid.uuid4())
        self.key: bytes | None = None
        self.seq = 0
        self.gen: int | None = None
        self.refused: str | None = None
        self.seen: dict[str, str] = {}
        self.owned: list[dict[str, Any]] = []
        self.kept: set[int] = set()
        self.tabs: dict[int, str] = {}
        self.accounts: dict[str, str | None] = {}        # host -> signed-in account shown on the page
        self.acts: list[dict[str, Any]] = []
        self.hold = False                                 # commands hang: they're "in flight"
        self.held: list[dict[str, Any]] = []              # the commands hanging (release_held answers them)
        self._n = 100
        self.proc: subprocess.Popen[bytes] | None = None
        self._lock = threading.Lock()
        self.wire: list[str] = []                        # every message, both directions, exactly as sent
        self.values: dict[tuple[int, int], str] = {}      # (tab, element) -> value set by a fill
        self.feed_xml = ""
        self.fills: list[dict[str, Any]] = []
        self.windows: dict[tuple[int, int, int, int], bool | None] = {}   # window bounds -> active tab has a password
        # background-by-construction model
        self.ferb_window: int | None = None               # FERB's minimized window (one per browser session)
        self.windows_created = 0
        self.window_state = "minimized"
        self.tab_windows: dict[int, int] = {}             # tab id -> window id
        self.tab_active: dict[int, bool] = {}
        self.attached: set[int] = set()                   # tabs with the debugger attached
        self.attaches = 0
        self.detached_reason: dict[int, str] = {}
        self.dialogs: list[dict[str, Any]] = []           # every dialog the extension answered
        self.pending_dialogs: dict[int, list[dict[str, Any]]] = {}
        self.pings = 0
        self.uploads: list[dict[str, Any]] = []
        self.screenshot_bytes = 10                        # size of the fake jpeg
        self.scroll_pos: dict[tuple[int, str | None], int] = {}
        self.opens = 0
        # downloads (D35): the extension holds a captured download until the daemon pulls it with download_part
        self.held_downloads: dict[str, bytes] = {}         # transfer id -> the bytes the browser's response carried
        self.download_parts: list[dict[str, Any]] = []    # every download_part the daemon sent, as sent
        self.download_aborts: list[str] = []               # transfers the daemon cancelled (failed in the browser)
        self.download_releases: list[str] = []             # transfers handed back to the browser untouched
        self.download_part_delay = 0.0                     # seconds per part (a slow transfer)
        self.armed: set[int] = set()                       # tabs the extension holds downloads for (hello reports them)
        self.disarmed: list[int] = []                      # disarm commands received
        self.fail_release = False                          # release_tab fails (e.g. the transport broke)
        self.nav_refuse: set[str] = set()                  # navigating here: the tab couldn't be armed (D35)
        self.download_fail_at: int | None = None          # a part at/after this offset fails (the stream broke)
        self.download_urls: dict[str, tuple[str, bytes]] = {}   # navigating here becomes a download (name, bytes)
        self._window()  # Pre-provisioned fixture window; cold creation is tested explicitly.

    # ----- transport (the real native host) -----

    def connect(self, hello: bool = True) -> None:
        env = {**os.environ, "FERB_HOME": str(self.home)}
        self.proc = subprocess.Popen([sys.executable, "-m", "ferb.native_host.main", self.origin],
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env)
        threading.Thread(target=self._read, args=(self.proc,), daemon=True).start()
        if hello:
            self.hello()

    def kill(self) -> None:
        """The MV3 worker died / the browser dropped the port: the host goes away mid-whatever."""
        if self.proc:
            self.proc.kill()
            self.proc.wait(5)
            self.proc = None
        self.gen = None
        self.attached.clear()                             # a worker restart loses its debugger sessions
        self.ferb_window = None                           # ...and storage.session (the window id)

    def _read(self, proc: "subprocess.Popen[bytes]") -> None:
        assert proc.stdout
        while True:
            head = proc.stdout.read(4)
            if len(head) < 4:
                return
            (n,) = struct.unpack("<I", head)
            raw = proc.stdout.read(n)
            self.wire.append(raw.decode())
            self._on(json.loads(raw))

    def post(self, msg: dict[str, Any]) -> None:
        assert self.proc and self.proc.stdin
        data = json.dumps(msg).encode()
        self.wire.append(data.decode())
        with self._lock:
            self.proc.stdin.write(struct.pack("<I", len(data)) + data)
            self.proc.stdin.flush()

    def send(self, mtype: str, body: dict[str, Any], key: bytes | None = None, seq: int | None = None) -> None:
        msg: dict[str, Any] = {"install_id": self.install_id, "type": mtype, "body_json": json.dumps(body)}
        k = key or self.key
        if k:
            if seq is None:
                self.seq += 1
                seq = self.seq
            msg["seq"] = seq
            text = f"{self.install_id}|{seq}|{mtype}|{msg['body_json']}".encode()
            msg["mac"] = base64.b64encode(hmac.new(k, text, hashlib.sha256).digest()).decode()
        self.post(msg)

    def hello(self) -> None:
        self.send("hello", {"version": self.version, "protocol": self.protocol, "browser": self.browser,
                            "paired": bool(self.key), "owned": self.owned,
                            "seen": [{"id": k, "status": v} for k, v in self.seen.items()], "armed": sorted(self.armed)})

    def pair(self, token: str) -> None:
        self.send("pair", {"token": token})

    def wait_for(self, cond: Any, timeout: float = 10) -> None:
        end = time.monotonic() + timeout
        while not cond():
            if time.monotonic() > end:
                raise TimeoutError("condition never became true")
            time.sleep(0.02)

    # ----- the extension's side -----

    def _on(self, m: dict[str, Any]) -> None:
        if m.get("type") == "welcome":
            self.gen = m["gen"]
        elif m.get("type") == "paired":
            self.key, self.seq = base64.b64decode(m["key"]), 0
            self.hello()
        elif m.get("type") == "refused":
            self.refused = m.get("reason")
        elif m.get("type") == "cmd":
            threading.Thread(target=self._cmd, args=(m,), daemon=True).start()

    def _cmd(self, m: dict[str, Any]) -> None:
        if m["gen"] != self.gen:
            self.send("result", {"id": m["id"], "gen": m["gen"], "status": "stale"})
            return
        if m["id"] in self.seen:
            self.send("result", {"id": m["id"], "gen": self.gen, "status": "duplicate"})
            return
        self.seen[m["id"]] = "started"                           # recorded BEFORE executing
        if self.hold and m["op"] != "ping":                       # (keepalive pings are answered regardless)
            self.held.append(m)
            return                                                # in flight until the connection drops / release
        self._run(m)

    def _run(self, m: dict[str, Any]) -> None:
        try:
            value = getattr(self, "op_" + m["op"])(m["args"], m["id"])
            body = {"id": m["id"], "gen": self.gen, "status": "ok", "value": value}
        except Exception as e:
            body = {"id": m["id"], "gen": self.gen, "status": "error", "error": str(e)}
        self.seen[m["id"]] = "done"
        self.send("result", body)

    def release_held(self) -> None:
        """Answer the commands that were hanging (the worker got round to them)."""
        self.hold = False
        held, self.held = self.held, []
        for m in held:
            self._run(m)

    def cancel_debugger(self, tab_id: int) -> None:
        """The owner pressed Cancel on the "started debugging" bar: the session ends; a held act fails mid-flight."""
        self.attached.discard(tab_id)
        self.detached_reason[tab_id] = "canceled_by_user"
        self.send("event", {"event": "detached", "tab_id": tab_id, "reason": "canceled_by_user"})
        held, self.held = self.held, []
        for m in held:
            if m["args"].get("tab_id") == tab_id and m["op"] in ("act", "scroll", "screenshot", "upload"):
                self.seen[m["id"]] = "done"
                self.send("result", {"id": m["id"], "gen": self.gen, "status": "error", "error": "detached:canceled_by_user"})
            else:
                self.held.append(m)

    def tab_event(self, event: str, tab_id: int, url: str | None = None, title: str | None = None,
                  incognito: bool | None = None) -> None:
        self.send("tab_event", {"event": event, "tab_id": tab_id, "url": url, "active": self.tab_active.get(tab_id, False),
                                "title": title, "incognito": incognito})

    def _attach(self, tab_id: int) -> None:
        if tab_id not in self.tabs:
            raise RuntimeError(f"No tab with id: {tab_id}.")
        if tab_id not in self.attached:
            self.attached.add(tab_id)
            self.attaches += 1
            self.detached_reason.pop(tab_id, None)

    def _window(self) -> int:
        if self.ferb_window is None:
            self._n += 1
            self.ferb_window = self._n
            self.windows_created += 1
            self.window_state = "minimized"
            self._n += 1                                      # the blank placeholder tab
            self.tabs[self._n] = "about:blank"
            self.tab_windows[self._n] = self.ferb_window
            self.tab_active[self._n] = True
        return self.ferb_window

    def op_open(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        if a.get("window_id") is not None:
            w = a["window_id"]
            if w not in self.tab_windows.values():
                raise ValueError("No window with that id")
        elif self.ferb_window is not None or a.get("allow_foreground"):
            w = self._window()
        else:
            return {"opened": False, "reason": "window_selection_required", "needs_foreground": False,
                    "new_window_needs_foreground": True, "detail": "Choose an exact existing window",
                    "windows": [{"id": w, "state": "normal", "focused": False, "type": "normal"}
                                for w in sorted(set(self.tab_windows.values()))]}
        self.opens += 1
        self._n += 1
        self.tabs[self._n] = a["url"]
        self.tab_windows[self._n] = w
        self.tab_active[self._n] = False
        self.owned.append({"id": cid, "tab_id": self._n, "url": a["url"]})
        self.tab_event("created", self._n, a["url"])
        return {"tab_id": self._n, "url": a["url"], "title": page(a["url"])["title"], "window_id": w}

    def op_navigate(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        if a["url"] in self.nav_refuse:
            return {"navigated": False, "reason": "download_capture_unavailable", "tab_id": a["tab_id"],
                    "url": self.tabs[a["tab_id"]], "detail": "FERB couldn't prepare this tab to hold downloads"}
        if a["url"] in self.download_urls:                # the response is a download: the extension holds it, the
            name, data = self.download_urls[a["url"]]     # navigation never commits, and it ends once FERB took it
            did = self.start_download(a["tab_id"], name, data, a["url"])
            end = time.monotonic() + 10
            while did in self.held_downloads and time.monotonic() < end:
                time.sleep(0.01)
            old = self.tabs[a["tab_id"]]
            return {"tab_id": a["tab_id"], "url": old, "title": page(old)["title"]}
        self.tabs[a["tab_id"]] = a["url"]
        self.tab_event("updated", a["tab_id"], a["url"])
        return {"tab_id": a["tab_id"], "url": a["url"], "title": page(a["url"])["title"]}

    def start_download(self, tab_id: int, name: str, data: bytes, source_url: str, size: int | None | str = "len",
                       mime: str = "application/octet-stream", source: str = "attachment") -> str:
        """The extension caught a download in this tab (Fetch, or a blob/data link) and announces it; it keeps the
        bytes until the daemon pulls them part by part (download_part) or cancels it."""
        did = "d-" + uuid.uuid4().hex[:16]
        self.held_downloads[did] = data
        self.send("event", {"event": "download", "tab_id": tab_id, "download_id": did, "name": name, "mime": mime,
                            "size": len(data) if size == "len" else size, "source_url": source_url, "source": source})
        return did

    def uncaptured_download(self, tab_id: int, name: str, source_url: str, detail: str = "started") -> None:
        """The browser started a download in this tab that the extension couldn't intercept (Page.downloadWillBegin)."""
        self.send("event", {"event": "download", "tab_id": tab_id, "uncaptured": True, "name": name,
                            "source_url": source_url, "detail": detail})

    def link_to_new_tab(self, tab_id: int, url: str) -> None:
        """content.js caught a link in this armed tab that opens a new tab; FERB's daemon opens it itself."""
        self.send("event", {"event": "open", "tab_id": tab_id, "url": url, "window_id": self.tab_windows.get(tab_id)})

    def popup(self, opener: int, url: str) -> int:
        """The page in this tab opened a new tab (window.open); the extension armed it and reports it."""
        self._n += 1
        self.tabs[self._n] = url
        self.tab_windows[self._n] = self.tab_windows.get(opener) or 0
        self.armed.add(self._n)
        self.send("event", {"event": "popup", "tab_id": self._n, "opener_tab_id": opener, "url": url})
        return self._n

    def op_disarm(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        self.disarmed.append(a["tab_id"])
        self.armed.discard(a["tab_id"])
        self.attached.discard(a["tab_id"])
        return {"disarmed": True, "tab_id": a["tab_id"]}

    def op_download_part(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        self.download_parts.append(dict(a))
        did = a["download_id"]
        if did not in self.held_downloads:
            raise RuntimeError("download transfer unavailable")
        if a.get("release"):
            self.held_downloads.pop(did)
            self.download_releases.append(did)
            return {"released": True, "handed_back": True}
        if a.get("abort"):
            self.held_downloads.pop(did)
            self.download_aborts.append(did)
            return {"aborted": True}
        if self.download_part_delay:
            time.sleep(self.download_part_delay)
        data, offset = self.held_downloads[did], int(a["offset"])
        if self.download_fail_at is not None and offset >= self.download_fail_at:
            self.held_downloads.pop(did)
            raise RuntimeError("the response stream broke")
        chunk = data[offset:offset + DOWNLOAD_PART]
        eof = offset + len(chunk) >= len(data)
        if eof:
            self.held_downloads.pop(did)
        return {"data": base64.b64encode(chunk).decode(), "offset": offset, "eof": eof}

    def op_close(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        if a['tab_id'] in self.kept:
            return self.op_release_tab(dict(a, keep_open=True), cid) | {'closed': False}
        self.tabs.pop(a["tab_id"], None)
        self.attached.discard(a["tab_id"])
        self.owned = [o for o in self.owned if o['tab_id'] != a['tab_id']]
        return {"closed": True}

    def op_release_tab(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        tab = a['tab_id']
        if self.fail_release:
            raise RuntimeError('transport failed')
        if tab not in self.tabs:
            raise ValueError('tab no longer open')
        if a.get('keep_open'):
            self.kept.add(tab)
        self.attached.discard(tab)
        self.owned = [o for o in self.owned if o['tab_id'] != tab]
        return {'tab_id': tab, 'released': True, 'kept_open': tab in self.kept, 'url': self.tabs[tab]}

    def op_list_tabs(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        return {"tabs": [{"tab_id": t, "host": urlsplit(u).hostname, "active": self.tab_active.get(t, False),
                          "window_id": self.tab_windows.get(t), "kept_open": t in self.kept,
                          "ferb": t not in self.kept and self.tab_windows.get(t) == self.ferb_window}
                         for t, u in self.tabs.items()]}

    def op_snapshot(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        self._attach(a["tab_id"])
        url = self.tabs[a["tab_id"]]
        p = page(url)
        frames: dict[int, list[dict[str, Any]]] = {}
        containers: set[str] = set()
        for i, e in enumerate(p["elements"]):
            filled = (a["tab_id"], i) in self.values
            el = {"id": i, "role": e["role"], "label": e["label"],
                  "value": ("filled" if filled else "") if e.get("secret") else "", "rect": [0, 0, 10, 10],
                  "in_view": e.get("in_view", True), "input_type": e.get("input_type"), "href": e.get("href"),
                  "form": e.get("form"), "secret": bool(e.get("secret"))}
            if e.get("offscreen"):
                el["offscreen"] = e["offscreen"]
            if e.get("container"):
                el["container"] = e["container"]
                containers.add(e["container"])
            frames.setdefault(e.get("frame", 0), []).append(el)
        scrollers = [{"id": c, "label": "list", "rect": [0, 0, 300, 400],
                      "y": self.scroll_pos.get((a["tab_id"], c), 0), "height": 2000, "viewport_h": 400,
                      "at_end": self.scroll_pos.get((a["tab_id"], c), 0) >= 1600} for c in sorted(containers)]
        out = [{"frame_id": f, "gen": a["snap"], "url": url, "title": p["title"], "elements": els,
                "text": p["text"], "truncated": False,
                "scroll": {"y": self.scroll_pos.get((a["tab_id"], None), 0), "height": 3000, "viewport_h": 900,
                           "at_end": self.scroll_pos.get((a["tab_id"], None), 0) >= 2100},
                "scrollers": scrollers if f == 0 else []} for f, els in sorted(frames.items())] or \
            [{"frame_id": 0, "gen": a["snap"], "url": url, "title": p["title"], "elements": [], "text": p["text"],
              "truncated": False, "scroll": {"y": 0, "height": 900, "viewport_h": 900, "at_end": True}, "scrollers": []}]
        return {"frames": out, "dialogs": self.pending_dialogs.pop(a["tab_id"], [])}

    def op_window_password(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        b = tuple(int(v) for v in a["bounds"])
        if b not in self.windows:
            return {"matched": False, "password": None}
        return {"matched": True, "password": self.windows[b]}

    def op_frame_origin(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        url = self.tabs[a["tab_id"]]
        framed = [e for e in page(url)["elements"] if e.get("frame") == a["frame_id"] and e.get("origin")]
        u = urlsplit(url)
        return {"origin": framed[0]["origin"] if framed else f"{u.scheme}://{u.netloc}"}

    def op_fill(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        """Like the real extension: open the seal for THIS command id and origin, or refuse; never echo the value."""
        from ferb.identity.seal import seal_key, unseal
        self.fills.append({"id": cid, "origin": a["origin"], "sealed": a["sealed"]})
        try:
            value = unseal(seal_key(self.key or b""), a["sealed"], cid, a["origin"])
        except Exception:
            return {"ok": False, "detail": "seal_refused"}
        if self.op_frame_origin({"tab_id": a["tab_id"], "frame_id": a["frame_id"]}, cid)["origin"] != a["origin"]:
            return {"ok": False, "detail": "origin_changed"}
        self.values[(a["tab_id"], a["el"])] = value
        return {"ok": True, "detail": "filled"}

    def op_read_source(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        url = self.tabs[a["tab_id"]]
        if url.split("?")[0] != "https://mail.google.com/mail/feed/atom":
            raise RuntimeError("read_source is only for FERB's inbox feed")
        from ferb.identity.seal import seal, seal_key
        return {"sealed": seal(seal_key(self.key or b""), self.feed_xml, cid, "https://mail.google.com/mail/feed/atom")}

    def op_act(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        self.acts.append(a)
        el = page(self.tabs[a["tab_id"]])["elements"][a["el"]]
        trusted = not a.get("page_mode") and a["kind"] not in ("select", "scroll")
        if trusted:
            self._attach(a["tab_id"])                           # the debugger: on first use, kept for the tab's life
        if a["kind"] == "click" and el.get("dialog"):          # the click opens a dialog the extension must answer
            expect = a.get("expect_dialog")
            d = {"type": el["dialog"], "message": "Sure?", "default": "",
                 "handled": "accepted" if expect == "accept" else "dismissed" if expect == "dismiss" else "pending"}
            self.dialogs.append({"tab_id": a["tab_id"], **d})
            self.pending_dialogs.setdefault(a["tab_id"], []).append(d)
            self.send("event", {"event": "dialog", "tab_id": a["tab_id"], **d})
        if a["kind"] == "click" and el.get("href"):
            self.op_navigate({"tab_id": a["tab_id"], "url": el["href"]}, cid)
        return {"ok": True, "detail": a["kind"] + (" (trusted)" if trusted else " (page events)")}

    def op_dialog(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        pending = [d for d in self.dialogs if d["tab_id"] == a["tab_id"] and d["handled"] == "pending"]
        if not a.get("decision"):
            return {"pending":pending[-1] if pending else None}
        if not pending:
            return {"ok":False,"reason":"no_pending_dialog"}
        d = pending[-1]
        d["handled"] = "accepted" if a["decision"] == "accept" else "dismissed"
        return {"ok":True, **d}

    def op_scroll(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        self._attach(a["tab_id"])
        c = a.get("container")
        key = (a["tab_id"], c)
        step = 340 if c else 765
        top = 2000 - 400 if c else 3000 - 900
        y = min(max(self.scroll_pos.get(key, 0) + (-step if a.get("direction") == "up" else step), 0), top)
        self.scroll_pos[key] = y
        return {"container": c, "y": y, "height": 2000 if c else 3000, "viewport_h": 400 if c else 900,
                "at_end": y <= 0 if a.get("direction") == "up" else y >= top}

    def op_upload(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        self._attach(a["tab_id"])
        self.uploads.append(a)
        return {"ok": True, "detail": f"{len(a['paths'])} file(s) attached"}

    def op_screenshot(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        self._attach(a["tab_id"])
        data = base64.b64encode(os.urandom(self.screenshot_bytes)).decode()
        if len(data) <= SHOT_PART:
            return {"data": data, "format": "jpeg"}
        self._shot = data
        return {"parts": (len(data) + SHOT_PART - 1) // SHOT_PART, "size": len(data), "format": "jpeg"}

    def op_screenshot_part(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        i = a["part"]
        return {"data": self._shot[i * SHOT_PART:(i + 1) * SHOT_PART]}

    def op_ping(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        self.pings += 1
        return {"ok": True}

    def op_wait(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        return {"met": True}

    def op_account(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        host = urlsplit(self.tabs[a["tab_id"]]).hostname or ""
        if host not in ("mail.google.com", "www.youtube.com"):
            return {"supported": False}
        if host == "www.youtube.com" and not urlsplit(self.tabs[a["tab_id"]]).path.startswith("/account"):
            return {"supported": True, "account": "signed_in_unknown" if self.accounts.get(host) else None}
        return {"supported": True, "account": self.accounts.get(host)}

    def op_wake_close(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        return {"closed": 1}
