"""Owns memory storage, provenance and versions. Must not treat retrieved observations as owner authorization or silently cut stored content.

Memory items: SQLite is the source of truth; every rule about where an item may come from is here.

No Keep questions (MEM-12): FERB saves facts itself, each labelled by where it came from
(`LABELS`), and the owner corrects by talking or on the memory page. Memory never authorizes an action.
Origins, enforced in code:
- `owner_stated` ("your words"): the owner's own words (`ferb memory add`, `edit`, or chat capture whose saved text
  appears word for word in what the owner typed, `contains_words`) -> active on commit.
- `inferred` ("FERB's understanding"): FERB's wording of what the owner said, or a conclusion the daily distill drew
  from their day -> active on commit too; it reaches the core profile (Tier 1, profile.py) only once it holds up.
- `observed` ("observed"): from the owner's own activity. Its sources are trail rows with actor "owner" (FERB's own
  activity never becomes the owner's habits), active once that evidence spans 3 separate days; or the code-gathered
  facts of the owner's Mac (installed apps, their use, browsers and profiles: a `MAC_PROFILE_KIND` journal row,
  MEM-13), active at once, and in the core profile at once (MEM-423).
- `third_party`: pages, emails, pasted text. A fact about the world only: never a preference, never about the owner.

Versions: kind + subject + key name one fact (subject and key normalized alike, `subject_key` / `slug`). A newer version
closes the old one (`valid_to`) and links to it (`supersedes`); nothing is overwritten. A version that rests on the
owner's words now (`from_owner`: chat capture, `memory add`/`edit`, a distilled quote of theirs) ends every current
version of its fact and the items the planner names it `replaces` (code checks each exists and holds); any other
version never ends something the owner stated (both stay; the owner's wins). `save` returns what it replaced, so FERB's
line names it. An item also ends at the end of a day the owner's words give it (`expire_at`) or when it no longer holds
(`retire`); it stays as history, and `items` and `prompt_ready` treat it as current until then (L36 §6).

`forget` is simple (MEM-2): the item, all its versions, their source links and the
watcher-captured trail rows they came from (unless the owner keeps them), the search index, the WAL; days whose trail
rows went are re-summarized (or their summary deleted once the raw rows are gone). The owner's chat message that stated
it stays (chat history is the owner's own record), other facts from the same message stay, and FERB's past answers and
traces are not scrubbed. A person or site the item names leaves the known destinations too, so a send there asks again
(MEM-11). A one-way fingerprint stops distillation from proposing it again."""

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
from ferb.policy import blocked as blocked_
from ferb.sanitize.redact import word_char  # one word-character rule with redaction (L36 §7)
from ferb.store.db import Database

KINDS = ("fact", "preference", "person", "project", "env")
ORIGINS = ("owner_stated", "observed", "inferred", "third_party")
# MEM-12: how each origin is named to the owner and to every role (the memory page, FERB's lines, the prompts)
LABELS = {"owner_stated": "your words", "inferred": "FERB's understanding", "observed": "observed",
          "third_party": "from outside content"}
OBSERVED_DAYS = 3
# MEM-13: the journal row holding the code-gathered facts of the owner's Mac an observed item may rest on
MAC_PROFILE_KIND = "memory.mac_profile"
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
        if (start == 0 or not word_char(hay[start - 1])) and (end == len(hay) or not word_char(hay[end])):
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
        before = m.start() == 0 or not word_char(text[m.start() - 1])
        if before and (m.end() == len(text) or not word_char(text[m.end()])):
            return m.start(), m.end()
    return None


def _clause_span(text: str, start: int, end: int) -> tuple[int, int]:
    left = max(text.rfind(c, 0, start) for c in _CLAUSE_MARKS) + 1
    rights = [i for i in (text.find(c, end) for c in _CLAUSE_MARKS) if i != -1]
    return left, min(rights) if rights else len(text)


def owner_clauses(text: str, part: str) -> str | None:
    """The clause(s) of the owner's `text` that hold `part` as whole words, in their own spelling: what is saved as
    their words (owner 2026-11-04, MEM-419: only the span that states the fact, never their whole message). A clause ends
    at , ; : ! ? or a line break, never at a period ("my prof. Zorblat"), so a negation or condition in the same clause
    stays ("never share my links publicly, just summarize" saves "never share my links publicly"); one a mark sets
    apart ("If it's a work email, sign it as Sam") stays only when `part` holds it, as the planner is told. None when
    `part` isn't in `text`."""
    found = _found(text, part)
    if found is None:
        return None
    left, right = _clause_span(text, *found)
    return text[left:right].strip(_EDGE + "!?,;:") or None


