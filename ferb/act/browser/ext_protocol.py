"""Owns the versioned extension wire schema and message authentication; must not interpret page or owner intent.

Extension messages carry install_id, type, body_json, monotonic seq and HMAC. Commands bind id and generation.
Protocol 8 adds multipart full evidence/action transport, explicit dialogs and exact-frame local upload receipts.
Protocol 10 adds downloads (D35): a `download` event for a download caught in a tab FERB drives, its bytes pulled in
parts with `download_part`. Chromium native-message size is a per-frame transport bound; content travels in chunks
without clipping."""

import base64
import hashlib
import hmac
import json
from typing import Any, Literal

from pydantic import BaseModel, Field, ValidationError

PROTOCOL_VERSION = 10
MIN_EXTENSION_VERSION = "0.14.0"
FERB_EXTENSION_ID = "jaclnphefmmgmpofnabffahdkbjjompm"   # from the key in extensions/chromium/manifest.json
MAX_MESSAGE = 1024 * 1024                      # same cap as the daemon socket

ExtType = Literal["hello", "pair", "result", "error", "tab_event", "event"]
Op = Literal["open", "navigate", "close", "list_tabs", "snapshot", "act", "wait", "screenshot", "screenshot_part", "record_frame",
             "wake_close", "frame_origin", "fill", "read_source", "window_password", "scroll", "find",
             "upload", "result_part", "command_part", "dialog", "ping", "point", "pointer", "pointer_cancel", "release_tab",
             "download_part"]


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
    """Observed tab dialog, intercepted file chooser, debugger detachment, or a download caught in a tab FERB drives
    (held by the extension until `download_part` takes or cancels it); no implied user decision."""
    event: Literal["dialog", "detached", "file_chooser", "download"]
    tab_id: int
    type: str | None = None                    # dialog: alert | confirm | prompt | beforeunload
    message: str | None = None
    handled: str | None = None                 # dialog: accepted | dismissed
    reason: str | None = None                  # detached: canceled_by_user | target_closed | ...
    frame_id: str | None = None
    session_id: str | None = None
    backend_node_id: int | None = None
    mode: str | None = None
    native_picker_shown: bool | None = None
    transferred: bool | None = None
    direct_upload_available: bool | None = None
    detail: str | None = None
    download_id: str | None = Field(default=None, max_length=64)   # download: the extension's transfer id
    name: str | None = Field(default=None, max_length=4096)        # download: the name the response suggested
    mime: str | None = Field(default=None, max_length=256)
    size: int | None = Field(default=None, ge=0)                   # download: Content-Length / blob size, if known
    source_url: str | None = Field(default=None, max_length=8192)  # download: masked by the extension, redacted here
    source: str | None = None                  # download: attachment | type | link | page
    uncaptured: bool | None = None             # download: the browser started it itself; nothing is held


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
