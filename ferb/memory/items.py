"""Owns memory storage, provenance and versions. Must not treat retrieved observations as owner authorization or silently cut stored content.

Memory items: SQLite is the source of truth; every rule about where an item may come from is here.

Origins, enforced in code:
- `owner_stated`: the owner's own words (`ferb memory add`, `edit`, or chat capture whose saved text appears word for
  word in what the owner typed, `contains_words`) -> active on commit; it supersedes any earlier version of the same
  fact.
- `observed`: from the owner's own activity. Its sources must all be trail rows with actor "owner" (FERB's own
  activity never becomes the owner's habits); it activates once that evidence spans 3 separate days.
- `inferred`: stays proposed until the owner confirms it (FERB's own wording of what the owner said is inferred); until
  then it never reaches a prompt (`prompt_ready`).
- `third_party`: pages, emails, pasted text. A fact about the world only: never a preference, never about the owner.

Versions: kind + subject + key name one fact. A newer version closes the old one (`valid_to`) and links to it
(`supersedes`); nothing is overwritten. An observed / inferred / third-party item never supersedes something the owner
stated: both stay active and recall flags the conflict (the owner's wins). An item also ends at the end of a day the
owner's words give it (`expire_at`) or when their words say it no longer holds (`retire`); it stays as history, and
`items` and `prompt_ready` treat it as current until then (L36 §6).

`forget` purges: the item, all its versions, items built on the same sources, the sources themselves (journal
entries, trail rows) unless the owner keeps them, the search index, the WAL; days whose trail rows went are
re-summarized (or their summary deleted once the raw rows are gone), and a day's narrative written from a forgotten
chat message is deleted. What FERB's own records say of it goes too (`echoes`, I2). A one-way fingerprint stops
distillation from proposing it again. `drop` (the owner's Drop on one of FERB's guesses, L36) deletes that proposal
alone and keeps the same fingerprint."""

import hashlib
import json
import re
import unicodedata
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from typing import Any, Callable, Iterable, Literal

from ferb.core.errors import FerbError
from ferb.core.interfaces import Clock, IdSource
from ferb.core.types import MemoryItem
from ferb.store.db import Database

KINDS = ("fact", "preference", "person", "project", "env")
ORIGINS = ("owner_stated", "observed", "inferred", "third_party")
OBSERVED_DAYS = 3
Source = tuple[Literal["journal", "trail"], str]
# Stamps are compared as text in SQL; their UTC offsets can differ by 26 hours, so a bound there is this much wider
# and the exact comparison is made on datetimes (`current`)
SQL_SLACK = timedelta(days=2)


class MemoryRefused(FerbError):
    pass


def query_terms(text: str) -> list[str]:
    """Lowercased word runs by Unicode category (letters, numbers, marks, private use), matching the FTS tokenizer
    (migration 0014; it lowercases, it does not casefold: "straße" stays whole). Python's \\w drops combining marks and would split Devanagari words."""
    terms: list[str] = []
    current = ""
    for ch in text.lower():
        cat = unicodedata.category(ch)
        if cat[0] in "LNM" or cat == "Co":
            current += ch
        elif current:
            terms.append(current)
            current = ""
    if current:
        terms.append(current)
    return list(dict.fromkeys(terms))


def weighted(terms: list[str], total: int, matches: Callable[[str], int]) -> list[str]:
    """The query terms that carry weight, for `Memory.recall` and the one ranked search (L36 §3): a term found in more
    than half of the `total` documents says nothing about any one of them (data-driven, no word list). The floor of 2
    keeps a shared topic word useful when few documents exist: two memories about "editor" must both recall.
    `matches(term)`: how many documents hold it."""
    return [t for t in terms if total < 2 or matches(t) <= max(2, total / 2)]


def _spaced(text: str) -> str:
    return " ".join(text.casefold().split())


def _wordlike(ch: str) -> bool:
    return unicodedata.category(ch)[0] in "LNM" or unicodedata.category(ch) == "Co"


def contains_words(text: str, part: str) -> bool:
    """Whether `part` appears in `text` as whole words, word for word (case and spacing aside; the sentence punctuation
    around `part` doesn't count). "the" is in "summarize the email" but "Rao" is not in "Raoul" (I1, MP2)."""
    needle = _spaced(part).strip(" .,;:!?\"'“”‘’()[]")
    if not needle:
        return False
    hay = _spaced(text)
    start = hay.find(needle)
    while start != -1:
        end = start + len(needle)
        if (start == 0 or not _wordlike(hay[start - 1])) and (end == len(hay) or not _wordlike(hay[end])):
            return True
        start = hay.find(needle, start + 1)
    return False


