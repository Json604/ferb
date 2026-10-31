"""Owns daily memory proposals and their evidence links. Must not treat retrieved observations as owner authorization or silently cut stored content.

Daily distillation: for each past day, in order —
1. the permanent summary, by code, before anything else (so a purge is never blocked on a model);
2. one model run (the worker runtime from config, no tools, typed output, unmetered): a short narrative of the day
   and proposed memory items citing evidence handles (summary.py);
3. then the retention purge, which only ever deletes days that have their summary.

Code checks every proposal before `Memory.save` (which checks the sources again): handles must exist; an observed
item cites only the owner's own trail (E); nothing about the owner or their preferences may rest on FERB's activity
(F) or on pasted third-party text (P); a third-party item must cite pasted text. No Keep questions (notes/memory.md
decision 15): an inferred item is saved at once as FERB's understanding. It may quote the owner's words from one of its
C handles: when chat capture's check passes on that message's typed lines, the owner's sentence is saved as theirs
(`owner_said`). A fact already stored (its kind, subject and key hold a current item, whatever its origin) is skipped,
unless the owner's own quoted words replace it (the live store's duplicate pairs came from re-proposing stored keys).
An item with "retire" names a current item the owner's words or the day's evidence say no longer holds: it is retired
and told once in chat, with the owner's quote when it passes the same check (`_may_end`), else as FERB's own check;
words older than the item never retire it. Quoted words that would replace an item the owner stated must also be newer
than it and a whole clause sharing a telling word with it (`_may_end`); otherwise the new version is kept beside it as
FERB's understanding. Items whose check date has come (`Memory.due_for_review`) are listed for the model; one it
doesn't retire holds up and its check date is cleared.
A model failure leaves the summary in place and is retried on later hourly runs or an explicit retry."""

import hashlib
import json
from datetime import date, datetime, timedelta
from typing import Any, Awaitable, Callable

from ferb.core import origin
from ferb.core.interfaces import Clock
from ferb.core.prompts import load_prompt
from ferb.core.types import Identity, Task
from ferb.memory import summary
from ferb.memory.calls import learning_call
from ferb.memory.items import (KINDS, Memory, MemoryRefused, capture, query_terms, slug, subject_key,
                               whole_clauses)
from ferb.store.db import Database

# L36 §2: the owner's own words an inferred item rests on. Ruling: the model cites one of the item's C handles (it sees
# handles, not journal ids); code maps the handle to its message.
QUOTE_SCHEMA: dict[str, Any] = {"anyOf": [{"type": "null"}, {
    "type": "object", "additionalProperties": False, "required": ["handle", "text"],
    "properties": {"handle": {"type": "string"}, "text": {"type": "string"}}}]}
# L36 §6 (review fix round 1): why the owner's quoted words don't end or replace what FERB remembers (then it is FERB's own check)
NO_QUOTE = "no quote of the owner's own words"
OLDER_WORDS = "the owner's words are older than what FERB remembers"
NOT_A_CLAUSE = "the quote isn't a whole clause of the owner's words"
NOTHING_SHARED = "the quote shares no telling word with what FERB remembers"
# Re-review ruling (c): what FERB ended or replaced without asking is told once; a reply undoes it the usual way
RETIRED_NOTICE = "I retired “{item}” because you said “{quote}” ({day}). Reply to undo."
REPLACED_NOTICE = "I replaced “{item}” because you said “{quote}” ({day}). Reply to undo."
STALE_NOTICE = "Memory: I stopped using “{item}”: it looks out of date ({day}). Tell me if it still holds."
# Decision 17: the owner's own words FERB's check finds over are set aside, kept as history
ASIDE_NOTICE = ("Memory: “{item}” looks over ({day}), so I'll stop using it. It's kept; tell me if it still holds and "
                "I'll use it again.")
ALREADY_STORED = "already remembered (its kind, subject and key hold a current item)"


