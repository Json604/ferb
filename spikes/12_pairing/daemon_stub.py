"""Spike 12 stand-in daemon: one Unix socket, one connection per profile's native host, pairing.Pairing decides.

The native-host manifest is written into Chrome's *default* NativeMessagingHosts folder only while running
(owner-approved) and removed on stop. Host + launcher live outside TCC-protected folders.
"""

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
import time
from dataclasses import dataclass
from pathlib import Path

from pairing import Pairing, Verdict

HERE = Path(__file__).resolve().parent
EXT_ID = "ndaehbjibkcjpgcndbjbooadeepebgkn"
HOST_NAME = "com.ferb.spike12"
RUN = Path(tempfile.gettempdir()) / "ferb12"
SOCK = RUN / "d.sock"
CHROME_HOSTS = Path.home() / "Library/Application Support/Google/Chrome/NativeMessagingHosts"


def stop_stale_hosts() -> None:
    """Our own relay processes left from an earlier run (matched by their script path under RUN), never Chrome."""
    subprocess.run(["pkill", "-f", str(RUN / "host.py")], capture_output=True)


def install_host() -> None:
    stop_stale_hosts()
    RUN.mkdir(mode=0o700, exist_ok=True)
    shutil.copy(HERE / "host.py", RUN / "host.py")
    launcher = RUN / "host.sh"
    launcher.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{RUN / "host.py"}" "{SOCK}" "$@" 2>>"{RUN / "host.err"}"\n')
    launcher.chmod(stat.S_IRWXU)
    (CHROME_HOSTS / f"{HOST_NAME}.json").write_text(json.dumps({
        "name": HOST_NAME, "description": "FERB spike 12 host", "path": str(launcher), "type": "stdio",
        "allowed_origins": [f"chrome-extension://{EXT_ID}/"]}))


def uninstall_host() -> None:
    (CHROME_HOSTS / f"{HOST_NAME}.json").unlink(missing_ok=True)


@dataclass
class Event:
    conn_id: int
    msg: dict
    verdict: Verdict


class Daemon:
    def __init__(self) -> None:
        self.pairing = Pairing(time.monotonic)
        self.events: queue.Queue[Event] = queue.Queue()
        self.conns: dict[int, socket.socket] = {}
        self.conn_install: dict[int, str] = {}      # last install_id seen on each connection
        self._lock = threading.Lock()
        SOCK.parent.mkdir(mode=0o700, exist_ok=True)
        SOCK.unlink(missing_ok=True)
        old = os.umask(0o177)
        self._srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._srv.bind(str(SOCK))
        os.umask(old)
        self._srv.listen(8)
        threading.Thread(target=self._accept, daemon=True).start()

    def _accept(self) -> None:
        n = 0
        while True:
            try:
                conn, _ = self._srv.accept()
            except OSError:
                return
            n += 1
            with self._lock:
                self.conns[n] = conn
            threading.Thread(target=self._serve, args=(n, conn), daemon=True).start()

    def _serve(self, conn_id: int, conn: socket.socket) -> None:
        for line in conn.makefile("r"):
            msg = json.loads(line)
            verdict = self.pairing.handle(msg)
            with self._lock:
                self.conn_install[conn_id] = msg.get("install_id")
            if verdict.reply:
                self._send(conn_id, verdict.reply)
            self.events.put(Event(conn_id, msg, verdict))

    def _send(self, conn_id: int, msg: dict) -> None:
        with self._lock:
            conn = self.conns[conn_id]
        conn.sendall(json.dumps(msg).encode() + b"\n")

    def act(self, install_id: str, msg: dict) -> None:
        """Route an action to an install — only if paired (ARCH 9.6)."""
        if not self.pairing.may_route_action(install_id):
            raise PermissionError(f"install {install_id} is not paired")
        with self._lock:
            conn_id = next(c for c, i in self.conn_install.items() if i == install_id)
        self._send(conn_id, msg)

    def send_raw(self, install_id: str, msg: dict) -> None:
        """Test helper for non-action control messages (e.g. forget_pairing)."""
        with self._lock:
            conn_id = next(c for c, i in self.conn_install.items() if i == install_id)
        self._send(conn_id, msg)

    def wait(self, pred, timeout: float = 30) -> Event:
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            try:
                ev = self.events.get(timeout=max(0.1, end - time.monotonic()))
            except queue.Empty:
                break
            if pred(ev):
                return ev
        raise TimeoutError("expected message never arrived")

    def close(self) -> None:
        self._srv.close()
        with self._lock:
            for c in self.conns.values():
                c.close()
        SOCK.unlink(missing_ok=True)
