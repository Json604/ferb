"""Owns the provider API keys FERB keeps in the Keychain (D32). Must not log, store or return them anywhere else.

Service "ferb", account "api/<provider>". `ferb setup --key <provider>` stores one (the `security` tool prompts for it, so
it is never an argument); runtimes read it at call time and the daemon adds it to its redactor."""

import asyncio
import subprocess
from typing import Iterable

from ferb.identity.secrets import SERVICE


def account(provider: str) -> str:
    from ferb.runtimes.catalog import api_providers
    if provider not in api_providers():
        raise ValueError(f"unknown provider {provider!r}")
    return f"api/{provider}"


async def api_key(provider: str) -> str | None:
    proc = await asyncio.create_subprocess_exec("security", "find-generic-password", "-s", SERVICE, "-a",
                                                account(provider), "-w", stdout=asyncio.subprocess.PIPE,
                                                stderr=asyncio.subprocess.DEVNULL)
    out, _ = await proc.communicate()
    key = out.decode().strip() if proc.returncode == 0 else ""
    return key or None


def stored_api_keys(providers: Iterable[str]) -> list[str]:
    """The API keys of `providers` (the ones the owner's runtime table uses), for the redactor's known-value registry
    only (read inside the daemon at start, in a thread); a provider with no key is skipped, and no other provider's
    key is ever read."""
    from ferb.runtimes.catalog import api_providers
    found = []
    for provider in sorted(set(providers) & set(api_providers())):
        out = subprocess.run(["security", "find-generic-password", "-s", SERVICE, "-a", account(provider), "-w"],
                             stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        if out.returncode == 0 and (key := out.stdout.decode().strip()):
            found.append(key)
    return found


def has_api_key(provider: str) -> bool:
    """Presence only: no -w, so the value is never read."""
    return subprocess.run(["security", "find-generic-password", "-s", SERVICE, "-a", account(provider)],
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0
