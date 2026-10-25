"""Own connection roles and scoped run tokens. Must not infer task authorization or choose accounts.

- cli: proven by socket ownership — the peer's uid must be the owner's.
- mcp_shim: proven by a run token (scoped to one worker, expiring, revocable; only its hash is kept). The final
  checker's token is observe-only: the gateway offers and runs only observation tools for it (V3, W4).
- native_host: the relay a browser launches for the FERB extension. The socket peer must be the owner's uid and
  the caller origin one of FERB's extension ids; after that the connection is a message channel whose every
  message is authenticated by pairing (install_id + HMAC + seq), not by method lists."""

import hashlib
import hmac
import os
import secrets
from dataclasses import dataclass
from enum import Enum
from typing import Callable

from ferb.core.errors import AuthError
from ferb.core.interfaces import Clock
from ferb.core.types import Identity


class Role(Enum):
    CLI = "cli"
    MCP_SHIM = "mcp_shim"
    NATIVE_HOST = "native_host"
    VOICE = "voice"                        # FERB.app: push-to-talk transcripts


# Fixed method list per role. New RPC methods declare their allowed roles here, never per-connection.
METHODS: dict[Role, frozenset[str]] = {
    Role.CLI: frozenset({"ping", "logs.subscribe", "trace.get", "stop.engage", "stop.lift",
                         "doctor.check", "clipboard.request", "automation.request", "chat.send", "test.mode", "setup.persona",
                         "chat.approve", "chat.answer", "identity.check", "identity.pair", "browsers.list",
                         "browsers.pair",
                         "browsers.unpair", "browsers.installed", "ps.list", "ps.kill", "ps.attach", "ps.send", "sense.pause", "sense.resume", "sense.status",
                         "memory.learning", "memory.list", "memory.show", "memory.add", "memory.confirm", "memory.edit",
                         "memory.retract", "memory.forget", "memory.distill", "timeline.get",
                         "jev.stats", "jev.enable", "jev.reload", "runtimes.check", "reminders.list", "reminders.cancel",
                         "reminders.unseen", "chat.status", "results.ack", "chat.attach", "task.stop", "schedules.list", "schedules.change",
                         "voice.reload", "voice.switch", "verdict.set", "recovery.list", "recovery.restore",
                         "conversations.list", "conversations.change", "notifications.rules", "notifications.label", "attention.ack",
                         "reports.build", "ui.click_through",
                         "setup.browsers", "setup.browser_open", "runtimes.sign_in", "runtimes.status", "runtimes.use", "setup.permissions", "setup.permission_request", "setup.finish",}),
    Role.MCP_SHIM: frozenset({"ping", "tools.list", "tools.call"}),
    Role.NATIVE_HOST: frozenset(),
    Role.VOICE: frozenset({"ping", "voice.attach", "voice.transcript"}),
}


def allowed(role: Role, method: str) -> bool:
    return method in METHODS[role]


def check_cli_peer(peer_uid: int, owner_uid: int) -> None:
    if peer_uid != owner_uid:
        raise AuthError("cli role requires the owner's uid")


@dataclass(frozen=True)
class RunGrant:
    task_id: str
    worker_id: str
    scopes: frozenset[str]
    identity: Identity
    expires_at: float                  # Clock.monotonic() deadline
    observe_only: bool = False         # the final checker (V3, W4): the gateway allows observation tools only


class RunTokens:
    """Issue / validate / expire / revoke. Tokens are returned once; only SHA-256 hashes are stored."""

    def __init__(self, clock: Clock) -> None:
        self._clock = clock
        self._grants: dict[str, RunGrant] = {}      # sha256(token) -> grant

    @staticmethod
    def _digest(token: str) -> str:
        return hashlib.sha256(token.encode()).hexdigest()

    def issue(self, task_id: str, worker_id: str, scopes: set[str], identity: Identity, ttl_s: float,
              observe_only: bool = False) -> str:
        token = secrets.token_urlsafe(32)
        self._grants[self._digest(token)] = RunGrant(task_id, worker_id, frozenset(scopes), identity,
                                                     self._clock.monotonic() + ttl_s, observe_only)
        return token

    def validate(self, token: str) -> RunGrant:
        digest = self._digest(token)
        # constant-time comparison against stored digests (don't leak which prefix matched)
        grant = next((g for d, g in self._grants.items() if hmac.compare_digest(d, digest)), None)
        if grant is None:
            raise AuthError("unknown or revoked run token")
        if self._clock.monotonic() >= grant.expires_at:
            del self._grants[digest]
            raise AuthError("run token expired")
        return grant

    def recheck(self, grant: RunGrant) -> RunGrant:
        """For an already-authenticated connection: the grant must still be registered and unexpired, so a
        revoke or expiry takes effect on open connections at their next call."""
        if not any(g is grant for g in self._grants.values()):
            raise AuthError("run token revoked")
        if self._clock.monotonic() >= grant.expires_at:
            raise AuthError("run token expired")
        return grant

    def revoke_worker(self, worker_id: str) -> int:
        doomed = [d for d, g in self._grants.items() if g.worker_id == worker_id]
        for d in doomed:
            del self._grants[d]
        return len(doomed)

    def revoke_all(self) -> int:
        n = len(self._grants)
        self._grants.clear()
        return n


def check_voice_peer(peer_uid: int, owner_uid: int, exe: str | None, expected_exe: str,
                     signed: Callable[[str], bool]) -> None:
    """Only the installed FERB.app may speak for the owner: the owner's uid, the installed executable, and a code
    signature meeting the requirement recorded at setup. Anything else on the Mac is refused."""
    if peer_uid != owner_uid:
        raise AuthError("voice role requires the owner's uid")
    if not exe or os.path.realpath(exe) != os.path.realpath(expected_exe):
        raise AuthError("voice role is only for the installed FERB.app")
    if not signed(expected_exe):
        raise AuthError("FERB.app's signature doesn't match the one recorded at setup")


def check_native_host(peer_uid: int, owner_uid: int, origin: str, extension_ids: frozenset[str]) -> None:
    if peer_uid != owner_uid:
        raise AuthError("native_host role requires the owner's uid")
    ext = origin.removeprefix("chrome-extension://").rstrip("/")
    if not origin.startswith("chrome-extension://") or ext not in extension_ids:
        raise AuthError("unknown extension origin")
