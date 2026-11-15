"""Shared helpers for the release tooling's tests (scripts/tests): the checkout's paths, tiny real Mach-O files, zsh
run after sourcing the build's shell libraries, and the stub signing tools every signing test runs against (nothing
here signs with a real identity, reaches Apple's notary or touches a keychain). Stdlib only."""

from __future__ import annotations

import os
import plistlib
import re
import shlex
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
CRATES = ROOT / "engine" / "crates"
SIGN_APP = SCRIPTS / "sign-app.sh"
BUILD_APP = SCRIPTS / "build-app-rust.sh"
BUILD_DMG = SCRIPTS / "build-dmg.sh"
APP_PARTS = SCRIPTS / "app-parts.sh"
RUST_PARTS = SCRIPTS / "rust-parts.sh"
SYSTEM_PATH = "/usr/bin:/bin:/usr/sbin:/sbin"


class TempCase(unittest.TestCase):
    """A test with its own temporary folder, `self.tmp`, removed afterwards."""

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory(prefix="ferb-scripts-")
        self.tmp = Path(self._tmp.name).resolve()
        self.addCleanup(self._tmp.cleanup)


# ---------- tiny real Mach-O files ----------

SOURCE = "int main(void) { return 0; }\n"


def clang() -> list[str]:
    if shutil.which("clang"):
        return ["clang"]
    if shutil.which("xcrun"):
        return ["xcrun", "clang"]
    raise unittest.SkipTest("needs clang (Xcode or the Command Line Tools)")


def compile_macho(dest: Path, *archs: str) -> Path:
    """`dest`, an executable built for `archs` (two or more: a universal file); replaces whatever is there."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    src = dest.with_name(dest.name + ".c")
    src.write_text(SOURCE)
    flags = [flag for arch in archs for flag in ("-arch", arch)]
    subprocess.run([*clang(), *flags, "-o", str(dest), str(src)], check=True, capture_output=True)
    src.unlink()
    return dest


def sized(path: Path, size: int) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"\0" * size)
    return path


def lipo_archs(path: Path) -> list[str]:
    return subprocess.run(["lipo", "-archs", str(path)], capture_output=True, text=True).stdout.split()


# ---------- the shell libraries ----------

def parts(code: str) -> subprocess.CompletedProcess[str]:
    """zsh code run after sourcing scripts/app-parts.sh, in a clean zsh with the system PATH."""
    return subprocess.run(["/bin/zsh", "-f", "-c", f"source {shlex.quote(str(APP_PARTS))}; {code}"],
                          capture_output=True, text=True, env={"PATH": SYSTEM_PATH})


def zsh(code: str, **env: str) -> subprocess.CompletedProcess[str]:
    """zsh code run after sourcing app-parts.sh, signing.sh (ad hoc) and rust-parts.sh, with the system PATH."""
    src = " ".join(f"source {shlex.quote(str(SCRIPTS / s))};" for s in ("app-parts.sh", "signing.sh", "rust-parts.sh"))
    return subprocess.run(["/bin/zsh", "-f", "-c", f"ROOT={shlex.quote(str(ROOT))}; {src} {code}"],
                          capture_output=True, text=True,
                          env={"PATH": SYSTEM_PATH, "HOME": "/nonexistent-home", **env})


def zsh_vars(case: unittest.TestCase, code: str, *names: str) -> dict[str, str]:
    done = zsh(code + "; " + " ".join(f"print -r -- \"{n}=${n}\";" for n in names))
    case.assertEqual(done.returncode, 0, done.stderr)
    return dict(line.split("=", 1) for line in done.stdout.splitlines() if "=" in line)


# ---------- the Rust sources, read directly ----------

def rust_const(path: Path, name: str) -> str:
    """The value of `pub const NAME: &str = "…";` in a Rust source (exactly one)."""
    found = re.findall(rf'^\s*pub const {name}: &str = "([^"]*)";', path.read_text(), re.M)
    if len(found) != 1:
        raise AssertionError(f"{path} names {name} {len(found)} times")
    return found[0]


def workspace_version() -> str:
    """engine/Cargo.toml's [workspace.package] version (read without tomllib, which Python 3.9 lacks)."""
    text = (ROOT / "engine" / "Cargo.toml").read_text()
    section = re.search(r"^\[workspace\.package\]\s*$(.*?)(?=^\[|\Z)", text, re.M | re.S)
    if section is None:
        raise AssertionError("engine/Cargo.toml has no [workspace.package]")
    found = re.findall(r'^version\s*=\s*"([^"]*)"\s*$', section.group(1), re.M)
    if len(found) != 1:
        raise AssertionError("engine/Cargo.toml's [workspace.package] names no single version")
    return found[0]