async def _untold(ident: str, refs: list[str], text: str, notice_origin: str) -> None:
    return None
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
        # one notice line in the owner's chat, once per id: (id, refs, text, origin)
        self.tell: Callable[[str, list[str], str, str], Awaitable[None]] = _untold

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
        retired: list[str] = []                    # L36 §6: items retired on the owner's words or FERB's check
        stale: list[dict[str, str]] = []           # retired as FERB's own check (no quote of the owner's), with why
        guessed: list[dict[str, str]] = []         # quoted words kept beside the owner's item instead, with why
        due = {it.id for it in self.memory.due_for_review()}
        items = out.get("items", [])
        drop: set[int] = set()
        jev = self.jev
        if getattr(getattr(jev, "client", None), "refused", None) is not None:
            jev = None                                 # Jev refused the key: not asked again, no rows written
        if jev is not None and items:
            from ferb.decide.decider import Ask
            try:
                got = await jev.ask({"day": day, "items": [{"n": i, "kind": x.get("kind"), "subject": x.get("subject"),
                                                                "content": x.get("content")} for i, x in enumerate(items)]},
                                         [Ask(f"memory.worth_keeping#{i}", "yes", f"distill:{day}#{i}")
                                          for i in range(len(items))])
            except Exception:
                got = {}
            drop = {int(k.split("#")[1]) for k, v in got.items() if v == "no"}
            for i in drop:
                jev.mark_effect(f"memory.worth_keeping#{i}", f"distill:{day}#{i}", "dropped")
        for n, item in enumerate(items):
            if n in drop:
                refused.append("a second check found it not worth keeping")
                continue
            try:
                if item.get("retire"):                 # L36 §6, decision 15: retired and told once, never asked
                    check(item, ev)
                    target = next(iter(self._current(item)), None)
                    if target is None:
                        raise MemoryRefused("it retires nothing FERB remembers")
                    stated = owner_said(item, ev)
                    why = (NO_QUOTE if stated is None
                           else self._may_end(item, ev, stated[1], ends=[target], about=[target]))
                    if why == OLDER_WORDS:
                        raise MemoryRefused(OLDER_WORDS)
                    if not why:                        # the owner's own words say it is over
                        self.memory.retire(target.id, f"the owner's words on {day}")
                        retired.append(target.id)
                        assert stated is not None
                        await self._tell(RETIRED_NOTICE, "retired", target, item, stated[1])
                        continue
                    # FERB's own check (decision 17): its understanding is retired, the owner's words set aside
                    outcome = self.memory.looks_over(target.id, f"FERB's check of {day}: {why}")
                    retired.append(target.id)
                    stale.append({"item": target.id, "why": why, "outcome": outcome})
                    await self._tell_stale(target, day, aside=outcome == "set_aside")
                    continue
                srcs = check(item, ev)
                origin_, content = item["origin"], item["content"]
                confidence = float(item.get("confidence", 0.6))
                stated, why, ends = owner_said(item, ev), "", self._current(item)
                if ends and stated is None:
                    raise MemoryRefused(ALREADY_STORED)
                if stated is not None:                  # what it would replace: the current versions, unless one is it
                    if any(e.origin == "owner_stated" and e.content.lower() == stated[0].strip().lower() for e in ends):
                        ends = []
                    why = self._may_end(item, ev, stated[1], ends=ends,
                                        about=[e for e in ends if e.origin == "owner_stated"]) if ends else ""
                owner_now = stated is not None and not why
                if owner_now:                           # exactly as if the planner had captured it in that turn
                    assert stated is not None
                    origin_, content, confidence, srcs = "owner_stated", stated[0], 1.0, [("journal", stated[1])]
                m = self.memory.save(item["kind"], item["subject"], item["key"], content, origin_,
                                     srcs, confidence, from_owner=owner_now).item  # type: ignore[arg-type]
                proposed += 1
                if why:
                    guessed.append({"item": m.id, "why": why})
                elif stated is not None and ends and m.origin == "owner_stated":
                    await self._tell(REPLACED_NOTICE, "replaced",
                                     next((e for e in ends if e.origin == "owner_stated"), ends[0]), item, stated[1])
            except (MemoryRefused, KeyError, ValueError) as e:
                refused.append(str(e))
        for item_id in due - set(retired):              # checked and not retired: it holds up
            self.memory.set_review(item_id, None)
        done: dict[str, Any] = {"proposed": proposed, "refused": refused, "retired": retired,
                                "retired_as_check": stale, "kept_beside": guessed}
        self._run(day, "distilled", json.dumps(done))
        return {"day": day, "status": "distilled", **done}

    def _current(self, item: dict[str, Any]) -> list[Any]:
        """The current active versions of the fact an item names, by kind, subject and key as `Memory.propose` stores
        them: what a retire ends, or what a new version replaces."""
        subject = subject_key(self.memory.redact(str(item.get("subject", ""))))
        key = slug(self.memory.redact(str(item.get("key", ""))))
        return [i for i in self.memory.items("active", subject=subject) if i.kind == item.get("kind") and i.key == key]

    def _may_end(self, item: dict[str, Any], ev: summary.DayEvidence, journal_id: str, *, ends: list[Any],
                 about: list[Any]) -> str:
        """Why the owner's quoted words (from the message `journal_id`) may not end or replace the current items `ends`,
        or "" (L36 §6, review fix round 1). The message must be newer than each of them (old words never undo a later
        version). For `about` (a retired item, or an owner-stated one replaced) the quote must be whole clauses of
        what the owner typed (`whole_clauses`, a chat forget's rule: never one word they happened to type) that share
        a telling word with what the item says (`Memory.telling_words`)."""
        said = self._said_at(journal_id)
        if said is None or any(said <= e.valid_from for e in ends):
            return OLDER_WORDS
        quote = item["quote"]
        if about and not whole_clauses(ev.owner_words[str(quote["handle"])][1], str(quote["text"])):
            return NOT_A_CLAUSE
        telling = set(self.memory.telling_words(str(quote["text"])))
        if any(not telling & set(query_terms(e.content)) for e in about):
            return NOTHING_SHARED
        return ""

    def _said_at(self, journal_id: str) -> datetime | None:
        """When the owner's message was journaled."""
        row = self.db.conn.execute("SELECT at FROM journal WHERE id = ?", (journal_id,)).fetchone()
        return datetime.fromisoformat(row["at"]) if row else None

    async def _tell(self, template: str, verb: str, ended: Any, item: dict[str, Any], journal_id: str) -> None:
        """Re-review ruling (c): FERB ended or replaced `ended` on the owner's words without asking, so it tells them
        once, with the item, their quote and the day they said it (`tell`: the daemon's notice line, journaled
        `chat.notice`). A reply undoes it through the planner's usual steer, forget or edit; no other path."""
        from ferb.memory.learning import memory_origin
        said = self._said_at(journal_id)
        text = template.format(item=" ".join(ended.content.split()), quote=" ".join(str(item["quote"]["text"]).split()),
                               day=f"{said.astimezone():%a %d %b}" if said else "today")
        await self.tell(f"memory-{verb}-{ended.id}", ["memory:" + ended.id, "journal:" + journal_id], text,
                        origin.join({origin.FERB, origin.OWNER, memory_origin(ended.origin)}))

    async def _tell_stale(self, ended: Any, day: str, *, aside: bool = False) -> None:
        """Decisions 15 and 17: an item FERB retired (or, the owner's own words, set aside) on its own check of the day
        is told once, quietly; a reply undoes it."""
        from ferb.memory.learning import memory_origin
        text = (ASIDE_NOTICE if aside else STALE_NOTICE).format(item=" ".join(ended.content.split()), day=day)
        await self.tell(f"memory-stale-{ended.id}", ["memory:" + ended.id], text,
                        origin.join({origin.FERB, origin.INFERRED, memory_origin(ended.origin)}))

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
        due = [f"- {i.kind} / {i.subject} / {i.key}: {i.content} (stated {i.valid_from.astimezone():%Y-%m-%d})"
               for i in self.memory.due_for_review()]
        ctx = (f"Day: {day}\n\nEvidence (cite handles):\n" + "\n".join(ev.lines) +
               "\n\nWhat FERB already remembers (don't propose these again; retire one that no longer holds):\n" +
               ("\n".join(known) or "(nothing)") +
               "\n\nPatterns seen before, not yet confirmed (if today's evidence shows one again, propose it with the "
               "same key):\n" + ("\n".join(pending) or "(none)") +
               "\n\nDue for a check (FERB set a date to check these still hold; retire one that is over):\n" +
               ("\n".join(due) or "(none)"))
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
