"""Packaged FERB.app and DMG (L27): the app's Info.plist, the one signing step (ad-hoc now, Developer ID plus
notarization later, no rebuild) driven against stub signing tools, and a smoke test of a real bundle.

The real-bundle test checks a built app: `FERB_APP_BUNDLE=/path/FERB.app`, or `FERB_BUILD_APP=1` to build one into a
temp dir first (minutes, ~700 MB; needs swift, uv, npm and their caches or network). Otherwise it skips, saying so."""

import os
import plistlib
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

import ferb
from ferb.device.install import APP_BUNDLE_ID

ROOT = Path(__file__).resolve().parent.parent
NOTCH_PLIST = ROOT / "ui" / "mac" / "FERBNotch" / "Info.plist"
BUILD_APP = ROOT / "scripts" / "build-app.sh"
BUILD_DMG = ROOT / "scripts" / "build-dmg.sh"
DEV_ID = "Developer ID Application: Test Owner (TEAM123456)"


# ---------- the app's identity ----------

def test_ferb_app_info_plist_names_ferb_and_declares_its_usage_descriptions() -> None:
    info = plistlib.loads(NOTCH_PLIST.read_bytes())
    assert info["CFBundleName"] == info["CFBundleDisplayName"] == "FERB"
    assert info["CFBundleIdentifier"] == APP_BUNDLE_ID == "com.ferb.FERB"
    assert info["CFBundleExecutable"] == "FERBNotch" and info["CFBundlePackageType"] == "APPL"
    assert info["LSUIElement"] is True                                       # background first: no Dock icon
    assert info["NSMicrophoneUsageDescription"].strip() and info["NSSpeechRecognitionUsageDescription"].strip()
    voice = plistlib.loads((ROOT / "macapp" / "FERB" / "Info.plist").read_bytes())
    assert voice["CFBundleIdentifier"] != info["CFBundleIdentifier"]        # the nested voice helper keeps its own id


def test_the_development_build_keeps_its_own_identity() -> None:
    """`ui/mac/FERBNotch/build.sh` (the owner's dev notch) must not take the packaged app's bundle id, or macOS would
    mix up the two apps' permissions."""
    script = (ROOT / "ui" / "mac" / "FERBNotch" / "build.sh").read_text()
    assert "dev.ferb.notch" in script and "FERB Notch" in script


def test_build_scripts_are_executable_zsh_that_parses() -> None:
    for script in (BUILD_APP, BUILD_DMG, ROOT / "scripts" / "signing.sh"):
        assert script.is_file(), script
        assert subprocess.run(["/bin/zsh", "-n", str(script)], capture_output=True).returncode == 0, script
    assert os.access(BUILD_APP, os.X_OK) and os.access(BUILD_DMG, os.X_OK)


def test_cli_reports_its_version() -> None:
    out = subprocess.run([sys.executable, "-I", "-m", "ferb.cli", "--version"], capture_output=True, text=True,
                         cwd=ROOT)
    assert out.returncode == 0 and out.stdout.strip() == f"ferb {ferb.__version__}"


# ---------- the signing step, against stub tools ----------

STUB_CODESIGN = r"""#!/bin/zsh
# Logs every call. -dv: a "vendor" path carries another team's Developer ID signature, anything else is ad-hoc.
print -r -- "codesign|${(j:|:)@}" >> "$STUB_LOG"
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
  for (( i = 1; i < $#; i++ )); do [[ "${@[i]}" == -srcfolder ]] && ls -l "${@[i+1]}" >> "$STUB_DIR/staging.txt"; done
  print "fake dmg" > "${@[-1]}"
fi
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
            "STUB_LOG": str(tmp_path / "calls.log"), "STUB_DIR": str(tmp_path)}


def _fake_app(root: Path) -> Path:
    """An assembled FERB.app shaped like build-app.sh's output, with real Mach-O files (copies of /usr/bin/true)."""
    app = root / "FERB.app"
    c = app / "Contents"
    machos = ["MacOS/FERBNotch", "Resources/python/bin/python3.12", "Resources/python/lib/libpython3.12.dylib",
              "Resources/python/lib/python3.12/site-packages/pkg/_ext.cpython-312-darwin.so",
              "Resources/python/lib/python3.12/site-packages/vendor_sdk/_bundled/vendor-tool",
              "Helpers/ferb-secrets", "Helpers/FERB Voice.app/Contents/MacOS/FERB", "Helpers/FERB.app/Contents/MacOS/FERB"]
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
    (c / "Helpers/FERB Voice.app/Contents/Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "com.ferb.app"}))
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
                 str(c / "Helpers/FERB Voice.app"), str(c / "Helpers/FERB.app"), str(app)):
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
    assert "com.apple.security.device.audio-input" in _ent(tmp_path, c / "Helpers/FERB Voice.app")
    assert not any(s.endswith("/Helpers/FERB.app/Contents/MacOS/FERB") for s in signed)   # signed with its app
    assert any(c.startswith("codesign|--verify|--deep|--strict") and c.endswith(str(app)) for c in calls)
    assert not any(c.startswith("xcrun") for c in calls)                    # no notarization without a Developer ID


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


