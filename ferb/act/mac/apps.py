"""Enumerate running processes and launch or explicitly activate one app. Must not resolve ambiguity by order or restore focus."""

from dataclasses import dataclass
from typing import Any

from ferb.act.mac import browsers


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


def pids_of(bundle_ids: Any) -> set[int]:
    """The pids of every running app (whatever its activation policy: accessory and UI-element apps included) whose
    bundle id, casefolded, is in `bundle_ids` (fix round 3, H1: the notch is an accessory app)."""
    from AppKit import NSWorkspace
    return {int(a.processIdentifier()) for a in NSWorkspace.sharedWorkspace().runningApplications()
            if a.bundleIdentifier() and str(a.bundleIdentifier()).casefold() in bundle_ids}


def candidates(name_or_bundle: str) -> list[RunningApp]:
    """Return all matching processes. Exact equality is lookup mechanics, never first-match selection."""
    apps = running()
    if name_or_bundle.startswith("pid:"):
        try:
            pid = int(name_or_bundle[4:])
        except ValueError:
            return []
        return [a for a in apps if a.pid == pid]
    q = browsers.clean(name_or_bundle)                  # invisible marks aside (WhatsApp's name starts with U+200E)
    return [a for a in apps if a.bundle_id.casefold() == q or browsers.clean(a.name) == q]


def find(name_or_bundle: str) -> RunningApp | None:
    matches = candidates(name_or_bundle)
    return matches[0] if len(matches) == 1 else None


# The one browser list's Launch Services check (ferb/act/mac/browsers.py), here too so a driver's `apps` can be faked.
opens_web_links = browsers.opens_web_links
known_opens_web_links = browsers.known_opens_web_links


# D31: what an app's Info.plist declares about running programs. Terminal.app declares the Shell role for executables
# and shell scripts; an editor declares Editor for shell scripts. A fact the app states, read once per bundle id.
SHELL_TYPES = frozenset({"public.unix-executable", "com.apple.terminal.shell-script", "public.shell-script"})
_shell_apps: dict[str, bool] = {}


def shell_role(info: Any) -> bool:
    """Whether an Info.plist (a mapping) has a document type with CFBundleTypeRole "Shell" for executables or shell
    scripts."""
    types = (info.get("CFBundleDocumentTypes") if info is not None else None) or ()
    return any(str(t.get("CFBundleTypeRole") or "") == "Shell" and
               bool(SHELL_TYPES & {str(u) for u in (t.get("LSItemContentTypes") or ())}) for t in types)


def declares_shell(bundle_id: str) -> bool:
    """Whether the installed app's Info.plist declares the Shell role for executables or shell scripts (`shell_role`;
    ferb/policy/terminal.py counts such an app a terminal). Read-only, cached per bundle id; False when the app or its
    Info.plist can't be read."""
    key = bundle_id.strip().casefold()
    if key not in _shell_apps:
        try:
            from AppKit import NSWorkspace
            from Foundation import NSBundle
            url = NSWorkspace.sharedWorkspace().URLForApplicationWithBundleIdentifier_(bundle_id)
            _shell_apps[key] = shell_role(NSBundle.bundleWithURL_(url).infoDictionary() if url is not None else None)
        except Exception:
            return False
    return _shell_apps[key]


def default_app_for(path: str) -> str | None:
    """The bundle id of the app Launch Services would open this file with (a double-click, Finder's open), or None.
    Read-only; nothing opens (D31 round 2, O1)."""
    try:
        from AppKit import NSWorkspace
        from Foundation import NSBundle, NSURL
        url = NSWorkspace.sharedWorkspace().URLForApplicationToOpenURL_(NSURL.fileURLWithPath_(path))
        bundle = NSBundle.bundleWithURL_(url) if url is not None else None
        ident = bundle.bundleIdentifier() if bundle is not None else None
        return str(ident) if ident else None
    except Exception:
        return None


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


