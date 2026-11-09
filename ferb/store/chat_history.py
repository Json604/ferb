"""Own the chat as FERB.app shows it (`chat_history`, APP-805): each entry (a turn, a late reply, a notice) in the app's
own shape, written by the app as it changes, read back a page at a time. Must not interpret an entry: its body is the
app's, kept as it came (redacted); only the columns the pages need are read."""

import json
import re
import uuid
from datetime import datetime, timezone
from typing import Any, Callable

from ferb.core.errors import Refused
from ferb.store.db import Database

KINDS = frozenset({"turn", "reply", "notice"})
# Ruling (DMN-555): a day's first page and each earlier one, as Signal Desktop loads a conversation (owner 2026-11-09:
# "30, but on scroll up it should automatically load those")
PAGE = 30
PAGE_MAX = 200
# Ruling (DMN-555): entries one call may write; a call is also bounded by the socket's 1 MB message (server.MAX_MESSAGE)
WRITE_MAX = 500
REF_MAX = 200
_DAY = re.compile(r"\d{4}-\d{2}-\d{2}")


def _at(value: Any) -> str:
    """An entry's time as UTC ISO 8601 with microseconds, so text order is time order. Refused without an offset."""
    try:
        when = datetime.fromisoformat(str(value).replace("Z", "+00:00"))
    except ValueError as e:
        raise Refused("at must be an ISO 8601 time with its offset") from e
    if when.tzinfo is None:
        raise Refused("at must be an ISO 8601 time with its offset")
    return when.astimezone(timezone.utc).isoformat(timespec="microseconds")


def _checked(entry: Any) -> dict[str, Any]:
    """One entry as sent, checked: its id a UUID, a known kind, its time, its day, a body object."""
    if not isinstance(entry, dict):
        raise Refused("each entry must be an object")
    try:
        ident = str(uuid.UUID(str(entry.get("id"))))
    except ValueError as e:
        raise Refused("an entry's id must be a UUID") from e
    kind = entry.get("kind")
    if kind not in KINDS:
        raise Refused("an entry's kind must be turn, reply or notice")
    day = entry.get("day")
    if not isinstance(day, str) or not _DAY.fullmatch(day):
        raise Refused("an entry's day must be YYYY-MM-DD")
    body = entry.get("body")
    if not isinstance(body, dict):
        raise Refused("an entry's body must be an object")
    ref = entry.get("ref")
    if ref is not None and (not isinstance(ref, str) or len(ref) > REF_MAX):
        raise Refused("an entry's ref must be a short text")
    if not isinstance(entry.get("open", False), bool):
        raise Refused("an entry's open must be true or false")
    return {"id": ident.upper(), "kind": kind, "at": _at(entry.get("at")), "day": day,
            "open": int(entry.get("open", False)), "ref": ref or None, "body": body}


def cursor(row: Any) -> str:
    return f"{row['at']}|{row['id']}"


