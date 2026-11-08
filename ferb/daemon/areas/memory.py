"""Own the daemon's memory glue: learning stores' wiring, searches, forgets, the core profile (Tier 1), what FERB saw on
this Mac, the hourly distillation, the weekly profile pass and episode cards, chat capture and the memory RPCs. Must not
let memory authorize an action or save a secret. FERB saves facts itself, labelled by where they came from, and asks
no Keep question (MEM-12)."""

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
from ferb.memory.learning import SEARCH_TEXT_VERSION, reindex_search, search_indexed
from ferb.orchestrator.worker import reason
from ferb.store.db import Database


def _named_node(node: dict[str, Any]) -> str:
    """How FERB names a learned item to the owner in a "Forget for good?" question."""
    return f"learned item “{' '.join(str(node['key']).split())}” ({node['kind']})"


# Step 9a: FERB's questions about memory that no longer exist (MEM-12: no Keep / Edit / Drop, no Retire / Keep);
# rows an older build left waiting are dropped at start
GONE_QUESTIONS = ("confirm", "edit", "retire")
# A fact FERB's weekly check found out of date, told once in chat (MEM-12: quiet, corrected by talking)
ENDED_NOTICE = "Memory: I stopped using {items}, which look out of date. Tell me if any still holds."
# MEM-14: the owner's own words FERB's check found over are set aside (kept, restored by saying they still hold)
SET_ASIDE_NOTICE = ("Memory: {items} look over, so I'll stop using them. They're kept; tell me if any still holds and "
                    "I'll use it again.")


# Ruling (step 12a): the newest rows a `timeline.get` returns unless it asks for more (a developer listing)
TIMELINE_ROWS = 1000

