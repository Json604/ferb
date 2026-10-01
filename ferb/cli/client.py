"""Socket client shared by all `ferb` commands. A missing, dying or vanished daemon is always `NotRunning`."""

import json
import socket
from pathlib import Path
from typing import Any, Iterator

from ferb.core.errors import FerbError, NotRunning


class RpcError(FerbError):
    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code = code


class Client:
    def __init__(self, sock_path: Path, role: str = "cli", token: str | None = None, timeout: float = 10) -> None:
        self._sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._sock.settimeout(timeout)
        try:
            self._sock.connect(str(sock_path))
        except (FileNotFoundError, ConnectionRefusedError):
            self._sock.close()
            raise NotRunning("FERB daemon is not running (start it with `ferb daemon run`)") from None
        self._file = self._sock.makefile("r")
        self._next_id = 0
        params: dict[str, Any] = {"role": role}
        if token is not None:
            params["token"] = token
        self.call("auth.hello", params)

    def _send(self, method: str, params: dict[str, Any]) -> int:
        self._next_id += 1
        try:
            self._sock.sendall(json.dumps({"jsonrpc": "2.0", "id": self._next_id, "method": method,
                                           "params": params}).encode() + b"\n")
        except (BrokenPipeError, ConnectionResetError):
            raise NotRunning("FERB daemon went away") from None
        return self._next_id

    def _read(self) -> dict[str, Any]:
        try:
            line = self._file.readline()
        except socket.timeout:
            raise NotRunning(f"FERB didn't answer within {self._sock.gettimeout():.0f} s") from None
        except ConnectionResetError:
            raise NotRunning("FERB daemon went away") from None
        if not line:   # EOF: e.g. connected to a daemon that was dying (spike 8)
            raise NotRunning("FERB daemon closed the connection")
        msg: dict[str, Any] = json.loads(line)
        return msg

    def set_timeout(self, seconds: float) -> None:
        """For calls known to take long (e.g. waking FERB's browser profile)."""
        self._sock.settimeout(seconds)

    def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        rid = self._send(method, params or {})
        while True:
            msg = self._read()
            if msg.get("id") != rid:
                continue
            if "error" in msg:
                raise RpcError(msg["error"]["code"], msg["error"]["message"])
            result: dict[str, Any] = msg["result"]
            return result

    def stream(self, method: str, params: dict[str, Any] | None = None) -> Iterator[dict[str, Any]]:
        """Yield notifications until the daemon ends the stream. Stops when the caller stops iterating."""
        self._sock.settimeout(None)
        rid = self._send(method, params or {})
        while True:
            msg = self._read()
            if msg.get("method") == f"{method}.event":
                yield msg["params"]
            elif msg.get("id") == rid:
                if "error" in msg:
                    raise RpcError(msg["error"]["code"], msg["error"]["message"])
                return

    def close(self) -> None:
        self._file.close()
        self._sock.close()
