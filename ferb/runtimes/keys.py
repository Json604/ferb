"""Owns the provider API keys FERB keeps in the Keychain (D32). Must not log, store or return them anywhere else.

GATE-543 (owner 2026-11-08: "yes add it to the settings work"), GATE-547: a key is a Keychain item written and read
only through FERB's signed secret helper (ferb/identity/helper.py), service "ferb", account "api.sealed/<provider>".
The helper creates the item, so its access list trusts the helper's pinned binary rather than /usr/bin/security: a
generic tool (`security find-generic-password -w`) now gets a Keychain prompt instead of the value. What it does NOT
stop: the helper does no caller check, so any program running as the owner can run it and ask for a key (by design:
such a program could run FERB's signed Python anyway, and a caller check would change the helper's pinned bytes,
which asks again for every saved secret; GATE-548). What is enforced besides: only FERB.app may store or delete a key
(keys.store / keys.delete, APP_ONLY); FERB's sandbox keeps models, tasks and the shell away from the Keychain and
~/.ferb; a key never goes into an environment, a log, a span or the journal.

FERB.app hands a key to the daemon over its signed connection and the daemon stores it through the helper and reads
it back; the daemon reads it through the helper once per provider and keeps it in memory for its run (fewer Keychain
reads, so no new prompts, T-502), and registers it with the redactor.

An older item (account "api/<provider>", written by `security`: an earlier FERB.app's, or one a downgraded FERB wrote
again) is moved whenever one exists, on store and on read (GATE-547): on read it is the newer key (only an older build
writes it), so it is stored sealed, read back and only then deleted; on store the owner's new key replaces both. A
delete that fails is journaled (provider only). One lock per provider covers each read, move, store and delete.
Presence checks never read a value."""

import asyncio
import subprocess
import threading
from typing import Any, Callable, Iterable

from ferb.identity.secrets import SERVICE

Problem = Callable[[str, dict[str, Any]], None]

_cache: dict[str, str] = {}
_locks: dict[str, threading.Lock] = {}
_guard = threading.Lock()


def _lock(provider: str) -> threading.Lock:
    with _guard:
        return _locks.setdefault(provider, threading.Lock())


def account(provider: str) -> str:
    """The older item's account (written by `security`): read only to move it."""
    from ferb.runtimes.catalog import api_providers
    if provider not in api_providers():
        raise ValueError(f"unknown provider {provider!r}")
    return f"api/{provider}"


def sealed(provider: str) -> str:
    """The helper's item for this provider's key."""
    return "api.sealed/" + account(provider).removeprefix("api/")


def _helper(body: dict[str, Any]) -> dict[str, Any]:
    from ferb.identity.helper import request
    return request(body)


helper: Callable[[dict[str, Any]], dict[str, Any]] = _helper        # tests swap a fake Keychain in


def _security(*args: str) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(["security", *args], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)


def _present(acct: str) -> bool:
    """Presence only: no -w, so no value is read and no prompt is shown."""
    return _security("find-generic-password", "-s", SERVICE, "-a", acct).returncode == 0


def _sealed_value(provider: str) -> str:
    from ferb.core.errors import Refused
    try:
        return str(helper({"operation": "lookup", "service": SERVICE, "account": sealed(provider)}).get("value") or "")
    except Refused:
        return ""


def _store_sealed(provider: str, value: str) -> bool:
    """Store through the helper and read back through it; True when it reads back the same."""
    from ferb.core.errors import Refused
    try:
        helper({"operation": "store", "service": SERVICE, "account": sealed(provider), "value": value})
    except Refused:
        return False
    return _sealed_value(provider) == value


def _delete_old(provider: str, problem: Problem | None) -> bool:
    """Delete the older `security` item; a failure is journaled with the provider only. True when it is gone."""
    gone = _security("delete-generic-password", "-s", SERVICE, "-a", account(provider)).returncode == 0
    if not gone and _present(account(provider)):
        if problem is not None:
            problem("keys.old_item_not_deleted", {"provider": provider})
        return False
    return True


def store_key(provider: str, value: str, problem: Problem | None = None) -> bool:
    """The owner's key: stored through the helper and read back; an older item is then deleted (the new key replaces
    it). True when the new key reads back the same."""
    with _lock(provider):
        if not _store_sealed(provider, value):
            return False
        _cache[provider] = value
        if _present(account(provider)):
            _delete_old(provider, problem)
        return True


def delete_key(provider: str, problem: Problem | None = None) -> bool:
    """The owner's Remove key: the helper's item and any older one go; True when neither is left."""
    from ferb.core.errors import Refused
    with _lock(provider):
        _cache.pop(provider, None)
        try:
            helper({"operation": "delete", "service": SERVICE, "account": sealed(provider)})
        except Refused:
            pass
        if _present(account(provider)):
            _delete_old(provider, problem)
        _cache.pop(provider, None)
        return not has_api_key(provider)


def read_key(provider: str, problem: Problem | None = None) -> str | None:
    """The provider's key: from memory, else an older item (the newer key) moved through the helper, else the
    helper's item."""
    with _lock(provider):
        if provider in _cache:
            return _cache[provider]
        value = ""
        if _present(account(provider)):
            out = _security("find-generic-password", "-s", SERVICE, "-a", account(provider), "-w")
            old = out.stdout.decode().strip() if out.returncode == 0 else ""
            if old and _store_sealed(provider, old):     # stored and read back through the helper: the old item goes
                _delete_old(provider, problem)
            value = old
        value = value or _sealed_value(provider)
        if value:
            _cache[provider] = value
        return value or None


async def api_key(provider: str, problem: Problem | None = None) -> str | None:
    return await asyncio.to_thread(read_key, provider, problem)


def stored_api_keys(providers: Iterable[str], problem: Problem | None = None) -> list[str]:
    """The API keys of `providers` (the ones the owner's runtime table uses), for the redactor's known-value registry
    only (read inside the daemon at start, in a thread); a provider with no key is skipped, and no other provider's
    key is ever read."""
    from ferb.runtimes.catalog import api_providers
    found = []
    for provider in sorted(set(providers) & set(api_providers())):
        if has_api_key(provider) and (key := read_key(provider, problem)):
            found.append(key)
    return found


def has_api_key(provider: str) -> bool:
    """Presence only (the helper's item, or an older one not moved yet): never a value."""
    return _present(sealed(provider)) or _present(account(provider))
