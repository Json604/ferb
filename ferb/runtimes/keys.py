"""Owns the provider API keys FERB keeps in the Keychain (D32). Must not log, store or return them anywhere else.

Service "ferb", account "api/<provider>". `ferb setup --key <provider>` stores one (the `security` tool prompts for it, so
it is never an argument); runtimes read it at call time and the daemon adds it to its redactor."""

import asyncio
import subprocess

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


def has_api_key(provider: str) -> bool:
    """Presence only: no -w, so the value is never read."""
    return subprocess.run(["security", "find-generic-password", "-s", SERVICE, "-a", account(provider)],
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0