def installed_bundle(name: str) -> str | None:
    """The bundle id of the installed app macOS finds by this name (Launch Services' own lookup, as `open -a` does),
    for an app that isn't running (GATE-560); None when it finds none or the name is a `pid:`. Lookup mechanics only."""
    if not name.strip() or name.startswith("pid:"):
        return None
    from AppKit import NSBundle, NSWorkspace
    path = NSWorkspace.sharedWorkspace().fullPathForApplication_(browsers.INVISIBLE.sub("", name).strip())
    bundle = NSBundle.bundleWithPath_(path) if path else None
    found = bundle.bundleIdentifier() if bundle is not None else None
    return str(found) if found else None


def launch(bundle_id: str) -> bool:
    """Open an installed app by bundle id in the background (`open -g`: not activated), leaving a running one as it
    is. Not hidden (`-j` is gone, step 12e fix 4): a hidden app's windows are off screen, so a window screenshot and
    pointer input to its window can't reach them; Accessibility reads either way."""
    if candidates(bundle_id):
        return True  # Running is a fact; reopening windows is a separate explicit action.
    from AppKit import NSWorkspace
    ws = NSWorkspace.sharedWorkspace()
    url = ws.URLForApplicationWithBundleIdentifier_(bundle_id)
    if url is None:
        return False
    import subprocess
    return subprocess.run(["open", "-g", "-b", bundle_id], capture_output=True).returncode == 0


def is_application(path: str) -> bool:
    """Blocking: Launch Services counts this path an application (an .app bundle, an applet): opening it starts a
    program. Read-only; unreadable counts as one (the open is refused rather than run unchecked)."""
    try:
        from Foundation import NSURL, NSURLIsApplicationKey
        ok, value, _ = NSURL.fileURLWithPath_(path).getResourceValue_forKey_error_(None, NSURLIsApplicationKey, None)
        return bool(value) if ok else True
    except Exception:
        return True


# Launch Services types whose files start code when opened, whichever app opens them (hands ruling, mac.open): a
# program or script (a .jar opens in Java Launcher, a .py in Python Launcher), an installer package, a configuration
# profile, a shortcut or workflow
RUNNABLE_TYPES = ("public.executable", "public.script", "com.apple.installer-package-archive",
                  "com.apple.installer-package", "com.apple.installer-meta-package", "com.apple.mobileconfig",
                  "com.apple.configprofile", "com.apple.shortcut", "com.apple.shortcuts.workflow-file",
                  "com.apple.automator-workflow")


def runs_when_opened(path: str) -> bool:
    """Blocking: opening this file can start code, by its Launch Services type (it conforms to one of
    `RUNNABLE_TYPES`) or because it is a package directory; a structural fact about the file, not about the app.
    Read-only; an unknown type (a dynamic `dyn.` one) or an unreadable one counts as one (the open asks first)."""
    try:
        from AppKit import NSWorkspace
        ws = NSWorkspace.sharedWorkspace()
        if ws.isFilePackageAtPath_(path):
            return True
        kind, _ = ws.typeOfFile_error_(path, None)
        if not kind or str(kind).startswith("dyn."):
            return True
        return any(ws.type_conformsToType_(kind, t) for t in RUNNABLE_TYPES)
    except Exception:
        return True


# Configuration profiles install settings whichever app opens them: an app's Editor role never lifts their question
PROFILE_TYPES = ("com.apple.mobileconfig", "com.apple.configprofile")
_document_types: dict[str, tuple[Any, ...]] = {}


def editor_role(info: Any, kind: str, extension: str, conforms: Any) -> bool:
    """Whether an Info.plist (a mapping) declares the app the Editor of a file: a CFBundleDocumentTypes entry with
    CFBundleTypeRole "Editor" listing a content type the file's type (`kind`) conforms to (`conforms(kind, type)`), or
    its extension in CFBundleTypeExtensions. Viewer, Shell, None or no role is not."""
    types = (info.get("CFBundleDocumentTypes") if info is not None else None) or ()
    for t in types:
        if str(t.get("CFBundleTypeRole") or "") != "Editor":
            continue
        if kind and any(conforms(kind, str(u)) for u in (t.get("LSItemContentTypes") or ())):
            return True
        if extension and extension in {str(e).casefold() for e in (t.get("CFBundleTypeExtensions") or ())}:
            return True
    return False


