"""Own the daemon's memory glue: learning stores' wiring, searches, forgets, FERB's memory questions, the hourly
distillation and episode cards, chat capture and the memory RPCs. Must not save anything the owner didn't state or
confirm."""

import asyncio
import json
import os
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Awaitable, Callable

from ferb.core import origin as origin_
from ferb.core.errors import Deferred, FerbError, Refused
from ferb.daemon.areas.shared import EDIT_HIDDEN, _all_hidden
from ferb.daemon.server import Session
from ferb.memory.learning import (PLANNER_MEMORY_ITEMS, PROMPT_LEAVES_OUT, SEARCH_TEXT_VERSION, reindex_search,
                                  search_indexed)
from ferb.orchestrator.worker import reason
from ferb.store.db import Database


def _named_node(node: dict[str, Any]) -> str:
    """How FERB names a learned item to the owner in a "Forget for good?" question."""
    return f"learned item “{' '.join(str(node['key']).split())}” ({node['kind']})"


# L36 §2: FERB's questions about one of its guesses (a chat paraphrase; distillation asks with distill.KEEP_GUESS)
BACKLOG_OFFERED = "memory.backlog_offered"     # L36: the older guesses are offered once, as one list


GUESS_REPLACED = "memory.guess_replaced"       # a waiting guess the owner's own statement of its fact settled


KEEP_WORDING = "Keep this, in FERB's words (you didn't say it this way)? “{text}”"


EDIT_QUESTION = ("How should FERB remember it? Type it in your own words; FERB saves exactly what you type. "
                 "(Its guess: “{text}”)")


# L36 §6: an item distillation thinks is out of date without the owner's words for it (Retire / Keep)
RETIRE_QUESTION = "Is this no longer true? “{text}” FERB thinks something you said changed it."


RETIRE_KEPT = "memory.retire_kept"            # the owner's Keep (or a skip) on that question: not asked again


