"""Spike 13 stand-in daemon: connection generations + a minimal action ledger (ARCH 9.4, section 7 step 9).

- Every (re)connect gets a new generation; commands carry it.
- Ledger: executing -> done | failed; if the connection drops first -> unknown. Unknown is NEVER resent;
  it is reconciled from a fresh read (the tabs the extension re-announces / lists).
"""

import json
import os
import queue
import socket
import threading
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path


@dataclass
class Entry:
    id: str
    gen: int
    name: str
    args: dict
    state: str = "executing"          # executing | done | failed | unknown
    result: dict | None = None
    reconciled: bool = False


@dataclass
class Daemon:
    sock_path: Path
    gen: int = 0
    ledger: dict[str, Entry] = field(default_factory=dict)
    hellos: list[dict] = field(default_factory=list)
    events: "queue.Queue[dict]" = field(default_factory=queue.Queue)
    _conn: socket.socket | None = None
    _lock: threading.Lock = field(default_factory=threading.Lock)

    def start(self) -> "Daemon":
        self.sock_path.unlink(missing_ok=True)
        old = os.umask(0o177)
        self._srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._srv.bind(str(self.sock_path))
        os.umask(old)
        self._srv.listen(4)
        threading.Thread(target=self._accept, daemon=True).start()
        return self

    def _accept(self) -> None:
        while True:
            try:
                conn, _ = self._srv.accept()
            except OSError:
                return
            threading.Thread(target=self._serve, args=(conn,), daemon=True).start()

    def _serve(self, conn: socket.socket) -> None:
        my_gen = None
        for line in conn.makefile("r"):
            msg = json.loads(line)
            if msg["type"] == "hello":
                with self._lock:
                    self.gen += 1
                    my_gen = self.gen
                    self._conn = conn
                    self.hellos.append({**msg, "gen": my_gen})
                self._write(conn, {"type": "welcome", "gen": my_gen})
            elif msg["type"] in ("result", "error"):
                e = self.ledger.get(msg["id"])
                if e and e.gen == msg["gen"] and e.state == "executing":
                    e.state = "done" if msg["type"] == "result" else "failed"
                    e.result = msg.get("result") or {"error": msg.get("error")}
            self.events.put({**msg, "_conn_gen": my_gen, **({"gen": my_gen} if msg["type"] == "hello" else {})})
        # connection gone: whatever this generation left in flight is now unknown
        with self._lock:
            for e in self.ledger.values():
                if e.gen == my_gen and e.state == "executing":
                    e.state = "unknown"
            if self._conn is conn:
                self._conn = None
        self.events.put({"type": "_disconnected", "_conn_gen": my_gen})

    def _write(self, conn: socket.socket, msg: dict) -> None:
        conn.sendall(json.dumps(msg).encode() + b"\n")

    def send(self, name: str, args: dict | None = None, id: str | None = None) -> Entry:
        with self._lock:
            if self._conn is None:
                raise ConnectionError("extension not connected")
            e = Entry(id or uuid.uuid4().hex, self.gen, name, args or {})
            self.ledger[e.id] = e                          # intent recorded before it goes out
            self._write(self._conn, {"type": "cmd", "id": e.id, "gen": e.gen, "name": name, "args": e.args})
        return e

    def send_raw(self, msg: dict) -> None:
        """Test hook: put an arbitrary message on the wire (e.g. a stale-generation command)."""
        with self._lock:
            self._write(self._conn, msg)

    def reconcile_open(self, e: Entry, owned: list[dict]) -> None:
        """Fresh read decides what an unknown `open` did. Never resends."""
        assert e.state == "unknown"
        hit = next((t for t in owned if t.get("id") == e.id), None)
        e.state, e.result, e.reconciled = ("done", {"tabId": hit["tabId"]}, True) if hit else ("failed", None, True)

    def wait(self, pred, timeout: float = 30) -> dict:
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            try:
                m = self.events.get(timeout=max(0.05, end - time.monotonic()))
            except queue.Empty:
                break
            if pred(m):
                return m
        raise TimeoutError("expected message never arrived")

    def close(self) -> None:
        self._srv.close()
        self.sock_path.unlink(missing_ok=True)