_CLAUSE_MARKS = ",;:!?\n"     # where a clause ends; a period isn't one ("Dr. Rao"), so a clause is never cut short
_EDGE = " \t.\"'“”‘’()[]"


def _found(text: str, part: str) -> tuple[int, int] | None:
    """Where `part` sits in `text` as whole words (case and spacing aside), or None."""
    words = part.strip(_EDGE + ",;:!?").split()
    if not words:
        return None
    for m in re.finditer(r"\s+".join(re.escape(w) for w in words), text, re.IGNORECASE):
        before = m.start() == 0 or not _wordlike(text[m.start() - 1])
        if before and (m.end() == len(text) or not _wordlike(text[m.end()])):
            return m.start(), m.end()
    return None


def _clause_span(text: str, start: int, end: int) -> tuple[int, int]:
    left = max(text.rfind(c, 0, start) for c in _CLAUSE_MARKS) + 1
    rights = [i for i in (text.find(c, end) for c in _CLAUSE_MARKS) if i != -1]
    return left, min(rights) if rights else len(text)


# Memory capture's sentence rule (re-review R5, controller rulings: correctness over brevity, never drop a negation):
# nothing inside the owner's typed words ends a sentence but a line break. No ".", "!" or "?" splits ("my prof.
# Zorblat", "never! Share my links"), so the saved text may be longer than the part. The redactor lane has its own
# clause rule (ferb/sanitize/redact.py); the two helpers are to be unified at integration.
def _sentence_ends(text: str) -> list[int]:
    return [m.start() for m in re.finditer(r"\n", text)]


def owner_sentences(text: str, part: str) -> str | None:
    """The whole sentence(s) of the owner's `text` that hold `part` as whole words, in their own spelling: what is
    saved as their words, so a slice can never drop a negation or a condition, whatever its commas or marks ("Never,
    ever share my links publicly"; "never! Share my links"). Only a line break ends one (R5). None when `part` isn't in
    `text` (re-review F4)."""
    found = _found(text, part)
    if found is None:
        return None
    ends = _sentence_ends(text)
    left = max((e for e in ends if e < found[0]), default=-1) + 1
    right = min((e for e in ends if e >= found[1]), default=len(text))
    return text[left:right].strip(_EDGE + "!?") or None


def whole_clauses(text: str, part: str) -> bool:
    """Whether `part` is exactly one or more whole clauses of `text`: a meaningful span of the owner's words, never any
    word they happened to type ("the"). A chat forget's quote must be one."""
    found = _found(text, part)
    if found is None:
        return False
    left, right = _clause_span(text, *found)
    return not any(_wordlike(c) for c in text[left:found[0]] + text[found[1]:right])


@dataclass(frozen=True)
class Capture:
    """What chat capture saves from the owner's typed words (`capture`)."""
    origin: str          # "owner_stated", "inferred", or "" when nothing is saved
    content: str
    refused: str = ""    # why nothing is saved, in code's words (CAPTURE_TOLD has the owner's)


# Each refusal as the owner reads it in FERB's line (owner 2026-10-24: the reason, never "a problem on my side")
CAPTURE_TOLD = {"quote not in the owner's words": "the words I quoted from you aren't in what you typed",
                "the quote holds nothing of what would be saved": "the words I quoted from you don't say it"}


def capture(own_words: str, content: str, quote: str) -> Capture:
    """Chat capture's check (memory review I1, MP2), shared by the planner's remember entries and distillation's quotes
    (L36 §2). `own_words`: what the owner typed (pasted parts left out), with its line breaks. The quote must be whole
    words of it. Then the whole sentence(s) holding `content` are saved as the owner's own words (`owner_sentences`);
    when the content isn't there, `content` is FERB's wording (inferred), provided the quote shares a word with it."""
    content = content.strip()
    if not contains_words(own_words, quote):
        return Capture("", content, "quote not in the owner's words")
    clause = owner_sentences(own_words, content)
    if clause is None and not set(query_terms(quote)) & set(query_terms(content)):
        return Capture("", content, "the quote holds nothing of what would be saved")
    return Capture("owner_stated", clause) if clause else Capture("inferred", content)


