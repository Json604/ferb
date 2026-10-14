"""Owns the owner's front window at the moment they send a message (L12; D6/D26 clarified). Must not bring anything
forward, choose a window by what it shows, or keep an unredacted pixel.

The front window is the topmost on-screen normal window whose owner process isn't FERB's own: the notch UI and
FERB.app take key focus while the owner types, and the daemon and what it started without an app bundle of its own
(its helpers, the commands it runs) are FERB too. An app with a bundle of its own is the owner's, whoever launched it,
and FERB's own browser profile windows are windows the owner sees: they count. When the message came from a CLI, the
window of the app hosting it (a terminal running `ferb chat`) is skipped while it is in front: the next one is the
owner's. What is kept: app, bundle id, window title, a browser's front tab URL and title, and one screenshot of that
window only (D29). The tab is read before the shot when it can be; when the browser's Automation check is still
answering, it is read again after the shot. The screenshot goes through FERB's screenshot path (secret fields
the window's tree shows are masked in the capture), then the F5 on-device redaction; when either fails the image is
withheld and the text facts stay. Text facts are redacted here, at capture (rule 14).

The owner's exclusion list governs perception recording, not what FERB sees when asked (owner, fix round 2: "no
exceptions"): a site on it is still seen, title and pixels, but its origin never names the host
(`external:web:excluded`, the gateway's convention) and its URL is the redactor's excluded marker; S4 still counts it as
outside content. An app on it is never looked at (rule 14: a password manager shows secrets field masking can't
reliably cover) and never named: the planner is told an app FERB doesn't look at ([excluded], as the redactor writes
it everywhere else) is in front."""

import asyncio
from dataclasses import dataclass, field
from typing import Any, Awaitable, Callable

from ferb.act.mac.ax_browser import is_browser
from ferb.act.mac.screenshot import Window
from ferb.core import origin as origins
from ferb.core.errors import Refused
from ferb.sanitize.redact import EXCLUDED

# FERB's own apps, as their bundles declare them: ui/mac/FERBNotch/Info.plist and macapp/FERB/Info.plist
FERB_APPS = frozenset({"dev.ferb.notch", "com.ferb.app"})
# libproc's PROC_PIDTBSDINFO answer (<sys/proc_info.h> struct proc_bsdinfo): its size, and where pbi_ppid lies in it
PROC_PIDTBSDINFO, BSDINFO_SIZE, PPID_OFFSET = 3, 136, 16
EXCLUDED_APP = f"an app FERB doesn't look at ({EXCLUDED}) is in front: no title, page or screenshot"


@dataclass(frozen=True)
class FrontWindow:
    app: str
    bundle_id: str
    title: str
    url: str | None                   # a browser's front tab, when FERB can read it
    tab_title: str | None
    image_ref: str | None             # the evidence-store row of the redacted screenshot; None: withheld
    origin: str                       # what the window shows, as content labels (MP2): data, never instructions
    withheld: str | None = None       # why there is no screenshot
    image: bytes | None = field(default=None, repr=False, compare=False)   # those same redacted bytes, in memory

    def facts(self) -> dict[str, Any]:
        """The labelled facts the planner is given (a browser's tab only when it was read)."""
        out: dict[str, Any] = {"app": self.app}
        if self.bundle_id:
            out["bundle_id"] = self.bundle_id
        if self.title:
            out["window_title"] = self.title
        if self.url or self.tab_title:
            out |= {"url": self.url, "tab_title": self.tab_title}
        return out


@dataclass(frozen=True)
class Sources:
    """What a capture reads. The daemon supplies the real ones; tests supply fakes."""
    windows: Callable[[], list[Window]]                   # on-screen normal windows, front to back
    app_of: Callable[[int], tuple[str, str] | None]       # a process's (app name, bundle id); None: not an app
    own: Callable[[int, str], bool]                       # (pid, bundle id): a FERB process (`ferb_process`)
    excluded: Callable[[str, str, str | None], bool]      # (bundle id, app name, url): on the owner's exclusion list
    tab: Callable[[str, bool], tuple[str, str] | None]    # (bundle id, after the shot): its front tab (url, title)
    shoot: Callable[[Window], Awaitable[bytes]]           # that window's JPEG, its tree's secret fields masked
    redact_image: Callable[[bytes], bytes]                # F5 on-device redaction; raises when it can't redact
    redact: Callable[[Any], Any]                          # text redaction
    keep: Callable[[bytes, dict[str, Any], str], Awaitable[str]]   # (image, facts, origin) -> evidence id
    sender: int | None = None                             # the app hosting the CLI the message came from (`sending_app`)


