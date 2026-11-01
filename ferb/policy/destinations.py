"""Owns the destinations FERB knows (SQ6) and the owner's recorded confirmations (D33, S4). Must not judge intent or
content, or decide whether a call needs the owner (ferb/policy/confirm.py decides from these facts).

A known destination is one the owner named in a request (emails, URL hosts and phone numbers in their own words, by
shape), chose in an answer, or confirmed to FERB's question (GATE-205): every known one is known for normal
use in later tasks (no new-recipient question), but only one the owner approved (chose in an answer or confirmed) is
known for the sink rule, so sending the owner's private data to a site only mentioned in a request, after outside
content was read, still asks once (`approved`).
Values are normalized: an email or host lowercased, a phone number's digits, a name casefolded. Never message content.
A confirmation is the owner's Yes or No to FERB's question in one task tree, keyed by what it covers; it carries
across that tree's retries (rule 9), except that the owner's own later message carrying the task on reopens a No (FERB
asks it again; L11 round 4). A used row (an escape run, a payment that ran or is running) covers nothing more.
Rule 14: what is stored goes through the redactor first. A destination or key holding a specific secret (a saved
secret's value or a credential format: `Redactor.hit`) is never known and never covered: FERB asks again. Any other
destination or key the redactor would change (a long number, a label-like word) is stored and compared by a stable
digest of its normalized form (`stable`), never by its redacted text, so two values never share one stored
"[redacted]" and one that merely looks secret is still remembered."""

import hashlib
from typing import TYPE_CHECKING, Any, Callable, Collection

from ferb.core.interfaces import Clock, IdSource
from ferb.policy import provenance

if TYPE_CHECKING:
    from ferb.store.db import Database

KINDS = ("email", "phone", "host", "name")
HOW = ("named", "answered", "confirmed")
APPROVED = ("answered", "confirmed")      # how a destination the owner approved became known (GATE-205)
Redact = Callable[[Any], Any]


def _redacted(redact: Redact, text: str) -> str:
    return str(redact({"value": text})["value"])


def stable(redact: Redact, text: str) -> str | None:
    """The form a key or destination is stored and compared in (rule 14): None when it holds a specific secret
    (`Redactor.hit`; a redactor without `hit` counts any change it would make), the text itself when the redactor
    leaves it alone, else "sha256:" and the digest of the text, so it is remembered without being stored."""
    hit = getattr(redact, "hit", None)
    if hit is not None and hit(text):
        return None
    if _redacted(redact, text) == text:
        return text
    if hit is None:
        return None
    return "sha256:" + hashlib.sha256(text.encode()).hexdigest()


def destination_of(value: str) -> tuple[str, str]:
    """(kind, value) of a recipient or contact by its shape: an email address, a phone number, a URL's host, else a
    name."""
    shape = provenance.destination_shape(value)
    if shape == "url":
        return "host", provenance.host_of(value.strip())
    return (shape if shape in ("email", "phone") else "name"), value


class KnownDestinations:
    def __init__(self, db: "Database", clock: Clock, ids: IdSource, redact: Redact) -> None:
        self.db, self.clock, self.ids, self.redact = db, clock, ids, redact

    @staticmethod
    def normalize(kind: str, value: str) -> str:
        text = str(value or "").strip()
        if kind == "phone":
            return "".join(c for c in text if c.isdigit())
        if kind == "host":
            return text.lower().rstrip(".")
        if kind == "email":
            return text.lower()
        return " ".join(text.casefold().split())

    def _stored(self, kind: str, value: str) -> str | None:
        norm = self.normalize(kind, value)
        return stable(self.redact, norm) if kind in KINDS and norm else None

    def known(self, kind: str, value: str) -> bool:
        """Known for normal use: named in a request, chosen in an answer or confirmed."""
        stored = self._stored(kind, value)
        return stored is not None and self.db.conn.execute(
            "SELECT 1 FROM known_destinations WHERE kind = ? AND value = ?", (kind, stored)).fetchone() is not None

    def approved(self, kind: str, value: str) -> bool:
        """Known for the sink rule (GATE-205): the owner chose it in an answer or confirmed it to FERB's question;
        a destination only named in a request is not."""
        stored = self._stored(kind, value)
        return stored is not None and self.db.conn.execute(
            "SELECT 1 FROM known_destinations WHERE kind = ? AND value = ? AND how IN ('answered', 'confirmed')",
            (kind, stored)).fetchone() is not None

    def record(self, kind: str, value: str, how: str, request_id: str | None = None,
               span_id: str | None = None) -> None:
        """Keep a destination; one already known keeps how it first became known, except that the owner's approval
        of one only named before upgrades it (GATE-205)."""
        stored = self._stored(kind, value)
        if stored is None or how not in HOW:
            return
        self.db.conn.execute(
            "INSERT INTO known_destinations (id, kind, value, how, request_id, span_id, at) VALUES (?, ?, ?, ?, ?, ?, ?) "
            "ON CONFLICT(kind, value) DO UPDATE SET how = excluded.how, request_id = excluded.request_id, "
            "span_id = excluded.span_id, at = excluded.at "
            "WHERE known_destinations.how = 'named' AND excluded.how != 'named'",
            (self.ids.new_id(), kind, stored, how, request_id, span_id, self.clock.now().isoformat()))

    @staticmethod
    def from_words(words: str) -> list[tuple[str, str]]:
        """The destinations in a text by shape, in order: whole email tokens, the hosts of URL tokens, and phone-like
        runs of at least provenance.MIN_PHONE_DIGITS digits. Names are never taken from text."""
        found: list[tuple[str, str]] = []
        for tok in provenance.raw_tokens(words):
            if provenance.destination_shape(tok) == "email":
                found.append(("email", tok.lower()))
            elif host := provenance.host_of(tok):
                found.append(("host", host))
        found += [("phone", digits) for digits in provenance.phone_runs(words)]
        return list(dict.fromkeys(found))


