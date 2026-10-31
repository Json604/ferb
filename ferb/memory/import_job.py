"""Owns one memory import as a background job (notes/memory-import.md): its estimate, run record, progress, resume,
cancel and the one summary notice. Must not keep what the conversations said (only ids, counts and hashes), run a
model call while the owner's chat turn is under way, or ask the owner to approve the facts (decision 6).

A run reads its source again each time it starts or resumes (the export path the owner chose is kept), skips the
conversation parts already read (`memory_import_done`, by hash), and works chunk by chunk: wait while the owner's turn
runs, recall, verify, save, then record the chunk's parts done in the same breath as its facts, so a restart never
reads a chunk twice nor loses one. Three chunks failing in a row stop the run (failed, said why); a later start goes on
from there. Model calls are counted up front (`estimate`) and after (`counts.model_calls`)."""

import asyncio
import json
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from ferb.core.prompts import load_prompt
from ferb.core.types import Identity, Task
from ferb.memory import import_extract as ex
from ferb.memory.import_sources import (
    EXPORTS,
    SOURCES,
    Conversation,
    ImportUnreadable,
    read,
    window_start,
)
from ferb.memory.items import Memory, label

SOURCE_KIND = "memory.import"              # the journal row a run's facts rest on (ids and counts only)
FINISHED_KIND = "memory.import_finished"
VERIFY_CHARS = 30_000                      # one verification call's candidates and evidence
CLAIM_EVIDENCE_CHARS = 1500                # an owner message shown as a claim's evidence
FAILURES_IN_A_ROW = 3
IDLE_POLL_S = 3.0
EXAMPLES = 3


class ImportBusy(ValueError):
    pass


