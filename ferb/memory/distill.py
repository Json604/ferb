"""Owns daily memory proposals and their evidence links. Must not treat retrieved observations as owner authorization or silently cut stored content.

Daily distillation: for each past day, in order —
1. the permanent summary, by code, before anything else (so a purge is never blocked on a model);
2. one model run (the worker runtime from config, no tools, typed output, unmetered): a short narrative of the day
   and proposed memory items citing evidence handles (summary.py);
3. then the retention purge, which only ever deletes days that have their summary.

Code checks every proposal before `Memory.propose` (which checks the sources again): handles must exist; an observed
item cites only the owner's own trail (E); nothing about the owner or their preferences may rest on FERB's activity
(F) or on pasted third-party text (P); a third-party item must cite pasted text. An inferred item may quote the
owner's words from one of its C handles: when chat capture's check passes on that message's typed lines, the owner's
sentence is saved as theirs (`owner_said`); every other new inferred item is asked about at once, Keep / Edit / Drop
(`ask_keep`, L36). An item with "retire" names a current item (kind, subject, key) the owner's words say no longer
holds: it is retired on a quote that passes the same check, else the owner is asked, Retire / Keep (`ask_retire`, L36
§6). Quoted words that would end an item, or replace one the owner stated, must also be newer than it and a whole
clause sharing a telling word with it (`_may_end`); otherwise a retire is asked and a new version is kept as a guess.
A model failure leaves the summary in place and is retried on later hourly runs or an explicit retry."""

import hashlib
import json
from datetime import date, datetime, timedelta
from typing import Any, Callable

from ferb.core.interfaces import Clock
from ferb.core.prompts import load_prompt
from ferb.core.types import Identity, Task
from ferb.memory import summary
from ferb.memory.calls import learning_call
from ferb.memory.items import KINDS, Memory, MemoryRefused, capture, query_terms, slug, whole_clauses
from ferb.store.db import Database

# L36 §2: the owner's own words an inferred item rests on. Ruling: the model cites one of the item's C handles (it sees
# handles, not journal ids); code maps the handle to its message.
QUOTE_SCHEMA: dict[str, Any] = {"anyOf": [{"type": "null"}, {
    "type": "object", "additionalProperties": False, "required": ["handle", "text"],
    "properties": {"handle": {"type": "string"}, "text": {"type": "string"}}}]}
KEEP_GUESS = "Keep this? “{text}” FERB guessed it from your day on {day}."
# L36 §6 (review fix round 1): why the owner's quoted words don't end or replace what FERB remembers (then it asks)
NO_QUOTE = "no quote of the owner's own words"
OLDER_WORDS = "the owner's words are older than what FERB remembers"
NOT_A_CLAUSE = "the quote isn't a whole clause of the owner's words"
NOTHING_SHARED = "the quote shares no telling word with what FERB remembers"
DISTILL_SCHEMA: dict[str, Any] = {
    "type": "object", "additionalProperties": False, "required": ["narrative", "items"],
    "properties": {
        "narrative": {"type": "string"},
        "items": {"type": "array", "items": {
            "type": "object", "additionalProperties": False,
            "required": ["kind", "subject", "key", "content", "origin", "confidence", "handles", "quote", "retire"],
            "properties": {
                "kind": {"type": "string", "enum": list(KINDS)},
                "subject": {"type": "string"}, "key": {"type": "string"}, "content": {"type": "string"},
                "origin": {"type": "string", "enum": ["observed", "inferred", "third_party"]},
                "confidence": {"type": "number"},
                "handles": {"type": "array", "items": {"type": "string"}}, "quote": QUOTE_SCHEMA,
                # L36 §6: this item says a remembered one (same kind, subject, key) no longer holds
                "retire": {"type": "boolean"}}}}}}


def check(item: dict[str, Any], ev: summary.DayEvidence) -> list[tuple[str, str]]:
    """The proposal's sources, or MemoryRefused."""
    hs = [str(h) for h in item.get("handles", [])]
    if not hs or any(h not in ev.handles for h in hs):
        raise MemoryRefused("cites evidence that doesn't exist")
    kinds = {h[0] for h in hs}
    about_owner = item["kind"] == "preference" or str(item.get("subject", "owner")).strip() in ("", "owner")
    if item["origin"] == "observed" and kinds != {"E"}:
        raise MemoryRefused("an observed item cites only the owner's own activity")
    if about_owner and "F" in kinds:
        raise MemoryRefused("FERB's own activity never becomes the owner's habits or preferences")
    if item["origin"] == "third_party" and "P" not in kinds:
        raise MemoryRefused("a third-party item must cite the third-party text")
    if item["origin"] != "third_party" and about_owner and "P" in kinds:
        raise MemoryRefused("pasted text can't become a fact about the owner")
    return [s for h in hs for s in ev.handles[h]]


