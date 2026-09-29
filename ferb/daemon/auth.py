"""Connection roles, per-role method lists, and run tokens (section 2).

- cli: proven by socket ownership — the peer's uid must be the owner's.
- mcp_shim: proven by a run token (scoped to one worker, expiring, revocable; only its hash is kept).
- native_host: the relay a browser launches for the FERB extension. The socket peer must be the owner's uid and
  the caller origin one of FERB's extension ids; after that the connection is a message channel whose every
  message is authenticated by pairing (install_id + HMAC + seq, section 9.6), not by method lists.
"""

import hashlib
import hmac
import secrets
from dataclasses import dataclass
from enum import Enum

from ferb.core.errors import AuthError
from ferb.core.interfaces import Clock
from ferb.core.types import Identity


class Role(Enum):
    CLI = "cli"
    MCP_SHIM = "mcp_shim"
    NATIVE_HOST = "native_host"


# Fixed method list per role. Later build steps add methods here, never per-connection.
METHODS: dict[Role, frozenset[str]] = {
    Role.CLI: frozenset({"ping", "logs.subscribe", "trace.get", "stop.engage", "stop.lift", "undo",
                         "autonomy.get", "autonomy.set", "doctor.check", "automation.request", "chat.send",
                         "chat.approve", "identity.check", "identity.pair", "browsers.list", "browsers.pair",
                         "browsers.unpair", "ps.list", "ps.kill", "sense.pause", "sense.resume", "sense.status",
                         "memory.list", "memory.show", "memory.add", "memory.confirm", "memory.edit",
                         "memory.retract", "memory.forget", "memory.distill", "timeline.get",
                         "jev.stats", "jev.enable", "jev.reload", "reminders.list", "reminders.cancel",
                         "reminders.unseen"}),
    Role.MCP_SHIM: frozenset({"ping", "tools.list", "tools.call"}),
    Role.NATIVE_HOST: frozenset(),
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


class RunTokens:
    """Issue / validate / expire / revoke. Tokens are returned once; only SHA-256 hashes are stored."""

    def __init__(self, clock: Clock) -> None:
        self._clock = clock
        self._grants: dict[str, RunGrant] = {}      # sha256(token) -> grant

    @staticmethod
    def _digest(token: str) -> str:
        return hashlib.sha256(token.encode()).hexdigest()

    def issue(self, task_id: str, worker_id: str, scopes: set[str], identity: Identity, ttl_s: float) -> str:
        token = secrets.token_urlsafe(32)
        self._grants[self._digest(token)] = RunGrant(task_id, worker_id, frozenset(scopes), identity,
                                                     self._clock.monotonic() + ttl_s)
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


def check_native_host(peer_uid: int, owner_uid: int, origin: str, extension_ids: frozenset[str]) -> None:
    if peer_uid != owner_uid:
        raise AuthError("native_host role requires the owner's uid")
    ext = origin.removeprefix("chrome-extension://").rstrip("/")
    if not origin.startswith("chrome-extension://") or ext not in extension_ids:
        raise AuthError("unknown extension origin")
