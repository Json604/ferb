"""Packaged FERB.app and DMG (L27): the app's Info.plists, icons and sounds, the build's shell libraries, the one
signing step (ad hoc, release, Developer ID and notarization, no rebuild: scripts/sign-app.sh) and the disk image and
appcast, all driven against stub signing tools. Ported from tests/test_packaging.py to stdlib unittest; the fake app
is the Rust build's shape (scripts/build-app-rust.sh). Nothing here signs for real, notarizes or reads a keychain."""

from __future__ import annotations

import importlib.util
import json
import os
import plistlib
import re
import shlex
import shutil
import subprocess
import textwrap
import unittest
from datetime import datetime, timezone
from pathlib import Path

from scripts.tests.support import (BUILD_APP, BUILD_DMG, CRATES, DEV_ID, DEVID_LISTED, DEVID_SHA, RELEASE_PIECES,
                                   ROOT, SCRIPTS, SHA, SIGN_APP, TEAM, TempCase, compile_macho, devid_env, ent,
                                   executable, fake_app, parts, read_calls, release_env, run, rust_const, sign_only,
                                   signed_paths, stubs, team_requirement)

NOTCH_PLIST = ROOT / "apps" / "mac" / "FERBNotch" / "Info.plist"
# The bundles macOS shows as FERB: the app and its daemon helper (Privacy lists, Activity Monitor)
BUNDLES = {"FERB.app": NOTCH_PLIST, "Helpers/FERB.app": ROOT / "apps" / "mac" / "FERBAgent" / "Info.plist"}
APP_ICON = ROOT / "assets" / "logo" / "AppIcon.icns"


def _icon_file(info: dict) -> str:
    name = info["CFBundleIconFile"]
    return name if name.endswith(".icns") else name + ".icns"


def _load(name: str):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / f"{name}.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# ---------- the app's identity ----------

class Identity(TempCase):
    def test_ferb_app_info_plist_names_ferb_and_declares_its_usage_descriptions(self) -> None:
        info = plistlib.loads(NOTCH_PLIST.read_bytes())
        self.assertTrue(info["CFBundleName"] == info["CFBundleDisplayName"] == "FERB")
        app_id = rust_const(CRATES / "ferb-device/src/install.rs", "APP_BUNDLE_ID")
        self.assertTrue(info["CFBundleIdentifier"] == app_id == "com.ferb.FERB")
        self.assertTrue(info["CFBundleExecutable"] == "FERBNotch" and info["CFBundlePackageType"] == "APPL")
        self.assertIs(info["LSUIElement"], True)                             # background first: no Dock icon
        self.assertTrue(info["NSMicrophoneUsageDescription"].strip())
        self.assertTrue(info["NSSpeechRecognitionUsageDescription"].strip())
        # SENSE-420: Location is FERB.app's own grant; macOS shows these words in its question
        self.assertTrue(info["NSLocationUsageDescription"].strip())
        self.assertEqual(info["NSLocationWhenInUseUsageDescription"], info["NSLocationUsageDescription"])
        self.assertFalse((ROOT / "macapp").exists())                          # FERB Voice merged into the app

    # decision: REL-4
    def test_every_bundle_names_ferbs_icon_and_the_icon_holds_every_size(self) -> None:
        """Owner, 2026-11-01: FERB's logo everywhere. Each bundle names AppIcon, made from assets/logo/ferb-logo.svg by
        scripts/make_logo.py, and the icon carries every size macOS asks for."""
        for bundle, plist in BUNDLES.items():
            self.assertEqual(_icon_file(plistlib.loads(plist.read_bytes())), APP_ICON.name, bundle)
        out = self.tmp / "AppIcon.iconset"
        done = subprocess.run(["iconutil", "-c", "iconset", "-o", str(out), str(APP_ICON)], capture_output=True,
                              text=True)
        self.assertEqual(done.returncode, 0, done.stderr)
        sizes = {f"icon_{s}x{s}{x}.png" for s in (16, 32, 128, 256, 512) for x in ("", "@2x")}
        self.assertEqual({f.name for f in out.iterdir()}, sizes)

    def test_the_development_build_keeps_its_own_identity(self) -> None:
        """`apps/mac/FERBNotch/build.sh` (the owner's dev notch) must not take the packaged app's bundle id, or macOS
        would mix up the two apps' permissions."""
        script = (ROOT / "apps" / "mac" / "FERBNotch" / "build.sh").read_text()
        self.assertTrue("dev.ferb.notch" in script and "FERB Notch" in script)

    # decision: APP-525, APP-535
    def test_the_sound_family_ships_as_lossless_alac_with_one_banner_copy(self) -> None:
        """The committed sounds are ALAC in .caf (lossless, about half the size of the .wav renders); the banners'
        sound lives once, at the package root build-app-rust.sh copies into Contents/Resources, never also in the
        Sounds bundle; the Google Material sounds' CC BY 4.0 credit sits beside them and in both notices the app
        ships."""
        notch = ROOT / "apps" / "mac" / "FERBNotch"
        sounds = notch / "Sources" / "FERBNotch" / "Resources" / "Sounds"
        files = sorted(p.name for p in sounds.iterdir())
        self.assertTrue("CREDITS.md" in files and all(n.endswith(".caf") for n in files if n != "CREDITS.md"))
        banner = notch / "ferb-notification-banner.caf"
        self.assertTrue(banner.is_file() and not (sounds / banner.name).exists())
        for caf in [banner, *sounds.glob("*.caf")]:
            head = caf.read_bytes()[:64]
            self.assertTrue(head[:4] == b"caff" and head[8:12] == b"desc" and head[28:32] == b"alac", caf.name)
        script = BUILD_APP.read_text()
        self.assertIn('cp "$NOTCH/ferb-notification-banner.caf" "$CONTENTS/Resources/ferb-notification-banner.caf"',
                      script)
        self.assertNotIn(".wav", script)
        for notices in (ROOT / "THIRD_PARTY_NOTICES.md", notch / "ThirdPartyNotices.txt"):
            self.assertIn("CC BY 4.0", notices.read_text(), notices.name)


# ---------- the build's shell libraries ----------

