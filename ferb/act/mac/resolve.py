"""Per-app method choice (section 8): AppleScript / CLI -> Accessibility -> real input (screenshot + click later).
From the device profile: AppleScript only for scriptable apps."""

from ferb.device.profile import AppInfo, DeviceProfile, methods_for


def methods(bundle_id: str, profile: DeviceProfile | None) -> list[str]:
    app = next((a for a in profile.apps if a.bundle_id == bundle_id), None) if profile else None
    return methods_for(app) if app else ["ax", "input"]


def scriptable(bundle_id: str, profile: DeviceProfile | None) -> bool:
    return "applescript" in methods(bundle_id, profile)


__all__ = ["AppInfo", "methods", "scriptable"]
