"""L34 Phase G: Sparkle ships inside FERB.app, signed inside out as Sparkle documents for apps signed outside Xcode;
release builds name FERB's public feed and key and send no system profile; the development build never updates; the
private update key never goes in the repository; Sparkle's license ships."""

import json
import os
import plistlib
import re
import subprocess
from pathlib import Path

import pytest

from tests.support.signing_stubs import entitlements, macho, signed_calls, stub_env

ROOT = Path(__file__).resolve().parent.parent
BUILD_APP = ROOT / "scripts" / "build-app.sh"
NOTCH = ROOT / "ui" / "mac" / "FERBNotch"


def _fake_app(root: Path) -> Path:
    app = root / "FERB.app"
    c = app / "Contents"
    macho(c / "MacOS" / "FERBNotch")
    b = c / "Frameworks" / "Sparkle.framework" / "Versions" / "B"
    for rel in ("Sparkle", "Autoupdate", "Updater.app/Contents/MacOS/Updater",
                "XPCServices/Installer.xpc/Contents/MacOS/Installer",
                "XPCServices/Downloader.xpc/Contents/MacOS/Downloader"):
        macho(b / rel)
    (c / "Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "com.ferb.FERB",
                                                    "CFBundleExecutable": "FERBNotch"}))
    return app


def test_sparkle_is_signed_inside_out_before_the_app(tmp_path: Path) -> None:
    app = _fake_app(tmp_path / "out")
    done = subprocess.run([str(BUILD_APP), "--sign-only", str(app)], capture_output=True, text=True,
                          env=stub_env(tmp_path))
    assert done.returncode == 0, done.stderr + done.stdout
    calls = signed_calls(tmp_path)
    paths = [c.split("|")[-1] for c in calls]
    fw = app / "Contents" / "Frameworks" / "Sparkle.framework"
    b = fw / "Versions" / "B"
    order = [str(b / "XPCServices" / "Installer.xpc"), str(b / "XPCServices" / "Downloader.xpc"), str(b / "Autoupdate"),
             str(b / "Updater.app"), str(fw)]
    assert [p for p in paths if p in order] == order
    assert paths[-1] == str(app)
    assert "|--preserve-metadata=entitlements|" in next(c for c in calls if c.endswith("Downloader.xpc"))
    inner = ("/Versions/B/Sparkle", "/MacOS/Updater", "/MacOS/Installer", "/MacOS/Downloader")
    assert not any(p.endswith(inner) for p in paths)              # signed with their bundles, never one by one
    assert all("|--options|runtime|" in c for c in calls)
    # self-signed code has no Team ID, so library validation would refuse Sparkle.framework (as the daemon's modules)
    assert "com.apple.security.cs.disable-library-validation" in entitlements(tmp_path, app)


def test_release_builds_name_the_public_feed_and_key_and_send_no_profile() -> None:
    text = BUILD_APP.read_text()
    assert "https://github.com/Json604/ferb/releases/latest/download/appcast-$ARCH.xml" in text
    for key in ("SUPublicEDKey", "SUEnableAutomaticChecks bool true", "SUAutomaticallyUpdate bool true",
                "SUScheduledCheckInterval integer 86400", "SUEnableSystemProfiling bool false"):
        assert key in text, key
    assert "-add_rpath @executable_path/../Frameworks" in text
    assert "SUFeedURL" not in (NOTCH / "Info.plist").read_text()          # the development build never updates


def test_sparkle_is_pinned_exactly() -> None:
    text = (NOTCH / "Package.swift").read_text()
    assert re.search(r'\.package\(url: "https://github\.com/sparkle-project/Sparkle", exact: "2\.\d+\.\d+"\)', text)
    assert '.product(name: "Sparkle", package: "Sparkle")' in text
    pins = json.loads((NOTCH / "Package.resolved").read_text())["pins"]
    assert [p["identity"] for p in pins] == ["sparkle"]


def test_the_private_update_key_stays_outside_the_checkout() -> None:
    done = subprocess.run([str(ROOT / "scripts" / "sparkle-keys.sh"), "--export", str(ROOT / "key-backup")],
                          capture_output=True, text=True)
    assert done.returncode == 1 and "outside this checkout" in done.stderr
    assert not (ROOT / "key-backup").exists()
    assert "sparkle_private" in (ROOT / ".gitignore").read_text()


def test_sparkles_license_ships() -> None:
    for notices in (ROOT / "THIRD_PARTY_NOTICES.md", NOTCH / "ThirdPartyNotices.txt"):
        assert "Sparkle" in notices.read_text(), notices


@pytest.mark.skipif(not os.environ.get("FERB_APP_BUNDLE"), reason="set FERB_APP_BUNDLE=/path/FERB.app to check a build")
def test_a_built_app_embeds_sparkle_and_its_feed() -> None:
    app = Path(os.environ["FERB_APP_BUNDLE"])
    info = plistlib.loads((app / "Contents" / "Info.plist").read_bytes())
    assert info["SUFeedURL"].startswith("https://github.com/Json604/ferb/releases/latest/download/appcast-")
    assert info["SUPublicEDKey"] and info["SUEnableSystemProfiling"] is False
    assert (app / "Contents" / "Frameworks" / "Sparkle.framework").is_dir()
    loads = subprocess.run(["otool", "-l", str(app / "Contents" / "MacOS" / "FERBNotch")], capture_output=True,
                           text=True).stdout
    assert "@executable_path/../Frameworks" in loads
    assert subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)]).returncode == 0


def test_every_swift_build_path_fetches_dependencies_anonymously() -> None:
    """The build never touches the owner's keychain: git's credential helper is off for SwiftPM's fetch of Sparkle."""
    root = Path(__file__).resolve().parent.parent
    for script in ("scripts/build-app.sh", "ui/mac/FERBNotch/build.sh", "scripts/sparkle-keys.sh"):
        assert "swiftpm-anonymous.sh" in (root / script).read_text(), script
    done = subprocess.run(["/bin/zsh", "-f", "-c", f'source {root}/scripts/swiftpm-anonymous.sh; '
                           'git config --list | grep -i "^credential"'], capture_output=True, text=True)
    assert done.stdout.strip() == "credential.helper=", done.stdout
