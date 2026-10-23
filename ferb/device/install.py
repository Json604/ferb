"""Own daemon bundle and launch-agent installation mechanics. Must not decide app targets or task behavior.

Dedicated interpreter + venv + LaunchAgent, `ferb setup --update`, and uninstall.

- The interpreter lives in ~/.ferb/python (uv-managed, used only by FERB). TCC grants attach to it (spike 8/15).
- FERB is installed into ~/.ferb/venv as a copy (not a link to the repo): the daemon never runs code from the
  repo, and only changes when `ferb setup --update` reinstalls it.
- From a packaged FERB.app (L27) nothing is built or downloaded: launchd starts the app's nested agent app (FERB, a
  native stub embedding the app's own Python), so macOS names FERB in permission prompts and Privacy & Security; the
  secret helper and the pinned Codex come from the app, exactly as its signature seals them; `ferb` is a link to the
  app's launcher. The home, logs and data are the same ~/.ferb, so switching between the two keeps everything.
- Uninstall removes only FERB's own things; `--purge` refuses a folder without the `.ferb-home` marker."""

import hashlib
import json
import os
import plistlib
import re
import shlex
import shutil
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

import ferb
from ferb.core.errors import FerbError
from ferb.paths import app_bundle as app_bundle   # FERB.app's own location (setup and the daemon ask it here)
from ferb.paths import BUNDLE_PYTHON as BUNDLE_PYTHON   # its plain interpreter (python3.12)

LABEL = "com.ferb.daemon"
PYTHON_VERSION = "3.12"
MARKER = ".ferb-home"
NATIVE_HOST_NAME = "com.ferb.native_host"            # extensions/chromium/protocol.js FERB_HOST
NATIVE_HOST_MANIFEST = NATIVE_HOST_NAME + ".json"

# The packaged app's layout (scripts/build-app.sh builds it; ui/mac/FERBNotch/Info.plist names it).
APP_BUNDLE_ID = "com.ferb.FERB"
BUNDLE_CLI = Path("Contents/Resources/bin/ferb")
BUNDLE_NODE = Path("Contents/Resources/node")                 # holds node_modules/@openai/codex-darwin-arm64
BUNDLE_SECRET_HELPER = Path("Contents/Helpers/ferb-secrets")
BUNDLE_VOICE_APP = Path("Contents/Helpers/FERB Voice.app")    # macapp/FERB, nested until it merges into the app
# The daemon: an app named FERB whose executable embeds the bundled Python, always isolated (no PYTHON* variables,
# no user site-packages, no working folder on sys.path) and never writing bytecode into the sealed bundle
BUNDLE_AGENT_APP = Path("Contents/Helpers/FERB.app")
BUNDLE_AGENT = BUNDLE_AGENT_APP / "Contents" / "MacOS" / "FERB"
# The same isolation for the app's plain interpreter (the `ferb` command and the native host)
BUNDLE_PYTHON_FLAGS = ("-I", "-B")

Runner = Callable[[list[str]], "subprocess.CompletedProcess[str]"]


def default_runner(cmd: list[str]) -> "subprocess.CompletedProcess[str]":
    return subprocess.run(cmd, capture_output=True, text=True)


class InstallError(FerbError):
    pass


def repo_root() -> Path:
    return Path(ferb.__file__).resolve().parent.parent


def voice_app(home: Path, bundle: Path | None) -> Path:
    """FERB's voice/speech app: nested in FERB.app when FERB runs from one, else built into ~/.ferb by setup."""
    return bundle / BUNDLE_VOICE_APP if bundle else home / "FERB.app"


def designated_requirement(codesign_output: str) -> str:
    """The designated requirement from `codesign -d -r-`; ad-hoc code reports its implicit one as a comment."""
    for line in codesign_output.splitlines():
        for prefix in ("designated =>", "# designated =>"):
            if line.startswith(prefix):
                return line[len(prefix):].strip()
    return ""


def sealed_cdhash(bundle: Path, nested: str) -> str | None:
    """The code hash FERB.app's own signature records for a nested helper (`Helpers/ferb-secrets`), from its seal."""
    try:
        seal = plistlib.loads((bundle / "Contents" / "_CodeSignature" / "CodeResources").read_bytes())
        value = seal["files2"][nested]["cdhash"]
    except (OSError, KeyError, TypeError, plistlib.InvalidFileException):
        return None
    return value.hex() if isinstance(value, bytes) else None


