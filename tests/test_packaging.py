"""Packaged FERB.app and DMG (L27): the app's Info.plist, the one signing step (ad-hoc now, Developer ID plus
notarization later, no rebuild) driven against stub signing tools, and a smoke test of a real bundle.

The real-bundle test checks a built app: `FERB_APP_BUNDLE=/path/FERB.app`, or `FERB_BUILD_APP=1` to build one into a
temp dir first (minutes, ~700 MB; needs swift, uv, npm and their caches or network). Otherwise it skips, saying so."""

import json
import os
import plistlib
import shlex
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

import ferb
from ferb.device.install import APP_BUNDLE_ID
from tests.support.macho import compile_macho

ROOT = Path(__file__).resolve().parent.parent
NOTCH_PLIST = ROOT / "ui" / "mac" / "FERBNotch" / "Info.plist"
BUILD_APP = ROOT / "scripts" / "build-app.sh"
BUILD_DMG = ROOT / "scripts" / "build-dmg.sh"
APP_PARTS = ROOT / "scripts" / "app-parts.sh"
LOCK = ROOT / "uv.lock"
DEV_ID = "Developer ID Application: Test Owner (TEAM123456)"


# ---------- the app's identity ----------

def test_ferb_app_info_plist_names_ferb_and_declares_its_usage_descriptions() -> None:
    info = plistlib.loads(NOTCH_PLIST.read_bytes())
    assert info["CFBundleName"] == info["CFBundleDisplayName"] == "FERB"
    assert info["CFBundleIdentifier"] == APP_BUNDLE_ID == "com.ferb.FERB"
    assert info["CFBundleExecutable"] == "FERBNotch" and info["CFBundlePackageType"] == "APPL"
    assert info["LSUIElement"] is True                                       # background first: no Dock icon
    assert info["NSMicrophoneUsageDescription"].strip() and info["NSSpeechRecognitionUsageDescription"].strip()
    assert not (ROOT / "macapp").exists()                                    # FERB Voice merged into the app


# The bundles macOS shows as FERB: the app and its daemon helper (Privacy lists, Activity Monitor)
BUNDLES = {"FERB.app": NOTCH_PLIST, "Helpers/FERB.app": ROOT / "ui" / "mac" / "FERBAgent" / "Info.plist"}
APP_ICON = ROOT / "assets" / "logo" / "AppIcon.icns"


def _icon_file(info: dict[str, Any]) -> str:
    name = info["CFBundleIconFile"]
    return name if name.endswith(".icns") else name + ".icns"


# decision: REL-4
def test_every_bundle_names_ferbs_icon_and_the_icon_holds_every_size(tmp_path: Path) -> None:
    """Owner, 2026-11-01: FERB's logo everywhere. Each bundle names AppIcon, made from assets/logo/ferb-logo.svg by
    scripts/make_logo.py, and the icon carries every size macOS asks for."""
    for bundle, plist in BUNDLES.items():
        assert _icon_file(plistlib.loads(plist.read_bytes())) == APP_ICON.name, bundle
    out = tmp_path / "AppIcon.iconset"
    done = subprocess.run(["iconutil", "-c", "iconset", "-o", str(out), str(APP_ICON)], capture_output=True, text=True)
    assert done.returncode == 0, done.stderr
    sizes = {f"icon_{s}x{s}{x}.png" for s in (16, 32, 128, 256, 512) for x in ("", "@2x")}
    assert {f.name for f in out.iterdir()} == sizes


def test_the_development_build_keeps_its_own_identity() -> None:
    """`ui/mac/FERBNotch/build.sh` (the owner's dev notch) must not take the packaged app's bundle id, or macOS would
    mix up the two apps' permissions."""
    script = (ROOT / "ui" / "mac" / "FERBNotch" / "build.sh").read_text()
    assert "dev.ferb.notch" in script and "FERB Notch" in script


# decision: APP-525, APP-535
def test_the_sound_family_ships_as_lossless_alac_with_one_banner_copy() -> None:
    """The committed sounds are ALAC in .caf (lossless, about half the size of the .wav renders); the banners' sound
    lives once, at the package root build-app.sh copies into Contents/Resources, never also in the Sounds bundle; the
    Google Material sounds' CC BY 4.0 credit sits beside them and in both notices the app ships."""
    notch = ROOT / "ui" / "mac" / "FERBNotch"
    sounds = notch / "Sources" / "FERBNotch" / "Resources" / "Sounds"
    files = sorted(p.name for p in sounds.iterdir())
    assert "CREDITS.md" in files and files and all(n.endswith(".caf") for n in files if n != "CREDITS.md")
    banner = notch / "ferb-notification-banner.caf"
    assert banner.is_file() and not (sounds / banner.name).exists()
    for caf in [banner, *sounds.glob("*.caf")]:
        head = caf.read_bytes()[:64]
        assert head[:4] == b"caff" and head[8:12] == b"desc" and head[28:32] == b"alac", caf.name
    script = BUILD_APP.read_text()
    assert 'cp "$NOTCH/ferb-notification-banner.caf" "$CONTENTS/Resources/ferb-notification-banner.caf"' in script
    assert ".wav" not in script
    for notices in (ROOT / "THIRD_PARTY_NOTICES.md", notch / "ThirdPartyNotices.txt"):
        assert "CC BY 4.0" in notices.read_text(), notices.name


def test_build_scripts_are_executable_zsh_that_parses() -> None:
    for script in (BUILD_APP, BUILD_DMG, ROOT / "scripts" / "signing.sh", APP_PARTS):
        assert script.is_file(), script
        assert subprocess.run(["/bin/zsh", "-n", str(script)], capture_output=True).returncode == 0, script
    assert os.access(BUILD_APP, os.X_OK) and os.access(BUILD_DMG, os.X_OK)


def _parts(code: str) -> subprocess.CompletedProcess[str]:
    """zsh code run after sourcing scripts/app-parts.sh, in a clean zsh with the system PATH."""
    return subprocess.run(["/bin/zsh", "-f", "-c", f"source {shlex.quote(str(APP_PARTS))}; {code}"],
                          capture_output=True, text=True, env={"PATH": "/usr/bin:/bin:/usr/sbin:/sbin"})


def _sized(path: Path, size: int) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"\0" * size)
    return path


def test_size_report_lists_ferb_apps_parts_largest_first(tmp_path: Path) -> None:
    app = tmp_path / "FERB.app"
    c = app / "Contents"
    site = c / "Resources/python/lib/python3.12/site-packages"
    _sized(c / "Resources/node/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex", 3_000_000)
    _sized(site / "claude_agent_sdk/_bundled/claude", 2_000_000)
    _sized(site / "claude_agent_sdk/__init__.py", 100_000)
    _sized(site / "regex/_regex.cpython-312-darwin.so", 400_000)
    _sized(c / "Resources/python/lib/python3.12/json/__init__.py", 300_000)
    _sized(c / "Resources/python/lib/libpython3.12.dylib", 700_000)
    _sized(c / "Helpers/ferb-secrets", 100_000)
    _sized(c / "Info.plist", 1_000)
    notch = compile_macho(c / "MacOS/FERBNotch", "arm64").stat().st_size
    done = _parts(f"ferb_size_report {shlex.quote(str(app))}")
    assert done.returncode == 0, done.stderr
    lines = done.stdout.splitlines()
    total = 3_000_000 + 2_000_000 + 100_000 + 400_000 + 300_000 + 700_000 + 100_000 + 1_000 + notch
    assert lines[0] == f"FERB.app (arm64): {total / 1e6:.1f} MB"
    rows = [line.split(" MB  ", 1) for line in lines[1:]]
    named = {name: float(mb) for mb, name in rows}
    assert named == {"Codex (Resources/node)": 3.0, "Claude Code (claude_agent_sdk/_bundled)": 2.0,
                     "Python packages (site-packages without Claude Code)": 0.5, "Python standard library": 0.3,
                     "libpython": 0.7, "Helpers (agent, secrets)": 0.1,
                     "Notch app (MacOS)": round(notch / 1e6, 1), "Everything else": 0.0}
    sizes = [float(mb) for mb, _ in rows]
    assert sizes == sorted(sizes, reverse=True)


