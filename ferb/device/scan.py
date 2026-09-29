"""Device scan (section 8 step 4): installed apps from their Info.plist only, scriptability, browsers present.

Reads app bundles only — never app data folders. Browsers are detected by bundle presence; Brave is never read
beyond that (owner's rule), and browser drivers are reported "not built yet" until build steps 5/6.
"""

import platform
import plistlib
from datetime import datetime
from pathlib import Path

from ferb.device.profile import AppInfo, BrowserInfo, DeviceProfile

APP_ROOTS = [Path("/Applications"), Path("/Applications/Utilities"), Path("/System/Applications"),
             Path("/System/Applications/Utilities"), Path.home() / "Applications"]

BROWSERS = {
    "com.google.Chrome": ("chrome", {"ext:chrome": "FERB extension, per paired profile (`ferb browsers`)",
                                     "ax:chrome": "Accessibility read fallback"}),
    "com.brave.Browser": ("brave", {"ext:brave": "FERB extension, per paired profile (`ferb browsers`)",
                                    "ax:brave": "Accessibility read fallback"}),
    "com.apple.Safari": ("safari", {"applescript_read": "watch-only (Block 1), when Automation is granted"}),
}


def read_app(bundle: Path) -> AppInfo | None:
    info_path = bundle / "Contents" / "Info.plist"
    try:
        with open(info_path, "rb") as f:
            info = plistlib.load(f)
    except (OSError, plistlib.InvalidFileException, ValueError):
        return None
    bundle_id = info.get("CFBundleIdentifier")
    if not isinstance(bundle_id, str) or not bundle_id:
        return None
    resources = bundle / "Contents" / "Resources"
    try:
        has_sdef = any(resources.glob("*.sdef"))
    except OSError:
        has_sdef = False
    scriptable = bool(info.get("NSAppleScriptEnabled")) or "OSAScriptingDefinition" in info or has_sdef
    name = info.get("CFBundleDisplayName") or info.get("CFBundleName") or bundle.stem
    return AppInfo(bundle_id, str(name), str(info.get("CFBundleShortVersionString", "")), str(bundle), scriptable)


def scan_apps(roots: list[Path] = APP_ROOTS) -> list[AppInfo]:
    seen: dict[str, AppInfo] = {}
    for root in roots:
        try:
            bundles = sorted(root.glob("*.app"))
        except OSError:
            continue
        for bundle in bundles:
            app = read_app(bundle)
            if app and app.bundle_id not in seen:       # first root wins (/Applications before ~/Applications)
                seen[app.bundle_id] = app
    return sorted(seen.values(), key=lambda a: a.bundle_id)


def browsers_from(apps: list[AppInfo]) -> list[BrowserInfo]:
    out = []
    for app in apps:
        if app.bundle_id in BROWSERS:
            name, drivers = BROWSERS[app.bundle_id]
            out.append(BrowserInfo(name, app.bundle_id, app.path, dict(drivers)))
    return sorted(out, key=lambda b: b.name)


def scan(device_id: str, roots: list[Path] = APP_ROOTS) -> DeviceProfile:
    apps = scan_apps(roots)
    return DeviceProfile(device_id=device_id, os="macos", os_version=platform.mac_ver()[0] or "unknown",
                         scanned_at=datetime.now().astimezone().isoformat(timespec="seconds"), apps=apps,
                         browsers=browsers_from(apps))