def test_dmg_holds_the_app_and_an_applications_link_and_is_notarized_only_with_a_developer_id(tmp_path: Path) -> None:
    app = _fake_app(tmp_path / "out")
    env = _stubs(tmp_path)
    dmg = tmp_path / "out" / "FERB.dmg"
    done = subprocess.run([str(BUILD_DMG), str(app), str(dmg)], capture_output=True, text=True, env=env)
    assert done.returncode == 0, done.stderr + done.stdout
    staging = (tmp_path / "staging.txt").read_text()
    assert "Applications -> /Applications" in staging and "FERB.app" in staging
    calls = (tmp_path / "calls.log").read_text().splitlines()
    assert any(c.startswith("hdiutil|create|") and "|-volname|FERB|" in c and c.endswith(str(dmg)) for c in calls)
    assert not any(c.startswith(("codesign", "xcrun")) for c in calls) and dmg.is_file()

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


def test_real_bundle_structure_signature_and_embedded_cli(tmp_path_factory: pytest.TempPathFactory,
                                                          tmp_path: Path) -> None:
    app = _real_bundle(tmp_path_factory)
    c = app / "Contents"
    info = plistlib.loads((c / "Info.plist").read_bytes())
    assert info["CFBundleIdentifier"] == APP_BUNDLE_ID and info["CFBundleShortVersionString"]
    assert info["CFBundleVersion"].isdigit()
    for rel in ("MacOS/FERBNotch", "Resources/python/bin/python3.12", "Resources/bin/ferb", "Helpers/ferb-secrets",
                "Helpers/FERB Voice.app/Contents/MacOS/FERB", "Helpers/FERB.app/Contents/MacOS/FERB",
                "Resources/node/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex"):
        assert (c / rel).is_file(), rel
    voice = plistlib.loads((c / "Helpers/FERB Voice.app/Contents/Info.plist").read_bytes())
    assert voice["CFBundleIdentifier"] == "com.ferb.app" and voice["NSSpeechRecognitionUsageDescription"]
    agent = plistlib.loads((c / "Helpers/FERB.app/Contents/Info.plist").read_bytes())
    assert (agent["CFBundleIdentifier"], agent["CFBundleName"]) == ("com.ferb.daemon", "FERB")   # what macOS shows
    assert agent["LSBackgroundOnly"] is True and agent["NSAppleEventsUsageDescription"]
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
    for argv in (["-c", "print('ran')"], ["-m", "os"], [], ["-m", "ferb.daemon", "x"]):   # only FERB's entry points
        refused = subprocess.run([str(stub), *argv], capture_output=True, text=True, env=env)
        assert refused.returncode == 64 and "ran" not in refused.stdout, argv
    foreign = tmp_path / "foreign-ferb"
    foreign.mkdir()
    refused = subprocess.run([str(stub), "-m", "ferb.daemon"], capture_output=True, text=True,
                             env={**env, "FERB_HOME": str(foreign)})
    assert refused.returncode == 64 and "FERB_HOME" in refused.stderr          # never another FERB home
    log = tmp_path / "events.jsonl"                                           # FERB's detector, through the agent
    detector = subprocess.Popen([str(stub), "-I", "-m", "ferb.device.detector", str(log)], cwd=tmp_path,
                                env={k: v for k, v in env.items() if k != "FERB_HOME"} |
                                {"PYTHONEXECUTABLE": str(tmp_path / "evil-shim"), "PYTHONPATH": str(tmp_path)},
                                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        import json
        import time
        deadline = time.monotonic() + 30
        while not (log.exists() and log.read_text().strip()) and time.monotonic() < deadline:
            time.sleep(0.1)
        started = json.loads(log.read_text().splitlines()[0])
    finally:
        detector.terminate()
        detector.wait(timeout=10)
    assert started["event"] == "started" and Path(started["executable"]).resolve() == stub.resolve()
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
    for rel in ("ferb/__init__.py", "ferb/cli/__init__.py", "ferb/device/__init__.py", "ferb/daemon/__init__.py"):
        (site / rel).parent.mkdir(parents=True, exist_ok=True)
        (site / rel).write_text("")
    report = ("import json, os, sys\nprint(json.dumps({'module': __name__, 'argv': sys.argv[1:], "
              "'executable': sys.executable, 'isolated': sys.flags.isolated, 'ferb_home': os.environ.get('FERB_HOME'), "
              "'python_env': sorted(k for k in os.environ if k.startswith('PYTHON') or k == '__PYVENV_LAUNCHER__')}))\n")
    for module in ("ferb/cli/mcp.py", "ferb/device/detector.py", "ferb/daemon/__main__.py"):
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
    for argv in (["-I", "-m", "ferb.cli.mcp"], ["-I", "-m", "ferb.device.detector", str(tmp_path / "e.jsonl")]):
        seen = json.loads(_stub(agent_stub, *argv).stdout)
        assert seen["module"] == "__main__" and seen["isolated"] == 1
    refused = (["-c", "print('ran')"], ["-m", "os"], ["-m", "ferb.cli.mcp"], ["-I", "-m", "ferb.cli"],
               [str(tmp_path / "script.py")], ["-m", "ferb.daemon", "--extra"], ["-I", "-m", "ferb.device.detector"],
               ["-I", "-m", "ferb.cli.mcp", "x"], ["-i", "-m", "ferb.daemon"], [])
    (tmp_path / "script.py").write_text("print('ran')\n")
    for argv in refused:
        r = _stub(agent_stub, *argv)
        assert r.returncode == 64 and "ran" not in r.stdout and "FERB" in r.stderr, argv


def test_the_agent_scrubs_python_variables_and_keeps_its_own_executable(agent_stub: Path, tmp_path: Path) -> None:
    import json
    r = _stub(agent_stub, "-I", "-m", "ferb.cli.mcp", PYTHONEXECUTABLE=str(tmp_path / "evil-shim"),
              __PYVENV_LAUNCHER__=str(tmp_path / "evil"), PYTHONPATH=str(tmp_path), PYTHONHOME=str(tmp_path),
              PYTHONINSPECT="1")
    seen = json.loads(r.stdout)
    assert Path(seen["executable"]).resolve() == agent_stub.resolve()
    assert seen["python_env"] == []                         # none reaches FERB or the processes it starts


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
    if owner.is_dir():                                      # the same folder by another path is the owner's
        link.symlink_to(owner)
        seen = json.loads(_stub(agent_stub, "-I", "-m", "ferb.cli.mcp", FERB_HOME=str(link)).stdout)
        assert seen["ferb_home"] == str(link)


def test_the_detector_runs_as_a_ferb_module(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    import ferb.device.watch as watch
    started: list[list[str]] = []

    class Proc:
        def __init__(self, argv: list[str], **kw: object) -> None:
            started.append(argv)

        def terminate(self) -> None:
            pass
    monkeypatch.setattr(watch.subprocess, "Popen", Proc)
    det = watch.Detector(tmp_path / "e.jsonl")
    det.stop()
    assert started == [[sys.executable, "-I", "-m", "ferb.device.detector", str(tmp_path / "e.jsonl")]]
    import ferb.device.detector as detector
    assert callable(detector.main)
