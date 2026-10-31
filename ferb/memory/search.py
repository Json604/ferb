"""The one fused memory search (notes/memory.md decision 8, target design 2; step 9b). Must not grant authority to what
it finds, read a whole store to answer one query, or let a prompt grow with memory.

One search serves what FERB remembers, each kind from its own store, ranked the same way:
- facts: `memory_items` directly (its `memory_fts` keyword index; no copy anywhere else), what may shape prompts only
  (`prompt_ready`; a fact the owner set aside is not, decision 17);
- learned nodes: episode cards with their how-notes, playbooks, day summaries (`Learning.search`, NODE_KINDS);
- old chat: the owner's own words of each message and FERB's replies, answers, questions and notices (`chat_fts`,
  filled from the journal as it grows, `index_chat`).
Each kind is ranked by bm25 over the words that carry weight (`items.weighted`: data-driven, no word list), the task's
target words ranked alone (`focus`), and the local embedding model's nearest (embeddings.py, above MIN_SIMILARITY),
fused by reciprocal rank, plus a small recency nudge (the owner's words don't age). A query whose every word most
documents share finds nothing, by similarity too.

What each role is given is bounded by one set of limits (`PLANNER`, `WORKER`; Ruling under decision 7): prompts
don't grow with memory; what is left out is counted in one line and stays findable with the lookup tools.

Embeddings are computed in the background (`embed_pending`, a batch at a time on a connection of its own), never in
a turn; a query's own embedding is one table lookup per token."""
from __future__ import annotations

import json
import math
import re
from dataclasses import dataclass
from datetime import datetime
from typing import Any, Callable, Iterable, Iterator

from ferb.memory import embeddings
from ferb.memory.items import SQL_SLACK, prompt_ready, query_terms, weighted

# Learned node kinds search serves (fact and identity nodes are no longer made: facts are searched in memory_items)
NODE_KINDS = ("episode", "playbook", "activity_summary", "episode_annotation")
# The conversation kinds the chat index holds (supervisor.CONVERSATION_KINDS)
CHAT_KINDS = ("chat.message", "chat.reply", "chat.clarify", "task.answer", "chat.notice")
# Ruling (step 9b, measured on potion-base-8M): cosine below this is not "about the same thing" (related pairs from
# the owner's real chats scored 0.21-0.47, unrelated -0.09-0.14)
MIN_SIMILARITY = 0.2
NEAREST = 100                       # how many nearest vectors a ranking takes per kind
KEYWORD_MOST = 1000                 # how many keyword matches a ranking takes (rank 1000 adds 0.001)
RECENCY_WEIGHT = 0.05               # as Learning.search: recency reorders near-equal matches only
RECENCY_HALF_LIFE_DAYS = 30
EMBED_BATCH = 64                    # vectors written per transaction by the background embedder
CHAT_INDEX_BATCH = 500              # journal rows read per step of `index_chat`
CHARS_PER_TOKEN = 4                 # as FERB's context counts text


@dataclass(frozen=True)
class Limits:
    """What one role is given of retrieved memory each time (Ruling under decision 7, step 9b, coordinator 2026-10-31):
    facts and learned items together within `memory_tokens` and at most `memory_items`; old chat exchanges within
    `chat_tokens` and at most `chat_exchanges`; today's activity within `activity_tokens`."""
    memory_tokens: int
    memory_items: int
    chat_tokens: int
    chat_exchanges: int
    activity_tokens: int


PLANNER = Limits(memory_tokens=4000, memory_items=20, chat_tokens=2000, chat_exchanges=5, activity_tokens=2000)
# Workers look up more with memory.read, memory.learned, chat.history and trail.query. Today's activity is one search
# shared by both roles (`Daemon._relevant_context`), so its limit is the same.
WORKER = Limits(memory_tokens=2000, memory_items=10, chat_tokens=0, chat_exchanges=0, activity_tokens=2000)


def tokens(text: str) -> int:
    return (len(text) + CHARS_PER_TOKEN - 1) // CHARS_PER_TOKEN


@dataclass
class Hit:
    kind: str            # 'fact', a learned node kind, or 'chat'
    doc: str             # memory:<id>, a node id, or journal:<id>
    score: float
    at: str
    owner: bool          # rests on the owner's own words
    data: Any            # the MemoryItem, the node dict, or the chat row


def _tied(rows: Iterable[tuple[str, float]], *, positional: bool = False) -> dict[str, int]:
    out: dict[str, int] = {}
    rank, last = -1, None
    for position, (ident, score) in enumerate(rows):
        if score != last:
            rank, last = (position if positional else rank + 1), score
        out.setdefault(ident, rank)
    return out