def whole_clauses(text: str, part: str) -> bool:
    """Whether `part` is exactly one or more whole clauses of `text`: a meaningful span of the owner's words, never any
    word they happened to type ("the"). A chat forget's quote must be one."""
    found = _found(text, part)
    if found is None:
        return False
    left, right = _clause_span(text, *found)
    return not any(word_char(c) for c in text[left:found[0]] + text[found[1]:right])


def quoted(owner_words: str, quote: str, *, clause: bool = False) -> bool:
    """The one quote check (step 8c) for everything a chat turn does on the owner's say-so: remember and its end date,
    reminders, schedules and their changes, reminder cancels, forgets and conversation changes. `quote` must be whole
    words of what the owner typed (`owner_words`: pasted or quoted parts never count), case and spacing aside; a quote
    of several of the owner's messages joined by blank lines counts when each part is. `clause`: a forget's quote must
    also be whole clauses (a meaningful span, never any word they happened to type)."""
    parts = [part for part in re.split(r"\n[ \t]*\n", quote) if part.strip()]
    check = whole_clauses if clause else contains_words
    return bool(parts) and all(check(owner_words, part) for part in parts)



def forgets_on_record(db: Database) -> list[dict[str, Any]]:
    """ORCH-578: what FERB can say of what it forgot: each forget the memory store journaled (`memory.forgot`, its own
    record, once per forget), when and how many items. A forgotten item's contents are never kept (memory_forgotten
    holds only fingerprints)."""
    import json
    rows = db.conn.execute("SELECT at, data_json FROM journal WHERE kind = 'memory.forgot' ORDER BY seq").fetchall()
    return [{"at": str(r[0]), "items": data["items"]} for r in rows
            if "via" not in (data := json.loads(r[1] or "{}")) and isinstance(data.get("items"), int)]

@dataclass(frozen=True)
class Capture:
    """What chat capture saves from the owner's typed words (`capture`)."""
    origin: str          # "owner_stated", "inferred", or "" when nothing is saved
    content: str
    refused: str = ""    # why nothing is saved, in code's words (CAPTURE_TOLD has the owner's)


# Each refusal as the owner reads it in FERB's line (owner 2026-10-24: the reason, never "a problem on my side"); both
# are the quote check's, so the line says a safety check stopped it (owner 2026-11-04, ORCH-519: ferb/policy/blocked.py)
CAPTURE_TOLD = {"quote not in the owner's words": blocked_.NOT_IN_WORDS,
                "the quote holds nothing of what would be saved": blocked_.DOESNT_SAY}


def capture(own_words: str, content: str, quote: str) -> Capture:
    """Chat capture's check (memory review I1, MP2), shared by the planner's remember entries and distillation's quotes
    (L36 §2). `own_words`: what the owner typed (pasted parts left out), with its line breaks. The quote must be whole
    words of it. Then the clause(s) holding `content` are saved as the owner's own words (`owner_clauses`); when the
    content isn't there, `content` is FERB's wording (inferred), provided the quote shares a word with it."""
    content = content.strip()
    if not quoted(own_words, quote):
        return Capture("", content, "quote not in the owner's words")
    clause = owner_clauses(own_words, content)
    if clause is None and not set(query_terms(quote)) & set(query_terms(content)):
        return Capture("", content, "the quote holds nothing of what would be saved")
    return Capture("owner_stated", clause) if clause else Capture("inferred", content)


def owner_span_at(text: str, part: str) -> tuple[int, int] | None:
    """Where `part` is in `text` as the owner spelled it (whole words, case and spacing aside): (start, end), or None."""
    return _found(text, part)


def current(item: MemoryItem, now: datetime | None = None) -> bool:
    """Whether an item still holds: no end, or (with `now`) an end still ahead, as the owner's dated words give one
    (L36 §6). Without `now`, only an item without an end."""
    return item.valid_to is None or (now is not None and item.valid_to > now)


def prompt_ready(item: MemoryItem, now: datetime | None = None) -> bool:
    """Whether an item may shape FERB's prompts: a current active item (one with an end date, until then: L36 §6), or
    an observed pattern still gathering days (shown as an unconfirmed hint). Each carries its label (`LABELS`)."""
    return current(item, now) and not item.set_aside and (
        item.status == "active" or (item.status == "proposed" and item.origin == "observed"))


def label(origin: str) -> str:
    return LABELS.get(origin, origin)


