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
        next((a for a in apps if a.name.lower() == q), None)


def frontmost() -> RunningApp | None:
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