class ShellLibraries(TempCase):
    def test_build_scripts_are_executable_zsh_that_parses(self) -> None:
        for name in ("build-app-rust.sh", "sign-app.sh", "build-dmg.sh", "signing.sh", "app-parts.sh",
                     "rust-parts.sh", "release-appcast.sh", "sparkle-keys.sh", "swift-test-gate.sh",
                     "swiftpm-anonymous.sh"):
            script = SCRIPTS / name
            self.assertTrue(script.is_file(), script)
            self.assertEqual(subprocess.run(["/bin/zsh", "-n", str(script)], capture_output=True).returncode, 0,
                             script)
        for script in (BUILD_APP, SIGN_APP, BUILD_DMG, SCRIPTS / "release-appcast.sh"):
            self.assertTrue(executable(script), script)

    def test_the_build_and_the_daemon_name_the_same_codex_binary_for_each_architecture(self) -> None:
        """app-parts.sh's FERB_CODEX_BIN is ferb-runtimes' native_bin under Resources/node, for each architecture."""
        src = (CRATES / "ferb-runtimes/src/codex/mod.rs").read_text()
        self.assertIn('"node_modules/@openai/{package}/vendor/{triple}/bin/codex"', src)
        self.assertIn('"x86_64" => ("codex-darwin-x64", "x86_64-apple-darwin")', src)
        self.assertIn('_ => ("codex-darwin-arm64", "aarch64-apple-darwin")', src)
        for arch, package, triple in (("arm64", "codex-darwin-arm64", "aarch64-apple-darwin"),
                                      ("x86_64", "codex-darwin-x64", "x86_64-apple-darwin")):
            done = parts(f"ferb_arch_names {arch} && print -r -- $FERB_CODEX_BIN")
            self.assertEqual(done.stdout.strip(),
                             f"Resources/node/node_modules/@openai/{package}/vendor/{triple}/bin/codex", done.stderr)
        self.assertNotEqual(parts("ferb_arch_names ppc").returncode, 0)

    def test_arch_check_names_every_file_built_for_another_architecture(self) -> None:
        app = self.tmp / "FERB.app"
        c = app / "Contents"
        compile_macho(c / "MacOS/FERBNotch", "arm64")
        compile_macho(c / "Helpers/FERB.app/Contents/MacOS/FERB", "arm64")
        script = c / "Resources/ferb/run.sh"
        script.parent.mkdir(parents=True)
        script.write_text("#!/bin/zsh\nexit 0\n")
        script.chmod(0o755)                                                   # an executable script: not Mach-O
        self.assertEqual(parts(f"ferb_check_arch {shlex.quote(str(app))} arm64").returncode, 0)
        compile_macho(c / "Helpers/ferb-secrets", "x86_64")
        compile_macho(c / "Helpers/ferb-native-host", "arm64", "x86_64")
        done = parts(f"ferb_check_arch {shlex.quote(str(app))} arm64")
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("Contents/Helpers/ferb-secrets: x86_64", done.stderr)
        self.assertIn("Contents/Helpers/ferb-native-host: ", done.stderr)
        self.assertNotIn("MacOS/FERBNotch", done.stderr)


# ---------- the appcast ----------

class Appcast(TempCase):
    # decision: APP-903, REL-423
    def test_the_appcast_marks_the_signer_switch_as_an_update_the_owner_is_asked_about(self) -> None:
        """Every appcast from the first Developer ID build on names that build as Sparkle's minimumAutoupdateVersion:
        an installed FERB from before it (self-signed) never installs it in the background but asks first, while
        Developer ID builds keep updating silently. A minimum after the build being published is refused (it would
        ask every time)."""
        make = _load("make_appcast")
        now = datetime(2026, 11, 13, tzinfo=timezone.utc)
        signed = 'sparkle:edSignature="QUJD" length="42"'
        args = ("0.28.0", "202611200900", "v0.28.0", "FERB-AppleSilicon.dmg", signed, "14.2", now)
        self.assertNotIn("minimumAutoupdateVersion", make.appcast(*args))           # before the Developer ID
        self.assertIn("<sparkle:minimumAutoupdateVersion>202611200900</sparkle:minimumAutoupdateVersion>",
                      make.appcast(*args, "202611200900"))
        with self.assertRaisesRegex(ValueError, "first Developer ID build"):
            make.appcast(*args, "202612010900")


# ---------- the signing step, against stub tools (scripts/sign-app.sh) ----------

