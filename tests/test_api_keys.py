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
        self.delete_fails = False

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
            if self.delete_fails:
                return subprocess.CompletedProcess(cmd, 1, b"")
            gone = self.security_items.pop(acct, None) is not None
            return subprocess.CompletedProcess(cmd, 0 if gone else 44, b"")
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


# decision: GATE-547
def test_a_new_key_replaces_an_older_item_even_for_a_provider_not_in_use(keychain: FakeKeychain) -> None:
    """Review #1 (a): a provider the runtime table doesn't use is never read at start, so saving its key in Settings
    must remove the older, `security`-readable item itself."""
    keychain.security_items["api/openai"] = "sk-old"
    assert keys.store_key("openai", "sk-new")
    assert keychain.helper_items == {"api.sealed/openai": "sk-new"} and keychain.security_items == {}


# decision: GATE-547
def test_an_older_item_beside_a_sealed_one_is_the_newer_key_and_is_moved(keychain: FakeKeychain) -> None:
    """Review #1 (b): a downgraded FERB showed "no key", the owner entered it again and that build wrote the older
    item; back on this build the older item is the newer key: it is read, sealed, read back and deleted."""
    keychain.helper_items["api.sealed/anthropic"] = "sk-stale"
    keychain.security_items["api/anthropic"] = "sk-reentered"
    assert keys.read_key("anthropic") == "sk-reentered"
    assert keychain.helper_items == {"api.sealed/anthropic": "sk-reentered"} and keychain.security_items == {}


# decision: GATE-547
def test_an_older_item_that_wont_delete_is_journaled_by_provider_only(keychain: FakeKeychain) -> None:
    keychain.security_items["api/google"] = "g-old"
    keychain.delete_fails = True
    seen: list[tuple[str, dict[str, Any]]] = []
    assert keys.read_key("google", lambda kind, data: seen.append((kind, data))) == "g-old"
    assert seen == [("keys.old_item_not_deleted", {"provider": "google"})]


# decision: GATE-547
def test_remove_key_leaves_nothing_in_memory(keychain: FakeKeychain) -> None:
    assert keys.store_key("openai", "sk-1") and keys.read_key("openai") == "sk-1"
    assert keys.delete_key("openai")
    assert keys.read_key("openai") is None and keys._cache == {}


# decision: GATE-547
def test_check_calls_and_key_changes_are_for_ferb_app_only() -> None:
    from ferb.daemon.auth import APP_ONLY
    assert {"keys.store", "keys.delete", "runtimes.check_calls"} <= APP_ONLY


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
