"""A fake FERB extension speaking the real protocol (extensions/chromium, ARCHITECTURE.md 9.4/9.6) through the
real `ferb-native-host` process to a real daemon: install id, pairing, HMAC + seq, generations, seen / owned ids,
tab events. Pages are in-memory. `hold` makes commands hang (in flight) so tests can drop the connection mid-command.
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
    return {"title": "Page", "text": f"page {u.path}", "elements": [{"role": "link", "label": "Home",
                                                                    "href": f"{u.scheme}://{u.netloc}/"}]}


class FakeExtension:
    def __init__(self, home: Path, browser: str = "chrome", origin: str = ORIGIN,
                 protocol: int = PROTOCOL_VERSION) -> None:
        self.home, self.browser, self.origin, self.protocol = home, browser, origin, protocol
        self.install_id = str(uuid.uuid4())
        self.key: bytes | None = None
        self.seq = 0
        self.gen: int | None = None
        self.refused: str | None = None
        self.seen: dict[str, str] = {}
        self.owned: list[dict[str, Any]] = []
        self.tabs: dict[int, str] = {}
        self.accounts: dict[str, str | None] = {}        # host -> signed-in account shown on the page
        self.acts: list[dict[str, Any]] = []
        self.hold = False                                 # commands hang: they're "in flight"
        self._n = 100
        self.proc: subprocess.Popen[bytes] | None = None
        self._lock = threading.Lock()
        self.wire: list[str] = []                        # every message, both directions, exactly as sent
        self.values: dict[tuple[int, int], str] = {}      # (tab, element) -> value set by a fill
        self.feed_xml = ""
        self.fills: list[dict[str, Any]] = []

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
        self.send("hello", {"version": "0.6.0", "protocol": self.protocol, "browser": self.browser,
                            "paired": bool(self.key), "owned": self.owned,
                            "seen": [{"id": k, "status": v} for k, v in self.seen.items()]})

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
        if self.hold:
            return                                                # in flight until the connection drops
        try:
            value = getattr(self, "op_" + m["op"])(m["args"], m["id"])
            body = {"id": m["id"], "gen": self.gen, "status": "ok", "value": value}
        except Exception as e:
            body = {"id": m["id"], "gen": self.gen, "status": "error", "error": str(e)}
        self.seen[m["id"]] = "done"
        self.send("result", body)

    def tab_event(self, event: str, tab_id: int, url: str | None = None) -> None:
        self.send("tab_event", {"event": event, "tab_id": tab_id, "url": url, "active": False})

    def op_open(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        self._n += 1
        self.tabs[self._n] = a["url"]
        self.owned.append({"id": cid, "tab_id": self._n, "url": a["url"]})
        self.tab_event("created", self._n, a["url"])
        return {"tab_id": self._n, "url": a["url"], "title": page(a["url"])["title"]}

    def op_navigate(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        self.tabs[a["tab_id"]] = a["url"]
        self.tab_event("updated", a["tab_id"], a["url"])
        return {"tab_id": a["tab_id"], "url": a["url"], "title": page(a["url"])["title"]}

    def op_close(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        self.tabs.pop(a["tab_id"], None)
        return {"closed": True}

    def op_list_tabs(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        return {"tabs": [{"tab_id": t, "host": urlsplit(u).hostname, "active": False} for t, u in self.tabs.items()]}

    def op_snapshot(self, a: dict[str, Any], cid: str) -> dict[str, Any]:
        url = self.tabs[a["tab_id"]]
        p = page(url)
        frames: dict[int, list[dict[str, Any]]] = {}
        for i, e in enumerate(p["elements"]):
            filled = (a["tab_id"], i) in self.values
            frames.setdefault(e.get("frame", 0), []).append(
                {"id": i, "role": e["role"], "label": e["label"],
                 "value": ("filled" if filled else "") if e.get("secret") else "", "rect": [0, 0, 10, 10],
                 "input_type": e.get("input_type"), "href": e.get("href"), "form": e.get("form"),
                 "secret": bool(e.get("secret"))})
        return {"frames": [{"frame_id": f, "gen": a["snap"], "url": url, "title": p["title"], "elements": els,
                            "text": p["text"], "truncated": False} for f, els in sorted(frames.items())]
                or [{"frame_id": 0, "gen": a["snap"], "url": url, "title": p["title"], "elements": [],
                     "text": p["text"], "truncated": False}]}

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
        if a["kind"] == "click" and el.get("href"):
            self.op_navigate({"tab_id": a["tab_id"], "url": el["href"]}, cid)
        return {"ok": True, "detail": a["kind"]}

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
