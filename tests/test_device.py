"""Device scan + profile (section 8 step 4): Info.plist-only scan, scriptability, diff, method order, and the
local, never-pushed git history in ~/.ferb/device/."""

import plistlib
from pathlib import Path

from ferb.cli.setup import device_id
from ferb.config import Config
from ferb.device.profile import DeviceProfile, ProfileStore, diff, methods_for
from ferb.device.scan import scan, scan_apps


def make_app(root: Path, name: str, bundle_id: str | None, version: str = "1.0", binary: bool = False,
             sdef: bool = False, **extra: object) -> Path:
    b = root / f"{name}.app"
    (b / "Contents" / "Resources").mkdir(parents=True)
    info: dict[str, object] = {"CFBundleName": name, "CFBundleShortVersionString": version, **extra}
    if bundle_id:
        info["CFBundleIdentifier"] = bundle_id
    (b / "Contents" / "Info.plist").write_bytes(plistlib.dumps(info, fmt=plistlib.FMT_BINARY if binary
                                                               else plistlib.FMT_XML))
    if sdef:
        (b / "Contents" / "Resources" / f"{name}.sdef").write_text("<dictionary/>")
    return b


def test_scan_reads_info_plists_and_detects_scriptability(tmp_path: Path) -> None:
    apps = tmp_path / "Applications"
    make_app(apps, "Notes", "com.apple.Notes", sdef=True)
    make_app(apps, "Mail", "com.apple.mail", NSAppleScriptEnabled=True)
    make_app(apps, "Pages", "com.apple.iWork.Pages", binary=True, OSAScriptingDefinition="Pages.sdef")
    make_app(apps, "Calc", "com.example.calc")
    make_app(apps, "Broken", None)                        # no bundle id: skipped
    (apps / "Junk.app").mkdir()                          # no Info.plist: skipped
    found = {a.bundle_id: a for a in scan_apps([apps])}
    assert set(found) == {"com.apple.Notes", "com.apple.mail", "com.apple.iWork.Pages", "com.example.calc"}
    assert [found[b].scriptable for b in ("com.apple.Notes", "com.apple.mail", "com.apple.iWork.Pages",
                                          "com.example.calc")] == [True, True, True, False]
    assert found["com.apple.Notes"].ax_support == "untested"


def test_first_root_wins_and_browsers_are_detected_by_bundle_only(tmp_path: Path) -> None:
    sys_apps, user_apps = tmp_path / "Applications", tmp_path / "user"
    make_app(sys_apps, "Google Chrome", "com.google.Chrome", "154")
    make_app(user_apps, "Google Chrome", "com.google.Chrome", "99")
    make_app(sys_apps, "Brave Browser", "com.brave.Browser")
    p = scan("dev-1", [sys_apps, user_apps])
    assert p.app("com.google.Chrome").version == "154"
    assert [b.name for b in p.browsers] == ["brave", "chrome"]
    brave = next(b for b in p.browsers if b.name == "brave")
    assert "ext:brave" in brave.drivers and not any("not built" in s for s in brave.drivers.values())


def test_profile_round_trip_diff_and_methods(tmp_path: Path) -> None:
    apps = tmp_path / "Applications"
    make_app(apps, "Notes", "com.apple.Notes", "1.0", sdef=True)
    make_app(apps, "Calc", "com.example.calc")
    old = scan("dev-1", [apps])
    assert DeviceProfile.from_json(old.to_json()) == old
    make_app(apps, "Figma", "com.figma.Desktop")
    import shutil
    shutil.rmtree(apps / "Calc.app")
    (apps / "Notes.app" / "Contents" / "Info.plist").write_bytes(plistlib.dumps(
        {"CFBundleIdentifier": "com.apple.Notes", "CFBundleName": "Notes", "CFBundleShortVersionString": "2.0"}))
    d = diff(old, scan("dev-1", [apps]))
    assert (d.added, d.removed, d.changed) == (["com.figma.Desktop"], ["com.example.calc"], ["com.apple.Notes"])
    assert methods_for(old.app("com.apple.Notes")) == ["applescript", "ax", "screenshot"]
    assert methods_for(old.app("com.example.calc")) == ["ax", "screenshot"]


def test_profile_history_is_a_local_repo_that_is_never_pushed(tmp_path: Path) -> None:
    apps = tmp_path / "Applications"
    make_app(apps, "Notes", "com.apple.Notes")
    store = ProfileStore(tmp_path / "home")
    p1 = scan("dev-1", [apps])
    assert store.save(p1, "first") is True
    later = scan("dev-1", [apps])
    later.scanned_at = "2099-01-01T00:00:00+00:00"                      # a later scan with the same apps
    assert store.save(later, "same apps") is False                      # only the time differs: no commit
    make_app(apps, "Calc", "com.example.calc")
    assert store.save(scan("dev-1", [apps]), "calc installed") is True
    assert store.history() == ["calc installed", "first"]               # newest first; "same apps" made none
    assert store.remotes() == []
    assert store.load("dev-1").app("com.example.calc") is not None
    assert (tmp_path / "home" / "device").stat().st_mode & 0o077 == 0


def test_device_id_is_created_once_and_stays(tmp_path: Path) -> None:
    c = Config(tmp_path / "home")
    first = device_id(c)
    assert device_id(c) == first and len(first) == 32
