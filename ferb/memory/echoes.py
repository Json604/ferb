"""Forget is complete (memory review I2): what FERB's own records say of a forgotten item goes with it.

An explicit forget deletes the item, its versions and its sources (items.py). FERB's own records can still state it:
the code line "Remembered: …", FERB's answer to the forgotten message, a reply written from the item, and the planner's
stored turns and exposures that copied those. Here, after the deletion:
- FERB's whole answer to a forgotten chat message is removed (it answered words that are gone);
- in FERB's other conversation rows (replies, questions, task answers and outcomes), a line is removed when it holds an
  item's text or a forgotten message's text word for word, or, in a row whose turn was given the item (its source refs
  cite the item, its derived nodes or the forgotten entries), a word of the item that nothing left holds: no remaining
  owner message, memory item or FERB row not given the item (data-driven, no word list);
- every span (the planner's turns, exposures, task evidence) loses those lines and those texts.
Journal rows are append-only (they are only ever deleted, for a forget): a changed row is deleted and written again
with the same sequence number, id, time, kind and origin. The owner's own messages are never rewritten."""
import json
from dataclasses import dataclass, field
from typing import Any

from ferb.memory.items import contains_words, query_terms

REMOVED = "[removed: it held something the owner asked FERB to forget]"
FERB_KINDS = ("chat.reply", "chat.clarify", "task.answer", "task.outcome")
OWNER_KINDS = ("chat.message", "memory.added", "memory.edited")
FIELDS = ("text", "display_text", "summary", "answer")


@dataclass
class Forgotten:
    """What a forget deletes, read before it does: the items' texts, the forgotten entries' texts, the requests of the
    forgotten chat messages, and every ref that names something forgotten."""
    contents: list[str] = field(default_factory=list)
    said: list[str] = field(default_factory=list)
    requests: set[str] = field(default_factory=set)
    refs: set[str] = field(default_factory=set)

    @property
    def phrases(self) -> list[str]:
        return [t for t in [*self.contents, *self.said] if t.strip()]


def capture(db: Any, items: list[str], journal: list[str], trail: list[str]) -> Forgotten:
    """Read before the deletion: the items' contents (every version), the forgotten entries' texts and requests, and
    the refs naming them, with the learning nodes built on them (a planner turn cites a fact by its node)."""
    out = Forgotten()
    for ident in items:
        row = db.conn.execute("SELECT content FROM memory_items WHERE id=?", (ident,)).fetchone()
        if row is not None:
            out.contents.append(str(row[0]))
    for ident in journal:
        row = db.conn.execute("SELECT kind, data_json FROM journal WHERE id=?", (ident,)).fetchone()
        if row is None:
            continue
        data = json.loads(row["data_json"])
        out.said.append(str(data.get("text") or data.get("content") or ""))
        if row["kind"] == "chat.message" and data.get("request_id"):
            out.requests.add(str(data["request_id"]))
    out.refs = ({"memory:" + i for i in items} | {"journal:" + j for j in journal} | {"entry:" + j for j in journal}
                | {"trail:" + t for t in trail})
    frontier = set(out.refs)
    while frontier:                                   # the nodes built on them, at any depth
        found = {r[0] for ref in frontier for r in db.conn.execute(
            "SELECT node_id FROM learning_sources WHERE source_ref=?", (ref,))} - out.refs
        out.refs |= found
        frontier = found
    return out


def given(db: Any, refs: set[str]) -> set[str]:
    """FERB's conversation rows whose turn was given one of these refs (their recorded source refs cite it)."""
    if not refs:
        return set()
    db.conn.execute("CREATE TEMP TABLE IF NOT EXISTS forgetting_refs (ref TEXT PRIMARY KEY)")
    try:
        db.conn.executemany("INSERT OR IGNORE INTO forgetting_refs VALUES (?)", [(r,) for r in refs])
        rows = db.conn.execute(
            f"SELECT DISTINCT j.id FROM journal j, json_each(j.data_json, '$.source_refs') r JOIN forgetting_refs f "
            f"ON r.value = f.ref WHERE j.kind IN ({','.join('?' * len(FERB_KINDS))}) "
            "AND json_type(j.data_json, '$.source_refs') = 'array'", FERB_KINDS).fetchall()
    finally:
        db.conn.execute("DELETE FROM forgetting_refs")
    return {r[0] for r in rows}


