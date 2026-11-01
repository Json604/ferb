"""L34 C (owner 2026-10-25: "Build a Claude sign-in like Codex's"): FERB's own Claude sign-in. The pinned Claude Code
signs in with FERB's Claude home and keeps the credential itself; FERB records only that it finished. A fake Claude Code
stands in for the real one: nothing here opens a browser or touches the Keychain."""
import asyncio
import json
import time
from pathlib import Path
from typing import Any, Callable

import pytest

from ferb.cli import runtimes as runtimes_cli
from ferb.cli.client import Client, RpcError
from ferb.core.errors import FerbError, Unsupported
from ferb.runtimes import claude_login as CL
from ferb.runtimes.claude_sdk import ClaudeRuntime
from tests.conftest import RunningDaemon
from tests.support.fake_claude import CREDENTIAL, GOOD_CODE, PAGE, fake_claude


async def until_async(check: Callable[[], bool]) -> None:
    while not check():
        await asyncio.sleep(0.01)


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

def test_a_pasted_code_finishes_the_sign_in_and_only_a_marker_is_left(tmp_path: Path) -> None:
    cli = fake_claude(tmp_path / "bin")
    secrets: list[str] = []
    finished: list[bool] = []

    async def run() -> dict[str, str]:
        sign_in = CL.ClaudeSignIn(tmp_path / "home", lambda: cli, secrets.append, finished.append)
        assert (await sign_in.start())["state"] == "waiting"
        await until_async(lambda: bool(sign_in.url))
        assert sign_in.url == PAGE
        await sign_in.send_code(f"  {GOOD_CODE} ")
        await until_async(lambda: sign_in.state != "waiting")
        return sign_in.status()
    assert asyncio.run(asyncio.wait_for(run(), 10)) == {"state": "signed_in", "url": "", "why": ""}
    assert finished == [True] and secrets == [GOOD_CODE]           # the code joined the redactor first
    home = tmp_path / "home" / "claude-home"
    assert CL.claude_home(tmp_path / "home") == home
    marker = (home / CL.SIGNED_IN).read_text()
    assert CREDENTIAL not in marker and GOOD_CODE not in marker
    assert home.stat().st_mode & 0o777 == 0o700


def test_a_wrong_code_fails_and_says_so_in_the_apps_words(tmp_path: Path) -> None:
    cli = fake_claude(tmp_path / "bin")
    finished: list[bool] = []

    async def run() -> dict[str, str]:
        sign_in = CL.ClaudeSignIn(tmp_path / "home", lambda: cli, lambda code: None, finished.append)
        await sign_in.start()
        await until_async(lambda: bool(sign_in.url))
        await sign_in.send_code("wrong-code")
        await until_async(lambda: sign_in.state != "waiting")
        return sign_in.status()
    status = asyncio.run(asyncio.wait_for(run(), 10))
    assert status["state"] == "failed" and "Try again" in status["why"] and "ferb" not in status["why"]
    assert finished == [False] and CL.claude_home(tmp_path / "home") is None


def test_cancel_stops_the_waiting_sign_in(tmp_path: Path) -> None:
    cli = fake_claude(tmp_path / "bin")
    finished: list[bool] = []

    async def run() -> dict[str, str]:
        sign_in = CL.ClaudeSignIn(tmp_path / "home", lambda: cli, lambda code: None, finished.append)
        await sign_in.start()
        await until_async(lambda: bool(sign_in.url))
        status = await sign_in.cancel()
        assert sign_in._proc is None
        return status
    assert asyncio.run(asyncio.wait_for(run(), 10)) == {"state": "idle", "url": "", "why": ""}
    assert finished == [] and CL.claude_home(tmp_path / "home") is None


def test_a_code_needs_a_waiting_sign_in_and_is_one_word(tmp_path: Path) -> None:
    cli = fake_claude(tmp_path / "bin")

    async def run() -> None:
        sign_in = CL.ClaudeSignIn(tmp_path / "home", lambda: cli, lambda code: None, lambda ok: None)
        with pytest.raises(FerbError, match="waiting"):
            await sign_in.send_code(GOOD_CODE)
        await sign_in.start()
        with pytest.raises(FerbError, match="Paste the code"):
            await sign_in.send_code("two words")
        await sign_in.cancel()
    asyncio.run(asyncio.wait_for(run(), 10))


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


def test_status_says_which_runtimes_ferb_is_signed_in_to(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("ferb.runtimes.keys.has_api_key", lambda provider: provider == "openai")
    monkeypatch.setattr("ferb.runtimes.codex_exec.codex_binary", lambda *a, **k: Path("/x/codex"))
    rows = {r["name"]: r for r in runtimes_cli.status(tmp_path)["runtimes"]}
    assert (rows["claude"]["signed_in"], rows["codex"]["signed_in"], rows["openai"]["signed_in"]) == (False, False, True)
    CL.mark(tmp_path)
    assert {r["name"]: r for r in runtimes_cli.status(tmp_path)["runtimes"]}["claude"]["signed_in"] is True


def test_the_keychain_service_of_ferbs_claude_sign_in_is_named_from_its_home(tmp_path: Path) -> None:
    """`ferb uninstall --purge` deletes Claude Code's item for FERB's Claude home by its service name (never read)."""
    import hashlib
    digest = hashlib.sha256(str(tmp_path / "claude-home").encode()).hexdigest()[:8]
    assert CL.keychain_service(tmp_path) == f"Claude Code-credentials-{digest}"


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