def owner_span(text: str, part: str) -> str | None:
    """`part` as the owner spelled it in `text` (whole words, case and spacing aside), or None."""
    found = _found(text, part)
    return text[found[0]:found[1]] if found else None


def owner_span_at(text: str, part: str) -> tuple[int, int] | None:
    """Where `owner_span` finds `part` in `text`: (start, end), or None."""
    return _found(text, part)


def current(item: MemoryItem, now: datetime | None = None) -> bool:
    """Whether an item still holds: no end, or (with `now`) an end still ahead, as the owner's dated words give one
    (L36 §6). Without `now`, only an item without an end."""
    return item.valid_to is None or (now is not None and item.valid_to > now)


def prompt_ready(item: MemoryItem, now: datetime | None = None) -> bool:
    """Whether an item may shape FERB's prompts: a current active item (one with an end date, until then: L36 §6), or
    an observed pattern still gathering days (shown as an unconfirmed hint). An inferred item (model wording: a
    paraphrase of the owner, a distilled guess) waits for the owner's Keep (I1, MP2)."""
    return current(item, now) and (item.status == "active"
                                   or (item.status == "proposed" and item.origin == "observed"))


def fingerprint(kind: str, subject: str, key: str, content: str) -> str:
    norm = "|".join(" ".join(str(x).lower().split()) for x in (kind, subject, key, content))
    return hashlib.sha256(norm.encode()).hexdigest()


def slug(text: str, n: int | None = None) -> str:
    value = re.sub(r"[^\w]+", "_", text.casefold()).strip("_") or "note"
    if n is not None and len(value) > n:
        # Filesystem/display keys remain bounded without collapsing two long subjects.
        return value[:max(0, n - 17)] + "_" + hashlib.sha256(value.encode()).hexdigest()[:16]
    return value


@dataclass
class Recalled:
    item: MemoryItem
    conflict: bool = False                  # another active version of this fact disagrees


@dataclass
class ForgetPlan:
    items: list[str] = field(default_factory=list)            # item ids
    journal: list[str] = field(default_factory=list)          # journal entry ids
    trail: list[str] = field(default_factory=list)            # trail row ids
    rebuild_days: list[str] = field(default_factory=list)     # summaries rebuilt from the rows that remain
    delete_summaries: list[str] = field(default_factory=list)  # days whose raw rows are gone: summary deleted
    delete_narratives: list[str] = field(default_factory=list)  # days whose narrative was written from forgotten chat
    echoes: Any = None                                         # what FERB's own records say of it (echoes.Forgotten)
    scrubbed: Any = None                                       # what the echo scrub changed (echoes.Scrubbed)

    def describe(self, db: Database) -> list[str]:
        """What a forget deletes, naming each memory item (review M4: items built on the same sources go too)."""
        out = []
        for r in db.conn.execute(f"SELECT content, kind, subject, key FROM memory_items WHERE id IN "
                                 f"({','.join('?' * len(self.items))}) ORDER BY seq", self.items).fetchall() \
                if self.items else []:
            out.append(f"memory item “{' '.join(str(r['content']).split())}” ({r['kind']} · {r['subject']} · {r['key']})")
        if self.journal:
            kinds = [r["kind"] for r in db.conn.execute(
                f"SELECT kind FROM journal WHERE id IN ({','.join('?' * len(self.journal))})", self.journal)]
            out.append(f"{len(self.journal)} journal entr{'y' if len(self.journal) == 1 else 'ies'} "
                       f"({', '.join(sorted(set(kinds)))})")
        if self.trail:
            out.append(f"{len(self.trail)} trail row(s)")
        if self.rebuild_days:
            out.append("daily summaries rebuilt without them: " + ", ".join(self.rebuild_days))
        if self.delete_summaries:
            out.append("daily summaries deleted (raw rows already gone): " + ", ".join(self.delete_summaries))
        if self.delete_narratives:
            out.append("daily narratives deleted (written from the forgotten words): " + ", ".join(self.delete_narratives))
        out.append("FERB's lines that state it, in its replies and its stored model turns")
        return out


