"""L34 C (owner 2026-10-25: "Build a Claude sign-in like Codex's"): FERB's own Claude sign-in. The pinned Claude Code
signs in with FERB's Claude home and keeps the credential itself; FERB records only that it finished. A fake Claude Code
stands in for the real one: nothing here opens a browser or touches the Keychain."""
import asyncio
import json
import time
from pathlib import Path
from typing import Any, Callable

import pytest

from ferb.cli.client import Client, RpcError
from ferb.core.errors import Unsupported
from ferb.runtimes import claude_login as CL
from ferb.runtimes.claude_sdk import ClaudeRuntime
from tests.conftest import RunningDaemon
from tests.support.fake_claude import CREDENTIAL, GOOD_CODE, PAGE, fake_claude


def until(check: Callable[[], bool], seconds: float = 10) -> None:
    end = time.monotonic() + seconds
    while not check():
        if time.monotonic() > end:
            raise AssertionError("never happened")
        time.sleep(0.05)


def on_loop(daemon: RunningDaemon, fn: Callable[[], Any]) -> Any:
    async def go() -> Any:
        return fn()
    return daemon.run(go())


# ---------- the sign-in itself ----------


# decision: RUN-1
def test_a_missing_bundled_claude_code_says_so_plainly(tmp_path: Path) -> None:
    def missing() -> Path:
        raise Unsupported("the Claude Agent SDK's bundled Claude Code is missing")
    status = asyncio.run(CL.ClaudeSignIn(tmp_path, missing, lambda code: None, lambda ok: None).start())
    assert status["state"] == "failed" and "Download FERB again" in status["why"]
    assert "ferb " not in status["why"] and "SDK" not in status["why"]          # no command, no internals


# decision: RUN-409
def test_only_a_saved_subscription_login_counts_and_no_key_takes_over(tmp_path: Path) -> None:
    assert CL.signed_in('{"loggedIn": true, "authMethod": "claude.ai", "apiProvider": "firstParty"}')
    assert not CL.signed_in('{"loggedIn": false, "authMethod": "none"}')
    assert not CL.signed_in('{"loggedIn": true, "authMethod": "api_key"}')
    assert not CL.signed_in('{"loggedIn": true, "apiProvider": "bedrock"}')
    assert not CL.signed_in("Not logged in. Run claude auth login to authenticate.")
    env = CL.env(tmp_path, {"ANTHROPIC_API_KEY": "sk-x", "CLAUDE_CODE_OAUTH_TOKEN": "t", "PATH": "/usr/bin"})
    assert env["ANTHROPIC_API_KEY"] == env["CLAUDE_CODE_OAUTH_TOKEN"] == "" and env["PATH"] == "/usr/bin"
    assert env["CLAUDE_CONFIG_DIR"] == str(tmp_path / "claude-home")
    assert CL.login_argv(Path("/c")) == ["/c", "auth", "login", "--claudeai"]
    assert CL.status_argv(Path("/c")) == ["/c", "auth", "status", "--json"]
    assert CL.sign_in_url(f"If the browser didn't open, visit: {PAGE}") == PAGE


# ---------- runs, status and the terminal ----------

# decision: RUN-409
def test_runs_use_only_ferbs_own_claude_home(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    proxy = type("P", (), {"base_url": lambda self, run_id: "http://unused"})()
    rt = ClaudeRuntime("supervisor", proxy, str(tmp_path))                          # type: ignore[arg-type]
    with pytest.raises(Unsupported, match="isn't signed in for FERB"):  # never the owner's own ~/.claude login
        rt._options("run", None)
    CL.mark(tmp_path)
    monkeypatch.setattr(CL, "cli_binary", lambda: Path("/bundled/claude"))
    options = rt._options("run", None)
    env = options.env
    assert env["CLAUDE_CONFIG_DIR"] == str(tmp_path / "claude-home")
    assert env["CLAUDE_CODE_OAUTH_TOKEN"] == "" and env["ANTHROPIC_API_KEY"] == ""   # nothing can take over the login
    # the bundled Claude Code by path, never a `claude` on PATH; tool search off behind FERB's proxy (Claude Code
    # 2.1.304+ would add a DeferredToolPlaceholder tool, which FERB's tool check stops)
    assert options.cli_path == Path("/bundled/claude") and env["ENABLE_TOOL_SEARCH"] == "false"

    def missing() -> Path:
        raise Unsupported("the Claude Agent SDK's bundled Claude Code is missing")
    monkeypatch.setattr(CL, "cli_binary", missing)
    with pytest.raises(Unsupported, match="bundled Claude Code is missing"):
        rt._options("run", None)


# ---------- through the daemon ----------

def test_claude_signs_in_through_the_daemon_and_nothing_secret_is_recorded(daemon: RunningDaemon, tmp_path: Path,
                                                                           monkeypatch: pytest.MonkeyPatch) -> None:
    cli = fake_claude(tmp_path / "bin")
    monkeypatch.setattr(CL, "cli_binary", lambda: cli)
    monkeypatch.setattr("ferb.runtimes.keys.has_api_key", lambda provider: False)
    d = daemon.daemon
    client = Client(daemon.sock)

    def claude_row() -> dict[str, Any]:
        return next(r for r in client.call("runtimes.status")["runtimes"] if r["name"] == "claude")
    try:
        assert client.call("runtimes.sign_in", {"runtime": "claude"})["state"] == "waiting"
        until(lambda: claude_row()["sign_in"]["url"] == PAGE)
        client.call("runtimes.sign_in", {"runtime": "claude", "code": GOOD_CODE})
        until(lambda: claude_row()["signed_in"] is True)
        assert claude_row()["sign_in"]["state"] == "signed_in"
        with pytest.raises(RpcError, match="Only Claude"):
            client.call("runtimes.sign_in", {"runtime": "codex"})
        with pytest.raises(RpcError, match="waiting"):
            client.call("runtimes.sign_in", {"runtime": "claude", "code": GOOD_CODE})   # nothing waits any more
    finally:
        client.close()
    recorded = on_loop(daemon, lambda: json.dumps(
        [dict(r) for r in d.db.conn.execute("SELECT * FROM journal")] +       # type: ignore[union-attr]
        [dict(r) for r in d.db.conn.execute("SELECT * FROM spans")], default=str))   # type: ignore[union-attr]
    assert CREDENTIAL not in recorded and GOOD_CODE not in recorded and PAGE not in recorded
    kinds = on_loop(daemon, lambda: [e.kind for e in d._journal().entries()])
    assert "runtimes.sign_in" in kinds and "runtimes.sign_in_finished" in kinds
    assert GOOD_CODE not in d._redact_text(f"the code was {GOOD_CODE}")       # the code joined the redactor
