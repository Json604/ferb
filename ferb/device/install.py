"""Dedicated interpreter + venv + LaunchAgent (section 8 steps 5-6), `ferb setup --update`, and uninstall.

- The interpreter lives in ~/.ferb/python (uv-managed, used only by FERB). TCC grants attach to it (spike 8/15).
- FERB is installed into ~/.ferb/venv as a copy (not a link to the repo): the daemon never runs code from the
  repo, and only changes when `ferb setup --update` reinstalls it.
- Uninstall removes only FERB's own things; `--purge` refuses a folder without the `.ferb-home` marker.
"""

import hashlib
import os
import plistlib
import shutil
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

import ferb
from ferb.core.errors import FerbError

LABEL = "com.ferb.daemon"
PYTHON_VERSION = "3.12"
MARKER = ".ferb-home"

Runner = Callable[[list[str]], "subprocess.CompletedProcess[str]"]


def default_runner(cmd: list[str]) -> "subprocess.CompletedProcess[str]":
    return subprocess.run(cmd, capture_output=True, text=True)


class InstallError(FerbError):
    pass


def repo_root() -> Path:
    return Path(ferb.__file__).resolve().parent.parent


def code_hash(package_dir: Path) -> str:
    """Content hash of a ferb package tree: tells a stale installed copy from the checkout."""
    h = hashlib.sha256()
    for f in sorted(package_dir.rglob("*")):
        if f.is_file() and "__pycache__" not in f.parts and f.suffix in (".py", ".sql"):
            h.update(str(f.relative_to(package_dir)).encode())
            h.update(f.read_bytes())
    return h.hexdigest()[:16]


@dataclass
class Installer:
    home: Path
    label: str = LABEL
    run: Runner = default_runner
    launch_agents: Path = Path.home() / "Library" / "LaunchAgents"

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

    def granted_binary(self) -> Path | None:
        """The real executable TCC sees (the venv's python resolves to the dedicated interpreter)."""
        return self.venv_python.resolve() if self.venv_python.exists() else None

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

    def install_ferb(self, source: Path | None = None) -> None:
        """A copy (non-editable), reinstalled every time: that's what `--update` means."""
        self._ok(["uv", "pip", "install", "--quiet", "--python", str(self.venv_python), "--reinstall-package", "ferb",
                  str(source or repo_root())], "installing FERB into its venv")

    def plist(self) -> dict[str, object]:
        logs = self.home / "logs"
        return {
            "Label": self.label,
            "ProgramArguments": [str(self.venv_python), "-m", "ferb.daemon"],
            "EnvironmentVariables": {"FERB_HOME": str(self.home)},
            "WorkingDirectory": str(self.home),
            "RunAtLoad": True,
            "KeepAlive": {"SuccessfulExit": False},       # restart after a crash, not after a clean exit
            "ThrottleInterval": 5,
            "ProcessType": "Interactive",
            "LimitLoadToSessionType": "Aqua",             # the owner's GUI session: Accessibility needs it
            "StandardOutPath": str(logs / "daemon.out.log"),
            "StandardErrorPath": str(logs / "daemon.err.log"),
        }

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

    def install(self, update_only: bool = False) -> None:
        self.write_marker()
        if update_only:
            if not self.venv_python.exists():
                raise InstallError("FERB isn't installed yet: run `ferb setup` first")
            self.install_ferb()
            self.restart() if self.loaded() else self.load()
            return
        interpreter = self.ensure_interpreter()
        self.ensure_venv(interpreter)
        self.install_ferb()
        changed = self.write_plist()
        if changed or not self.loaded():
            self.load()
        else:
            self.restart()

    # ---------- uninstall ----------

    def uninstall(self, purge: bool = False) -> list[str]:
        """Remove the LaunchAgent (and, from build step 6, native-host files). purge=True also deletes FERB's home,
        after the caller has asked the owner. Returns what was removed."""
        removed = []
        if self.loaded():
            self.unload()
            removed.append(f"LaunchAgent {self.label} (unloaded)")
        if self.plist_path.exists():
            self.plist_path.unlink()
            removed.append(str(self.plist_path))
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
