"""Running apps: list, launch/activate, frontmost (section 3 act/mac). Never quits or force-quits an app."""

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


def find(name_or_bundle: str) -> RunningApp | None:
    q = name_or_bundle.lower()
    apps = running()
    return next((a for a in apps if a.bundle_id.lower() == q), None) or \
        next((a for a in apps if a.name.lower().strip("\u200e") == q.strip("\u200e")), None) or \
        _by_bundle(name_or_bundle)


def _by_bundle(bundle_id: str) -> RunningApp | None:
    """Asked of macOS directly — always current, even where the shared running-app list hasn't refreshed yet."""
    if "." not in bundle_id:
        return None
    from AppKit import NSRunningApplication
    for a in NSRunningApplication.runningApplicationsWithBundleIdentifier_(bundle_id):
        if not a.isTerminated():
            return RunningApp(str(a.bundleIdentifier()), str(a.localizedName() or ""), int(a.processIdentifier()),
                              bool(a.isActive()))
    return None


def frontmost() -> RunningApp | None:
    """The app the owner is in. Accessibility's focused application first: NSWorkspace's "active" flag goes stale in
    a long-running daemon (found in step 10), which made FERB "put back" the wrong app and leave the owner on another
    desktop (owner's report, 2026-10-01)."""
    try:
        from ferb.sense.watchers.base import focused_app
        f = focused_app()
    except Exception:
        f = None
    if f is not None:
        return RunningApp(f.bundle_id, f.name, f.pid, True)
    return next((a for a in running() if a.active), None)


def activate(bundle_id: str) -> bool:
    from AppKit import NSApplicationActivateIgnoringOtherApps, NSRunningApplication
    apps = NSRunningApplication.runningApplicationsWithBundleIdentifier_(bundle_id)
    return bool(apps) and bool(apps[0].activateWithOptions_(NSApplicationActivateIgnoringOtherApps))


def launch(bundle_id: str) -> bool:
    """Open (or bring back) an installed app by bundle id, without taking focus when it's already running."""
    from AppKit import NSWorkspace
    ws = NSWorkspace.sharedWorkspace()
    url = ws.URLForApplicationWithBundleIdentifier_(bundle_id)
    if url is None:
        return False
    import subprocess
    return subprocess.run(["open", "-g", "-b", bundle_id], capture_output=True).returncode == 0