class Signing(TempCase):
    def test_ad_hoc_is_the_default_and_signs_every_nested_binary_inside_out(self) -> None:
        app, calls, done = sign_only(self.tmp)
        self.assertEqual(done.returncode, 0, done.stderr + done.stdout)
        signed = signed_paths(calls)
        c = app / "Contents"
        vendor = c / "Resources/node/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin"
        for path in (c / "Helpers/ferb-secrets", c / "Helpers/ferb-native-host", c / "Resources/bin/ferb",
                     vendor / "lib/libtool.dylib", c / "Helpers/FERB.app", app):
            self.assertIn(str(path), signed)
        self.assertEqual(signed[-1], str(app))                                # the app last, after everything in it
        self.assertNotIn(str(vendor / "bin/codex"), signed)                   # another team's signature is kept
        self.assertFalse(any(s.endswith((".md", "/MacOS/FERB")) for s in signed))
        sign_calls = [c for c in calls if "|--sign|" in c]
        self.assertTrue(all("|--sign|-|" in c and "|--options|runtime|" in c and "|--timestamp=none|" in c
                            for c in sign_calls))
        self.assertTrue(any("|--identifier|com.ferb.secrets|" in c and c.endswith("/Helpers/ferb-secrets")
                            for c in calls))
        self.assertFalse(any("com.ferb.python" in c for c in calls))          # the Rust app has no interpreter
        agent = ent(self.tmp, c / "Helpers/FERB.app")                         # the daemon runs as the agent app FERB
        self.assertIn("com.apple.security.automation.apple-events", agent)
        self.assertIn("com.apple.security.cs.disable-library-validation", agent)   # ad-hoc code has no Team ID
        self.assertIn("com.apple.security.device.audio-input", ent(self.tmp, app))
        self.assertIn("com.apple.security.personal-information.location", ent(self.tmp, app))   # SENSE-420
        self.assertIn("com.apple.security.device.camera", ent(self.tmp, app))                    # APP-983: Show me
        self.assertNotIn("com.apple.security.device.camera", agent)                             # the daemon holds none
        self.assertTrue(any("|--identifier|com.ferb.power|" in c and c.endswith("/Helpers/ferb-power")
                            for c in calls))                                                     # APP-986
        self.assertNotIn("com.apple.security.personal-information.location", agent)          # the daemon holds none
        self.assertIn("com.apple.security.cs.disable-library-validation", ent(self.tmp, app))
        self.assertFalse(any(s.endswith("/Helpers/FERB.app/Contents/MacOS/FERB") for s in signed))  # with its app
        self.assertTrue(any(c.startswith("codesign|--verify|--deep|--strict") and c.endswith(str(app))
                            for c in calls))
        self.assertFalse(any(c.startswith("xcrun") for c in calls))          # no notarization without a Developer ID
        self.assertIn(f"Signed: {app} (identity: -)", done.stdout)
        self.assertFalse((app.parent / ".sign-work").exists())                # its work folder is removed

    # decision: REL-426
    def test_sign_app_refuses_what_isnt_an_app_bundle(self) -> None:
        env = stubs(self.tmp)
        (self.tmp / "folder").mkdir()
        for argv in ([SIGN_APP], [SIGN_APP, self.tmp / "folder"], [SIGN_APP, self.tmp / "missing.app"]):
            done = run(argv, env)
            self.assertNotEqual(done.returncode, 0, argv)
            self.assertTrue("usage" in done.stderr or "isn't an app bundle" in done.stderr, done.stderr)
        self.assertEqual(read_calls(self.tmp), [])

    def test_the_bundle_has_no_voice_helper(self) -> None:
        """Hearing is FERB.app's own modes: nothing is built, listed, signed or pinned as a nested voice app."""
        app, calls, done = sign_only(self.tmp)
        self.assertEqual(done.returncode, 0, done.stderr + done.stdout)
        self.assertFalse((app / "Contents/Helpers/FERB Voice.app").exists())
        self.assertFalse(any("Voice" in piece for piece in signed_paths(calls)))
        for script in ("build-app-rust.sh", "sign-app.sh", "signing.sh", "app-parts.sh", "rust-parts.sh"):
            text = (SCRIPTS / script).read_text()
            self.assertTrue("FERB Voice" not in text and "macapp" not in text, script)
        self.assertFalse(any(ident == "com.ferb.app" for _, ident in RELEASE_PIECES))
        stray = fake_app(self.tmp / "stray")                                  # an unknown nested app isn't signed
        (stray / "Contents/Helpers/Other.app/Contents").mkdir(parents=True)
        (stray / "Contents/Helpers/Other.app/Contents/Info.plist").write_bytes(
            plistlib.dumps({"CFBundleIdentifier": "x"}))
        (self.tmp / "stray-run").mkdir()
        refused = run([SIGN_APP, stray], stubs(self.tmp / "stray-run"))
        self.assertTrue(refused.returncode != 0 and "no piece of FERB.app" in refused.stderr, refused.stderr)

    def test_developer_id_signs_with_a_timestamp_then_notarizes_and_staples_without_rebuilding(self) -> None:
        app, calls, done = sign_only(self.tmp, SIGN_IDENTITY=DEV_ID, NOTARY_KEYCHAIN_PROFILE="ferb-notary")
        self.assertEqual(done.returncode, 0, done.stderr + done.stdout)
        sign_calls = [c for c in calls if "|--sign|" in c]
        self.assertTrue(sign_calls and all(f"|--sign|{DEV_ID}|" in c and "|--timestamp|" in c for c in sign_calls))
        agent = ent(self.tmp, app / "Contents/Helpers/FERB.app")
        self.assertNotIn("disable-library-validation", agent)                 # one team signs all
        self.assertIn("apple-events", agent)
        submit = next(c for c in calls if c.startswith("xcrun|notarytool|submit"))
        self.assertTrue("|--wait|" in submit and submit.endswith("|--keychain-profile|ferb-notary"))
        self.assertIn(f"xcrun|stapler|staple|{app}", calls)
        self.assertGreater(calls.index(submit), max(calls.index(c) for c in sign_calls))   # once fully signed
        self.assertFalse((app.parent / ".sign-work").exists())                # the notarization zip went with it

    def test_a_developer_id_named_by_its_certificate_hash_is_still_notarized(self) -> None:
        sha1 = "0123456789ABCDEF0123456789ABCDEF01234567"
        listed = f'  1) {sha1} "{DEV_ID}"'
        _, calls, done = sign_only(self.tmp, SIGN_IDENTITY=sha1.lower(), STUB_IDENTITIES=listed,
                                   NOTARY_KEYCHAIN_PROFILE="ferb-notary")
        self.assertEqual(done.returncode, 0, done.stderr + done.stdout)
        self.assertTrue(all("|--timestamp|" in c for c in calls if "|--sign|" in c))
        self.assertTrue(any(c.startswith("xcrun|notarytool|submit") for c in calls))

    def test_another_certificate_is_signed_but_never_notarized(self) -> None:
        sha1 = "89ABCDEF0123456789ABCDEF0123456789ABCDEF"
        listed = f'  1) {sha1} "Apple Development: Test Owner (TEAM123456)"'
        app, calls, done = sign_only(self.tmp, SIGN_IDENTITY=sha1, STUB_IDENTITIES=listed)
        self.assertEqual(done.returncode, 0, done.stderr + done.stdout)
        self.assertTrue(all("|--timestamp=none|" in c for c in calls if "|--sign|" in c))
        self.assertFalse(any(c.startswith("xcrun") for c in calls))
        self.assertIn("disable-library-validation", ent(self.tmp, app / "Contents/Helpers/FERB.app"))  # no Team ID

    def test_developer_id_accepts_an_api_key_from_the_environment(self) -> None:
        key = self.tmp / "AuthKey_ABC.p8"
        key.write_text("-----BEGIN PRIVATE KEY-----\nnot a real key\n-----END PRIVATE KEY-----\n")
        _, calls, done = sign_only(self.tmp, SIGN_IDENTITY=DEV_ID, NOTARY_KEY=str(key), NOTARY_KEY_ID="ABC123DEFG",
                                   NOTARY_ISSUER="00000000-0000-0000-0000-000000000000")
        self.assertEqual(done.returncode, 0, done.stderr + done.stdout)
        submit = next(c for c in calls if c.startswith("xcrun|notarytool|submit"))
        self.assertTrue(submit.endswith(
            f"|--key|{key}|--key-id|ABC123DEFG|--issuer|00000000-0000-0000-0000-000000000000"), submit)

    def test_developer_id_without_notary_credentials_stops_before_signing(self) -> None:
        _, calls, done = sign_only(self.tmp, SIGN_IDENTITY=DEV_ID)
        self.assertTrue(done.returncode != 0 and "NOTARY_KEYCHAIN_PROFILE" in done.stderr, done.stderr)
        self.assertEqual(signed_paths(calls), [])

    def test_release_signing_uses_only_the_pinned_certificate_and_pins_it_in_each_requirement(self) -> None:
        app, calls, done = sign_only(self.tmp, **release_env())
        self.assertEqual(done.returncode, 0, done.stderr + done.stdout)
        sign_calls = [c for c in calls if "|--sign|" in c and "--verify" not in c]
        self.assertTrue(sign_calls and all(f"|--sign|{SHA.upper()}|" in c and "|--timestamp=none|" in c
                                           for c in sign_calls))
        for rel, ident in RELEASE_PIECES:
            piece = app / rel if rel else app
            call = next(c for c in sign_calls if c.endswith(f"|{piece}"))
            self.assertIn(f'|--requirements|=designated => identifier "{ident}" and certificate leaf = H"{SHA}"|',
                          call)
            self.assertIn(f'codesign|--verify|--strict|-R=identifier "{ident}" and certificate leaf = H"{SHA}"|{piece}',
                          calls)
        lib = next(c for c in sign_calls if c.endswith("libtool.dylib"))
        self.assertNotIn("--requirements", lib)                               # libraries keep codesign's own
        self.assertFalse(any(c.endswith("/bin/codex") for c in sign_calls))   # another team's signature is kept
        self.assertIn("com.apple.security.cs.disable-library-validation",
                      ent(self.tmp, app / "Contents/Helpers/FERB.app"))
        self.assertFalse(any(c.startswith("xcrun") for c in calls))           # a self-made certificate: no notary

    def test_release_signing_refuses_a_missing_or_other_certificate_before_signing_anything(self) -> None:
        cases = ({**release_env(), "STUB_IDENTITIES": ""},                                         # not in keychain
                 {**release_env(), "STUB_IDENTITIES": f'  1) {"F" * 40} "FERB Release Signing"'},   # same name
                 {**release_env(), "FERB_TEST_RELEASE_SHA1": ""},                                   # no SHA-1
                 {**release_env(), "FERB_TEST_RELEASE_SHA1": "not-a-sha1"})
        for i, env in enumerate(cases):
            (self.tmp / str(i)).mkdir()
            _, calls, done = sign_only(self.tmp / str(i), **env)
            self.assertTrue(done.returncode != 0 and "FERB Release Signing" in done.stderr, (i, done.stderr))
            self.assertEqual(signed_paths(calls), [], i)

    def test_release_mode_stays_release_for_the_build_scripts_it_starts(self) -> None:
        """build-dmg.sh starts build-app-rust.sh: SIGN_IDENTITY must still say `release` there, not a bare hash that
        would skip the pinned requirements; codesign gets the hash through FERB_SIGN_ID."""
        env = {**stubs(self.tmp), **release_env(), "ROOT": str(ROOT)}
        done = run(["/bin/zsh", "-f", "-c", 'source "$ROOT/scripts/signing.sh" && print -r -- "$SIGN_IDENTITY $FERB_SIGN_ID"'],
                   env)
        self.assertEqual(done.stdout.strip(), f"release {SHA.upper()}", done.stderr)

    # decision: REL-421
    def test_devid_signs_with_the_teams_developer_id_by_its_hash_and_checks_each_piece_against_the_team(self) -> None:
        """SIGN_IDENTITY=devid picks the one Developer ID Application identity of scripts/developer-team.txt's team by
        its SHA-1 (the name, a legal name, is never passed or printed), keeps codesign's own team requirement, verifies
        every piece macOS grants permissions to against the team, keeps library validation on, then notarizes and
        staples."""
        app, calls, done = sign_only(self.tmp, **devid_env())
        self.assertEqual(done.returncode, 0, done.stderr + done.stdout)
        self.assertNotIn("Test Owner", done.stdout + done.stderr)
        sign_calls = [c for c in calls if "|--sign|" in c and "--verify" not in c]
        self.assertTrue(sign_calls and all(f"|--sign|{DEVID_SHA}|" in c and "|--timestamp|" in c
                                           for c in sign_calls))
        self.assertFalse(any("--requirements" in c for c in sign_calls))     # codesign's default DR names the team
        self.assertFalse(any(c.endswith("/bin/codex") for c in sign_calls))   # another team's Developer ID is kept
        for rel, ident in RELEASE_PIECES:
            piece = app / rel if rel else app
            self.assertIn(f"codesign|--verify|--strict|-R={team_requirement(ident)}|{piece}", calls)
        for piece in (app, app / "Contents/Helpers/FERB.app"):
            self.assertNotIn("disable-library-validation", ent(self.tmp, piece))   # one team signs everything
        submit = next(c for c in calls if c.startswith("xcrun|notarytool|submit"))
        self.assertGreater(calls.index(submit), max(calls.index(c) for c in sign_calls))
        self.assertIn(f"xcrun|stapler|staple|{app}", calls)
        (self.tmp / "sourced").mkdir()
        env = {**stubs(self.tmp / "sourced"), **devid_env(), "ROOT": str(ROOT)}
        shown = run(["/bin/zsh", "-f", "-c", 'source "$ROOT/scripts/signing.sh" && print -r -- "$SIGN_IDENTITY $FERB_SIGN_ID"'],
                    env)
        self.assertEqual(shown.stdout.strip(), f"devid {DEVID_SHA}", shown.stderr)   # build-dmg.sh stays devid

    # decision: REL-421
    def test_devid_refuses_a_missing_ambiguous_or_other_identity_before_signing_anything(self) -> None:
        other_kinds = "\n".join(line for line in DEVID_LISTED.splitlines() if DEV_ID not in line)
        cases = ({**devid_env(), "STUB_IDENTITIES": ""},                                  # not in the keychain
                 {**devid_env(), "STUB_IDENTITIES": other_kinds},                         # Apple Development etc.
                 {**devid_env(), "STUB_IDENTITIES": f'{DEVID_LISTED}\n  5) {"D" * 40} "{DEV_ID}"'},   # two
                 {**devid_env(), "FERB_TEST_TEAM_ID": ""},                                # no recorded team
                 {**devid_env(), "FERB_TEST_TEAM_ID": "TEAM-12345"})                      # malformed
        for i, env in enumerate(cases):
            (self.tmp / str(i)).mkdir()
            _, calls, done = sign_only(self.tmp / str(i), **env)
            self.assertTrue(done.returncode != 0 and ("Developer ID" in done.stderr or "Team ID" in done.stderr),
                            (i, done.stderr))
            self.assertNotIn("Test Owner", done.stderr, i)
            self.assertEqual(signed_paths(calls), [], i)

    # decision: REL-422
    def test_every_mach_o_file_is_signed_whatever_its_name_and_an_unsigned_one_stops_the_build(self) -> None:
        """Notarization refuses a bundle holding one unsigned executable or library (kb/plans/developer-id.md §4): a
        Mach-O file is found by its content, so one without a code name or the execute bit is signed too, and after
        signing every Mach-O file is checked, in every mode; in devid mode an ad-hoc leftover is refused as well."""
        app = fake_app(self.tmp / "out")
        blob = app / "Contents/Resources/extensions/chromium/_native.bin"
        blob.parent.mkdir(parents=True)
        shutil.copy("/usr/bin/true", blob)
        blob.chmod(0o644)
        env = stubs(self.tmp)
        done = run([SIGN_APP, app], env)
        self.assertEqual(done.returncode, 0, done.stderr + done.stdout)
        calls = read_calls(self.tmp)
        self.assertIn(str(blob), signed_paths(calls))
        for checked in (blob, app / "Contents/MacOS/FERBNotch", app / "Contents/Helpers/FERB.app/Contents/MacOS/FERB"):
            self.assertIn(f"codesign|--verify|--strict|{checked}", calls)
        self.assertFalse(any(c.endswith("/profile.md") for c in calls))       # not code: never signed or checked

        for i, (mode, stub) in enumerate(((env, {"STUB_UNSIGNED": str(blob)}),
                                          ({**env, **devid_env()}, {"STUB_UNSIGNED": str(blob)}),
                                          ({**env, **devid_env()}, {"STUB_ADHOC": str(blob)}))):
            (self.tmp / "calls.log").unlink()
            refused = run([SIGN_APP, app], {**mode, **stub})
            self.assertNotEqual(refused.returncode, 0, i)
            self.assertIn("chromium/_native.bin", refused.stderr, (i, refused.stderr))
            self.assertFalse(any(c.startswith("xcrun|notarytool") for c in read_calls(self.tmp)), i)

    def test_build_app_refuses_an_unknown_architecture_and_intel_without_rosetta(self) -> None:
        env = stubs(self.tmp)
        out = self.tmp / "out"
        bad = run([BUILD_APP, "--arch", "ppc", out], env)
        self.assertTrue(bad.returncode != 0 and "arm64 or x86_64" in bad.stderr, bad.stderr)
        alone = run([BUILD_APP, "--arch"], env)
        self.assertTrue(alone.returncode != 0 and "usage" in alone.stderr, alone.stderr)
        folder = self.tmp / "stubs"
        (folder / "uname").write_text('#!/bin/zsh\nif [[ "$1" == -m ]]; then print arm64; else /usr/bin/uname "$@"; fi\n')
        (folder / "arch").write_text("#!/bin/zsh\nexit 1\n")                   # Rosetta isn't installed
        for name in ("uname", "arch"):
            (folder / name).chmod(0o755)
        intel = run([BUILD_APP, "--arch", "x86_64", out], env)
        self.assertTrue(intel.returncode != 0 and "Rosetta" in intel.stderr, intel.stderr)
        self.assertFalse(out.exists())                                        # refused before anything was written


