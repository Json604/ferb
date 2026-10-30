"""Owns FERB's own Claude sign-in (L34 C; owner 2026-10-25: "Build a Claude sign-in like Codex's"). Must not read,
keep, log or forward the credential, and never types into a terminal (rule 10, D31).

Like FERB's Codex sign-in (`codex login` with CODEX_HOME=~/.ferb/codex-home), the pinned Claude Code that the Claude
Agent SDK bundles runs its own non-interactive `auth login --claudeai` with CLAUDE_CONFIG_DIR=~/.ferb/claude-home, so
FERB's runs never load the owner's personal Claude Code settings. Claude Code opens the sign-in page in the browser,
gets the result on its localhost callback (or a code the owner pastes from the page), and writes the credential into
the login Keychain itself (`security -i` on stdin; item `Claude Code-credentials-<first 8 hex of sha256(config dir)>`).
FERB never sees it: runs started with the same CLAUDE_CONFIG_DIR read that saved login. FERB records only that a
sign-in finished (SIGNED_IN, a marker that holds no secret), after `auth status --json` said so. Checked on Claude Code
2.1.283 (claude-agent-sdk 0.2.160); its `setup-token` is an interactive terminal screen and isn't used."""

import asyncio
import json
import os
import re
from contextlib import suppress
from pathlib import Path
from typing import Callable

from ferb.core.errors import FerbError, Unsupported

SIGNED_IN = ".ferb-signed-in"
_URL = re.compile(r"https://\S+")


def cli_binary() -> Path:
    """The pinned Claude Code the SDK bundles (claude_agent_sdk/_bundled/claude)."""
    import claude_agent_sdk
    path = Path(claude_agent_sdk.__file__).parent / "_bundled" / "claude"
    if not path.is_file():
        raise Unsupported("the Claude Agent SDK's bundled Claude Code is missing")
    return path


def home_dir(ferb_home: Path) -> Path:
    return ferb_home / "claude-home"


def claude_home(ferb_home: Path) -> Path | None:
    """FERB's own Claude home once a sign-in there finished; None: FERB's Claude runtime isn't available (it never
    falls back to the owner's own ~/.claude login)."""
    home = home_dir(ferb_home)
    return home if (home / SIGNED_IN).is_file() else None


def prepare(ferb_home: Path) -> Path:
    home = home_dir(ferb_home)
    home.mkdir(parents=True, exist_ok=True)
    os.chmod(home, 0o700)
    return home


def env(ferb_home: Path, base: dict[str, str] | None = None) -> dict[str, str]:
    """Claude Code's environment for signing in: FERB's Claude home; no key or token in this environment can take over
    the login; telemetry and error reports off, as for runs."""
    out = dict(os.environ if base is None else base)
    out.update({"CLAUDE_CONFIG_DIR": str(home_dir(ferb_home)), "ANTHROPIC_API_KEY": "", "ANTHROPIC_AUTH_TOKEN": "",
                "CLAUDE_CODE_OAUTH_TOKEN": "", "DISABLE_TELEMETRY": "1", "DISABLE_ERROR_REPORTING": "1"})
    return out


def login_argv(cli: Path) -> list[str]:
    return [str(cli), "auth", "login", "--claudeai"]


def logout_argv(cli: Path) -> list[str]:
    return [str(cli), "auth", "logout"]


def status_argv(cli: Path) -> list[str]:
    return [str(cli), "auth", "status", "--json"]


def signed_in(status_output: str) -> bool:
    """`auth status --json` says a Claude subscription login is saved there (an API key doesn't count: FERB's Claude
    runs refuse one)."""
    try:
        status = json.loads(status_output)
    except ValueError:
        return False
    return (isinstance(status, dict) and status.get("loggedIn") is True
            and status.get("apiProvider", "firstParty") == "firstParty"
            and "api" not in str(status.get("authMethod", "")).lower())


def mark(ferb_home: Path) -> None:
    (prepare(ferb_home) / SIGNED_IN).write_text(
        "FERB's own Claude sign-in finished; Claude Code keeps the credential in the login Keychain.\n")


def unmark(ferb_home: Path) -> None:
    """A sign-out: the marker goes, so FERB stops offering Claude until the owner signs in again."""
    (home_dir(ferb_home) / SIGNED_IN).unlink(missing_ok=True)