def version_tuple(v: str) -> tuple[int, ...]:
    """A version's numbers (ferb-browser protocol.rs version_tuple: digits per dot-separated part, 0 when none)."""
    return tuple(int("".join(ch for ch in part if ch.isdigit()) or "0") for part in v.split("."))


# ---------- the signing step, against stub tools ----------

DEV_ID = "Developer ID Application: Test Owner (TEAM123456)"

STUB_CODESIGN = r"""#!/bin/zsh
# Logs every call. -dv: a "vendor" path carries another team's Developer ID signature, anything else is ad-hoc.
print -r -- "codesign|${(j:|:)@}" >> "$STUB_LOG"
if [[ " $* " == *" --verify "* && -n "${STUB_VERIFY_FAILS:-}" ]]; then exit 1; fi
# STUB_UNSIGNED: a path that carries no signature; STUB_ADHOC: one signed, but by no team (any -R check fails)
if [[ " $* " == *" --verify "* && "${@[-1]}" == "${STUB_UNSIGNED:-//}" ]]; then
  print -u2 "${@[-1]}: code object is not signed at all"; exit 1
fi
if [[ " $* " == *" --verify "* && " $* " == *" -R="* && "${@[-1]}" == "${STUB_ADHOC:-//}" ]]; then exit 3; fi
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
if [[ "$1" == stapler && "$2" == validate && -n "${STUB_STAPLE_MISSING:-}" ]]; then exit 65; fi
exit 0
"""
# Gatekeeper's assessment: notarized Developer ID code of TEAM123456 unless the test says otherwise
STUB_SPCTL = r"""#!/bin/zsh
print -r -- "spctl|${(j:|:)@}" >> "$STUB_LOG"
if [[ -n "${STUB_SPCTL_REJECT:-}" ]]; then
  print -u2 "${@[-1]}: rejected"; print -u2 "source=Unnotarized Developer ID"; exit 3
fi
print -u2 "${@[-1]}: accepted"; print -u2 "source=Notarized Developer ID"
print -u2 "origin=Developer ID Application: Test Owner (${STUB_SPCTL_TEAM:-TEAM123456})"
"""
STUB_HDIUTIL = r"""#!/bin/zsh
print -r -- "hdiutil|${(j:|:)@}" >> "$STUB_LOG"
if [[ "$1" == create ]]; then
  for (( i = 1; i < $#; i++ )); do
    if [[ "${@[i]}" == -srcfolder ]]; then
      ls -lA "${@[i+1]}" >> "$STUB_DIR/staging.txt"
      (cd "${@[i+1]}" && find . -mindepth 1 -maxdepth 2 ! -path './FERB.app/*' | sort) > "$STUB_DIR/staged.txt"
    fi
  done
  print "fake dmg" > "${@[-1]}"
fi
for (( i = 1; i < $#; i++ )); do
  [[ "$1" == attach && "${@[i]}" == -mountpoint ]] && mkdir -p "${@[i+1]}"
  [[ "$1" == convert && "${@[i]}" == -o ]] && cp "$2" "${@[i+1]}"
done
if [[ "$1" == detach ]]; then
  xattr -px com.apple.FinderInfo "$2" | tr -d ' \n' >> "$STUB_DIR/volume-flags.txt"
  ls -A "$2" > "$STUB_DIR/mounted.txt"
  rm -rf "$2"
fi
exit 0
"""
# dmg_layout.py, run as `$FERB_DMG_PYTHON -I dmg_layout.py ds-store MOUNT`: logged, and a .DS_Store left on the mount
STUB_LAYOUT_PYTHON = r"""#!/bin/zsh
print -r -- "layout|${(j:|:)@}" >> "$STUB_LOG"
if [[ "$3" == ds-store ]]; then : > "$4/.DS_Store"; fi
exit 0
"""


def stubs(tmp: Path) -> dict[str, str]:
    """The stub tools in tmp/stubs, first on PATH; the environment that runs a script against them."""
    folder = tmp / "stubs"
    folder.mkdir(parents=True)
    for name, body in (("codesign", STUB_CODESIGN), ("xcrun", STUB_XCRUN), ("hdiutil", STUB_HDIUTIL),
                       ("security", STUB_SECURITY), ("layout-python", STUB_LAYOUT_PYTHON), ("spctl", STUB_SPCTL)):
        (folder / name).write_text(body)
        (folder / name).chmod(0o755)
    return {"PATH": f"{folder}:{SYSTEM_PATH}", "HOME": str(tmp / "home"),
            "STUB_LOG": str(tmp / "calls.log"), "STUB_DIR": str(tmp), "FERB_DEV_DMG": "1",
            "FERB_DMG_PYTHON": str(folder / "layout-python")}