def codesign_field(codesign_output: str, name: str) -> str:
    """A `Name=value` line of `codesign -dv`; empty when absent."""
    return next((line.split("=", 1)[1].strip() for line in codesign_output.splitlines()
                 if line.startswith(name + "=")), "")


def trusted_requirement(requirement: str, *, pinned_ok: bool) -> bool:
    """A requirement that identifies the code: a signing certificate (the local one, or a Developer ID), or, for
    FERB.app's ad-hoc build, the exact code hash. An identifier alone proves nothing."""
    if "certificate leaf" in requirement:
        return True
    return pinned_ok and re.fullmatch(r'cdhash H"[0-9a-f]{40}"', requirement) is not None


def read_only_volume(path: Path) -> bool:
    return bool(os.statvfs(path).f_flag & os.ST_RDONLY)


def code_hash(package_dir: Path) -> str:
    """Hash code and packaged resources, canonicalizing the checkout's external extension folder."""
    h = hashlib.sha256()
    resources = {str(f.relative_to(package_dir)): f for f in package_dir.rglob('*')
                 if f.is_file() and '__pycache__' not in f.parts and f.name != '.DS_Store'
                 and f.suffix not in ('.pyc', '.pyo')}
    extension = package_dir.parent / 'extensions' / 'chromium'
    if (extension / 'manifest.json').is_file():
        resources = {key: path for key, path in resources.items() if not key.startswith('_extension/')}
        resources.update({'_extension/' + str(f.relative_to(extension)): f for f in extension.rglob('*')
                          if f.is_file() and f.name != '.DS_Store'})
    for name, f in sorted(resources.items()):
        h.update(name.encode() + b'\0')
        h.update(f.read_bytes())
        h.update(b'\0')
    return h.hexdigest()[:16]


