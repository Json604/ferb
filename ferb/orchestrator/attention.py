"""Owner notices (L9): FERB messages in the owner's chat, made from observations. A notice is persisted, then posted to
chat (journaled `chat.notice`, published `attention.notice`); from then on it is told. `delivered_at` only records that
a client showed it, which takes it out of `chat.status`. Its text is model-written from outside observations, so it
keeps that origin, and outside text it repeats stays apart in `quotes` with its own origin (MP2)."""
import json
from functools import partial
from typing import Any

from ferb.core import origin
from ferb.memory.lineage import ENTRY
from ferb.sense.batches import app_of
from ferb.store.db import swap
from ferb.store.spans import rewrite_recorded

ORIGIN = origin.join({origin.external("observation"), origin.INFERRED})


def _quotes(value: Any, source: str) -> list[dict[str, str]]:
    """Quotes as shown: their non-empty text, labelled by code with where the considered rows came from (MP2, rule 5).
    An origin the model supplies is ignored; a quote without text is dropped."""
    if not isinstance(value, list):
        return []
    return [{"text": q["text"], "origin": source} for q in value
            if isinstance(q, dict) and isinstance(q.get("text"), str) and q["text"].strip()]


class AttentionNotices:
    def __init__(self, db: Any, clock: Any, redact: Any) -> None:
        self.db, self.clock, self.redact = db, clock, redact

    def add(self, ident: str, refs: list[str], text: str, quotes: Any = None,
            considered: list[str] | None = None, quote_origin: str = origin.external("observation"),
            notice_origin: str = ORIGIN, cards: list[dict[str, Any]] | None = None) -> dict[str, Any] | None:
        """The new notice, or None when that id was already made (posted once: a replayed batch never posts again).
        `refs` are the rows it cites; `considered` every row the consideration saw (the D14 purge reaches both);
        `quote_origin` the least trusted origin of those rows, which every quote carries."""
        clean = self.redact({"text": text, "quotes": _quotes(quotes, quote_origin)})
        made = self.db.conn.execute(
            "INSERT OR IGNORE INTO attention_notices (id, source_refs_json, text, origin, created_at, delivered_at, "
            "quotes_json, considered_refs_json, cards_json) VALUES (?,?,?,?,?,NULL,?,?,?)",
            (ident, json.dumps(refs), clean["text"], notice_origin, self.clock.now().isoformat(), json.dumps(clean["quotes"]),
             json.dumps(sorted(set(considered or []) | set(refs))),
             json.dumps(self.redact({"cards": cards or []})["cards"]))).rowcount
        return dict(self.db.conn.execute("SELECT * FROM attention_notices WHERE id=?", (ident,)).fetchone()) if made else None

    @staticmethod
    def view(row: dict[str, Any]) -> dict[str, Any]:
        """What clients render (`chat.status.notices`, `attention.notice`): {id, text, quotes, at}, and `cards` when it
        has any (step 12c)."""
        cards = json.loads(row.get("cards_json") or "[]")
        return {"id": row["id"], "text": row["text"], "quotes": json.loads(row["quotes_json"]), "at": row["created_at"],
                **({"cards": cards} if cards else {})}

    def unseen(self) -> list[dict[str, Any]]:
        return [self.view(r) for r in self.list()]

    def told(self, apps: set[str] | None = None) -> list[dict[str, Any]]:
        """The notices FERB has told the owner (posting to chat is telling, shown or not): every one, or with `apps`
        those about any of those apps, by the rows each notice cited or considered (L10: a scope, not a number)."""
        rows = [dict(r) for r in self.db.conn.execute("SELECT * FROM attention_notices ORDER BY created_at")]
        if apps is not None:
            about: dict[str, set[str]] = {}
            rows_of = ("SELECT n.id, t.data_json FROM attention_notices n, json_each({}) ref "
                       "JOIN trail t ON ref.value LIKE 'trail:%' AND t.id = substr(ref.value, 7)")
            for r in self.db.conn.execute(rows_of.format("n.source_refs_json") + " UNION " +
                                          rows_of.format("COALESCE(n.considered_refs_json, '[]')")):
                about.setdefault(r[0], set()).add(app_of(json.loads(r[1])))
            rows = [r for r in rows if about.get(r["id"], set()) & apps]
        return [self.view(r) for r in rows]

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


CONTEXT_REMOVED = "Removed: this recorded context held text that has expired or that the owner forgot."