def subject_key(subject: str) -> str:
    """A subject as memory stores it, normalized like the key (fix list: "project:home-move" and
    "Project:Home move" name one subject): "owner" when empty; a "<kind>:<name>" subject keeps its kind and slugs its
    name."""
    subject = subject.strip()
    if not subject:
        return "owner"
    head, sep, rest = subject.partition(":")
    if not sep:
        return slug(subject)
    return f"{slug(head)}:{slug(rest)}" if rest.strip() else slug(head)


@dataclass(frozen=True)
class Saved:
    """What `Memory.save` stored: the item, the current items it replaced (FERB's line names them), and the ids the
    planner named that weren't replaced (no such current item), with code's reason."""
    item: MemoryItem
    replaced: list[MemoryItem] = field(default_factory=list)
    not_replaced: list[str] = field(default_factory=list)


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
class ForgetPlan:
    items: list[str] = field(default_factory=list)            # the item's versions
    trail: list[str] = field(default_factory=list)            # watcher-captured trail rows they came from
    rebuild_days: list[str] = field(default_factory=list)     # summaries rebuilt from the rows that remain
    delete_summaries: list[str] = field(default_factory=list)  # days whose raw rows are gone: summary deleted
    kept: int = 0                                              # the owner's own messages it came from (they stay)
    # MEM-11: known destinations (people, sites, addresses) the item names: a send to them asks again
    destinations: list[tuple[str, str]] = field(default_factory=list)   # (kind, value)

    def describe(self, db: Database) -> list[str]:
        """Exactly what the forget deletes, and what it keeps."""
        out = []
        for r in db.conn.execute(f"SELECT content, kind, subject, key FROM memory_items WHERE id IN "
                                 f"({','.join('?' * len(self.items))}) ORDER BY seq", self.items).fetchall() \
                if self.items else []:
            out.append(f"memory item “{' '.join(str(r['content']).split())}” ({r['kind']} · {r['subject']} · {r['key']})")
        if self.trail:
            out.append(f"{len(self.trail)} recorded activity row(s) it was learned from")
        if self.rebuild_days:
            out.append("daily summaries rebuilt without them: " + ", ".join(self.rebuild_days))
        if self.delete_summaries:
            out.append("daily summaries deleted (raw rows already gone): " + ", ".join(self.delete_summaries))
        if self.destinations:
            out.append("FERB's record of having sent to " + ", ".join(v for _, v in self.destinations)
                       + " (sending there asks you again)")
        return out

    def keeps(self) -> str:
        """What stays, in one line for the owner."""
        return ("Your own messages stay in the chat history; FERB won't learn it from them again."
                if self.kept else "FERB won't learn it again.")


