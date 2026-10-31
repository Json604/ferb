"""Owns turning imported conversations into memory facts (notes/memory-import.md, Shape 2-5). Must not let anything but
the owner's own messages state a fact about them, save an imported fact as the owner's words, add a known destination,
or keep the conversations themselves.

1. **Parts and chunks**: each conversation (already redacted, import_sources.py) is rendered as lines with a stable
   reference per message (`c1a2b3c.4`: the conversation's hash, the message's place); a long one is split into parts on
   message boundaries, and parts are packed into chunks of about `CHUNK_CHARS`. Progress is kept per part (its hash
   only), so a restart or a later import of the same export skips what is done.
2. **Recall** (one model call a chunk, import_recall.md): candidate facts, preferences, projects, people and recurring
   tasks, each with the message it rests on and the exact words quoted. Code keeps only candidates whose message is the
   owner's (or an assistant's saved memory, a claim) and whose quote is whole words of that message.
3. **Verification** (one model call a chunk, import_verify.md): each candidate with its evidence — the quoted message
   and its neighbours, or for a claim the owner's messages of that source sharing its words — and the facts FERB
   already holds that look alike. The verifier keeps what the owner's words support, may tighten the wording, names the
   owner message and exact quote it rests on, and says which held fact it is the same fact as (`same_as`), when it
   ended or is due a check. Code checks the quote again against that owner message.
4. **Merge and save**: candidates naming one fact share its kind, subject and key (a held fact's, when `same_as` names
   it); within a fact the newer words win (`save` in the order the owner said them: a newer version ends the older,
   kept as history); a fact FERB holds in the owner's own words, or holds as of a later date, is left as it is. Every
   fact is saved as FERB's understanding (`inferred`, never `owner_stated`: decision 2), its label "imported from
   <assistant>", resting on the run's journal row; it joins the core profile only once it holds up (decision 6)."""

import hashlib
import json
import math
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from datetime import date, datetime, timedelta
from typing import Any

from ferb.memory.import_sources import SOURCES, Conversation, Message
from ferb.memory.items import (
    KINDS,
    Memory,
    MemoryRefused,
    query_terms,
    quoted,
    slug,
    subject_key,
)

CHUNK_CHARS = 24_000                   # one recall call's conversations (~6k tokens)
EVIDENCE_PER_CLAIM = 3                 # the owner's messages shown with a claim
SIMILAR_PER_CANDIDATE = 3              # held facts looked up per candidate
SIMILAR_MOST = 30                      # held facts shown to one verification call
REVIEW_DAYS_MOST = 3650

RECALL_SCHEMA: dict[str, Any] = {
    "type": "object", "additionalProperties": False, "required": ["candidates"],
    "properties": {"candidates": {"type": "array", "items": {
        "type": "object", "additionalProperties": False,
        "required": ["kind", "subject", "key", "content", "message", "quote"],
        "properties": {"kind": {"type": "string", "enum": list(KINDS)}, "subject": {"type": "string"},
                       "key": {"type": "string"}, "content": {"type": "string"}, "message": {"type": "string"},
                       "quote": {"type": "string"}}}}}}
VERIFY_SCHEMA: dict[str, Any] = {
    "type": "object", "additionalProperties": False, "required": ["verdicts"],
    "properties": {"verdicts": {"type": "array", "items": {
        "type": "object", "additionalProperties": False,
        "required": ["id", "supported", "kind", "subject", "key", "content", "message", "quote", "same_as", "ended",
                     "until", "review_in_days"],
        "properties": {"id": {"type": "string"}, "supported": {"type": "boolean"},
                       "kind": {"type": "string", "enum": list(KINDS)}, "subject": {"type": "string"},
                       "key": {"type": "string"}, "content": {"type": "string"}, "message": {"type": "string"},
                       "quote": {"type": "string"}, "same_as": {"type": "string"}, "ended": {"type": "boolean"},
                       "until": {"type": "string"}, "review_in_days": {"type": "integer"}}}}}}


def digest(*parts: str) -> str:
    return hashlib.sha256("\x1f".join(parts).encode()).hexdigest()


@dataclass
class Part:
    """One conversation, or a slice of a long one, as the model reads it."""
    key: str                                   # its hash: what progress keeps
    conversation: Conversation
    lines: list[str]
    refs: dict[str, Message]                   # reference -> message

    @property
    def chars(self) -> int:
        return sum(len(line) + 1 for line in self.lines)