def _match(terms: list[str]) -> str:
    return " OR ".join('"' + t.replace('"', '""') + '"' for t in terms)


def _age_days(at: str, now: datetime) -> float:
    try:
        when = datetime.fromisoformat(at)
    except ValueError:
        return 0.0
    if (when.tzinfo is None) != (now.tzinfo is None):
        when = when.replace(tzinfo=now.tzinfo)
    return max(0.0, (now - when).total_seconds() / 86400)


def _recency(at: str, now: datetime, owner: bool) -> float:
    age = 0.0 if owner else _age_days(at, now)
    return RECENCY_WEIGHT * math.pow(0.5, age / RECENCY_HALF_LIFE_DAYS)


def fused(rankings: list[dict[str, int]]) -> dict[str, float]:
    scores: dict[str, float] = {}
    for ranking in rankings:
        for ident, n in ranking.items():
            scores[ident] = scores.get(ident, 0.0) + 1 / (n + 1)
    return scores


def own_words(text: str) -> str:
    """A message's own words: the parts the owner pasted (untrusted) left out."""
    return re.sub(r'<untrusted-data id="([^"]+)">.*?</untrusted-data id="\1">', " ", text, flags=re.S)


def chat_text(kind: str, data: dict[str, Any]) -> str:
    """What the conversation index holds of an entry: the owner's own words of a message; FERB's text otherwise (a
    notice with the texts it quotes)."""
    text = str(data.get("text") or "")
    if kind == "chat.message":
        return " ".join(own_words(text).split())
    if kind == "chat.notice":
        quotes = [str(q.get("text", "")) if isinstance(q, dict) else str(q) for q in data.get("quotes") or []]
        return " ".join(" ".join([text, *quotes]).split())
    return " ".join(text.split())


def short(text: str, chars: int = 300) -> str:
    """FERB's short form of a reply or report: its first paragraph, clipped."""
    first = text.strip().split("\n\n", 1)[0].strip()
    first = " ".join(first.split())
    return first if len(first) <= chars else first[:chars - 1].rstrip() + "…"


def fact_text(content: str, about: str, key: str, subject: str) -> str:
    """What the embedding of a fact is made from: its words, what they are about, and its key's and subject's words."""
    def words(slug: str) -> str:
        return " ".join(query_terms(slug.split(":", 1)[1] if ":" in slug else slug))
    parts = [content, about or "", words(key)] + ([words(subject)] if subject != "owner" else [])
    return " ".join(" ".join(p.split()) for p in parts if p and p.strip())


