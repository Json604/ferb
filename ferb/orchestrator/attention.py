"""Owner notices (L9): FERB messages in the owner's chat, made from observations. A notice is persisted, then posted to
chat (journaled `chat.notice`, published `attention.notice`); from then on it is told. `delivered_at` only records that
a client showed it, which takes it out of `chat.status`. Its text is model-written from outside observations, so it
keeps that origin, and outside text it repeats stays apart in `quotes` with its own origin (MP2)."""
import json
from typing import Any

from ferb.act.browser.profiles import BROWSERS
from ferb.core import origin

ORIGIN = origin.join({origin.external("observation"), origin.INFERRED})
# A tab row names its browser ("brave"), an Accessibility row its bundle: both are the same app (L10).
BROWSER_BUNDLES = {name: browser.bundle_id for name, browser in BROWSERS.items()} | {"safari": "com.apple.Safari"}


def app_of(data: dict[str, Any]) -> str:
    """The app an observation row is about: its bundle id, or the bundle of the browser a tab row names; "" when the
    row names no app (files, idle). A fact recorded on the row, never inferred from its content."""
    if data.get("bundle_id"):
        return str(data["bundle_id"])
    browser = str(data.get("browser") or "")
    return BROWSER_BUNDLES.get(browser, browser)


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
            considered: list[str] | None = None, quote_origin: str = origin.external("observation")) -> dict[str, Any] | None:
        """The new notice, or None when that id was already made (posted once: a replayed batch never posts again).
        `refs` are the rows it cites; `considered` every row the consideration saw (the D14 purge reaches both);
        `quote_origin` the least trusted origin of those rows, which every quote carries."""
        clean = self.redact({"text": text, "quotes": _quotes(quotes, quote_origin)})
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
    `attention.consider` spans, and any other recorded model context that was shown it in the chat history (L10). A
    consideration that saw one of the rows loses its recorded model context and its answer's text; any other model
    context that was told the notice (already_told, the chat history) loses that notice's text. Audit rows stay.
    Idempotent: a notice already carrying `replacement` is left as it is. Used by the D14 seven-day purge (I3) and by
    an explicit forget. Returns how many notices it cleared."""
    cleared, said = 0, []
    for row in db.conn.execute("SELECT * FROM attention_notices").fetchall():
        cited = json.loads(row["source_refs_json"])
        if row["text"] == replacement or (not refs.intersection(cited)
                                          and not refs.intersection(json.loads(row["considered_refs_json"]))):
            continue
        said += [row["text"], *(q["text"] for q in json.loads(row["quotes_json"]))]
        db.conn.execute("UPDATE attention_notices SET text=?,quotes_json='[]',delivered_at=COALESCE(delivered_at,?) "
                        "WHERE id=?", (replacement, now, row["id"]))
        db.conn.execute("DELETE FROM journal WHERE kind='chat.notice' AND json_extract(data_json,'$.id')=?", (row["id"],))
        for ref in cited:
            db.conn.execute("UPDATE notification_told SET text=? WHERE event_id=? AND text=?",
                            (replacement, ref.removeprefix("trail:"), row["text"]))
        cleared += 1
    # A told notice sits in a later consideration's context as a JSON "text" field (already_told, or a quote), and in
    # the chat history the planner and later considerations were given (L10) in the same form.
    told = [('"text": ' + json.dumps(text), '"text": ' + json.dumps(replacement)) for text in said if text]
    if told:                                              # one pass over the other spans, only when needed
        holds = " OR ".join(["instr(input_json, ?) > 0"] * len(told))
        for span in db.conn.execute(f"SELECT id,input_json FROM spans WHERE kind!='attention.consider' AND ({holds})",
                                    [json.dumps(old)[1:-1] for old, _ in told]).fetchall():
            inp = json.loads(span["input_json"])
            new_inp = {k: _without(v, told) for k, v in inp.items()}
            if new_inp != inp:
                db.conn.execute("UPDATE spans SET input_json=? WHERE id=?", (json.dumps(new_inp), span["id"]))
    for span in db.conn.execute("SELECT id,input_json,output_json FROM spans WHERE kind='attention.consider'").fetchall():
        inp = json.loads(span["input_json"])
        out = json.loads(span["output_json"]) if span["output_json"] else None
        if refs.intersection(inp.get("source_refs") or []):
            new_inp = inp | ({"context": CONTEXT_REMOVED} if "context" in inp else {})
            new_out = out | {"text": replacement, "quotes": []} if isinstance(out, dict) and (
                out.get("text") or out.get("quotes")) else out
        else:
            new_inp, new_out = {k: _without(v, told) for k, v in inp.items()}, out
        if new_inp != inp or new_out != out:
            db.conn.execute("UPDATE spans SET input_json=?,output_json=? WHERE id=?",
                            (json.dumps(new_inp), None if new_out is None else json.dumps(new_out), span["id"]))
    return cleared


def _without(value: Any, told: list[tuple[str, str]]) -> Any:
    if not isinstance(value, str):
        return value
    for old, new in told:
        value = value.replace(old, new)
    return value