@dataclass
class Chunk:
    source: str
    parts: list[Part]

    @property
    def text(self) -> str:
        return f"Source: {SOURCES[self.source]}\n\n" + "\n\n".join("\n".join(p.lines) for p in self.parts)

    @property
    def refs(self) -> dict[str, tuple[Message, Conversation]]:
        return {ref: (m, p.conversation) for p in self.parts for ref, m in p.refs.items()}


def _tag(conv: Conversation) -> str:
    return "c" + digest(conv.source, conv.id)[:6]


def _line(ref: str, m: Message, claims: bool) -> str:
    who = "saved memory (the assistant's claim)" if claims else m.role
    when = m.at.astimezone().strftime("%Y-%m-%d") if m.at else "undated"
    return f"[{ref} · {who} · {when}] {m.text}"


def parts(conversations: list[Conversation], limit: int = CHUNK_CHARS) -> list[Part]:
    """Every conversation as parts within `limit` characters, cut on message boundaries."""
    out: list[Part] = []
    for conv in conversations:
        tag = _tag(conv)
        head = ("Saved memory" if conv.claims else "Conversation") + f" {tag} — “{conv.title or 'untitled'}”"
        lines, refs, n = [head], dict[str, Message](), 0
        for i, m in enumerate(conv.messages, 1):
            line = _line(f"{tag}.{i}", m, conv.claims)
            if refs and sum(len(x) + 1 for x in lines) + len(line) > limit:
                out.append(Part(digest(conv.source, conv.id, str(n), *lines), conv, lines, refs))
                n += 1
                lines, refs = [head + f" (part {n + 1})"], {}
            lines.append(line)
            refs[f"{tag}.{i}"] = m
        out.append(Part(digest(conv.source, conv.id, str(n), *lines), conv, lines, refs))
    return out


def chunks(source: str, todo: list[Part], limit: int = CHUNK_CHARS) -> list[Chunk]:
    """Parts packed in order into chunks of about `limit` characters (a part over it stays whole, alone)."""
    out: list[Chunk] = []
    for part in todo:
        if out and sum(p.chars for p in out[-1].parts) + part.chars <= limit:
            out[-1].parts.append(part)
        else:
            out.append(Chunk(source, [part]))
    return out


class OwnerIndex:
    """The owner's messages of one source, for finding the ones a claim's words appear in (retrieval by shared
    words weighted by rarity; the verifier judges)."""

    def __init__(self, conversations: list[Conversation]) -> None:
        self.messages: list[tuple[str, Message, frozenset[str]]] = []
        df: dict[str, int] = {}
        for conv in conversations:
            if conv.claims:
                continue
            tag = _tag(conv)
            for i, m in enumerate(conv.messages, 1):
                if m.role == "owner":
                    terms = frozenset(query_terms(m.text))
                    self.messages.append((f"{tag}.{i}", m, terms))
                    for t in terms:
                        df[t] = df.get(t, 0) + 1
        n = max(1, len(self.messages))
        # a word in more than half of the messages says nothing about any one (`weighted`, the one search's rule)
        self.weight = {t: math.log((n + 1) / c) for t, c in df.items() if n < 2 or c <= max(2, n / 2)}

    def find(self, text: str, most: int = EVIDENCE_PER_CLAIM) -> list[tuple[str, Message]]:
        terms = set(query_terms(text))
        scored = sorted(((sum(self.weight.get(t, 0) for t in terms & m_terms), ref, m)
                         for ref, m, m_terms in self.messages if terms & m_terms), key=lambda x: -x[0])
        return [(ref, m) for score, ref, m in scored[:most] if score > 0]


@dataclass
class Candidate:
    id: str
    kind: str
    subject: str
    key: str
    content: str
    message: str                     # the reference it rests on
    quote: str
    claim: bool                      # proposed from an assistant's saved memory
    evidence: list[tuple[str, Message, str]] = field(default_factory=list)   # (ref, message, how shown)


@dataclass
class Verified:
    kind: str
    subject: str
    key: str
    content: str
    said_at: datetime
    source: str
    same_as: str = ""                # a held memory id or another candidate's id
    ended: bool = False
    until: date | None = None
    review_in_days: int = 0
    id: str = ""