def expire_derived(db: Any, refs: set[str], replacement: str, now: str) -> int:
    """Clear the text of every notice derived from these source rows (cited or considered), wherever it was copied:
    the notice (taken out of `chat.status`), its `chat.notice` journal entry, the told receipts carrying its text, the
    `attention.consider` spans, and any other span that recorded it from the chat history (L10), including the plain
    text a planner's conversation exposure keeps as its payload (L28). A
    consideration that saw one of the rows loses its recorded model context and its answer's text; any other model
    context that was told the notice (already_told, the chat history) loses that notice's text. Audit rows stay.
    Idempotent: a notice already carrying `replacement` is left as it is. Used by the D14 seven-day purge (I3) and by
    an explicit forget. It runs on the event loop's own connection, where no span can finish while it runs, so it takes
    one pass; each notice and span is written only while it still holds what was read (`swap`, `rewrite_recorded`),
    since a forget's scrub in its worker thread may rewrite the same rows (fix wave 6a). Returns how many notices it
    cleared."""
    cleared, said = 0, []
    entries: dict[str, str] = {}                  # the notices' chat entries (lineage handle) -> their plain text
    for row in db.conn.execute("SELECT * FROM attention_notices").fetchall():
        cited = json.loads(row["source_refs_json"])
        if not refs.intersection(cited) and not refs.intersection(json.loads(row["considered_refs_json"])):
            continue
        replaced: list[tuple[Any, ...]] = []        # cleared only while it holds what was read: a forget's scrub may
        if swap(db, "attention_notices", "id", row["id"], ("text", "quotes_json", "delivered_at"),   # rewrite it
                partial(_cleared, replacement, now, replaced), (row["text"], row["quotes_json"], row["delivered_at"])
                ) is None:
            continue
        text, quotes = replaced[-1][0], json.loads(replaced[-1][1])
        said += [text, *(q["text"] for q in quotes)]
        for entry in db.conn.execute("SELECT id,data_json FROM journal WHERE kind='chat.notice' AND "
                                     "json_extract(data_json,'$.id')=?", (row["id"],)).fetchall():
            entries[ENTRY + entry["id"]] = notice_payload(json.loads(entry["data_json"]))
        db.conn.execute("DELETE FROM journal WHERE kind='chat.notice' AND json_extract(data_json,'$.id')=?", (row["id"],))
        for ref in cited:
            db.conn.execute("UPDATE notification_told SET text=? WHERE event_id=? AND text=?",
                            (replacement, ref.removeprefix("trail:"), text))
        cleared += 1
    # A told notice sits in a later consideration's context as a JSON "text" field (already_told, or a quote), and in
    # the chat history (L10) in the same form: in the planner's recorded context, in the history block a task keeps
    # as a `context.exposure` output, and in a `chat.history` or `task.evidence` read (written without ASCII escapes,
    # or with that context encoded once more as a JSON string).
    told = list({form: (form, new) for text in said if text for form, new in _forms(text, replacement)}.values())

    def considered(inp: Any, out: Any) -> tuple[Any, Any]:
        if refs.intersection(inp.get("source_refs") or []):
            return (inp | ({"context": CONTEXT_REMOVED} if "context" in inp else {}),
                    out | {"text": replacement, "quotes": []} if isinstance(out, dict) and (
                        out.get("text") or out.get("quotes")) else out)
        return {k: _without(v, told) for k, v in inp.items()}, out

    if told:
        scrub_spans(db, told)
    if entries:
        _scrub_payloads(db, entries, replacement)
    for span in db.conn.execute("SELECT id,input_json,output_json FROM spans WHERE kind='attention.consider'").fetchall():
        rewrite_recorded(db, span["id"], considered, held=(span["input_json"], span["output_json"]))
    return cleared


def _cleared(replacement: str, now: str, replaced: list[tuple[Any, ...]], held: tuple[Any, ...]) -> tuple[Any, ...]:
    """A notice's text and quotes cleared (`held`: text, quotes, delivered_at); what it replaced is kept in `replaced`,
    so the copies cleared next are those of the text it really held. One already cleared is left as it is."""
    if held[0] == replacement:
        return held
    replaced.append(held)
    return replacement, "[]", held[2] or now


def _forms(text: str, replacement: str) -> list[tuple[str, str]]:
    """A told string as a JSON "text" field, with its replacement, in each way FERB's records write it."""
    out = []
    for ascii_only in (True, False):
        field = ('"text": ' + json.dumps(text, ensure_ascii=ascii_only),
                 '"text": ' + json.dumps(replacement, ensure_ascii=ascii_only))
        out += [field, (json.dumps(field[0], ensure_ascii=ascii_only)[1:-1],
                        json.dumps(field[1], ensure_ascii=ascii_only)[1:-1])]
    return out


