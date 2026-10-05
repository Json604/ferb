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

    async def _reveal(self, ref: SecretRef) -> str:
        if ref.key not in self._specs():
            raise SecretUnavailable(ref.key)
        code, out = await self._find(ref, reveal=True)
        if code != 0:
            raise SecretUnavailable(ref.key)             # never include output: keep secrets out of messages
        return out.removesuffix("\n")

    async def _find(self, ref: SecretRef, reveal: bool) -> tuple[int, str]:
        cmd = ["security", "find-generic-password", "-s", SERVICE, "-a", ref.key, *(["-w"] if reveal else [])]
        proc = await asyncio.create_subprocess_exec(*cmd, *self._keychain, stdout=asyncio.subprocess.PIPE,
                                                    stderr=asyncio.subprocess.DEVNULL)
        out, _ = await proc.communicate()
        return proc.returncode or 0, out.decode() if reveal else ""
