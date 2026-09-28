"""Daemon side of extension pairing (ARCH 9.6) — the decisions only; transport lives elsewhere.

- Tokens: one per (browser, folder), single-use, expire after TOKEN_TTL.
- Pairing binds install_id -> (browser, folder) and issues a per-install key (never logged).
- Unknown/unpaired install_ids may only send `hello` and `pair`. Everything else needs a valid MAC and a strictly
  increasing seq. Actions are only routed to paired installs.
"""

import base64
import hashlib
import hmac
import json
import secrets
from dataclasses import dataclass, field
from typing import Callable

TOKEN_TTL = 300.0  # seconds (monotonic)
UNPAIRED_ALLOWED = {"hello", "pair"}


@dataclass
class Install:
    browser: str
    folder: str
    key: bytes
    last_seq: int = 0


@dataclass(frozen=True)
class Verdict:
    accepted: bool
    reason: str
    reply: dict | None = None             # message to send back on this connection
    binding: tuple[str, str] | None = None  # (browser, folder) the message is attributed to


@dataclass
class Pairing:
    now: Callable[[], float]              # injected monotonic clock
    tokens: dict[str, tuple[str, str, float]] = field(default_factory=dict)
    installs: dict[str, Install] = field(default_factory=dict)

    def issue_token(self, browser: str, folder: str) -> str:
        token = secrets.token_urlsafe(24)
        self.tokens[token] = (browser, folder, self.now() + TOKEN_TTL)
        return token

    def handle(self, msg: dict) -> Verdict:
        install_id, mtype = msg.get("install_id"), msg.get("type")
        if not isinstance(install_id, str) or not isinstance(mtype, str):
            return Verdict(False, "malformed")
        inst = self.installs.get(install_id)

        if mtype == "pair":
            return self._pair(install_id, msg)
        if inst is None:
            if mtype in UNPAIRED_ALLOWED:
                return Verdict(True, "unpaired_" + mtype)
            return Verdict(False, "unpaired_install")          # no trail events, no results, nothing else
        if not self._authentic(inst, msg):
            return Verdict(False, "bad_mac")
        seq = msg["seq"]
        if seq <= inst.last_seq:
            return Verdict(False, "replay")
        inst.last_seq = seq
        return Verdict(True, "ok", binding=(inst.browser, inst.folder))

    def may_route_action(self, install_id: str) -> bool:
        return install_id in self.installs

    def _pair(self, install_id: str, msg: dict) -> Verdict:
        try:
            token = json.loads(msg.get("body_json", "{}")).get("token")
        except json.JSONDecodeError:
            return Verdict(False, "malformed")
        entry = self.tokens.pop(token, None) if isinstance(token, str) else None  # single-use: popped either way
        if entry is None:
            return Verdict(False, "unknown_token")
        browser, folder, expires = entry
        if self.now() > expires:
            return Verdict(False, "expired_token")
        key = secrets.token_bytes(32)
        self.installs[install_id] = Install(browser, folder, key)   # re-pairing replaces the old key
        return Verdict(True, "paired", reply={"type": "paired", "key": base64.b64encode(key).decode()},
                       binding=(browser, folder))

    @staticmethod
    def _authentic(inst: Install, msg: dict) -> bool:
        seq, mac = msg.get("seq"), msg.get("mac")
        if not isinstance(seq, int) or not isinstance(mac, str):
            return False
        text = f"{msg['install_id']}|{seq}|{msg['type']}|{msg.get('body_json', '')}".encode()
        expected = base64.b64encode(hmac.new(inst.key, text, hashlib.sha256).digest()).decode()
        return hmac.compare_digest(expected, mac)
