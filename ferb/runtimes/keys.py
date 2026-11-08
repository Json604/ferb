"""Owns the provider API keys FERB keeps in the Keychain (D32). Must not log, store or return them anywhere else.

GATE-543 (owner 2026-11-08: "yes add it to the settings work"): a key is a Keychain item written and read only through
FERB's signed secret helper (ferb/identity/helper.py), service "ferb", account "api.sealed/<provider>", so the item's
access list trusts that helper, not /usr/bin/security (an item `security` wrote, as FERB.app's Settings did before,
trusts /usr/bin/security, which any program of the owner's can run to read it: checked 2026-11-08 on the owner's
item's ACL, never its value). FERB.app hands a key to the daemon over its signed connection (keys.store, APP_ONLY) and
the daemon stores it through the helper; the daemon reads it through the helper once per provider and keeps it in
memory for its run (fewer Keychain reads, so no new prompts, T-502), and registers it with the redactor.

An older item (account "api/<provider>", written by `security`) is moved on first use: read once through `security`,
stored through the helper, read back through the helper, and only then deleted. Presence checks never read a value."""

import asyncio
import subprocess
import threading
from typing import Any, Callable, Iterable

from ferb.identity.secrets import SERVICE

_cache: dict[str, str] = {}
_lock = threading.Lock()


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


def store_key(provider: str, value: str) -> bool:
    """Store a key through the helper and read it back through the helper; True when it reads back the same."""
    from ferb.core.errors import Refused
    try:
        helper({"operation": "store", "service": SERVICE, "account": sealed(provider), "value": value})
    except Refused:
        return False
    if _sealed_value(provider) != value:
        return False
    with _lock:
        _cache[provider] = value
    return True


def delete_key(provider: str) -> bool:
    """The owner's Remove key: the helper's item and any older one go; True when neither is left."""
    from ferb.core.errors import Refused
    with _lock:
        _cache.pop(provider, None)
    try:
        helper({"operation": "delete", "service": SERVICE, "account": sealed(provider)})
    except Refused:
        pass
    if _present(account(provider)):
        _security("delete-generic-password", "-s", SERVICE, "-a", account(provider))
    return not has_api_key(provider)


def read_key(provider: str) -> str | None:
    """The provider's key: from memory, else through the helper, else an older item moved through the helper first."""
    with _lock:
        if provider in _cache:
            return _cache[provider]
    value = _sealed_value(provider)
    if not value and _present(account(provider)):
        out = _security("find-generic-password", "-s", SERVICE, "-a", account(provider), "-w")
        old = out.stdout.decode().strip() if out.returncode == 0 else ""
        if old and store_key(provider, old):          # stored and read back through the helper: the old item goes
            _security("delete-generic-password", "-s", SERVICE, "-a", account(provider))
        value = old
    if value:
        with _lock:
            _cache[provider] = value
    return value or None


async def api_key(provider: str) -> str | None:
    return await asyncio.to_thread(read_key, provider)


def stored_api_keys(providers: Iterable[str]) -> list[str]:
    """The API keys of `providers` (the ones the owner's runtime table uses), for the redactor's known-value registry
    only (read inside the daemon at start, in a thread); a provider with no key is skipped, and no other provider's
    key is ever read."""
    from ferb.runtimes.catalog import api_providers
    found = []
    for provider in sorted(set(providers) & set(api_providers())):
        if has_api_key(provider) and (key := read_key(provider)):
            found.append(key)
    return found


def has_api_key(provider: str) -> bool:
    """Presence only (the helper's item, or an older one not moved yet): never a value."""
    return _present(sealed(provider)) or _present(account(provider))
