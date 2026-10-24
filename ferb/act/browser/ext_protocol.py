"""Owns the versioned extension wire schema and message authentication; must not interpret page or owner intent.

Extension messages carry install_id, type, body_json, monotonic seq and HMAC. Commands bind id and generation.
Protocol 8 adds multipart full evidence/action transport, explicit dialogs and exact-frame local upload receipts.
Protocol 10 adds downloads (D35): a `download` event for a download caught in a tab FERB drives, its bytes pulled in
parts with `download_part` (or it is aborted, or released back to the browser); `open`/`popup` events for new tabs a
task tab starts (`open_seen` answers an `open` within the extension's ~1 s wait, after which the browser opens the link
itself; `open_plain` gives one back to the browser; `popup_ready` lets the page hook load a claimed popup); the hello
lists armed tabs and `disarm` lets go of one the daemon doesn't hold. Chromium native-message size is a per-frame transport bound; content travels in chunks
without clipping. Protocol 12 marks secret fills unknown when input began but a changed widget prevents readback,
so P19 requires reconciliation or owner-approved retry rather than treating partial input as a known failure.
Protocol 13 adds caller-selected exact split-code fields to sealed fill; older extensions must not ignore the group.
Protocol 14 adds `media` (L26): play/pause/mute/unmute/seek one video/audio element of the latest snapshot through the
content script, and each snapshot frame lists its media elements' state.

Protocol 14 (L13) stops refusing captures of pages with secret fields: `screenshot` and `record_frame` return the
picture with a `mask`, the page's mask state before and after the capture (the top document's secret-field rects and
its frame-host boxes, a host holding a secret painted whole, top-viewport CSS px), that the daemon paints over before
anything else sees it, or `withheld` when they can't be known; `window_secrets` (was `window_password`) returns that
mask state for the active tab of a window a native capture shows. An older daemon would pass such a picture on unpainted, and a newer
one can't mask an older extension's, so neither side accepts the other.
Protocol 15 (fix wave 4, extension 0.15.4): `window_secrets` takes the captured window's `title`, tells windows at the
same bounds apart by their active tab's title, and answers with that tab (`tab`: URL with secret parameters masked,
title), which the daemon checks against the captured window and the front window uses as its tab. An older daemon
would ignore the check; an older extension can't break ties or name the tab.
Protocol 16 (fix wave 4 round 3, extension 0.15.5): `screenshot` and `record_frame` attach fully when the page can be
scripted and with the debugger alone when it can't (the input-readiness probe needs scripting), and send a page they
can't read with `unread`, its look before
and `window` ({bounds, active, minimized, title, emulated?}); a page the debugger can't attach to answers `withheld`
with `undebuggable` and `window`, for the daemon's native window capture.
Protocol 17 (fix wave 4 round 5, extension 0.15.6): a capture of a page scripts can't reach stays armed (no full
attach and detach per capture), a tab already attached fully isn't attached again, and a scriptable page's input
setup or readiness failure refuses no capture: the reply's `input_note` says so.
Extension 0.15.8 (L31) adds optional reply fields without a protocol change: each snapshot media entry's `id` (the
element's own for the page's life) and each `list_tabs` tab's `audible` and `muted`. An older daemon ignores them; with
an older extension they are absent, so a loop probe on them sees its field missing (refused at arming, a trip after).
Extension 0.15.9 (L33) adds optional reply fields without a protocol change, for FERB's cursor: the `snapshot` reply
carries the tab's `window` (as protocol 16's captures do); each frame its `visual` viewport with its pinch zoom
(x, y, w, h, scale), its `scroll` sideways too (`x`, `width`) and `body_scrolls` when its body scrolls by itself; and
each element `pinned` (fixed or sticky) and `scroll_box` (a box other than the page scrolls it). Its centring scrolls
are instant (`behavior: "instant"`), so a smooth-scrolling page no longer moves under real input's aim. Without these
fields (an older extension) no browser point is published."""

import base64
import hashlib
import hmac
import json
from typing import Any, Literal

from pydantic import BaseModel, Field, ValidationError

PROTOCOL_VERSION = 17
MIN_EXTENSION_VERSION = "0.15.6"
FERB_EXTENSION_ID = "jaclnphefmmgmpofnabffahdkbjjompm"   # from the key in extensions/chromium/manifest.json
MAX_MESSAGE = 1024 * 1024                      # same cap as the daemon socket

ExtType = Literal["hello", "pair", "result", "error", "tab_event", "event"]
Op = Literal["open", "navigate", "close", "list_tabs", "snapshot", "act", "wait", "screenshot", "screenshot_part", "record_frame",
             "wake_close", "frame_origin", "fill", "read_source", "window_secrets", "scroll", "find",
             "upload", "result_part", "command_part", "dialog", "ping", "point", "pointer", "pointer_cancel", "release_tab",
             "download_part", "disarm", "open_plain", "popup_ready", "open_seen", "media"]


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
    armed: list[int] = []                      # tabs holding downloads for FERB (D35): the daemon disarms any it doesn't hold


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
    event: Literal["dialog", "detached", "file_chooser", "download", "open", "popup"]
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
    uncaptured: bool | None = None             # download: nothing is held (the browser started it itself, or detail
                                               # "dropped": a page's save past the hold caps, neither taken nor given)
    url: str | None = Field(default=None, max_length=8192)         # open: the link a task tab asked for; popup: masked
    opener_tab_id: int | None = None           # popup: the task tab that opened it
    window_id: int | None = None               # open: the window of the task tab (its new tab opens there, inactive)
    background: bool | None = None             # open: a modifier or middle click (the owner's intent: a background tab)
    open_id: str | None = Field(default=None, max_length=64)       # open: what `open_seen` answers


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
