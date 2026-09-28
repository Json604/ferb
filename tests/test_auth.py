"""auth (section 2): per-role method lists, CLI by uid, run tokens (hash-only, expiring, revocable)."""

import pytest

from ferb.clock.clock import FakeClock
from ferb.core.errors import AuthError, Unsupported
from ferb.core.types import Identity
from ferb.daemon.auth import METHODS, Role, RunTokens, allowed, check_cli_peer, check_native_host


def test_each_role_only_gets_its_own_methods() -> None:
    assert allowed(Role.CLI, "trace.get")
    assert not allowed(Role.MCP_SHIM, "trace.get")
    assert not allowed(Role.MCP_SHIM, "logs.subscribe")
    assert METHODS[Role.NATIVE_HOST] == frozenset()


def test_cli_requires_the_owner_uid() -> None:
    check_cli_peer(501, 501)
    with pytest.raises(AuthError):
        check_cli_peer(502, 501)


def test_native_host_fails_explicitly_until_pairing_exists() -> None:
    with pytest.raises(Unsupported):
        check_native_host()


def test_run_token_lifecycle_expire_and_revoke() -> None:
    clock = FakeClock()
    tokens = RunTokens(clock)
    t = tokens.issue("task-1", "w-1", {"fs:read"}, Identity.OWNER, ttl_s=60)
    g = tokens.validate(t)
    assert (g.task_id, g.worker_id, g.scopes, g.identity) == ("task-1", "w-1", frozenset({"fs:read"}), Identity.OWNER)
    clock.advance(61)
    with pytest.raises(AuthError, match="expired"):
        tokens.validate(t)
    t2 = tokens.issue("task-1", "w-2", set(), Identity.OWNER, ttl_s=60)
    g2 = tokens.validate(t2)
    assert tokens.revoke_all() == 1
    with pytest.raises(AuthError):
        tokens.validate(t2)
    with pytest.raises(AuthError, match="revoked"):
        tokens.recheck(g2)              # an open connection loses access at its next call


def test_revoke_one_worker_only() -> None:
    tokens = RunTokens(FakeClock())
    a = tokens.issue("t", "w-a", set(), Identity.OWNER, 60)
    b = tokens.issue("t", "w-b", set(), Identity.OWNER, 60)
    assert tokens.revoke_worker("w-a") == 1
    with pytest.raises(AuthError):
        tokens.validate(a)
    tokens.validate(b)


def test_token_value_is_never_stored() -> None:
    tokens = RunTokens(FakeClock())
    t = tokens.issue("t", "w", set(), Identity.OWNER, 60)
    assert t not in tokens._grants                      # keyed by hash
    assert t not in repr(tokens._grants)
    with pytest.raises(AuthError):
        tokens.validate(t + "x")