def check_recall(answer: dict[str, Any], chunk: Chunk, index: OwnerIndex,
                 redact: Callable[[str], str] = lambda s: s) -> tuple[list[Candidate], list[str]]:
    """Recall's candidates that rest on the owner's own words (or a saved memory's claim), with their evidence; and why
    the others were dropped. What the model wrote is redacted again before it goes anywhere (the verifier, memory)."""
    refs, kept, dropped = chunk.refs, list[Candidate](), list[str]()
    for n, c in enumerate(answer.get("candidates") or []):
        ref = str(c.get("message", "")).strip("[] ")
        found = refs.get(ref)
        if found is None:
            dropped.append("cites no message of this chunk")
            continue
        msg, conv = found
        if msg.role != "owner" and not conv.claims:
            dropped.append("rests on an assistant's or tool's words, not the owner's")
            continue
        quote = redact(str(c.get("quote", "")))
        if not quoted(msg.text, quote):
            dropped.append("the quote isn't in that message")
            continue
        cand = Candidate(f"v{n + 1}", str(c["kind"]), redact(str(c.get("subject") or "owner")),
                         redact(str(c.get("key") or "")), redact(" ".join(str(c.get("content") or "").split())), ref,
                         quote, conv.claims)
        if not cand.content or not cand.key:
            dropped.append("empty")
            continue
        if conv.claims:
            cand.evidence = [(ref, msg, "saved memory (the assistant's claim)")] + [
                (r, m, "owner") for r, m in index.find(cand.content + " " + cand.quote)]
        else:
            ordered = list(conv.messages)
            at = ordered.index(msg)
            tag = ref.rsplit(".", 1)[0]
            for i in range(max(0, at - 1), min(len(ordered), at + 2)):
                cand.evidence.append((f"{tag}.{i + 1}", ordered[i], ordered[i].role))
        kept.append(cand)
    return kept, dropped


def verify_context(source: str, candidates: list[Candidate], held: list[Any], label: Callable[[str], str]) -> str:
    lines = [f"Source: {SOURCES[source]}", ""]
    for c in candidates:
        lines += [f"Candidate {c.id}: kind={c.kind} subject={c.subject} key={c.key}",
                  f"  says: {c.content}", f"  quoted: “{c.quote}” (from {c.message})", "  evidence:"]
        for ref, m, who in c.evidence:
            when = m.at.astimezone().strftime("%Y-%m-%d") if m.at else "undated"
            lines.append(f"    [{ref} · {who} · {when}] {m.text}")
        lines.append("")
    lines.append("What FERB already remembers that looks alike (data):")
    for it in held:
        lines.append(f"  [{it.id}] {it.kind} / {it.subject} / {it.key}: {it.content} ({label(it.origin)}, as of "
                     f"{it.valid_from.astimezone():%Y-%m-%d})")
    if not held:
        lines.append("  (nothing)")
    return "\n".join(lines)


def check_verify(answer: dict[str, Any], candidates: list[Candidate], held_ids: set[str],
                 source: str) -> tuple[list[Verified], list[str]]:
    """The verifier's supported facts that rest on an owner message shown as its evidence, whose quote is whole words of
    that message; and why the others were dropped."""
    by_id = {c.id: c for c in candidates}
    kept, dropped, seen = list[Verified](), list[str](), set[str]()
    for v in answer.get("verdicts") or []:
        c = by_id.get(str(v.get("id")))
        if c is None or c.id in seen:
            dropped.append("names no candidate")
            continue
        seen.add(c.id)
        if not v.get("supported"):
            dropped.append("not supported by the owner's words")
            continue
        owner = {ref: m for ref, m, who in c.evidence if who == "owner"}
        msg = owner.get(str(v.get("message", "")).strip("[] "))
        if msg is None or msg.at is None:
            dropped.append("rests on no owner message shown")
            continue
        if not quoted(msg.text, str(v.get("quote", ""))):
            dropped.append("the quote isn't in that owner message")
            continue
        content = " ".join(str(v.get("content") or "").split())
        if not content:
            dropped.append("empty")
            continue
        until: date | None = None
        if str(v.get("until") or "").strip():
            try:
                until = date.fromisoformat(str(v["until"]).strip()[:10])
            except ValueError:
                until = None
        same = str(v.get("same_as") or "").strip("[] ")
        kept.append(Verified(str(v.get("kind") or c.kind), str(v.get("subject") or c.subject),
                             str(v.get("key") or c.key), content, msg.at, source,
                             same if same in held_ids or same in by_id else "", bool(v.get("ended")), until,
                             max(0, min(REVIEW_DAYS_MOST, int(v.get("review_in_days") or 0))), c.id))
    return kept, dropped


@dataclass
class SaveCounts:
    saved: list[str] = field(default_factory=list)          # new items
    credited: int = 0                                         # the same fact again: more evidence, no new item
    kept_owner: int = 0                                       # the owner's own words already hold this fact
    older: int = 0                                            # FERB holds this fact as of a later date
    history: int = 0                                          # saved already over (an old job, a finished project)
    refused: list[str] = field(default_factory=list)


