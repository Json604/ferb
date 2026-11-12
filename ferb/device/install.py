"""Own daemon bundle and launch-agent installation mechanics. Must not decide app targets or task behavior.

Dedicated interpreter + venv + the background service, `ferb setup --update`, and uninstall.

- The daemon never imports code from a checkout or from its working folder (kb area daemon): a checkout's
  launch definition runs `python -P -m ferb.daemon` (no working folder on sys.path) with FERB's home as its working
  folder; FERB.app's agent always runs isolated.
- From a packaged FERB.app (L27) nothing is built or downloaded. A FERB.app signed with a Developer ID registers the
  background service itself (SMAppService, the definition inside the app, `BUNDLE_SERVICE`; DMN-589): setup then only
  retires any user LaunchAgent (`retire_launch_agent`) and records that the service is the app's (`in_app`). Any
  other FERB.app, and a Developer ID one whose registration macOS refused (the fallback), runs it as a user LaunchAgent
  (~/Library/LaunchAgents/com.ferb.daemon.plist, DMN-513) that setup writes and loads with launchctl, running the app's
  nested agent app (FERB, a native stub embedding the app's own Python) with exactly `-m ferb.daemon`, so macOS names
  FERB in permission prompts and Privacy & Security and lists it under "Allow in the Background": without a Team ID an
  SMAppService registration's launch constraint names one exact build, so macOS killed every later build's daemon.
  FERB.app unregisters a registration a 2026-11 build left before it runs setup; setup replaces whatever plist an older
  install left (another build's agent, ~/.ferb/venv's python) and records which build, which signer and which
  definition it set up (`SERVICE_RECORD`), which the app reads. The plist is user-writable, so FERB.app's launch and the
  daemon's start put back a definition that isn't exactly what setup writes (`restore_definition`).
  The secret helper, the pinned Codex and the embedding model come from the app, exactly as its signature seals them;
  `ferb` is a link to the app's launcher.
- A checkout (no app bundle) uses the same LaunchAgent code path under the same label, running ~/.ferb/venv's python
  (Rulings, step 12b). The interpreter lives in ~/.ferb/python
  (uv-managed, used only by FERB); TCC grants attach to it (spike 8/15). FERB is installed as a copy, never a link to
  the repo, from the locked dependencies (uv.lock, package-lock.json), into a new venv that replaces the old one only
  after everything installed, with the daemon stopped: a failed install leaves the old venv and the running daemon as
  they were, and code is never mixed.
- The home, logs and data are the same ~/.ferb either way, so switching between the two keeps everything.
- Uninstall removes only FERB's own things; `--purge` refuses a folder without the `.ferb-home` marker, and removes
  every FERB Keychain item (`KEYCHAIN_SERVICES`, deleted by service, never read)."""

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
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

import ferb
from ferb.core.errors import FerbError
from ferb.paths import app_bundle as app_bundle   # FERB.app's own location (setup and the daemon ask it here)
from ferb.paths import BUNDLE_PYTHON as BUNDLE_PYTHON   # its plain interpreter (python3.12)

LABEL = "com.ferb.daemon"
# FERB.app's login LaunchAgent: the fallback the app writes when macOS refuses its login item (L34 Phase F;
# ui/mac/FERBNotch/Sources/NotchCore/LoginItemPlan.swift). Uninstall removes it, and only a file with this label.
APP_LOGIN_LABEL = "com.ferb.FERB.login"
PYTHON_VERSION = "3.12"
MARKER = ".ferb-home"
NATIVE_HOST_NAME = "com.ferb.native_host"            # extensions/chromium/protocol.js FERB_HOST
NATIVE_HOST_MANIFEST = NATIVE_HOST_NAME + ".json"

# The packaged app's layout (scripts/build-app.sh builds it; ui/mac/FERBNotch/Info.plist names it).
APP_BUNDLE_ID = "com.ferb.FERB"
# The background service's definition inside FERB.app, for SMAppService.agent(plistName:) (ui/mac/FERBAgent/
# com.ferb.daemon.plist, copied there by scripts/build-app.sh): what a Developer ID FERB.app registers (DMN-589), and the
# name FERB.app unregisters a registration a 2026-11 self-signed build left by (DMN-513).
BUNDLE_SERVICE = Path("Contents/Library/LaunchAgents") / f"{LABEL}.plist"
# The local embedding model shipped in FERB.app (MEM-6; scripts/build-app.sh fetches it once)
BUNDLE_MODELS = Path("Contents/Resources/models")
# What FERB.app's setup last installed (build, app, label, signer, where the service's definition lives and, for a
# LaunchAgent, the sha256 of the one it wrote), for the app to tell a new build, a new signer or a definition someone
# changed from the one set up
SERVICE_RECORD = "service.json"
# The record's `service`: FERB.app registered the service itself (SMAppService), or setup wrote a user LaunchAgent
SERVICE_IN_APP, SERVICE_LAUNCH_AGENT = "app", "launch_agent"
# Every Keychain service FERB writes: its own items (API keys, the Jev key, [secrets]) and the owner's saved
# secrets and cards (ferb/identity/owner_secrets.py). FERB's Claude sign-in adds Claude Code's own item for FERB's
# Claude home (`claude_login.keychain_service`).
KEYCHAIN_SERVICES = ("ferb", "ferb.owner-secrets")
KEYCHAIN_MAX_ITEMS = 10_000            # a bound on the delete loop, never reached by a real Keychain
DEV_APP_BUNDLE_ID = "dev.ferb.notch"        # the development notch build (ui/mac/FERBNotch/build.sh)
BUNDLE_CLI = Path("Contents/Resources/bin/ferb")
BUNDLE_NODE = Path("Contents/Resources/node")                 # node_modules/@openai/codex-darwin-<arm64|x64>
BUNDLE_SECRET_HELPER = Path("Contents/Helpers/ferb-secrets")
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