class MemorySearch:
    """The search over one database connection (the daemon's, or a worker thread's own). `model`: the name of the
    embedding model whose vectors to rank by (None: keywords and recency only)."""

    def __init__(self, db: Any, clock: Any, model: str | None = None) -> None:
        self.db, self.clock, self.model = db, clock, model

    # ---------- facts ----------

    def _current_facts(self) -> str:
        return ("m.status IN ('active','proposed') AND (m.valid_to IS NULL OR m.valid_to > ?) "
                "AND m.id NOT IN (SELECT item_id FROM memory_set_aside)")

    def _fact_terms(self, words: list[str], now: datetime) -> list[str]:
        slack = (now - SQL_SLACK).isoformat()
        total = int(self.db.conn.execute(f"SELECT COUNT(*) FROM memory_items m WHERE {self._current_facts()}",
                                         (slack,)).fetchone()[0])

        def matches(term: str) -> int:
            return int(self.db.conn.execute(
                f"SELECT COUNT(*) FROM memory_fts f JOIN memory_items m ON m.seq = f.rowid WHERE memory_fts MATCH ? "
                f"AND {self._current_facts()}", (_match([term]), slack)).fetchone()[0])
        return weighted(words, total, matches)

    def _fact_bm25(self, terms: list[str], now: datetime, *, positional: bool = False) -> dict[str, int]:
        rows = self.db.conn.execute(
            f"SELECT m.id, bm25(memory_fts) FROM memory_fts JOIN memory_items m ON m.seq = memory_fts.rowid "
            f"WHERE memory_fts MATCH ? AND {self._current_facts()} ORDER BY 2 LIMIT {KEYWORD_MOST}",
            (_match(terms), (now - SQL_SLACK).isoformat())).fetchall()
        return _tied(((str(r[0]), float(r[1])) for r in rows), positional=positional)

    def _vector_ranks(self, vector: list[int] | None, kinds: Iterable[str], prefix: str = "") -> dict[str, int]:
        if vector is None or self.model is None:
            return {}
        near = [(doc, score) for doc, score in embeddings.nearest(self.db, self.model, vector, kinds, NEAREST)
                if score >= MIN_SIMILARITY]
        return {doc.removeprefix(prefix): n for n, (doc, _) in enumerate(near)}

    def facts(self, memory: Any, query: str, *, vector: list[int] | None = None, focus: str = "",
              limit: int | None = None, leave_out: Iterable[str] = ()) -> list[Hit]:
        """The facts that may shape prompts, best first. `leave_out`: item ids not to return (the core profile's,
        already in every prompt), dropped before `limit`."""
        now = self.clock.now()
        words = query_terms(query)
        terms = self._fact_terms(words, now) if words else []
        if words and not terms:
            return []
        rankings: list[dict[str, int]] = []
        if terms:
            rankings.append(self._fact_bm25(terms, now))
            focused = self._fact_terms(query_terms(focus), now) if focus else []
            if focused:
                rankings.append(self._fact_bm25(focused, now, positional=True))
            near = self._vector_ranks(vector, ("fact",), "memory:")
            if near:
                rankings.append(near)
        else:                                   # no words: the current facts, newest first
            rows = self.db.conn.execute(f"SELECT m.id FROM memory_items m WHERE {self._current_facts()} "
                                        "ORDER BY m.valid_from DESC, m.seq DESC", ((now - SQL_SLACK).isoformat(),))
            rankings.append({str(r[0]): n for n, r in enumerate(rows)})
        scores = fused(rankings)
        skip = set(leave_out)
        order = []
        for ident, score in scores.items():
            if ident in skip:
                continue
            row = self.db.conn.execute("SELECT origin, valid_from FROM memory_items WHERE id = ?", (ident,)).fetchone()
            if row is None:
                continue
            owner = row[0] == "owner_stated"
            boost = _recency(str(row[1]), now, owner) if words else 0.0
            order.append((round(score + boost, 9), bool(words) and owner, str(row[1]), ident, score + boost))
        order.sort(reverse=True)
        out: list[Hit] = []
        for *_, ident, score in order:
            if limit is not None and len(out) >= limit:
                break
            item = memory.get(ident)
            if item is None or not prompt_ready(item, now) or getattr(item, "set_aside", False):
                continue
            out.append(Hit("fact", "memory:" + ident, score, item.valid_from.isoformat(),
                           item.origin == "owner_stated", item))
        return out

    # ---------- old chat ----------

    def index_chat(self, stop: Callable[[], bool] = lambda: False) -> int:
        """Index the conversation entries journaled since the last call (cheap: only new rows). Returns how many."""
        done = 0
        marks = ",".join("?" * len(CHAT_KINDS))
        while not stop():
            if not self.db.conn.execute(        # nothing new: no write lock taken (the common case, every search)
                    f"SELECT 1 FROM journal WHERE seq > COALESCE((SELECT CAST(value AS INTEGER) FROM recall_state "
                    f"WHERE name = 'chat_seq'), 0) AND kind IN ({marks}) LIMIT 1", CHAT_KINDS).fetchone():
                break
            own = not self.db.conn.in_transaction
            if own:                             # the watermark is read inside the write: two indexers never overlap
                self.db.conn.execute("BEGIN IMMEDIATE")
            try:
                row = self.db.conn.execute("SELECT value FROM recall_state WHERE name = 'chat_seq'").fetchone()
                after = int(row[0]) if row else 0
                rows = self.db.conn.execute(
                    f"SELECT seq, id, at, kind, data_json FROM journal WHERE seq > ? AND kind IN ({marks}) ORDER BY "
                    f"seq LIMIT ?", (after, *CHAT_KINDS, CHAT_INDEX_BATCH)).fetchall()
                for r in rows:
                    data = json.loads(r["data_json"])
                    text = chat_text(str(r["kind"]), data)
                    if not text:
                        continue
                    session = "" if r["kind"] == "task.answer" else str(data.get("session_id") or (
                        "" if r["kind"] == "chat.notice" else "owner"))
                    request = str(data.get("read_with") or data.get("request_id") or data.get("task_id") or "")
                    self.db.conn.execute("INSERT INTO chat_fts (doc, kind, session, at, request, text) VALUES "
                                         "(?,?,?,?,?,?)", ("journal:" + str(r["id"]), r["kind"], session, r["at"],
                                                           request, text))
                if rows:
                    self.db.conn.execute("INSERT OR REPLACE INTO recall_state VALUES ('chat_seq', ?)",
                                         (str(rows[-1]["seq"]),))
                if own:
                    self.db.conn.execute("COMMIT")
            except BaseException:
                if own and self.db.conn.in_transaction:
                    self.db.conn.execute("ROLLBACK")
                raise
            done += len(rows)
            if len(rows) < CHAT_INDEX_BATCH:
                break
        return done

    def _chat_terms(self, words: list[str]) -> list[str]:
        total = int(self.db.conn.execute("SELECT COUNT(*) FROM chat_fts").fetchone()[0])

        def matches(term: str) -> int:
            return int(self.db.conn.execute("SELECT COUNT(*) FROM chat_fts WHERE chat_fts MATCH ?",
                                            (_match([term]),)).fetchone()[0])
        return weighted(words, total, matches)

    def chat(self, query: str, *, vector: list[int] | None = None, limit: int = 20,
             kinds: Iterable[str] = CHAT_KINDS, accept: Callable[[dict[str, Any]], bool] = lambda row: True,
             exclude: Iterable[str] = (), since: str | None = None, until: str | None = None) -> list[Hit]:
        """Conversation entries matching `query`, best first: rows {doc, kind, session, at, request, text}. `accept`:
        which rows count (the caller's chat); `exclude`: docs already shown; `since`/`until`: ISO bounds on `at`."""
        self.index_chat()
        now = self.clock.now()
        words = query_terms(query)
        terms = self._chat_terms(words) if words else []
        if not terms:
            return []
        rankings = [_tied((str(r[0]), float(r[1])) for r in self.db.conn.execute(
            f"SELECT doc, bm25(chat_fts) FROM chat_fts WHERE chat_fts MATCH ? ORDER BY 2 LIMIT {KEYWORD_MOST}",
            (_match(terms),)))]
        near = self._vector_ranks(vector, ("chat",))
        if near:
            rankings.append(near)
        scores = fused(rankings)
        wanted, skip = set(kinds), set(exclude)
        rows: dict[str, dict[str, Any]] = {}
        ids = sorted(scores)
        for at in range(0, len(ids), 500):
            chunk = ids[at:at + 500]
            for r in self.db.conn.execute(f"SELECT doc, kind, session, at, request, text FROM chat_fts WHERE doc IN "
                                          f"({','.join('?' * len(chunk))})", chunk):
                rows[str(r[0])] = dict(zip(("doc", "kind", "session", "at", "request", "text"), r))
        order = []
        for doc, score in scores.items():
            row = rows.get(doc)
            if row is None or doc in skip or row["kind"] not in wanted:
                continue
            if since and str(row["at"]) < since or until and str(row["at"]) > until:
                continue
            if not accept(row):
                continue
            owner = row["kind"] == "chat.message"
            total = score + _recency(str(row["at"]), now, False)
            order.append((round(total, 9), str(row["at"]), doc, total, owner, row))
        order.sort(reverse=True)
        return [Hit("chat", doc, total, str(row["at"]), owner, row)
                for _, _, doc, total, owner, row in order[:limit]]

    # ---------- the background embedder ----------

    def _pending(self) -> Iterator[tuple[str, str, str, str]]:
        """(doc, kind, at, text) of what has no vector yet for this model, a bounded batch at a time."""
        assert self.model is not None
        now = self.clock.now()
        from ferb.memory.learning import search_text
        rows = self.db.conn.execute(
            f"SELECT m.id, m.content, m.subject, m.key, m.valid_from, COALESCE(c.about, '') AS about FROM memory_items m "
            f"LEFT JOIN memory_context c ON c.item_id = m.id WHERE {self._current_facts()} AND NOT EXISTS "
            f"(SELECT 1 FROM recall_vectors v WHERE v.doc = 'memory:' || m.id AND v.model = ?) LIMIT ?",
            ((now - SQL_SLACK).isoformat(), self.model, EMBED_BATCH)).fetchall()
        for r in rows:
            yield "memory:" + str(r["id"]), "fact", str(r["valid_from"]), fact_text(
                str(r["content"]), str(r["about"]), str(r["key"]), str(r["subject"]))
        marks = ",".join("?" * len(NODE_KINDS))
        for r in self.db.conn.execute(
                f"SELECT n.id, n.kind, n.key, n.body_json, n.recorded_at FROM learning_nodes n WHERE n.kind IN ({marks}) "
                f"AND n.valid_to IS NULL AND n.state NOT IN ('retired','rolled_back') AND NOT EXISTS (SELECT 1 FROM "
                f"recall_vectors v WHERE v.doc = n.id AND v.model = ?) LIMIT ?",
                (*NODE_KINDS, self.model, EMBED_BATCH)).fetchall():
            yield str(r["id"]), str(r["kind"]), str(r["recorded_at"]), search_text(r["kind"], r["key"],
                                                                                   json.loads(r["body_json"]))
        mark = self.db.conn.execute("SELECT value FROM recall_state WHERE name = ?",
                                    ("chat_vec:" + self.model,)).fetchone()
        for r in self.db.conn.execute("SELECT rowid, doc, at, text FROM chat_fts WHERE rowid > ? ORDER BY rowid "
                                      "LIMIT ?", (int(mark[0]) if mark else 0, EMBED_BATCH)).fetchall():
            yield str(r["doc"]), "chat", str(r["at"]), str(r["text"]) + "\x00" + str(r["rowid"])

    def embed_pending(self, embedder: Any, *, stop: Callable[[], bool] = lambda: False,
                      pause: Callable[[], None] = lambda: None) -> dict[str, Any]:
        """Embed what has no vector yet, a batch per transaction (`pause` between batches lets the machine breathe),
        and drop vectors whose fact or node no longer counts. Returns counts and the time spent encoding."""
        import time
        self.model = embedder.name
        done, encode_s, batches = 0, 0.0, 0
        self.index_chat(stop)
        while not stop():
            batch = list(self._pending())
            if not batch:
                break
            chat_mark = None
            written = []
            for doc, kind, at, text in batch:
                if kind == "chat":
                    text, _, rowid = text.rpartition("\x00")
                    chat_mark = int(rowid)
                started = time.process_time()
                vector = embedder.encode(text) if text.strip() else None
                encode_s += time.process_time() - started
                written.append((doc, kind, at, embeddings.text_hash(text), vector))
            self.db.conn.execute("BEGIN IMMEDIATE")
            try:
                for doc, kind, at, digest, vector in written:
                    if vector is None:          # nothing the model knows: a zero-information marker keeps it done
                        vector = [0] * embedder.dimensions
                    embeddings.store(self.db.conn, doc, kind, at, embedder.name, digest, vector)
                if chat_mark is not None:
                    self.db.conn.execute("INSERT OR REPLACE INTO recall_state VALUES (?, ?)",
                                         ("chat_vec:" + embedder.name, str(chat_mark)))
                self.db.conn.execute("COMMIT")
            except BaseException:
                if self.db.conn.in_transaction:
                    self.db.conn.execute("ROLLBACK")
                raise
            done += len(written)
            batches += 1
            pause()
        removed = self.drop_stale(embedder.name)
        return {"embedded": done, "batches": batches, "encode_cpu_s": round(encode_s, 4), "removed": removed}

    def drop_stale(self, model: str) -> int:
        """Vectors of facts that no longer count (ended, retracted, set aside) and of closed nodes go."""
        cutoff = (self.clock.now() - SQL_SLACK).isoformat()
        marks = ",".join("?" * len(NODE_KINDS))
        n = self.db.conn.execute(
            "DELETE FROM recall_vectors WHERE model = ? AND kind = 'fact' AND substr(doc, 8) IN (SELECT id FROM "
            "memory_items WHERE status NOT IN ('active','proposed') OR (valid_to IS NOT NULL AND valid_to < ?) "
            "UNION SELECT item_id FROM memory_set_aside)", (model, cutoff)).rowcount
        n += self.db.conn.execute(
            f"DELETE FROM recall_vectors WHERE model = ? AND kind IN ({marks}) AND doc IN (SELECT id FROM "
            f"learning_nodes WHERE valid_to IS NOT NULL OR state IN ('retired','rolled_back'))",
            (model, *NODE_KINDS)).rowcount
        return int(n)

    def pending(self, model: str) -> bool:
        """Whether anything waits for a vector (one cheap query per kind)."""
        self.model = model
        return next(self._pending(), None) is not None or bool(self.db.conn.execute(
            "SELECT 1 FROM journal WHERE seq > COALESCE((SELECT CAST(value AS INTEGER) FROM recall_state WHERE "
            f"name = 'chat_seq'), 0) AND kind IN ({','.join('?' * len(CHAT_KINDS))}) LIMIT 1", CHAT_KINDS).fetchone())


def within(lines: list[str], budget_tokens: int, most: int) -> tuple[list[str], int]:
    """The leading lines that fit `budget_tokens` and `most`; and how many were left out. Never cuts a line."""
    shown: list[str] = []
    used = 0
    for line in lines:
        cost = tokens(line) + 1
        if len(shown) >= most or used + cost > budget_tokens:
            continue
        shown.append(line)
        used += cost
    return shown, len(lines) - len(shown)
