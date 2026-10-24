"""Enumerate running processes and launch or explicitly activate one app. Must not resolve ambiguity by order or restore focus."""

import threading
import time
from dataclasses import dataclass


@dataclass(frozen=True)
class RunningApp:
    bundle_id: str
    name: str
    pid: int
    active: bool


def running() -> list[RunningApp]:
    from AppKit import NSApplicationActivationPolicyRegular, NSWorkspace
    out = []
    for a in NSWorkspace.sharedWorkspace().runningApplications():
        if a.activationPolicy() != NSApplicationActivationPolicyRegular or not a.bundleIdentifier():
            continue
        out.append(RunningApp(str(a.bundleIdentifier()), str(a.localizedName() or ""), int(a.processIdentifier()),
                              bool(a.isActive())))
    return out


def candidates(name_or_bundle: str) -> list[RunningApp]:
    """Return all matching processes. Exact equality is lookup mechanics, never first-match selection."""
    apps = running()
    if name_or_bundle.startswith("pid:"):
        try:
            pid = int(name_or_bundle[4:])
        except ValueError:
            return []
        return [a for a in apps if a.pid == pid]
    q = name_or_bundle.casefold().strip("\u200e")
    return [a for a in apps if a.bundle_id.casefold() == q or a.name.casefold().strip("\u200e") == q]


def find(name_or_bundle: str) -> RunningApp | None:
    matches = candidates(name_or_bundle)
    return matches[0] if len(matches) == 1 else None


HANDLERS_TTL_S = 60.0     # how long the https-handler list is reused before Launch Services is asked again (speed)
_clock = time.monotonic
_handlers: tuple[float, frozenset[str]] | None = None    # (when enumerated, bundle ids casefolded)
_handlers_lock = threading.Lock()


def _enumerate_handlers() -> frozenset[str]:
    """Every app Launch Services lists as opening https links, by bundle id (casefolded). Read-only; nothing opens."""
    from AppKit import NSWorkspace
    from Foundation import NSURL, NSBundle
    link = NSURL.URLWithString_("https://example.com")                   # only its scheme is looked up
    found: set[str] = set()
    for url in NSWorkspace.sharedWorkspace().URLsForApplicationsToOpenURL_(link) or ():
        bundle = NSBundle.bundleWithURL_(url)
        ident = bundle.bundleIdentifier() if bundle is not None else None
        if ident:
            found.add(str(ident).casefold())
    return frozenset(found)


def _fresh() -> frozenset[str] | None:
    with _handlers_lock:
        if _handlers is not None and _clock() - _handlers[0] < HANDLERS_TTL_S:
            return _handlers[1]
    return None


def opens_web_links(bundle_id: str) -> bool:
    """Whether Launch Services lists this app among those that open https links: a web browser, or an app that shows
    web pages like one (D12: mac.secret binds no secret to it). Enumerates at most once per HANDLERS_TTL_S; it blocks,
    so async code calls it off the event loop. False when Launch Services can't be asked (nothing is kept): FERB's own
    browser list (`profiles.is_web_browser`) still applies."""
    global _handlers
    handlers = _fresh()
    if handlers is None:
        try:
            handlers = _enumerate_handlers()
        except Exception:
            return False
        with _handlers_lock:
            _handlers = (_clock(), handlers)
    return bundle_id.strip().casefold() in handlers


def known_opens_web_links(bundle_id: str) -> bool:
    """`opens_web_links` from a list enumerated within HANDLERS_TTL_S, never enumerating (for synchronous checks on
    the event loop); False without one."""
    handlers = _fresh()
    return handlers is not None and bundle_id.strip().casefold() in handlers


def frontmost() -> RunningApp | None:
    """The app the owner is in. Accessibility's focused application first: NSWorkspace's "active" flag goes stale in
    a long-running daemon, which made FERB "put back" the wrong app and leave the owner on another
    desktop (owner's report, 2026-10-01)."""
    try:
        from ferb.sense.watchers.base import focused_app
        f = focused_app()
    except Exception:
        f = None
    if f is not None:
        return RunningApp(f.bundle_id, f.name, f.pid, True)
    return next((a for a in running() if a.active), None)


def activate(bundle_id: str, pid: int | None = None) -> bool:
    """Explicit foreground route: activate one already-resolved process, never the first bundle match."""
    from AppKit import NSApplicationActivateIgnoringOtherApps, NSRunningApplication
    matches = candidates(f"pid:{pid}" if pid is not None else bundle_id)
    if len(matches) != 1 or matches[0].bundle_id != bundle_id:
        return False
    app = NSRunningApplication.runningApplicationWithProcessIdentifier_(matches[0].pid)
    return bool(app and app.activateWithOptions_(NSApplicationActivateIgnoringOtherApps))


def launch(bundle_id: str) -> bool:
    """Open (or bring back) an installed app by bundle id, without taking focus when it's already running."""
    if candidates(bundle_id):
        return True  # Running is a fact; reopening windows is a separate explicit action.
    from AppKit import NSWorkspace
    ws = NSWorkspace.sharedWorkspace()
    url = ws.URLForApplicationWithBundleIdentifier_(bundle_id)
    if url is None:
        return False
    import subprocess
    return subprocess.run(["open", "-g", "-j", "-b", bundle_id], capture_output=True).returncode == 0