# ---------- the disk image ----------

def _dmg_layout_check(case: TempCase, volume: Path) -> dict:
    """dmg_layout.write_ds_store run on `volume` in dmg_layout.py's own locked environment (its PEP 723 metadata and
    scripts/dmg_layout.py.lock, as build-dmg.sh runs it), with the alias stood in (an HFS+ image's file ids fit a
    classic alias; a temp folder's APFS ids don't), and the .DS_Store's records read back as JSON."""
    uv = shutil.which("uv")
    if uv is None:
        raise unittest.SkipTest("needs uv to run dmg_layout.py in its locked environment")
    source = (SCRIPTS / "dmg_layout.py").read_text()
    block = re.search(r"^# /// script$.*?^# ///$", source, re.M | re.S)
    case.assertIsNotNone(block, "dmg_layout.py carries no PEP 723 block")
    checker = case.tmp / "check" / "check_ds_store.py"
    checker.parent.mkdir()
    checker.write_text(block.group(0) + "\n" + textwrap.dedent(f"""
        import json, sys
        sys.path.insert(0, {str(SCRIPTS)!r})
        import dmg_layout
        import mac_alias
        from ds_store import DSStore

        class StandIn:
            def __init__(self, path):
                self.path, self.volume = path, type("Volume", (), {{"posix_path": "?"}})()

            def to_bytes(self):
                return f"alias:{{self.path}} on {{self.volume.posix_path}}".encode()

        mac_alias.Alias.for_file = classmethod(lambda cls, path: StandIn(path))
        volume = sys.argv[1]
        dmg_layout.write_ds_store(__import__("pathlib").Path(volume))
        out = []
        with DSStore.open(volume + "/.DS_Store", "r") as store:
            for entry in store:
                row = {{"file": entry.filename, "code": entry.code.decode()}}
                if entry.code == b"pBBk":
                    row["bookmark"] = isinstance(entry.value, mac_alias.Bookmark)
                    row["path"] = entry.value.get(0x1004)[-3:]
                    row["volume"] = entry.value.get(0x2002)
                if entry.code == b"Iloc":
                    row["at"] = list(entry.value)
                if entry.code == b"icvp":
                    row["type"] = entry.value["backgroundType"]
                    row["alias"] = entry.value["backgroundImageAlias"].decode()
                if entry.code == b"bwsp":
                    row["bars"] = [entry.value["ShowToolbar"], entry.value["ShowSidebar"]]
                out.append(row)
        print(json.dumps(out))
        """))
    shutil.copy(SCRIPTS / "dmg_layout.py.lock", checker.with_name(checker.name + ".lock"))
    done = subprocess.run([uv, "run", "--quiet", "--script", "--locked", str(checker), str(volume)],
                          capture_output=True, text=True, cwd=checker.parent,
                          env={**os.environ, "UV_NO_CONFIG": "1"})
    case.assertEqual(done.returncode, 0, done.stderr)
    return {"rows": json.loads(done.stdout.splitlines()[-1])}


