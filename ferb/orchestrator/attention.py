"""Owner notices (L9): FERB messages in the owner's chat, made from observations. A notice is persisted, then posted to
chat (journaled `chat.notice`, published `attention.notice`); from then on it is told. `delivered_at` only records that
a client showed it, which takes it out of `chat.status`. Its text is model-written from outside observations, so it
keeps that origin, and outside text it repeats stays apart in `quotes` with its own origin (MP2)."""
import json
from typing import Any

from ferb.core import origin

ORIGIN = origin.join({origin.external("observation"), origin.INFERRED})


def _quotes(value: Any) -> list[dict[str, str]]:
    """The shape a quote must have to be shown: {text, origin}, both non-empty strings; anything else is dropped."""
    if not isinstance(value, list):
        return []
    return [{"text": q["text"], "origin": q["origin"]} for q in value
            if isinstance(q, dict) and isinstance(q.get("text"), str) and q["text"].strip()
            and isinstance(q.get("origin"), str) and q["origin"].strip()]


class AttentionNotices:
    def __init__(self, db: Any, clock: Any, redact: Any) -> None:
        self.db, self.clock, self.redact = db, clock, redact

    def add(self, ident: str, refs: list[str], text: str, quotes: Any = None,
            considered: list[str] | None = None) -> dict[str, Any] | None:
        """The new notice, or None when that id was already made (posted once: a replayed batch never posts again).
        `refs` are the rows it cites; `considered` every row the consideration saw (the D14 purge reaches both)."""
        clean = self.redact({"text": text, "quotes": _quotes(quotes)})
        made = self.db.conn.execute(
            "INSERT OR IGNORE INTO attention_notices (id, source_refs_json, text, origin, created_at, delivered_at, "
            "quotes_json, considered_refs_json) VALUES (?,?,?,?,?,NULL,?,?)",
            (ident, json.dumps(refs), clean["text"], ORIGIN, self.clock.now().isoformat(), json.dumps(clean["quotes"]),
             json.dumps(sorted(set(considered or []) | set(refs))))).rowcount
        return dict(self.db.conn.execute("SELECT * FROM attention_notices WHERE id=?", (ident,)).fetchone()) if made else None

    @staticmethod
    def view(row: dict[str, Any]) -> dict[str, Any]:
        """What clients render (`chat.status.notices`, `attention.notice`): {id, text, quotes, at}."""
        return {"id": row["id"], "text": row["text"], "quotes": json.loads(row["quotes_json"]), "at": row["created_at"]}

    def unseen(self) -> list[dict[str, Any]]:
        return [self.view(r) for r in self.list()]

    def told(self) -> list[dict[str, Any]]:
        """Every notice FERB has told the owner: posting to chat is telling, shown or not."""
        return [self.view(dict(r)) for r in self.db.conn.execute("SELECT * FROM attention_notices ORDER BY created_at")]

    def acknowledge(self, idents: list[str]) -> list[dict[str, Any]]:
        """A client showed these notices. Unknown ids refuse the whole acknowledgement."""
        rows = [self.db.conn.execute('SELECT * FROM attention_notices WHERE id=?', (ident,)).fetchone() for ident in idents]
        if any(row is None for row in rows):
            raise ValueError('No such owner notice')
        for ident in idents:
            self.db.conn.execute('UPDATE attention_notices SET delivered_at=COALESCE(delivered_at,?) WHERE id=?',
                                 (self.clock.now().isoformat(), ident))
        return [dict(row) for row in rows]

    def list(self, *, pending: bool = True) -> list[dict[str, Any]]:
        """Stored rows: not yet shown by a client (pending), or shown."""
        return [dict(r) for r in self.db.conn.execute('SELECT * FROM attention_notices' +
            (' WHERE delivered_at IS NULL' if pending else ' WHERE delivered_at IS NOT NULL') + ' ORDER BY created_at')]


def expire_derived(db: Any, refs: set[str], replacement: str, now: str) -> int:
    """Clear the text of every notice derived from these source rows (cited or considered), wherever it was copied:
    the notice (taken out of `chat.status`), its `chat.notice` journal entry, the told receipts carrying its text and
    the consideration's span output. Used by the D14 seven-day purge (I3) and by an explicit forget."""
    cleared = 0
    for row in db.conn.execute("SELECT * FROM attention_notices").fetchall():
        cited = json.loads(row["source_refs_json"])
        if not refs.intersection(cited) and not refs.intersection(json.loads(row["considered_refs_json"])):
            continue
        db.conn.execute("UPDATE attention_notices SET text=?,quotes_json='[]',delivered_at=COALESCE(delivered_at,?) "
                        "WHERE id=?", (replacement, now, row["id"]))
        db.conn.execute("DELETE FROM journal WHERE kind='chat.notice' AND json_extract(data_json,'$.id')=?", (row["id"],))
        for ref in cited:
            db.conn.execute("UPDATE notification_told SET text=? WHERE event_id=? AND text=?",
                            (replacement, ref.removeprefix("trail:"), row["text"]))
        cleared += 1
    for span in db.conn.execute("SELECT id,input_json,output_json FROM spans WHERE kind='attention.consider' "
                                "AND output_json IS NOT NULL").fetchall():
        if refs.intersection(json.loads(span["input_json"]).get("source_refs") or []):
            out = json.loads(span["output_json"])
            if isinstance(out, dict) and (out.get("text") or out.get("quotes")):
                db.conn.execute("UPDATE spans SET output_json=? WHERE id=?",
                                (json.dumps(out | {"text": replacement, "quotes": []}), span["id"]))
    return cleared