def stating(db: Any, forgotten: Forgotten) -> set[str]:
    """FERB's conversation rows that hold an item's or a forgotten message's text word for word."""
    phrases = forgotten.phrases
    if not phrases:
        return set()
    marks = ','.join('?' * len(FERB_KINDS))
    return {row[0] for row in db.conn.execute(f"SELECT id, data_json FROM journal WHERE kind IN ({marks})", FERB_KINDS)
            if any(contains_words(text, p) for text in _texts(json.loads(row[1])) for p in phrases)}


def _texts(data: dict[str, Any]) -> list[str]:
    return [v for k, v in data.items() if k in FIELDS and isinstance(v, str)]


def scrub(db: Any, forgotten: Forgotten) -> list[str]:
    """Remove what FERB's own records say of the forgotten item (module docstring), after its deletion. Returns the ids
    of the journal rows rewritten."""
    phrases = forgotten.phrases
    if not phrases and not forgotten.requests:
        return []
    cited = given(db, forgotten.refs)
    marks = ','.join('?' * len(FERB_KINDS))
    ferb = db.conn.execute(f"SELECT seq, id, at, kind, data_json, origin FROM journal WHERE kind IN ({marks}) "
                           "ORDER BY seq", FERB_KINDS).fetchall()
    # the item's words nothing left holds: no remaining owner text or memory item, no FERB row not given the item
    words = {w for c in forgotten.contents for w in query_terms(c)}
    others = [str(r[0]) for r in db.conn.execute("SELECT content FROM memory_items")]
    for row in db.conn.execute(f"SELECT data_json FROM journal WHERE kind IN ({','.join('?' * len(OWNER_KINDS))})",
                               OWNER_KINDS):
        if not words:
            break
        data = json.loads(row[0])
        others.append(str(data.get("text") or data.get("content") or ""))
    others += [t for r in ferb if r["id"] not in cited for t in _texts(json.loads(r["data_json"]))]
    for text in others:
        if not words:
            break
        words -= set(query_terms(text))

    def states(line: str, row_given: bool) -> bool:
        return any(contains_words(line, p) for p in phrases) or (row_given and bool(words & set(query_terms(line))))

    removed: list[str] = []
    rewritten: list[str] = []
    for row in ferb:
        data = json.loads(row["data_json"])
        answer = row["kind"] in ("chat.reply", "chat.clarify") and data.get("request_id") in forgotten.requests
        new = dict(data)
        for key in FIELDS:
            value = data.get(key)
            if not isinstance(value, str) or not value:
                continue
            if answer:
                new[key] = REMOVED
                removed += [line for line in value.split("\n") if states(line, True)]
                continue
            lines = value.split("\n")
            kept = [REMOVED if line.strip() and states(line, row["id"] in cited) else line for line in lines]
            removed += [line for line, now in zip(lines, kept) if now is REMOVED]
            new[key] = "\n".join(kept)
        if new != data:
            db.conn.execute("DELETE FROM journal WHERE seq=?", (row["seq"],))
            db.conn.execute("INSERT INTO journal (seq, id, at, kind, data_json, origin) VALUES (?,?,?,?,?,?)",
                            (row["seq"], row["id"], row["at"], row["kind"], json.dumps(new), row["origin"]))
            rewritten.append(row["id"])
    told = {s.strip() for s in [*removed, *phrases] if s.strip()}
    if told:
        from ferb.orchestrator.attention import scrub_spans
        pairs = {(s, REMOVED) for s in told}
        for ascii_only in (True, False):                  # as a JSON document inside a recorded string holds it
            pairs |= {(json.dumps(s, ensure_ascii=ascii_only)[1:-1], json.dumps(REMOVED, ensure_ascii=ascii_only)[1:-1])
                      for s in told}
        scrub_spans(db, sorted(pairs, key=lambda p: -len(p[0])), every_kind=True)
    return rewritten
