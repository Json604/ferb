"""Owns configured FERB Keychain secret storage and exact origin constraints; must not expose values to models.
The account hint/origin specification is nonsecret. Only the driver's sealed fill reveals a value after origin checks.
Owner setup creates Keychain entries in their terminal; errors never include secret values or subprocess output."""

import asyncio
from pathlib import Path
from typing import Any, Callable
from urllib.parse import urlsplit

from ferb.core.types import SecretRef, SecretSpec

SERVICE = "ferb"


class SecretUnavailable(Exception):
    pass


def normalize_origin(origin: str) -> str:
    """scheme://host[:non-default port], lowercased host — the form allowed_origins are compared in."""
    u = urlsplit(origin)
    default = {"http": 80, "https": 443}.get(u.scheme)
    port = u.port
    return f"{u.scheme}://{u.hostname or ''}" + (f":{port}" if port and port != default else "")


def origin_allowed(origin: str, spec: SecretSpec) -> bool:
    return bool(origin) and normalize_origin(origin) in {normalize_origin(o) for o in spec.allowed_origins}


def specs_from_config(section: dict[str, Any] | None) -> dict[str, SecretSpec]:
    out = {}
    for key, v in (section or {}).items():
        out[key] = SecretSpec(SecretRef(key), [str(o) for o in v.get("allowed_origins", [])],
                              str(v.get("account_hint", "")))
    return out


class KeychainSecretStore:
    def __init__(self, specs: Callable[[], dict[str, SecretSpec]], keychain: Path | None = None) -> None:
        self._specs = specs                              # read at call time: config edits apply without restart
        self._keychain = [str(keychain)] if keychain else []   # None: the default search list (login keychain)

    def specs(self) -> list[SecretSpec]:
        return list(self._specs().values())

    def spec(self, ref: SecretRef) -> SecretSpec:
        return self._specs()[ref.key]

    def has(self, ref: SecretRef) -> bool:
        return ref.key in self._specs()                  # presence in Keychain is checked by `present`

    async def present(self, ref: SecretRef) -> bool:
        return ref.key in self._specs() and (await self._find(ref, reveal=False))[0] == 0

    def values(self) -> tuple[list[str], int]:
        """Every configured secret's value, for the redactor's known-value registry only (read inside the daemon at
        start, in a thread), and how many couldn't be read."""
        import subprocess
        found, unreadable = [], 0
        for key in list(self._specs()):
            try:
                if not self._keychain:
                    from ferb.identity.helper import request
                    value = str(request({'operation': 'lookup', 'service': SERVICE, 'account': key}).get('value', ''))
                else:
                    out = subprocess.run(["security", "find-generic-password", "-s", SERVICE, "-a", key, "-w",
                                          *self._keychain], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
                    value = out.stdout.decode().removesuffix("\n") if out.returncode == 0 else ""
            except Exception:
                value = ""
            if value:
                found.append(value)
            else:
                unreadable += 1
        return found, unreadable

    async def _reveal(self, ref: SecretRef) -> str:
        if ref.key not in self._specs():
            raise SecretUnavailable(ref.key)
        code, out = await self._find(ref, reveal=True)
        if code != 0:
            raise SecretUnavailable(ref.key)             # never include output: keep secrets out of messages
        return out.removesuffix("\n") if self._keychain else out

    async def _find(self, ref: SecretRef, reveal: bool) -> tuple[int, str]:
        if not self._keychain:
            from ferb.identity.helper import request
            try:
                out = await asyncio.to_thread(request, {'operation': 'lookup' if reveal else 'present',
                                                        'service': SERVICE, 'account': ref.key})
                return (0, str(out.get('value', ''))) if reveal or out.get('present') else (1, '')
            except Exception:
                return 1, ''
        cmd = ["security", "find-generic-password", "-s", SERVICE, "-a", ref.key, *(["-w"] if reveal else [])]
        proc = await asyncio.create_subprocess_exec(*cmd, *self._keychain, stdout=asyncio.subprocess.PIPE,
                                                    stderr=asyncio.subprocess.DEVNULL)
        out, _ = await proc.communicate()
        return proc.returncode or 0, out.decode() if reveal else ""