class Memory:
    def __init__(self, db: Database, clock: Clock, ids: IdSource, publish: Callable[[str, dict[str, Any]], None],
                 on_change: Callable[[], None] = lambda: None, redact: Callable[[str], str] = lambda s: s) -> None:
        self.db, self.clock, self.ids, self.publish, self.on_change = db, clock, ids, publish, on_change
        self.redact = redact                         # secrets never reach memory or its export
        self.rebuild_summary: Callable[[str], bool] = lambda day: False   # summary.py (True: rebuilt)
        self.on_forget: Callable[[ForgetPlan], None] = lambda plan: None
        # told the items that were deleted (forgotten: True) or retracted (False), by any path: FERB's questions about
        # them close (a deletion closes the owner's "Forget for good?" too)
        self.on_settled: Callable[[list[str], bool], None] = lambda ids, deleted: None
        # told the current items that stopped holding now (L36 §6): superseded by a newer version (its id) or retired
        # (None); an end date reached is not told
        self.on_ended: Callable[[list[str], str | None], None] = lambda ids, new: None
        # told an item that became current (a new fact, or a new version of one): the core profile follows its fact
        self.on_saved: Callable[[MemoryItem], None] = lambda item: None
        # MEM-14: told the owner's items FERB set aside as over (the core profile drops them; nothing else ends)
        self.on_set_aside: Callable[[list[str]], None] = lambda ids: None

    # ---------- reads ----------

    def get(self, item_id: str) -> MemoryItem | None:
        r = self.db.conn.execute("SELECT * FROM memory_items WHERE id = ?", (item_id,)).fetchone()
        return self._item(r) if r else None

    def _item(self, r: Any) -> MemoryItem:
        srcs = [f"{s['kind']}:{s['source_id']}" for s in self.db.conn.execute(
            "SELECT kind, source_id FROM memory_sources WHERE item_id = ? ORDER BY kind, source_id", (r["id"],))]
        extra = self.db.conn.execute("SELECT about, review_at FROM memory_context WHERE item_id = ?",
                                     (r["id"],)).fetchone()
        aside = self.db.conn.execute("SELECT 1 FROM memory_set_aside WHERE item_id = ?", (r["id"],)).fetchone()
        return MemoryItem(r["id"], r["kind"], r["subject"], r["key"], r["content"], srcs,
                          datetime.fromisoformat(r["observed_at"]), datetime.fromisoformat(r["valid_from"]),
                          datetime.fromisoformat(r["valid_to"]) if r["valid_to"] else None, r["supersedes"],
                          float(r["confidence"]), r["origin"], r["status"],
                          str(extra["about"]) if extra else "",
                          datetime.fromisoformat(extra["review_at"]) if extra and extra["review_at"] else None,
                          aside is not None)

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

    def current_items(self) -> list[MemoryItem]:
        """What holds now and may shape FERB's prompts (`prompt_ready`): what memory.read lists without an id."""
        now = self.clock.now()
        return [it for it in self.items(None) if prompt_ready(it, now)]

    def history(self, item_id: str) -> list[MemoryItem]:
        """Every version of this fact, oldest first."""
        it = self.get(item_id)
        if it is None:
            return []
        rows = self.db.conn.execute("SELECT * FROM memory_items WHERE kind = ? AND subject = ? AND key = ? "
                                    "ORDER BY seq", (it.kind, it.subject, it.key)).fetchall()
        return [self._item(r) for r in rows]

    def telling_words(self, text: str, at: datetime | None = None) -> list[str]:
        """The words of `text` that carry weight among the active items valid now (or at `at`): `weighted`, the one
        search's rule (a word in more than half of them says nothing about any one). A distilled quote that ends or
        replaces an item must share one with it (L36 §6, review fix round 1)."""
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
        """The current active versions of a fact: of this kind, subject and key."""
        when = (at or self.clock.now()).isoformat()
        rows = self.db.conn.execute(
            "SELECT * FROM memory_items WHERE kind = ? AND subject = ? AND key = ? AND status = 'active' "
            "AND valid_from <= ? AND (valid_to IS NULL OR valid_to > ?)", (kind, subject, key, when, when)).fetchall()
        return [self._item(r) for r in rows]

    def evidence_days(self, item_id: str) -> int:
        return int(self.db.conn.execute(
            "SELECT COUNT(DISTINCT day) AS n FROM memory_sources WHERE item_id = ? AND kind = 'trail' "
            "AND actor = 'owner'", (item_id,)).fetchone()["n"])

    def _mac_sources_only(self, item_id: str) -> bool:
        """Whether every source of an item is the code-gathered facts of the owner's Mac (MEM-13)."""
        rows = self.db.conn.execute(
            "SELECT s.kind, j.kind AS jkind FROM memory_sources s LEFT JOIN journal j ON j.id = s.source_id "
            "AND s.kind = 'journal' WHERE s.item_id = ?", (item_id,)).fetchall()
        return bool(rows) and all(r["kind"] == "journal" and r["jkind"] == MAC_PROFILE_KIND for r in rows)

    def from_mac_scan(self, item_id: str) -> bool:
        """Whether an item is one of the Mac profile scan's facts (MEM-13, MEM-24): it rests on a `MAC_PROFILE_KIND`
        journal row, the gathered facts `mac_profile.store` saves each of its conclusions on (setup's scan and each
        weekly re-interpretation alike). A structural fact of its sources, never of what it says."""
        return self.db.conn.execute(
            "SELECT 1 FROM memory_sources s JOIN journal j ON j.id = s.source_id WHERE s.item_id = ? "
            "AND s.kind = 'journal' AND j.kind = ? LIMIT 1", (item_id, MAC_PROFILE_KIND)).fetchone() is not None

    # ---------- writes ----------

    def propose(self, kind: str, subject: str, key: str, content: str, origin: str, sources: Iterable[Source],
                confidence: float = 0.7) -> MemoryItem:
        """`save`, for callers that need only the item."""
        return self.save(kind, subject, key, content, origin, sources, confidence).item

    def save(self, kind: str, subject: str, key: str, content: str, origin: str, sources: Iterable[Source],
             confidence: float = 0.7, *, replaces: Iterable[str] = (), about: str = "",
             from_owner: bool | None = None) -> Saved:
        """Every item enters here. Enforces the origin rules; observed evidence accumulates on one proposal.
        `from_owner` (default: owner_stated): the version rests on the owner's words now, so it ends every current
        version of its fact and the current items it `replaces` (ids the planner named, checked here). `about`: what the
        owner's words are about (MEM-10), stored next to them, never in them."""
        kind, subject, key = kind.strip(), subject_key(self.redact(subject)), slug(self.redact(key))
        content = self.redact(content.strip())
        srcs = list(dict.fromkeys(sources))
        owner_now = origin == "owner_stated" if from_owner is None else from_owner
        self._check(kind, subject, key, content, origin, srcs)
        now = self.clock.now()
        same = self.db.conn.execute(             # identical content is the same item: credited, never a twin
            "SELECT id FROM memory_items WHERE kind = ? AND subject = ? AND key = ? AND lower(content) = lower(?) "
            "AND (origin = ? OR origin = 'owner_stated') AND status != 'retracted' AND valid_to IS NULL "
            "ORDER BY origin = 'owner_stated' DESC, seq DESC",
            (kind, subject, key, content, origin)).fetchone()
        if same is None and origin == "observed":  # a pending pattern: same key = same pattern, any wording
            same = self.db.conn.execute(
                "SELECT id FROM memory_items WHERE kind = ? AND subject = ? AND key = ? AND origin = 'observed' "
                "AND status = 'proposed' AND valid_to IS NULL ORDER BY seq LIMIT 1", (kind, subject, key)).fetchone()
        if same is not None:                                     # the same fact again: more evidence, no new item
            item_id = same["id"]
            if owner_now:                                        # MEM-14: saying it again says it still holds
                self.restore(item_id)
        else:
            item_id = self.ids.new_id()
            self.db.conn.execute(
                "INSERT INTO memory_items (id, kind, subject, key, content, origin, status, confidence, observed_at, "
                "valid_from, created_at) VALUES (?, ?, ?, ?, ?, ?, 'proposed', ?, ?, ?, ?)",
                (item_id, kind, subject, key, content, origin, confidence, now.isoformat(), now.isoformat(),
                 now.isoformat()))
            self.publish("memory.proposed", {"id": item_id, "kind": kind, "subject": subject, "origin": origin})
        self._add_sources(item_id, srcs)
        if about.strip():
            self.set_about(item_id, about)
        wanted = [i for i in dict.fromkeys(replaces) if i and i != item_id]
        named, refused = self._replaceable(wanted) if owner_now else ([], [(i, "") for i in wanted])
        replaced: list[MemoryItem] = []
        if origin in ("owner_stated", "inferred", "third_party") or (
                origin == "observed" and (self.evidence_days(item_id) >= OBSERVED_DAYS
                                          or self._mac_sources_only(item_id))):
            reason = {"owner_stated": "owner stated", "inferred": "FERB's understanding",
                      "third_party": "a fact about the world"}.get(origin, "observed")
            replaced = self._commit(item_id, reason, owner_now, named)
            if same is not None and named:        # already current: the facts it was named to replace still end
                replaced += self._end([o for o in named if o.id not in {r.id for r in replaced}], item_id)
        it = self.get(item_id)
        assert it is not None
        if same is not None and it.status == "active" and not replaced:
            self.on_saved(it)                     # seen again: the core profile may now take it (MEM-15)
        return Saved(it, replaced, [i for i, _ in refused])

    def _replaceable(self, ids: list[str]) -> tuple[list[MemoryItem], list[tuple[str, str]]]:
        """The current items of `ids` (what a new version may replace), and the others with why not."""
        now, ok, refused = self.clock.now(), list[MemoryItem](), list[tuple[str, str]]()
        for i in ids:
            it = self.get(i)
            if it is None:
                refused.append((i, "no such memory"))
            elif it.status != "active" or not current(it, now):
                refused.append((i, "it no longer holds"))
            else:
                ok.append(it)
        return ok, refused

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
            journal = [s[1] for s in srcs if s[0] == "journal"]
            mac = journal and len(journal) == len(srcs) and all(
                (r := self.db.conn.execute("SELECT kind FROM journal WHERE id = ?", (j,)).fetchone()) is not None
                and r["kind"] == MAC_PROFILE_KIND for j in journal)
            if not mac and (not trail or len(trail) != len(srcs) or actors != {"owner"}):
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
        """proposed -> active (an observed pattern whose evidence now spans enough days, by any path)."""
        for item_id in ids:
            it = self.get(item_id)
            if it is not None and it.status != "active":
                self._commit(item_id, reason, it.origin == "owner_stated", [])

    def _commit(self, item_id: str, reason: str, owner_now: bool, named: list[MemoryItem]) -> list[MemoryItem]:
        """Make a proposed item the current version of its fact; returns the current items it ended. Its fact's
        current versions end when it rests on the owner's words now, else only those the owner didn't state; the
        named items end only on the owner's words (`save`). An item already active ends nothing here."""
        it = self.get(item_id)
        if it is None or it.status == "active":
            return []
        now = self.clock.now().isoformat()
        prev = [o for o in self._versions(it.kind, it.subject, it.key) if o.id != it.id]
        closes = [o for o in prev if owner_now or o.origin != "owner_stated"]
        closes += [o for o in named if o.id not in {c.id for c in closes}]
        for o in closes:
            self.db.conn.execute("UPDATE memory_items SET valid_to = ? WHERE id = ?", (now, o.id))
        same_fact = [o for o in closes if (o.kind, o.subject, o.key) == (it.kind, it.subject, it.key)]
        self.db.conn.execute("UPDATE memory_items SET status = 'active', valid_from = ?, supersedes = ? WHERE id = ?",
                             (now, (same_fact or closes)[-1].id if closes else None, item_id))
        self.publish("memory.committed", {"id": item_id, "reason": reason})
        if closes:
            self.on_ended([o.id for o in closes], item_id)
        saved = self.get(item_id)
        assert saved is not None
        self.on_saved(saved)
        self.on_change()
        return closes

    def _end(self, items: list[MemoryItem], by: str) -> list[MemoryItem]:
        """End these current items now in favour of `by` (an already-current item the owner's words named)."""
        if not items:
            return []
        now = self.clock.now().isoformat()
        for o in items:
            self.db.conn.execute("UPDATE memory_items SET valid_to = ? WHERE id = ?", (now, o.id))
        self.on_ended([o.id for o in items], by)
        self.on_change()
        return items

    def set_about(self, item_id: str, about: str) -> None:
        """MEM-10: what the owner's words are about, next to them (redacted; one line)."""
        text = " ".join(self.redact(about).split())[:200]
        self.db.conn.execute("INSERT INTO memory_context (item_id, about) VALUES (?, ?) "
                             "ON CONFLICT(item_id) DO UPDATE SET about = excluded.about", (item_id, text))

    def set_review(self, item_id: str, at: datetime | None) -> None:
        """When FERB checks a period fact still holds (None: no check due)."""
        stamp = at.astimezone(self.clock.now().tzinfo).isoformat() if at is not None else None
        self.db.conn.execute("INSERT INTO memory_context (item_id, review_at) VALUES (?, ?) "
                             "ON CONFLICT(item_id) DO UPDATE SET review_at = excluded.review_at", (item_id, stamp))

    def due_for_review(self) -> list[MemoryItem]:
        """Current items whose check date has come."""
        now = self.clock.now()
        rows = self.db.conn.execute("SELECT item_id FROM memory_context WHERE review_at IS NOT NULL").fetchall()
        due = [it for r in rows if (it := self.get(r["item_id"])) is not None and it.review_at is not None
               and it.review_at <= now and it.status == "active" and current(it, now)]
        return sorted(due, key=lambda it: it.review_at or now)

    def retract(self, item_id: str, reason: str, *, never_again: bool = False) -> None:
        """Soft: kept for history, never recalled. `never_again`: its fingerprint is kept too, so distillation never
        proposes these words for this fact again."""
        it = self.get(item_id)
        if it is None:
            raise MemoryRefused(f"no item {item_id}")
        self.db.conn.execute("UPDATE memory_items SET status = 'retracted' WHERE id = ?", (item_id,))
        if never_again:
            self._never_again(it)
        self.on_ended([item_id], None)
        self.on_settled([item_id], False)
        self.on_change()

    def expire_at(self, item_id: str, at: datetime) -> None:
        """The owner's words give this item an end (L36 §6): it stops being valid at `at`, kept as history."""
        stamp = at.astimezone(self.clock.now().tzinfo).isoformat()     # the clock's offset, as other stamps have
        self.db.conn.execute("UPDATE memory_items SET valid_to = ? WHERE id = ? AND valid_to IS NULL", (stamp, item_id))
        self.publish("memory.expires", {"id": item_id, "at": stamp})
        self.on_change()

    def retire(self, item_id: str, reason: str) -> None:
        """This current item no longer holds (the owner's words, FERB's check of a stale fact, the memory page): it
        stops being valid now, kept as history (L36 §6)."""
        it, now = self.get(item_id), self.clock.now()
        if it is None or it.status != "active" or not current(it, now):
            raise MemoryRefused(f"no current item {item_id}")
        self.db.conn.execute("UPDATE memory_items SET valid_to = ? WHERE id = ?", (now.isoformat(), item_id))
        self.publish("memory.retired", {"id": item_id, "reason": reason})
        self.on_ended([item_id], None)
        self.on_change()

    def looks_over(self, item_id: str, why: str) -> str:
        """MEM-14 (owner, 2026-10-31): FERB's check found this current item over. FERB's understanding and what it
        observed are retired ("retired"); the owner's own words are only set aside ("set_aside"): no longer used,
        kept as history, restored by the owner saying it still holds (`restore`). Their end date, when they gave one,
        ends them by itself (`expire_at`). The caller tells the owner once."""
        it, now = self.get(item_id), self.clock.now()
        if it is None or it.status != "active" or not current(it, now):
            raise MemoryRefused(f"no current item {item_id}")
        if it.origin != "owner_stated":
            self.retire(item_id, why)
            return "retired"
        self.db.conn.execute("INSERT OR REPLACE INTO memory_set_aside (item_id, at, why) VALUES (?, ?, ?)",
                             (item_id, now.isoformat(), " ".join(self.redact(why).split())[:200]))
        self.publish("memory.set_aside", {"id": item_id, "reason": why})
        self.on_set_aside([item_id])
        self.on_change()
        return "set_aside"

    def restore(self, item_id: str) -> bool:
        """The owner says a fact FERB set aside still holds (MEM-14): it is used again. False when it wasn't set
        aside."""
        if self.db.conn.execute("DELETE FROM memory_set_aside WHERE item_id = ?", (item_id,)).rowcount == 0:
            return False
        self.publish("memory.restored", {"id": item_id})
        it = self.get(item_id)
        if it is not None:
            self.on_saved(it)
        self.on_change()
        return True

    def _never_again(self, it: MemoryItem) -> None:
        self.db.conn.execute("INSERT OR IGNORE INTO memory_forgotten (fingerprint, at) VALUES (?, ?)",
                             (fingerprint(it.kind, it.subject, it.key, it.content), self.clock.now().isoformat()))

    def edit(self, item_id: str, content: str, source: Source) -> MemoryItem:
        """The owner's correction: a new owner-stated version (the old one is kept as history)."""
        it = self.get(item_id)
        if it is None:
            raise MemoryRefused(f"no item {item_id}")
        return self.save(it.kind, it.subject, it.key, content, "owner_stated", [source], 1.0,
                         replaces=[item_id], about=it.about).item

    # ---------- forget = purge ----------

    def plan_forget(self, item_id: str, keep_sources: bool = False) -> ForgetPlan:
        """What forgetting this item deletes: its versions and, unless `keep_sources`, the trail rows they came from.
        Never another item (a fact stated in the same message stays) and never the owner's chat messages."""
        it = self.get(item_id)
        if it is None:
            raise MemoryRefused(f"no item {item_id}")
        items = sorted({v.id for v in self.history(item_id)} | {item_id})
        marks = ",".join("?" * len(items))
        sources = self.db.conn.execute(f"SELECT DISTINCT kind, source_id FROM memory_sources WHERE item_id IN "
                                       f"({marks})", items).fetchall()
        rows = {s["source_id"] for s in sources if s["kind"] == "trail"}
        if rows:                                  # a row another fact still stands on stays, with that fact (owner rule)
            shared = self.db.conn.execute(
                f"SELECT DISTINCT source_id FROM memory_sources WHERE kind = 'trail' AND item_id NOT IN ({marks}) "
                f"AND source_id IN ({','.join('?' * len(rows))})", [*items, *sorted(rows)]).fetchall()
            rows -= {r["source_id"] for r in shared}
        plan = ForgetPlan(items, [] if keep_sources else sorted(rows),
                          kept=sum(1 for s in sources if s["kind"] == "journal"))
        if plan.trail:
            summarized = self._summary_days()
            plan.rebuild_days = [d for d in sorted({r["day"] for r in self._trail_rows(plan.trail)}) if d in summarized]
        plan.destinations = self._named_destinations(items)
        return plan

    def _named_destinations(self, items: list[str]) -> list[tuple[str, str]]:
        """MEM-11: the known destinations these items name (as stored values: whole words of what they say, their
        label, or their subject's name; a site by its host or its name, "example" for www.example.com; a phone number by
        its digits). Structural matching of stored values, never a judgment of intent; one matched too eagerly only
        makes the next send ask again."""
        marks = ",".join("?" * len(items))
        texts = []
        for r in self.db.conn.execute(
                f"SELECT m.subject, m.key, m.content, c.about FROM memory_items m LEFT JOIN memory_context c "
                f"ON c.item_id = m.id WHERE m.id IN ({marks})", items):
            name = str(r["subject"]).partition(":")[2].replace("_", " ").replace("-", " ")
            texts.append("\n".join(part for part in (str(r["content"]), str(r["about"] or ""), name,
                                                    str(r["key"]).replace("_", " ")) if part.strip()))
        text = "\n".join(texts)
        digits = re.sub(r"[\s\-().+]", "", text)
        found: list[tuple[str, str]] = []
        for r in self.db.conn.execute("SELECT kind, value FROM known_destinations ORDER BY at").fetchall():
            kind, value = str(r["kind"]), str(r["value"])
            if value.startswith("sha256:") or not value:
                continue
            if kind == "phone":
                hit = len(value) >= 6 and value in digits
            elif kind == "host":
                bare = value.removeprefix("www.")
                labels = bare.split(".")
                site = labels[-2] if len(labels) >= 2 else bare
                hit = any(contains_words(text, v) for v in {value, bare} | ({site} if len(site) >= 4 else set()))
            else:
                hit = contains_words(text, value) or any(
                    len(n) >= 4 and contains_words(value, n) for n in
                    (str(x["subject"]).partition(":")[2].replace("_", " ") for x in self.db.conn.execute(
                        f"SELECT subject FROM memory_items WHERE id IN ({marks}) AND subject LIKE 'person:%'", items)))
            if hit:
                found.append((kind, value))
        return found

    def _summary_days(self) -> set[str]:
        return {str(json.loads(r["data_json"]).get("day")) for r in self.db.conn.execute(
            "SELECT data_json FROM journal WHERE kind = 'trail.day_summary'")}

    def forget(self, item_id: str, keep_sources: bool = False) -> ForgetPlan:
        """Delete the item, its versions, their source links and (unless `keep_sources`) the trail rows they came from,
        in one transaction; write each version's fingerprint so it is never learned again; rebuild the summaries of
        the days whose rows went. `on_forget` (the daemon: the item's search index nodes and the copies of those rows)
        runs first; then FERB's questions about it close (`on_settled`) and the export is written again
        (`on_change`). Synchronous and small: nothing waits for a scrub."""
        plan = self.plan_forget(item_id, keep_sources)
        self.on_forget(plan)
        now = self.clock.now().isoformat()
        c = self.db.conn
        marks = ",".join("?" * len(plan.items))
        prints = [fingerprint(r["kind"], r["subject"], r["key"], r["content"]) for r in c.execute(
            f"SELECT kind, subject, key, content FROM memory_items WHERE id IN ({marks})", plan.items)]
        c.execute("BEGIN")
        try:
            c.execute(f"DELETE FROM memory_sources WHERE item_id IN ({marks})", plan.items)
            c.execute(f"DELETE FROM memory_context WHERE item_id IN ({marks})", plan.items)
            c.execute(f"DELETE FROM memory_profile WHERE item_id IN ({marks})", plan.items)
            c.execute(f"DELETE FROM memory_set_aside WHERE item_id IN ({marks})", plan.items)
            c.execute(f"DELETE FROM memory_items WHERE id IN ({marks})", plan.items)
            for kind, value in plan.destinations:          # MEM-11: a send there asks again
                c.execute("DELETE FROM known_destinations WHERE kind = ? AND value = ?", (kind, value))
            for tid in plan.trail:
                c.execute("DELETE FROM trail WHERE id = ?", (tid,))
                # another item that also cited the row keeps no link to a row that is gone
                c.execute("DELETE FROM memory_sources WHERE kind = 'trail' AND source_id = ?", (tid,))
            for day in plan.rebuild_days:
                if not self._has_raw(day):
                    plan.delete_summaries.append(day)
            for day in plan.delete_summaries:
                c.execute("DELETE FROM journal WHERE kind = 'trail.day_summary' AND json_extract(data_json, '$.day') = ?",
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
        self.db.truncate_wal("memory.forget")                         # no copy left behind in the WAL
        self.on_settled(plan.items, True)
        self.on_change()
        return plan

    def _has_raw(self, day: str) -> bool:
        return self.db.conn.execute("SELECT 1 FROM trail WHERE day = ? LIMIT 1", (day,)).fetchone() is not None