# The pieces of the Rust FERB.app: Mach-O files (copies of /usr/bin/true, a universal x86_64 + arm64e file); Codex's
# binary sits under a "vendor" path, which the stub codesign reports as another team's Developer ID (kept as is)
FAKE_MACHOS = ("MacOS/FERBNotch", "Helpers/FERB.app/Contents/MacOS/FERB", "Helpers/ferb-native-host",
               "Helpers/ferb-secrets", "Resources/bin/ferb",
               "Resources/node/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex",
               "Resources/node/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/lib/libtool.dylib")


def fake_app(root: Path) -> Path:
    """An assembled FERB.app shaped like build-app-rust.sh's output, with real Mach-O files."""
    app = root / "FERB.app"
    c = app / "Contents"
    for rel in FAKE_MACHOS:
        (c / rel).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy("/usr/bin/true", c / rel)
    (c / "Resources/ferb/memory").mkdir(parents=True)
    (c / "Resources/ferb/memory/profile.md").write_text("# profile\n")
    (c / "Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "com.ferb.FERB",
                                                    "CFBundleShortVersionString": "0.28.0",
                                                    "CFBundleVersion": "202611150201"}))
    (c / "Helpers/FERB.app/Contents/Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "com.ferb.daemon"}))
    return app


def ent(tmp: Path, signed: Path) -> str:
    """The entitlements the stub codesign was given for `signed`."""
    return (tmp / ("ent" + str(signed).replace("/", "_"))).read_text()


def signed_paths(calls: list[str]) -> list[str]:
    """The paths signed, in order (codesign calls that write a signature)."""
    return [c.split("|")[-1] for c in calls if c.startswith("codesign") and "|--sign|" in c and "--verify" not in c]


def read_calls(tmp: Path) -> list[str]:
    log = tmp / "calls.log"
    return log.read_text().splitlines() if log.exists() else []


def sign_only(tmp: Path, **env: str) -> tuple[Path, list[str], subprocess.CompletedProcess[str]]:
    """A fake FERB.app signed again by scripts/sign-app.sh against the stubs."""
    app = fake_app(tmp / "out")
    done = subprocess.run([str(SIGN_APP), str(app)], capture_output=True, text=True, env={**stubs(tmp), **env})
    return app, read_calls(tmp), done


SHA = "0123456789abcdef0123456789abcdef01234567"
RELEASE_LISTED = f'  1) {SHA.upper()} "FERB Release Signing" (CSSMERR_TP_NOT_TRUSTED)'
# Each piece macOS grants permissions to (signing.sh's list)
RELEASE_PIECES = (("", "com.ferb.FERB"), ("Contents/Helpers/FERB.app", "com.ferb.daemon"),
                  ("Contents/Helpers/ferb-secrets", "com.ferb.secrets"))


def release_env() -> dict[str, str]:
    """SIGN_IDENTITY=release with the stub keychain listing FERB's release certificate (self-made: never trusted)."""
    return {"SIGN_IDENTITY": "release", "FERB_TEST_RELEASE_SHA1": SHA, "STUB_IDENTITIES": RELEASE_LISTED}


# The devid mode (T-27): the stub keychain lists the team's Developer ID Application identity among others of the same
# team or name that must never be picked
TEAM = "TEAM123456"
DEVID_SHA = "FEDCBA9876543210FEDCBA9876543210FEDCBA98"
DEVID_LISTED = "\n".join((f'  1) {"A" * 40} "Apple Development: Test Owner ({TEAM})"',
                          f'  2) {DEVID_SHA} "{DEV_ID}"',
                          f'  3) {"B" * 40} "Developer ID Installer: Test Owner ({TEAM})"',
                          f'  4) {"C" * 40} "Developer ID Application: Test Owner (OTHER12345)"'))


def team_requirement(ident: str) -> str:
    return f'identifier "{ident}" and anchor apple generic and certificate leaf[subject.OU] = "{TEAM}"'


def devid_env() -> dict[str, str]:
    return {"SIGN_IDENTITY": "devid", "FERB_TEST_TEAM_ID": TEAM, "STUB_IDENTITIES": DEVID_LISTED,
            "NOTARY_KEYCHAIN_PROFILE": "ferb-notary"}


def run(argv: list[str], env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run([str(a) for a in argv], capture_output=True, text=True, env=env)


def executable(path: Path) -> bool:
    return os.access(path, os.X_OK)
