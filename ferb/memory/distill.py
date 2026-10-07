"""Owns daily memory proposals and their evidence links. Must not treat retrieved observations as owner authorization or silently cut stored content.

Daily distillation: for each past day, in order —
1. the permanent summary, by code, before anything else (so a purge is never blocked on a model);
2. one model run (the worker runtime from config, no tools, typed output, unmetered): a short narrative of the day
   and proposed memory items citing evidence handles (summary.py);
3. then the retention purge, which only ever deletes days that have their summary.

Code checks every proposal before `Memory.propose` (which checks the sources again): handles must exist; an observed
item cites only the owner's own trail (E); nothing about the owner or their preferences may rest on FERB's activity
(F) or on pasted third-party text (P); a third-party item must cite pasted text. A model failure leaves the summary in
place and is retried on later hourly runs or an explicit retry."""

import json
from datetime import date, datetime, timedelta
from typing import Any, Callable

from ferb.core.interfaces import Clock
from ferb.core.context import ContextBlock, assemble_parts, count_tokens
from ferb.core.prompts import load_prompt
from ferb.core.structured import check_final
from ferb.core.types import Identity, ModelRequest, Task
from ferb.memory import summary
from ferb.memory.items import KINDS, Memory, MemoryRefused
from ferb.store.db import Database

DISTILL_SCHEMA: dict[str, Any] = {
    "type": "object", "additionalProperties": False, "required": ["narrative", "items"],
    "properties": {
        "narrative": {"type": "string"},
        "items": {"type": "array", "items": {
            "type": "object", "additionalProperties": False,
            "required": ["kind", "subject", "key", "content", "origin", "confidence", "handles"],
            "properties": {
                "kind": {"type": "string", "enum": list(KINDS)},
                "subject": {"type": "string"}, "key": {"type": "string"}, "content": {"type": "string"},
                "origin": {"type": "string", "enum": ["observed", "inferred", "third_party"]},
                "confidence": {"type": "number"},
                "handles": {"type": "array", "items": {"type": "string"}}}}}}}


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


class Distiller:
    def __init__(self, db: Database, journal: Any, memory: Memory, make_runtime: Callable[..., Any], clock: Clock,
                 purge: Callable[[], Any] = lambda: None, device_id: str = "dev",
                 record_usage: Callable[[str, dict[str, Any]], None] = lambda runtime, data: None) -> None:
        self.db, self.journal, self.memory, self.make_runtime = db, journal, memory, make_runtime
        self.clock, self.purge, self.device_id = clock, purge, device_id
        self.record_usage = record_usage
        self.jev: Any = None                          # memory.worth_keeping: may only drop

    def pending_days(self, today: date | None = None) -> list[str]:
        """Past days with anything to remember that aren't done yet (oldest first)."""
        today = today or self.clock.now().astimezone().date()
        days = {r["day"] for r in self.db.conn.execute("SELECT DISTINCT day FROM trail WHERE day < ?",
                                                       (today.isoformat(),))}
        for r in self.db.conn.execute("SELECT at FROM journal WHERE kind = 'chat.message'"):
            d = datetime.fromisoformat(r["at"]).astimezone().date()
            if d < today:
                days.add(d.isoformat())
        done = {r["day"] for r in self.db.conn.execute("SELECT day FROM distill_runs WHERE status = 'distilled'")}
        return sorted(d for d in days if d not in done or not summary.summarized(self.db, d))

    def write_summary(self, day: str) -> summary.DayEvidence:
        ev = summary.build(self.db, day)
        if not summary.summarized(self.db, day):
            self.journal.append(summary.SUMMARY_KIND, ev.digest)
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
        except Exception as e:                                        # summary stays; retried on a later run
            self._run(day, "failed", f"{type(e).__name__}: {e}")
            return {"day": day, "status": "failed"}
        self.journal.append(summary.NARRATIVE_KIND, {"day": day, "text": str(out.get("narrative", ""))})
        proposed, refused = 0, []
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
                srcs = check(item, ev)
                self.memory.propose(item["kind"], item["subject"], item["key"], item["content"], item["origin"],
                                    srcs, float(item.get("confidence", 0.6)))  # type: ignore[arg-type]
                proposed += 1
            except (MemoryRefused, KeyError, ValueError) as e:
                refused.append(str(e))
        self._run(day, "distilled", json.dumps({"proposed": proposed, "refused": refused}))
        return {"day": day, "status": "distilled", "proposed": proposed, "refused": refused}

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
        parts, cacheable = assemble_parts([ContextBlock(ctx, f"daily evidence {day}", protected=True)],
                                          reserved_tokens=count_tokens(system))
        run = await rt.start(task, ModelRequest(system, tuple(parts), cacheable), "")
        final = ""
        try:
            async for e in rt.events(run):
                if e.kind == "final":
                    final = str(e.data.get("text", ""))
                elif e.kind == "usage":
                    self.record_usage(rt.name, dict(e.data))
                elif e.kind in ("error", "violation"):
                    raise RuntimeError(str(e.data))
        finally:
            await rt.cancel(run)
        final = check_final(final, DISTILL_SCHEMA)
        parsed = json.loads(final)
        if not isinstance(parsed, dict):
            raise ValueError("not an object")
        return parsed


def yesterday(clock: Clock) -> str:
    return (clock.now().astimezone().date() - timedelta(days=1)).isoformat()