def edits(bundle_id: str, path: str) -> bool:
    """Blocking: the installed app states it opens this file as its Editor (`editor_role`; the app's own Info.plist, a
    fact it declares, read once per bundle id), so opening the file there shows it rather than runs it. Never for a
    configuration profile; False when the app, its Info.plist or the file's type can't be read (the open asks)."""
    import os
    try:
        from AppKit import NSWorkspace
        ws = NSWorkspace.sharedWorkspace()
        kind, _ = ws.typeOfFile_error_(path, None)
        kind = str(kind) if kind else ""

        def conforms(a: str, b: str) -> bool:
            return bool(ws.type_conformsToType_(a, b))
        if not kind or any(conforms(kind, t) for t in PROFILE_TYPES):
            return False
        key = bundle_id.strip().casefold()
        if key not in _document_types:
            from Foundation import NSBundle
            url = ws.URLForApplicationWithBundleIdentifier_(bundle_id)
            info = NSBundle.bundleWithURL_(url).infoDictionary() if url is not None else None
            _document_types[key] = tuple((info or {}).get("CFBundleDocumentTypes") or ())
        extension = os.path.splitext(path.rstrip("/"))[1][1:].casefold()
        return editor_role({"CFBundleDocumentTypes": _document_types[key]}, kind, extension, conforms)
    except Exception:
        return False


OPEN_WAIT_S = 15.0       # how long FERB waits for macOS to confirm an open (speed only: nothing is retried)


def open_in(bundle_id: str, target: str) -> str | None:
    """Blocking (step 12e fix 5): open a file (an absolute path) or a web address in this installed app without
    activating it (NSWorkspace's open configuration, `activates` off; the app launches in the background when it isn't
    running). None when macOS confirmed it, else why not. The gateway's checks (protection lists, the address's site,
    the terminal question) come first; this only opens."""
    import threading
    from AppKit import NSWorkspace, NSWorkspaceOpenConfiguration
    from Foundation import NSURL
    ws = NSWorkspace.sharedWorkspace()
    app_url = ws.URLForApplicationWithBundleIdentifier_(bundle_id)
    if app_url is None:
        return f"no installed app {bundle_id}"
    url = NSURL.fileURLWithPath_(target) if target.startswith("/") else NSURL.URLWithString_(target)
    if url is None:
        return f"{target!r} isn't an address macOS can open"
    config = NSWorkspaceOpenConfiguration.configuration()
    config.setActivates_(False)
    done, failed = threading.Event(), []

    def finished(app: Any, error: Any) -> None:
        if error is not None:
            failed.append(str(error.localizedDescription()))
        done.set()
    ws.openURLs_withApplicationAtURL_configuration_completionHandler_([url], app_url, config, finished)
    if not done.wait(OPEN_WAIT_S):
        return f"macOS didn't confirm the open within {OPEN_WAIT_S:.0f} s; check with mac_snapshot before trying again"
    return failed[0] if failed else None


def _code(four: str) -> int:
    return int.from_bytes(four.encode("ascii"), "big")


def reopen(pid: int) -> str | None:
    """Blocking (step 12e fix 4): ask a running app for a window the way a Dock click does (the core `reopen` Apple
    Event, kAEReopenApplication), without bringing it forward: sent without `kAECanSwitchLayer`, never interacting,
    no reply awaited. An app may still come forward by itself in answer; that is the call's screen fact (hands
    HAND-212).
    None when sent, else why not."""
    from Foundation import NSAppleEventDescriptor
    target = NSAppleEventDescriptor.descriptorWithProcessIdentifier_(pid)
    event = NSAppleEventDescriptor.appleEventWithEventClass_eventID_targetDescriptor_returnID_transactionID_(
        _code("aevt"), _code("rapp"), target, -1, 0)              # kAutoGenerateReturnID, kAnyTransactionID
    no_reply, never_interact, dont_record = 0x1, 0x10, 0x1000     # NSAppleEventSend… (no CanSwitchLayer, 0x40)
    _, error = event.sendEventWithOptions_timeout_error_(no_reply | never_interact | dont_record, 5.0, None)
    return None if error is None else str(error.localizedDescription())