class MemoryGlue:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    def _build_memory(self) -> None:
        from ferb.memory import summary
        from ferb.memory.distill import Distiller
        assert self.db is not None and self.journal is not None
        from ferb.memory.tools import LearningRead
        self.learning.note = lambda kind, data: self._journal().append(kind, data)
        self.learning.secret_values = lambda: getattr(self.redact, "secret_values", set)()
        self.registry.add(LearningRead(lambda: self.learning, self._learned_search))
        self.memory.on_forget = self._forget_item_copies
        self.memory.on_settled = lambda ids, deleted: self._close_memory_questions(ids, None if deleted else
                                                                                  ("confirm", "edit", "retire"))
        self.memory.on_ended = self._memory_ended
        self.memory.on_guess_replaced = lambda guess, by: self._journal().append(GUESS_REPLACED,
                                                                                {"id": guess, "by": by})
        journal = self.journal
        self.memory.rebuild_summary = lambda day: summary.rebuild(self.db, journal, day)
        self.distiller = Distiller(self.db, self.journal, self.memory, self._make_runtime, self.clock,
                                   purge=self._purge_trail, device_id=str(self.config.get("device", "id", "dev")),
                                   record_usage=self._record_learning_usage("distill"), spans=self.spans)
        self.distiller.ask_keep = self._ask_keep
        self.distiller.ask_retire = self._ask_retire
        self.distiller.tell = self._tell_owner
        self._restore_memory_questions()
        if self.run_distill:
            self._distill_task = asyncio.get_running_loop().create_task(self._distill_loop())
        if self.run_retention:
            self._retention_task = asyncio.get_running_loop().create_task(self._retention_loop())
        if self.resource_log is not None:
            self._resources_task = asyncio.get_running_loop().create_task(self._resources_loop())

    def _app_versions(self) -> dict[str, str]:
        profile = self._device_profile()
        return {a.bundle_id: a.version for a in profile.apps} if profile else {}

    async def _capture_learning_episode(self, task_id: str) -> None:
        """A finished task's episode card, after its outcome (`Supervisor.on_episode`). Failures are journaled."""
        try:
            if self.learning is not None:
                await self._capture_card(task_id, self._app_versions(), report=True)
        except Deferred:
            raise                                     # the supervisor journals it; the hourly pass captures it again
        except Exception as e:
            self._journal().append("memory.episode_failed", {"task_id": task_id, "error": self._redact_text(str(e))})

    async def _capture_card(self, task_id: str, apps: dict[str, str] | None, *, report: bool = False) -> Any:
        """Capture (or refresh, `apps` None) a task's episode card (L28). The after-outcome capture and the hourly
        re-capture of cards whose inputs changed (an owner verdict) both come here, one at a time. The reads run in the card reader
        (`_read_card`); only the write is on the loop. When a card input changed while the reader read (a forget, a
        verdict, a write of a card: `Learning.generation`), it reads again. `report`: journal the refs skipped as
        damaged by redaction or expired by retention, and a card skipped as built on forgotten content (C1). Raises
        LearningRefused when the card can't be built."""
        assert self.learning is not None
        async with self._card_lock:
            while True:
                generation = self.learning.generation
                prepared = await self._read_card(task_id, apps)
                if generation == self.learning.generation:
                    break
            if report and prepared:
                for kind, key in (("memory.episode_refs_damaged", "damaged"), ("memory.episode_refs_expired", "expired")):
                    if prepared.get(key):
                        self._journal().append(kind, {"task_id": task_id, "refs": prepared[key]})
                if prepared.get("forgotten"):        # ids only: what they held is gone
                    self._journal().append("memory.episode_skipped", {"task_id": task_id,
                                                                     "why": "built on forgotten content"})
            return self.learning.write_episode(prepared)

    async def _read_card(self, task_id: str, apps: dict[str, str] | None) -> dict[str, Any] | None:
        """Every read of a card, in `ferb.memory.card_reader`: a process of its own on a read-only connection, because
        the lineage resolution is CPU-bound (1.3-1.6 s on the owner's data) and in a thread it would still hold the
        interpreter lock against the event loop. A reader whose wait is cancelled or fails is killed and reaped. Once
        the daemon is stopping (its readers already killed), no reader starts: Deferred. The home also goes on stdin,
        only as a check the reader makes against the home it finds itself."""
        from ferb.memory.learning import LearningRefused
        if self._shutting_down:
            raise Deferred("FERB is stopping; this card is captured again after the next start")
        proc = await asyncio.create_subprocess_exec(
            *self.card_reader, stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.DEVNULL, env={**os.environ, "FERB_HOME": str(self.home)})
        self._card_readers.add(proc)
        try:
            out, _ = await proc.communicate(json.dumps({"task_id": task_id, "apps": apps,
                                                        "home": str(self.home)}).encode())
        finally:
            self._card_readers.discard(proc)
            if proc.returncode is None:
                proc.kill()
                await proc.wait()
        try:
            answer = json.loads(out)
        except ValueError:
            if self._shutting_down:                  # killed by shutdown
                raise Deferred("FERB is stopping; this card is captured again after the next start") from None
            answer = {"error": f"the card reader exited {proc.returncode} without an answer"}
        if "error" in answer:
            raise LearningRefused(str(answer["error"]))
        prepared: dict[str, Any] | None = answer["prepared"]
        return prepared

    def _stop_card_readers(self) -> None:
        """Shutdown: kill every reader still in flight rather than wait for it; the hourly pass captures those cards
        again (`_refresh_cards`)."""
        for proc in list(self._card_readers):
            if proc.returncode is None:
                proc.kill()

    async def _refresh_cards(self) -> dict[str, Any]:
        """The hourly pass's episode cards (L28): each card whose inputs changed (a new verdict, a rebuilt episode) or
        that was never captured is read again, one at a time and only while no task runs; then the day summaries are
        indexed for search (`index_summaries`, in a worker thread) and, with an adopted embedding model, the new nodes
        are embedded. A card that can't be built waits for its inputs to change."""
        from ferb.memory.learning import LearningRefused, index_summaries
        l = self.learning

        def idle() -> bool:
            return self.supervisor is None or not self.supervisor.running_trees()
        if not idle():
            return {"status": "busy"}
        for task_id in l.cards_to_read():
            if not idle():                           # a task started: the next pass reads the rest
                return {"status": "busy"}
            try:
                await self._capture_card(task_id, None)
                if not l.db.conn.execute("SELECT 1 FROM learning_card_inputs WHERE task_id=?", (task_id,)).fetchone():
                    l.card_read(task_id, l.card_inputs(task_id).get(task_id, ""))   # nothing to write: read
            except LearningRefused as e:
                l.card_read(task_id, l.card_inputs(task_id).get(task_id, ""), l.redact(str(e)) or "refused")
            except Deferred:                         # FERB is stopping: the next start's pass reads the rest
                return {"status": "stopping"}
            await asyncio.sleep(0)
        assert self.db is not None
        path, events = self.db.path, list[tuple[Any, ...]]()

        def index() -> None:
            from ferb.memory.learning import Learning
            db = Database(path)
            try:
                own = Learning(db, l.clock, lambda *event: events.append(event), l.redact, l.redact_id)
                own.secret_values = l.secret_values
                index_summaries(own, stop=lambda: False)
            finally:
                db.close()
        await asyncio.to_thread(index)
        for event in events:
            l.publish(*event)
        if l.embedder is not None:
            from ferb.memory.embeddings import index_async
            try:
                await index_async(l, l.embedder)
            except Exception as e:
                l.publish("memory.embedding_unavailable", {"error": l.redact(str(e))})
        return {"status": "done"}

    def _learning_identity(self) -> str:
        if not self.learning:
            return ""
        self.learning.sync_facts(self.memory)
        return self.learning.identity()

    async def _reindex_search(self) -> None:
        """L36 §3: the search index by content (`search_text`), once per SEARCH_TEXT_VERSION, asked at every start (a
        no-op once done). It runs in a worker thread on a connection of its own, in short batches each its own
        transaction (`reindex_search`): the event loop never waits for it, a forget waits one batch at most, and a stop
        or a crash keeps what was done (the next start finishes). Searches wait for it (`_search_ready`), never ranking
        a half re-indexed store. Journaled only when nodes were re-indexed (m3); a failure is journaled and leaves
        search as it stands until the next start."""
        assert self.db is not None
        path, stop = self.db.path, self._reindex_stop

        def work() -> tuple[bool, bool]:
            db = Database(path)
            try:
                db.conn.execute("PRAGMA busy_timeout = 60000")   # a long write elsewhere delays it, never fails it
                return reindex_search(db, stop=stop.is_set), search_indexed(db)
            finally:
                db.close()
        try:
            reindexed, finished = await asyncio.to_thread(work)
        except Exception as e:
            self._journal().append("memory.search_reindex_failed",
                                   {"error": self._redact_text(f"{type(e).__name__}: {e}")})
            return
        if reindexed:
            self._journal().append("memory.search_reindexed", {"version": SEARCH_TEXT_VERSION, "finished": finished})

    async def _search_ready(self) -> None:
        """Wait for the start's search re-index, if it still runs (`_reindex_search`)."""
        if self._reindex_task is not None and not self._reindex_task.done():
            await asyncio.wait({self._reindex_task})

    async def _query_vector(self, words: str) -> tuple[list[int] | None, bool]:
        """The query's embedding when the owner adopted a model, computed off the event loop (L28); (None, False)
        after a failed encode, reported once (keyword ranking only)."""
        embedder = self.learning.embedder if self.learning else None
        if embedder is None or not words:
            return None, True
        try:
            return await asyncio.to_thread(embedder.encode, words, query=True), True
        except Exception as e:                       # reported once; keyword ranking only, no second encode
            self._journal().append("memory.embedding_unavailable", {"error": self._redact_text(str(e))})
            return None, False

    async def _search(self, query: str, *, limit: int | None = None, leave_out: frozenset[str] = frozenset(),
                      sources: bool = True, focus: str = '', **filters: Any) -> list[dict[str, Any]]:
        """The one ranked search (L36 §3) for the planner, attention, workers, memory.read and memory.learned: memory
        items synced to fact nodes first, the query's embedding off the loop, SQLite on it. `sources` False: without
        each node's source refs (a prompt's lines). `focus`: the target words of the tasks the caller works for, also
        ranked alone (`Learning.search`; the planner's and workers' searches, L36 §4 fix round)."""
        if not self.learning:
            return []
        await self._search_ready()
        self.learning.sync_facts(self.memory)
        vector, embed = await self._query_vector(query)
        return list(self.learning.search(query, query_vector=vector, embed=embed, limit=limit, leave_out=leave_out,
                                         sources=sources, focus=focus, **filters))

    async def _learned_search(self, query: str, **filters: Any) -> list[dict[str, Any]]:
        """memory.learned's search (L36 §3): the one search, with its SQLite reads in a worker thread on a connection
        of its own, as the re-index's are (`_reindex_search`; final review M3). A broad or empty query reads every
        matched node with all its source refs (a day's or week's rollup cites tens of thousands), so on the event loop
        it could hold every other call. It waits for the re-index, and facts are synced on the loop first (a write, on
        the daemon's connection); the query's embedding is computed off the loop (`_query_vector`). What the thread's
        store reports (an embedding failure) is published after it, on the loop."""
        from ferb.memory.learning import Learning
        if not self.learning:
            return []
        assert self.db is not None
        await self._search_ready()
        self.learning.sync_facts(self.memory)
        vector, embed = await self._query_vector(query)
        learning, path = self.learning, self.db.path
        told: list[tuple[str, dict[str, Any]]] = []

        def work() -> list[dict[str, Any]]:
            db = Database(path)
            try:
                reader = Learning(db, learning.clock, lambda topic, data: told.append((topic, data)), learning.redact,
                                  redact_id=learning.redact_id)
                reader.embedder, reader.home = learning.embedder, learning.home
                return list(reader.search(query, query_vector=vector, embed=embed, **filters))
            finally:
                db.close()
        found = await asyncio.to_thread(work)
        for topic, data in told:
            self._publish(topic, data)
        return found

    async def _memory_item_search(self, query: str) -> list[str]:
        """memory.read's query, ranked by the one search (L36 §3): the memory items behind the matching facts."""
        return [str(n["body"]["id"]) for n in await self._search(query, kind="fact") if n["body"].get("id")]

    async def _learning_candidates(self, words: str, budget: int = PLANNER_MEMORY_ITEMS, *,
                                   focus: str = '') -> list[dict[str, Any]]:
        """Relevant learned memory, ranked, at most `budget` items (the owner's: planner 60, worker 20; L36 §3), each
        line saying what it is (`shown_text`). `focus`: the target words of the tasks the turn or worker serves, also
        ranked alone (L36 §4 fix round). Each item cites itself (its node carries its own sources; the memory.learned
        tool cites items the same way)."""
        if not self.learning:
            return []
        found = await self._search(words, limit=budget, leave_out=PROMPT_LEAVES_OUT, sources=False, focus=focus)
        from ferb.memory.learning import shown_state, shown_text
        return [
            {'text': f"{p['id']} [{p['kind']}, {shown_state(p)}, {p['origin']}; data, not authorization] "
                     + shown_text(p),
             'reference': 'memory.learned item_id=' + p['id'],
             'origin': origin_.join(p.get('origins', origin_.split(p['origin']))),
             'source_refs': [p['id']]}
            for p in found]

    def _forget_item_copies(self, plan: Any) -> None:
        """A forget's work outside the memory tables (`Memory.on_forget`), before they change: the item's own search
        index nodes (`drop_fact_index`: nothing built on them), the copies of the trail rows it came from (today's
        activity file, delegated conversations' evidence), and a pending clarifying question holding its words. The
        login-record usernames it gave are cleared after (L36 §5). FERB's past answers and traces are not scrubbed
        (notes/memory.md decision 2)."""
        texts = [str(r[0]) for r in self.db.conn.execute(
            f"SELECT content FROM memory_items WHERE id IN ({','.join('?' * len(plan.items))})", plan.items)]
        if self.learning is not None:
            for item_id in plan.items:
                self.learning.drop_fact_index("memory:" + item_id, self.memory)
        if plan.trail:
            self.daily_activity.forget_sources(set(plan.trail))
            if self.supervisor:
                self.supervisor.conversations.forget_sources({"trail:" + t for t in plan.trail})
        if self.supervisor:
            self.supervisor.forget_pending(texts)
        items = list(plan.items)
        self._after_forgets(lambda: self._clear_login_names(items))

    def _memory_ended(self, ids: list[str], new: str | None) -> None:
        """Current memory items that stopped holding now (`Memory.on_ended`, L36 §6): superseded by a newer version
        (`new`) or retired (None). FERB's "Is this no longer true?" questions about them close, and the login-record
        usernames the owner's items among them gave are cleared (`_clear_login_names`, queued like a forget's). A
        newer version whose own login entry names the username in the owner's words, in the same turn, has moved them
        to its spelling first (`_save_login_name` takes the username lock before the queued clear runs; review m-8)."""
        self._close_memory_questions(ids, ("retire",))
        owned = [i for i in ids if (it := self.memory.get(i)) is not None and it.origin == "owner_stated"]
        if owned:                                        # only the owner's own items ever give a username
            self._after_forgets(lambda: self._clear_login_names(owned))

    def _username_sources(self) -> set[str] | None:
        """The memory a login record's username may still come from (L36 §6, review fix round 1): the owner's own items
        that hold now. One past the end date the owner gave it, retired, retracted or forgotten names no account when a
        question or `secrets.list` reads the record (read at use: nothing sweeps the record). None without memory."""
        if self.memory is None:
            return None
        return {"memory:" + i for i in self.memory.current_ids("owner_stated")}

    async def _clear_login_names(self, item_ids: list[str]) -> None:
        """Clear every login record's username that came from one of these memory items (forgotten, retracted, retired
        or superseded: `_memory_ended`), in a worker thread: the handle file's lock may be held by a hidden input's
        Keychain save (coordinator ruling). After any username write already waiting (`_login_names`, review m-3).
        Whatever stops it is journaled, never swallowed (review I-3)."""
        from ferb.identity.owner_secrets import OwnerSecrets
        async with self._login_names:
            try:
                cleared = await asyncio.to_thread(OwnerSecrets(self.home).clear_usernames,
                                                  {"memory:" + i for i in item_ids})
            except Refused as e:                         # an unreadable handle file: left as it is, said in the journal
                self._journal().append("memory.login_unreadable", {"error": str(e)})
                return
            except Exception as e:                       # a failed write (a full disk): the username stays, said
                self._journal().append("memory.login_clear_failed", {"items": sorted(item_ids),
                                                                     "error": f"{type(e).__name__}: {e}"})
                return
        if cleared:
            self._journal().append("memory.login_cleared", {"places": cleared})

    def _forget_memory(self, item_id: str, keep_sources: bool = False) -> Any:
        """The simple forget (notes/memory.md decision 2), now, on the event loop: `Memory.forget`."""
        plan = self.memory.forget(item_id, keep_sources)
        self._journal().append("memory.forgot", {"items": len(plan.items), "trail": len(plan.trail)})
        return plan

    def _after_forgets(self, work: Callable[[], Awaitable[None]]) -> None:
        """Work a forget or an ended item leaves behind (clearing login-record usernames, in a worker thread), queued
        behind what is still running: one at a time. A chat turn and the forget RPCs wait for it
        (`_settle_forgets`)."""
        prior = list(self._forgetting)

        async def run() -> None:
            await asyncio.gather(*prior, return_exceptions=True)
            await work()
        job = asyncio.get_running_loop().create_task(run())
        self._forgetting.add(job)
        job.add_done_callback(self._forgetting.discard)

    def _forget_nodes(self, refs: list[str]) -> list[str]:
        """Learned items the owner asked to forget go (`Learning.forget`: every version, what is built on them,
        tombstones), and FERB's questions about them close."""
        deleted = self.learning.forget(refs)
        self._close_memory_questions(deleted)
        return deleted

    async def _settle_forgets(self) -> None:
        """Wait until the work every forget left behind has finished. A finished one leaves `_forgetting` only when its done
        callback runs, a loop pass later; gathering finished tasks completes without yielding, so waiting on it again
        would spin the event loop and that callback would never run. Only unfinished ones are awaited."""
        while pending := [job for job in self._forgetting if not job.done()]:
            await asyncio.gather(*pending, return_exceptions=True)

    def _chat_forget(self, item_id: str, request_id: str) -> str:
        """A forget the owner asked for in chat (memory review I2), by the id the planner was shown (a memory item, the
        fact node it was shown as, or another learned item). Nothing is deleted now: FERB asks a plain Yes/No question
        naming exactly what goes and what stays, as `ferb memory forget` does, and `_question_answered` forgets on the
        Yes."""
        node = self.learning.get(item_id) if self.learning is not None else None
        ident = str(node["body"].get("id", "")) if node and node["kind"] == "fact" else item_id
        if self.memory is not None and self.memory.get(ident) is not None:
            plan = self.memory.plan_forget(ident)
            target, lines, keeps = ("memory", ident), plan.describe(self.db), plan.keeps()
        elif node is not None and node["kind"] not in ("fact", "identity") and self.learning is not None:
            doomed = sorted(self.learning.descendants([item_id]))
            target = ("learning", item_id)
            lines, keeps = [f"{_named_node(node)} and {len(doomed) - 1} record(s) built on it"], ""
        else:
            return "I couldn't find that memory, so nothing was deleted."
        reason = ("Forget for good? This deletes:\n" + "\n".join("- " + line for line in lines) +
                  (f"\n{keeps}" if keeps else ""))
        self._ask_owner("forget:" + target[0], target[1], reason, "memory-forget-" + request_id)
        return reason + "\nAnswer Yes or No."

    def _ask_owner(self, kind: str, ident: str, reason: str, request_id: str, approval_id: str | None = None) -> None:
        """One of FERB's own questions about memory, its capability and options by kind (`MEMORY_QUESTIONS`): a forget's
        Yes / No and a guess's Keep / Edit / Drop are FERB checks (one of their options answers it, in chat or with the
        notch's buttons); a guess's Edit takes the owner's own words (L36). `_question_answered` does what the answer
        asks. It is kept in `memory_questions` until it is answered, so a restart asks it again
        (`_restore_memory_questions`; rule 9)."""
        from ferb.core.types import Identity
        from ferb.orchestrator.steering import MEMORY_QUESTIONS
        from ferb.policy.rules import ActionRequest
        capability, options = MEMORY_QUESTIONS[kind.split(":", 1)[0]]
        action = ActionRequest(capability, "owner_question", Identity.OWNER, frozenset(), "", "", request_id,
                               frozenset())
        ap = self.state.approvals.question(action, reason, list(options), approval_id)
        self.db.conn.execute("INSERT OR IGNORE INTO memory_questions VALUES (?,?,?,?,?,?)",
                             (ap.id, kind, ident, reason, request_id, self.clock.now().isoformat()))
        self._owner_followups[ap.id] = (kind, ident)
        self._publish("approval.requested", {"approval_id": ap.id, "capability": ap.capability, "kind": ap.kind,
                                             "reason": ap.reason, "task_id": None, "options": list(ap.options)})

    def _ask_once(self, kind: str, item_id: str, reason: str, open_kinds: tuple[str, ...]) -> None:
        """One of FERB's questions about a memory item (`_ask_owner`), never while a question of `open_kinds` about it
        waits (scan m6: shared by `_ask_keep` and `_ask_retire`)."""
        if self.db.conn.execute(f"SELECT 1 FROM memory_questions WHERE target = ? AND kind IN "
                                f"({','.join('?' * len(open_kinds))})", (item_id, *open_kinds)).fetchone():
            return
        self._ask_owner(kind, item_id, reason, f"memory-{kind}-{item_id}")

    def _ask_keep(self, item: Any, reason: str) -> None:
        """L36 §2 ("Ask right away"): one of FERB's guesses about the owner, asked at once with Keep / Edit / Drop;
        only while it waits (an inferred item still proposed), and never twice for one item (an open Keep or Edit
        question about it is enough)."""
        if item.origin == "inferred" and item.status == "proposed":
            self._ask_once("confirm", item.id, reason, ("confirm", "edit"))

    def _ask_retire(self, item: Any) -> None:
        """L36 §6: a current item distillation thinks is out of date, without the owner's words for it, asked with
        Retire / Keep: once while it waits, and never again for that item once the owner kept it (`RETIRE_KEPT`;
        rule 9: the answer carries). Their own words still retire it (`Distiller.run_day`)."""
        if self.db.conn.execute("SELECT 1 FROM journal WHERE kind = ? AND json_extract(data_json, '$.id') = ?",
                                (RETIRE_KEPT, item.id)).fetchone():
            return
        self._ask_once("retire", item.id, RETIRE_QUESTION.format(text=" ".join(item.content.split())), ("retire",))

    def _settle_guess(self, item_id: str, action: str, text: str = "") -> str:
        """The owner's answer to one of FERB's guesses (L36 §2). Keep makes it active, owner-confirmed. Edit saves the
        owner's text as their own words (a new owner-stated version of the same fact) and retracts the guess. Drop
        deletes the guess alone (`Memory.drop`) with its index node (`Learning.drop_fact_index`); Ruling: never
        `forget`, which purges every version and the owner's messages, and everything built on the guess (live
        2026-10-25: 750 learned items, among them the episode cards of every task whose memory reads returned it).
        While a waiting "Forget for good?" would delete the guess (about it, or about another version of the same fact:
        `_forget_would_take`), Drop retracts it instead (scan ruling I1, re-review R1, fix round 1): a deletion would
        close a question about it unanswered, or cut it from the messages the question names, and that Yes still purges
        every copy. A replaced or dropped guess keeps its fingerprint either way. A line for the
        owner; FerbError for an unknown action, an empty edit, or an edit secret redaction hid whole (`EDIT_HIDDEN`)."""
        item = self.memory.get(item_id)
        if item is None or item.status != "proposed" or item.origin != "inferred":
            return "That guess isn't waiting any more."
        if action == "keep":
            self.memory.confirm(item_id)
            return "Kept."
        if action == "drop":
            if self._forget_would_take(item_id):
                self.memory.retract(item_id, "the owner said Drop", never_again=True)
                return "Dropped. FERB won't suggest it again; your question about forgetting it still waits."
            self.memory.drop(item_id)
            self.learning.drop_fact_index("memory:" + item_id, self.memory)   # its index node alone goes with it
            return "Dropped. FERB won't suggest it again."
        words = text.strip()
        if action == "edit" and words:
            if _all_hidden(self._redact_text(words)):
                raise FerbError(EDIT_HIDDEN)
            entry = self._journal().append("memory.edited", {"id": item_id, "content": words})
            mine = self.memory.edit(item_id, words, ("journal", entry.id))
            self.memory.retract(item_id, "the owner wrote it in their own words", never_again=True)
            return "Saved in your words: " + " ".join(mine.content.split())
        raise FerbError("Pick Keep, Edit or Drop; Edit needs the words to save.")

    def _guess_state(self, item_id: str) -> str:
        """How one of FERB's guesses stands now, for the notch (L36): "waiting" for the owner's answer, "kept",
        "edited" (replaced by the owner's own words: their Edit, or their own statement of the same fact while it
        waited, journaled `memory.guess_replaced`), or "dropped" (deleted, or retracted by a Drop while a forget
        waits; a forget deletes it too); "not_a_guess" for a proposed item that isn't one (an observed pattern still
        gathering days: `_settle_guess` takes only a proposed inferred item, review Q-M2). An answer that reached a
        guess already settled, or one whose reply was lost, shows this rather than the answer the owner gave (live
        2026-10-25: a Drop that went through was shown as not recorded)."""
        item = self.memory.get(item_id)
        if item is None:
            return "dropped"
        if item.status == "proposed":
            return "waiting" if item.origin == "inferred" else "not_a_guess"
        if item.status == "active":
            return "kept"
        journal = self._journal()
        return "edited" if (journal.of_kind("memory.edited", "id", item_id)
                            or journal.of_kind(GUESS_REPLACED, "id", item_id)) else "dropped"

    def _guess_backlog(self) -> dict[str, Any] | None:
        """L36 ruling ("The backlog"): FERB's guesses that were waiting before it asked about each at once, offered
        once as one "Review N older guesses" list (each Keep / Edit / Drop, through op "guess"); a guess already asked
        about is not in it. Marked offered whenever it is read, so it never comes back; None when there is nothing."""
        if self._journal().of_kind(BACKLOG_OFFERED):
            return None
        asked = {r["target"] for r in self.db.conn.execute("SELECT target FROM memory_questions")}
        waiting = [i for i in self.memory.items("proposed") if i.origin == "inferred" and i.id not in asked]
        entry = self._journal().append(BACKLOG_OFFERED, {"count": len(waiting)})
        if not waiting:
            return None
        return {"id": entry.id, "text": f"Review {len(waiting)} older guess{'' if len(waiting) == 1 else 'es'}",
                "proposals": [{"id": i.id, "text": " ".join(i.content.split())} for i in waiting]}

    def _forget_would_take(self, item_id: str) -> bool:
        """Whether a waiting "Forget for good?" about a memory would delete this item: its target, another version of the
        same fact, or an item built on the same sources (`plan_forget`, worked out as its Yes would)."""
        from ferb.memory.items import MemoryRefused
        for row in self.db.conn.execute("SELECT target FROM memory_questions WHERE kind = 'forget:memory'").fetchall():
            try:
                if item_id in self.memory.plan_forget(row["target"]).items:
                    return True
            except MemoryRefused:                        # its memory went another way
                continue
        return False

    def _close_memory_questions(self, targets: Any, kinds: tuple[str, ...] | None = None) -> None:
        """FERB's questions about memory that stopped waiting (re-review NEW-1): of `kinds` (None: every kind) about
        `targets`. A memory confirmed or retracted another way closes its "Keep this?" (Keep / Edit / Drop), Edit and
        "Is this no longer true?" (Retire / Keep) questions only, and one superseded or retired the last of these
        (`_memory_ended`, L36 §6); a memory or learned item deleted closes every question about it, the owner's "Forget
        for good?" too, which nothing else closes (re-review R1). Each is closed (superseded: never a No or a Drop),
        dropped from what its answer would do, and deleted from `memory_questions`, so no later turn or restart shows
        its words again."""
        gone = set(targets)
        if not gone:
            return
        for approval_id, (kind, target) in list(self._owner_followups.items()):
            if target in gone and (kinds is None or kind in kinds):
                self._owner_followups.pop(approval_id, None)
                if self.state.approvals.supersede(approval_id) is not None:
                    self._publish("approval.resolved", {"approval_id": approval_id, "state": "superseded"})
        marks = ",".join("?" * len(gone))
        which = f" AND kind IN ({','.join('?' * len(kinds))})" if kinds is not None else ""
        self.db.conn.execute(f"DELETE FROM memory_questions WHERE target IN ({marks}){which}",
                             [*sorted(gone), *(kinds or ())])

    def _restore_memory_questions(self) -> None:
        """At start: FERB's questions about memory that were still waiting (a forget's Yes / No, a guess's Keep / Edit /
        Drop and its Edit, an out-of-date item's Retire / Keep) are asked again, with their ids, words and options; one
        whose memory has gone, stopped waiting or stopped holding meanwhile is dropped."""
        from ferb.memory.items import current
        for row in self.db.conn.execute("SELECT * FROM memory_questions ORDER BY at").fetchall():
            if row["kind"] == "forget:learning":
                gone = self.learning is None or self.learning.get(row["target"]) is None
            else:
                item = self.memory.get(row["target"]) if self.memory is not None else None
                gone = item is None or (row["kind"] in ("confirm", "edit") and item.status != "proposed") or (
                    row["kind"] == "retire" and (item.status != "active" or not current(item, self.clock.now())))
            if gone:
                self.db.conn.execute("DELETE FROM memory_questions WHERE approval_id = ?", (row["approval_id"],))
            elif self.state.approvals.get(row["approval_id"]) is None:
                self._ask_owner(row["kind"], row["target"], row["reason"], row["request_id"], row["approval_id"])

    def _question_answered(self, ap: Any) -> str:
        """FERB's own follow-up to an answered question: a chat forget on its Yes (I2); a guess's Keep, Edit (which asks
        for the owner's words) or Drop (`_settle_guess`, L36); an out-of-date item's Retire, or its Keep (a skip too:
        nothing changes, and FERB doesn't ask about it again, L36 §6). A line for the owner, or ''."""
        followup = self._owner_followups.pop(ap.id, None)
        if followup is None:
            return ""
        self.db.conn.execute("DELETE FROM memory_questions WHERE approval_id = ?", (ap.id,))
        what, ident = followup
        from ferb.memory.items import MemoryRefused
        yes = ap.state == "granted" and ap.choice == "Yes"
        if what == "retire":
            if ap.state == "granted" and ap.choice == "Retire":
                try:
                    self.memory.retire(ident, "the owner said so")
                except MemoryRefused:
                    return "That memory isn't current any more."
                return "Retired: FERB won't use it any more."
            self._journal().append(RETIRE_KEPT, {"id": ident})
            return "Kept as it is."
        if what == "confirm":
            # Ruling: a declined question is Drop (skipping FERB's own check is a No, L3)
            choice = ap.choice if ap.state == "granted" else "Drop"
            if choice == "Edit":
                item = self.memory.get(ident)
                if item is None or item.status != "proposed":
                    return "That guess isn't waiting any more."
                self._ask_owner("edit", ident, EDIT_QUESTION.format(text=" ".join(item.content.split())),
                                "memory-edit-" + ident)
                return ""
            return self._settle_guess(ident, "keep" if choice == "Keep" else "drop")
        if what == "edit":
            if ap.state != "granted" or not ap.answer:
                return self._settle_guess(ident, "drop")
            try:
                return self._settle_guess(ident, "edit", ap.answer)
            except FerbError as e:      # e.g. words routed from chat already hidden (rule 14): asked again, never saved
                self._ask_owner("edit", ident, ap.reason, "memory-edit-" + ident)
                return str(e)
        if not yes:
            return "Kept: nothing was forgotten."
        kind = what.removeprefix("forget:")
        try:
            if kind == "memory":
                gone = len(self._forget_memory(ident).items)
            elif self.learning.get(ident) is None:
                raise MemoryRefused(ident)
            else:
                gone = len(self._forget_nodes([ident]))
        except MemoryRefused:                            # forgotten another way while the question waited
            return "It was already gone: nothing else was deleted."
        self._journal().append("memory.forgot", {"via": "chat", "items": gone})
        return f"Forgot it: {gone} item(s) deleted." if kind == "memory" else \
            f"Forgot it: {gone} item(s) deleted, with the records built on them."

    async def _memory_learning(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        from ferb.memory.learning import LearningRefused
        l = self.learning
        if l is None:
            raise LearningRefused("learning store unavailable")
        op, ident = params.get("op", "inventory"), str(params.get("id", ""))
        if op == "backlog":
            return {"backlog": self._guess_backlog()}
        if op == "guess":
            line = self._settle_guess(ident, str(params.get("action", "")), str(params.get("text") or ""))
            return {"line": line, "state": self._guess_state(ident)}
        if op == "guess_state":
            return {"state": self._guess_state(ident)}
        if op == "inventory":
            l.sync_facts(self.memory)
            return {"items": [n | {"confirmable": not n["confirmed"] and (
                n["kind"] == "identity_proposal" or (n["kind"] == "fact" and n["state"] == "proposed"))}
                for n in l.items(history=bool(params.get("history")))]}
        if op == "search":             # its SQLite reads and the query's embedding off the event loop
            return {"items": await self._learned_search(str(params.get("query", "")), **{
                k: str(params[k]) for k in ("kind", "app", "person", "since", "until") if params.get(k)})}
        if op == "get":
            node = l.get(ident)
            return {"item": node, "history": [n for n in l.items(history=True)
                    if node and n["kind"] == node["kind"] and n["key"] == node["key"]]}
        if op == "confirm":
            node = l.get(ident)
            if node and node["kind"] == "fact":           # a memory item waiting for the owner (I1): confirm it there
                self.memory.confirm(str(node["body"]["id"]))
                l.sync_facts(self.memory)
                return {"item": self._item_dict(self.memory.get(str(node["body"]["id"])))}
            return {"item": l.confirm(ident)}
        if op == "edit":
            node = l.get(ident)
            if node and node["kind"] == "fact":
                item_id = node["body"]["id"]
                await self._memory_edit(session, {"id": item_id, "content": str(params.get("text", ""))})
                l.sync_facts(self.memory)
                return {"items": l.search(node["key"])}
            return {"item": l.edit(ident, str(params.get("text", "")))}
        if op == "retire":
            return {"item": l.retire(ident)}
        if op == "forget":
            node = l.get(ident)
            if node and node["kind"] == "fact":
                item_id = node["body"]["id"]
                if params.get("confirm"):
                    self._forget_memory(item_id)
                    await self._settle_forgets()
                    return {"deleted": [ident]}
                plan = self.memory.plan_forget(item_id)
                return {"descendants": [], "will_delete": plan.describe(self.db), "keeps": plan.keeps()}
            doomed = sorted(l.descendants([ident]))
            if params.get("confirm"):
                return {"deleted": self._forget_nodes([ident])}
            return {"descendants": doomed}
        if op == "rollback":
            return {"invalidated": l.rollback(str(params.get("batch", "")))}
        if op == "digest":
            return {"digest": l.digest()}
        if op == "identity":
            return {"item": l.put("identity", str(params.get("key", "owner")), {"text": str(params["text"])}, [],
                    origin_label="owner", made_by="owner:identity/1", owner=True, state="active")}
        if op == "eval_export":
            from ferb.eval.memory import history_queries
            return history_queries(l)
        if op == "embedding":
            from ferb.memory import embeddings
            action = str(params.get("action", "status"))
            missing = embeddings.extra_missing()
            if action == "status":
                return {"active": getattr(l.embedder, "name", None), "available": missing is None,
                        "evaluations": [dict(r) for r in
                                        self.db.conn.execute("SELECT * FROM learning_embedding_evals ORDER BY at")]}
            if missing is not None:                      # found before any download (M8)
                raise LearningRefused(f"local embeddings need FERB's optional memory extra ({missing}), which this "
                                      "install doesn't have; nothing was downloaded")
            manifest_path = Path(str(params["manifest"]))
            data = embeddings.manifest(manifest_path)
            root = self.home / "models" / "embeddings" / embeddings.digest(data)
            await asyncio.to_thread(embeddings.download, root, data)
            if action == "prepare":
                return {"ready": True, "path": str(root), "active": False}
            model = await asyncio.to_thread(embeddings.LocalMLX, root, data)
            if action == "evaluate":
                cases = json.loads(Path(str(params["dataset"])).read_text())
                return await embeddings.evaluate_async(l, model, cases.get("cases", []) if isinstance(cases, dict) else cases)
            if action == "adopt":
                embeddings.adopt(l, model, str(params["evaluation"]))
                config = self.home / "memory-embedding.json"
                config.write_text(json.dumps({"manifest": data, "root": str(root), "model": model.name}))
                config.chmod(0o600)
                return {"active": model.name}
            raise LearningRefused("unknown embedding action")
        raise LearningRefused("unknown learning operation")

    def _record_learning_usage(self, role: str) -> Callable[[str, dict[str, Any]], None]:
        """A learner's usage recorder: each usage event is a `model.usage` span naming that learner's role (distill)."""
        def record(runtime: str, data: dict[str, Any]) -> None:
            span = self.spans.start("model.usage", task_id=None, parent_id=None,
                                    input={"runtime": runtime, "role": role})
            self.spans.finish(span, output=data, cost_usd=data.get("cost_usd"))
        return record

    def _purge_trail(self) -> Any:
        if self.trail is None:
            return None
        purged, refused = self.trail.purge(int(self.config.get("sense", "trail_retention_days", 30)))
        if purged:
            self._journal().append("trail.purged", {"days": purged})
        return purged, refused

    async def _distill_loop(self) -> None:
        """Hourly: summarize and distill every past day not done yet, then purge (summarized days only)."""
        while True:
            await self._memory_pass()
            await asyncio.sleep(3600)

    async def _memory_pass(self) -> None:
        """One hourly pass: distill, refresh episode cards, restore the memory embedding. No intent labels or
        consolidation (procedure learning was removed, notes/memory.md). Each job has its own error scope, so one
        failing never stops the others (L16); a failure never takes the daemon down and is tried again next hour."""
        assert self.distiller is not None
        jobs: list[tuple[str, Callable[[], Awaitable[Any]]]] = [("memory.distill_failed", self.distiller.catch_up)]
        if self.learning is not None:
            jobs.append(("memory.cards_failed", self._refresh_cards))
            jobs.append(("memory.embedding_restore_failed", self._restore_memory_embedding))
        for failed, job in jobs:
            try:
                await job()
            except asyncio.CancelledError:
                raise
            except Exception as e:
                self._journal().append(failed, {"error": f"{type(e).__name__}: {e}"})

    async def _restore_memory_embedding(self) -> None:
        if self.learning.embedder is not None:
            return
        config = self.home / "memory-embedding.json"
        from ferb.memory.embeddings import LocalMLX, extra_missing
        if not config.is_file() or extra_missing() is not None:     # keyword ranking only (status says why)
            return
        value = json.loads(config.read_text())
        adopted = self.db.conn.execute("SELECT 1 FROM learning_embedding_evals WHERE model=? AND adopted=1",
                                       (value["model"],)).fetchone()
        if adopted:
            self.learning.embedder = await asyncio.to_thread(LocalMLX, Path(value["root"]), value["manifest"])

    def _export_memory(self) -> None:
        from ferb.memory.export import export
        if self.memory is not None:
            export(self.memory, self.home / "memory")

    async def _remember(self, items: list[Any], own_words: str, source_id: str,
                        sites: frozenset[str] = frozenset(),
                        elsewhere: frozenset[str] = frozenset()) -> list[dict[str, str]]:
        """Chat capture (memory review I1, MP2; the check is `items.capture`, shared with distillation's quotes, L36).
        `own_words`: what the owner typed (pasted parts left out). The quote must be whole words of it. Saved as
        owner_stated is only the owner's own text: the whole sentence(s) of their words that hold the planner's content
        (`owner_sentences`), so a slice never drops a negation or a condition. Any other content is FERB's wording of
        what they said: an inferred proposal, out of every prompt until they answer Keep to the question that offers
        it (`_ask_keep`); one whose quote shares no word with it rests on nothing the owner said and is refused. A
        preference in the owner's typed words joins their identity whatever was on screen (owner decision 2026-10-20:
        "No hold for my typed words"). Per saved item: its receipt and the line code adds to the reply; per refused
        item: why, in plain words for the owner's line (owner 2026-10-24: the reason, never "a problem on my side").
        An entry with `login_origin` also gives that site's login record its username (`_save_login_name`, in a worker
        thread; `sites`: the sites the turn's task names, L36 §5; `elsewhere`: those the owner typed only for a task of
        another chat, which a refusal names)."""
        from ferb.memory.items import CAPTURE_TOLD, capture
        saved: list[dict[str, str]] = []
        for it in items:
            got = capture(own_words, str(it.content), str(it.quote))
            if got.refused:
                self._journal().append("memory.capture_refused", {"key": it.key, "why": got.refused})
                saved.append({"refused": CAPTURE_TOLD[got.refused]})
                continue
            try:
                m = self.memory.propose(it.kind, it.subject, it.key, got.content, got.origin,
                                        [("journal", source_id)], 1.0 if got.origin == "owner_stated" else 0.7)
            except FerbError as e:
                self._journal().append("memory.capture_refused", {"key": it.key, "why": str(e)})
                saved.append({"refused": reason(e, self.redact)})
                continue
            text = " ".join(m.content.split())
            ends = (" " + self._until(it.until, m.id, own_words)) if getattr(it, "until", None) is not None else ""
            told = (await self._save_login_name(it, m.id, own_words, sites, m.origin, elsewhere)
                    if getattr(it, "login_origin", None) else "")
            if m.origin == "owner_stated" or m.status == "active":      # active: FERB's wording the owner kept before
                saved.append({"receipt": f"remembered ({m.id}): {m.content}",
                              "line": f"Remembered: {text}{ends}" + (f"\n{told}" if told else "")})
            else:
                self._ask_keep(m, KEEP_WORDING.format(text=text))
                saved.append({"receipt": f"proposed ({m.id}): {m.content} (FERB's wording; waits for the owner's Keep)",
                              "line": "Saved in my words, not yours, so I won't use it until you answer Keep: "
                                      f"“{text}”.{ends}"})
        return saved

    def _until(self, until: Any, item_id: str, own_words: str) -> str:
        """A remembered item's last day from the owner's own date words (L36 §6): the quote must be whole words of what
        they typed; code works out the day (`end_of_day`: the model never computes dates, owner 2026-09-30) and the
        item leaves memory after it, whatever its origin. A day already over, or date words the owner didn't type,
        save it without an end, said in the line and journaled (`memory.until_refused`). A line for the owner."""
        from ferb.clock.when import When, WhenError, end_of_day, local_zone
        from ferb.memory.items import contains_words
        if not contains_words(own_words, str(until.quote)):
            self._journal().append("memory.until_refused", {"item": item_id, "why": "date words not typed"})
            return "(saved without an end date: the date words I quoted aren't in what you typed)"
        try:
            zone = local_zone()
            end = end_of_day(When(day=until.day, date=until.date, in_days=until.in_days), self.clock.now(), zone)
        except WhenError as e:
            self._journal().append("memory.until_refused", {"item": item_id, "why": str(e)})
            return f"(saved without an end date: {e})"
        self.memory.expire_at(item_id, end)
        return f"(until {(end - timedelta(seconds=1)).astimezone(zone):%a %d %b})"

    async def _save_login_name(self, entry: Any, item_id: str, own_words: str, sites: frozenset[str],
                               origin: str = "owner_stated", elsewhere: frozenset[str] = frozenset()) -> str:
        """L36 §5: an owner-stated entry's username for its site's login record. Only when the username is the owner's
        own words (kept in their spelling, `owner_span`; never FERB's wording, which is an inferred item) and the site
        is an https:// origin whose host the owner typed: in this message, or among the sites of the turn's tasks (the
        new, controlled or answered one: `Supervisor._turn_sites` keeps only those the owner typed in this message or
        that task's recorded words, review I-1, final review I1), exactly, a leading "www." aside
        (`same_site`, the rule a record's sites follow: never a subdomain, which on shared hosting is another tenant's).
        A span of the owner's words that touches a "[redacted]" marker is refused (review m-1). Every record whose
        username came from an older version of this fact takes the new spelling too, whether or not this entry's own
        site passes (that record's site passed when its username was saved; review m-8): `_memory_ended` clears what
        is left of the replaced versions after this write. The write runs in a worker thread (the handle file's lock
        may be held by a hidden input's Keychain save; coordinator ruling), one at a time (`_login_names`, review m-3).
        A line for the owner, or "" (the refusal, whatever its cause, is journaled, `memory.login_refused`, unless the
        entry's own place was updated as an older version's record, and the turn goes on: review m-2). A site the owner
        didn't name is also said, in code's words ("Not saved as your username for <site>: you didn't name that site",
        final review I1; "… in this chat" when they named it only for a task of another chat, `elsewhere`); the memory
        itself is saved either way."""
        import re
        from ferb.identity.owner_secrets import OwnerSecrets, login_place, same_site, site_host, typed_site
        from ferb.memory.items import owner_span_at
        from ferb.policy import provenance
        from ferb.sanitize.redact import REDACTED
        place = login_place(str(entry.login_origin))
        host = site_host(place) if place.startswith("https://") else ""
        named = bool(host) and (typed_site(host, provenance.prepare(own_words))
                                or any(same_site(site_host(site), host) for site in sites))
        span = owner_span_at(own_words, str(entry.content)) if origin == "owner_stated" else None
        hidden = span is not None and any(m.start() <= span[1] and span[0] <= m.end()
                                          for m in re.finditer(re.escape(REDACTED), own_words))
        why = ("the site isn't an https:// address" if not host else "the site isn't one you named" if not named
               else "the username isn't in your words" if span is None
               else "the username was hidden as a secret" if hidden else "")
        name = own_words[span[0]:span[1]] if span is not None and not hidden else ""
        older = {"memory:" + v.id for v in self.memory.history(item_id) if v.id != item_id} if name else set()
        followed: list[str] = []
        if name and (not why or older):
            save, source = not why, "memory:" + item_id

            def write() -> list[str]:
                store = OwnerSecrets(self.home)
                if save:
                    store.set_username(place, name, source)
                return store.replace_usernames(older, name, source)
            async with self._login_names:
                try:
                    followed = await asyncio.to_thread(write)
                except Refused as e:
                    why = str(e)
                except Exception as e:                   # a failed write (a full disk): said, and the turn goes on
                    why = f"{type(e).__name__}: {e}"
        if followed:
            self._journal().append("memory.login_updated", {"item": item_id, "places": followed})
        if not why:
            return f"Saved as your login name for {host}."
        lines = [f"Updated your login name for {', '.join(site_host(p) or p for p in followed)}."] if followed else []
        if place not in followed:
            self._journal().append("memory.login_refused", {"item": item_id, "place": place, "why": why})
            if host and not named:                       # the one refusal the owner can act on (final review I1)
                here = " in this chat" if any(same_site(site_host(s), host) for s in elsewhere) else ""
                lines.append(f"Not saved as your username for {host}: you didn't name that site{here}.")
        return " ".join(lines)

    def _item_dict(self, it: Any, conflict: bool = False) -> dict[str, Any]:
        return {"id": it.id, "kind": it.kind, "subject": it.subject, "key": it.key, "content": it.content,
                "origin": it.origin, "status": it.status, "valid_from": it.valid_from.isoformat(),
                "valid_to": it.valid_to.isoformat() if it.valid_to else None, "supersedes": it.supersedes,
                "sources": it.source_ids, "evidence_days": self.memory.evidence_days(it.id), "conflict": conflict}

    async def _memory_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        status = params.get("status", "active")
        items = self.memory.items(None if status == "all" else status, params.get("subject"),
                                  valid_now=not params.get("history"))
        return {"items": [self._item_dict(i) for i in items]}

    async def _memory_show(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        subject = str(params["subject"])
        items = self.memory.items(None, subject, valid_now=False)
        return {"subject": subject, "items": [self._item_dict(i) for i in items]}

    async def _memory_add(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        from ferb.memory.items import slug
        content = str(params["content"]).strip()
        entry = self._journal().append("memory.added", {"content": content})
        it = self.memory.propose(str(params.get("kind") or "fact"), str(params.get("subject") or "owner"),
                                 str(params.get("key") or slug(content, 30)), content, "owner_stated",
                                 [("journal", entry.id)], 1.0)
        return self._item_dict(it)

    async def _memory_confirm(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        return self._item_dict(self.memory.confirm(str(params["id"])))

    async def _memory_edit(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        entry = self._journal().append("memory.edited", {"id": params["id"], "content": params["content"]})
        return self._item_dict(self.memory.edit(str(params["id"]), str(params["content"]), ("journal", entry.id)))

    async def _memory_retract(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        self.memory.retract(str(params["id"]), str(params.get("reason", "the owner")))
        await self._clear_login_names([str(params["id"])])   # L36 §5: a username goes with the memory it came from
        return {"retracted": params["id"]}

    async def _memory_forget(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        keep = bool(params.get("keep_sources"))
        if not params.get("confirm"):
            plan = self.memory.plan_forget(str(params["id"]), keep)
            return {"confirmed": False, "will_delete": plan.describe(self.db), "keeps": plan.keeps()}
        plan = self.memory.plan_forget(str(params["id"]), keep)
        described = plan.describe(self.db)
        self._forget_memory(str(params["id"]), keep)
        await self._settle_forgets()                     # the RPC answers once the forget is complete
        return {"confirmed": True, "deleted": described, "keeps": plan.keeps()}

    async def _timeline(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        since = datetime.fromisoformat(params["since"]).astimezone() if params.get("since") else None
        rows = self._journal().entries(since)
        return {"entries": [{"at": e.at.isoformat(), "kind": e.kind, "data": e.data} for e in rows]}

    def _remember_persona(self, content: str) -> None:
        content = self._redact_text(content)
        entry = self._journal().append('memory.added', {'content': content})
        from ferb.device.persona import PERSONA_KEY
        self.memory.propose('preference', 'owner', PERSONA_KEY, content, 'owner_stated',
                            [('journal', entry.id)], 1.0)

    def _prepare_persona(self, refresh: bool = False) -> dict[str, Any]:
        if self._shutting_down:
            return {'state': 'closed'}
        from ferb.daemon.persona import PersonaReview
        if self.persona is None:
            self.persona = PersonaReview(self.config, self.state.approvals, None,
                remember=self._remember_persona, publish=self._publish,
                record=lambda kind, data: self._journal().append(kind, data), redact=self.redact)
        if self.persona_scan is not None:
            # Snapshot only owner-attributed app names/counts on the database thread; no window titles/content.
            activity = [dict(r) for r in self.db.conn.execute(
                "SELECT json_extract(data_json,'$.bundle_id') AS bundle_id, "
                "json_extract(data_json,'$.app') AS app, COUNT(*) AS focus_events, MAX(at) AS last_used "
                "FROM trail WHERE source='app_focus' AND actor='owner' GROUP BY bundle_id")]
            device = self.config.get('device', 'id')
            if not device:
                device = self.ids.new_id()
                self.config.set('device', 'id', device)
                self.device_id = str(device)
            excluded = self.config.exclude_apps()
            self.persona.collect = lambda: self.persona_scan(self.home, device, activity, excluded)
        return self.persona.start(refresh=refresh)

    async def _setup_persona(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if params.get('status') and self.persona is not None:
            return self.persona.status()
        return self._prepare_persona(refresh=params.get('refresh') is True)

    def _time_meanings(self) -> list[tuple[str, str]]:
        """The owner's own meanings of fuzzy time words ("next friday", "evening"), stored when they corrected FERB."""
        if self.memory is None:
            return []
        return [(it.key.removeprefix("time_").replace("_", " "), it.content)
                for it in self.memory.items("active", "owner") if it.kind == "preference" and it.key.startswith("time_")]
