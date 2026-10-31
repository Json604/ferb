"""A fake packaged FERB.app (L27) with the layout scripts/build-app.sh produces, its signature's seal of the nested
code, and a command runner that answers for those signatures. Nothing here signs, builds or installs anything real."""

import plistlib
import subprocess
from pathlib import Path

from tests.conftest import FakeRunner

from ferb.runtimes.codex_exec import _NATIVE_BIN as CODEX_BIN   # this Mac's architecture, as the installer looks
CDHASH = "8a144e4088377ef93330e517658dfe41eb6f997d"
PINNED = f'cdhash H"{CDHASH}"'
AGENT = Path("Contents/Helpers/FERB.app/Contents/MacOS/FERB")
NESTED = ("Helpers/ferb-secrets", "Helpers/FERB Voice.app", "Helpers/FERB.app")


def make_ferb_app(root: Path, build: str = "202610140201", bundle_id: str = "com.ferb.FERB",
                  sealed: str = CDHASH) -> Path:
    """`sealed`: the code hash FERB.app's signature records for each nested helper."""
    app = root / "FERB.app"
    contents = app / "Contents"
    for rel in ("MacOS/FERBNotch", "Resources/python/bin/python3.12", "Resources/bin/ferb", "Helpers/ferb-secrets",
                "Helpers/FERB Voice.app/Contents/MacOS/FERB", str(AGENT.relative_to("Contents")),
                "Resources/node/" + str(CODEX_BIN)):
        f = contents / rel
        f.parent.mkdir(parents=True, exist_ok=True)
        f.write_bytes(b"fake " + rel.encode())
        f.chmod(0o755)
    service = contents / "Library" / "LaunchAgents" / "com.ferb.daemon.plist"    # as scripts/build-app.sh copies it
    service.parent.mkdir(parents=True)
    definition = plistlib.loads((Path(__file__).parents[2] / "ui/mac/FERBAgent/com.ferb.daemon.plist").read_bytes())
    service.write_bytes(plistlib.dumps({**definition, "EnvironmentVariables": {"FERB_APP_BUILD": build}}))
    (contents / "Info.plist").write_bytes(plistlib.dumps({
        "CFBundleIdentifier": bundle_id, "CFBundleName": "FERB", "CFBundleExecutable": "FERBNotch",
        "CFBundleShortVersionString": "0.27.0", "CFBundleVersion": build}))
    (contents / "_CodeSignature").mkdir()
    (contents / "_CodeSignature" / "CodeResources").write_bytes(plistlib.dumps({"files2": {
        rel: {"cdhash": bytes.fromhex(sealed), "requirement": f'cdhash H"{sealed}"'} for rel in NESTED}}))
    return app


class BundleRunner(FakeRunner):
    """FakeRunner, plus codesign answers for FERB.app's code and copies of it: code hash `cdhash` and designated
    requirement `requirement` (ad-hoc by default), the app bundle's own requirement `app_requirement` (None: the same
    `requirement`), the app's Team ID `team` (None: ad-hoc); `bad_signature` makes `codesign --verify` fail; the
    linked Codex reports `codex_version`."""

    def __init__(self, home: Path, bad_signature: bool = False, codex_version: str = "", cdhash: str = CDHASH,
                 requirement: str = PINNED, team: str | None = None, app_requirement: str | None = None) -> None:
        super().__init__(home)
        self.bad_signature, self.codex_version = bad_signature, codex_version
        self.cdhash, self.requirement, self.team = cdhash, requirement, team
        self.app_requirement = app_requirement

    def __call__(self, cmd: list[str]) -> "subprocess.CompletedProcess[str]":
        if cmd[0].endswith("codesign") and "-r-" in cmd:
            self.cmds.append(cmd)
            is_app = Path(cmd[-1]).name == "FERB.app" and "/Contents/" not in cmd[-1]   # not the nested agent
            requirement = self.app_requirement if is_app and self.app_requirement is not None else self.requirement
            prefix = "# " if requirement.startswith("cdhash") else ""
            return subprocess.CompletedProcess(cmd, 0, "", f"Executable={cmd[-1]}\n{prefix}designated => "
                                                           f"{requirement}\n")
        if cmd[0].endswith("codesign") and "-dv" in cmd:
            self.cmds.append(cmd)
            signer = f"Authority=Developer ID Application: Owner ({self.team})\n" if self.team else "Signature=adhoc\n"
            return subprocess.CompletedProcess(cmd, 0, "", f"Executable={cmd[-1]}\nCDHash={self.cdhash}\n{signer}"
                                                           f"TeamIdentifier={self.team or 'not set'}\n")
        if cmd[0].endswith("codesign") and "--verify" in cmd and self.bad_signature:
            self.cmds.append(cmd)
            return subprocess.CompletedProcess(cmd, 1, "", f"{cmd[-1]}: a sealed resource is missing or invalid")
        if cmd[1:] == ["--version"] and cmd[0].endswith("/codex"):
            self.cmds.append(cmd)
            return subprocess.CompletedProcess(cmd, 0, self.codex_version, "")
        return super().__call__(cmd)