def said_at(memory: Memory, item: Any) -> datetime:
    """When a held fact was said: an imported one's own date, else when FERB saved it."""
    row = memory.db.conn.execute("SELECT MAX(said_at) FROM memory_import_items WHERE item_id = ?", (item.id,)).fetchone()
    return datetime.fromisoformat(row[0]) if row and row[0] else item.valid_from


def save(memory: Memory, facts: list[Verified], *, run_id: str, source_entry: str,
         now: datetime) -> SaveCounts:
    """Merge and save (step 4). Facts naming one fact share its kind, subject and key; each fact's versions are saved
    oldest first, so the newer words win and the older stay as history."""
    counts = SaveCounts()
    by_id = {f.id: f for f in facts}
    for f in facts:                                  # a held fact named: its kind, subject and key
        root, hops = f, 0
        while root.same_as in by_id and by_id[root.same_as] is not root and hops < len(facts):
            root, hops = by_id[root.same_as], hops + 1
        if root.same_as and root.same_as not in by_id:
            held = memory.get(root.same_as)
            if held is not None:
                f.kind, f.subject, f.key = held.kind, held.subject, held.key
        elif root is not f:
            f.kind, f.subject, f.key = root.kind, root.subject, root.key
    groups: dict[tuple[str, str, str], list[Verified]] = {}
    for f in facts:
        name = (f.kind, subject_key(memory.redact(f.subject)), slug(memory.redact(f.key)))
        groups.setdefault(name, []).append(f)
    about = f"imported from {SOURCES[facts[0].source]}" if facts else ""
    for (kind, subject, key), versions in groups.items():
        for f in sorted(versions, key=lambda x: x.said_at):
            current = [i for i in memory.items("active", subject=subject) if i.kind == kind and i.key == key]
            if any(i.origin == "owner_stated" for i in current):
                counts.kept_owner += 1
                continue
            if any(said_at(memory, i) > f.said_at for i in current):
                counts.older += 1
                continue
            try:
                got = memory.save(kind, subject, key, f.content, "inferred", [("journal", source_entry)], 0.6,
                                  about=about, from_owner=False)
            except (MemoryRefused, ValueError) as e:
                counts.refused.append(str(e))
                continue
            item = got.item
            fresh = item.id not in {i.id for i in current}
            over = f.ended or (f.until is not None and f.until < now.astimezone().date())
            if not fresh:
                counts.credited += 1
            elif over:
                counts.history += 1
            else:
                counts.saved.append(item.id)
            memory.db.conn.execute(
                "INSERT INTO memory_import_items (item_id, source, run_id, said_at) VALUES (?, ?, ?, ?) "
                "ON CONFLICT(item_id, source) DO UPDATE SET said_at = MAX(said_at, excluded.said_at)",
                (item.id, f.source, run_id, f.said_at.isoformat()))
            if fresh and (f.ended or f.until is not None):
                end = (datetime.combine(f.until, datetime.max.time(), tzinfo=now.astimezone().tzinfo) if f.until
                       else f.said_at)
                memory.expire_at(item.id, end)
            elif fresh and f.review_in_days:
                memory.set_review(item.id, now + timedelta(days=f.review_in_days))
    return counts


def imported_from(db: Any, item_ids: list[str]) -> dict[str, list[str]]:
    """Which assistants each item was imported from (the memory page's filter)."""
    if not item_ids:
        return {}
    out: dict[str, list[str]] = {}
    for start in range(0, len(item_ids), 500):
        batch = item_ids[start:start + 500]
        for r in db.conn.execute(f"SELECT item_id, source FROM memory_import_items WHERE item_id IN "
                                 f"({','.join('?' * len(batch))}) ORDER BY source", batch):
            out.setdefault(r["item_id"], []).append(r["source"])
    return out


async def similar_held(memory: Memory, candidates: list[Candidate],
                       similar: Callable[[str], Awaitable[list[str]]]) -> list[Any]:
    """The held facts that look like these candidates (the one fused search), current ones only, at most
    `SIMILAR_MOST`."""
    ids: list[str] = []
    for c in candidates:
        for i in (await similar(f"{c.key.replace('_', ' ')} {c.content}"))[:SIMILAR_PER_CANDIDATE]:
            if i not in ids:
                ids.append(i)
    now = memory.clock.now()
    found = [memory.get(i) for i in ids[:SIMILAR_MOST * 2]]
    return [it for it in found if it is not None and it.status == "active"
            and (it.valid_to is None or it.valid_to > now) and not it.set_aside][:SIMILAR_MOST]


def as_json(value: Any) -> dict[str, Any]:
    parsed = json.loads(value) if isinstance(value, str) else value
    if not isinstance(parsed, dict):
        raise ValueError("not an object")
    return parsed
