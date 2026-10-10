"""Owns the destinations FERB knows (SQ6) and the owner's recorded confirmations (D33, S4). Must not judge intent or
content, or decide whether a call needs the owner (ferb/policy/confirm.py decides from these facts).

A known destination is one the owner named in a request (emails, URL hosts and phone numbers in their own words, by
shape), chose in an answer, or confirmed to FERB's question; it is never "new" for the sink rule in a later task.
Values are normalized: an email or host lowercased, a phone number's digits, a name casefolded. Never message content.
A confirmation is the owner's Yes or No to FERB's question in one task tree, keyed by what it covers; it carries
across that tree's retries (rule 9). A used row (an escape run, a payment that ran or is running) covers nothing more.
Rule 14: what is stored goes through the redactor first. A destination or key the redactor changes (it holds a secret)
is never known and never covered: FERB asks again rather than let two secrets share one stored "[redacted]"."""

from typing import TYPE_CHECKING, Any, Callable

from ferb.core.interfaces import Clock, IdSource
from ferb.policy import provenance

if TYPE_CHECKING:
    from ferb.store.db import Database

KINDS = ("email", "phone", "host", "name")
HOW = ("named", "answered", "confirmed")
Redact = Callable[[Any], Any]


def _redacted(redact: Redact, text: str) -> str:
    return str(redact({"value": text})["value"])


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

    def known(self, kind: str, value: str) -> bool:
        norm = self.normalize(kind, value)
        if kind not in KINDS or not norm or _redacted(self.redact, norm) != norm:
            return False
        return self.db.conn.execute("SELECT 1 FROM known_destinations WHERE kind = ? AND value = ?",
                                    (kind, norm)).fetchone() is not None

    def record(self, kind: str, value: str, how: str, request_id: str | None = None,
               span_id: str | None = None) -> None:
        """Keep a destination; one already known keeps how it first became known."""
        norm = self.normalize(kind, value)
        if kind not in KINDS or how not in HOW or not norm or _redacted(self.redact, norm) != norm:
            return
        self.db.conn.execute(
            "INSERT OR IGNORE INTO known_destinations (id, kind, value, how, request_id, span_id, at) "
            "VALUES (?, ?, ?, ?, ?, ?, ?)",
            (self.ids.new_id(), kind, norm, how, request_id, span_id, self.clock.now().isoformat()))

    @staticmethod
    def from_words(words: str) -> list[tuple[str, str]]:
        """The destinations in a text by shape, in order: whole email tokens, the hosts of URL tokens, and phone-like
        runs of at least provenance.MIN_PHONE_DIGITS digits. Names are never taken from text."""
        found: list[tuple[str, str]] = []
        for tok in provenance.tokens(words):
            if provenance.destination_shape(tok) == "email":
                found.append(("email", tok))
            elif host := provenance.host_of(tok):
                found.append(("host", host))
        found += [("phone", digits) for digits in provenance.phone_runs(words)]
        return list(dict.fromkeys(found))


class Confirmations:
    def __init__(self, db: "Database", clock: Clock, ids: IdSource, redact: Redact) -> None:
        self.db, self.clock, self.ids, self.redact = db, clock, ids, redact

    def find(self, tree_id: str, key: str) -> dict[str, Any] | None:
        """The owner's latest answer in this task tree to the question `key` names, or None (always None for a key
        holding a secret: it is stored redacted, so it could match another one)."""
        if _redacted(self.redact, key) != key:
            return None
        row = self.db.conn.execute("SELECT * FROM confirmations WHERE tree_id = ? AND key = ? "
                                   "ORDER BY rowid DESC LIMIT 1", (tree_id, key)).fetchone()
        return dict(row) if row is not None else None

    def covers(self, cid: str, key: str) -> bool:
        """Whether row `cid` is a Yes to the question `key` names (never for a key holding a secret)."""
        if _redacted(self.redact, key) != key:
            return False
        row = self.db.conn.execute("SELECT 1 FROM confirmations WHERE id = ? AND key = ? AND granted = 1",
                                   (cid, key)).fetchone()
        return row is not None

    def record(self, tree_id: str, rule: str, key: str, granted: bool, ask_span: str | None) -> str:
        cid = str(self.ids.new_id())
        self.db.conn.execute("INSERT INTO confirmations (id, tree_id, rule, key, granted, ask_span, used, at) "
                             "VALUES (?, ?, ?, ?, ?, ?, 0, ?)",
                             (cid, tree_id, rule, _redacted(self.redact, key), int(granted), ask_span,
                              self.clock.now().isoformat()))
        return cid

    def use(self, cid: str) -> None:
        """A Yes that covers one run only (an escape, a payment, an account-security change) is claimed by the call
        about to run; nothing else is covered by it."""
        self.db.conn.execute("UPDATE confirmations SET used = 1 WHERE id = ?", (cid,))

    def release(self, cid: str) -> None:
        """The claiming call never made its attempt (refused, or cancelled before it ran) or the tool reported it
        failed: the Yes covers its retry again. After an unknown outcome the Yes stays spent; only the identical retry
        the owner approves under P19 may use it once more (the gateway checks the ledger)."""
        self.db.conn.execute("UPDATE confirmations SET used = 0 WHERE id = ?", (cid,))
