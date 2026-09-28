"""Extension install <-> browser profile pairing (section 9.6): the daemon's decisions, persisted.

- FERB issues a one-time token for (browser, folder, identity); it's handed to the extension only through the
  extension's own pairing page opened in that profile. Tokens are single-use and expire in minutes.
- Pairing binds install_id -> (browser, folder, identity) and issues a per-install key (never logged or returned
  by any method). Re-pairing a profile replaces its previous install.
- An unpaired install may send only `hello` and `pair`. Everything else needs a valid MAC and a strictly increasing
  seq (persisted, so a daemon restart doesn't reopen replays).
"""

import base64
import secrets
from dataclasses import dataclass
from typing import Any

from ferb.act.browser.ext_protocol import ExtMessage, authentic
from ferb.core.interfaces import Clock
from ferb.core.types import Identity
from ferb.store.db import Database

TOKEN_TTL_S = 300.0
UNPAIRED_ALLOWED = frozenset({"hello", "pair"})


@dataclass(frozen=True)
class Binding:
    install_id: str
    browser: str
    folder: str
    identity: Identity


@dataclass(frozen=True)
class Verdict:
    accepted: bool
    reason: str
    reply: dict[str, Any] | None = None       # sent back on this connection
    binding: Binding | None = None             # who the message is attributed to


class Pairings:
    def __init__(self, db: Database, clock: Clock) -> None:
        self.db, self.clock = db, clock
        self._tokens: dict[str, tuple[str, str, Identity, float]] = {}

    def issue_token(self, browser: str, folder: str, identity: Identity = Identity.OWNER) -> str:
        token = secrets.token_urlsafe(24)
        self._tokens[token] = (browser, folder, identity, self.clock.monotonic() + TOKEN_TTL_S)
        return token

    def binding(self, install_id: str) -> Binding | None:
        r = self.db.conn.execute("SELECT browser, folder, identity FROM ext_installs WHERE install_id = ?",
                                 (install_id,)).fetchone()
        return Binding(install_id, r["browser"], r["folder"], Identity(r["identity"])) if r else None

    def for_profile(self, browser: str, folder: str) -> Binding | None:
        r = self.db.conn.execute("SELECT install_id FROM ext_installs WHERE browser = ? AND folder = ?",
                                 (browser, folder)).fetchone()
        return self.binding(r["install_id"]) if r else None

    def all(self) -> list[Binding]:
        rows = self.db.conn.execute("SELECT install_id FROM ext_installs ORDER BY browser, folder").fetchall()
        return [b for r in rows if (b := self.binding(r["install_id"])) is not None]

    def unpair(self, browser: str, folder: str) -> bool:
        return self.db.conn.execute("DELETE FROM ext_installs WHERE browser = ? AND folder = ?",
                                    (browser, folder)).rowcount > 0

    def handle(self, m: ExtMessage) -> Verdict:
        if m.type == "pair":
            return self._pair(m)
        r = self.db.conn.execute("SELECT key, last_seq FROM ext_installs WHERE install_id = ?",
                                 (m.install_id,)).fetchone()
        if r is None:
            if m.type in UNPAIRED_ALLOWED:
                return Verdict(True, "unpaired_" + m.type)
            return Verdict(False, "unpaired_install")          # no trail events, no results, nothing else
        if not authentic(bytes(r["key"]), m):
            return Verdict(False, "bad_mac")
        assert m.seq is not None
        if m.seq <= int(r["last_seq"]):
            return Verdict(False, "replay")
        self.db.conn.execute("UPDATE ext_installs SET last_seq = ? WHERE install_id = ?", (m.seq, m.install_id))
        return Verdict(True, "ok", binding=self.binding(m.install_id))

    def _pair(self, m: ExtMessage) -> Verdict:
        try:
            token = m.body().get("token")
        except ValueError:
            return Verdict(False, "malformed")
        entry = self._tokens.pop(token, None) if isinstance(token, str) else None   # single-use either way
        if entry is None:
            return Verdict(False, "unknown_token")
        browser, folder, identity, expires = entry
        if self.clock.monotonic() > expires:
            return Verdict(False, "expired_token")
        key = secrets.token_bytes(32)
        c = self.db.conn
        c.execute("BEGIN")
        c.execute("DELETE FROM ext_installs WHERE (browser = ? AND folder = ?) OR install_id = ?",
                  (browser, folder, m.install_id))
        c.execute("INSERT INTO ext_installs (install_id, browser, folder, identity, key, last_seq, paired_at) "
                  "VALUES (?, ?, ?, ?, ?, 0, ?)",
                  (m.install_id, browser, folder, identity.value, key, self.clock.now().isoformat()))
        c.execute("COMMIT")
        return Verdict(True, "paired", reply={"type": "paired", "key": base64.b64encode(key).decode()},
                       binding=Binding(m.install_id, browser, folder, identity))