class MemoryGlue:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    def _build_memory(self) -> None:
        from ferb.memory import summary
        from ferb.memory.distill import Distiller
        from ferb.memory.profile import CoreProfile
        from ferb.memory.upgrade import clean_before_tier1, close_fact_nodes
        assert self.db is not None and self.journal is not None
        from ferb.memory.tools import LearningRead
        self.learning.note = lambda kind, data: self._journal().append(kind, data)
        self.learning.secret_values = lambda: getattr(self.redact, "secret_values", set)()
        self.registry.add(LearningRead(lambda: self.learning, self._learned_search))
        self.memory.on_forget = self._forget_item_copies
        self.memory.on_settled = lambda ids, deleted: self._close_memory_questions(ids) if deleted else None
        self.memory.on_ended = self._memory_ended
        self.profile = CoreProfile(self.memory, self.clock, record=lambda kind, data: self._journal().append(kind, data))
        self.memory.on_saved = self.profile.saved
        self.memory.on_set_aside = lambda ids: self.profile.remove(ids, by="code")
        self.profile.on_joined = self._now_always_on
        journal = self.journal
        self.memory.rebuild_summary = lambda day: summary.rebuild(self.db, journal, day)
        self.distiller = Distiller(self.db, self.journal, self.memory, self._make_runtime, self.clock,
                                   device_id=str(self.config.get("device", "id", "dev")),
                                   record_usage=self._record_learning_usage("distill"), spans=self.spans)
        self.distiller.tell = self._tell_owner
        # step 9a upgrade (STORE-403): memory cleaned once, then the core profile seeded from it, both journaled
        clean_before_tier1(self.memory, lambda kind, data: self._journal().append(kind, data))
        self.profile.seed()
        self.profile.settle("start")                  # MEM-429: a start settles what joined Tier 1 before it
        # step 9b upgrade: the old fact-node copies close once (search reads memory_items itself)
        close_fact_nodes(self.db, self.clock.now().isoformat(), lambda kind, data: self._journal().append(kind, data))
        self._restore_memory_questions()
        if self.run_distill:
            self._distill_task = asyncio.get_running_loop().create_task(self._distill_loop())
        if self.run_retention:
            self._retention_task = asyncio.get_running_loop().create_task(self._retention_loop())
        if self.resource_log is not None:
            self._resources_task = asyncio.get_running_loop().create_task(self._resources_loop())
        if self.run_embeddings:                      # MEM-6: the local model, then new text in the background
            self._embed_task = asyncio.get_running_loop().create_task(self._embed_loop())
        self._resume_imports()                       # memory import: a run a restart interrupted goes on

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
        from ferb.orchestrator.test_runs import TestRuns
        if self.db is not None and TestRuns.of(self.db).task(task_id):
            return None                       # GATE-550: nothing is learned from a test run's task
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
        indexed for search (`index_summaries`, in a worker thread). New text is embedded by the background embedder
        (`_embed_loop`). A card that can't be built is tried once more, then waits for its inputs to change."""
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
            except LearningRefused as e:                # MEM-422: each failure journaled, with why and what follows
                why = l.redact(str(e)) or "refused"
                status = l.card_read(task_id, l.card_inputs(task_id).get(task_id, ""), why)
                self._journal().append("memory.cards_failed", {
                    "task_id": task_id, "error": why,
                    "next": "waits until its task changes" if status == "parked" else "tried once more next hour"})
            except Deferred:                         # FERB is stopping: the next start's pass reads the rest
                return {"status": "stopping"}
            await asyncio.sleep(0)
        assert self.db is not None
        path, events = self.db.path, list[tuple[Any, ...]]()

        def index() -> None:
            from ferb.memory.learning import Learning
            db = Database(path)
            try:
                own = Learning(db, l.clock, lambda *event: events.append(event), l.redact, l.redact_id,
                               l.redact_value)
                own.secret_values = l.secret_values
                index_summaries(own, stop=lambda: False)
            finally:
                db.close()
        await asyncio.to_thread(index)
        for event in events:
            l.publish(*event)
        return {"status": "done"}

    def _core_profile_text(self) -> str:
        """Tier 1's settled block for every role's instructions (planner and workers; kb area memory)."""
        profile = getattr(self, "profile", None)
        return profile.prompt_text() if profile is not None and self.memory is not None else ""

    def _core_profile_pending(self) -> str:
        """MEM-429: Tier 1's entries newly always on since its block settled, for after each prompt's cached part."""
        profile = getattr(self, "profile", None)
        return profile.pending_text() if profile is not None and self.memory is not None else ""

    def _settle_core_profile(self, why: str) -> None:
        profile = getattr(self, "profile", None)
        if profile is not None:
            profile.settle(why)

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
        """The query's embedding by the local model, computed off the event loop (L28); (None, False) after a failed
        encode, reported once (keyword ranking only); (None, True) without a model or words."""
        embedder = self.learning.embedder if self.learning else None
        if embedder is None or not words.strip():
            return None, True
        try:
            return await asyncio.to_thread(embedder.encode, words, query=True), True
        except Exception as e:                       # reported once; keyword ranking only, no second encode
            self._journal().append("memory.embedding_unavailable", {"error": self._redact_text(str(e))})
            return None, False

    def _memory_search(self, db: Any = None) -> Any:
        """The one fused search (ferb/memory/search.py) on the daemon's connection or `db`."""
        from ferb.memory.search import MemorySearch
        embedder = self.learning.embedder if self.learning else None
        return MemorySearch(db if db is not None else self.db, self.clock,
                            model=getattr(embedder, "name", None) if embedder is not None else None)

    async def _learned_search(self, query: str, **filters: Any) -> list[dict[str, Any]]:
        """memory.learned's search over learned nodes (past tasks with their how-notes, playbooks, day summaries): its
        SQLite reads in a worker thread on a connection of its own, as the re-index's are (final review M3), the
        query's embedding off the loop. What the thread's store reports (an embedding failure) is published after."""
        from ferb.memory.learning import PROMPT_LEAVES_OUT, Learning
        if not self.learning:
            return []
        assert self.db is not None
        await self._search_ready()
        vector, embed = await self._query_vector(query)
        learning, path = self.learning, self.db.path
        told: list[tuple[str, dict[str, Any]]] = []

        def work() -> list[dict[str, Any]]:
            db = Database(path)
            try:
                reader = Learning(db, learning.clock, lambda topic, data: told.append((topic, data)), learning.redact,
                                  redact_id=learning.redact_id)
                reader.embedder, reader.home = learning.embedder, learning.home
                return list(reader.search(query, query_vector=vector, embed=embed, sources=False,
                                          leave_out=PROMPT_LEAVES_OUT, limit=self.MEMORY_LEARNED_MOST, **filters))
            finally:
                db.close()
        found = await asyncio.to_thread(work)
        for topic, data in told:
            self._publish(topic, data)
        return found

    # Ruling (step 9b): a lookup tool ranks at most this many (memory.read, memory.learned); they page through them
    MEMORY_LEARNED_MOST = 100

    async def _memory_item_search(self, query: str) -> list[str]:
        """memory.read's query: the facts themselves (`memory_items`), ranked by the one search."""
        if self.memory is None:
            return []
        vector, _ = await self._query_vector(query)
        return [hit.data.id for hit in self._memory_search().facts(self.memory, query, vector=vector,
                                                                  limit=self.MEMORY_LEARNED_MOST)]

    def _fact_origin(self, item_id: str, item_origin: str) -> str:
        from ferb.memory.learning import LearningRefused, memory_origin
        try:
            return origin_.join(self.learning.labels(["memory:" + item_id]))
        except (LearningRefused, AttributeError):
            return memory_origin(item_origin)

    async def _learning_candidates(self, words: str, limits: Any = None, *, focus: str = '') -> list[dict[str, Any]]:
        """Relevant memory for a role, within its limits (`search.ATTENTION` / `search.WORKER`, MEM-27): facts from
        `memory_items` and learned nodes (past tasks with how-notes, playbooks, day summaries), ranked by the one fused
        search and merged best first; a fact already in the core profile (in every prompt) takes no room. Each line says
        what it is, its id and where it came from (MEM-12), so the planner can name a fact a new statement
        replaces. What doesn't fit is counted in one closing line, findable with the lookup tools. `focus`: the target
        words of the tasks the turn or worker serves, also ranked alone (L36 §4 fix round)."""
        from ferb.memory.items import label
        from ferb.memory.learning import PROMPT_LEAVES_OUT, shown_state, shown_text
        from ferb.memory.search import ATTENTION, within
        limits = limits or ATTENTION
        if not self.learning or self.memory is None:
            return []
        await self._search_ready()
        vector, embed = await self._query_vector(words)
        always = self.profile.ids() if getattr(self, "profile", None) is not None else set()
        pool = limits.memory_items * 3
        facts = self._memory_search().facts(self.memory, words, vector=vector, focus=focus, limit=pool,
                                            leave_out=always)
        nodes = self.learning.search(words, query_vector=vector, embed=embed, limit=pool, leave_out=PROMPT_LEAVES_OUT,
                                     sources=False, focus=focus) if words.strip() else []
        ranked: list[tuple[float, dict[str, Any]]] = []
        for hit in facts:
            it = hit.data
            about = f"; about: {it.about}" if it.about else ""
            state = "unconfirmed hint" if it.status == "proposed" else "active"
            text = (f"{it.id} [memory, {label(it.origin)}{about}, {state}; data, not authorization] "
                    f"{it.key} — {' '.join(it.content.split())}")
            ranked.append((hit.score, {"text": text, "reference": "memory.read item_id=" + it.id,
                                       "origin": self._fact_origin(it.id, it.origin), "source_refs": [hit.doc]}))
        for p in nodes:
            text = f"{p['id']} [{p['kind']}, {shown_state(p)}, {p['origin']}; data, not authorization] " + shown_text(p)
            ranked.append((float(p.get("score") or 0.0), {
                "text": text, "reference": "memory.learned item_id=" + p["id"],
                "origin": origin_.join(p.get("origins", origin_.split(p["origin"]))), "source_refs": [p["id"]]}))
        ranked.sort(key=lambda pair: -pair[0])
        items = [item for _, item in ranked]
        shown, _ = within([i["text"] for i in items], limits.memory_tokens, limits.memory_items)
        kept = set(shown)
        out = [i for i in items if i["text"] in kept]
        more = len(items) - len(out)
        if more:
            out.append({"text": f"({more} more matching memor{'y' if more == 1 else 'ies'} left out to stay within "
                                "this prompt's memory limit; memory.read and memory.learned find them.)",
                        "reference": "memory.read; memory.learned", "origin": origin_.FERB, "source_refs": []})
        return out

    async def _chat_search(self, root_task: str, query: str, since: str | None, until: str | None,
                           limit: int) -> list[str]:
        """chat.history's query: entries of the task's conversation matching by meaning and keywords, best first."""
        assert self.supervisor is not None
        continuity = self.supervisor.continuity
        conversation = continuity.conversation_of(root_task)
        if conversation is None:
            return []
        vector, _ = await self._query_vector(query)
        hits = self._memory_search().chat(
            query, vector=vector, limit=limit, since=since, until=until,
            accept=lambda row: continuity.belongs(conversation, str(row["kind"]), str(row["session"]),
                                                  str(row["request"])))
        return [hit.doc.removeprefix("journal:") for hit in hits]

    # ---------- the local embedding model (MEM-6) ----------

    # Ruling (step 9b): how often the background embedder looks for new text, and its pause between batches
    EMBED_EVERY_S = 60
    EMBED_PAUSE_S = 0.05

    def _embedding_root(self) -> tuple[Path, bool]:
        """Where the model's files are: `[memory] embedding_model` when set, else the copy FERB.app ships
        (Contents/Resources/models, read in place, never fetched), else a checkout's copy under ~/.ferb/models (setup
        fetches it at install); and whether FERB may fetch them there itself (a checkout whose setup couldn't)."""
        from ferb.device import install
        from ferb.memory import embeddings
        configured = str(self.config.get("memory", "embedding_model", "") or "").strip()
        if configured:
            return Path(configured).expanduser(), False
        bundle = install.app_bundle()
        if bundle is not None:
            return embeddings.default_root(bundle / install.BUNDLE_MODELS.parent, embeddings.MODEL), False
        return embeddings.default_root(self.home, embeddings.MODEL), True

    async def _load_embedder(self) -> None:
        """Load the pinned model, fetching it once when FERB keeps its own copy (journaled). Search runs on keywords
        and recency until it is there; a failure is journaled and tried again by the next start."""
        from ferb.memory import embeddings
        if self.learning is None or self.learning.embedder is not None:
            return
        root, fetch = self._embedding_root()
        try:
            if fetch and not root.exists():         # a checkout whose setup couldn't fetch it (FERB.app ships it)
                await asyncio.to_thread(embeddings.download, root, embeddings.MODEL)
                self._journal().append("memory.embedding_model", {"state": "fetched", "path": str(root)})
            self.learning.embedder = await asyncio.to_thread(embeddings.StaticEmbedder, root, embeddings.MODEL)
        except Exception as e:
            self._journal().append("memory.embedding_model", {
                "state": "unavailable", "error": self._redact_text(f"{type(e).__name__}: {e}")[:300]})

    async def _embed_now(self) -> dict[str, Any] | None:
        """Embed what has no vector yet, in a worker thread on a connection of its own (never in a turn); a pass that
        embedded a batch or more is journaled with its cost (`memory.embedded`: counts, encode CPU seconds, wall
        seconds)."""
        import time
        from ferb.memory.search import EMBED_BATCH
        embedder = self.learning.embedder if self.learning else None
        if embedder is None or self.db is None:
            return None
        path, stop = self.db.path, self._reindex_stop

        def work() -> dict[str, Any]:
            db = Database(path)
            try:
                db.conn.execute("PRAGMA busy_timeout = 60000")
                return dict(self._memory_search(db).embed_pending(
                    embedder, stop=stop.is_set, pause=lambda: time.sleep(self.EMBED_PAUSE_S)))
            finally:
                db.close()
        started = time.monotonic()
        out = await asyncio.to_thread(work)
        out["wall_s"] = round(time.monotonic() - started, 3)
        if out["embedded"] >= EMBED_BATCH:
            self._journal().append("memory.embedded", out)
        return out

    async def _embed_loop(self) -> None:
        """Background: the model once, then new text embedded every EMBED_EVERY_S (a cheap check when nothing is new).
        First start of this build: everything already stored is embedded here (upgrade, STORE-403)."""
        await self._load_embedder()
        while not self._shutting_down:
            try:
                await self._embed_now()
            except asyncio.CancelledError:
                raise
            except Exception as e:
                self._journal().append("memory.embed_failed", {"error": self._redact_text(f"{type(e).__name__}: {e}")})
            await asyncio.sleep(self.EMBED_EVERY_S)

    def _forget_item_copies(self, plan: Any) -> None:
        """A forget's work outside the memory tables (`Memory.on_forget`), before they change: the item's own search
        index nodes (`drop_fact_index`: nothing built on them), the copies of the trail rows it came from (today's
        activity file, delegated conversations' evidence), and a pending clarifying question holding its words. The
        login-record usernames it gave are cleared after (L36 §5). FERB's past answers and traces are not scrubbed
        (MEM-2)."""
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
        (`new`) or retired or retracted (None). The core profile follows at once (the new version takes each one's
        place; a retired one leaves it, MEM-7), and the login-record usernames the owner's items among them gave
        are cleared (`_clear_login_names`, queued like a forget's). A newer version whose own login entry names the
        username in the owner's words, in the same turn, has moved them to its spelling first (`_save_login_name`
        takes the username lock before the queued clear runs; review m-8)."""
        if getattr(self, "profile", None) is not None:
            self.profile.follow(ids, new)
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

    def _owner_contacts(self) -> frozenset[str]:
        """The owner's own email addresses and phone numbers (GATE-538, owner D7), by shape: the accounts their paired
        browser profiles are signed in to, and those written in their own words about themselves in memory (owner
        stated, subject owner, holding now)."""
        from ferb.core.types import Identity
        from ferb.policy.confirm import contacts_in
        found: set[str] = set()
        for b in (self.pairings.all() if self.pairings is not None else []):
            if b.identity is Identity.OWNER:
                found |= contacts_in(self._profile_account(b.browser, b.folder))
        for item in (self.memory.items(subject="owner") if self.memory is not None else []):
            if item.origin == "owner_stated":
                found |= contacts_in(item.content)
        return frozenset(found)

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
        """The simple forget (MEM-2), now, on the event loop: `Memory.forget`."""
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

    def _ask_owner(self, kind: str, ident: str, reason: str, request_id: str, approval_id: str | None = None,
                   options: list[str] | None = None) -> str:
        """FERB's own question outside any task: a forget's Yes / No (permanent deletion asks, rule 5), the downloads
        cap, which profile FERB browses in (APP-777, `options`: the owner's profiles and None). A FERB check: one of
        its options answers it, in chat or with the notch's buttons. `_question_answered` does what the answer asks.
        It is kept in `memory_questions` until it is answered, so a restart asks it again (`_restore_memory_questions`;
        rule 9). Its id."""
        from ferb.core.types import Identity
        from ferb.orchestrator.steering import MEMORY_QUESTIONS
        from ferb.policy.rules import ActionRequest
        capability, fixed = MEMORY_QUESTIONS[kind.split(":", 1)[0]]
        action = ActionRequest(capability, "owner_question", Identity.OWNER, frozenset(), "", "", request_id,
                               frozenset())
        ap = self.state.approvals.question(action, reason, list(options if options is not None else fixed), approval_id)
        self._memory_questions().keep(ap.id, kind, ident, reason, request_id, self.clock.now().isoformat())
        self._owner_followups[ap.id] = (kind, ident)
        self._publish("approval.requested", {"approval_id": ap.id, "capability": ap.capability, "kind": ap.kind,
                                             "reason": ap.reason, "task_id": None, "options": list(ap.options)})
        return ap.id

    def _memory_questions(self) -> Any:
        from ferb.memory.questions import MemoryQuestions
        assert self.db is not None
        return MemoryQuestions(self.db)

    def _close_memory_questions(self, targets: Any, kinds: tuple[str, ...] | None = None) -> None:
        """FERB's questions about memory that stopped waiting (re-review NEW-1): of `kinds` (None: every kind) about
        `targets`, which were deleted (a forget's "Forget for good?" about another version, a learned item built on it).
        Each is closed (superseded: never a No), dropped from what its answer would do, and deleted from
        `memory_questions`, so no later turn or restart shows its words again."""
        gone = set(targets)
        if not gone:
            return
        for approval_id, (kind, target) in list(self._owner_followups.items()):
            if target in gone and (kinds is None or kind in kinds):
                self._owner_followups.pop(approval_id, None)
                if self.state.approvals.supersede(approval_id) is not None:
                    self._publish("approval.resolved", {"approval_id": approval_id, "state": "superseded"})
        self._memory_questions().drop_about(gone, kinds)

    def _restore_memory_questions(self) -> None:
        """At start: a forget's Yes / No still waiting is asked again, with its id, words and options; one whose memory
        has gone meanwhile is dropped. Keep / Edit / Drop and Retire / Keep questions an older build left waiting are
        dropped (MEM-12: FERB asks none; the guess they were about is already saved as FERB's understanding)."""
        questions = self._memory_questions()
        questions.drop_kinds(GONE_QUESTIONS)
        for row in questions.waiting():
            if row["kind"] == "browsing":
                gone = True                              # APP-777: the next task that needs it asks, with fresh profiles
            elif row["kind"].startswith("downloads:"):
                gone = False                             # the downloads folder's cap: asked again as it was
            elif row["kind"] == "forget:learning":
                gone = self.learning is None or self.learning.get(row["target"]) is None
            else:
                gone = self.memory is None or self.memory.get(row["target"]) is None
            if gone:
                questions.answered(row["approval_id"])
            elif self.state.approvals.get(row["approval_id"]) is None:
                self._ask_owner(row["kind"], row["target"], row["reason"], row["request_id"], row["approval_id"])

    def _question_answered(self, ap: Any) -> str:
        """FERB's own follow-up to an answered question: a chat forget on its Yes (I2). A line for the owner, or ''."""
        followup = self._owner_followups.pop(ap.id, None)
        if followup is None:
            return ""
        self._memory_questions().answered(ap.id)
        what, ident = followup
        if what.startswith("downloads:"):
            return self._downloads_answered(ap, ident)
        if what == "browsing":
            return self._browsing_answered(ap, ident)
        from ferb.memory.items import MemoryRefused
        if not (ap.state == "granted" and ap.choice == "Yes"):
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
        if op == "backlog":                # MEM-12: FERB keeps no guesses waiting (an older app asks)
            return {"backlog": None}
        if op in ("guess", "guess_state"):
            raise LearningRefused("FERB no longer asks about its guesses: it saves what it understands, labelled as "
                                  "its understanding; correct it by telling FERB or on the memory page")
        if op == "inventory":                # learned nodes without their source refs (fix list, step 9b)
            return {"items": [n | {"confirmable": not n["confirmed"] and n["kind"] == "identity_proposal"}
                              for n in l.items(history=bool(params.get("history")), sources=False)]}
        if op == "search":             # its SQLite reads and the query's embedding off the event loop
            return {"items": await self._learned_search(str(params.get("query", "")), **{
                k: str(params[k]) for k in ("kind", "app", "person", "since", "until") if params.get(k)})}
        if op == "get":
            node = l.get(ident, sources=False)
            if node is not None:
                node["source_count"] = int(self.db.conn.execute(
                    "SELECT COUNT(*) FROM learning_sources WHERE node_id = ?", (ident,)).fetchone()[0])
            return {"item": node, "history": l.versions(node["kind"], node["key"]) if node else []}
        if op == "confirm":
            node = l.get(ident)
            if node and node["kind"] == "fact":           # MEM-12: a memory item is never waiting for a Keep
                raise LearningRefused("memories need no confirming; correct one with edit or forget")
            return {"item": l.confirm(ident)}
        if op == "edit":
            node = l.get(ident)
            if node and node["kind"] == "fact":
                item_id = node["body"]["id"]
                return {"item": await self._memory_edit(session, {"id": item_id,
                                                                  "content": str(params.get("text", ""))})}
            return {"item": l.edit(ident, str(params.get("text", "")))}
        if op == "retire":
            node = l.get(ident)
            if node and node["kind"] == "fact":           # fix list: the owner's "Not right" on a fact retires the item
                from ferb.memory.items import MemoryRefused
                item_id = str(node["body"]["id"])
                try:
                    self.memory.retire(item_id, "the owner, on the memory page")
                except MemoryRefused:
                    raise LearningRefused("that memory no longer holds") from None
                return {"item": self._item_dict(self.memory.get(item_id))}
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
        if op == "eval_export":
            from ferb.eval.memory import history_queries
            return history_queries(l)
        if op == "embedding":            # MEM-6: the pinned local model, always on once its files are here
            embedder = l.embedder
            root, fetch = self._embedding_root()
            return {"active": getattr(embedder, "name", None), "path": str(root), "fetched_by_ferb": fetch,
                    "vectors": int(self.db.conn.execute("SELECT COUNT(*) FROM recall_vectors").fetchone()[0])}
        raise LearningRefused("unknown learning operation")

    def _record_learning_usage(self, role: str) -> Callable[[str, dict[str, Any]], None]:
        """A learner's usage recorder: each usage event is a `model.usage` span naming that learner's role (distill)."""
        def record(runtime: str, data: dict[str, Any]) -> None:
            span = self.spans.start("model.usage", task_id=None, parent_id=None,
                                    input={"runtime": runtime, "role": role})
            self.spans.finish(span, output=data, cost_usd=data.get("cost_usd"))
        return record

    async def _distill_loop(self) -> None:
        """Hourly: summarize and distill every past day not done yet, then purge (summarized days only)."""
        while True:
            await self._memory_pass()
            await asyncio.sleep(3600)

    async def _memory_pass(self) -> None:
        """One hourly pass: distill, refresh episode cards, merge playbooks, then (when due) the weekly core-profile
        pass and the weekly re-check of what FERB saw on this Mac. Each job has its own error scope, so
        one failing never stops the others (L16); a failure never takes the daemon down and is tried again next hour."""
        assert self.distiller is not None
        jobs: list[tuple[str, Callable[[], Awaitable[Any]]]] = [("memory.distill_failed", self.distiller.catch_up)]
        if self.learning is not None:
            jobs.append(("memory.cards_failed", self._refresh_cards))
        if self.learning is not None:
            jobs.append(("memory.playbooks_failed", self._merge_playbooks))
        jobs.append(("memory.profile_failed", self._profile_pass))
        jobs.append(("memory.mac_profile_failed", self._refresh_mac_profile))
        for failed, job in jobs:
            try:
                await job()
            except asyncio.CancelledError:
                raise
            except Exception as e:
                self._journal().append(failed, {"error": f"{type(e).__name__}: {e}"})

    async def _merge_playbooks(self) -> list[str]:
        """Tier 3 playbooks (step 9b): verified how-notes of the same kind of work merged, a few model calls a pass,
        only while no task runs and once the local model is there (it finds the candidates)."""
        from ferb.memory.playbooks import merge
        if self.learning is None or self.learning.embedder is None:
            return []
        if self.supervisor is not None and self.supervisor.running_trees():
            return []
        return await merge(self.learning, self._make_runtime, self.learning.embedder,
                           device_id=str(self.config.get("device", "id", "dev")), spans=self.spans,
                           record=lambda kind, data: self._journal().append(kind, data),
                           usage=self._record_learning_usage("playbook"))

    async def _memory_repeats(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """"Gets better at repeated tasks", measured: each playbook's runs (attempts, time, verified, memory used)."""
        from ferb.memory.playbooks import repeat_stats
        assert self.db is not None
        return {"playbooks": repeat_stats(self.db.conn)}

    async def _memory_restore(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """MEM-14: the owner says a fact FERB set aside still holds (the memory page's Restore)."""
        item_id = str(params.get("id") or "")
        if self.memory.get(item_id) is None:
            raise FerbError("no memory with that id")
        restored = self.memory.restore(item_id)
        return {"restored": restored, "item": self._item_dict(self.memory.get(item_id))}

    async def _profile_pass(self, *, force: bool = False) -> dict[str, Any] | None:
        """The weekly core-profile pass (MEM-7), when due (`CoreProfile.due`: a week since the last, or the profile
        over its budget) or forced (the `memory.profile` RPC's op "pass"). Facts it found out of date are told once,
        quietly (MEM-12)."""
        profile = getattr(self, "profile", None)
        if profile is None or not (force or profile.due()):
            return None
        out = await profile.run_pass(self._make_runtime, device_id=str(self.config.get("device", "id", "dev")),
                                     spans=self.spans, usage=self._record_learning_usage("profile"))
        ended = [e for e in out.get("ended_items") or [] if e.get("outcome") != "set_aside"]
        aside = [e for e in out.get("ended_items") or [] if e.get("outcome") == "set_aside"]
        stamp = self.clock.now().isoformat()
        if ended:
            text = ENDED_NOTICE.format(items=", ".join(f"“{e['text']}”" for e in ended))
            await self._tell_owner(f"memory-profile-ended-{stamp}", ["memory:" + e["id"] for e in ended], text,
                                   origin_.join({origin_.FERB, origin_.INFERRED}))
        if aside:                                    # MEM-14: the owner's words, told once, kept
            text = SET_ASIDE_NOTICE.format(items=", ".join(f"“{e['text']}”" for e in aside))
            await self._tell_owner(f"memory-profile-aside-{stamp}", ["memory:" + e["id"] for e in aside], text,
                                   origin_.join({origin_.FERB, origin_.OWNER}))
        return out

    def _now_always_on(self, items: list[Any]) -> None:
        """MEM-15, MEM-423: facts of FERB's understanding or observation (the Mac scan's too) that just joined the core
        profile, each told once by a quiet notice with its note card (the 8e note card, `always_on`), Correct / Forget
        on it; never a question."""
        from ferb.memory.items import label
        from ferb.orchestrator import cards
        for it in items:
            text = " ".join(it.content.split())
            line = f"Now always on: “{text}” ({label(it.origin)})"
            card = cards.note(it.id, text, "saved", line) | {"label": label(it.origin), "origin": it.origin,
                                                            "always_on": True}
            self._spawn(self._tell_owner(f"memory-always-on-{it.id}", ["memory:" + it.id], line,
                                         origin_.join({origin_.FERB, origin_.INFERRED}), cards=[card]),
                        "always-on notice")

    async def _refresh_mac_profile(self) -> None:
        """MEM-13, "refreshed as usage changes": weekly, with the profile pass's cadence (Ruling), FERB gathers the
        Mac's facts again and re-interprets them only when they changed; quiet."""
        from ferb.memory.profile import PASS_DAYS
        if self._shutting_down:
            return
        review = self._mac_profile_service()
        seen = review.state.seen() if review is not None else {}
        if not seen:
            return
        checked = seen.get("checked_at") or seen.get("at")
        if checked and self.clock.now() - datetime.fromisoformat(str(checked)) < timedelta(days=PASS_DAYS):
            return
        await review.refresh()

    def _export_memory(self) -> None:
        from ferb.memory.export import export
        if self.memory is not None:
            export(self.memory, self.home / "memory")

    async def _remember(self, items: list[Any], own_words: str, source_id: str,
                        sites: frozenset[str] = frozenset(),
                        elsewhere: frozenset[str] = frozenset(), about: str = "") -> list[dict[str, Any]]:
        """Chat capture (memory review I1, MP2; the check is `items.capture`, shared with distillation's quotes, L36).
        `own_words`: what the owner wrote (`untrusted.mark`'s own words). The quote must be whole words of it. Saved as
        owner_stated ("your words") is only the owner's own text: the clause(s) of their words that hold the planner's
        content (`owner_clauses`, MEM-419: the span that states the fact, never the whole message). Any other content is
        FERB's wording of what they said, saved at once as FERB's understanding (MEM-12: no Keep question); one
        whose quote shares no word with it rests on nothing the owner said and is refused. Either rests on the owner's
        words now, so it replaces every current version of its fact (same kind, subject and key) and the items the
        planner named in `replaces` (code checks each holds), and FERB's line names what it replaced. `about`: what the
        words are about (MEM-10: the turn's task, else the item's key), stored next to them. Per saved item: its
        receipt, the quiet "Memory: noted …" line code adds to the reply and its note card; per refused item: why, in
        plain words (owner 2026-10-24), and `guard` when the quote check refused it (the planner's line then says a
        safety check stopped it, ORCH-519). An entry with `login_origin` also gives that site's login record its username
        (`_save_login_name`, in a worker thread; `sites`: the sites the turn's task names, L36 §5; `elsewhere`: those
        the owner typed only for a task of another chat, which a refusal names). A `review_in_days` puts a check date on
        a period fact without an end date in the owner's words."""
        from ferb.memory.items import CAPTURE_TOLD, capture, label
        from ferb.orchestrator import cards
        saved: list[dict[str, Any]] = []
        for it in items:
            got = capture(own_words, str(it.content), str(it.quote))
            if got.refused:
                self._journal().append("memory.capture_refused", {"key": it.key, "why": got.refused})
                saved.append({"refused": CAPTURE_TOLD[got.refused], "guard": got.refused})
                continue
            topic = " ".join(str(about or str(it.key).replace("_", " ").replace("-", " ")).split())
            try:
                done = self.memory.save(it.kind, it.subject, it.key, got.content, got.origin,
                                        [("journal", source_id)], 1.0 if got.origin == "owner_stated" else 0.7,
                                        replaces=[self._memory_id(r) for r in getattr(it, "replaces", None) or []],
                                        about=topic, from_owner=True)
            except FerbError as e:
                self._journal().append("memory.capture_refused", {"key": it.key, "why": str(e)})
                saved.append({"refused": reason(e, self.redact)})
                continue
            m = done.item
            if done.not_replaced:
                self._journal().append("memory.replace_refused", {"item": m.id, "ids": done.not_replaced})
            days = int(getattr(it, "review_in_days", 0) or 0)
            if days > 0 and m.valid_to is None:
                self.memory.set_review(m.id, self.clock.now() + timedelta(days=days))
            text = " ".join(m.content.split())
            ends = (" " + self._until(it.until, m.id, own_words)) if getattr(it, "until", None) is not None else ""
            stopped, ends = (ends.strip(), "") if ends.startswith(" \n") else ("", ends)   # a safety check's own line
            told = (await self._save_login_name(it, m.id, own_words, sites, m.origin, elsewhere)
                    if getattr(it, "login_origin", None) else "")
            told = "\n".join(part for part in (told, stopped) if part)
            replaced = "; ".join(f"“{' '.join(o.content.split())}”" for o in done.replaced)
            line = (f"Memory: noted “{text}” ({label(m.origin)}){ends}"
                    + (f" — replaces {replaced}" if replaced else ""))
            receipt = (f"remembered ({m.id}, {label(m.origin)}): {m.content}"
                       + (f" (replaces {', '.join(o.id for o in done.replaced)})" if done.replaced else ""))
            # step 8e: each saved item's note card, from the stored item (its id is what Correct / Forget act on)
            card = cards.note(m.id, text, "saved", line) | {"label": label(m.origin), "origin": m.origin,
                                                          "replaces": [o.id for o in done.replaced]}
            saved.append({"receipt": receipt, "line": line + (f"\n{told}" if told else ""), "card": card})
        return saved

    def _memory_id(self, ident: str) -> str:
        """A memory id the planner named: a memory item's id, or the fact node it was shown as."""
        ident = str(ident).strip()
        if self.memory is not None and self.memory.get(ident) is not None:
            return ident
        node = self.learning.get(ident) if self.learning is not None and ident else None
        return str(node["body"].get("id", ident)) if node and node["kind"] == "fact" else ident

    def _until(self, until: Any, item_id: str, own_words: str) -> str:
        """A remembered item's last day from the owner's own date words (L36 §6): the quote must be whole words of what
        they typed; code works out the day (`end_of_day`: the model never computes dates, owner 2026-09-30) and the
        item leaves memory after it, whatever its origin. A day already over, or date words the owner didn't type,
        save it without an end, said in the line and journaled (`memory.until_refused`). A line for the owner: date words
        the owner didn't write are the quote check's, said as a safety check on a line of its own (starting "\\n")."""
        from ferb.clock.when import When, WhenError, end_of_day, local_zone
        from ferb.memory.items import quoted
        if not quoted(own_words, str(until.quote)):
            from ferb.policy import blocked as blocked_
            self._journal().append("memory.until_refused", {"item": item_id, "why": "date words not typed"})
            return "\n" + blocked_.blocked(blocked_.END_DATE, "the date words I quoted from you aren't in what you wrote",
                                           "it's saved without an end date", "tell me when it ends and I'll add it")
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

    def _item_dict(self, it: Any, conflict: bool = False, imported: list[str] | None = None) -> dict[str, Any]:
        from ferb.memory.import_extract import imported_from
        from ferb.memory.items import label
        if imported is None:
            imported = imported_from(self.db, [it.id]).get(it.id, [])
        in_profile = self.profile.ids() if getattr(self, "profile", None) is not None else set()
        return {"id": it.id, "kind": it.kind, "subject": it.subject, "key": it.key, "content": it.content,
                "origin": it.origin, "label": label(it.origin), "about": it.about, "status": it.status,
                "valid_from": it.valid_from.isoformat(),
                "valid_to": it.valid_to.isoformat() if it.valid_to else None, "supersedes": it.supersedes,
                "review_at": it.review_at.isoformat() if it.review_at else None, "in_profile": it.id in in_profile,
                "set_aside": it.set_aside,
                "sources": it.source_ids, "evidence_days": self.memory.evidence_days(it.id), "conflict": conflict,
                "imported_from": imported}

    async def _memory_profile(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """The app's memory page (MEM-12: it shows and edits everything): the core profile as markdown and as
        sections, its budget and use. op "add" (an item into the profile, the owner's choice), "remove" (out of it; the
        memory stays), "pass" (run the weekly pass now). Facts themselves change through memory.edit / retract /
        forget."""
        op, ident = str(params.get("op") or "get"), str(params.get("id") or "")
        if op == "add":
            try:
                self.profile.add(ident, str(params.get("section") or ""), by="owner")
            except ValueError as e:
                raise FerbError(str(e)) from None
        elif op == "remove":
            self.profile.remove([ident], by="owner")
        elif op == "pass":
            await self._profile_pass(force=True)
        elif op != "get":
            raise FerbError("unknown memory.profile op: get, add, remove or pass")
        return self.profile.view()

    async def _memory_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Every memory item for the memory page and the CLI, each with its label, whether it is in the core profile
        and which assistants it was imported from; `history` also lists ended versions; "all" every status;
        `imported_from`: only items imported from that source ("any": from any)."""
        from ferb.memory.import_extract import imported_from
        status = params.get("status", "active")
        items = self.memory.items(None if status == "all" else status, params.get("subject"),
                                  valid_now=not params.get("history"))
        where = imported_from(self.db, [i.id for i in items])
        wanted = str(params.get("imported_from") or "")
        if wanted:
            items = [i for i in items if (where.get(i.id) if wanted == "any" else wanted in where.get(i.id, []))]
        return {"items": [self._item_dict(i, imported=where.get(i.id, [])) for i in items]}

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

    async def _memory_edit(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """The owner's correction (the note card's Correct, the memory page): their words become a new version, saved
        as theirs; words redaction would hide whole are refused, never saved."""
        if _all_hidden(self._redact_text(str(params["content"]))):
            raise FerbError(EDIT_HIDDEN)
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
        limit = max(1, int(params.get("limit") or TIMELINE_ROWS))
        rows = self._journal().entries(since, limit + 1)             # bounded: never the whole journal (step 12a)
        return {"entries": [{"at": e.at.isoformat(), "kind": e.kind, "data": e.data} for e in rows[-limit:]],
                "more": len(rows) > limit}

    def _mac_profile_service(self) -> Any:
        """What FERB saw on this Mac (MEM-13): built once per daemon, its collector reading only owner-attributed
        app names and counts (no window titles or content) on the database thread."""
        if self.persona is None:
            from ferb.daemon.persona import PersonaReview
            self.persona = PersonaReview(self.config, None, build=self._build_mac_profile,
                                         record=lambda kind, data: self._journal().append(kind, data),
                                         tell=self._tell_mac_profile, tell_joined=self._tell_mac_joins,
                                         redact=self.redact,
                                         clock=lambda: self.clock.now().isoformat())
        if self.persona_scan is not None and self.persona.collect is None:
            scan = self.persona_scan

            def collect() -> Any:
                # read when FERB gathers (not at every chat attach): owner-attributed focus counts, on the loop
                activity = [dict(r) for r in self.db.conn.execute(
                    "SELECT json_extract(data_json,'$.bundle_id') AS bundle_id, "
                    "json_extract(data_json,'$.app') AS app, COUNT(*) AS focus_events, MAX(at) AS last_used "
                    "FROM trail WHERE source='app_focus' AND actor='owner' GROUP BY bundle_id")]
                device = self.config.get('device', 'id')
                if not device:
                    device = self.ids.new_id()
                    self.config.set('device', 'id', device)
                    self.device_id = str(device)
                return scan(self.home, device, activity, self.config.exclude_apps())
            self.persona.collect = collect
        return self.persona

    def _prepare_persona(self, refresh: bool = False, shown_by: str = "chat") -> dict[str, Any]:
        if self._shutting_down:
            return {'state': 'closed'}
        return dict(self._mac_profile_service().start(refresh=refresh, shown_by=shown_by))

    async def _build_mac_profile(self, facts: dict[str, Any]) -> dict[str, Any]:
        """MEM-13: a model interprets the gathered facts; code stores them as observed memory. They join the core
        profile at once (MEM-423) without a notice each (MEM-427): `joined` lists the stored facts that are newly in it
        (a join, or a new version taking an entry's place), for `PersonaReview` to tell as one notice when nothing
        else shows them."""
        from ferb.memory.mac_profile import interpret, store
        answer = await interpret(self._make_runtime, facts, device_id=str(self.config.get("device", "id", "dev")),
                                 spans=self.spans, usage=self._record_learning_usage("mac_profile"))
        before = self.profile.ids()
        with self.profile.holding_joins():
            stored = store(self.memory, lambda kind, data: self._journal().append(kind, data), facts, answer)
        now = self.profile.ids() - before
        return stored | {"joined": [f["id"] for f in stored["facts"] if f["id"] in now]}

    async def _tell_mac_joins(self, ids: list[str]) -> None:
        """MEM-427: a re-check's scan facts newly in the core profile, told as ONE quiet notice carrying a note card
        each (Correct / Forget), its line as one join's (`_now_always_on`)."""
        from ferb.memory.items import label
        from ferb.orchestrator import cards
        items = [it for i in ids if (it := self.memory.get(i)) is not None]
        if not items:
            return
        texts = [" ".join(it.content.split()) for it in items]
        line = "Now always on: " + ", ".join(f"“{t}”" for t in texts) + f" ({label(items[0].origin)})"
        made = [cards.note(it.id, t, "saved", line) | {"label": label(it.origin), "origin": it.origin,
                                                       "always_on": True} for it, t in zip(items, texts)]
        await self._tell_owner(f"memory-always-on-scan-{self.clock.now().isoformat()}",
                               ["memory:" + it.id for it in items], line,
                               origin_.join({origin_.FERB, origin_.INFERRED}), cards=made)

    async def _tell_mac_profile(self, text: str) -> None:
        """Shown once in chat when the setup step didn't show it (MEM-13: "here's what I saw", no yes/no)."""
        await self._tell_owner(f"memory-mac-profile-{self.clock.now().isoformat()}", [], text,
                               origin_.join({origin_.FERB, origin_.PRIVATE}))

    async def _setup_persona(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """The setup step (MEM-13): {} starts what FERB saw on this Mac, once; {"refresh": true} again;
        {"status": true} only reads it. The answer's state "seen" carries the summary to show and the facts saved."""
        if params.get('status') and self.persona is not None:
            return dict(self.persona.status())
        return self._prepare_persona(refresh=params.get('refresh') is True, shown_by="setup")

    def _time_meanings(self) -> list[tuple[str, str]]:
        """The owner's own meanings of fuzzy time words ("next friday", "evening"), stored when they corrected FERB."""
        if self.memory is None:
            return []
        return [(it.key.removeprefix("time_").replace("_", " "), it.content)
                for it in self.memory.items("active", "owner") if it.kind == "preference" and it.key.startswith("time_")]
