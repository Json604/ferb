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
fields (an older extension) no browser point is published.
Extension 0.15.10 (L31) changes no reply's shape and no protocol: a snapshot waits up to 300 ms for media it hasn't
seen before, unpaused, to move, so its first `playing` already says whether it plays (content.js `firstLook`); an
older extension reports `playing` false on its first look, as before. It also adds one command, `rest {tab_id,
resting}` (file-chooser interception off while a loop's held tab rests between rounds, on for a round), which the
daemon sends only to an extension reporting 0.15.10 or later (`ext_bridge.REST_VERSION`): an older extension is never
sent it (its held tab keeps the picker intercepted), and an older daemon never sends it, so neither side meets a
command it doesn't know and the protocol number and minimum stay. It fails closed: a detach forgets `resting`, `rest`
records it even without a session, and any acting command (act, fill, upload, navigate, media, scroll, pointer) in a
resting tab takes the picker back before it runs; the daemon retries an unconfirmed rest(false) once. A dialog the
owner closes in the browser (`Page.javascriptDialogClosed`) clears the extension's pending dialog.
Extension 0.15.11 (D31) adds one optional reply field without a protocol change: a snapshot element inside xterm.js's
own nodes (class `xterm` on an ancestor, or `xterm-helper-textarea` on the element) carries `terminal: true`, so the
daemon asks the owner before typing or pressing a key into it (a web terminal). An older extension never reports it:
until it updates, a web terminal reached through browser tools isn't recognized (one reached through mac.act still is,
by the DOM classes Accessibility exposes). Review round 1 (D31): the minimum is therefore 0.15.11 (the owner published
it), with the protocol unchanged at 17: an older extension meets the "reload the FERB extension" refusal at hello, so no
web terminal goes unrecognized. Extension 0.15.12 changes no protocol and no minimum: snapshots, screenshot points and
element adoption reach closed shadow roots (live 2026-10-26: Cloudflare's checkbox frame sits in one), and a pointer
press that never started (its move never reached the target's frame) is a known failure with `reason:
pointer_did_not_reach_target_frame`. Extension 0.15.13 adds one command without a protocol change: `show_tab {tab_id}`
brings that tab forward in its own window (restored if minimized, focused), sent only when the owner clicks FERB's
human-check notification.

Protocol 18 (step 5c, extension 0.16.0, the minimum; one bump for all of it, so the version gates for `rest` and
`show_tab` are gone):
- Daemon messages that act are signed: `welcome` and `cmd` travel as {type, seq, body (the JSON text), mac}, the mac an
  HMAC-SHA256 with the pairing key over "<nonce>|<seq>|<type>|<body>", the nonce fresh in each hello and seq strictly
  increasing per connection (`daemon_envelope`), so a message can't be forged without the key nor replayed on another
  connection; the extension acts on nothing else. It takes a `paired` only while its own pairing page is pending.
- Tab groups (HAND-201): `open` takes `group` {root, task, title}: the tab joins its task's collapsed group;
  `adopt_tab` records a popup (grouped) or the owner's borrowed tab (never grouped). The hello's `tabs` lists the
  records still valid in this browser session ({tab_id, task, root, borrowed, cid, url}), so a reconnect or a daemon
  restart re-adopts them or closes those of an ended task; `close` closes only a tab FERB opened; a `taken` event says
  the owner moved a FERB tab out of its group (theirs now); `release_tab` with keep_open hands a tab over out of the
  group.
- Card fields (autocomplete cc-number / cc-csc / cc-exp*) are secret like password and code fields; elements carry
  `field_kind`, `submit` (what a click or Enter would submit: action host, method, the form's secret kinds), `path`
  (DOM path) and `backend_node` (the debugger's node id, when the accessibility pass mapped it); a frame may carry
  `commerce` {amount, merchant} from structured data; an act that can submit reports the page's own `submitted` events.
- A read's error (READS) is a known failure, never "outcome unknown". Queued messages leave after the welcome, signed
  then. `list_tabs` URLs are masked. The page hook's events and the linking attributes have random names per
  service-worker start (no fixed `ferb-armed` / `data-ferb-*` marker).

Protocol 19 (step 14b, extension 0.17.0, the minimum; HAND-207, "sound is part of background-first"):
- `open` and `adopt_tab` (a popup of a FERB tab) take `muted`: the tab is muted through the browser's own tab mute
  before anything loads (the page is untouched) and the reply says whether it is (`muted`). `mute_tab {tab_id, muted}`
  mutes or unmutes only a tab FERB opened (its records hold it, not borrowed, not handed over); any other answers
  `changed: false, reason: not_ferbs`. A tab that becomes the owner's (`release_tab` keep_open, or moved out of its
  group: `taken`) gets its sound back when FERB's extension had muted it.
- A `sound` tab event: a tab's sound started or stopped, or it was muted or unmuted (`changed`: audible / muted), with
  its `audible`, `muted` and whether it is FERB's (`ferb`, as `list_tabs` says it); no URL or title. The daemon records
  one in a tab that isn't FERB's, near a FERB call, as a disturbance fact.
An older extension would open loud tabs and send no sound events, so the minimum moves with the protocol.

Protocol 20 (step 12e, extension 0.18.0, the minimum; HAND-210, download limits, and the bugs the research
found):
- `download_part` reads parts of DOWNLOAD_PART = 512 KiB (was 72,000 bytes): its reply (about 700,000 base64
  characters) goes whole, never split into result parts. The host relays at most 1 MiB a message (MAX_MESSAGE; Chrome
  itself takes up to 64 MiB from the extension to the host, 1 MB the other way, which download data never travels).
- `popup_refused {tab_id}`: a popup the daemon won't make the task's (past browser.task_max_tabs) is closed by the
  extension while it is still blank (the page's hook never loads its URL), so it never becomes an ordinary tab whose
  downloads skip every limit. A link past the cap is simply not opened (no `open_plain`).
- A page's saves dropped past the hold caps are all reported: the first at once, then those that follow within the
  minute in one event, each `download` event with detail "dropped" carrying `count`.
An older extension would hand a capped popup to the browser and report drops once a minute at most.

Protocol 21 (step 12e, extension 0.19.0, the minimum; HAND-213, owner 2026-10-31):
- A `sound` tab event carries `muted_reason`: who last changed the tab's mute, as the browser reports it
  (`mutedInfo.reason`: "user", "capture" or "extension"; null when it was never changed). The owner's own mute or
  unmute ("user") is never recorded as a disturbance fact.
An older extension sends no reason, so the owner's own mute could be counted against FERB."""

import base64
import hashlib
import hmac
import json
from typing import Any, Literal

from pydantic import BaseModel, Field, ValidationError

PROTOCOL_VERSION = 21
MIN_EXTENSION_VERSION = "0.19.0"
FERB_EXTENSION_ID = "jaclnphefmmgmpofnabffahdkbjjompm"   # from the key in extensions/chromium/manifest.json
MAX_MESSAGE = 1024 * 1024                      # same cap as the daemon socket

ExtType = Literal["hello", "pair", "result", "error", "tab_event", "event"]
Op = Literal["open", "navigate", "close", "list_tabs", "snapshot", "act", "wait", "screenshot", "screenshot_part", "record_frame",
             "wake_close", "frame_origin", "fill", "read_source", "window_secrets", "scroll", "find",
             "upload", "result_part", "command_part", "dialog", "ping", "point", "pointer", "pointer_cancel", "release_tab",
             "download_part", "disarm", "open_plain", "popup_ready", "open_seen", "media", "rest", "show_tab",
             "adopt_tab", "mute_tab", "popup_refused"]
# Commands that never change the page (protocol.js READ_OPS): one that fails or times out is a known failure.
READS: frozenset[str] = frozenset({"list_tabs", "snapshot", "wait", "screenshot", "screenshot_part", "record_frame",
                                   "frame_origin", "read_source", "window_secrets", "find", "result_part", "ping",
                                   "point"})


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
    nonce: str = Field(default="", max_length=64)   # protocol 18: this connection's signing nonce (daemon_envelope)
    tabs: list[dict[str, Any]] = []            # protocol 18: the tabs FERB holds in this browser session (its records)


class Result(BaseModel):
    id: str
    gen: int
    status: Literal["ok", "error", "stale", "duplicate"]
    value: Any = None
    error: str | None = None
    outcome: Literal["done", "failed", "unknown"] | None = None


class TabEvent(BaseModel):
    event: Literal["created", "updated", "activated", "removed", "sound"]
    tab_id: int
    url: str | None = None
    title: str | None = None
    active: bool | None = None
    incognito: bool | None = None
    audible: bool | None = None                # sound (protocol 19): the browser's own facts after the change
    muted: bool | None = None
    ferb: bool | None = None                   # sound: a tab FERB opened, or in FERB's own window
    changed: list[Literal["audible", "muted"]] = []
    muted_reason: str | None = None            # sound (protocol 21): mutedInfo.reason, who last changed the mute


class Event(BaseModel):
    """Observed tab dialog, intercepted file chooser, debugger detachment, or a download caught in a tab FERB drives
    (held by the extension until `download_part` takes or cancels it); no implied user decision."""
    event: Literal["dialog", "detached", "file_chooser", "download", "open", "popup", "taken"]
    tab_id: int
    type: str | None = None                    # dialog: alert | confirm | prompt | beforeunload
    message: str | None = None
    handled: str | None = None                 # dialog: accepted | dismissed
    reason: str | None = None                  # detached: canceled_by_user | target_closed | ...; taken: how it left
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
    count: int | None = Field(default=None, ge=1)  # download "dropped" (protocol 20): how many saves this event reports
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
    return {"id": cid, "gen": gen, "op": op, "args": args, "timeout_ms": timeout_ms}


def daemon_mac(key: bytes, nonce: str, seq: int, mtype: str, body: str) -> str:
    return base64.b64encode(hmac.new(key, f"{nonce}|{seq}|{mtype}|{body}".encode(), hashlib.sha256).digest()).decode()


def daemon_envelope(key: bytes, nonce: str, seq: int, mtype: Literal["welcome", "cmd"],
                    body: dict[str, Any]) -> dict[str, Any]:
    """A daemon message the extension acts on (protocol 18): its body as JSON text (relayed unchanged as a string), a
    counter strictly increasing on this connection and an HMAC with the pairing key over this connection's hello nonce,
    the counter, the type and the body."""
    text = json.dumps(body)
    return {"type": mtype, "seq": seq, "body": text, "mac": daemon_mac(key, nonce, seq, mtype, text)}