def parent_pid(pid: int) -> int | None:
    """A process's parent as the kernel reports it (libproc), or None when it can't be read."""
    import ctypes
    try:
        lib = ctypes.CDLL("/usr/lib/libproc.dylib")
        call = lib.proc_pidinfo
        call.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int]
        call.restype = ctypes.c_int
        buf = ctypes.create_string_buffer(BSDINFO_SIZE)
        n = call(pid, PROC_PIDTBSDINFO, 0, buf, BSDINFO_SIZE)
    except (OSError, AttributeError):
        return None
    return int.from_bytes(buf.raw[PPID_OFFSET:PPID_OFFSET + 4], "little") if n >= BSDINFO_SIZE else None


def ferb_process(pid: int, bundle_id: str, daemon_pid: int,
                 parent: Callable[[int], int | None] = parent_pid) -> bool:
    """Whether a window's owner is FERB: one of FERB's own apps, the daemon, or a process the daemon started that has
    no app bundle of its own (walked up through the parents; a process launchd started, as Launch Services opens apps,
    isn't the daemon's). An app with its own bundle is the owner's even when the daemon launched it (a browser)."""
    if bundle_id.lower() in FERB_APPS or pid == daemon_pid:
        return True
    if bundle_id:
        return False
    seen: set[int] = set()
    while pid > 1 and pid not in seen:
        if pid == daemon_pid:
            return True
        seen.add(pid)
        pid = parent(pid) or 0
    return False


def sending_app(pid: int, app_of: Callable[[int], tuple[str, str] | None],
                parent: Callable[[int], int | None] = parent_pid) -> int | None:
    """The app hosting a client process (the terminal running `ferb chat`): its first ancestor, itself included, with
    a bundle id. None: no app above it."""
    seen: set[int] = set()
    while pid > 1 and pid not in seen:
        if (app_of(pid) or ("", ""))[1]:
            return pid
        seen.add(pid)
        pid = parent(pid) or 0
    return None


def app_of(pid: int) -> tuple[str, str] | None:
    """A process's app name and bundle id (NSRunningApplication); None when it isn't an app."""
    from AppKit import NSRunningApplication  # type: ignore[import-untyped]
    app = NSRunningApplication.runningApplicationWithProcessIdentifier_(pid)
    if app is None:
        return None
    return str(app.localizedName() or ""), str(app.bundleIdentifier() or "")


def _topmost(src: Sources) -> tuple[Window, str, str] | None:
    """The topmost window not owned by FERB, with its owner's app name and bundle id; when that is the sending CLI's
    app window, the one after it."""
    sender_skipped = False
    for w in src.windows():
        name, bundle = src.app_of(w.pid) or ("", "")
        if src.own(w.pid, bundle):
            continue
        if w.pid == src.sender and not sender_skipped:     # the window the owner typed this message in
            sender_skipped = True
            continue
        return w, name, bundle
    return None


async def _tab(src: Sources, bundle: str, after_shot: bool) -> tuple[str | None, str | None] | None:
    """A browser's front tab (url, title), when it can be read now."""
    tab = await asyncio.to_thread(src.tab, bundle, after_shot)
    return (tab[0] or None, tab[1] or None) if tab is not None else None


async def capture(src: Sources) -> FrontWindow | None:
    """The owner's front window now, or None when no window but FERB's is in front. Never brings anything forward."""
    from ferb.act.mac.tools import _sources            # the mac tools' labels for what an app's window shows
    found = await asyncio.to_thread(_topmost, src)
    if found is None:
        return None
    w, name, bundle = found
    if src.excluded(bundle, name, None):               # never named or looked at: no title, tab or pixels (rule 14)
        return FrontWindow(EXCLUDED, "", "", None, None, None, origins.PRIVATE, EXCLUDED_APP)
    browser = is_browser(bundle)
    tab = await _tab(src, bundle, False) if browser else None
    image: bytes | None = None
    withheld: str | None = None
    try:
        raw = await src.shoot(w)
    except Refused as e:                               # FERB's own words: why this window gave no picture
        withheld = str(e)
    except Exception as e:
        withheld = f"the window couldn't be captured ({type(e).__name__})"
    else:
        try:
            image = await asyncio.to_thread(src.redact_image, raw)
        except Exception:
            withheld = "the screenshot couldn't be redacted, so it was withheld"
    if browser and tab is None:                        # its Automation check may have answered during the shot
        tab = await _tab(src, bundle, True)
    url, tab_title = tab if tab is not None else (None, None)
    excluded_site = bool(url) and src.excluded(bundle, name, url)
    read = (("web", url),) if url else _sources(bundle)
    origin = origins.external("web", "excluded") if excluded_site else \
        origins.join(origins.from_read_source(kind, ident) for kind, ident in read)
    facts = src.redact({"app": name, "bundle_id": bundle, "title": w.title, "url": EXCLUDED if excluded_site else url,
                        "tab_title": tab_title})
    ref: str | None = None
    if image is not None:
        try:
            ref = await src.keep(image, facts, origin)
        except Exception as e:
            image, withheld = None, f"the screenshot couldn't be kept as evidence ({type(e).__name__})"
    return FrontWindow(facts["app"], facts["bundle_id"], facts["title"], facts["url"], facts["tab_title"], ref,
                       origin, withheld, image)