class Memory:
    def __init__(self, db: Database, clock: Clock, ids: IdSource, publish: Callable[[str, dict[str, Any]], None],
                 on_change: Callable[[], None] = lambda: None, redact: Callable[[str], str] = lambda s: s) -> None:
        self.db, self.clock, self.ids, self.publish, self.on_change = db, clock, ids, publish, on_change
        self.redact = redact                         # secrets never reach memory or its export
        self.rebuild_summary: Callable[[str], bool] = lambda day: False   # summary.py (True: rebuilt)
        self.on_forget: Callable[[ForgetPlan], None] = lambda plan: None
        # told the items that stopped waiting for the owner's Yes (confirmed or retracted: False) or were deleted
        # (forgotten: True), by any path. A confirm or retract closes only FERB's "Keep this?" questions about them; a
        # deletion closes every question about them, the owner's "Forget for good?" too (re-review NEW-1, R1)
        self.on_settled: Callable[[list[str], bool], None] = lambda ids, deleted: None
        # after the echo scrub: the episode cards whose body states the item go, with what is built on them (Q1)
        self.after_forget: Callable[[ForgetPlan], None] = lambda plan: None
        # told the current items that stopped holding now (L36 §6): superseded by a newer version (its id) or retired
        # (None); an end date reached is not told
        self.on_ended: Callable[[list[str], str | None], None] = lambda ids, new: None

    # ---------- reads ----------

    def get(self, item_id: str) -> MemoryItem | None:
        r = self.db.conn.execute("SELECT * FROM memory_items WHERE id = ?", (item_id,)).fetchone()
        return self._item(r) if r else None

    def _item(self, r: Any) -> MemoryItem:
        srcs = [f"{s['kind']}:{s['source_id']}" for s in self.db.conn.execute(
            "SELECT kind, source_id FROM memory_sources WHERE item_id = ? ORDER BY kind, source_id", (r["id"],))]
        return MemoryItem(r["id"], r["kind"], r["subject"], r["key"], r["content"], srcs,
                          datetime.fromisoformat(r["observed_at"]), datetime.fromisoformat(r["valid_from"]),
                          datetime.fromisoformat(r["valid_to"]) if r["valid_to"] else None, r["supersedes"],
                          float(r["confidence"]), r["origin"], r["status"])

    def items(self, status: str | None = "active", subject: str | None = None, valid_now: bool = True) -> list[MemoryItem]:
        q, args = "SELECT * FROM memory_items WHERE 1=1", list[Any]()
        if status:
            q, args = q + " AND status = ?", args + [status]
        if subject:
            q, args = q + " AND subject = ?", args + [subject]
        now = self.clock.now()
        if valid_now:                                    # a coarse bound in SQL (`SQL_SLACK`); the exact one below
            q, args = q + " AND (valid_to IS NULL OR valid_to > ?)", args + [(now - SQL_SLACK).isoformat()]
        found = [self._item(r) for r in self.db.conn.execute(q + " ORDER BY subject, kind, key, seq", args)]
        return [it for it in found if not valid_now or current(it, now)]

    def history(self, item_id: str) -> list[MemoryItem]:
        """Every version of this fact, oldest first."""
        it = self.get(item_id)
        if it is None:
            return []
        rows = self.db.conn.execute("SELECT * FROM memory_items WHERE kind = ? AND subject = ? AND key = ? "
                                    "ORDER BY seq", (it.kind, it.subject, it.key)).fetchall()
        return [self._item(r) for r in rows]

    def recall(self, query: str, k: int = 8, at: datetime | None = None) -> list[Recalled]:
        """Keyword (FTS5) recall of active items valid now (or at `at`); conflicting versions are flagged and the
        owner's own statement comes first."""
        when = (at or self.clock.now()).isoformat()
        terms = self.telling_words(query, at)
        if not terms:
            return []
        match = " OR ".join(f'"{t}"' for t in terms)
        rows = self.db.conn.execute(
            "SELECT m.* FROM memory_fts f JOIN memory_items m ON m.seq = f.rowid WHERE memory_fts MATCH ? "
            "AND m.status = 'active' AND valid_from <= ? AND (valid_to IS NULL OR valid_to > ?) "
            "ORDER BY bm25(memory_fts) LIMIT ?", (match, when, when, k)).fetchall()
        hits = [self._item(r) for r in rows]
        out = []
        for it in hits:
            others = [o for o in self._versions(it.kind, it.subject, it.key, at) if o.id != it.id]
            out.append(Recalled(it, conflict=any(o.content != it.content for o in others)))
        out.sort(key=lambda r: 0 if r.item.origin == "owner_stated" else 1)
        return out

    def telling_words(self, text: str, at: datetime | None = None) -> list[str]:
        """The words of `text` that carry weight among the active items valid now (or at `at`): `weighted`, the one
        search's rule (a word in more than half of them says nothing about any one). `recall` ranks by them; a distilled
        quote that ends or replaces an item must share one with it (L36 §6, review fix round 1)."""
        when = (at or self.clock.now()).isoformat()
        terms = query_terms(text)
        valid = "m.status = 'active' AND valid_from <= ? AND (valid_to IS NULL OR valid_to > ?)"
        total = int(self.db.conn.execute(f"SELECT COUNT(*) FROM memory_items m WHERE {valid}",
                                         (when, when)).fetchone()[0]) if terms else 0

        def matches(term: str) -> int:
            return int(self.db.conn.execute(
                f"SELECT COUNT(*) FROM memory_fts f JOIN memory_items m ON m.seq = f.rowid "
                f"WHERE memory_fts MATCH ? AND {valid}", (f'"{term}"', when, when)).fetchone()[0])
        return weighted(terms, total, matches) if total else []

    def current_ids(self, origin: str | None = None) -> set[str]:
        """The ids of the active items that hold now (of `origin`), as `items` finds them, without reading each one:
        a login record's username still comes from one of these (L36 §6, review fix round 1)."""
        now = self.clock.now()
        q, args = ("SELECT id, valid_to FROM memory_items WHERE status = 'active' "
                   "AND (valid_to IS NULL OR valid_to > ?)", [(now - SQL_SLACK).isoformat()])
        if origin:
            q, args = q + " AND origin = ?", args + [origin]
        return {r["id"] for r in self.db.conn.execute(q, args)
                if not r["valid_to"] or datetime.fromisoformat(r["valid_to"]) > now}

    def _versions(self, kind: str, subject: str, key: str, at: datetime | None = None) -> list[MemoryItem]:
        when = (at or self.clock.now()).isoformat()
        rows = self.db.conn.execute(
            "SELECT * FROM memory_items WHERE kind = ? AND subject = ? AND key = ? AND status = 'active' "
            "AND valid_from <= ? AND (valid_to IS NULL OR valid_to > ?)", (kind, subject, key, when, when)).fetchall()
        return [self._item(r) for r in rows]

    def evidence_days(self, item_id: str) -> int:
        return int(self.db.conn.execute(
            "SELECT COUNT(DISTINCT day) AS n FROM memory_sources WHERE item_id = ? AND kind = 'trail' "
            "AND actor = 'owner'", (item_id,)).fetchone()["n"])

    # ---------- writes ----------

    def propose(self, kind: str, subject: str, key: str, content: str, origin: str, sources: Iterable[Source],
                confidence: float = 0.7) -> MemoryItem:
        """Every item enters here. Enforces the origin rules; observed evidence accumulates on one proposal."""
        kind, subject, key = kind.strip(), self.redact(subject.strip()) or "owner", slug(self.redact(key))
        content = self.redact(content.strip())
        srcs = list(dict.fromkeys(sources))
        self._check(kind, subject, key, content, origin, srcs)
        now = self.clock.now()
        same = self.db.conn.execute(             # identical content is always the same item and is credited first
            "SELECT id FROM memory_items WHERE kind = ? AND subject = ? AND key = ? AND lower(content) = lower(?) "
            "AND origin = ? AND status != 'retracted' AND valid_to IS NULL",
            (kind, subject, key, content, origin)).fetchone()
        if same is None and origin == "observed":  # a pending pattern: same key = same pattern, any wording
            same = self.db.conn.execute(
                "SELECT id FROM memory_items WHERE kind = ? AND subject = ? AND key = ? AND origin = 'observed' "
                "AND status = 'proposed' AND valid_to IS NULL ORDER BY seq LIMIT 1", (kind, subject, key)).fetchone()
        if same is not None:                                     # the same fact again: more evidence, no new item
            item_id = same["id"]
        else:
            item_id = self.ids.new_id()
            self.db.conn.execute(
                "INSERT INTO memory_items (id, kind, subject, key, content, origin, status, confidence, observed_at, "
                "valid_from, created_at) VALUES (?, ?, ?, ?, ?, ?, 'proposed', ?, ?, ?, ?)",
                (item_id, kind, subject, key, content, origin, confidence, now.isoformat(), now.isoformat(),
                 now.isoformat()))
            self.publish("memory.proposed", {"id": item_id, "kind": kind, "subject": subject, "origin": origin})
        self._add_sources(item_id, srcs)
        if origin == "owner_stated" or (origin == "observed" and self.evidence_days(item_id) >= OBSERVED_DAYS):
            self.commit([item_id], "owner stated" if origin == "owner_stated" else f"seen on {OBSERVED_DAYS}+ days")
        elif origin == "third_party":
            self.commit([item_id], "a fact about the world")
        it = self.get(item_id)
        assert it is not None
        return it

    def _check(self, kind: str, subject: str, key: str, content: str, origin: str, srcs: list[Source]) -> None:
        if kind not in KINDS or origin not in ORIGINS:
            raise MemoryRefused(f"unknown kind / origin {kind} / {origin}")
        if not content:
            raise MemoryRefused("empty content")
        if not srcs:
            raise MemoryRefused("an item needs at least one source")
        trail = [s for s in srcs if s[0] == "trail"]
        rows = self._trail_rows([s[1] for s in trail])
        if len(rows) != len(trail):
            raise MemoryRefused("a trail source doesn't exist")
        actors = {r["actor"] for r in rows}
        if origin == "observed":
            if not trail or len(trail) != len(srcs) or actors != {"owner"}:
                raise MemoryRefused("an observed item needs the owner's own trail rows only (never FERB's activity)")
        if origin in ("observed", "inferred") and "ferb" in actors and (kind == "preference" or subject == "owner"):
            raise MemoryRefused("FERB's own activity never becomes the owner's habits or preferences")
        if origin == "third_party" and (kind == "preference" or subject == "owner"):
            raise MemoryRefused("third-party content can't become a preference or a fact about the owner")
        if origin != "owner_stated":
            hit = self.db.conn.execute("SELECT 1 FROM memory_forgotten WHERE fingerprint = ?",
                                       (fingerprint(kind, subject, key, content),)).fetchone()
            if hit is not None:
                raise MemoryRefused("the owner asked FERB to forget this")

    def _trail_rows(self, ids: list[str]) -> list[Any]:
        if not ids:
            return []
        return self.db.conn.execute(f"SELECT id, day, actor FROM trail WHERE id IN ({','.join('?' * len(ids))})",
                                    ids).fetchall()

    def _add_sources(self, item_id: str, srcs: list[Source]) -> None:
        trail = {r["id"]: r for r in self._trail_rows([s[1] for s in srcs if s[0] == "trail"])}
        for kind, sid in srcs:
            day, actor = None, None
            if kind == "trail" and sid in trail:
                day, actor = trail[sid]["day"], trail[sid]["actor"]
            elif kind == "journal":
                r = self.db.conn.execute("SELECT at FROM journal WHERE id = ?", (sid,)).fetchone()
                day = datetime.fromisoformat(r["at"]).astimezone().date().isoformat() if r else None
            self.db.conn.execute("INSERT OR IGNORE INTO memory_sources (item_id, kind, source_id, day, actor) "
                                 "VALUES (?, ?, ?, ?, ?)", (item_id, kind, sid, day, actor))

    def commit(self, ids: list[str], reason: str) -> None:
        """proposed -> active. A newer version of the same fact supersedes the old (history kept), except that only
        the owner's own statement may supersede one the owner stated."""
        now = self.clock.now().isoformat()
        settled: list[str] = []
        for item_id in ids:
            it = self.get(item_id)
            if it is None or it.status == "active":
                continue
            settled.append(item_id)
            prev = [o for o in self._versions(it.kind, it.subject, it.key) if o.id != it.id]
            closes = [o for o in prev if it.origin == "owner_stated" or o.origin != "owner_stated"]
            for o in closes:
                self.db.conn.execute("UPDATE memory_items SET valid_to = ? WHERE id = ?", (now, o.id))
            self.db.conn.execute("UPDATE memory_items SET status = 'active', valid_from = ?, supersedes = ? "
                                 "WHERE id = ?", (now, closes[-1].id if closes else None, item_id))
            self.publish("memory.committed", {"id": item_id, "reason": reason})
            if closes:
                self.on_ended([o.id for o in closes], item_id)
        if settled:
            self.on_settled(settled, False)
        self.on_change()

    def confirm(self, item_id: str) -> MemoryItem:
        it = self.get(item_id)
        if it is None or it.status != "proposed":
            raise MemoryRefused(f"no proposed item {item_id}")
        self.commit([item_id], "confirmed by the owner")
        out = self.get(item_id)
        assert out is not None
        return out

    def retract(self, item_id: str, reason: str, *, never_again: bool = False) -> None:
        """Soft: kept for history, never recalled. `never_again`: its fingerprint is kept too, so distillation never
        proposes these words for this fact again (the owner replaced or dropped one of FERB's guesses, L36)."""
        it = self.get(item_id)
        if it is None:
            raise MemoryRefused(f"no item {item_id}")
        self.db.conn.execute("UPDATE memory_items SET status = 'retracted' WHERE id = ?", (item_id,))
        if never_again:
            self._never_again(it)
        self.on_settled([item_id], False)
        self.on_change()

    def drop(self, item_id: str) -> None:
        """The owner's Drop on one of FERB's guesses (L36 §2): this proposal alone is deleted and its fingerprint kept,
        so distillation never proposes it again. The owner's messages it came from and every other version of the fact
        stay: a guess never reached a prompt, and `forget` would purge every version of its key with their sources."""
        it = self.get(item_id)
        if it is None or it.status != "proposed":
            raise MemoryRefused(f"no proposed item {item_id}")
        self.db.conn.execute("DELETE FROM memory_sources WHERE item_id = ?", (item_id,))
        self.db.conn.execute("DELETE FROM memory_items WHERE id = ?", (item_id,))
        self._never_again(it)
        self.on_settled([item_id], True)
        self.on_change()

    def expire_at(self, item_id: str, at: datetime) -> None:
        """The owner's words give this item an end (L36 §6): it stops being valid at `at`, kept as history."""
        stamp = at.astimezone(self.clock.now().tzinfo).isoformat()     # the clock's offset, as other stamps have
        self.db.conn.execute("UPDATE memory_items SET valid_to = ? WHERE id = ? AND valid_to IS NULL", (stamp, item_id))
        self.publish("memory.expires", {"id": item_id, "at": stamp})
        self.on_change()

    def retire(self, item_id: str, reason: str) -> None:
        """The owner's words say this current item no longer holds (L36 §6): it stops being valid now, kept as
        history."""
        it, now = self.get(item_id), self.clock.now()
        if it is None or it.status != "active" or not current(it, now):
            raise MemoryRefused(f"no current item {item_id}")
        self.db.conn.execute("UPDATE memory_items SET valid_to = ? WHERE id = ?", (now.isoformat(), item_id))
        self.publish("memory.retired", {"id": item_id, "reason": reason})
        self.on_ended([item_id], None)
        self.on_change()

    def _never_again(self, it: MemoryItem) -> None:
        self.db.conn.execute("INSERT OR IGNORE INTO memory_forgotten (fingerprint, at) VALUES (?, ?)",
                             (fingerprint(it.kind, it.subject, it.key, it.content), self.clock.now().isoformat()))

    def edit(self, item_id: str, content: str, source: Source) -> MemoryItem:
        """The owner's correction: a new owner-stated version (the old one is kept as history)."""
        it = self.get(item_id)
        if it is None:
            raise MemoryRefused(f"no item {item_id}")
        return self.propose(it.kind, it.subject, it.key, content, "owner_stated", [source], 1.0)

    # ---------- forget = purge ----------

    def plan_forget(self, item_id: str, keep_sources: bool = False) -> ForgetPlan:
        it = self.get(item_id)
        if it is None:
            raise MemoryRefused(f"no item {item_id}")
        items = {v.id for v in self.history(item_id)}
        sources: set[tuple[str, str]] = set()
        if not keep_sources:
            changed = True
            while changed:                                           # items built on the same sources, transitively
                changed = False
                for iid in list(items):
                    for s in self.db.conn.execute("SELECT kind, source_id FROM memory_sources WHERE item_id = ?",
                                                  (iid,)):
                        sources.add((s["kind"], s["source_id"]))
                for kind, sid in list(sources):
                    for r in self.db.conn.execute("SELECT item_id FROM memory_sources WHERE kind = ? AND "
                                                  "source_id = ?", (kind, sid)):
                        if r["item_id"] not in items:
                            items.add(r["item_id"])
                            changed = True
        plan = ForgetPlan(sorted(items), sorted(s for k, s in sources if k == "journal"),
                          sorted(s for k, s in sources if k == "trail"))
        if plan.trail:
            days = sorted({r["day"] for r in self._trail_rows(plan.trail)})
            summarized = self._summary_days()
            for d in days:
                if d in summarized:
                    plan.rebuild_days.append(d)
        narrated = self._narrative_days()
        for jid in plan.journal:                                     # a summary itself as a source: it goes
            r = self.db.conn.execute("SELECT kind, at, data_json FROM journal WHERE id = ?", (jid,)).fetchone()
            if r is not None and r["kind"] == "trail.day_summary":
                plan.delete_summaries.append(str(json.loads(r["data_json"]).get("day")))
            elif r is not None and r["kind"] == "chat.message":       # the day's narrative was written from it
                day = datetime.fromisoformat(r["at"]).astimezone().date().isoformat()
                if day in narrated and day not in plan.delete_narratives:
                    plan.delete_narratives.append(day)
        plan.rebuild_days = [d for d in plan.rebuild_days if d not in plan.delete_summaries]
        return plan

    def _narrative_days(self) -> set[str]:
        return {str(json.loads(r["data_json"]).get("day")) for r in self.db.conn.execute(
            "SELECT data_json FROM journal WHERE kind = 'trail.day_narrative'")}

    def _summary_days(self) -> set[str]:
        return {str(json.loads(r["data_json"]).get("day")) for r in self.db.conn.execute(
            "SELECT data_json FROM journal WHERE kind = 'trail.day_summary'")}

    def forget(self, item_id: str, keep_sources: bool = False, *, finish: bool = True) -> ForgetPlan:
        """`finish` False: the deletions only; the caller runs `echoes.finish` (the daemon does it off the event loop),
        then `after_forget` and an emptied WAL."""
        from ferb.memory import echoes
        plan = self.plan_forget(item_id, keep_sources)
        plan.echoes = echoes.capture(self.db, plan.items, plan.journal, plan.trail)   # read before anything goes
        # Delete derived copies first. If that fails, retain the source/plan so the owner can retry.
        self.on_forget(plan)
        now = self.clock.now().isoformat()
        c = self.db.conn
        prints = [fingerprint(r["kind"], r["subject"], r["key"], r["content"]) for r in c.execute(
            f"SELECT kind, subject, key, content FROM memory_items WHERE id IN ({','.join('?' * len(plan.items))})",
            plan.items)]
        c.execute("BEGIN")
        try:
            for iid in plan.items:
                c.execute("DELETE FROM memory_sources WHERE item_id = ?", (iid,))
                c.execute("DELETE FROM memory_items WHERE id = ?", (iid,))
            for jid in plan.journal:
                c.execute("DELETE FROM journal WHERE id = ?", (jid,))
            for tid in plan.trail:
                c.execute("DELETE FROM trail WHERE id = ?", (tid,))
            for day in plan.rebuild_days:
                if not self._has_raw(day):
                    plan.delete_summaries.append(day)
            for day in plan.delete_summaries:
                c.execute("DELETE FROM journal WHERE kind = 'trail.day_summary' AND json_extract(data_json, '$.day') = ?",
                          (day,))
                c.execute("DELETE FROM journal WHERE kind = 'trail.day_narrative' AND json_extract(data_json, '$.day') = ?",
                          (day,))
            for day in plan.delete_narratives:
                c.execute("DELETE FROM journal WHERE kind = 'trail.day_narrative' AND json_extract(data_json, '$.day') = ?",
                          (day,))
            for fp in prints:
                c.execute("INSERT OR IGNORE INTO memory_forgotten (fingerprint, at) VALUES (?, ?)", (fp, now))
            c.execute("COMMIT")
        except Exception:
            c.execute("ROLLBACK")
            raise
        plan.rebuild_days = [d for d in plan.rebuild_days if d not in plan.delete_summaries]
        for day in plan.rebuild_days:
            self.rebuild_summary(day)
        # memory_fts zeroes a deleted item's terms itself (FTS5 secure-delete, migration 0037): no rebuild here
        if finish:
            plan.scrubbed = echoes.finish(self.db, plan.echoes)       # what FERB's own records said of it (I2)
            self.after_forget(plan)
            self.db.truncate_wal("memory.forget")                     # no copy left behind in the WAL
        self.on_settled(plan.items, True)
        self.on_change()
        return plan

    def _has_raw(self, day: str) -> bool:
        return self.db.conn.execute("SELECT 1 FROM trail WHERE day = ? LIMIT 1", (day,)).fetchone() is not None