def ferb_command() -> str:
    """The `ferb` command the owner can run: FERB.app's own launcher, or `ferb` from a checkout."""
    from ferb.device.install import BUNDLE_CLI
    from ferb.paths import app_bundle
    bundle = app_bundle()
    return f'"{bundle / BUNDLE_CLI}"' if bundle is not None else "ferb"


def by_hand() -> str:
    """The exact steps to sign in without FERB.app. The owner runs them; FERB never types into a terminal (rule 10)."""
    return f"To sign in by hand, run this in Terminal yourself: {ferb_command()} setup --claude-login"


def sign_in_url(line: str) -> str | None:
    found = _URL.search(line)
    return found.group(0).rstrip(".,)") if found else None


class ClaudeSignIn:
    """One sign-in at a time, run by the daemon for FERB.app (`runtimes.sign_in`): its state, the page to open when the
    browser didn't, and the owner's pasted code. Claude Code's output is read only for that page's address; nothing it
    prints is kept, logged or journaled. `done(signed_in)` is told when a sign-in ends (not when it is cancelled)."""

    def __init__(self, ferb_home: Path, cli: Callable[[], Path], add_secret: Callable[[str], None],
                 done: Callable[[bool], None]) -> None:
        self.ferb_home, self.cli, self.add_secret, self.done = ferb_home, cli, add_secret, done
        self.state, self.url, self.why = "idle", "", ""
        self._proc: asyncio.subprocess.Process | None = None
        self._follow_task: asyncio.Task[None] | None = None
        self._cancelled = False

    def status(self) -> dict[str, str]:
        return {"state": self.state, "url": self.url, "why": self.why}

    async def start(self) -> dict[str, str]:
        if self.state == "waiting":
            return self.status()
        try:
            cli = self.cli()
        except Unsupported as e:
            self.state, self.url, self.why = "failed", "", f"FERB can't sign in to Claude here ({e}). {by_hand()}"
            return self.status()
        prepare(self.ferb_home)
        self.state, self.url, self.why, self._cancelled = "waiting", "", "", False
        self._proc = await asyncio.create_subprocess_exec(
            *login_argv(cli), env=env(self.ferb_home), stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT, start_new_session=True)
        self._follow_task = asyncio.create_task(self._follow(cli, self._proc))
        return self.status()

    async def _follow(self, cli: Path, proc: asyncio.subprocess.Process) -> None:
        assert proc.stdout is not None
        async for raw in proc.stdout:                  # read for the page's address only; nothing else is kept
            url = sign_in_url(raw.decode("utf-8", "replace"))
            if url and not self.url:
                self.url = url
        code = await proc.wait()
        self._proc = None
        if self._cancelled:
            return
        ok = code == 0 and await self._confirmed(cli)
        if ok:
            mark(self.ferb_home)
        self.state, self.url = ("signed_in" if ok else "failed"), ""
        self.why = "" if ok else f"Claude didn't confirm the sign-in. Try again. {by_hand()}"
        self.done(ok)

    async def _confirmed(self, cli: Path) -> bool:
        proc = await asyncio.create_subprocess_exec(
            *status_argv(cli), env=env(self.ferb_home), stdin=asyncio.subprocess.DEVNULL,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL)
        out, _ = await proc.communicate()
        return signed_in(out.decode("utf-8", "replace"))

    async def send_code(self, code: str) -> dict[str, str]:
        """The code the sign-in page shows when its own redirect couldn't finish: to Claude Code's stdin only."""
        code = code.strip()
        proc = self._proc
        if self.state != "waiting" or proc is None or proc.stdin is None:
            raise FerbError("No Claude sign-in is waiting for a code; start it again")
        if not code or any(c.isspace() for c in code):
            raise FerbError("Paste the code the sign-in page shows")
        self.add_secret(code)                          # rule 14: redacted wherever anything echoes it
        proc.stdin.write(code.encode() + b"\n")
        await proc.stdin.drain()
        return self.status()

    async def cancel(self) -> dict[str, str]:
        self._cancelled = True
        proc = self._proc
        if proc is not None and proc.returncode is None:
            proc.terminate()
        if self._follow_task is not None:
            with suppress(Exception):
                await self._follow_task
        self.state, self.url, self.why = "idle", "", ""
        return self.status()

    async def close(self) -> None:
        """Daemon shutdown: a sign-in still waiting stops."""
        if self.state == "waiting":
            await self.cancel()