def launchctl_words(r: "subprocess.CompletedProcess[str]") -> str:
    """A failed launchctl call in words, never empty (live 2026-11-04: "restarting the daemon failed:" and nothing
    after it): its exit code, then what it printed (error output first, its last 400 characters)."""
    said = ((r.stderr or "").strip() or (r.stdout or "").strip())[-400:]
    return f"launchctl exited {r.returncode}: {said}" if said else f"launchctl exited {r.returncode} and printed nothing"


# `ferb setup --service` exits with this when the owner switched the service off in Login Items (FERB.app's
# ServicePlan.switchedOffStatus): the app guides that one System Settings visit instead of showing an error.
SWITCHED_OFF_STATUS = 3
SWITCHED_OFF_HOW = ("FERB's background service is switched off: System Settings → General → Login Items → Allow in "
                    "the Background → switch FERB on.")


class ServiceSwitchedOff(InstallError):
    """launchd lists FERB's label as disabled (`Installer.disabled`): nothing was loaded."""


def repo_root() -> Path:
    return Path(ferb.__file__).resolve().parent.parent


def voice_app(home: Path, bundle: Path | None) -> Path:
    """The app hearing runs (L26): FERB.app itself when FERB runs from one, else the copy of the notch app a checkout's
    setup builds into ~/.ferb."""
    return bundle if bundle else home / "FERB.app"


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


def certificate_leaf(requirement: str) -> str | None:
    """The certificate a requirement of exactly FERB's release form pins (scripts/signing.sh: `identifier "…" and
    certificate leaf = H"<SHA-1>"`), as a lowercase SHA-1; None for any other requirement."""
    m = re.fullmatch(r'identifier "[^"]+" and certificate leaf = H"([0-9a-fA-F]{40})"', requirement.strip())
    return m.group(1).lower() if m else None


def _labelled(path: Path, label: str) -> bool:
    """A launchd plist FERB wrote: it exists and names `label` (anything else at that path is left alone)."""
    try:
        return plistlib.loads(path.read_bytes()).get("Label") == label
    except (OSError, ValueError, AttributeError):          # missing, not a plist (InvalidFileException), not a dict
        return False


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


def write_native_hosts(home: Path, launcher: Path, python: list[str], extension_ids: tuple[str, ...],
                       dirs: tuple[Path, ...]) -> tuple[list[Path], list[tuple[Path, str]]]:
    """FERB's native-host launcher (in FERB's home) and its manifest in each of `dirs` (browsers' NativeMessagingHosts
    folders): (written, [(folder, why)]). Each manifest is written whole (a browser never reads half a file) and read
    back: one that isn't there with FERB's words afterwards counts as failed (newer macOS can refuse a folder in
    another app's data without an error FERB sees). One folder's failure never stops the others."""
    launcher.parent.mkdir(parents=True, exist_ok=True)
    command = " ".join(shlex.quote(a) for a in python)
    launcher.write_text(f'#!/bin/sh\nFERB_HOME="{home}" exec {command} -m ferb.native_host.main "$@"\n')
    launcher.chmod(0o700)
    manifest = json.dumps({"name": NATIVE_HOST_NAME, "description": "FERB native messaging host",
                           "path": str(launcher), "type": "stdio",
                           "allowed_origins": [f"chrome-extension://{i}/" for i in extension_ids]}, indent=2)
    written: list[Path] = []
    failed: list[tuple[Path, str]] = []
    for d in dirs:
        target = d / NATIVE_HOST_MANIFEST
        tmp: str | None = None
        try:
            d.mkdir(parents=True, exist_ok=True)
            fd, tmp = tempfile.mkstemp(prefix=".ferb-", suffix=".json", dir=d)
            with os.fdopen(fd, "w") as f:
                f.write(manifest)
            os.chmod(tmp, 0o644)
            os.replace(tmp, target)
            tmp = None
            if target.read_text() != manifest:
                raise OSError("the manifest read back differs from what FERB wrote")
            written.append(target)
        except OSError as e:
            failed.append((d, e.strerror or str(e)))
        finally:
            if tmp is not None:
                Path(tmp).unlink(missing_ok=True)
    return written, failed


