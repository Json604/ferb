# tests/test_api_keys.py
"""Provider API keys live in the Keychain and nowhere else (D32, rule 14), written and read through FERB's signed
secret helper (GATE-543); an older item `security` wrote is moved on first use. A fake Keychain only: no real one."""
import subprocess
from typing import Any

import pytest

from ferb.core.errors import Refused
from ferb.runtimes import keys


class FakeKeychain:
    """The login Keychain as the helper and `security` see it: (service, account) -> value, by who wrote it."""

    def __init__(self) -> None:
        self.helper_items: dict[str, str] = {}
        self.security_items: dict[str, str] = {}
        self.reads: list[str] = []

    def helper(self, body: dict[str, Any]) -> dict[str, Any]:
        assert body["service"] == "ferb"
        acct, op = body["account"], body["operation"]
        if op == "store":
            self.helper_items[acct] = body["value"]
            return {"ok": True}
        if op == "delete":
            return {"deleted": self.helper_items.pop(acct, None) is not None}
        if op == "lookup":
            self.reads.append("helper:" + acct)
            if acct not in self.helper_items:
                raise Refused("the Keychain didn't show this item")
            return {"value": self.helper_items[acct]}
        raise AssertionError(op)

    def security(self, cmd: list[str], **kw: Any) -> subprocess.CompletedProcess[bytes]:
        assert cmd[0] == "security"
        verb, acct = cmd[1], cmd[cmd.index("-a") + 1]
        items = {**self.security_items, **self.helper_items}
        if verb == "find-generic-password":
            if acct not in items:
                return subprocess.CompletedProcess(cmd, 44, b"")
            if "-w" in cmd:
                self.reads.append("security:" + acct)
                return subprocess.CompletedProcess(cmd, 0, (items[acct] + "\n").encode())
            return subprocess.CompletedProcess(cmd, 0, b"")
        if verb == "delete-generic-password":
            self.security_items.pop(acct, None)
            return subprocess.CompletedProcess(cmd, 0, b"")
        raise AssertionError(verb)


@pytest.fixture
def keychain(monkeypatch: pytest.MonkeyPatch) -> FakeKeychain:
    fake = FakeKeychain()
    monkeypatch.setattr(keys, "helper", fake.helper)
    monkeypatch.setattr(subprocess, "run", fake.security)
    keys._cache.clear()
    yield fake
    keys._cache.clear()


# decision: GATE-543, GATE-544
def test_a_key_is_stored_and_read_through_the_helper_only(keychain: FakeKeychain) -> None:
    assert keys.store_key("openai", "sk-test-123")
    assert keychain.helper_items == {"api.sealed/openai": "sk-test-123"} and keychain.security_items == {}
    keys._cache.clear()
    assert keys.read_key("openai") == "sk-test-123" and keys.read_key("anthropic") is None
    assert not any(r.startswith("security:") for r in keychain.reads)        # never through /usr/bin/security
    reads = len(keychain.reads)
    assert keys.read_key("openai") == "sk-test-123" and len(keychain.reads) == reads   # kept in memory: no more reads
    assert keys.has_api_key("openai") and not keys.has_api_key("google")
    assert keys.delete_key("openai") and not keys.has_api_key("openai")


# decision: GATE-543
def test_an_older_item_is_moved_once_and_deleted_only_after_it_reads_back(keychain: FakeKeychain,
                                                                          monkeypatch: pytest.MonkeyPatch) -> None:
    keychain.security_items["api/anthropic"] = "sk-ant-old-456"
    assert keys.has_api_key("anthropic")
    assert keys.read_key("anthropic") == "sk-ant-old-456"
    assert keychain.helper_items == {"api.sealed/anthropic": "sk-ant-old-456"} and keychain.security_items == {}
    # a store that doesn't read back keeps the older item: nothing is lost
    keychain.security_items["api/google"] = "g-old-789"
    monkeypatch.setattr(keys, "_sealed_value", lambda provider: "")
    assert keys.read_key("google") == "g-old-789" and keychain.security_items == {"api/google": "g-old-789"}


# decision: GATE-543
def test_the_apps_key_is_stored_sealed_and_never_journaled(daemon: Any, keychain: FakeKeychain) -> None:
    d = daemon.daemon
    key = "sk-ant-" + "api03-" + "TESTKEYVALUE0123456789"   # made here: no key-shaped literal in the file
    assert daemon.run(d._keys_store(None, {"provider": "anthropic", "key": key})) == {"stored": True}
    assert keychain.helper_items == {"api.sealed/anthropic": key}
    assert d.redact.text(f"echo {key}") == "echo [redacted]"
    rows = daemon.run(_journal(d))
    assert rows and key not in str(rows)
    assert daemon.run(d._keys_delete(None, {"provider": "anthropic"})) == {"removed": True}
    assert keychain.helper_items == {}


async def _journal(d: Any) -> list[Any]:
    return [e.data for e in d._journal().of_kind("keys.stored")]


def test_unknown_provider_is_refused() -> None:
    with pytest.raises(ValueError):
        keys.account("someone")
