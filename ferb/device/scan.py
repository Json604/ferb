"""Discover installed apps, declared scripting and command-line tools. Must not infer task targets from this inventory.

Device scan: installed apps from their Info.plist only, scriptability, browsers present.

Reads app bundles only — never app data folders. Browsers are detected by bundle presence; Brave is never read
beyond that (owner's rule). Driver descriptions identify available transports, not verified app capabilities."""

import platform
import plistlib
from datetime import datetime
from pathlib import Path

from ferb.act.browser.profiles import BROWSERS as PAIRED
from ferb.act.mac import browsers as browser_list
from ferb.device.profile import AppInfo, BrowserInfo, DeviceProfile

APP_ROOTS = [Path("/Applications"), Path("/Applications/Utilities"), Path("/System/Applications"),
             Path("/System/Applications/Utilities"), Path.home() / "Applications"]

WEB_SCHEMES = frozenset({"http", "https"})


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


def registers_web(bundle: Path) -> bool:
    """Whether an app's Info.plist registers it for http or https links (CFBundleURLTypes): how macOS knows an app
    as a web browser."""
    try:
        with open(bundle / "Contents" / "Info.plist", "rb") as f:
            info = plistlib.load(f)
    except (OSError, plistlib.InvalidFileException, ValueError):
        return False
    types = info.get("CFBundleURLTypes") if isinstance(info, dict) else None
    return any(isinstance(t, dict) and WEB_SCHEMES & {str(x).casefold() for x in t.get("CFBundleURLSchemes") or ()}
               for t in types or ())


def browsers_from(apps: list[AppInfo]) -> list[BrowserInfo]:
    """The installed web browsers (the one list, ferb/act/mac/browsers.py: known bundle ids, or an app whose
    Info.plist registers it for web links), each with the transports FERB has for it: the extension for the Chromium
    browsers it pairs with, AppleScript reads for Safari, Accessibility for every one."""
    out = []
    for app in apps:
        if not (browser_list.known(app.bundle_id) or registers_web(Path(app.path))):
            continue
        name = browser_list.pair_key(app.bundle_id)
        name = name if name != app.bundle_id else app.name.casefold()
        drivers = {f"ax:{name}": "Accessibility read fallback"}
        if any(b.bundle_id.casefold() == app.bundle_id.casefold() for b in PAIRED.values()):
            drivers[f"ext:{name}"] = "FERB extension, per paired profile (`ferb browsers`)"
        if name == "safari":
            drivers = {"applescript_read": "watch-only (Block 1), when Automation is granted"}
        out.append(BrowserInfo(name, app.bundle_id, app.path, drivers))
    return sorted(out, key=lambda b: b.name)


def scan(device_id: str, roots: list[Path] = APP_ROOTS) -> DeviceProfile:
    apps = scan_apps(roots)
    return DeviceProfile(device_id=device_id, os="macos", os_version=platform.mac_ver()[0] or "unknown",
                         scanned_at=datetime.now().astimezone().isoformat(timespec="seconds"), apps=apps,
                         browsers=browsers_from(apps))
