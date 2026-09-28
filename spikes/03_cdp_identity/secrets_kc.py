"""Keychain-backed secret store for FERB's own identity (spike version of identity/secrets.py).

Keychain item: generic password, service "ferb", account = SecretRef.key (e.g. "ferb/email/password").
SecretSpec (allowed origins, account hint) is not secret and lives outside the Keychain; here it is passed in.
No public reveal: only fill.py calls `_reveal`, right before writing into a verified frame.
"""

import subprocess
from dataclasses import dataclass
from pathlib import Path

SERVICE = "ferb"


@dataclass(frozen=True)
class SecretRef:
    key: str


@dataclass(frozen=True)
class SecretSpec:
    ref: SecretRef
    allowed_origins: list[str]
    account_hint: str


class KeychainSecretStore:
    def __init__(self, specs: list[SecretSpec], keychain: Path | None = None) -> None:
        self._specs = {s.ref.key: s for s in specs}
        self._keychain = [str(keychain)] if keychain else []  # None -> default search list

    def _find(self, ref: SecretRef, reveal: bool) -> subprocess.CompletedProcess:
        cmd = ["security", "find-generic-password", "-s", SERVICE, "-a", ref.key]
        if reveal:
            cmd.append("-w")
        return subprocess.run(cmd + self._keychain, capture_output=True, text=True)

    def has(self, ref: SecretRef) -> bool:
        return ref.key in self._specs and self._find(ref, reveal=False).returncode == 0

    def spec(self, ref: SecretRef) -> SecretSpec:
        return self._specs[ref.key]

    def _reveal(self, ref: SecretRef) -> str:
        r = self._find(ref, reveal=True)
        if r.returncode != 0:
            raise KeyError(ref.key)  # never include stderr/stdout: keep secrets out of messages
        return r.stdout.removesuffix("\n")