class Confirmations:
    def __init__(self, db: "Database", clock: Clock, ids: IdSource, redact: Redact) -> None:
        self.db, self.clock, self.ids, self.redact = db, clock, ids, redact

    def find(self, tree_id: str, key: str, reopened: Collection[str] = ()) -> dict[str, Any] | None:
        """The owner's latest answer in this task tree to the question `key` names, or None (always None for a key
        holding a secret: it is stored redacted, so it could match another one). `reopened`: ids of No answers the
        owner's own later message carrying the task on reopened (Gateway.reopen_noes); such a No reads as unanswered,
        so a caller that asks asks again. A caller that never asks passes none: the No stays for it."""
        stored = stable(self.redact, key)
        if stored is None:
            return None
        row = self.db.conn.execute("SELECT * FROM confirmations WHERE tree_id = ? AND key = ? "
                                   "ORDER BY rowid DESC LIMIT 1", (tree_id, stored)).fetchone()
        if row is None or (not row["granted"] and row["id"] in reopened):
            return None
        return dict(row)

    def noes(self, tree_id: str) -> list[str]:
        """The ids of this tree's answers that are each question's latest and a No."""
        latest: dict[str, Any] = {}
        for row in self.db.conn.execute("SELECT id, key, granted FROM confirmations WHERE tree_id = ? ORDER BY rowid",
                                        (tree_id,)):
            latest[row["key"]] = row
        return [row["id"] for row in latest.values() if not row["granted"]]

    def refused(self, tree_id: str) -> list[str]:
        """The keys whose latest answer in this task tree is No."""
        latest = {row["key"]: row["granted"] for row in self.db.conn.execute(
            "SELECT key, granted FROM confirmations WHERE tree_id = ? ORDER BY rowid", (tree_id,))}
        return [key for key, granted in latest.items() if not granted]

    def covers(self, cid: str, key: str) -> bool:
        """Whether row `cid` is a Yes to the question `key` names (never for a key holding a secret)."""
        stored = stable(self.redact, key)
        if stored is None:
            return False
        row = self.db.conn.execute("SELECT 1 FROM confirmations WHERE id = ? AND key = ? AND granted = 1",
                                   (cid, stored)).fetchone()
        return row is not None

    def record(self, tree_id: str, rule: str, key: str, granted: bool, ask_span: str | None) -> str:
        """The owner's answer, keyed by its stable form; a key holding a specific secret is stored redacted (it never
        matches again, so FERB asks again)."""
        cid = str(self.ids.new_id())
        stored = stable(self.redact, key)
        self.db.conn.execute("INSERT INTO confirmations (id, tree_id, rule, key, granted, ask_span, used, at) "
                             "VALUES (?, ?, ?, ?, ?, ?, 0, ?)",
                             (cid, tree_id, rule, stored if stored is not None else _redacted(self.redact, key),
                              int(granted), ask_span, self.clock.now().isoformat()))
        return cid

    def age_s(self, row: dict[str, Any]) -> float:
        """Seconds since that answer was recorded."""
        from datetime import datetime
        return (self.clock.now() - datetime.fromisoformat(str(row["at"]))).total_seconds()

    def use(self, cid: str) -> None:
        """A Yes that covers one run only (an escape, a payment, an account-security change) is claimed by the call
        about to run; nothing else is covered by it."""
        self.db.conn.execute("UPDATE confirmations SET used = 1 WHERE id = ?", (cid,))

    def release(self, cid: str) -> None:
        """The claiming call never made its attempt (refused, or cancelled before it ran) or the tool reported it
        failed: the Yes covers its retry again. After an unknown outcome the Yes stays spent; only the identical retry
        the owner approves under P19 may use it once more (the gateway checks the ledger)."""
        self.db.conn.execute("UPDATE confirmations SET used = 0 WHERE id = ?", (cid,))
