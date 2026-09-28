"""KeychainSecretStore (section 5 SecretStore, 10.1): FERB identity only.

Keychain item: generic password, service "ferb", account = SecretRef.key (e.g. "ferb/email/password"). The
owner creates it in their own terminal (`security add-generic-password -s ferb -a <key> -w`), never through chat.
Its SecretSpec (allowed origins, account hint) is not secret and lives in the owner-only config.toml.

There is no public reveal: `_reveal` is called only by cdp_identity's origin-bound fill, right before the value is
set on an element in a verified frame. Errors never carry the value, stdout or stderr.
"""

import asyncio
from pathlib import Path
from typing import Any, Callable

from ferb.core.types import SecretRef, SecretSpec

SERVICE = "ferb"


class SecretUnavailable(Exception):
    pass


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
