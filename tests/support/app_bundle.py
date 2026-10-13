"""A fake packaged FERB.app (L27) with the layout scripts/build-app.sh produces, and a command runner that answers
for its ad-hoc signatures. Nothing here signs, builds or installs anything real."""

import plistlib
import subprocess
from pathlib import Path

from tests.conftest import FakeRunner

CODEX_BIN = Path("node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex")
PINNED = 'cdhash H"8a144e4088377ef93330e517658dfe41eb6f997d"'


def make_ferb_app(root: Path, build: str = "202610140201", bundle_id: str = "com.ferb.FERB") -> Path:
    app = root / "FERB.app"
    contents = app / "Contents"
    for rel in ("MacOS/FERBNotch", "Resources/python/bin/python3.12", "Resources/bin/ferb", "Helpers/ferb-secrets",
                "Helpers/FERB Voice.app/Contents/MacOS/FERB", "Resources/node/" + str(CODEX_BIN)):
        f = contents / rel
        f.parent.mkdir(parents=True, exist_ok=True)
        f.write_bytes(b"fake " + rel.encode())
        f.chmod(0o755)
    (contents / "Info.plist").write_bytes(plistlib.dumps({
        "CFBundleIdentifier": bundle_id, "CFBundleName": "FERB", "CFBundleExecutable": "FERBNotch",
        "CFBundleShortVersionString": "0.27.0", "CFBundleVersion": build}))
    return app


class BundleRunner(FakeRunner):
    """FakeRunner, plus: FERB.app's code (and copies of it) reports an ad-hoc designated requirement (a pinned code
    hash), `bad_signature` makes `codesign --verify` fail, and the linked Codex reports `codex_version`."""

    def __init__(self, home: Path, bad_signature: bool = False, codex_version: str = "") -> None:
        super().__init__(home)
        self.bad_signature, self.codex_version = bad_signature, codex_version

    def __call__(self, cmd: list[str]) -> "subprocess.CompletedProcess[str]":
        if cmd[0].endswith("codesign") and "-r-" in cmd:
            self.cmds.append(cmd)
            return subprocess.CompletedProcess(cmd, 0, "", f"Executable={cmd[-1]}\n# designated => {PINNED}\n")
        if cmd[0].endswith("codesign") and "--verify" in cmd and self.bad_signature:
            self.cmds.append(cmd)
            return subprocess.CompletedProcess(cmd, 1, "", f"{cmd[-1]}: a sealed resource is missing or invalid")
        if cmd[1:] == ["--version"] and cmd[0].endswith("/codex"):
            self.cmds.append(cmd)
            return subprocess.CompletedProcess(cmd, 0, self.codex_version, "")
        return super().__call__(cmd)