def owner_said(item: dict[str, Any], ev: summary.DayEvidence) -> tuple[str, str] | None:
    """The owner's own words an inferred item rests on (L36 §2): its quote cites one of the item's own C handles, and
    chat capture's check (`capture`) finds the quote and the content in that message's typed lines. Ruling: a quote
    counts only on an inferred item. Their whole sentence(s) and the message's journal id, else None."""
    quote = item.get("quote")
    if item.get("origin") != "inferred" or not isinstance(quote, dict):
        return None
    handle = str(quote.get("handle", ""))
    if handle not in [str(h) for h in item.get("handles", [])] or handle not in ev.owner_words:
        return None
    journal_id, typed = ev.owner_words[handle]
    got = capture(typed, str(item.get("content", "")), str(quote.get("text", "")))
    return (got.content, journal_id) if got.origin == "owner_stated" else None


class Distiller:
    def __init__(self, db: Database, journal: Any, memory: Memory, make_runtime: Callable[..., Any], clock: Clock,
                 purge: Callable[[], Any] = lambda: None, device_id: str = "dev",
                 record_usage: Callable[[str, dict[str, Any]], None] = lambda runtime, data: None,
                 spans: Any = None) -> None:
        self.db, self.journal, self.memory, self.make_runtime = db, journal, memory, make_runtime
        self.clock, self.purge, self.device_id = clock, purge, device_id
        self.record_usage, self.spans = record_usage, spans   # spans: where each model call is recorded
        self.jev: Any = None                          # memory.worth_keeping: may only drop
        # L36 §2 ("Ask right away"): each new inferred proposal becomes the owner's Keep / Edit / Drop question
        self.ask_keep: Callable[[Any, str], None] = lambda item, reason: None
        # L36 §6: a current item FERB thinks is out of date, without the owner's words for it: Retire / Keep
        self.ask_retire: Callable[[Any], None] = lambda item: None

    def pending_days(self, today: date | None = None) -> list[str]:
        """Past days with anything to remember that aren't done yet (oldest first). A day whose distillation failed
        waits until its summary changes (a forget rebuilt it) or `ferb memory distill DAY` (M5): never every hour."""
        today = today or self.clock.now().astimezone().date()
        days = {r["day"] for r in self.db.conn.execute("SELECT DISTINCT day FROM trail WHERE day < ?",
                                                       (today.isoformat(),))}
        for r in self.db.conn.execute("SELECT at FROM journal WHERE kind = 'chat.message'"):
            d = datetime.fromisoformat(r["at"]).astimezone().date()
            if d < today:
                days.add(d.isoformat())
        done = {r["day"] for r in self.db.conn.execute("SELECT day FROM distill_runs WHERE status = 'distilled'")}
        failed = {r["day"]: r["inputs"] for r in self.db.conn.execute(
            "SELECT day, inputs FROM distill_runs WHERE status = 'failed'")}
        return sorted(d for d in days if (d not in done or not summary.summarized(self.db, d))
                      and not (d in failed and failed[d] is not None and failed[d] == self._summary_id(d)))

    def _summary_id(self, day: str) -> str | None:
        """The day summary a distillation reads (its journal id: a rebuilt summary has a new one)."""
        row = self.db.conn.execute("SELECT id FROM journal WHERE kind = ? AND json_extract(data_json, '$.day') = ? "
                                   "ORDER BY seq DESC LIMIT 1", (summary.SUMMARY_KIND, day)).fetchone()
        return str(row["id"]) if row else None

    def write_summary(self, day: str) -> summary.DayEvidence:
        ev = summary.build(self.db, day)
        if not summary.summarized(self.db, day):
            self.journal.append(summary.SUMMARY_KIND, ev.digest | {"made_by": summary.SUMMARY_MADE_BY})
        self._run(day, "summarized", None, bump=False)
        return ev

    async def run_day(self, day: str) -> dict[str, Any]:
        ev = self.write_summary(day)                                  # 1. always first, no model
        row = self.db.conn.execute("SELECT * FROM distill_runs WHERE day = ?", (day,)).fetchone()
        if row["status"] == "distilled":
            return {"day": day, "status": row["status"]}
        if not ev.lines:
            self._run(day, "distilled", "nothing to distill")
            return {"day": day, "status": "distilled", "proposed": 0}
        try:
            out = await self._model(day, ev)                          # 2. one model run
        except Exception as e:              # summary stays; retried once it changes, or by `ferb memory distill` (M5)
            self._run(day, "failed", f"{type(e).__name__}: {e}")
            self.db.conn.execute("UPDATE distill_runs SET inputs = ? WHERE day = ?", (self._summary_id(day), day))
            return {"day": day, "status": "failed"}
        self.journal.append(summary.NARRATIVE_KIND, {"day": day, "text": str(out.get("narrative", "")),
                                                    "made_by": str(out.get("made_by", ""))})
        proposed, refused = 0, []
        retired: list[str] = []                    # L36 §6: items retired on the owner's words
        unconfirmed: list[dict[str, str]] = []     # retires asked about instead, with why
        guessed: list[dict[str, str]] = []         # quoted words kept as a guess instead of a new version, with why
        items = out.get("items", [])
        drop: set[int] = set()
        if self.jev is not None and items:
            from ferb.decide.decider import Ask
            try:
                got = await self.jev.ask({"day": day, "items": [{"n": i, "kind": x.get("kind"), "subject": x.get("subject"),
                                                                "content": x.get("content")} for i, x in enumerate(items)]},
                                         [Ask(f"memory.worth_keeping#{i}", "yes", f"distill:{day}#{i}")
                                          for i in range(len(items))])
            except Exception:
                got = {}
            drop = {int(k.split("#")[1]) for k, v in got.items() if v == "no"}
            for i in drop:
                self.jev.mark_effect(f"memory.worth_keeping#{i}", f"distill:{day}#{i}", "dropped")
        for n, item in enumerate(items):
            if n in drop:
                refused.append("a second check found it not worth keeping")
                continue
            try:
                if item.get("retire"):                 # L36 §6: on the owner's own words, else the owner is asked
                    check(item, ev)
                    target = next(iter(self._current(item)), None)
                    if target is None:
                        raise MemoryRefused("it retires nothing FERB remembers")
                    stated = owner_said(item, ev)
                    why = (NO_QUOTE if stated is None
                           else self._may_end(item, ev, stated[1], ends=[target], about=[target]))
                    if why:
                        self.ask_retire(target)
                        unconfirmed.append({"item": target.id, "why": why})
                    else:
                        self.memory.retire(target.id, f"the owner's words on {day}")
                        retired.append(target.id)
                    continue
                srcs = check(item, ev)
                origin_, content = item["origin"], item["content"]
                confidence = float(item.get("confidence", 0.6))
                stated, why = owner_said(item, ev), ""
                if stated is not None:                  # what it would replace: the current versions, unless one is it
                    ends = self._current(item)
                    if any(e.origin == "owner_stated" and e.content.lower() == stated[0].strip().lower() for e in ends):
                        ends = []
                    why = self._may_end(item, ev, stated[1], ends=ends,
                                        about=[e for e in ends if e.origin == "owner_stated"]) if ends else ""
                if stated is not None and not why:      # exactly as if the planner had captured it in that turn
                    origin_, content, confidence, srcs = "owner_stated", stated[0], 1.0, [("journal", stated[1])]
                m = self.memory.propose(item["kind"], item["subject"], item["key"], content, origin_, srcs,
                                        confidence)  # type: ignore[arg-type]
                proposed += 1
                if why:
                    guessed.append({"item": m.id, "why": why})
                if m.origin == "inferred" and m.status == "proposed":
                    self.ask_keep(m, KEEP_GUESS.format(text=" ".join(m.content.split()), day=day))
            except (MemoryRefused, KeyError, ValueError) as e:
                refused.append(str(e))
        done: dict[str, Any] = {"proposed": proposed, "refused": refused, "retired": retired,
                                "retire_unconfirmed": unconfirmed, "kept_as_guess": guessed}
        self._run(day, "distilled", json.dumps(done))
        return {"day": day, "status": "distilled", **done}

    def _current(self, item: dict[str, Any]) -> list[Any]:
        """The current active versions of the fact an item names, by kind, subject and key as `Memory.propose` stores
        them: what a retire ends, or what a new version replaces."""
        subject = self.memory.redact(str(item.get("subject", "")).strip()) or "owner"
        key = slug(self.memory.redact(str(item.get("key", ""))))
        return [i for i in self.memory.items("active", subject=subject) if i.kind == item.get("kind") and i.key == key]

    def _may_end(self, item: dict[str, Any], ev: summary.DayEvidence, journal_id: str, *, ends: list[Any],
                 about: list[Any]) -> str:
        """Why the owner's quoted words (from the message `journal_id`) may not end or replace the current items `ends`,
        or "" (L36 §6, review fix round 1). The message must be newer than each of them (old words never undo a later
        version). For `about` (a retired item, or an owner-stated one replaced) the quote must be whole clauses of
        what the owner typed (`whole_clauses`, a chat forget's rule: never one word they happened to type) that share
        a telling word with what the item says (`Memory.telling_words`)."""
        row = self.db.conn.execute("SELECT at FROM journal WHERE id = ?", (journal_id,)).fetchone()
        if row is None or any(datetime.fromisoformat(row["at"]) <= e.valid_from for e in ends):
            return OLDER_WORDS
        quote = item["quote"]
        if about and not whole_clauses(ev.owner_words[str(quote["handle"])][1], str(quote["text"])):
            return NOT_A_CLAUSE
        telling = set(self.memory.telling_words(str(quote["text"])))
        if any(not telling & set(query_terms(e.content)) for e in about):
            return NOTHING_SHARED
        return ""

    async def catch_up(self) -> list[dict[str, Any]]:
        out = [await self.run_day(d) for d in self.pending_days()]
        self.purge()                                                  # 3. only days with a summary can go
        return out

    def _run(self, day: str, status: str, detail: str | None, bump: bool = True) -> None:
        now = self.clock.now().isoformat()
        self.db.conn.execute(
            "INSERT INTO distill_runs (day, attempts, status, at, detail) VALUES (?, ?, ?, ?, ?) "
            "ON CONFLICT(day) DO UPDATE SET attempts = attempts + ?, status = CASE WHEN distill_runs.status = "
            "'distilled' THEN 'distilled' ELSE excluded.status END, at = excluded.at, detail = COALESCE(excluded.detail, "
            "distill_runs.detail)", (day, int(bump), status, now, detail, int(bump)))

    async def _model(self, day: str, ev: summary.DayEvidence) -> dict[str, Any]:
        known = [f"- {i.kind} / {i.subject} / {i.key}: {i.content}" for i in self.memory.items("active")]
        pending = [f"- {i.kind} / {i.subject} / {i.key}: {i.content}" for i in self.memory.items("proposed")
                   if i.origin == "observed"]
        ctx = (f"Day: {day}\n\nEvidence (cite handles):\n" + "\n".join(ev.lines) +
               "\n\nWhat FERB already remembers (reuse a key to update a fact):\n" + ("\n".join(known) or "(nothing)") +
               "\n\nPatterns seen before, not yet confirmed (if today's evidence shows one again, propose it with the "
               "same key):\n" + ("\n".join(pending) or "(none)"))
        rt = self.make_runtime("worker", mode="supervisor", tool_names=[], schema=DISTILL_SCHEMA)
        task = Task(f"distill-{day}", None, "distill", self.device_id, Identity.OWNER, set(), "verifier",
                    f"distill-{day}", [], None)
        system = load_prompt("memory/distill.md")
        # Distillation has no read tools. Preserve its evidence in full and disclose an overage
        # rather than silently cut a source it cannot fetch back during the run.
        # A day over the runtime's per-input character cap is refused by name (InputTooLarge), never sent to be
        # rejected (L28); the day stays failed until its evidence changes or `ferb memory distill DAY`.
        parsed = json.loads(await learning_call(rt, task, system, ctx, f"daily evidence {day}", DISTILL_SCHEMA,
                                                role="distill", spans=self.spans, usage=self.record_usage))
        if not isinstance(parsed, dict):
            raise ValueError("not an object")
        parsed["made_by"] = (f"model:{getattr(rt, 'name', 'unknown')}:{getattr(rt, 'model', None) or 'default'}"
                             f"|prompt:memory/distill.md@{hashlib.sha256(system.encode()).hexdigest()[:8]}")
        return parsed


def yesterday(clock: Clock) -> str:
    return (clock.now().astimezone().date() - timedelta(days=1)).isoformat()