class DiskImage(TempCase):
    # decision: REL-4
    def test_dmg_holds_the_app_and_an_applications_link_and_is_notarized_only_with_a_developer_id(self) -> None:
        app = fake_app(self.tmp / "out")
        env = stubs(self.tmp)
        dmg = self.tmp / "out" / "FERB.dmg"
        done = run([BUILD_DMG, app, dmg], env)
        self.assertEqual(done.returncode, 0, done.stderr + done.stdout)
        staging = (self.tmp / "staging.txt").read_text()
        self.assertTrue("Applications -> /Applications" in staging and "FERB.app" in staging)
        calls = read_calls(self.tmp)
        self.assertTrue(any(c.startswith("hdiutil|create|") and "|-volname|FERB|" in c for c in calls))
        self.assertTrue(any(c.startswith("hdiutil|convert|") and "|-format|ULMO|" in c and c.endswith(str(dmg))
                            for c in calls))
        self.assertFalse(any(c.startswith(("codesign", "xcrun")) for c in calls))
        self.assertTrue(dmg.is_file())
        # The volume shows FERB's logo: the icon is in the image and its root carries Finder's custom-icon flag
        self.assertIn(".VolumeIcon.icns", staging)
        self.assertEqual((self.tmp / "volume-flags.txt").read_text()[16:18], "04")
        self.assertEqual(sorted(p.name for p in dmg.parent.iterdir()), ["FERB.app", "FERB.dmg"])   # nothing left
        self.assertIn("window: before-devid", done.stdout)                   # not notarized: the first-open note

        for name in ("calls.log", "staging.txt"):
            (self.tmp / name).unlink()
        dmg.unlink()
        env.update(SIGN_IDENTITY=DEV_ID, NOTARY_KEYCHAIN_PROFILE="ferb-notary")
        done = run([BUILD_DMG, app, dmg], env)
        self.assertEqual(done.returncode, 0, done.stderr + done.stdout)
        self.assertIn("window: after-devid", done.stdout)                    # notarized: only the drag line
        calls = read_calls(self.tmp)
        self.assertIn(f"codesign|--force|--sign|{DEV_ID}|--timestamp|{dmg}", calls)
        self.assertTrue(any(c.startswith(f"xcrun|notarytool|submit|{dmg}|") for c in calls))
        self.assertIn(f"xcrun|stapler|staple|{dmg}", calls)
        for f in (self.tmp / "out").rglob("*"):                               # credentials are never written
            if f.is_file():
                self.assertNotIn(b"ferb-notary", f.read_bytes(), f)

    # decision: REL-408, REL-409, REL-427, REL-411
    def test_dmg_window_lays_out_exactly_the_app_the_applications_link_and_the_background(self) -> None:
        """Owner, 2026-11-04: a designed window, no text file. The image holds FERB.app, the Applications link and the
        hidden background (plus the hidden volume icon and .DS_Store), and the .DS_Store places exactly those two
        items over that background; the background's words follow the identity, and a preview may override them."""
        dmg_layout = _load("dmg_layout")                                      # its spec needs only the stdlib
        for variant in dmg_layout.VARIANTS:
            spec = dmg_layout.spec(variant)
            self.assertTrue(set(spec["icons"]) == {"FERB.app", "Applications"}
                            and spec["background"] == ".background/FERB.tiff")
            self.assertTrue(all(p.is_file() for p in dmg_layout.background_pngs(variant)))
        self.assertTrue(dmg_layout.spec("before-devid")["note"] and not dmg_layout.spec("after-devid")["note"])

        app = fake_app(self.tmp / "out")
        env = stubs(self.tmp)
        dmg = self.tmp / "out" / "FERB.dmg"
        done = run([BUILD_DMG, app, dmg], env)
        self.assertEqual(done.returncode, 0, done.stderr + done.stdout)
        staged = (self.tmp / "staged.txt").read_text().split()
        self.assertEqual(staged, ["./.VolumeIcon.icns", "./.background", "./.background/FERB.tiff", "./Applications",
                                  "./FERB.app"])
        mount = str(self.tmp / "out" / ".dmg-mount")
        self.assertIn(f"layout|-I|{SCRIPTS / 'dmg_layout.py'}|ds-store|{mount}", (self.tmp / "calls.log").read_text())
        self.assertEqual((self.tmp / "mounted.txt").read_text().split(), [".DS_Store"])   # before it is sealed

        # The .DS_Store itself, written onto a copy of the image's root
        volume = self.tmp / "volume"
        (volume / "FERB.app").mkdir(parents=True)
        (volume / "Applications").symlink_to("/Applications")
        (volume / ".background").mkdir()
        (volume / ".background" / "FERB.tiff").write_bytes(b"tiff")
        rows = _dmg_layout_check(self, volume)["rows"]
        records: dict = {}
        for row in rows:
            records.setdefault(row["file"], set()).add(row["code"])
            if row["code"] == "pBBk":                                         # the background inside the image
                self.assertTrue(row["bookmark"])
                self.assertEqual(row["path"], ["volume", ".background", "FERB.tiff"])
                self.assertEqual(row["volume"], "/Volumes/FERB")             # never the build's own mount
            if row["code"] == "Iloc":
                self.assertEqual(tuple(row["at"]), dmg_layout.ICONS[row["file"]])
            if row["code"] == "icvp":
                self.assertEqual(row["type"], 2)
                self.assertEqual(row["alias"], f"alias:{volume}/.background/FERB.tiff on /Volumes/FERB")
            if row["code"] == "bwsp":
                self.assertEqual(row["bars"], [False, False])
        self.assertEqual(set(records), {".", "FERB.app", "Applications"})
        self.assertTrue(records["FERB.app"] == records["Applications"] == {"Iloc"})

        dmg.unlink()
        preview = run([BUILD_DMG, app, dmg], {**env, "FERB_DMG_VARIANT": "after-devid"})
        self.assertTrue(preview.returncode == 0 and "window: after-devid" in preview.stdout, preview.stderr)
        dmg.unlink()
        bad = run([BUILD_DMG, app, dmg], {**env, "FERB_DMG_VARIANT": "fancy"})
        self.assertTrue(bad.returncode != 0 and not dmg.exists())

    # decision: REL-427, REL-428
    def test_dmg_layout_runs_from_its_own_lock_by_default(self) -> None:
        """Without FERB_DMG_PYTHON, build-dmg.sh runs dmg_layout.py with uv from the script's inline metadata and its
        committed lock (`uv run --script --locked`), pinning ds-store and mac-alias exactly; the lock agrees with the
        metadata."""
        script = BUILD_DMG.read_text()
        self.assertIn('uv run --quiet --script --locked "$@"', script)
        self.assertNotIn("--project", script)
        source = (SCRIPTS / "dmg_layout.py").read_text()
        self.assertIn('#   "ds-store==1.3.3",', source)
        self.assertIn('#   "mac-alias==2.2.3",', source)
        lock = (SCRIPTS / "dmg_layout.py.lock").read_text()
        for name, version in (("ds-store", "1.3.3"), ("mac-alias", "2.2.3")):
            self.assertRegex(lock, rf'name = "{name}"\nversion = "{re.escape(version)}"')
        uv = shutil.which("uv")
        if uv is None:
            raise unittest.SkipTest("needs uv")
        for name in ("dmg_layout.py", "make_logo.py"):
            check = subprocess.run([uv, "lock", "--check", "--script", str(SCRIPTS / name)], capture_output=True,
                                   text=True, env={**os.environ, "UV_NO_CONFIG": "1"})
            self.assertEqual(check.returncode, 0, name + ": " + check.stderr)

    def test_dmg_is_named_for_the_apps_architecture(self) -> None:
        env = {**stubs(self.tmp), "FERB_DMG_DIR": str(self.tmp / "dist")}
        for arch, name in (("arm64", "FERB-AppleSilicon.dmg"), ("x86_64", "FERB-Intel.dmg")):
            app = fake_app(self.tmp / arch)
            compile_macho(app / "Contents/MacOS/FERBNotch", arch)
            done = run([BUILD_DMG, app], env)
            self.assertEqual(done.returncode, 0, done.stderr + done.stdout)
            self.assertTrue((self.tmp / "dist" / name).is_file(), name)
        staging = (self.tmp / "staging.txt").read_text()
        self.assertTrue("Applications -> /Applications" in staging and "FERB.app" in staging and ".txt" not in staging)
        universal = fake_app(self.tmp / "fat")                # copies of /usr/bin/true: x86_64 and arm64e in one file
        done = run([BUILD_DMG, universal], env)
        self.assertTrue(done.returncode != 0 and "one-architecture" in done.stderr, done.stderr)
        self.assertEqual(sorted(p.name for p in (self.tmp / "dist").iterdir()),
                         ["FERB-AppleSilicon.dmg", "FERB-Intel.dmg"])

    # decision: REL-426
    def test_dmg_without_an_app_builds_both_apps_with_the_rust_build(self) -> None:
        """`build-dmg.sh` alone builds each architecture's app with scripts/build-app-rust.sh into
        $FERB_DMG_DIR/app-<arch>, then the image from it (here a stand-in build script in a copy of scripts/)."""
        root = self.tmp / "repo"
        (root / "scripts").mkdir(parents=True)
        for name in ("build-dmg.sh", "signing.sh"):
            shutil.copy(SCRIPTS / name, root / "scripts" / name)
        (root / "assets").symlink_to(ROOT / "assets")
        stand_in = root / "scripts" / "build-app-rust.sh"
        stand_in.write_text('#!/bin/zsh -f\nprint -r -- "build|$*" >> "$STUB_LOG"\n'
                            '[[ $1 == --arch ]] && mkdir -p "$3/FERB.app/Contents/MacOS" '
                            '&& cp /usr/bin/true "$3/FERB.app/Contents/MacOS/FERBNotch"\n')
        stand_in.chmod(0o755)
        dist = self.tmp / "dist"
        done = run([root / "scripts" / "build-dmg.sh"], {**stubs(self.tmp), "FERB_DMG_DIR": str(dist)})
        self.assertEqual(done.returncode, 0, done.stderr + done.stdout)
        calls = read_calls(self.tmp)
        self.assertIn(f"build|--arch arm64 {dist}/app-arm64", calls)
        self.assertIn(f"build|--arch x86_64 {dist}/app-x86_64", calls)
        self.assertTrue((dist / "FERB-AppleSilicon.dmg").is_file() and (dist / "FERB-Intel.dmg").is_file())
        self.assertNotIn("build-app.sh", BUILD_DMG.read_text().replace("build-app-rust.sh", ""))

    def test_a_disk_image_holds_only_a_release_signed_app(self) -> None:
        """What people download carries FERB's release certificate (L34), so their permissions and automatic updates
        carry over to the next version; an ad-hoc or otherwise signed app is refused unless FERB_DEV_DMG=1."""
        app = fake_app(self.tmp / "out")
        compile_macho(app / "Contents/MacOS/FERBNotch", "arm64")
        dist = self.tmp / "dist"
        dmg = dist / "FERB-AppleSilicon.dmg"
        env = {**stubs(self.tmp), "FERB_DMG_DIR": str(dist)}
        del env["FERB_DEV_DMG"]
        adhoc = run([BUILD_DMG, app], env)
        self.assertTrue(adhoc.returncode != 0 and "SIGN_IDENTITY=release" in adhoc.stderr and not dmg.exists())
        env.update(release_env())
        ok = run([BUILD_DMG, app], env)
        self.assertEqual(ok.returncode, 0, ok.stderr + ok.stdout)
        self.assertIn(f'codesign|--verify|--strict|-R=identifier "com.ferb.FERB" and certificate leaf = H"{SHA}"|{app}',
                      read_calls(self.tmp))
        dmg.unlink()
        env["STUB_VERIFY_FAILS"] = "1"                                         # the app is signed some other way
        other = run([BUILD_DMG, app], env)
        self.assertTrue(other.returncode != 0 and "isn't signed with FERB's release certificate" in other.stderr)
        self.assertFalse(dmg.exists())

    # decision: REL-420, REL-421
    def test_a_devid_disk_image_holds_only_a_notarized_team_app_and_is_itself_signed_notarized_and_checked(
            self) -> None:
        """With SIGN_IDENTITY=devid the image takes only an app of FERB's Developer ID team that carries its stapled
        ticket and that Gatekeeper accepts as notarized; the image is then signed by the same identity (its hash),
        notarized, stapled and assessed the way a browser download is. Its window has only the drag line."""
        app = fake_app(self.tmp / "out")
        compile_macho(app / "Contents/MacOS/FERBNotch", "arm64")
        dist = self.tmp / "dist"
        dmg = dist / "FERB-AppleSilicon.dmg"
        env = {**stubs(self.tmp), **devid_env(), "FERB_DMG_DIR": str(dist)}
        del env["FERB_DEV_DMG"]
        ok = run([BUILD_DMG, app], env)
        self.assertEqual(ok.returncode, 0, ok.stderr + ok.stdout)
        self.assertTrue("window: after-devid" in ok.stdout and "Test Owner" not in ok.stdout + ok.stderr)
        calls = read_calls(self.tmp)
        checks = [f"codesign|--verify|--strict|-R={team_requirement('com.ferb.FERB')}|{app}",
                  f"xcrun|stapler|validate|{app}", f"spctl|-a|-t|exec|-vv|{app}",
                  f"codesign|--force|--sign|{DEVID_SHA}|--timestamp|{dmg}",
                  f"xcrun|stapler|staple|{dmg}",
                  f"spctl|-a|-t|open|--context|context:primary-signature|-vv|{dmg}"]
        for check in checks:
            self.assertIn(check, calls)
        self.assertEqual([calls.index(c) for c in checks], sorted(calls.index(c) for c in checks))   # app first
        self.assertTrue(any(c.startswith(f"xcrun|notarytool|submit|{dmg}|") for c in calls))

        for stub in ({"STUB_VERIFY_FAILS": "1"}, {"STUB_STAPLE_MISSING": "1"}, {"STUB_SPCTL_REJECT": "1"},
                     {"STUB_SPCTL_TEAM": "OTHER12345"}):
            if dmg.exists():
                dmg.unlink()
            refused = run([BUILD_DMG, app], {**env, **stub})
            self.assertTrue(refused.returncode != 0 and not dmg.exists(), (stub, refused.stderr))
            self.assertNotIn("Test Owner", refused.stderr, stub)
        raw = {**env, "SIGN_IDENTITY": DEV_ID}                    # a Developer ID named directly: development only
        refused = run([BUILD_DMG, app], raw)
        self.assertTrue(refused.returncode != 0 and "SIGN_IDENTITY=devid" in refused.stderr, refused.stderr)