def test_the_build_and_the_daemon_name_the_same_codex_binary_for_each_architecture() -> None:
    from ferb.runtimes.codex_exec import native_bin
    for arch in ("arm64", "x86_64"):
        done = _parts(f"ferb_arch_names {arch} && print -r -- $FERB_CODEX_BIN")
        assert done.stdout.strip() == f"Resources/node/{native_bin(arch)}", done.stderr
    assert _parts("ferb_arch_names ppc").returncode != 0


def test_trim_removes_only_what_ferb_never_runs_and_keeps_one_architecture(tmp_path: Path) -> None:
    from ferb.runtimes.codex_exec import native_bin
    for arch in ("arm64", "x86_64"):
        app = tmp_path / arch / "FERB.app"
        c = app / "Contents"
        codex = c / "Resources/node" / native_bin(arch)
        vendor = codex.parent.parent
        std = c / "Resources/python/lib/python3.12"
        site = std / "site-packages"
        gone = [vendor / "bin/codex-code-mode-host", vendor / "codex-resources/voice/bin/codex-voice-host",
                vendor / "codex-resources/voice/lib/libglib-2.0.0.dylib", vendor / "codex-resources/zsh/bin/zsh",
                std / "lib2to3/__init__.py", std / "pydoc_data/topics.py", std / "config-3.12-darwin/python.o",
                std / "turtle.py", std / "__pycache__/turtle.cpython-312.pyc", site / "regex/tests/test_regex.py",
                site / "jsonschema/benchmarks/issue232.py", site / "claude_agent_sdk/testing/__init__.py",
                site / "objc/_objc.cpython-312-darwin.so.dSYM/Contents/Info.plist"]
        kept = [codex, vendor / "codex-path/rg", vendor / "codex-package.json", std / "json/__init__.py",
                std / "unittest/__init__.py", site / "regex/__init__.py", site / "claude_agent_sdk/_bundled/claude",
                site / "PyObjCTools/TestSupport.py", site / "cryptography-50.0.1.dist-info/licenses/LICENSE",
                site / "jsonschema/protocols.py"]
        for f in gone + kept:
            _sized(f, 10)
        fat = compile_macho(site / "objc/_objc.cpython-312-darwin.so", "arm64", "x86_64")
        done = _parts(f"ferb_trim_app {shlex.quote(str(app))} {arch}")
        assert done.returncode == 0, done.stderr
        assert [str(f.relative_to(app)) for f in gone if f.exists()] == [], arch
        assert [str(f.relative_to(app)) for f in kept if not f.exists()] == [], arch
        archs = subprocess.run(["lipo", "-archs", str(fat)], capture_output=True, text=True).stdout.split()
        assert archs == [arch]                                    # a universal module keeps only this app's slice


def test_arch_check_names_every_file_built_for_another_architecture(tmp_path: Path) -> None:
    app = tmp_path / "FERB.app"
    c = app / "Contents"
    compile_macho(c / "MacOS/FERBNotch", "arm64")
    compile_macho(c / "Resources/python/lib/libpython3.12.dylib", "arm64")
    launcher = _sized(c / "Resources/bin/ferb", 10)
    launcher.chmod(0o755)                                                     # an executable script: not Mach-O
    assert _parts(f"ferb_check_arch {shlex.quote(str(app))} arm64").returncode == 0
    compile_macho(c / "Helpers/ferb-secrets", "x86_64")
    compile_macho(c / "Resources/python/lib/python3.12/site-packages/objc/_objc.cpython-312-darwin.so",
                  "arm64", "x86_64")
    done = _parts(f"ferb_check_arch {shlex.quote(str(app))} arm64")
    assert done.returncode != 0
    assert "Contents/Helpers/ferb-secrets: x86_64" in done.stderr
    assert "_objc.cpython-312-darwin.so: " in done.stderr and "MacOS/FERBNotch" not in done.stderr


def test_cli_reports_its_version() -> None:
    out = subprocess.run([sys.executable, "-I", "-m", "ferb.cli", "--version"], capture_output=True, text=True,
                         cwd=ROOT)
    assert out.returncode == 0 and out.stdout.strip() == f"ferb {ferb.__version__}"


# ---------- the signing step, against stub tools ----------

STUB_CODESIGN = r"""#!/bin/zsh
# Logs every call. -dv: a "vendor" path carries another team's Developer ID signature, anything else is ad-hoc.
print -r -- "codesign|${(j:|:)@}" >> "$STUB_LOG"
if [[ " $* " == *" --verify "* && -n "${STUB_VERIFY_FAILS:-}" ]]; then exit 1; fi
if [[ " $* " == *" -dv "* ]]; then
  if [[ "${@[-1]}" == *vendor* ]]; then
    print -u2 "Authority=Developer ID Application: Vendor Inc (VENDOR1)"; print -u2 "TeamIdentifier=VENDOR1"
  else
    print -u2 "Signature=adhoc"
  fi
  exit 0
fi
for (( i = 1; i < $#; i++ )); do
  if [[ "${@[i]}" == --entitlements ]]; then cp "${@[i+1]}" "$STUB_DIR/ent${${@[-1]}//\//_}"; fi
done
exit 0
"""
STUB_SECURITY = r"""#!/bin/zsh
# find-identity: the code-signing identities the test gives (hash "name" per line)
print -r -- "${STUB_IDENTITIES:-}"
print "     0 valid identities found"
"""
STUB_XCRUN = r"""#!/bin/zsh
print -r -- "xcrun|${(j:|:)@}" >> "$STUB_LOG"
if [[ "$1" == notarytool && "$2" == submit ]]; then print '{"id":"0f00","status":"Accepted","message":"ok"}'; fi
exit 0
"""
STUB_HDIUTIL = r"""#!/bin/zsh
print -r -- "hdiutil|${(j:|:)@}" >> "$STUB_LOG"
if [[ "$1" == create ]]; then
  for (( i = 1; i < $#; i++ )); do [[ "${@[i]}" == -srcfolder ]] && ls -lA "${@[i+1]}" >> "$STUB_DIR/staging.txt"; done
  print "fake dmg" > "${@[-1]}"
fi
for (( i = 1; i < $#; i++ )); do
  [[ "$1" == attach && "${@[i]}" == -mountpoint ]] && mkdir -p "${@[i+1]}"
  [[ "$1" == convert && "${@[i]}" == -o ]] && cp "$2" "${@[i+1]}"
done
if [[ "$1" == detach ]]; then xattr -px com.apple.FinderInfo "$2" | tr -d ' \n' >> "$STUB_DIR/volume-flags.txt"; rmdir "$2"; fi
exit 0
"""


def _stubs(tmp_path: Path) -> dict[str, str]:
    stubs = tmp_path / "stubs"
    stubs.mkdir()
    for name, body in (("codesign", STUB_CODESIGN), ("xcrun", STUB_XCRUN), ("hdiutil", STUB_HDIUTIL),
                       ("security", STUB_SECURITY)):
        (stubs / name).write_text(body)
        (stubs / name).chmod(0o755)
    return {"PATH": f"{stubs}:/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(tmp_path / "home"),
            "STUB_LOG": str(tmp_path / "calls.log"), "STUB_DIR": str(tmp_path), "FERB_DEV_DMG": "1"}


