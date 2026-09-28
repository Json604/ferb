"""Spike 4 harness: a spare browser profile with the FERB extension, and a stand-in daemon on a Unix socket.

The browser runs without any remote-debugging flag; all control goes extension -> native host -> socket.
The only file outside the repo is the manifest Brave reads from Chrome's default folder (owner-approved; it exists
only while tests run: `install_host` / `uninstall_host`). Chrome's lives in the spare profile.
"""

import itertools
import json
import os
import queue
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import threading
from dataclasses import dataclass
from pathlib import Path

HERE = Path(__file__).resolve().parent
EXT = HERE / "ext"
HOST_NAME = "com.ferb.spike4"
EXT_ID = "hbbekolkdflkdbooinmmmlkghdoickoi"  # from the manifest "key"

# Launcher + host must live outside TCC-protected folders (Desktop/Documents/Downloads): the browser spawning
# them has no access there and the host "exits" without ever starting. Per-user TMPDIR (0700) for the spike.
RUN = Path(tempfile.gettempdir()) / "ferb4"
SPARE = HERE / ".spare-profiles"                      # git-ignored: persistent spare Chrome profile
SOCK = Path(tempfile.gettempdir()) / "ferb4" / "d.sock"  # per-user TMPDIR (0700); short for AF_UNIX

BROWSERS = {
    "chrome": "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "brave": "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
}
# Where each browser looks for native-host manifests when run on a spare --user-data-dir (measured):
# Chrome 154: <user-data-dir>/NativeMessagingHosts, i.e. the spare profile's own folder (inside this repo, ignored).
# Brave 151: Google Chrome's default folder; not its own default folder, not <user-data-dir>.
HOST_DIRS = {
    "chrome": SPARE / "chrome" / "NativeMessagingHosts",
    "brave": Path.home() / "Library/Application Support/Google/Chrome/NativeMessagingHosts",
}


def install_host() -> None:
    RUN.mkdir(mode=0o700, exist_ok=True)
    launcher = RUN / "host.sh"
    shutil.copy(HERE / "host.py", RUN / "host.py")
    launcher.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{RUN / "host.py"}" "{SOCK}" "$@" 2>>"{RUN / "host.err"}"\n')
    launcher.chmod(stat.S_IRWXU)
    manifest = json.dumps({"name": HOST_NAME, "description": "FERB spike 4 host", "path": str(launcher),
                           "type": "stdio", "allowed_origins": [f"chrome-extension://{EXT_ID}/"]})
    for d in HOST_DIRS.values():
        d.mkdir(mode=0o700, exist_ok=True)
        (d / f"{HOST_NAME}.json").write_text(manifest)


def uninstall_host() -> None:
    for d in HOST_DIRS.values():
        (d / f"{HOST_NAME}.json").unlink(missing_ok=True)


def chrome_spare_profile() -> Path:
    return SPARE / "chrome"


class ExtError(Exception):
    pass


@dataclass
class Hello:
    ua: str
    extension_id: str
    version: str | None


class SpareBrowser:
    def __init__(self, browser: str, timeout: float = 30) -> None:
        """Brave: fresh temporary profile + --load-extension. Chrome: the persistent spare profile where the
        owner loaded the extension by hand (branded Chrome ignores --load-extension)."""
        self.tmp = Path(tempfile.mkdtemp(prefix="ferb4-"))
        profile = chrome_spare_profile() if browser == "chrome" else self.tmp / "profile"
        SOCK.parent.mkdir(mode=0o700, exist_ok=True)
        SOCK.unlink(missing_ok=True)
        self._srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._srv.bind(str(SOCK))
        os.chmod(SOCK, 0o600)
        self._srv.listen(1)
        self._srv.settimeout(timeout)

        args = [BROWSERS[browser], f"--user-data-dir={profile}", "--no-first-run", "--no-default-browser-check"]
        if browser == "brave":
            args.append(f"--load-extension={EXT}")
        self.proc = subprocess.Popen(args + ["about:blank"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            self._conn, _ = self._srv.accept()
        except socket.timeout:
            self.close()
            raise ExtError(f"{browser}: native host never connected (extension not loaded or host not found)")
        self._rfile = self._conn.makefile("r")
        self._ids = itertools.count(1)
        self._pending: dict[int, queue.Queue] = {}
        self._first: queue.Queue = queue.Queue()
        threading.Thread(target=self._reader, daemon=True).start()
        h = self._first.get(timeout=timeout)
        self.hello = Hello(h["ua"], h["extensionId"], h.get("version"))
        on_disk = json.loads((EXT / "manifest.json").read_text())["version"]
        if self.hello.version != on_disk:
            self.close()
            raise ExtError(f"{browser}: running extension {self.hello.version} != {on_disk} on disk "
                           "(unpacked extension not reloaded)")

    def _reader(self) -> None:
        for line in self._rfile:
            msg = json.loads(line)
            if msg.get("type") == "hello":
                self._first.put(msg)
            elif (q := self._pending.pop(msg.get("id"), None)) is not None:
                q.put(msg)

    def cmd(self, type_: str, timeout: float = 30, **args) -> dict:
        mid = next(self._ids)
        q: queue.Queue = queue.Queue()
        self._pending[mid] = q
        self._conn.sendall(json.dumps({"id": mid, "type": type_, "args": args}).encode() + b"\n")
        msg = q.get(timeout=timeout)
        if msg["type"] == "error":
            raise ExtError(msg["error"])
        return msg["result"]

    def close(self) -> None:
        self.proc.terminate()
        try:
            self.proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        self._srv.close()
        SOCK.unlink(missing_ok=True)
