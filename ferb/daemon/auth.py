"""Own connection roles and scoped run tokens. Must not infer task authorization or choose accounts.

- cli: proven by socket ownership — the peer's uid must be the owner's.
- mcp_shim: proven by a run token (scoped to one worker, expiring, revocable; only its hash is kept). The final
  checker's token is observe-only: the gateway offers and runs only observation tools for it (V3, W4).
- native_host: the relay a browser launches for the FERB extension. The socket peer must be the owner's uid and
  the caller origin one of FERB's extension ids; after that the connection is a message channel whose every
  message is authenticated by pairing (install_id + HMAC + seq), not by method lists."""

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


# Fixed method list per role. New RPC methods declare their allowed roles here, never per-connection.
METHODS: dict[Role, frozenset[str]] = {
    Role.CLI: frozenset({"ping", "logs.subscribe", "trace.get", "stop.engage", "stop.lift",
                         "doctor.check", "clipboard.request", "automation.request", "chat.send", "test.mode", "setup.persona",
                         "chat.approve", "chat.answer", "identity.check", "identity.pair", "browsers.list",
                         "browsers.pair",
                         "browsers.unpair", "browsers.installed", "ps.list", "ps.kill", "ps.attach", "ps.send", "sense.pause", "sense.resume", "sense.status",
                         "memory.learning", "memory.list", "memory.show", "memory.add", "memory.profile", "memory.edit",
                         "memory.retract", "memory.forget", "memory.restore", "memory.repeats", "memory.import.estimate",
                         "memory.import.start", "memory.import.status", "memory.import.cancel", "timeline.get",
                         "jev.stats", "jev.enable", "jev.reload", "runtimes.check", "reminders.list", "reminders.cancel",
                         "chat.status", "results.ack", "chat.attach", "task.stop", "schedules.list", "schedules.change",
                         "verdict.set", "recovery.list", "recovery.restore",
                         "conversations.list", "conversations.change", "attention.ack",
                         "reports.build", "reports.send", "ui.click_through", "tasks.preview",
                         "setup.browsers", "setup.browser_open", "setup.browser_pair", "setup.browsers_pair_all", "browser.show_handoff", "reports.diagnostic", "update.prepare", "tasks.list", "notices.list", "settings.get", "settings.set", "notifications.subscribe", "notifications.categories", "runtimes.sign_in", "runtimes.status", "runtimes.use", "runtimes.switch_task", "setup.permissions", "setup.permission_request", "setup.finish",
                         "about.get", "about.guide", "secrets.saved", "secrets.delete", "identity.find",
                         "identity.choose", "seen.list", "seen.image", "intro.suggest", "intro.preview"}),
    Role.MCP_SHIM: frozenset({"ping", "tools.list", "tools.call"}),
    Role.NATIVE_HOST: frozenset(),
}


# Methods of the cli role that only FERB.app's own process may call (notes/preview.md: the live preview's masked window
# pictures borrow FERB's Screen Recording permission; any other program running as the owner gets "unauthorized").
# Step 12c: what lists or deletes the owner's saved secrets, writes FERB's own account into config or pairs its
# profile, and FERB.app's guide that becomes FERB's self-description (daemon decisions 6-8).
# Step 12e: "What FERB saw", the stored redacted screenshots and their facts; the setup window's Done, which records
# setup as done from the grants FERB.app reports for itself (only FERB.app can read them).
# The intro's Try beat (notes/setup-intro.md step 6): the suggested first task and its plan-only preview.
# The server checks the peer's code signature (app_peer.py) once per connection.
APP_ONLY: frozenset[str] = frozenset({"tasks.preview", "secrets.saved", "secrets.delete", "identity.find",
                                      "identity.choose", "identity.pair", "about.guide", "seen.list", "seen.image",
                                      "setup.finish", "intro.suggest", "intro.preview"})


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

    def revoke_tree(self, tree_id: str) -> int:
        """A task tree ended: every token of its tasks goes, so a token never outlives its task (a task with no
        deadline has no expiry of its own; notes/gateway.md fix list)."""
        root = tree_id.split(".", 1)[0]
        doomed = [d for d, g in self._grants.items() if g.task_id == root or g.task_id.startswith(root + ".")]
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