@dataclass
class Plan:
    """What a run will read: the conversations, their parts not yet read, and the chunks those make."""
    conversations: list[Conversation]
    todo: list[ex.Part]
    done_before: int
    chunks: list[ex.Chunk]

    def counts(self) -> dict[str, Any]:
        chars = sum(len(c.text) for c in self.chunks)
        return {"conversations": sum(1 for c in self.conversations if not c.claims),
                "saved_memories": sum(1 for c in self.conversations if c.claims),
                "messages": sum(len(c.messages) for c in self.conversations),
                "owner_messages": sum(1 for c in self.conversations for m in c.messages if m.role == "owner"),
                "parts": len(self.todo) + self.done_before, "parts_done_before": self.done_before,
                "chunks": len(self.chunks),
                # one recall call a chunk, then about one verification call a chunk (more when it finds a lot)
                "model_calls_estimate": 2 * len(self.chunks),
                "input_tokens_estimate": (chars * 2) // 4}


class ImportJob:
    def __init__(self, memory: Memory, *, make_runtime: Callable[..., Any], redact: Callable[[str], str],
                 append: Callable[[str, dict[str, Any]], Any], publish: Callable[[str, dict[str, Any]], None],
                 tell: Callable[..., Awaitable[None]], similar: Callable[[str], Awaitable[list[str]]],
                 busy: Callable[[], bool], new_id: Callable[[], str], spans: Any = None,
                 usage: Callable[[str, dict[str, Any]], None] = lambda *a: None, device_id: str = "dev",
                 home: Path | None = None) -> None:
        self.memory, self.db, self.clock = memory, memory.db, memory.clock
        self.make_runtime, self.redact, self.append, self.publish = make_runtime, redact, append, publish
        self.tell, self.similar, self.busy, self.new_id = tell, similar, busy, new_id
        self.spans, self.usage, self.device_id, self.home = spans, usage, device_id, home
        self.idle_poll = IDLE_POLL_S
        self.chunk_chars = ex.CHUNK_CHARS

    # ---------- reading ----------

    def read(self, source: str, path: str) -> list[Conversation]:
        """Blocking (a worker thread): the source's conversations in the window, redacted."""
        return read(source, Path(path) if path else None, now=self.clock.now(), redact=self.redact, home=self.home)

    def plan(self, source: str, conversations: list[Conversation]) -> Plan:
        all_parts = ex.parts(conversations)
        done = {r["part"] for r in self.db.conn.execute("SELECT part FROM memory_import_done WHERE source = ?",
                                                         (source,))}
        todo = [p for p in all_parts if p.key not in done]
        return Plan(conversations, todo, len(all_parts) - len(todo), ex.chunks(source, todo, self.chunk_chars))

    async def estimate(self, source: str, path: str) -> dict[str, Any]:
        """The dry run: what an import would read and how many model calls it takes, without a model call."""
        check(source, path)
        conversations = await asyncio.to_thread(self.read, source, path)
        now = self.clock.now()
        return {"source": source, "name": SOURCES[source], "since": window_start(now).isoformat(),
                "until": now.isoformat(), **self.plan(source, conversations).counts()}

    # ---------- runs ----------

    def create(self, source: str, path: str) -> str:
        check(source, path)
        if self.running():
            raise ImportBusy("an import is already running; cancel it or wait for it to finish")
        run_id, now = self.new_id(), self.clock.now().isoformat()
        self.db.conn.execute("INSERT INTO memory_import_runs (id, source, path, status, started_at, updated_at) "
                             "VALUES (?, ?, ?, 'running', ?, ?)", (run_id, source, path, now, now))
        return run_id

    def running(self) -> list[str]:
        return [r["id"] for r in self.db.conn.execute(
            "SELECT id FROM memory_import_runs WHERE status = 'running' ORDER BY started_at")]

    def row(self, run_id: str) -> dict[str, Any] | None:
        r = self.db.conn.execute("SELECT * FROM memory_import_runs WHERE id = ?", (run_id,)).fetchone()
        if r is None:
            return None
        return {"run": r["id"], "source": r["source"], "name": SOURCES.get(r["source"], r["source"]),
                "status": r["status"], "started_at": r["started_at"], "updated_at": r["updated_at"],
                "finished_at": r["finished_at"], "error": r["error"], "counts": json.loads(r["counts_json"])}

    def status(self) -> dict[str, Any]:
        """The running import, else the latest one, and the earlier runs (newest first)."""
        rows = [self.row(r["id"]) for r in self.db.conn.execute(
            "SELECT id FROM memory_import_runs ORDER BY status = 'running' DESC, started_at DESC, rowid DESC LIMIT 10")]
        runs = [r for r in rows if r is not None]
        return {"current": runs[0] if runs else None, "runs": runs}

    def _set(self, run_id: str, counts: dict[str, Any], status: str = "running", error: str = "") -> None:
        now = self.clock.now().isoformat()
        self.db.conn.execute(
            "UPDATE memory_import_runs SET status = ?, updated_at = ?, counts_json = ?, error = ?, finished_at = "
            "CASE WHEN ? = 'running' THEN NULL ELSE ? END WHERE id = ?",
            (status, now, json.dumps(counts), error, status, now, run_id))

    def end(self, run_id: str, status: str, error: str = "") -> None:
        """A run stopped from outside (cancelled), keeping its counts."""
        row = self.row(run_id)
        if row is not None and row["status"] == "running":
            self._set(run_id, row["counts"], status, error)
            self.append(FINISHED_KIND, {"run": run_id, "source": row["source"], "status": status,
                                        "counts": _numbers(row["counts"])})
            self.publish("memory.import.progress", {"run": run_id, "status": status, "counts": row["counts"]})

    async def run(self, run_id: str) -> dict[str, Any]:
        row = self.row(run_id)
        if row is None or row["status"] != "running":
            raise ValueError("no running import with that id")
        source = row["source"]
        path = str(self.db.conn.execute("SELECT path FROM memory_import_runs WHERE id = ?", (run_id,)).fetchone()[0])
        counts: dict[str, Any] = dict(row["counts"])
        counts.setdefault("model_calls", 0)
        try:
            conversations = await asyncio.to_thread(self.read, source, path)
        except (ImportUnreadable, OSError) as e:
            return self._finish(run_id, source, counts, "failed", str(e))
        plan = self.plan(source, conversations)
        for k, v in plan.counts().items():               # a resumed run keeps what it counted so far
            counts.setdefault(k, v)
        for k in ("chunks_done", "candidates", "dropped", "verified", "facts_saved", "credited", "kept_owner", "older",
                  "history", "refused", "failed_chunks"):
            counts.setdefault(k, 0)
        counts["chunks_left"] = len(plan.chunks)
        counts.setdefault("saved_ids", [])
        self._set(run_id, counts)
        self._progress(run_id, counts, "reading")
        index = ex.OwnerIndex(conversations)
        entry: str | None = None
        failures, last_error = 0, ""
        for chunk in plan.chunks:
            await self._wait_idle(run_id, counts)
            try:
                facts = await self._chunk(chunk, index, counts)
            except asyncio.CancelledError:
                raise
            except Exception as e:                       # the chunk stays unread: a later start tries it again
                failures, last_error = failures + 1, self.redact(f"{type(e).__name__}: {e}")[:300]
                counts["failed_chunks"] += 1
                self.append("memory.import_chunk_failed", {"run": run_id, "source": source, "error": last_error})
                if failures >= FAILURES_IN_A_ROW:
                    return self._finish(run_id, source, counts, "failed", last_error)
                continue
            failures = 0
            if facts and entry is None:
                entry = self._source_entry(run_id, source)
            got = ex.save(self.memory, facts, run_id=run_id, source_entry=entry or "",
                          now=self.clock.now()) if facts else ex.SaveCounts()
            now = self.clock.now().isoformat()
            for part in chunk.parts:                     # the chunk's parts are done with its facts saved
                self.db.conn.execute("INSERT OR IGNORE INTO memory_import_done (source, part, run_id, at) "
                                     "VALUES (?, ?, ?, ?)", (source, part.key, run_id, now))
            # the facts this run saved that still hold (a newer version from a later chunk replaces an older one)
            current = self.memory.current_ids("inferred")
            counts["saved_ids"] = [i for i in counts["saved_ids"] + got.saved if i in current]
            counts["facts_saved"] = len(counts["saved_ids"])
            for k in ("credited", "kept_owner", "older", "history"):
                counts[k] += getattr(got, k)
            counts["refused"] += len(got.refused)
            counts["chunks_done"] += 1
            counts["chunks_left"] -= 1
            self._set(run_id, counts)
            self._progress(run_id, counts, "reading")
        return self._finish(run_id, source, counts, "done")

    async def _wait_idle(self, run_id: str, counts: dict[str, Any]) -> None:
        """Never during the owner's chat turn: wait until it is over."""
        told = False
        while self.busy():
            if not told:
                self._progress(run_id, counts, "waiting for your current chat turn")
                told = True
            await asyncio.sleep(self.idle_poll)

    async def _chunk(self, chunk: ex.Chunk, index: ex.OwnerIndex, counts: dict[str, Any]) -> list[ex.Verified]:
        task = Task(f"memory-import-{ex.digest(chunk.source, *(p.key for p in chunk.parts))[:12]}", None,
                    f"import memory from {SOURCES[chunk.source]}", self.device_id, Identity.OWNER, set(), "verifier",
                    "memory-import", [], None)
        answer = await self._call(task, "memory/import_recall.md", chunk.text, ex.RECALL_SCHEMA, "memory_import_recall",
                                  counts)
        candidates, dropped = ex.check_recall(answer, chunk, index, self.redact)
        counts["candidates"] += len(candidates) + len(dropped)
        counts["dropped"] += len(dropped)
        facts: list[ex.Verified] = []
        for batch in _batches(candidates):
            held = await ex.similar_held(self.memory, batch, self.similar)
            ctx = ex.verify_context(chunk.source, batch, held, label)
            answer = await self._call(task, "memory/import_verify.md", ctx, ex.VERIFY_SCHEMA, "memory_import_verify",
                                      counts)
            kept, _gone = ex.check_verify(answer, batch, {it.id for it in held}, chunk.source)
            facts += kept
            counts["dropped"] += len(batch) - len(kept)
            counts["verified"] += len(kept)
        return facts

    async def _call(self, task: Task, prompt: str, ctx: str, schema: dict[str, Any], role: str,
                    counts: dict[str, Any]) -> dict[str, Any]:
        from ferb.memory.calls import learning_call
        rt = self.make_runtime("worker", mode="supervisor", tool_names=[], schema=schema)
        counts["model_calls"] += 1
        text = await learning_call(rt, task, load_prompt(prompt), ctx, f"imported history ({role})", schema,
                                   role=role, spans=self.spans, usage=self.usage)
        return ex.as_json(text)

    def _source_entry(self, run_id: str, source: str) -> str:
        """The journal row this run's facts from `source` rest on: which import, nothing it read."""
        return str(self.append(SOURCE_KIND, {"run": run_id, "source": source, "name": SOURCES[source]}).id)

    def _progress(self, run_id: str, counts: dict[str, Any], stage: str) -> None:
        self.publish("memory.import.progress", {"run": run_id, "status": "running", "stage": stage,
                                                "counts": _numbers(counts)})

    def _finish(self, run_id: str, source: str, counts: dict[str, Any], status: str, error: str = "") -> dict[str, Any]:
        self._set(run_id, counts, status, error)
        self.append(FINISHED_KIND, {"run": run_id, "source": source, "status": status, "counts": _numbers(counts),
                                    **({"error": error} if error else {})})
        self.publish("memory.import.progress", {"run": run_id, "status": status, "counts": _numbers(counts),
                                                **({"error": error} if error else {})})
        return self.row(run_id) or {}

    async def tell_done(self, run_id: str) -> None:
        """The one summary notice (decision 6): how many facts, a few examples as note cards, and the memory page."""
        from ferb.core import origin
        from ferb.orchestrator import cards
        row = self.row(run_id)
        if row is None:
            return
        counts, name = row["counts"], row["name"]
        items = [it for i in counts.get("saved_ids", []) if (it := self.memory.get(i)) is not None
                 and it.status == "active" and it.valid_to is None]
        n = int(counts.get("facts_saved", 0))
        examples = items[:EXAMPLES]
        if row["status"] == "done" and n:
            text = (f"Imported {n} fact{'s' if n != 1 else ''} from {name} as my understanding, for example "
                    + ", ".join(f"“{' '.join(it.content.split())}”" for it in examples)
                    + f". They're on the Memory page (imported from {name}); tell me if any is wrong.")
        elif row["status"] == "done":
            text = f"Imported nothing new from {name}: I found nothing more to remember there."
        else:
            text = (f"The import from {name} stopped ({row['error'] or row['status']}) after {n} fact"
                    f"{'s' if n != 1 else ''}. Start it again to go on from there.")
        line = f"Imported from {name}"
        notes = [cards.note(it.id, " ".join(it.content.split()), "saved", line)
                 | {"label": label(it.origin), "origin": it.origin} for it in examples]
        page = {"type": "page", "page": "memory", "filter": {"imported_from": row["source"]},
                "line": "Open the Memory page"}
        await self.tell(f"memory-import-{run_id}", ["memory:" + it.id for it in examples], text,
                        origin.join({origin.FERB, origin.INFERRED}), cards=notes + [page])


def check(source: str, path: str) -> None:
    if source not in SOURCES:
        raise ImportUnreadable(f"unknown source {source!r}: one of {', '.join(SOURCES)}")
    if source in EXPORTS and not path:
        raise ImportUnreadable(f"{SOURCES[source]} is imported from its data export: choose the file")


def _numbers(counts: dict[str, Any]) -> dict[str, Any]:
    return {k: v for k, v in counts.items() if k != "saved_ids"}


def _batches(candidates: list[ex.Candidate]) -> list[list[ex.Candidate]]:
    """Candidates in verification calls of about `VERIFY_CHARS`; a claim's evidence shown shorter."""
    for c in candidates:
        if c.claim:
            c.evidence = [(ref, m if who != "owner" else type(m)(m.role, _short(m.text), m.at), who)
                          for ref, m, who in c.evidence]
    out: list[list[ex.Candidate]] = []
    size = 0
    for c in candidates:
        chars = len(c.content) + len(c.quote) + sum(len(m.text) + 40 for _, m, _ in c.evidence) + 120
        if out and size + chars <= VERIFY_CHARS:
            out[-1].append(c)
            size += chars
        else:
            out.append([c])
            size = chars
    return out


def _short(text: str) -> str:
    from ferb.memory.import_sources import cut
    return cut(text, CLAIM_EVIDENCE_CHARS)