APPCAST_HDIUTIL = r"""#!/bin/zsh
print -r -- "hdiutil|${(j:|:)@}" >> "$STUB_LOG"
if [[ "$1" == attach ]]; then ditto "$STUB_APP" "${@[-2]}/FERB.app"; fi
if [[ "$1" == detach ]]; then rm -rf "${@[-1]}/FERB.app"; fi
exit 0
"""


class ReleaseAppcast(TempCase):
    """release-appcast.sh, run from a copy of the scripts whose `tools/kb/run` only records the gate, so nothing reaches
    the Keychain, Sparkle's key or the network."""

    def _setup(self, python: bool = False) -> tuple[Path, Path, dict[str, str]]:
        root = self.tmp / "repo"
        (root / "scripts").mkdir(parents=True)
        for name in ("release-appcast.sh", "signing.sh", "rust-parts.sh"):
            shutil.copy(SCRIPTS / name, root / "scripts" / name)
        (root / "tools/kb").mkdir(parents=True)
        (root / "tools/kb/run").write_text('#!/bin/zsh\nprint -r -- "gate|$*" >> "$STUB_LOG"\nexit 1\n')
        (root / "tools/kb/run").chmod(0o755)
        app = fake_app(self.tmp / "out")
        if python:
            (app / "Contents/Resources/python/bin").mkdir(parents=True)
        info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
        info.update(LSMinimumSystemVersion="14.2", FERBVersion="0.28.0",
                    SUFeedURL="https://ferb.kartikey.xyz/updates/appcast-arm64.xml")
        (app / "Contents/Info.plist").write_bytes(plistlib.dumps(info))
        dmg = self.tmp / "FERB-AppleSilicon.dmg"
        dmg.write_bytes(b"fake dmg")
        (self.tmp / "tmp").mkdir()
        env = {**stubs(self.tmp), "FERB_TEST_TEAM_ID": TEAM, "STUB_APP": str(app), "TMPDIR": str(self.tmp / "tmp")}
        (self.tmp / "stubs/hdiutil").write_text(APPCAST_HDIUTIL)
        return root / "scripts" / "release-appcast.sh", dmg, env

    def _run(self, script: Path, dmg: Path, env: dict[str, str], **stub: str) -> subprocess.CompletedProcess[str]:
        log = self.tmp / "calls.log"
        if log.exists():
            log.unlink()
        return run([script, "v0.28.0", dmg, "arm64"], {**env, **stub})

    # decision: REL-421
    def test_the_appcast_refuses_a_dmg_that_isnt_notarized_developer_id_before_signing_it(self) -> None:
        """release-appcast.sh EdDSA-signs a DMG's final bytes, so it refuses one whose app isn't FERB's team's,
        notarized and stapled, or which isn't itself stapled and accepted by Gatekeeper, before the release gate and
        sign_update."""
        script, dmg, env = self._setup()
        for stub in ({"STUB_VERIFY_FAILS": "1"}, {"STUB_STAPLE_MISSING": "1"}, {"STUB_SPCTL_REJECT": "1"},
                     {"STUB_SPCTL_TEAM": "OTHER12345"}):
            refused = self._run(script, dmg, env, **stub)
            self.assertNotEqual(refused.returncode, 0, stub)
            self.assertNotIn("gate|", (self.tmp / "calls.log").read_text(), (stub, refused.stderr))   # refused first
        passed = self._run(script, dmg, env)
        calls = read_calls(self.tmp)
        self.assertTrue("the repo gate refused" in passed.stderr and "gate|gate release" in calls, passed.stderr)
        self.assertIn(f"xcrun|stapler|validate|{dmg}", calls)
        self.assertIn(f"spctl|-a|-t|open|--context|context:primary-signature|-vv|{dmg}", calls)

    # decision: REL-426
    def test_the_appcast_refuses_an_app_that_carries_python(self) -> None:
        """Only the Rust build is released: a DMG whose app carries Contents/Resources/python (the Python build) is
        refused, plainly, before the release gate."""
        script, dmg, env = self._setup(python=True)
        refused = self._run(script, dmg, env)
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("carries Python", refused.stderr)
        self.assertNotIn("gate|", (self.tmp / "calls.log").read_text())


class RecordedIdentities(TempCase):
    # decision: REL-419
    def test_the_developer_id_team_is_recorded(self) -> None:
        """scripts/developer-team.txt names FERB's Developer ID team (owner, 2026-11-13): public, the 10-character
        Team ID every signed binary shows, one line. The certificate's name is never recorded."""
        text = (SCRIPTS / "developer-team.txt").read_text()
        self.assertTrue(len(text) == 11 and text.endswith("\n") and text[:10].isalnum()
                        and text[:10] == text[:10].upper(), repr(text))

    def test_the_release_certificates_sha1_is_recorded(self) -> None:
        """scripts/release-cert.sha1 names FERB's one release certificate (L34): public, 40 lowercase hex digits."""
        path = SCRIPTS / "release-cert.sha1"
        if not path.exists():
            raise unittest.SkipTest("the owner hasn't created FERB Release Signing yet "
                                                  "(steps at the top of scripts/signing.sh)")
        text = path.read_text()
        self.assertTrue(len(text) == 41 and text.endswith("\n") and all(ch in "0123456789abcdef" for ch in text[:40]))


if __name__ == "__main__":
    unittest.main()