def _fake_app(root: Path) -> Path:
    """An assembled FERB.app shaped like build-app.sh's output, with real Mach-O files (copies of /usr/bin/true)."""
    app = root / "FERB.app"
    c = app / "Contents"
    machos = ["MacOS/FERBNotch", "Resources/python/bin/python3.12", "Resources/python/lib/libpython3.12.dylib",
              "Resources/python/lib/python3.12/site-packages/pkg/_ext.cpython-312-darwin.so",
              "Resources/python/lib/python3.12/site-packages/vendor_sdk/_bundled/vendor-tool",
              "Helpers/ferb-secrets", "Helpers/FERB.app/Contents/MacOS/FERB"]
    for rel in machos:
        (c / rel).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy("/usr/bin/true", c / rel)
    (c / "Resources/python/lib/python3.12/site-packages/pkg/mod.py").write_text("x = 1\n")
    (c / "Resources/bin").mkdir(parents=True)
    (c / "Resources/bin/ferb").write_text("#!/bin/zsh\nexit 0\n")
    (c / "Resources/bin/ferb").chmod(0o755)
    (c / "Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "com.ferb.FERB",
                                                    "CFBundleShortVersionString": "0.27.0",
                                                    "CFBundleVersion": "202610140201"}))
    (c / "Helpers/FERB.app/Contents/Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "com.ferb.daemon"}))
    return app


def _ent(tmp_path: Path, signed: Path) -> str:
    """The entitlements the stub codesign was given for `signed`."""
    return (tmp_path / ("ent" + str(signed).replace("/", "_"))).read_text()


def _signed(calls: list[str]) -> list[str]:
    """The paths signed, in order (codesign calls that write a signature)."""
    return [c.split("|")[-1] for c in calls if c.startswith("codesign") and "|--sign|" in c and "--verify" not in c]


def _sign_only(tmp_path: Path, **env: str) -> tuple[Path, list[str], subprocess.CompletedProcess[str]]:
    app = _fake_app(tmp_path / "out")
    run_env = {**_stubs(tmp_path), **env}
    done = subprocess.run([str(BUILD_APP), "--sign-only", str(app)], capture_output=True, text=True, env=run_env)
    log = tmp_path / "calls.log"
    return app, (log.read_text().splitlines() if log.exists() else []), done


def test_ad_hoc_is_the_default_and_signs_every_nested_binary_inside_out(tmp_path: Path) -> None:
    app, calls, done = _sign_only(tmp_path)
    assert done.returncode == 0, done.stderr + done.stdout
    signed = _signed(calls)
    c = app / "Contents"
    python = str(c / "Resources/python/bin/python3.12")
    for path in (python, str(c / "Resources/python/lib/libpython3.12.dylib"), str(c / "Helpers/ferb-secrets"),
                 str(c / "Resources/python/lib/python3.12/site-packages/pkg/_ext.cpython-312-darwin.so"),
                 str(c / "Helpers/FERB.app"), str(app)):
        assert path in signed, (path, signed)
    assert signed[-1] == str(app)                                             # the app last, after everything in it
    assert not any("vendor-tool" in s or s.endswith((".py", "/ferb", "/MacOS/FERB")) for s in signed)
    sign_calls = [c for c in calls if "|--sign|" in c]
    assert all("|--sign|-|" in c and "|--options|runtime|" in c and "|--timestamp=none|" in c for c in sign_calls)
    assert any("|--identifier|com.ferb.secrets|" in c and c.endswith("/Helpers/ferb-secrets") for c in calls)
    assert any("|--identifier|com.ferb.python|" in c and c.endswith(python) for c in calls)
    agent = _ent(tmp_path, c / "Helpers/FERB.app")             # the daemon runs as the agent app FERB
    assert "com.apple.security.automation.apple-events" in agent
    assert "com.apple.security.cs.disable-library-validation" in agent       # ad-hoc code has no Team ID
    assert "com.apple.security.cs.disable-library-validation" in _ent(tmp_path, Path(python))
    assert "apple-events" not in _ent(tmp_path, Path(python))                # the plain interpreter sends none
    assert "com.apple.security.device.audio-input" in _ent(tmp_path, app)
    assert not any(s.endswith("/Helpers/FERB.app/Contents/MacOS/FERB") for s in signed)   # signed with its app
    assert any(c.startswith("codesign|--verify|--deep|--strict") and c.endswith(str(app)) for c in calls)
    assert not any(c.startswith("xcrun") for c in calls)                    # no notarization without a Developer ID


def test_the_bundle_has_no_voice_helper(tmp_path: Path) -> None:
    """Hearing is FERB.app's own modes: nothing is built, listed, signed or pinned as a nested voice app."""
    app, calls, done = _sign_only(tmp_path)
    assert done.returncode == 0, done.stderr + done.stdout
    assert not (app / "Contents/Helpers/FERB Voice.app").exists()
    assert not any("Voice" in piece for piece in _signed(calls))                # no piece signed, no entitlements file
    for script in ("build-app.sh", "signing.sh", "app-parts.sh"):
        text = (ROOT / "scripts" / script).read_text()
        assert "FERB Voice" not in text and "macapp" not in text, script
    assert not any(ident == "com.ferb.app" for _, ident in RELEASE_PIECES)
    stray = _fake_app(tmp_path / "stray")                                       # an unknown nested app isn't signed
    (stray / "Contents/Helpers/Other.app/Contents").mkdir(parents=True)
    (stray / "Contents/Helpers/Other.app/Contents/Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "x"}))
    (tmp_path / "stray-run").mkdir()
    refused = subprocess.run([str(BUILD_APP), "--sign-only", str(stray)], capture_output=True, text=True,
                             env=_stubs(tmp_path / "stray-run"))
    assert refused.returncode != 0 and "no piece of FERB.app" in refused.stderr


def test_developer_id_signs_with_a_timestamp_then_notarizes_and_staples_without_rebuilding(tmp_path: Path) -> None:
    app, calls, done = _sign_only(tmp_path, SIGN_IDENTITY=DEV_ID, NOTARY_KEYCHAIN_PROFILE="ferb-notary")
    assert done.returncode == 0, done.stderr + done.stdout
    sign_calls = [c for c in calls if "|--sign|" in c]
    assert sign_calls and all(f"|--sign|{DEV_ID}|" in c and "|--timestamp|" in c for c in sign_calls)
    assert "disable-library-validation" not in _ent(tmp_path, app / "Contents/Helpers/FERB.app")   # one team signs all
    assert "apple-events" in _ent(tmp_path, app / "Contents/Helpers/FERB.app")
    submit = next(c for c in calls if c.startswith("xcrun|notarytool|submit"))
    assert "|--wait|" in submit and submit.endswith("|--keychain-profile|ferb-notary")
    assert f"xcrun|stapler|staple|{app}" in calls
    assert calls.index(submit) > max(calls.index(c) for c in sign_calls)    # notarized only once fully signed


def test_a_developer_id_named_by_its_certificate_hash_is_still_notarized(tmp_path: Path) -> None:
    sha1 = "0123456789ABCDEF0123456789ABCDEF01234567"
    listed = f'  1) {sha1} "{DEV_ID}"'
    _, calls, done = _sign_only(tmp_path, SIGN_IDENTITY=sha1.lower(), STUB_IDENTITIES=listed,
                                NOTARY_KEYCHAIN_PROFILE="ferb-notary")
    assert done.returncode == 0, done.stderr + done.stdout
    assert all("|--timestamp|" in c for c in calls if "|--sign|" in c)
    assert any(c.startswith("xcrun|notarytool|submit") for c in calls)


def test_another_certificate_is_signed_but_never_notarized(tmp_path: Path) -> None:
    sha1 = "89ABCDEF0123456789ABCDEF0123456789ABCDEF"
    listed = f'  1) {sha1} "Apple Development: Test Owner (TEAM123456)"'
    app, calls, done = _sign_only(tmp_path, SIGN_IDENTITY=sha1, STUB_IDENTITIES=listed)
    assert done.returncode == 0, done.stderr + done.stdout
    assert all("|--timestamp=none|" in c for c in calls if "|--sign|" in c)
    assert not any(c.startswith("xcrun") for c in calls)
    assert "disable-library-validation" in _ent(tmp_path, app / "Contents/Helpers/FERB.app")   # no Apple Team ID


def test_developer_id_accepts_an_api_key_from_the_environment(tmp_path: Path) -> None:
    key = tmp_path / "AuthKey_ABC.p8"
    key.write_text("-----BEGIN PRIVATE KEY-----\nnot a real key\n-----END PRIVATE KEY-----\n")
    _, calls, done = _sign_only(tmp_path, SIGN_IDENTITY=DEV_ID, NOTARY_KEY=str(key), NOTARY_KEY_ID="ABC123DEFG",
                                NOTARY_ISSUER="00000000-0000-0000-0000-000000000000")
    assert done.returncode == 0, done.stderr + done.stdout
    submit = next(c for c in calls if c.startswith("xcrun|notarytool|submit"))
    assert submit.endswith(f"|--key|{key}|--key-id|ABC123DEFG|--issuer|00000000-0000-0000-0000-000000000000")


def test_developer_id_without_notary_credentials_stops_before_signing(tmp_path: Path) -> None:
    _, calls, done = _sign_only(tmp_path, SIGN_IDENTITY=DEV_ID)
    assert done.returncode != 0 and "NOTARY_KEYCHAIN_PROFILE" in done.stderr
    assert not _signed(calls)


SHA = "0123456789abcdef0123456789abcdef01234567"
RELEASE_LISTED = f'  1) {SHA.upper()} "FERB Release Signing" (CSSMERR_TP_NOT_TRUSTED)'
RELEASE_PIECES = (("", "com.ferb.FERB"), ("Contents/Helpers/FERB.app", "com.ferb.daemon"),
                  ("Contents/Resources/python/bin/python3.12", "com.ferb.python"),
                  ("Contents/Helpers/ferb-secrets", "com.ferb.secrets"))


def _release_env() -> dict[str, str]:
    """SIGN_IDENTITY=release with the stub keychain listing FERB's release certificate (self-made: never trusted)."""
    return {"SIGN_IDENTITY": "release", "FERB_TEST_RELEASE_SHA1": SHA, "STUB_IDENTITIES": RELEASE_LISTED}


def test_release_signing_uses_only_the_pinned_certificate_and_pins_it_in_each_requirement(tmp_path: Path) -> None:
    app, calls, done = _sign_only(tmp_path, **_release_env())
    assert done.returncode == 0, done.stderr + done.stdout
    sign_calls = [c for c in calls if "|--sign|" in c and "--verify" not in c]
    assert sign_calls and all(f"|--sign|{SHA.upper()}|" in c and "|--timestamp=none|" in c for c in sign_calls)
    for rel, ident in RELEASE_PIECES:
        piece = app / rel if rel else app
        call = next(c for c in sign_calls if c.endswith(f"|{piece}"))
        assert f'|--requirements|=designated => identifier "{ident}" and certificate leaf = H"{SHA}"|' in call, call
        assert f'codesign|--verify|--strict|-R=identifier "{ident}" and certificate leaf = H"{SHA}"|{piece}' in calls
    lib = next(c for c in sign_calls if c.endswith("libpython3.12.dylib"))
    assert "--requirements" not in lib                                  # libraries keep codesign's own requirement
    assert not any("vendor-tool" in c for c in sign_calls)              # another team's signature is kept
    assert "com.apple.security.cs.disable-library-validation" in _ent(tmp_path, app / "Contents/Helpers/FERB.app")
    assert not any(c.startswith("xcrun") for c in calls)                # a self-made certificate is never notarized


def test_release_signing_refuses_a_missing_or_other_certificate_before_signing_anything(tmp_path: Path) -> None:
    cases = ({**_release_env(), "STUB_IDENTITIES": ""},                                        # not in the keychain
             {**_release_env(), "STUB_IDENTITIES": f'  1) {"F" * 40} "FERB Release Signing"'},  # same name, other one
             {**_release_env(), "FERB_TEST_RELEASE_SHA1": ""},                                  # no recorded SHA-1
             {**_release_env(), "FERB_TEST_RELEASE_SHA1": "not-a-sha1"})
    for i, env in enumerate(cases):
        (tmp_path / str(i)).mkdir()
        _, calls, done = _sign_only(tmp_path / str(i), **env)
        assert done.returncode != 0 and "FERB Release Signing" in done.stderr, (i, done.stderr)
        assert not _signed(calls), i


def test_release_mode_stays_release_for_the_build_scripts_it_starts(tmp_path: Path) -> None:
    """build-dmg.sh starts build-app.sh: SIGN_IDENTITY must still say `release` there, not a bare hash that would skip
    the pinned requirements; codesign gets the hash through FERB_SIGN_ID."""
    env = {**_stubs(tmp_path), **_release_env(), "ROOT": str(ROOT)}
    done = subprocess.run(["/bin/zsh", "-f", "-c",
                           'source "$ROOT/scripts/signing.sh" && print -r -- "$SIGN_IDENTITY $FERB_SIGN_ID"'],
                          capture_output=True, text=True, env=env)
    assert done.stdout.strip() == f"release {SHA.upper()}", done.stderr


def test_build_app_refuses_an_unknown_architecture_and_intel_without_rosetta(tmp_path: Path) -> None:
    env = _stubs(tmp_path)
    out = tmp_path / "out"
    bad = subprocess.run([str(BUILD_APP), "--arch", "ppc", str(out)], capture_output=True, text=True, env=env)
    assert bad.returncode != 0 and "arm64 or x86_64" in bad.stderr
    alone = subprocess.run([str(BUILD_APP), "--arch"], capture_output=True, text=True, env=env)
    assert alone.returncode != 0 and "usage" in alone.stderr
    stubs = tmp_path / "stubs"
    (stubs / "uname").write_text('#!/bin/zsh\nif [[ "$1" == -m ]]; then print arm64; else /usr/bin/uname "$@"; fi\n')
    (stubs / "arch").write_text("#!/bin/zsh\nexit 1\n")                       # Rosetta isn't installed
    for name in ("uname", "arch"):
        (stubs / name).chmod(0o755)
    intel = subprocess.run([str(BUILD_APP), "--arch", "x86_64", str(out)], capture_output=True, text=True, env=env)
    assert intel.returncode != 0 and "Rosetta" in intel.stderr
    assert not out.exists()                                                   # refused before anything was written


# decision: REL-4
def test_dmg_holds_the_app_and_an_applications_link_and_is_notarized_only_with_a_developer_id(tmp_path: Path) -> None:
    app = _fake_app(tmp_path / "out")
    env = _stubs(tmp_path)
    dmg = tmp_path / "out" / "FERB.dmg"
    done = subprocess.run([str(BUILD_DMG), str(app), str(dmg)], capture_output=True, text=True, env=env)
    assert done.returncode == 0, done.stderr + done.stdout
    staging = (tmp_path / "staging.txt").read_text()
    assert "Applications -> /Applications" in staging and "FERB.app" in staging
    calls = (tmp_path / "calls.log").read_text().splitlines()
    assert any(c.startswith("hdiutil|create|") and "|-volname|FERB|" in c for c in calls)
    assert any(c.startswith("hdiutil|convert|") and "|-format|ULMO|" in c and c.endswith(str(dmg)) for c in calls)
    assert not any(c.startswith(("codesign", "xcrun")) for c in calls) and dmg.is_file()
    # The volume shows FERB's logo: the icon is in the image and its root carries Finder's custom-icon flag
    assert ".VolumeIcon.icns" in staging
    assert (tmp_path / "volume-flags.txt").read_text()[16:18] == "04"
    assert sorted(p.name for p in dmg.parent.iterdir()) == ["FERB.app", "FERB.dmg"]   # nothing left behind

    (tmp_path / "calls.log").unlink()
    (tmp_path / "staging.txt").unlink()
    dmg.unlink()
    env.update(SIGN_IDENTITY=DEV_ID, NOTARY_KEYCHAIN_PROFILE="ferb-notary")
    done = subprocess.run([str(BUILD_DMG), str(app), str(dmg)], capture_output=True, text=True, env=env)
    assert done.returncode == 0, done.stderr + done.stdout
    calls = (tmp_path / "calls.log").read_text().splitlines()
    assert f"codesign|--force|--sign|{DEV_ID}|--timestamp|{dmg}" in calls
    assert any(c.startswith(f"xcrun|notarytool|submit|{dmg}|") for c in calls)
    assert f"xcrun|stapler|staple|{dmg}" in calls
    for f in (tmp_path / "out").rglob("*"):                                  # credentials are never written anywhere
        if f.is_file():
            assert b"ferb-notary" not in f.read_bytes(), f


FIRST_OPEN = "First open — System Settings → Privacy & Security → Open Anyway.txt"


def test_dmg_is_named_for_the_apps_architecture_and_says_how_to_open_it_the_first_time(tmp_path: Path) -> None:
    env = {**_stubs(tmp_path), "FERB_DMG_DIR": str(tmp_path / "dist")}
    for arch, name in (("arm64", "FERB-AppleSilicon.dmg"), ("x86_64", "FERB-Intel.dmg")):
        app = _fake_app(tmp_path / arch)
        compile_macho(app / "Contents/MacOS/FERBNotch", arch)
        done = subprocess.run([str(BUILD_DMG), str(app)], capture_output=True, text=True, env=env)
        assert done.returncode == 0, done.stderr + done.stdout
        assert (tmp_path / "dist" / name).is_file(), name
    staging = (tmp_path / "staging.txt").read_text()
    assert FIRST_OPEN in staging and "Applications -> /Applications" in staging and "FERB.app" in staging
    universal = _fake_app(tmp_path / "fat")              # copies of /usr/bin/true: x86_64 and arm64e in one file
    done = subprocess.run([str(BUILD_DMG), str(universal)], capture_output=True, text=True, env=env)
    assert done.returncode != 0 and "one-architecture" in done.stderr
    assert sorted(p.name for p in (tmp_path / "dist").iterdir()) == ["FERB-AppleSilicon.dmg", "FERB-Intel.dmg"]


def test_a_disk_image_holds_only_a_release_signed_app(tmp_path: Path) -> None:
    """What people download carries FERB's release certificate (L34), so their permissions and automatic updates carry
    over to the next version; an ad-hoc or otherwise signed app is refused unless FERB_DEV_DMG=1 (test images)."""
    app = _fake_app(tmp_path / "out")
    compile_macho(app / "Contents/MacOS/FERBNotch", "arm64")
    dist = tmp_path / "dist"
    dmg = dist / "FERB-AppleSilicon.dmg"
    env = {**_stubs(tmp_path), "FERB_DMG_DIR": str(dist)}
    del env["FERB_DEV_DMG"]
    adhoc = subprocess.run([str(BUILD_DMG), str(app)], capture_output=True, text=True, env=env)
    assert adhoc.returncode != 0 and "SIGN_IDENTITY=release" in adhoc.stderr and not dmg.exists()
    env.update(_release_env())
    ok = subprocess.run([str(BUILD_DMG), str(app)], capture_output=True, text=True, env=env)
    assert ok.returncode == 0, ok.stderr + ok.stdout
    calls = (tmp_path / "calls.log").read_text().splitlines()
    assert f'codesign|--verify|--strict|-R=identifier "com.ferb.FERB" and certificate leaf = H"{SHA}"|{app}' in calls
    dmg.unlink()
    env["STUB_VERIFY_FAILS"] = "1"                                          # the app is signed some other way
    other = subprocess.run([str(BUILD_DMG), str(app)], capture_output=True, text=True, env=env)
    assert other.returncode != 0 and "isn't signed with FERB's release certificate" in other.stderr
    assert not dmg.exists()


def test_the_release_certificates_sha1_is_recorded() -> None:
    """scripts/release-cert.sha1 names FERB's one release certificate (L34): public, 40 lowercase hex digits. It exists
    once the owner has created the certificate (phase B task 4); until then the test says so."""
    path = ROOT / "scripts" / "release-cert.sha1"
    if not path.exists():
        pytest.skip("the owner hasn't created FERB Release Signing yet (steps at the top of scripts/signing.sh)")
    text = path.read_text()
    assert len(text) == 41 and text.endswith("\n") and all(ch in "0123456789abcdef" for ch in text[:40])


# ---------- a real bundle ----------

def _real_bundle(tmp_path_factory: pytest.TempPathFactory) -> Path:
    given = os.environ.get("FERB_APP_BUNDLE")
    if given:
        return Path(given)
    if os.environ.get("FERB_BUILD_APP") != "1":
        pytest.skip("set FERB_APP_BUNDLE=/path/FERB.app to check a built app, or FERB_BUILD_APP=1 to build one "
                    "(minutes, ~700 MB; needs swift, uv and npm)")
    missing = [t for t in ("swift", "uv", "npm", "codesign") if shutil.which(t) is None]
    if missing:
        pytest.skip(f"can't build FERB.app here: {', '.join(missing)} not found")
    out = tmp_path_factory.mktemp("ferb-app")
    subprocess.run([str(BUILD_APP), str(out)], check=True)
    return out / "FERB.app"


@pytest.fixture(scope="module")
def real_bundle(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """One built FERB.app for every real-bundle test (FERB_APP_BUNDLE, or FERB_BUILD_APP=1 builds it once)."""
    return _real_bundle(tmp_path_factory)


def _bundle_arch(app: Path) -> str:
    return subprocess.run(["lipo", "-archs", str(app / "Contents/MacOS/FERBNotch")], capture_output=True,
                          text=True).stdout.strip()


def test_real_bundle_structure_signature_and_embedded_cli(real_bundle: Path, tmp_path: Path) -> None:
    from ferb.runtimes.codex_exec import native_bin
    app = real_bundle
    c = app / "Contents"
    info = plistlib.loads((c / "Info.plist").read_bytes())
    assert info["CFBundleIdentifier"] == APP_BUNDLE_ID and info["CFBundleShortVersionString"]
    assert info["CFBundleVersion"].isdigit()
    for rel in ("MacOS/FERBNotch", "Resources/python/bin/python3.12", "Resources/bin/ferb", "Helpers/ferb-secrets",
                "Helpers/FERB.app/Contents/MacOS/FERB",
                "Resources/node/" + str(native_bin(_bundle_arch(app))),
                # the notch's SwiftPM resources, where Bundle.module looks in an app: FERB's mark and the sound family
                "Resources/FERBNotch_FERBNotch.bundle/Contents/Resources/ferb-mark.svg",
                "Resources/FERBNotch_FERBNotch.bundle/Contents/Resources/Sounds/ferb-logo.caf",
                "Resources/FERBNotch_FERBNotch.bundle/Contents/Resources/Sounds/ferb-room-ir.caf",
                "Resources/FERBNotch_FERBNotch.bundle/Contents/Resources/Sounds/CREDITS.md",
                # the banners' notification sound, where UNNotificationSound(named:) looks: the app's own Resources
                "Resources/ferb-notification-banner.caf"):
        assert (c / rel).is_file(), rel
    sounds = c / "Resources/FERBNotch_FERBNotch.bundle/Contents/Resources/Sounds"
    assert not list(sounds.glob("*.wav")) and not (sounds / "ferb-notification-banner.caf").exists()   # one copy
    assert not (c / "Helpers/FERB Voice.app").exists() and info["NSSpeechRecognitionUsageDescription"]
    agent = plistlib.loads((c / "Helpers/FERB.app/Contents/Info.plist").read_bytes())
    assert (agent["CFBundleIdentifier"], agent["CFBundleName"]) == ("com.ferb.daemon", "FERB")   # what macOS shows
    assert agent["LSBackgroundOnly"] is True and agent["NSAppleEventsUsageDescription"]
    for bundle in BUNDLES:                                                   # each shows FERB's logo, not a blank app
        root = app if bundle == "FERB.app" else c / bundle
        icon = root / "Contents/Resources" / _icon_file(plistlib.loads((root / "Contents/Info.plist").read_bytes()))
        assert icon.read_bytes() == APP_ICON.read_bytes(), bundle
    assert not list((c / "Resources/python/lib").glob("python3.12/EXTERNALLY-MANAGED"))
    verify = subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], capture_output=True, text=True)
    assert verify.returncode == 0, verify.stderr
    env = {"HOME": str(tmp_path), "FERB_HOME": str(tmp_path / ".ferb"), "PATH": "/usr/bin:/bin"}
    out = subprocess.run([str(c / "Resources/bin/ferb"), "--version"], capture_output=True, text=True, env=env)
    assert out.returncode == 0 and out.stdout.strip() == f"ferb {ferb.__version__}", out.stderr   # built from here
    probe = ("import sys, AppKit, Quartz, ScreenCaptureKit, Vision, ferb.daemon, ferb.device.install as i; "
             "print(i.app_bundle())")
    seen = subprocess.run([str(c / "Resources/python/bin/python3.12"), "-I", "-B", "-c", probe], capture_output=True,
                          text=True, env=env)
    assert seen.returncode == 0 and seen.stdout.strip() == str(app.resolve()), seen.stderr
    stub = c / "Helpers/FERB.app/Contents/MacOS/FERB"                        # the daemon's executable
    for argv in (["-c", "print('ran')"], ["-m", "os"], [], ["-m", "ferb.daemon", "x"],   # only FERB's entry points
                 ["-I", "-m", "ferb.device.detector", str(tmp_path / "e.jsonl")],
                 ["-I", "-m", "ferb.memory.card_reader", "/etc/passwd"]):
        refused = subprocess.run([str(stub), *argv], capture_output=True, text=True, env=env)
        assert refused.returncode == 64 and "ran" not in refused.stdout, argv
    foreign = tmp_path / "foreign-ferb"
    foreign.mkdir()
    refused = subprocess.run([str(stub), "-m", "ferb.daemon"], capture_output=True, text=True,
                             env={**env, "FERB_HOME": str(foreign)})
    assert refused.returncode == 64 and "FERB_HOME" in refused.stderr          # never another FERB home
    # The shipped agent is built from ui/mac/FERBAgent/main.c, whose clean environment and real sys.executable the
    # agent_stub tests above check; running its allowed entry points here would reach the owner's real ~/.ferb.
    from ferb.act.shell.local import WATCH                                  # L24's kernel-report stream watcher
    from ferb.paths import helper_python
    helper = helper_python(c / "Resources" / "python")
    assert Path(helper[0]).resolve() == (c / "Resources/python/bin/python3.12").resolve()   # never the agent
    watcher = subprocess.Popen([*helper, "-I", "-c", WATCH, "/usr/bin/true"], stdin=subprocess.PIPE,
                               stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, env=env)
    try:
        assert watcher.wait(timeout=30) == 0                                # runs FERB's watcher code in the app
    finally:
        assert watcher.stdin is not None
        watcher.stdin.close()
    built = subprocess.run([str(c / "Resources/python/bin/python3.12"), "-I", "-c",
                            "import platform; print(platform.python_version())"], capture_output=True, text=True, env=env)
    assert built.stdout.strip() == "3.12.13"                                 # the pinned CPython
    # No build-machine path anywhere in what ships: the checkout, the build folder, the builder's home
    marks = [p.encode() for p in {str(ROOT), str(app.parent), os.path.realpath(app.parent), str(Path.home())}]
    leaks = [str(f.relative_to(app)) for f in app.rglob("*") if f.is_file() and not f.is_symlink()
             and any(m in f.read_bytes() for m in marks)]
    assert leaks == [], leaks
    again = subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], capture_output=True, text=True)
    assert again.returncode == 0, "running FERB changed its own bundle: " + again.stderr