@dataclass
class Installer:
    home: Path
    label: str = LABEL
    run: Runner = default_runner
    launch_agents: Path = Path.home() / "Library" / "LaunchAgents"
    native_hosts_dirs: tuple[Path, ...] = ()      # where the native-host manifest goes (the CLI passes the real ones)
    extension_ids: tuple[str, ...] = ()
    bundle: Path | None = None                    # the FERB.app to install from; None: from a checkout
    # FERB.app registers the service itself (SMAppService, DMN-589: `setup --service --app-service`); None: as setup
    # last recorded (every other command)
    app_service: bool | None = None
    cli_dir: Path | None = None                   # where `ferb` is linked from FERB.app (the CLI passes ~/.local/bin)
    read_only: Callable[[Path], bool] = read_only_volume
    # A checkout's copy of the embedding model at install (the CLI passes `embeddings.download`); None: not fetched
    fetch_model: Callable[[Path], None] | None = None
    notes: list[str] = field(default_factory=list, compare=False)   # what setup should say about a step that failed
    _stopped: int | None = field(default=None, init=False, repr=False, compare=False)   # the last unload's process

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
    def staged_venv(self) -> Path:
        """Where a new venv is built; it becomes `venv` only after every install step succeeded (`_swap_venv`)."""
        return self.home / "venv.new"

    @property
    def retired_venv(self) -> Path:
        return self.home / "venv.old"

    @property
    def record_path(self) -> Path:
        return self.home / SERVICE_RECORD

    @property
    def in_app(self) -> bool:
        """The background service is FERB.app's own registration (SMAppService, DMN-589), not a user LaunchAgent."""
        if self.bundle is None:
            return False
        if self.app_service is not None:
            return self.app_service
        return self.recorded().get("service") == SERVICE_IN_APP

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
        """The interpreter for FERB's other processes (the native host): never with its working folder on sys.path."""
        return [str(self.bundle / BUNDLE_PYTHON), *BUNDLE_PYTHON_FLAGS] if self.bundle else [str(self.venv_python), "-P"]

    def daemon_command(self) -> list[str]:
        """FERB.app's agent accepts exactly `-m ferb.daemon` and always runs isolated; a checkout's venv python gets
        `-P`, so `-m` never puts the working folder first on sys.path (a checkout there once ran instead of the
        installed copy, kb area daemon)."""
        if self.bundle:
            return [str(self.daemon_python), "-m", "ferb.daemon"]
        return [str(self.daemon_python), "-P", "-m", "ferb.daemon"]

    def granted_binary(self) -> Path | None:
        """The real executable TCC sees (the venv's python resolves to the dedicated interpreter)."""
        return self.daemon_python.resolve() if self.daemon_python.exists() else None

    def grant_target(self) -> Path | None:
        """What the owner switches on in Privacy & Security: the agent app FERB, or the dedicated interpreter."""
        return self.bundle / BUNDLE_AGENT_APP if self.bundle else self.granted_binary()

    def active_program(self) -> str | None:
        """What the installed service runs now (another installation's, maybe): the user LaunchAgent's program, else
        the FERB.app agent setup last recorded (a 2026-11 build's SMAppService registration)."""
        try:
            args = plistlib.loads(self.plist_path.read_bytes()).get("ProgramArguments") or [None]
        except (OSError, plistlib.InvalidFileException, AttributeError):
            args = [None]
        if args[0]:
            return str(args[0])
        recorded = self.recorded().get("app")
        return str(Path(str(recorded)) / BUNDLE_AGENT) if recorded else None

    def recorded(self) -> dict[str, object]:
        """What FERB.app's setup last recorded ({} when nothing was, or it can't be read)."""
        try:
            data = json.loads(self.record_path.read_text())
        except (OSError, ValueError):
            return {}
        return data if isinstance(data, dict) else {}

    def write_record(self) -> None:
        """Which build of which FERB.app, signed by whom, was set up, and where the service's definition lives
        (FERB.app compares it with its own build and signer at launch; a new signer is the Developer ID switch, T-27).
        A LaunchAgent's record keeps its sha256 (DMN-514); FERB.app's own registration has no file to compare."""
        assert self.bundle is not None
        record: dict[str, object] = {"build": str(self.bundle_info().get("CFBundleVersion", "")),
                                     "app": str(self.bundle), "label": self.label, "signer": self.signer()}
        if self.in_app:
            record["service"] = SERVICE_IN_APP
        else:
            record["service"] = SERVICE_LAUNCH_AGENT
            record["definition"] = hashlib.sha256(self.plist_bytes()).hexdigest()
        data = json.dumps(record, indent=2)
        staged = self.record_path.with_name("." + SERVICE_RECORD + ".new")
        staged.write_text(data + "\n")
        os.replace(staged, self.record_path)

    def signer(self) -> str:
        """Who signed FERB.app (T-27), in the words FERB.app uses for its own signature at launch (NotchCore Signer):
        "team:<TEAMID>" for a Developer ID, "leaf:<SHA-1>" for FERB's self-made release certificate, else "adhoc"."""
        assert self.bundle is not None
        shown = self.run(['/usr/bin/codesign', '-dv', str(self.bundle)])
        team = codesign_field((shown.stdout or '') + (shown.stderr or ''), 'TeamIdentifier')
        if team and team != 'not set':
            return f"team:{team}"
        own = self.run(['/usr/bin/codesign', '-d', '-r-', str(self.bundle)])
        leaf = certificate_leaf(designated_requirement((own.stdout or '') + (own.stderr or '')))
        return f"leaf:{leaf}" if leaf else "adhoc"

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

    def _stage_venv(self, interpreter: Path, source: Path | None = None) -> None:
        """A new venv beside the old one, on FERB's own interpreter, with the locked dependencies (uv.lock, hashes
        required) and a copy of FERB (never a link to the repo). Relocatable, so it still works once renamed to
        `venv`. Nothing here touches the venv the daemon runs from."""
        root = source or repo_root()
        self._remove(self.staged_venv)
        self._ok(["uv", "venv", "--quiet", "--relocatable", "--python", str(interpreter), str(self.staged_venv)],
                 "creating FERB's new venv")
        python = str(self.staged_venv / "bin" / "python")
        with tempfile.TemporaryDirectory(prefix=".ferb-lock-", dir=self.home) as folder:
            locked = Path(folder) / "requirements.txt"
            self._ok(["uv", "export", "--project", str(root), "--frozen", "--no-dev", "--no-emit-project",
                      "--format", "requirements-txt", "--quiet", "-o", str(locked)],
                     "reading FERB's locked dependencies (uv.lock)")
            self._ok(["uv", "pip", "install", "--quiet", "--python", python, "--require-hashes", "-r", str(locked)],
                     "installing FERB's locked dependencies")
        self._ok(["uv", "pip", "install", "--quiet", "--python", python, "--no-deps", str(root)],
                 "installing FERB into its new venv")

    def _swap_venv(self) -> None:
        """The new venv in place of the old one (two renames, the daemon stopped), then the old one removed. A swap cut
        off between the renames is finished by the next install (`_recover_venv`)."""
        if not self.staged_venv.exists():
            return
        self._remove(self.retired_venv)
        if self.venv.exists() or self.venv.is_symlink():
            os.rename(self.venv, self.retired_venv)
        os.rename(self.staged_venv, self.venv)
        self._remove(self.retired_venv)

    def _recover_venv(self) -> None:
        """A swap cut off after the old venv moved aside: put it back, so the daemon has a venv until the new one is
        installed."""
        if not self.venv.exists() and self.retired_venv.exists():
            os.rename(self.retired_venv, self.venv)

    @staticmethod
    def _remove(path: Path) -> None:
        if path.is_symlink() or path.is_file():
            path.unlink()
        elif path.exists():
            shutil.rmtree(path)

    def codex_wanted(self) -> bool:
        """A role uses Codex ([runtimes] orchestrator/worker/checker; Codex when unset), so it is (re)installed; else
        it is left as it is (kb area runtimes: Codex re-pinned only if a role uses it). Signing in to Codex installs
        it too (`ferb setup --codex-login`)."""
        from ferb.config import Config, InsecureConfig
        from ferb.runtimes.catalog import ROLES
        try:
            config = Config(self.home)
            worker = str(config.get("runtimes", "worker", "codex"))
            return any(str(config.get("runtimes", role, worker if role == "checker" else "codex")) == "codex"
                       for role in ROLES)
        except InsecureConfig:
            return True

    def ensure_codex(self) -> None:
        """Pinned Codex in ~/.ferb/node (the daemon never uses a global Codex). Skipped if already pinned. From
        FERB.app, ~/.ferb/node becomes a link to the copy the app ships; npm never writes through such a link. From a
        checkout, `npm ci` installs exactly package-lock.json into a new folder that replaces the old one only once
        it reports the pinned version."""
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
            self._remove(node)                        # FERB's own npm install of another Codex version
            node.symlink_to(bundled)
            return
        staged = self.home / "node.new"
        self._remove(staged)
        staged.mkdir(parents=True)
        try:
            for name in ("package.json", "package-lock.json"):
                shutil.copyfile(repo_root() / name, staged / name)
            self._ok(["npm", "ci", "--prefix", str(staged), "--ignore-scripts", "--no-fund", "--no-audit",
                      "--loglevel=error"], "installing the pinned Codex (package-lock.json)")
            got = self.run([str(staged / _NATIVE_BIN), "--version"]).stdout.split()[-1:]
            if got != [CODEX_VERSION]:
                raise InstallError(f"package-lock.json installed Codex {' '.join(got) or 'nothing'}, not the pinned "
                                   f"{CODEX_VERSION}")
        except BaseException:
            self._remove(staged)
            raise
        self._remove(node)
        os.rename(staged, node)

    def ensure_embedding_model(self) -> None:
        """A checkout's copy of the local embedding model, fetched at install (pinned revision and SHA-256s, checked)
        so a new install's daemon has it from its first start; FERB.app ships its own (`BUNDLE_MODELS`). A failure
        (offline) doesn't stop the install: it is noted, search runs on keywords meanwhile and the daemon fetches it
        later."""
        from ferb.memory.embeddings import default_root
        if self.bundle is not None or self.fetch_model is None:
            return
        try:
            self.fetch_model(default_root(self.home))
        except Exception as e:
            self.notes.append(f"FERB's local embedding model wasn't fetched ({type(e).__name__}: {e}); memory search "
                              "uses keywords until the daemon fetches it")

    def install_files(self) -> None:
        """FERB's own files outside the venv: the browser extension's folder."""
        from ferb.device.extensions import install_extension
        install_extension(self.home)

    def plist(self) -> dict[str, object]:
        """The background service's user LaunchAgent (DMN-513, DMN-516): FERB.app's agent with exactly `-m ferb.daemon`
        (which build runs in FERB_APP_BUILD; Login Items lists it under FERB.app), or a checkout's venv python with
        `-P`; FERB's home as its working folder and its logs either way."""
        logs = self.home / "logs"
        env = {"FERB_HOME": str(self.home)}
        if self.bundle is not None:
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
        if self.bundle is not None:          # System Settings → Login Items lists the agent under FERB (launchd.plist(5))
            agent["AssociatedBundleIdentifiers"] = [str(self.bundle_info().get("CFBundleIdentifier", APP_BUNDLE_ID))]
        return agent

    def plist_bytes(self) -> bytes:
        """Exactly what setup writes (the record keeps its sha256; FERB.app compares the file with it at launch)."""
        return plistlib.dumps(self.plist())

    def definition_current(self) -> bool:
        """The LaunchAgent on disk is exactly what setup writes."""
        try:
            return self.plist_path.read_bytes() == self.plist_bytes()
        except OSError:
            return False

    def write_plist(self) -> bool:
        """Writes the LaunchAgent (whole, through a staged file) unless it is already exactly that: whatever an older
        install left there (another build's agent, ~/.ferb/venv's python) or someone changed is replaced. Returns True
        if the plist changed."""
        (self.home / "logs").mkdir(parents=True, exist_ok=True)
        self.launch_agents.mkdir(parents=True, exist_ok=True)
        if self.definition_current():
            return False
        staged = self.plist_path.with_name(f".{self.plist_path.name}.new")
        staged.write_bytes(self.plist_bytes())
        os.replace(staged, self.plist_path)
        return True

    def restore_definition(self) -> bool:
        """Rule 6 (DMN-514): the LaunchAgent is user-writable, so at each daemon start an existing plist that isn't
        exactly what setup writes is written again; launchd reads it at its next load. A missing one is left to
        FERB.app's launch, which sets the service up again; FERB.app's own registration (DMN-589) has none to put back.
        True when it was rewritten (the daemon journals it)."""
        if self.in_app or not self.plist_path.exists() or self.definition_current():
            return False
        return self.write_plist()

    def labelled_plist(self) -> bool:
        """A plist at the LaunchAgent's path naming FERB's label (this setup's, an older install's or a checkout's)."""
        return _labelled(self.plist_path, self.label)

    def disabled(self) -> bool:
        """The owner switched FERB's background service off (System Settings → General → Login Items → Allow in the
        Background): launchd lists its label as disabled and won't load it until it is switched on."""
        r = self.run(["launchctl", "print-disabled", self.domain])
        if r.returncode != 0:
            return False
        for line in (r.stdout or "").splitlines():
            name, _, state = line.strip().partition("=>")
            if name.strip().strip('"') == self.label:
                return state.strip() in ("disabled", "true")
        return False

    def _printed(self) -> str | None:
        r = self.run(["launchctl", "print", f"{self.domain}/{self.label}"])
        return r.stdout if r.returncode == 0 else None

    def loaded(self) -> bool:
        return self._printed() is not None

    def _loaded_from(self, definition: Path) -> bool:
        """launchd has the job loaded from `definition` (or says nothing about where it came from)."""
        out = self._printed()
        if out is None:
            return False
        path = next((line.split("=", 1)[1].strip() for line in out.splitlines() if line.strip().startswith("path = ")),
                    None)
        return path is None or Path(path) == definition

    def foreign_service(self) -> str | None:
        """For a checkout: launchd runs FERB's label from a definition that isn't the LaunchAgent (a 2026-11 FERB.app's
        SMAppService registration). A checkout can't take it over: only FERB.app can unregister it (DMN-516)."""
        out = self._printed()
        if self.bundle is not None or out is None:
            return None
        path = next((line.split("=", 1)[1].strip() for line in out.splitlines() if line.strip().startswith("path = ")),
                    None)
        return path if path is not None and Path(path) != self.plist_path else None

    def check_not_foreign(self, then: str) -> None:
        """A checkout refuses to install or uninstall over a FERB.app's SMAppService registration (`foreign_service`):
        booting it out would leave the registration behind, and only FERB.app can unregister it (opening a current
        FERB.app does, DMN-513)."""
        if refusal := self.foreign_service():
            raise InstallError(f"FERB.app's background service is registered ({refusal}); open FERB.app once (it "
                               f"moves it to a LaunchAgent) or turn it off in FERB.app (Settings → Advanced → "
                               f"Uninstall FERB…) {then}")

    def agent_pid(self) -> int | None:
        out = self._printed() or ""
        for line in out.splitlines():
            if line.strip().startswith("pid = "):
                return int(line.split("=")[1])
        return None

    def load(self) -> None:
        """The LaunchAgent loaded again: whatever launchd runs under FERB's label (an older definition, a 2026-11
        build's SMAppService job) is booted out first. Switched off in Login Items, nothing is loaded and
        ServiceSwitchedOff says where to switch it on."""
        if self.disabled():
            raise ServiceSwitchedOff(SWITCHED_OFF_HOW)
        # FERB.app asks macOS to unregister a 2026-11 build's SMAppService job just before setup, and macOS finishes
        # that a moment later: until then it can submit the job again right after the bootout, and the bootstrap fails
        # (live 2026-11-02: "Bootstrap failed: 5"). While another definition holds the label, boot it out and load
        # again, for up to 15 s.
        end = time.monotonic() + 15
        while True:
            self.unload()
            r = self.run(["launchctl", "bootstrap", self.domain, str(self.plist_path)])
            if r.returncode == 0:
                return
            held = self.loaded() and not self._loaded_from(self.plist_path)
            if not held or time.monotonic() >= end:
                raise InstallError(f"loading the LaunchAgent failed: {launchctl_words(r)}")
            time.sleep(0.5)

    def unload(self) -> None:
        """Stop the agent and wait, up to 10 s, until launchd has dropped it and its process has exited: launchd can
        drop it while the daemon is still shutting down and writing the store (L28, C1). `exiting` then says whether
        that process outlived the wait."""
        self._stopped = self.agent_pid()
        self.run(["launchctl", "bootout", f"{self.domain}/{self.label}"])
        end = time.monotonic() + 10
        while (self.loaded() or self.exiting() is not None) and time.monotonic() < end:
            time.sleep(0.1)

    def stop(self) -> None:
        """FERB.app's registered service (DMN-589) stopped but kept registered: the daemon is asked to end (SIGTERM
        through launchd) and exits cleanly, so launchd doesn't start it again (KeepAlive SuccessfulExit false) until
        `start`. Waits, up to 10 s, for its process to exit (`exiting` names one that outlived the wait)."""
        self._stopped = self.agent_pid()
        if self._stopped is None:
            return
        self.run(["launchctl", "kill", "SIGTERM", f"{self.domain}/{self.label}"])
        end = time.monotonic() + 10
        while (self.agent_pid() is not None or self.exiting() is not None) and time.monotonic() < end:
            time.sleep(0.1)

    def start(self) -> None:
        """FERB.app's registered service started again after `stop` (nothing when macOS hasn't loaded it)."""
        if self.loaded():
            r = self.run(["launchctl", "kickstart", f"{self.domain}/{self.label}"])
            if r.returncode != 0:
                raise InstallError(f"starting the daemon failed: {launchctl_words(r)}")

    def retire_launch_agent(self) -> bool:
        """The move to FERB.app's own registration (DMN-589): a user LaunchAgent naming FERB's label (T-29's, an older
        install's) is booted out when launchd runs it from that file, its process waited for (it may still be writing
        the store), and deleted, since the same label can't be loaded twice. True when there was one."""
        if not self.labelled_plist():
            return False
        if self.loaded() and self._loaded_from(self.plist_path):
            self.unload()
            self._check_exited()
        self.plist_path.unlink(missing_ok=True)
        return True

    def running(self) -> bool:
        """launchd runs FERB's daemon now (a loaded job with a process)."""
        return self.agent_pid() is not None

    def exiting(self) -> int | None:
        """The pid of the daemon the last `unload` stopped while that process still runs (signal 0 reaches it), else
        None. Asked through `run` like launchctl, so tests fake the process with it."""
        pid = self._stopped
        return pid if pid is not None and self.run(["/bin/kill", "-0", str(pid)]).returncode == 0 else None

    def restart(self) -> None:
        """The daemon restarted on its loaded definition with one `launchctl kickstart -k`, with no moment where the
        label is unloaded. When launchd has no job to restart, or one it is still tearing down (live 2026-11-04:
        macOS's own unregister of a same-label SMAppService registration booted FERB's LaunchAgent out just before
        the quiet update), the LaunchAgent is loaded instead: `load` boots out whatever holds the label, waits for it
        to go and bootstraps the plist (DMN-515, DMN-517). A failure says what launchctl answered to each."""
        r = self.run(["launchctl", "kickstart", "-k", f"{self.domain}/{self.label}"])
        if r.returncode == 0:
            return
        kicked = launchctl_words(r)
        try:
            self.load()
        except ServiceSwitchedOff:
            raise
        except InstallError as e:
            raise InstallError(f"restarting the daemon failed ({kicked}), and then {e}") from e

    def installed(self) -> bool:
        """Something to update: FERB.app's setup ran (its record), or the checkout's venv is there."""
        if self.bundle is not None:
            return self.record_path.exists() or self.labelled_plist()
        return self.venv_python.exists()

    def install(self, update_only: bool = False, while_stopped: Callable[[], None] | None = None) -> None:
        """Install (or, `update_only`, update) FERB and start its daemon on what was installed. `while_stopped`: work
        that must run while no daemon does, run once everything is installed, between stopping the daemon and
        starting it again. A failure before the switch-over leaves the running daemon and its code as they were."""
        if self.bundle:
            self.check_bundle()                       # before anything is written
            # An update with no setup record sets up (DMN-10): FERB.app asks for one when macOS still lists its
            # service, e.g. after FERB was deleted and installed again; the file work is the same either way.
            self.write_marker()
            self._install_from_bundle(while_stopped)
            return
        self._recover_venv()
        if update_only and not self.installed():
            raise InstallError("FERB isn't installed yet: run `ferb setup` first")
        self.check_not_foreign("before running FERB from this checkout")
        self.write_marker()
        interpreter = self.ensure_interpreter()
        try:                                          # everything that can fail, before the switch-over
            self._stage_venv(interpreter)
            self.install_secret_helper()
            if self.codex_wanted():
                self.ensure_codex()
            self.write_native_host()
            self.install_files()
            self.ensure_embedding_model()
        except BaseException:
            self._remove(self.staged_venv)
            raise
        self.write_plist()

        def switch_over() -> None:
            self._swap_venv()
            if while_stopped is not None:
                while_stopped()
        if self.loaded():
            self.while_stopped(switch_over)
        else:
            try:
                switch_over()
            finally:
                self.load()

    def _install_from_bundle(self, while_stopped: Callable[[], None] | None = None) -> None:
        """Nothing is built or downloaded: everything comes from the checked, signed FERB.app. The LaunchAgent is
        written (replacing an older install's, DMN-513) and loaded, or, unchanged and loaded from that file, the daemon
        is restarted on this build. For FERB.app's own registration (DMN-589) any LaunchAgent is retired instead and
        nothing is loaded: FERB.app registers the service right after this command and waits for it; a registration
        launchd already runs (an earlier Developer ID build's) is restarted on this build."""
        self.install_files()
        self.install_secret_helper()
        if self.codex_wanted():
            self.ensure_codex()
        self.write_native_host()
        self.link_cli()
        if self.in_app:
            self.retire_launch_agent()
            self.write_record()
            if while_stopped is not None:
                self.while_stopped(while_stopped)
            elif self.loaded():
                r = self.run(["launchctl", "kickstart", "-k", f"{self.domain}/{self.label}"])
                if r.returncode != 0:            # FERB.app's wait for the daemon decides; this says why it may fail
                    self.notes.append(f"restarting the daemon failed: {launchctl_words(r)}")
            return
        changed = self.write_plist()
        self.write_record()
        if while_stopped is not None:
            self.while_stopped(while_stopped)
        elif changed or not self._loaded_from(self.plist_path):
            self.load()
        else:
            self.restart()

    def while_stopped(self, work: Callable[[], None]) -> None:
        """Run `work` with the daemon stopped, then start it again whatever `work` does (an error or a stop included):
        the LaunchAgent is unloaded (launchd dropped it and, within its wait, its process exited) and loaded again
        (DMN-515); FERB.app's own registration (DMN-589) stays registered: its daemon is stopped and started (`stop`,
        `start`). `work` doesn't run while the stopped process is still exiting (`exiting`, past the stop's 10 s wait):
        it may still be writing the store, and a checkout's venv swap would delete the code it runs; that raises
        InstallError, and the daemon is started again."""
        if self.in_app:
            self.stop()
            try:
                self._check_exited()
                work()
            finally:
                self.start()
            return
        self.unload()
        try:
            self._check_exited()
            work()
        finally:
            self.load()

    def _check_exited(self) -> None:
        if (pid := self.exiting()) is not None:
            raise InstallError(f"FERB's daemon (process {pid}) hasn't exited since launchd stopped it")

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

    def own_requirement(self) -> str:
        """The requirement to pin for FERB.app itself (hearing's signature check): its own designated requirement, once
        check_bundle verified its signature: the exact code hash (ad-hoc), FERB's release certificate or a Developer ID
        team. An identifier alone proves nothing."""
        assert self.bundle is not None
        shown = self.run(['/usr/bin/codesign', '-d', '-r-', str(self.bundle)])
        requirement = designated_requirement((shown.stdout or '') + (shown.stderr or ''))
        if not trusted_requirement(requirement, pinned_ok=True):
            raise InstallError("FERB.app's signature names neither its exact code nor a certificate; download FERB again")
        return requirement

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
            self._swap_helper(staged, binary, requirement)
            return
        source = Path(ferb.__file__).parent / 'identity' / 'secret_helper.swift'
        staged = binary.with_name('.ferb-secrets.new')     # the helper in use stays until this one is signed
        try:
            self._ok(['xcrun', 'swiftc', '-O', '-o', str(staged), str(source), '-framework', 'Security',
                      '-framework', 'CryptoKit'], 'building the secret helper')
            self._ok(['/usr/bin/codesign', '--force', '--sign', 'FERB Local Code Signing', '--identifier',
                      'com.ferb.secrets', str(staged)], 'signing the secret helper (create FERB Local Code Signing in Keychain Access if missing)')
            shown = self.run(['/usr/bin/codesign', '-d', '-r-', str(staged)])
            requirement = designated_requirement((shown.stdout or '') + (shown.stderr or ''))
            if not trusted_requirement(requirement, pinned_ok=False):
                raise InstallError('the secret helper lacks a stable certificate identity')
        except BaseException:
            staged.unlink(missing_ok=True)
            raise
        self._swap_helper(staged, binary, requirement)

    def sealed_requirement(self, code: Path, nested: str) -> str:
        """The requirement to pin for `code`, a copy of FERB.app's nested `nested` (check_bundle verified the app's own
        signature first): only if it is exactly the code that signature seals (the same code hash), and then its
        code hash (ad-hoc), FERB's release requirement naming the very certificate FERB.app carries (L34), or a
        Developer ID requirement naming FERB.app's own team. A merely valid signature, another certificate or
        another team is refused."""
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
        own = self.run(['/usr/bin/codesign', '-d', '-r-', str(self.bundle)])
        leaf = certificate_leaf(designated_requirement((own.stdout or '') + (own.stderr or '')))
        if leaf is not None and certificate_leaf(requirement) == leaf:
            return requirement
        if (team and team != 'not set' and 'anchor apple generic' in requirement
                and re.search(rf'certificate leaf\[subject\.OU\] = "?{re.escape(team)}"?(\s|\)|$)', requirement)):
            return requirement
        raise InstallError(f"{code.name} is pinned by neither its exact code hash nor FERB.app's own certificate "
                           "or Developer ID team")

    @staticmethod
    def _swap_helper(staged: Path, binary: Path, requirement: str) -> None:
        """The new helper and its requirement go in together: both are written whole beside the ones in use first,
        then renamed one right after the other, so a failure while writing (a full disk, a quit) never leaves the new
        helper checked against the old pin, which ferb/identity/helper.py would refuse for every secret until the next
        setup."""
        pin = binary.with_suffix('.requirement')
        staged_pin = binary.with_name('.ferb-secrets.requirement.new')
        try:
            staged.chmod(0o700)
            staged_pin.write_text(requirement + '\n')
            staged_pin.chmod(0o600)
        except BaseException:
            staged.unlink(missing_ok=True)
            staged_pin.unlink(missing_ok=True)
            raise
        os.replace(staged, binary)
        os.replace(staged_pin, pin)

    # ---------- native host ----------

    @property
    def native_host_launcher(self) -> Path:
        return self.home / "bin" / "ferb-native-host"        # outside macOS-protected folders

    def write_native_host(self) -> list[Path]:
        """Launcher + manifest in each folder listed (a checkout's setup lists every installed browser's own, plus
        Chrome's for Brave and Opera, which read it; ferb.act.browser.autoinstall). FERB.app's setup lists none: FERB's
        background service writes them once it has Full Disk Access (DMN-507). A folder that refuses the manifest is
        said in `notes`, never skipped silently; the others are still written."""
        if not self.native_hosts_dirs:
            return []
        written, failed = write_native_hosts(self.home, self.native_host_launcher, self.python_command(),
                                             self.extension_ids, self.native_hosts_dirs)
        self.notes += [f"Couldn't connect FERB to a browser: {d} ({why})." for d, why in failed]
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
        """Stop the daemon and remove its LaunchAgent (FERB.app first unregisters any SMAppService registration a
        2026-11 build left), FERB.app's login fallback, the native-host files, the `ferb` link and the service record
        (`SERVICE_RECORD`). purge=True also deletes
        FERB's home and every FERB Keychain item (`remove_keychain_items`), after the caller has asked the owner.
        Returns what was removed. A checkout refuses while FERB.app's service is registered (as `install` does)."""
        self.check_not_foreign("before uninstalling FERB from this checkout")
        removed = []
        if self.loaded():
            self.unload()
            removed.append(f"background service {self.label} (stopped)")
        if self.labelled_plist():
            self.plist_path.unlink()
            removed.append(str(self.plist_path))
        login = self.launch_agents / f"{APP_LOGIN_LABEL}.plist"
        if _labelled(login, APP_LOGIN_LABEL):
            login.unlink()
            removed.append(str(login))
        for d in self.native_hosts_dirs:
            m = d / NATIVE_HOST_MANIFEST
            if m.exists():
                m.unlink()
                removed.append(str(m))
        if self.cli_link is not None and self._app_link(self.cli_link):
            self.cli_link.unlink()
            removed.append(str(self.cli_link))
        if self.record_path.is_file():               # no service is set up any more: a reinstall sets it up afresh
            self.record_path.unlink()
            removed.append(str(self.record_path))
        if purge:
            self._check_purge_target()
            removed += [f"Keychain items of {service}: {count}"
                        for service, count in self.remove_keychain_items().items() if count]
            shutil.rmtree(self.home)
            removed.append(str(self.home))
        return removed

    def keychain_services(self) -> tuple[str, ...]:
        """Every Keychain service holding FERB's items, Claude Code's item for FERB's own Claude home included."""
        from ferb.runtimes.claude_login import keychain_service
        return (*KEYCHAIN_SERVICES, keychain_service(self.home))

    def remove_keychain_items(self) -> dict[str, int]:
        """Delete every item of FERB's Keychain services, one `security delete-generic-password -s SERVICE` at a time
        (it deletes the first match) until none is left: values are never read or listed. Returns the count per
        service."""
        counts: dict[str, int] = {}
        for service in self.keychain_services():
            count = 0
            while count < KEYCHAIN_MAX_ITEMS and self.run(
                    ["/usr/bin/security", "delete-generic-password", "-s", service]).returncode == 0:
                count += 1
            counts[service] = count
        return counts

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
            said = ((r.stderr or "").strip() or (r.stdout or "").strip())[-400:]
            raise InstallError(f"{what} failed: {said or f'{cmd[0]} exited {r.returncode} and printed nothing'}")
