"""The pinned Codex binary pointed at a loopback model server (tests/support/model_loopback.py), for request-shape tests.

Codex runs with an API-key login holding a fake key in its own empty CODEX_HOME, and `openai_base_url` set to the
loopback server: no model runs, no credential is used and nothing leaves the Mac. FERB's own command line, catalog
lockdown and RPC parameters are used; only the login and the base URL differ from a real run (FERB's account check,
which wants a ChatGPT login, is tested on its own in tests/test_codex_streaming.py)."""
import json
import os
import subprocess
import threading
import time
from pathlib import Path
from typing import Any

import pytest

from ferb.runtimes import codex_exec as cx
from tests.support.model_loopback import ModelLoopback

REPO = Path(__file__).resolve().parents[2]


def pinned_codex() -> Path:
    """The pinned binary from the checkout's node_modules; the test is skipped without it and fails by name when it isn't
    the pinned version."""
    try:
        binary = cx.codex_binary(Path("/nonexistent-ferb-home"), REPO)
    except Exception:
        pytest.skip("the pinned Codex isn't installed in this checkout (npm install)")
    out = subprocess.run([str(binary), "--version"], capture_output=True, text=True, check=False).stdout.split()
    assert out[-1:] == [cx.CODEX_VERSION], (f"Codex {' '.join(out)} is not the pinned {cx.CODEX_VERSION}: re-measure "
                                           "the request shapes these tests pin before moving the pin")
    return binary


def ferb_home_with_catalog(root: Path) -> Path:
    """A FERB home holding only the pinned Codex's own catalog (`debug models --bundled`, offline, empty home)."""
    home = root / "ferb"
    (home / "run").mkdir(parents=True)
    assert cx.bundled_catalog(home) is not None, "the pinned Codex printed no model catalog"
    return home


def api_key_login(codex_home: Path) -> None:
    codex_home.mkdir(mode=0o700, parents=True, exist_ok=True)
    (codex_home / "auth.json").write_text(json.dumps({"auth_mode": "apikey", "OPENAI_API_KEY": "sk-loopback-test"}))


def loopback_env(codex_home: Path, user_home: Path) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if not k.startswith(("OPENAI_", "CODEX_"))}
    env.update(HOME=str(user_home), CODEX_HOME=str(codex_home), RUST_LOG="trace")
    return env


def base_url_flag(loopback: ModelLoopback) -> list[str]:
    return ["-c", "openai_base_url=" + json.dumps(loopback.url + "/v1")]


class AppServer:
    """A pinned app-server over stdio: JSON-RPC requests, its notifications skipped, its stderr drained (with
    RUST_LOG=trace it fills a pipe fast) and kept for the tool-catalog check."""

    def __init__(self, cmd: list[str], env: dict[str, str], cwd: Path) -> None:
        self.proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                     env=env, cwd=str(cwd))
        self.stderr: list[str] = []
        self._next = 0
        threading.Thread(target=self._drain, daemon=True).start()

    def _drain(self) -> None:
        assert self.proc.stderr is not None
        for line in self.proc.stderr:
            self.stderr.append(line.decode(errors="replace"))

    def rpc(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        assert self.proc.stdin is not None and self.proc.stdout is not None
        self._next += 1
        self.proc.stdin.write((json.dumps({"id": self._next, "method": method, "params": params}) + "\n").encode())
        self.proc.stdin.flush()
        while True:
            line = self.proc.stdout.readline()
            assert line, "the app-server exited: " + "".join(self.stderr[-5:])[-800:]
            message = json.loads(line)
            if message.get("id") == self._next and "method" not in message:
                return message                      # type: ignore[no-any-return]

    def initialize(self) -> None:
        self.rpc("initialize", {"clientInfo": {"name": "ferb", "title": "FERB", "version": "0.27"},
                                "capabilities": {"experimentalApi": True}})
        assert self.proc.stdin is not None
        self.proc.stdin.write(b'{"method":"initialized"}\n')
        self.proc.stdin.flush()

    def close(self) -> None:
        if self.proc.poll() is None:
            self.proc.kill()
        self.proc.wait()


def model_requests(loopback: ModelLoopback, count: int, timeout: float = 30) -> list[dict[str, Any]]:
    """The first `count` model requests Codex sent (a warm-up request comes first), waiting for them."""
    deadline = time.monotonic() + timeout
    while len(loopback.bodies()) < count:
        assert time.monotonic() < deadline, f"Codex sent {len(loopback.bodies())} of {count} model requests"
        time.sleep(0.1)
    return loopback.bodies()[:count]


def roles(body: dict[str, Any]) -> list[str]:
    """The request's input items as `type:role` (Codex 0.157.1 sends its tool list as an `additional_tools` item)."""
    return [f"{item.get('type')}:{item.get('role')}" for item in body.get("input", [])]


def texts(body: dict[str, Any]) -> list[str]:
    return ["".join(part.get("text", "") for part in item.get("content") or []) for item in body.get("input", [])]


def tool_names(body: dict[str, Any]) -> list[str]:
    names: list[str] = []
    for item in body.get("input", []):
        for tool in item.get("tools") or []:
            names += ([f"{tool['name']}.{t['name']}" for t in tool.get("tools", [])] if tool.get("type") == "namespace"
                      else [str(tool.get("name"))])
    return names