# The owner's size target in MB of 10^6 bytes (L34 answer A2: 600 / 630; raised 2026-11-01 after the runtime upgrades,
# owner: "yeah ofc raise the cap"; the step-13 arm64 build measured 705.6 MB)
MAX_APP_MB = {"arm64": 720, "x86_64": 750}


# decision: REL-401
def test_real_bundle_holds_only_what_ferb_runs(real_bundle: Path, tmp_path: Path) -> None:
    from ferb.runtimes.codex_exec import native_bin
    app = real_bundle
    c = app / "Contents"
    arch = _bundle_arch(app)
    assert arch in MAX_APP_MB, arch                                          # one architecture per app
    codex = c / "Resources/node" / native_bin(arch)
    vendor = codex.parent.parent
    for rel in ("bin/codex-code-mode-host", "codex-resources/voice", "codex-resources/zsh"):
        assert not (vendor / rel).exists(), rel
    std = c / "Resources/python/lib/python3.12"
    for rel in ("lib2to3", "pydoc_data", "config-3.12-darwin", "turtle.py", "site-packages/claude_agent_sdk/testing",
                "site-packages/jsonschema/benchmarks"):
        assert not (std / rel).exists(), rel
    assert not list(std.glob("site-packages/**/*.dSYM")) and not list(std.glob("site-packages/*/tests"))
    assert (std / "site-packages/claude_agent_sdk/_bundled/claude").is_file()   # Claude Code stays (owner, A1)
    env = {"HOME": str(tmp_path), "CODEX_HOME": str(tmp_path), "PATH": "/usr/bin:/bin"}
    models = subprocess.run([str(codex), "debug", "models", "--bundled"], capture_output=True, text=True, env=env,
                            cwd=tmp_path)
    assert models.returncode == 0 and json.loads(models.stdout)["models"], models.stderr
    probe = "import platform, claude_agent_sdk, cryptography, jsonschema, regex, referencing; print(platform.machine())"
    seen = subprocess.run([str(c / "Resources/python/bin/python3.12"), "-I", "-B", "-c", probe], capture_output=True,
                          text=True, env={**env, "FERB_HOME": str(tmp_path / ".ferb")})
    assert seen.returncode == 0 and seen.stdout.strip() == arch, seen.stderr
    size = sum(f.stat().st_size for f in app.rglob("*") if f.is_file() and not f.is_symlink())
    assert size <= MAX_APP_MB[arch] * 1_000_000, f"{size / 1e6:.1f} MB for {arch}"
    check = _parts(f"ferb_check_arch {shlex.quote(str(app))} {arch}")
    assert check.returncode == 0, check.stderr


