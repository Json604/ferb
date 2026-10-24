"""Shared helpers for tests of FERB's hidden-input questions (D12, L36 §5): the question a hidden input waits on, and a
stand-in for the signed Keychain helper, so no test reaches the owner's Keychain."""
import time
from typing import Any

import pytest


def asked(daemon: Any, kind: str = 'owner_secret') -> Any:
    """The first waiting question of `kind`, within 5 s."""
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        found = [ap for ap in daemon.daemon.state.approvals.pending() if ap.kind == kind]
        if found:
            return found[0]
        time.sleep(0.01)
    raise AssertionError(f'no {kind} question was asked')


def keychain_helper(monkeypatch: pytest.MonkeyPatch) -> dict[str, str]:
    """`ferb.identity.helper.request` replaced by a dict (Keychain account -> value) for service ferb.owner-secrets."""
    keychain: dict[str, str] = {}

    def helper(body: dict[str, Any], home: Any = None) -> dict[str, Any]:
        assert body['service'] == 'ferb.owner-secrets'
        if body['operation'] == 'store':
            keychain[body['account']] = body['value']
        return {'value': keychain.get(body['account'])}
    monkeypatch.setattr('ferb.identity.helper.request', helper)
    return keychain
