"""Sealed secrets for the extension path (section 10.1, safety test 21).

A secret or one-time code travels daemon -> native host -> extension only as AES-GCM ciphertext:
- key: HKDF-SHA256 of the per-install pairing key, label "ferb seal v1" (both sides derive it; it never crosses the
  wire),
- a fresh 12-byte nonce per seal,
- associated data "<fill command id>|<allowed origin>", so a sealed value opens only for that command and origin.
Only the extension's fill call opens it. This protects FERB's own messages and logs, not against malware running
as the owner's user.
"""

import base64
import os

from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.kdf.hkdf import HKDF

INFO = b"ferb seal v1"


def seal_key(pairing_key: bytes) -> bytes:
    return HKDF(algorithm=hashes.SHA256(), length=32, salt=None, info=INFO).derive(pairing_key)


def aad(command_id: str, origin: str) -> bytes:
    return f"{command_id}|{origin}".encode()


def seal(key: bytes, value: str, command_id: str, origin: str) -> dict[str, str]:
    nonce = os.urandom(12)
    ct = AESGCM(key).encrypt(nonce, value.encode(), aad(command_id, origin))
    return {"nonce": base64.b64encode(nonce).decode(), "ct": base64.b64encode(ct).decode()}


def unseal(key: bytes, sealed: dict[str, str], command_id: str, origin: str) -> str:
    """Tests and the fake extension only; the real extension opens seals in WebCrypto (protocol.js)."""
    return AESGCM(key).decrypt(base64.b64decode(sealed["nonce"]), base64.b64decode(sealed["ct"]),
                               aad(command_id, origin)).decode()
