"""Extension message protocol (section 9.4), mirrored by extensions/chromium/protocol.js.

ext -> daemon: `{install_id, type, body_json, seq?, mac?}`. Once paired, every message carries a strictly
increasing `seq` and `mac` = base64 HMAC-SHA256(key, "install_id|seq|type|body_json") (section 9.6).
daemon -> ext: `{type: "cmd", id, gen, op, args, timeout_ms}` for commands; `paired` / `refused` for control.
Protocol 5 (extension 0.9.0): exact visual pointer gestures and cancellation.
Protocol 4 introduced AX-first snapshots, point resolution and explicit command outcomes.
Protocol 3 introduced FERB's own minimized window, a debugger session per task tab, `event` messages
(dialogs, a detached debugger), container scrolls, uploads, chunked screenshots.
"""

import base64
import hashlib
import hmac
import json
from typing import Any, Literal

from pydantic import BaseModel, Field, ValidationError

PROTOCOL_VERSION = 5
MIN_EXTENSION_VERSION = "0.9.0"                # older extensions are refused at hello (reload the unpacked one)
FERB_EXTENSION_ID = "jaclnphefmmgmpofnabffahdkbjjompm"   # from the key in extensions/chromium/manifest.json
MAX_MESSAGE = 1024 * 1024                      # same cap as the daemon socket

ExtType = Literal["hello", "pair", "result", "error", "tab_event", "event"]
Op = Literal["open", "navigate", "close", "list_tabs", "snapshot", "act", "wait", "screenshot", "screenshot_part",
             "account", "wake_close", "frame_origin", "fill", "read_source", "window_password", "scroll", "find",
             "upload", "ping", "point", "pointer", "pointer_cancel"]


def version_tuple(v: str) -> tuple[int, ...]:
    out = []
    for part in v.split("."):
        digits = "".join(ch for ch in part if ch.isdigit())
        out.append(int(digits) if digits else 0)
    return tuple(out)


def version_ok(v: str, minimum: str = MIN_EXTENSION_VERSION) -> bool:
    return version_tuple(v) >= version_tuple(minimum)


class ExtMessage(BaseModel):
    install_id: str = Field(min_length=8, max_length=64)
    type: ExtType
    body_json: str = Field(max_length=MAX_MESSAGE)
    seq: int | None = None
    mac: str | None = None

    def body(self) -> dict[str, Any]:
        v = json.loads(self.body_json)
        if not isinstance(v, dict):
            raise ValueError("body must be an object")
        return v


class Hello(BaseModel):
    version: str                               # extension version
    protocol: int
    browser: str = ""                          # what the extension believes (informational; binding wins)
    paired: bool = False
    owned: list[dict[str, Any]] = []           # resources it created, each tagged with its command id
    seen: list[dict[str, Any]] = []            # recent command ids with started / done / failed / unknown (reconciliation)


class Result(BaseModel):
    id: str
    gen: int
    status: Literal["ok", "error", "stale", "duplicate"]
    value: Any = None
    error: str | None = None
    outcome: Literal["done", "failed", "unknown"] | None = None


class TabEvent(BaseModel):
    event: Literal["created", "updated", "activated", "removed"]
    tab_id: int
    url: str | None = None
    title: str | None = None
    active: bool | None = None
    incognito: bool | None = None


class Event(BaseModel):
    """Something that happened to a task tab without a command asking: a JS dialog the extension answered, or the
    debugger leaving the tab (the owner pressed Cancel on the infobar, or the tab closed)."""
    event: Literal["dialog", "detached"]
    tab_id: int
    type: str | None = None                    # dialog: alert | confirm | prompt | beforeunload
    message: str | None = None
    handled: str | None = None                 # dialog: accepted | dismissed
    reason: str | None = None                  # detached: canceled_by_user | target_closed | ...


def mac_text(install_id: str, seq: int, mtype: str, body_json: str) -> bytes:
    return f"{install_id}|{seq}|{mtype}|{body_json}".encode()


def sign(key: bytes, install_id: str, seq: int, mtype: str, body_json: str) -> str:
    return base64.b64encode(hmac.new(key, mac_text(install_id, seq, mtype, body_json), hashlib.sha256).digest()).decode()


def authentic(key: bytes, m: ExtMessage) -> bool:
    if m.seq is None or m.mac is None:
        return False
    return hmac.compare_digest(sign(key, m.install_id, m.seq, m.type, m.body_json), m.mac)


def parse(raw: dict[str, Any]) -> ExtMessage | None:
    try:
        return ExtMessage.model_validate(raw)
    except ValidationError:
        return None


def command(cid: str, gen: int, op: Op, args: dict[str, Any], timeout_ms: int) -> dict[str, Any]:
    return {"type": "cmd", "id": cid, "gen": gen, "op": op, "args": args, "timeout_ms": timeout_ms}