class ChatHistory:
    def __init__(self, db: Database, redact: Callable[[Any], Any], now: Callable[[], str]) -> None:
        self.db = db
        self.redact = redact
        self.now = now

    def _notice_text(self, ref: str | None, body: dict[str, Any]) -> dict[str, Any]:
        """A notice FERB recorded (attention_notices) says what that record says now: a forget or the typed-text purge
        that cleared it there clears it here too, whatever copy the app still holds (rule 7)."""
        if not ref:
            return body
        row = self.db.conn.execute("SELECT text, quotes_json FROM attention_notices WHERE id = ?", (ref,)).fetchone()
        if row is None:
            return body
        return body | {"text": row["text"], "quotes": json.loads(row["quotes_json"] or "[]")}

    def write(self, entries: Any, *, replace: bool) -> dict[str, int]:
        """Entries the app sent, redacted. `replace`: a changed entry (`chat.history.put`) replaces the one kept;
        otherwise (`chat.history.import`) one already kept stays as it is, so an import runs any number of times and
        never takes back a newer write. Returns how many rows changed and how many of the given ids are now kept."""
        if not isinstance(entries, list) or len(entries) > WRITE_MAX:
            raise Refused(f"entries must be a list of at most {WRITE_MAX}")
        checked = [_checked(e) for e in entries]
        verb = "INSERT OR REPLACE" if replace else "INSERT OR IGNORE"
        changed = 0
        now = self.now()
        with self.db.transaction():
            for e in checked:
                body = self.redact(e["body"])
                if e["kind"] == "notice":
                    body = self._notice_text(e["ref"], body)
                changed += self.db.conn.execute(
                    f"{verb} INTO chat_history (id, kind, at, day, open, ref, body_json, updated_at) "
                    "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    (e["id"], e["kind"], e["at"], e["day"], e["open"], e["ref"],
                     json.dumps(body, ensure_ascii=False), now)).rowcount
        ids = [e["id"] for e in checked]
        held = 0
        for i in range(0, len(ids), 500):
            chunk = ids[i:i + 500]
            held += int(self.db.conn.execute(
                f"SELECT COUNT(*) FROM chat_history WHERE id IN ({','.join('?' * len(chunk))})", chunk).fetchone()[0])
        return {"changed": changed, "held": held}

    @staticmethod
    def _out(row: Any) -> dict[str, Any]:
        return {"id": row["id"], "kind": row["kind"], "at": row["at"], "day": row["day"], "open": bool(row["open"]),
                **({"ref": row["ref"]} if row["ref"] else {}), "body": json.loads(row["body_json"])}

    def page(self, day: str, before: str | None = None, limit: int = PAGE) -> dict[str, Any]:
        """A day's newest `limit` entries before `before` (a page's `cursor`), oldest first; `more` when earlier ones
        exist; `context`: the turns a reply on the page answers that aren't on it (a reply is drawn with its
        question)."""
        if not _DAY.fullmatch(day):
            raise Refused("day must be YYYY-MM-DD")
        if isinstance(limit, bool) or not isinstance(limit, int) or not 1 <= limit <= PAGE_MAX:
            raise Refused(f"limit must be a whole number from 1 to {PAGE_MAX}")
        params: list[Any] = [day]
        where = "day = ?"
        if before:
            at, _, ident = str(before).partition("|")
            where += " AND (at < ? OR (at = ? AND id < ?))"
            params += [at, at, ident]
        rows = self.db.conn.execute(f"SELECT * FROM chat_history WHERE {where} ORDER BY at DESC, id DESC LIMIT ?",
                                    (*params, limit + 1)).fetchall()
        more = len(rows) > limit
        rows = list(reversed(rows[:limit]))
        shown = {r["id"] for r in rows}
        answered = {str(json.loads(r["body_json"]).get("turnID", "")).upper() for r in rows if r["kind"] == "reply"}
        wanted = sorted(answered - shown - {""})
        context = [self._out(r) for r in self.db.conn.execute(
            f"SELECT * FROM chat_history WHERE kind = 'turn' AND id IN ({','.join('?' * len(wanted))})", wanted)] \
            if wanted else []
        return {"entries": [self._out(r) for r in rows], "more": more,
                "cursor": cursor(rows[0]) if rows and more else None, "context": context}

    def open(self) -> list[dict[str, Any]]:
        """Every turn still waiting for FERB's final answer, any day: the app keeps them in hand at launch."""
        return [self._out(r) for r in self.db.conn.execute(
            "SELECT * FROM chat_history WHERE open = 1 ORDER BY at, id")]

    def days(self) -> list[str]:
        """Each day that has an entry, newest first."""
        return [r[0] for r in self.db.conn.execute("SELECT DISTINCT day FROM chat_history ORDER BY day DESC")]



def clear_notice(db: Any, ref: str, text: str) -> int:
    """A notice whose text was cleared (attention.expire_derived: a forget, the typed-text purge) says `text` and quotes
    nothing in the chat history too."""
    return int(db.conn.execute(
        "UPDATE chat_history SET body_json = json_set(body_json, '$.text', ?, '$.quotes', json('[]')) "
        "WHERE kind = 'notice' AND ref = ?", (text, ref)).rowcount)
