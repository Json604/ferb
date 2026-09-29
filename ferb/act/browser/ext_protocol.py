"""Extension message protocol (section 9.4), mirrored by extensions/chromium/protocol.js.

ext -> daemon: `{install_id, type, body_json, seq?, mac?}`. Once paired, every message carries a strictly
increasing `seq` and `mac` = base64 HMAC-SHA256(key, "install_id|seq|type|body_json") (section 9.6).
daemon -> ext: `{type: "cmd", id, gen, op, args, timeout_ms}` for commands; `paired` / `refused` for control.
"""

import base64
import hashlib
import hmac
import json
from typing import Any, Literal

from pydantic import BaseModel, Field, ValidationError

PROTOCOL_VERSION = 2
FERB_EXTENSION_ID = "jaclnphefmmgmpofnabffahdkbjjompm"   # from the key in extensions/chromium/manifest.json
MAX_MESSAGE = 1024 * 1024                      # same cap as the daemon socket

ExtType = Literal["hello", "pair", "result", "error", "tab_event"]
Op = Literal["open", "navigate", "close", "list_tabs", "snapshot", "act", "wait", "screenshot", "attach_debugger",
             "detach_debugger", "cdp", "account", "wake_close", "frame_origin", "fill",
             "read_source", "window_password"]


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
    seen: list[dict[str, Any]] = []            # recent command ids with "started" / "done" (reconciliation)


class Result(BaseModel):
    id: str
    gen: int
    status: Literal["ok", "error", "stale", "duplicate"]
    value: Any = None
    error: str | None = None


class TabEvent(BaseModel):
    event: Literal["created", "updated", "activated", "removed"]
    tab_id: int
    url: str | None = None
    title: str | None = None
    active: bool | None = None
    incognito: bool | None = None


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