# ---------- the agent stub: only FERB's own entry points, only FERB's own home (fix round 2) ----------

@pytest.fixture(scope="module")
def agent_stub(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """ui/mac/FERBAgent/main.c built into a FERB.app-shaped folder whose Python is this test's own interpreter's
    standard library plus a stand-in `ferb` package that reports what it sees."""
    clang = shutil.which("clang") or shutil.which("xcrun")
    base = Path(sys.base_prefix)
    lib = base / "lib" / "libpython3.12.dylib"
    if clang is None or not lib.exists() or not (base / "include" / "python3.12").is_dir():
        pytest.skip("needs clang and a CPython 3.12 with its headers and libpython")
    app = tmp_path_factory.mktemp("stub") / "FERB.app"
    python = app / "Contents" / "Resources" / "python"
    stdlib = python / "lib" / "python3.12"
    stdlib.mkdir(parents=True)
    (python / "lib" / "libpython3.12.dylib").symlink_to(lib)
    for entry in (base / "lib" / "python3.12").iterdir():
        if entry.name != "site-packages":
            (stdlib / entry.name).symlink_to(entry)
    site = stdlib / "site-packages"
    for rel in ("ferb/__init__.py", "ferb/cli/__init__.py", "ferb/device/__init__.py", "ferb/daemon/__init__.py",
                "ferb/memory/__init__.py"):
        (site / rel).parent.mkdir(parents=True, exist_ok=True)
        (site / rel).write_text("")
    report = ("import json, os, sys\nprint(json.dumps({'module': __name__, 'argv': sys.argv[1:], "
              "'executable': sys.executable, 'isolated': sys.flags.isolated, 'ferb_home': os.environ.get('FERB_HOME'), "
              "'environ': dict(os.environ)}))\n")
    for module in ("ferb/cli/mcp.py", "ferb/device/detector.py", "ferb/daemon/__main__.py", "ferb/memory/card_reader.py",
                   "ferb/device/grant_probe.py"):
        (site / module).write_text(report)
    exe = app / "Contents" / "Helpers" / "FERB.app" / "Contents" / "MacOS" / "FERB"
    exe.parent.mkdir(parents=True)
    cmd = ["clang"] if shutil.which("clang") else ["xcrun", "clang"]
    subprocess.run([*cmd, "-O2", f"-I{base / 'include' / 'python3.12'}", str(ROOT / "ui" / "mac" / "FERBAgent" / "main.c"),
                    str(lib), f"-Wl,-rpath,{base / 'lib'}", "-o", str(exe)], check=True)
    return exe


def _stub(exe: Path, *args: str, **env: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run([str(exe), *args], capture_output=True, text=True, timeout=60,
                          env={"PATH": "/usr/bin:/bin", "HOME": "/nonexistent-home", **env})


def test_the_agent_runs_only_ferbs_own_entry_points(agent_stub: Path, tmp_path: Path) -> None:
    import json
    for argv in (["-m", "ferb.daemon"], ["-I", "-m", "ferb.cli.mcp"], ["-I", "-m", "ferb.memory.card_reader"],
                 ["-I", "-m", "ferb.device.grant_probe"]):                 # APP-662's fresh grant reading
        seen = json.loads(_stub(agent_stub, *argv).stdout)
        assert seen["module"] == "__main__" and seen["isolated"] == 1
    (tmp_path / "script.py").write_text("print('ran')\n")
    refused = (["-c", "print('ran')"], ["-m", "os"], ["-m", "ferb.cli.mcp"], ["-I", "-m", "ferb.cli"],
               [str(tmp_path / "script.py")], ["-m", "ferb.daemon", "--extra"],
               ["-I", "-m", "ferb.device.detector", str(tmp_path / "e.jsonl")],   # `ferb watch` runs it, not the agent
               ["-I", "-m", "ferb.cli.mcp", "x"], ["-i", "-m", "ferb.daemon"], [],
               ["-m", "ferb.memory.card_reader"], ["-I", "-m", "ferb.memory.card_reader", "/etc/passwd"],
               ["-I", "-m", "ferb.memory"],                                 # the card reader: exactly, no path
               ["-m", "ferb.device.grant_probe"], ["-I", "-m", "ferb.device.grant_probe", "x"])
    for argv in refused:
        r = _stub(agent_stub, *argv)
        assert r.returncode == 64 and "ran" not in r.stdout and "FERB" in r.stderr, argv


def test_the_agent_starts_from_a_clean_environment_and_names_itself(agent_stub: Path, tmp_path: Path) -> None:
    """Only what FERB needs reaches Python and the processes it starts: no PYTHON* variable, no proxy or certificate
    override, the owner's HOME and a fixed PATH; sys.executable is the agent's real path however it was started."""
    import json
    import pwd
    owner = pwd.getpwuid(os.getuid())
    link = tmp_path / "shim-link"
    link.symlink_to(agent_stub)
    evil = str(tmp_path / "evil")
    r = _stub(link, "-I", "-m", "ferb.cli.mcp", PYTHONEXECUTABLE=evil, __PYVENV_LAUNCHER__=evil, PYTHONPATH=evil,
              PYTHONHOME=evil, PYTHONINSPECT="1", HTTPS_PROXY="http://127.0.0.1:9", SSL_CERT_FILE=evil,
              CODEX_HOME=evil, TIKTOKEN_CACHE_DIR=evil, TMPDIR=evil, HOME=evil, PATH=evil, DYLD_PRINT_LIBRARIES_X=evil,
              FERB_RUN_TOKEN="token-for-the-shim", LANG="en_US.UTF-8")   # DYLD_*: the signed agent's hardened runtime
    seen = json.loads(r.stdout)
    assert seen["executable"] == os.path.realpath(agent_stub)               # not the link it was started through
    env = seen["environ"]
    assert env["HOME"] == owner.pw_dir and env["PATH"] == "/usr/bin:/bin:/usr/sbin:/sbin"
    assert env["USER"] == env["LOGNAME"] == owner.pw_name and env["TMPDIR"] != evil
    assert env["FERB_RUN_TOKEN"] == "token-for-the-shim" and env["LANG"] == "en_US.UTF-8"   # what FERB needs stays
    assert not {"HTTPS_PROXY", "SSL_CERT_FILE", "CODEX_HOME", "TIKTOKEN_CACHE_DIR", "DYLD_PRINT_LIBRARIES_X",
                "__PYVENV_LAUNCHER__"} & set(env) and not [k for k in env if k.startswith("PYTHON")]


def test_an_environment_entry_without_a_value_is_dropped_not_spun_on(agent_stub: Path, tmp_path: Path) -> None:
    import json
    launcher = tmp_path / "launch"
    src = tmp_path / "launch.c"
    src.write_text('#include <unistd.h>\nint main(int c, char **v) { char *env[] = {"PYTHONFOO", "FERB_RUN_TOKEN=t", 0};'
                   ' char *args[] = {v[1], "-I", "-m", "ferb.cli.mcp", 0}; execve(v[1], args, env); return 99; }\n')
    cmd = ["clang"] if shutil.which("clang") else ["xcrun", "clang"]
    subprocess.run([*cmd, str(src), "-o", str(launcher)], check=True)
    r = subprocess.run([str(launcher), str(agent_stub)], capture_output=True, text=True, timeout=20)
    seen = json.loads(r.stdout)
    assert "PYTHONFOO" not in seen["environ"] and seen["environ"]["FERB_RUN_TOKEN"] == "t"


def test_the_agent_runs_only_with_the_owners_ferb_home(agent_stub: Path, tmp_path: Path) -> None:
    import json
    import pwd
    owner = Path(pwd.getpwuid(os.getuid()).pw_dir) / ".ferb"
    seen = json.loads(_stub(agent_stub, "-I", "-m", "ferb.cli.mcp").stdout)
    assert seen["ferb_home"] == str(owner)                  # unset: the owner's ~/.ferb, never $HOME's
    foreign = tmp_path / "other-ferb"
    foreign.mkdir()
    r = _stub(agent_stub, "-m", "ferb.daemon", FERB_HOME=str(foreign))
    assert r.returncode == 64 and "FERB_HOME" in r.stderr and r.stdout == ""
    link = tmp_path / "link-to-owner"
    if owner.is_dir():                                      # the same folder by another path: Python sees the owner's
        link.symlink_to(owner)
        seen = json.loads(_stub(agent_stub, "-I", "-m", "ferb.cli.mcp", FERB_HOME=str(link)).stdout)
        assert seen["ferb_home"] == str(owner)


def test_codex_binary_is_the_pinned_package_for_each_architecture() -> None:
    """The daemon starts the Codex binary of its own architecture (an Intel FERB.app's Python reports x86_64, also
    under Rosetta), and package-lock.json pins that architecture's package at FERB's Codex version."""
    import json
    import platform
    from ferb.runtimes.codex_exec import CODEX_VERSION, _NATIVE_BIN, native_bin
    assert native_bin("arm64") == Path("node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex")
    assert native_bin("x86_64") == Path("node_modules/@openai/codex-darwin-x64/vendor/x86_64-apple-darwin/bin/codex")
    assert _NATIVE_BIN == native_bin(platform.machine())
    lock = json.loads((ROOT / "package-lock.json").read_text())["packages"]
    for cpu in ("arm64", "x64"):
        entry = lock[f"node_modules/@openai/codex-darwin-{cpu}"]
        assert entry["version"] == f"{CODEX_VERSION}-darwin-{cpu}" and entry["integrity"].startswith("sha512-")


# ---------- locked dependencies install on both architectures (L34) ----------


def _wheel_fits(filename: str, arch: str) -> bool:
    """A wheel that CPython 3.12 on macOS `arch` (arm64 or x86_64) can install."""
    python, abi, platform = filename[: -len(".whl")].split("-")[-3:]
    if platform == "any":
        return True
    mac = any(tag.startswith("macosx_") and tag.endswith((f"_{arch}", "_universal2")) for tag in platform.split("."))
    cpython = any(tag in ("py3", "cp312") or (abi == "abi3" and tag.startswith("cp3") and int(tag[3:]) <= 12)
                  for tag in python.split("."))
    return mac and cpython


def _runtime_packages(arch: str) -> list[dict[str, Any]]:
    """Every package uv.lock installs into FERB.app's Python on macOS `arch`: ferb's dependencies and theirs, with the
    extras they ask for, markers evaluated for that Mac (no dev group, no optional memory extra)."""
    import tomllib

    from packaging.markers import Marker
    env = {"sys_platform": "darwin", "platform_system": "Darwin", "platform_machine": arch, "os_name": "posix",
           "python_version": "3.12", "python_full_version": "3.12.13", "implementation_name": "cpython",
           "platform_python_implementation": "CPython", "extra": ""}

    def applies(marker: str | None) -> bool:
        return marker is None or Marker(marker).evaluate(env)

    by_name: dict[str, list[dict[str, Any]]] = {}
    for p in tomllib.loads(LOCK.read_text())["package"]:
        by_name.setdefault(p["name"], []).append(p)

    def pick(dep: dict[str, Any]) -> dict[str, Any]:
        found = [p for p in by_name[dep["name"]] if dep.get("version", p["version"]) == p["version"]
                 and any(applies(m) for m in p.get("resolution-markers", [None]))]
        assert len(found) == 1, (arch, dep, [p["version"] for p in found])
        return found[0]

    seen: dict[tuple[str, str], dict[str, Any]] = {}
    walked: set[tuple[str, str, tuple[str, ...]]] = set()
    todo: list[tuple[dict[str, Any], tuple[str, ...]]] = [(by_name["ferb"][0], ())]
    while todo:
        pkg, extras = todo.pop()
        deps = list(pkg.get("dependencies", []))
        for extra in extras:
            deps += pkg.get("optional-dependencies", {}).get(extra, [])
        for dep in deps:
            if not applies(dep.get("marker")):
                continue
            p = pick(dep)
            key = (p["name"], p["version"], tuple(dep.get("extra", [])))
            if key in walked:
                continue
            walked.add(key)
            seen[(p["name"], p["version"])] = p
            todo.append((p, key[2]))
    return list(seen.values())


# decision: REL-402
def test_every_locked_runtime_package_installs_on_both_architectures() -> None:
    """FERB.app ships for Apple silicon and Intel (L34), so each package its Python loads needs a wheel for both; a
    source-only package would need a compiler toolchain at release time (cryptography 49 dropped Intel Mac wheels)."""
    for arch in ("arm64", "x86_64"):
        packages = _runtime_packages(arch)
        assert len(packages) > 40, arch                                       # the walk found the whole tree
        missing = sorted(f'{p["name"]} {p["version"]}' for p in packages
                         if not any(_wheel_fits(w["url"].rsplit("/", 1)[1], arch) for w in p.get("wheels", [])))
        assert missing == [], (arch, missing)


def test_real_release_bundle_pins_ferbs_certificate_in_every_piece_macos_grants_to(real_bundle: Path,
                                                                                  tmp_path: Path) -> None:
    if os.environ.get("FERB_RELEASE_BUNDLE") != "1":
        pytest.skip("set FERB_RELEASE_BUNDLE=1 with FERB_APP_BUNDLE pointing at a SIGN_IDENTITY=release build")
    from ferb.device.install import Installer, certificate_leaf, designated_requirement
    sha = os.environ.get("FERB_TEST_RELEASE_SHA1") or (ROOT / "scripts" / "release-cert.sha1").read_text().strip()
    for rel, ident in RELEASE_PIECES:
        piece = real_bundle / rel if rel else real_bundle
        shown = subprocess.run(["codesign", "-d", "-r-", str(piece)], capture_output=True, text=True)
        requirement = designated_requirement(shown.stdout + shown.stderr)
        assert requirement.startswith(f'identifier "{ident}" ') and certificate_leaf(requirement) == sha, requirement
        details = subprocess.run(["codesign", "-dv", "--verbose=2", str(piece)], capture_output=True, text=True).stderr
        assert "Authority=FERB Release Signing" in details and "TeamIdentifier=not set" in details, details
    copy = tmp_path / "ferb-secrets"
    shutil.copyfile(real_bundle / "Contents/Helpers/ferb-secrets", copy)       # what setup copies into ~/.ferb/bin
    pinned = Installer(tmp_path / ".ferb", bundle=real_bundle).sealed_requirement(copy, "Helpers/ferb-secrets")
    assert certificate_leaf(pinned) == sha