def notice_payload(data: dict[str, Any]) -> str:
    """A notice entry's own text (its `chat.notice` data): its text and the texts it quotes, one per line, with no
    wrapper, label or id. The planner's history block carries it as the S4 payload (L28); the purge removes exactly
    this text from the exposures that showed the entry."""
    quotes = [str(q.get("text", "")) for q in data.get("quotes", []) if isinstance(q, dict)]
    return "\n".join([str(data.get("text", "")), *quotes])


def notice_sources(db: Any, notice_id: str) -> list[str]:
    """The rows a notice cited or considered: its lineage, carried by whatever is derived from it (forget reaches it)."""
    row = db.conn.execute("SELECT source_refs_json, considered_refs_json FROM attention_notices WHERE id=?",
                          (notice_id,)).fetchone()
    return sorted(set(json.loads(row[0])) | set(json.loads(row[1] or "[]"))) if row is not None else []


def scrub_spans(db: Any, told: list[tuple[str, str]], *, every_kind: bool = False) -> None:
    """Every span but the considerations (handled by the caller, unless `every_kind`: a memory forget, I2) whose
    recorded input or output holds one of the told strings loses it. One scan, matched in SQL against a temporary table
    of the strings, so any number of them fits."""
    db.conn.execute("CREATE TEMP TABLE IF NOT EXISTS expiring_text (held TEXT PRIMARY KEY)")
    try:
        # as it sits inside the stored JSON: as written since step 12a (UTF-8), and with \\uXXXX escapes before
        db.conn.executemany("INSERT OR IGNORE INTO expiring_text VALUES (?)",
                            [(form,) for old, _ in told
                             for form in {json.dumps(old)[1:-1], json.dumps(old, ensure_ascii=False)[1:-1]}])
        rows = db.conn.execute(
            "SELECT id,input_json,output_json FROM spans s WHERE (? OR kind!='attention.consider') AND EXISTS (SELECT 1 "
            "FROM expiring_text e WHERE instr(s.input_json, e.held) > 0 OR instr(COALESCE(s.output_json, ''), e.held) > 0)",
            (int(every_kind),)).fetchall()
    finally:
        db.conn.execute("DELETE FROM expiring_text")
    for span in rows:                                 # each written only while it holds what was read
        rewrite_recorded(db, span["id"], lambda inp, out: (_without(inp, told), _without(out, told)),
                         held=(span["input_json"], span["output_json"]))


def _scrub_payloads(db: Any, entries: dict[str, str], replacement: str) -> None:
    """A planner turn's conversation exposure keeps the shown entries' own texts as its `payload` (L28), a notice's as
    plain text (notice_payload) that no told JSON form matches. Every exposure whose source refs name one of these
    notice entries loses exactly that text; an exposure that never showed the notice is not touched."""
    db.conn.execute("CREATE TEMP TABLE IF NOT EXISTS expiring_entries (ref TEXT PRIMARY KEY)")
    try:
        db.conn.executemany("INSERT OR IGNORE INTO expiring_entries VALUES (?)", [(ref,) for ref in entries])
        rows = db.conn.execute(
            "SELECT DISTINCT s.id, s.input_json, s.output_json FROM spans s, "
            "json_each(s.input_json, '$.source_refs') r JOIN expiring_entries e ON r.value = e.ref "
            "WHERE s.kind='context.exposure' AND s.output_json IS NOT NULL"
        ).fetchall()
    finally:
        db.conn.execute("DELETE FROM expiring_entries")

    def cleared(inp: Any, out: Any) -> tuple[Any, Any]:
        payload = out.get("payload") if isinstance(out, dict) else None
        if not isinstance(payload, str):
            return inp, out
        shown = set(inp.get("source_refs") or [])
        new = payload
        for ref, text in entries.items():
            if ref in shown and text:
                new = new.replace(text, replacement)
        return inp, (out | {"payload": new} if new != payload else out)
    for span in rows:                                 # each written only while it holds what was read
        rewrite_recorded(db, span["id"], cleared, held=(span["input_json"], span["output_json"]))


def _without(value: Any, told: list[tuple[str, str]]) -> Any:
    """`value` with every told string replaced, in every string it holds."""
    if isinstance(value, dict):
        return {k: _without(v, told) for k, v in value.items()}
    if isinstance(value, list):
        return [_without(v, told) for v in value]
    if not isinstance(value, str):
        return value
    for old, new in told:
        value = value.replace(old, new)
    return value