@dataclass
class Installer:
    home: Path
    label: str = LABEL
    run: Runner = default_runner
    launch_agents: Path = Path.home() / "Library" / "LaunchAgents"
    native_hosts_dirs: tuple[Path, ...] = ()      # where the native-host manifest goes (the CLI passes the real ones)
    extension_ids: tuple[str, ...] = ()
    bundle: Path | None = None                    # the FERB.app to install from; None: from a checkout
    cli_dir: Path | None = None                   # where `ferb` is linked from FERB.app (the CLI passes ~/.local/bin)
    read_only: Callable[[Path], bool] = read_only_volume

    # ---------- paths ----------

    @property
    def python_dir(self) -> Path:
        return self.home / "python"

    @property
    def venv(self) -> Path:
        return self.home / "venv"

    @property
    def venv_python(self) -> Path:
        return self.venv / "bin" / "python"

    @property
    def plist_path(self) -> Path:
        return self.launch_agents / f"{self.label}.plist"

    @property
    def domain(self) -> str:
        return f"gui/{os.getuid()}"

    def interpreter(self) -> Path | None:
        found = sorted(self.python_dir.glob(f"cpython-{PYTHON_VERSION}*/bin/python{PYTHON_VERSION}"))
        return found[-1] if found else None

    @property
    def daemon_python(self) -> Path:
        """What runs the daemon: FERB.app's agent (it embeds the app's Python), or the venv's python."""
        return self.bundle / BUNDLE_AGENT if self.bundle else self.venv_python

    def python_command(self) -> list[str]:
        """The interpreter for FERB's other processes (the native host)."""
        return [str(self.bundle / BUNDLE_PYTHON), *BUNDLE_PYTHON_FLAGS] if self.bundle else [str(self.venv_python)]

    def daemon_command(self) -> list[str]:
        return [str(self.daemon_python), "-m", "ferb.daemon"]

    def granted_binary(self) -> Path | None:
        """The real executable TCC sees (the venv's python resolves to the dedicated interpreter)."""
        return self.daemon_python.resolve() if self.daemon_python.exists() else None

    def grant_target(self) -> Path | None:
        """What the owner switches on in Privacy & Security: the agent app FERB, or the dedicated interpreter."""
        return self.bundle / BUNDLE_AGENT_APP if self.bundle else self.granted_binary()

    def active_program(self) -> str | None:
        """What the installed LaunchAgent runs now (another installation's, maybe)."""
        try:
            args = plistlib.loads(self.plist_path.read_bytes()).get("ProgramArguments") or [None]
        except (OSError, plistlib.InvalidFileException):
            return None
        return str(args[0]) if args[0] else None

    def bundle_info(self) -> dict[str, object]:
        if self.bundle is None:
            return {}
        info: dict[str, object] = plistlib.loads((self.bundle / "Contents" / "Info.plist").read_bytes())
        return info

    # ---------- steps ----------

    def write_marker(self) -> None:
        self.home.mkdir(parents=True, exist_ok=True)
        self.home.chmod(0o700)
        (self.home / MARKER).write_text("This folder is FERB's home. `ferb uninstall --purge` removes it.\n")

    def ensure_interpreter(self) -> Path:
        existing = self.interpreter()
        if existing:
            return existing
        self._ok(["uv", "python", "install", PYTHON_VERSION, "--install-dir", str(self.python_dir)],
                 "installing FERB's dedicated Python")
        found = self.interpreter()
        if found is None:
            raise InstallError(f"uv installed Python but no python{PYTHON_VERSION} found under {self.python_dir}")
        return found

    def ensure_venv(self, interpreter: Path) -> None:
        if self.venv_python.exists() and self.venv_python.resolve() == interpreter.resolve():
            return
        if self.venv.exists():
            shutil.rmtree(self.venv)                  # built on another interpreter: rebuild on FERB's own
        self._ok(["uv", "venv", "--quiet", "--python", str(interpreter), str(self.venv)], "creating FERB's venv")

    def ensure_codex(self) -> None:
        """Pinned Codex in ~/.ferb/node (the daemon never uses a global Codex). Skipped if already pinned. From
        FERB.app, ~/.ferb/node becomes a link to the copy the app ships; npm never writes through such a link."""
        from ferb.runtimes.codex_exec import CODEX_VERSION, _NATIVE_BIN
        node = self.home / "node"
        binary = node / _NATIVE_BIN
        if not self.bundle and node.is_symlink():
            node.unlink()                             # a link into FERB.app: a checkout keeps its own pinned Codex
        if binary.exists() and self.run([str(binary), "--version"]).stdout.split()[-1:] == [CODEX_VERSION]:
            return
        if self.bundle:
            bundled = self.bundle / BUNDLE_NODE
            if not (bundled / _NATIVE_BIN).is_file():
                raise InstallError(f"{self.bundle} has no pinned Codex; download FERB again")
            if node.is_symlink() or node.is_file():
                node.unlink()
            elif node.exists():
                shutil.rmtree(node)                   # FERB's own npm install of another Codex version
            node.symlink_to(bundled)
            return
        self._ok(["npm", "install", "--prefix", str(node), "--save-exact", "--no-fund", "--no-audit",
                  f"@openai/codex@{CODEX_VERSION}"], "installing the pinned Codex")

    def install_ferb(self, source: Path | None = None) -> None:
        """A copy (non-editable), reinstalled every time: that's what `--update` means."""
        self._ok(["uv", "pip", "install", "--quiet", "--python", str(self.venv_python), "--reinstall-package", "ferb",
                  str(source or repo_root())], "installing FERB into its venv")
        from ferb.device.extensions import install_extension
        install_extension(self.home)

    def plist(self) -> dict[str, object]:
        logs = self.home / "logs"
        env = {"FERB_HOME": str(self.home)}
        if self.bundle:                     # which app build runs (the agent itself keeps the daemon's Python isolated)
            env["FERB_APP_BUILD"] = str(self.bundle_info().get("CFBundleVersion", ""))
        agent: dict[str, object] = {
            "Label": self.label,
            "ProgramArguments": self.daemon_command(),
            "EnvironmentVariables": env,
            "WorkingDirectory": str(self.home),
            "RunAtLoad": True,
            "KeepAlive": {"SuccessfulExit": False},       # restart after a crash, not after a clean exit
            "ThrottleInterval": 5,
            "ProcessType": "Interactive",
            "LimitLoadToSessionType": "Aqua",             # the owner's GUI session: Accessibility needs it
            "StandardOutPath": str(logs / "daemon.out.log"),
            "StandardErrorPath": str(logs / "daemon.err.log"),
        }
        if self.bundle:                     # System Settings → Login Items lists the agent under FERB (launchd.plist(5))
            agent["AssociatedBundleIdentifiers"] = [str(self.bundle_info().get("CFBundleIdentifier", APP_BUNDLE_ID))]
        return agent

    def write_plist(self) -> bool:
        """Returns True if the plist changed."""
        (self.home / "logs").mkdir(parents=True, exist_ok=True)
        self.launch_agents.mkdir(parents=True, exist_ok=True)
        data = plistlib.dumps(self.plist())
        if self.plist_path.exists() and self.plist_path.read_bytes() == data:
            return False
        self.plist_path.write_bytes(data)
        return True

    def loaded(self) -> bool:
        return self.run(["launchctl", "print", f"{self.domain}/{self.label}"]).returncode == 0

    def agent_pid(self) -> int | None:
        out = self.run(["launchctl", "print", f"{self.domain}/{self.label}"]).stdout
        for line in out.splitlines():
            if line.strip().startswith("pid = "):
                return int(line.split("=")[1])
        return None

    def load(self) -> None:
        self.unload()
        self._ok(["launchctl", "bootstrap", self.domain, str(self.plist_path)], "loading the LaunchAgent")

    def unload(self) -> None:
        self.run(["launchctl", "bootout", f"{self.domain}/{self.label}"])
        end = time.monotonic() + 10
        while self.loaded() and time.monotonic() < end:
            time.sleep(0.1)

    def restart(self) -> None:
        self._ok(["launchctl", "kickstart", "-k", f"{self.domain}/{self.label}"], "restarting the daemon")

    def install(self, update_only: bool = False, while_stopped: Callable[[], None] | None = None) -> None:
        """`while_stopped`: work that must run while no daemon does (setup's one-time store clean-out), run once
        everything is installed, between stopping the agent and starting it again (`_start`)."""
        if self.bundle:
            self.check_bundle()                       # before anything is written
            self.write_marker()
            self._install_from_bundle(update_only, while_stopped)
            return
        self.write_marker()
        if update_only:
            if not self.venv_python.exists():
                raise InstallError("FERB isn't installed yet: run `ferb setup` first")
            self.install_ferb()
            self.install_secret_helper()
            self.ensure_codex()
            self.write_native_host()
            # FERB.app may have pointed launchd at itself since: the agent runs this installation again
            self._start(self.write_plist(), while_stopped)
            return
        interpreter = self.ensure_interpreter()
        self.ensure_venv(interpreter)
        self.install_ferb()
        self.install_secret_helper()
        self.ensure_codex()
        self.write_native_host()
        self._start(self.write_plist(), while_stopped)

    def _install_from_bundle(self, update_only: bool, while_stopped: Callable[[], None] | None = None) -> None:
        """Nothing is built or downloaded: everything comes from the checked, signed FERB.app."""
        if update_only and not self.plist_path.exists():
            raise InstallError("FERB isn't installed yet: run `ferb setup` first")
        from ferb.device.extensions import install_extension
        install_extension(self.home)
        self.install_secret_helper()
        self.ensure_codex()
        self.write_native_host()
        self.link_cli()
        self._start(self.write_plist(), while_stopped)

    def _start(self, changed: bool, while_stopped: Callable[[], None] | None) -> None:
        """The daemon on what was just installed: the agent loaded when its plist `changed` or it isn't loaded, else
        restarted. With `while_stopped`, the agent is unloaded first, that runs, and the agent is loaded again whatever
        it does (an error or a stop included)."""
        if while_stopped is not None:
            self.unload()
            try:
                while_stopped()
            finally:
                self.load()
        elif changed or not self.loaded():
            self.load()
        else:
            self.restart()

    def check_bundle(self) -> None:
        """The daemon runs from this FERB.app, so it must stay where it is and be the app as signed: not macOS's
        temporary copy (App Translocation), not the mounted disk image, not damaged or modified."""
        assert self.bundle is not None
        if "AppTranslocation" in self.bundle.parts:
            raise InstallError("macOS is running FERB from a temporary copy: quit FERB, move FERB.app to Applications "
                               "in Finder, then open it again")
        if self.read_only(self.bundle):
            raise InstallError("FERB is running from its disk image: drag FERB.app to Applications, then open it from "
                               "there")
        for part in (BUNDLE_AGENT, BUNDLE_PYTHON):
            if not (self.bundle / part).is_file():
                raise InstallError(f"{self.bundle} is incomplete (no {part}); download FERB again")
        self._ok(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(self.bundle)],
                 "checking FERB.app's signature (download FERB again if it was damaged)")

    def install_secret_helper(self) -> None:
        """Owner-invoked setup only. A checkout builds it and signs it with the stable local code-signing identity
        (no ad-hoc/unsigned fallback). FERB.app ships it signed; its requirement is then the app's signing
        certificate's team (Developer ID) or, ad-hoc, the exact code hash, and only for the very helper FERB.app's
        signature seals (sealed_requirement)."""
        binary = self.home / 'bin' / 'ferb-secrets'
        binary.parent.mkdir(parents=True, exist_ok=True)
        if self.bundle:
            staged = binary.with_name('.ferb-secrets.new')
            shutil.copyfile(self.bundle / BUNDLE_SECRET_HELPER, staged)   # a byte copy keeps the embedded signature
            try:
                requirement = self.sealed_requirement(staged, 'Helpers/ferb-secrets')
            except InstallError:
                staged.unlink(missing_ok=True)
                raise
            os.replace(staged, binary)
            self._record_requirement(binary, requirement)
            return
        source = Path(ferb.__file__).parent / 'identity' / 'secret_helper.swift'
        self._ok(['xcrun', 'swiftc', '-O', '-o', str(binary), str(source), '-framework', 'Security',
                  '-framework', 'CryptoKit'], 'building the secret helper')
        self._ok(['/usr/bin/codesign', '--force', '--sign', 'FERB Local Code Signing', '--identifier',
                  'com.ferb.secrets', str(binary)], 'signing the secret helper (create FERB Local Code Signing in Keychain Access if missing)')
        shown = self.run(['/usr/bin/codesign', '-d', '-r-', str(binary)])
        requirement = designated_requirement((shown.stdout or '') + (shown.stderr or ''))
        if not trusted_requirement(requirement, pinned_ok=False):
            raise InstallError('the secret helper lacks a stable certificate identity')
        self._record_requirement(binary, requirement)

    def sealed_requirement(self, code: Path, nested: str) -> str:
        """The requirement to pin for `code`, a copy of FERB.app's nested `nested` (check_bundle verified the app's own
        signature first): only if it is exactly the code that signature seals (the same code hash), and then its
        code hash (ad-hoc) or a Developer ID requirement naming FERB.app's own team. A merely valid signature, or
        another team's, is refused."""
        assert self.bundle is not None
        sealed = sealed_cdhash(self.bundle, nested)
        shown = self.run(['/usr/bin/codesign', '-dv', '--verbose=4', str(code)])
        if not sealed or codesign_field((shown.stdout or '') + (shown.stderr or ''), 'CDHash') != sealed:
            raise InstallError(f"{code.name} isn't the one FERB.app's signature seals; download FERB again")
        shown = self.run(['/usr/bin/codesign', '-d', '-r-', str(code)])
        requirement = designated_requirement((shown.stdout or '') + (shown.stderr or ''))
        app = self.run(['/usr/bin/codesign', '-dv', str(self.bundle)])
        team = codesign_field((app.stdout or '') + (app.stderr or ''), 'TeamIdentifier')
        if requirement == f'cdhash H"{sealed}"':
            return requirement
        if (team and team != 'not set' and 'anchor apple generic' in requirement
                and re.search(rf'certificate leaf\[subject\.OU\] = "?{re.escape(team)}"?(\s|\)|$)', requirement)):
            return requirement
        raise InstallError(f"{code.name} is pinned by neither its exact code hash nor FERB.app's Developer ID team")

    def _record_requirement(self, binary: Path, requirement: str) -> None:
        binary.chmod(0o700)
        dest = binary.with_suffix('.requirement')
        dest.write_text(requirement + '\n')
        dest.chmod(0o600)

    # ---------- native host ----------

    @property
    def native_host_launcher(self) -> Path:
        return self.home / "bin" / "ferb-native-host"        # outside macOS-protected folders

    def write_native_host(self) -> list[Path]:
        """Launcher + manifest in each folder the CLI lists: every installed browser's own, plus Chrome's for Brave,
        which reads it (verified during native-host installation tests; ferb.act.browser.autoinstall)."""
        if not self.native_hosts_dirs:
            return []
        launcher = self.native_host_launcher
        launcher.parent.mkdir(parents=True, exist_ok=True)
        python = " ".join(shlex.quote(a) for a in self.python_command())
        launcher.write_text(f'#!/bin/sh\nFERB_HOME="{self.home}" exec {python} -m ferb.native_host.main "$@"\n')
        launcher.chmod(0o700)
        manifest = json.dumps({"name": NATIVE_HOST_NAME, "description": "FERB native messaging host",
                               "path": str(launcher), "type": "stdio",
                               "allowed_origins": [f"chrome-extension://{i}/" for i in self.extension_ids]}, indent=2)
        written = []
        for d in self.native_hosts_dirs:
            d.mkdir(parents=True, exist_ok=True)
            fd, tmp = tempfile.mkstemp(prefix=".ferb-", suffix=".json", dir=d)   # a browser never reads half a file
            try:
                with os.fdopen(fd, "w") as f:
                    f.write(manifest)
                os.chmod(tmp, 0o644)
                os.replace(tmp, d / NATIVE_HOST_MANIFEST)
            except BaseException:
                Path(tmp).unlink(missing_ok=True)
                raise
            written.append(d / NATIVE_HOST_MANIFEST)
        return written

    # ---------- `ferb` on PATH (FERB.app) ----------

    @property
    def cli_link(self) -> Path | None:
        return self.cli_dir / "ferb" if self.cli_dir else None

    @staticmethod
    def _app_link(path: Path) -> bool:
        """A link FERB.app made: it points at some FERB.app's launcher (or at one that's gone)."""
        return path.is_symlink() and Path(os.readlink(path)).parts[-4:] == ("Contents", "Resources", "bin", "ferb")

    def link_cli(self) -> None:
        """`ferb` on PATH as a link to this app's launcher. Anything else already called `ferb` there is left alone."""
        link = self.cli_link
        if self.bundle is None or link is None:
            return
        if (link.exists() or link.is_symlink()) and not self._app_link(link):
            return
        link.parent.mkdir(parents=True, exist_ok=True)
        staged = link.with_name(".ferb.link-new")
        staged.unlink(missing_ok=True)
        staged.symlink_to(self.bundle / BUNDLE_CLI)
        os.replace(staged, link)

    def cli_status(self) -> str | None:
        """Where the `ferb` command is, for setup to say (FERB.app only). The app is the product and the command is
        for development (owner, 2026-10-14), so setup never edits shell profiles or asks about PATH."""
        link = self.cli_link
        if self.bundle is None or link is None:
            return None
        target = self.bundle / BUNDLE_CLI
        if not (link.is_symlink() and link.resolve() == target.resolve()):
            return f"`ferb` command: left {link} alone (it isn't a link FERB made); FERB's own is {target}"
        return f"`ferb` command: {link}"

    # ---------- uninstall ----------

    def uninstall(self, purge: bool = False) -> list[str]:
        """Remove the LaunchAgent and native-host files. purge=True also deletes FERB's home,
        after the caller has asked the owner. Returns what was removed."""
        removed = []
        if self.loaded():
            self.unload()
            removed.append(f"LaunchAgent {self.label} (unloaded)")
        if self.plist_path.exists():
            self.plist_path.unlink()
            removed.append(str(self.plist_path))
        for d in self.native_hosts_dirs:
            m = d / NATIVE_HOST_MANIFEST
            if m.exists():
                m.unlink()
                removed.append(str(m))
        if self.cli_link is not None and self._app_link(self.cli_link):
            self.cli_link.unlink()
            removed.append(str(self.cli_link))
        if purge:
            self._check_purge_target()
            shutil.rmtree(self.home)
            removed.append(str(self.home))
        return removed

    def _check_purge_target(self) -> None:
        home = self.home.resolve()
        forbidden = {Path("/").resolve(), Path.home().resolve(), (Path.home() / "Desktop").resolve()}
        if home in forbidden or len(home.parts) < 3:
            raise InstallError(f"refusing to delete {home}")
        if not (home / MARKER).is_file():
            raise InstallError(f"refusing to delete {home}: no {MARKER} marker (not a FERB home made by setup)")

    def _ok(self, cmd: list[str], what: str) -> None:
        r = self.run(cmd)
        if r.returncode != 0:
            raise InstallError(f"{what} failed: {(r.stderr or r.stdout).strip()[-400:]}")
