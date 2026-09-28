"""Install / start / stop the spike 8 LaunchAgent, and a CLI client for its socket.

Install location is ~/.ferb/spike8 (outside TCC-protected folders): a venv with pyobjc, daemon.py and spike 6's
probes.py. The plist goes in ~/Library/LaunchAgents and is removed by `uninstall()`.
"""

import json
import os
import plistlib
import shutil
import socket
import subprocess
import time
from pathlib import Path

LABEL = "com.ferb.spike8"
HERE = Path(__file__).resolve().parent
HOME = Path.home() / ".ferb" / "spike8"
VENV_PY = HOME / "venv" / "bin" / "python"
PLIST = Path.home() / "Library" / "LaunchAgents" / f"{LABEL}.plist"
DOMAIN = f"gui/{os.getuid()}"
SOCK = Path.home() / ".ferb" / "run" / "spike8.sock"


def install(program_dir: Path | None = None) -> None:
    """program_dir: where daemon.py runs from (default: the installed copy). Tests pass the repo to measure TCC."""
    HOME.mkdir(parents=True, exist_ok=True)
    os.chmod(HOME, 0o700)
    if not VENV_PY.exists():
        subprocess.run(["uv", "venv", "--quiet", "--python", "3.12", str(HOME / "venv")], check=True)
        subprocess.run(["uv", "pip", "install", "--quiet", "--python", str(VENV_PY),
                        "pyobjc-framework-ApplicationServices", "pyobjc-framework-Quartz", "pyobjc-framework-Cocoa"],
                       check=True)
    shutil.copy(HERE / "daemon.py", HOME / "daemon.py")
    shutil.copy(HERE.parent / "06_pyobjc" / "probes.py", HOME / "probes.py")
    src = program_dir or HOME
    PLIST.write_bytes(plistlib.dumps({
        "Label": LABEL,
        "ProgramArguments": [str(VENV_PY), str(src / "daemon.py")],
        "RunAtLoad": True,
        "KeepAlive": {"SuccessfulExit": False},  # restart after a crash, not after a clean exit
        "ThrottleInterval": 1,
        "ProcessType": "Interactive",
        "StandardOutPath": str(HOME / "daemon.out"),
        "StandardErrorPath": str(HOME / "daemon.err"),
    }))


def start() -> None:
    subprocess.run(["launchctl", "bootstrap", DOMAIN, str(PLIST)], check=True, capture_output=True)


def stop() -> None:
    subprocess.run(["launchctl", "bootout", f"{DOMAIN}/{LABEL}"], capture_output=True)
    end = time.monotonic() + 10
    while loaded() and time.monotonic() < end:
        time.sleep(0.1)


def uninstall() -> None:
    stop()
    PLIST.unlink(missing_ok=True)
    SOCK.unlink(missing_ok=True)


def loaded() -> bool:
    return subprocess.run(["launchctl", "print", f"{DOMAIN}/{LABEL}"], capture_output=True).returncode == 0


def launchd_pid() -> int | None:
    out = subprocess.run(["launchctl", "print", f"{DOMAIN}/{LABEL}"], capture_output=True, text=True).stdout
    for line in out.splitlines():
        if line.strip().startswith("pid = "):
            return int(line.split("=")[1])
    return None


class CliError(Exception):
    pass


def call(method: str, timeout: float = 5) -> dict:
    """The CLI side: one request over the daemon socket."""
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    try:
        s.connect(str(SOCK))
    except (FileNotFoundError, ConnectionRefusedError) as e:
        raise CliError(f"daemon not running ({e.__class__.__name__})") from None
    with s:
        try:
            s.sendall(json.dumps({"id": 1, "method": method}).encode() + b"\n")
            line = s.makefile("r").readline()
        except (BrokenPipeError, ConnectionResetError, socket.timeout) as e:
            raise CliError(f"daemon went away ({e.__class__.__name__})") from None
        if not line:  # connected to a daemon that is dying (e.g. just killed): EOF, not a reply
            raise CliError("daemon closed the connection")
        resp = json.loads(line)
    if "error" in resp:
        raise CliError(resp["error"])
    return resp["result"]


def wait_ping(timeout: float = 20) -> dict:
    end = time.monotonic() + timeout
    while True:
        try:
            return call("ping")
        except CliError:
            if time.monotonic() > end:
                raise
            time.sleep(0.2)
