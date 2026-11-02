"""The secret helper's pinned code (REL-404, T-502). Release tooling, stdlib only; never part of FERB.app.

macOS's login keychain admits code that has no Apple Team ID (FERB's self-made release certificate) to a saved item
by that code's exact hash (the item's partition list holds "cdhash:<hash>"), so every change to ferb-secrets' bytes
makes macOS ask again, once for every saved secret, with the login password. Its bytes change with its code and with
the compiler and SDK that build it. So FERB.app never compiles the helper: build-app.sh takes the arm64 or x86_64
slice of assets/secret-helper/ferb-secrets, which this script builds from ferb/identity/secret_helper.swift and pins
in assets/secret-helper/pin.json. The same bytes signed with the same certificate, identifier and flags keep the same
code hash (measured), so an approval holds across releases until the helper's code changes on purpose.

    python3 -I scripts/secret_helper.py --check   # build-app.sh: the pin matches the source and the binary
    python3 -I scripts/secret_helper.py --build   # after changing the helper's code (every saved secret asks again)
"""

import argparse
import hashlib
import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "ferb/identity/secret_helper.swift"
FOLDER = ROOT / "assets/secret-helper"
BINARY, PIN = FOLDER / "ferb-secrets", FOLDER / "pin.json"
ARCHS = ("arm64", "x86_64")
TARGET = "macos14.2"                       # FERB.app's minimum macOS (build-app.sh builds everything for it)


def code(text: str) -> str:
    """The helper's code without comments or blank space at line ends and with no blank lines: a comment-only edit
    keeps the pin (it compiles to the same bytes, measured)."""
    out, i, n, quoted = [], 0, len(text), False
    while i < n:
        c = text[i]
        if quoted:
            out.append(c)
            if c == "\\" and i + 1 < n:
                out.append(text[i + 1])
                i += 1
            elif c == '"':
                quoted = False
        elif c == '"':
            quoted = True
            out.append(c)
        elif text.startswith("//", i):
            while i < n and text[i] != "\n":
                i += 1
            continue
        elif text.startswith("/*", i):
            end = text.find("*/", i + 2)
            i = n if end < 0 else end + 2
            continue
        else:
            out.append(c)
        i += 1
    return "\n".join(line.rstrip() for line in "".join(out).splitlines() if line.strip()) + "\n"


def code_sha256(text: str) -> str:
    return hashlib.sha256(code(text).encode()).hexdigest()


def problems(source: Path = SOURCE, binary: Path = BINARY, pin: Path = PIN) -> list[str]:
    """Why the pinned helper can't ship; empty when it can."""
    try:
        recorded = json.loads(pin.read_text())
        built = binary.read_bytes()
    except (OSError, ValueError) as e:
        return [f"the pinned secret helper is missing or unreadable ({e}): run scripts/secret_helper.py --build"]
    out = []
    if recorded.get("code_sha256") != code_sha256(source.read_text()):
        out.append("ferb/identity/secret_helper.swift's code changed since the helper was pinned: run "
                   "scripts/secret_helper.py --build and commit assets/secret-helper (every saved secret asks for "
                   "the owner's OK once more on each Mac)")
    if recorded.get("binary_sha256") != hashlib.sha256(built).hexdigest():
        out.append("assets/secret-helper/ferb-secrets isn't the binary pin.json records")
    return out


def _run(*args: str) -> str:
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout.strip()


def build() -> None:
    """Compile each slice as build-app.sh compiled it before (absolute source path mapped to ".", -O, stripped), join
    them, and record what built them."""
    text = SOURCE.read_text()
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        src = work / "src" / "ferb/identity/secret_helper.swift"
        src.parent.mkdir(parents=True)
        src.write_text(text)
        slices = []
        for arch in ARCHS:
            out = work / arch / "ferb-secrets"
            out.parent.mkdir()
            _run("xcrun", "swiftc", "-O", "-target", f"{arch}-apple-{TARGET}", "-file-prefix-map", f"{work / 'src'}=.",
                 "-o", str(out), str(src), "-framework", "Security", "-framework", "CryptoKit")
            _run("strip", "-S", str(out))
            slices.append(str(out))
        FOLDER.mkdir(parents=True, exist_ok=True)
        _run("lipo", "-create", "-output", str(BINARY), *slices)
        BINARY.chmod(0o644)                # data in the repository; build-app.sh makes the slice it ships executable
        record = {"code_sha256": code_sha256(text), "source_sha256": hashlib.sha256(text.encode()).hexdigest(),
                  "binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(),
                  "slices": {arch: hashlib.sha256(Path(s).read_bytes()).hexdigest() for arch, s in zip(ARCHS, slices)},
                  "swiftc": _run("xcrun", "swiftc", "--version").splitlines()[0],
                  "sdk": _run("xcrun", "--show-sdk-version"), "target": TARGET}
    PIN.write_text(json.dumps(record, indent=2) + "\n")
    print(f"pinned {BINARY.relative_to(ROOT)} ({', '.join(ARCHS)}); commit assets/secret-helper")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--check", action="store_true")
    action.add_argument("--build", action="store_true")
    if parser.parse_args().build:
        build()
        return 0
    found = problems()
    for problem in found:
        print(f"secret_helper.py: {problem}", file=sys.stderr)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
