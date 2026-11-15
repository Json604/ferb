"""Fixtures for the Rust port of memory's items, profile, learning, distill, playbooks, import job and the rest
(rust/crates/ferb-learning), made by running the Python code (ENG-3).

    cd <repo> && uv run python rust/fixtures/learning/make.py

Each scenario starts a migrated store in a temporary FERB home (FERB_HOME and HOME point into it; nothing reads or
writes ~/.ferb), wires Memory, CoreProfile, Learning, Journal, Distiller and the rest the way the daemon does
(ferb/daemon/areas/memory.py), with a fake clock, sequential ids, a fake redactor (it masks "SECRET-" words) and a
scripted model, then runs a list of steps. For every step it records what the call returned or raised, the events
told (publish, hooks, model calls, spans), and every table that changed with its rows. The Rust test runs the same
steps and compares. `pure.json` holds case lists for the pure helpers.

Restart runs: the import job and the daily distill are run once while every write statement is counted; then once
per write with that write failing ("injected"), followed by a fresh run: what Python leaves behind is recorded.

PYTHONHASHSEED=0 and TZ="FX-05:30" (no daylight saving). All data is invented. The same bytes on every run."""

import asyncio
import hashlib
import json
import os
import re
import sqlite3
import sys
import tempfile
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from types import SimpleNamespace
from typing import Any

if os.environ.get("PYTHONHASHSEED") != "0" or os.environ.get("TZ") != "FX-05:30":
    os.environ["PYTHONHASHSEED"] = "0"
    os.environ["TZ"] = "FX-05:30"
    os.execv(sys.executable, [sys.executable, *sys.argv])

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
ROOT = Path(tempfile.mkdtemp(prefix="ferblearn-"))


def _ranks() -> bytes:
    """o200k_base.tiktoken as the tiktoken-rs crate the Rust port embeds carries it (rust/fixtures/context/make.py does
    the same), read before HOME moves, so Python counts tokens with the real tokenizer."""
    import glob
    pattern = str(Path.home() / ".cargo" / "registry" / "src" / "*" / "tiktoken-rs-0.12.1" / "assets" /
                  "o200k_base.tiktoken")
    for found in sorted(glob.glob(pattern)):
        return Path(found).read_bytes()
    raise SystemExit("o200k_base.tiktoken from tiktoken-rs 0.12.1 not found: build ferb-context once (cargo fetch)")


(ROOT / "home" / "cache" / "tokenizer").mkdir(parents=True)
(ROOT / "home" / "cache" / "tokenizer" / "o200k_base.tiktoken").write_bytes(_ranks())
os.environ["FERB_HOME"] = str(ROOT / "home")
os.environ["HOME"] = str(ROOT / "user")
(ROOT / "user").mkdir()
tempfile.tempdir = str(ROOT)
time.tzset()
sys.dont_write_bytecode = True
sys.path.insert(0, str(REPO))

import ferb.policy.security_digest as security_digest_mod  # noqa: E402
from ferb.core.ids import SequentialIds  # noqa: E402
from ferb.core.types import ModelLimits, RunHandle, RuntimeEvent  # noqa: E402
from ferb.memory import distill, export, import_extract, import_job, items, learning, lineage, mac_profile  # noqa: E402
from ferb.memory import playbooks, profile, questions, summary, upgrade  # noqa: E402
from ferb.memory.journal import Journal, journal_origin  # noqa: E402
from ferb.memory.tools import LearningRead  # noqa: E402
from ferb.store.db import Database  # noqa: E402

T0 = datetime(2026, 10, 20, 9, 0, 0, tzinfo=timezone.utc)
SEALED = "zebra-quilt-4417"           # an invented saved value the learning store must never hold


def fake_redact(text: str) -> str:
    return re.sub(r"SECRET-[A-Za-z0-9]+", "[redacted]", text)


def redact_walk(value: Any) -> Any:
    if isinstance(value, dict):
        return {k: redact_walk(v) for k, v in value.items()}
    if isinstance(value, list):
        return [redact_walk(v) for v in value]
    if isinstance(value, str):
        return fake_redact(value)
    return value


def iso(dt: datetime | None) -> str | None:
    return dt.isoformat() if dt is not None else None


def item_json(it: Any) -> Any:
    if it is None:
        return None
    return {"id": it.id, "kind": it.kind, "subject": it.subject, "key": it.key, "content": it.content,
            "source_ids": list(it.source_ids), "observed_at": iso(it.observed_at), "valid_from": iso(it.valid_from),
            "valid_to": iso(it.valid_to), "supersedes": it.supersedes, "confidence": it.confidence,
            "origin": it.origin, "status": it.status, "about": it.about, "review_at": iso(it.review_at),
            "set_aside": it.set_aside}


def raised(e: BaseException) -> dict[str, Any]:
    return {"raises": type(e).__name__, "message": str(e)}


WRITES = ("INSERT", "UPDATE", "DELETE", "REPLACE")


class Conn:
    """The store's connection, counting write statements and failing the one a restart run names."""

    def __init__(self, conn: sqlite3.Connection, env: "Env") -> None:
        self._conn, self._env = conn, env

    def _check(self, sql: str) -> None:
        if sql.lstrip().upper().startswith(WRITES):
            self._env.writes += 1
            if self._env.fail_at is not None and self._env.writes == self._env.fail_at:
                raise sqlite3.OperationalError("injected")

    def execute(self, sql: str, params: Any = ()) -> Any:
        self._check(sql)
        return self._conn.execute(sql, params)

    def executemany(self, sql: str, params: Any) -> Any:
        self._check(sql)
        return self._conn.executemany(sql, params)

    def __getattr__(self, name: str) -> Any:
        return getattr(self._conn, name)


class Clock:
    def __init__(self) -> None:
        self.t = T0

    def now(self) -> datetime:
        return self.t


class Spans:
    def __init__(self, env: "Env") -> None:
        self.env, self.n = env, 0

    def start(self, kind: str, task_id: Any = None, input: Any = None, parent_id: Any = None) -> Any:
        self.n += 1
        span = SimpleNamespace(id=f"span-{self.n}")
        self.env.ev("span.start", {"id": span.id, "kind": kind, "task_id": task_id, "parent_id": parent_id,
                                   "input": input})
        return span

    def finish(self, span: Any, output: Any = None, error: Any = None) -> None:
        self.env.ev("span.finish", {"id": span.id, "output": output, "error": error})


LIMITS = ModelLimits("fake-model", 1_000_000, 100_000, "test double")
TIGHT = ModelLimits("fake-model", 1_000_000, 100_000, "test double", max_input_chars=300)


class Runtime:
    name = "fake"
    model = "fake-model-1"

    def __init__(self, env: "Env", answer: Any) -> None:
        self.env, self.answer = env, answer
        self.limits = TIGHT if isinstance(answer, dict) and answer.get("tight") else LIMITS

    async def start(self, task: Any, request: Any, token: str) -> RunHandle:
        if isinstance(self.answer, dict) and self.answer.get("dynamic") == "import":
            ctx = request.parts[0]
            self.answer = {"answer": verify_answer(ctx) if "\nCandidate v" in ctx else recall_answer(ctx)}
        self.env.ev("model.start", {"answer": self.answer, "task": task.id, "goal": task.goal, "parent": task.parent_id,
                                    "request_id": task.request_id, "role": task.role, "device": task.device_id,
                                    "system_sha": hashlib.sha256(request.system.encode()).hexdigest(),
                                    "parts": list(request.parts), "cacheable": request.cacheable, "token": token})
        return RunHandle("r", "fake", task.id, {})

    async def events(self, run: RunHandle) -> Any:
        a = self.answer
        yield RuntimeEvent("usage", {"input_tokens": 7, "output_tokens": 3})
        if "error" in a:
            yield RuntimeEvent("error", {"message": a["error"]})
        elif "invalid" in a:
            yield RuntimeEvent("invalid_answer", {"message": a["invalid"]})
        elif "text" in a:
            yield RuntimeEvent("final", {"text": a["text"]})
        else:
            yield RuntimeEvent("final", {"text": json.dumps(a["answer"])})

    async def cancel(self, run: RunHandle) -> None:
        self.env.ev("model.cancel", {})


def schema_sha(schema: Any) -> str:
    return hashlib.sha256(json.dumps(schema, sort_keys=True).encode()).hexdigest()


DIGESTS: dict[str, Any] = {}
_digest = security_digest_mod.build


def recorded_digest(db: Any, day: str) -> dict:
    out = _digest(db, day)
    DIGESTS[day] = out
    return out


security_digest_mod.build = recorded_digest


class Env:
    def __init__(self, name: str) -> None:
        self.events: list[Any] = []
        self.writes, self.fail_at = 0, None
        folder = ROOT / name
        folder.mkdir(parents=True, exist_ok=True)
        self.folder = folder
        self.db = Database(folder / "ferb.db")
        self.db.migrate()
        self.db.conn = Conn(self.db.conn, self)     # type: ignore[assignment]
        self.db.on_wal_kept = lambda after, info: self.ev("wal_kept", {"after": after, **info})
        self.clock = Clock()
        self.ids = SequentialIds("id")
        self.answers: list[Any] = []
        self.busy_polls = 0
        self.spans = Spans(self)
        self.journal = Journal(self.db, self.clock, self.ids, redact_walk)
        self.memory = items.Memory(self.db, self.clock, self.ids, self.publish,
                                   on_change=lambda: self.ev("memory.changed", {}), redact=fake_redact)
        self.learning = learning.Learning(self.db, self.clock, self.publish, fake_redact)
        self.learning.secret_values = lambda: [SEALED]
        self.profile = profile.CoreProfile(self.memory, self.clock,
                                           record=lambda k, d: self.journal.append(k, d))
        self.profile.on_change = lambda: self.ev("profile.changed", {})
        self.profile.on_joined = lambda its: self.ev("profile.joined", {"ids": sorted(i.id for i in its)})
        m = self.memory
        m.on_forget = self._on_forget
        m.on_settled = lambda ids, deleted: self.ev("memory.settled", {"ids": list(ids), "deleted": deleted})
        m.on_ended = self._on_ended
        m.on_saved = self._on_saved
        m.on_set_aside = self._on_set_aside
        m.rebuild_summary = lambda day: summary.rebuild(self.db, self.journal, day)
        self.distiller = distill.Distiller(self.db, self.journal, m, self.make_runtime, self.clock,
                                           purge=lambda: self.ev("purge", {}), device_id="dev-1",
                                           record_usage=self.usage, spans=self.spans)
        self.distiller.tell = self.tell
        self.importer = self.new_importer()
        self.prepared: Any = None

    def new_importer(self) -> Any:
        job = import_job.ImportJob(self.memory, make_runtime=self.make_runtime, redact=fake_redact,
                                   append=lambda k, d: self.journal.append(k, d), publish=self.publish,
                                   tell=self.tell, similar=self.similar, busy=self.busy, new_id=self.ids.new_id,
                                   spans=self.spans, usage=self.usage, device_id="dev-1", home=ROOT / "user")
        job.idle_poll = 0
        return job

    def ev(self, name: str, data: Any) -> None:
        self.events.append([name, data])

    def publish(self, kind: str, data: Any) -> None:
        self.ev("publish", {"kind": kind, "data": data})

    def usage(self, name: str, data: Any) -> None:
        self.ev("usage", {"runtime": name, "data": data})

    async def tell(self, ident: str, refs: list[str], text: str, origin: str, **kw: Any) -> None:
        self.ev("tell", {"id": ident, "refs": refs, "text": text, "origin": origin, **kw})

    def busy(self) -> bool:
        if self.busy_polls > 0:
            self.busy_polls -= 1
            return True
        return False

    async def similar(self, query: str) -> list[str]:
        words = set(items.query_terms(query))
        rows = self.db.conn.execute("SELECT id, content FROM memory_items ORDER BY seq").fetchall()
        return [str(r[0]) for r in rows if words & set(items.query_terms(str(r[1])))]

    def make_runtime(self, role: str, mode: str = "", tool_names: Any = None, schema: Any = None) -> Runtime:
        self.ev("make_runtime", {"role": role, "mode": mode, "tools": tool_names, "schema": schema_sha(schema)})
        if not self.answers:
            raise RuntimeError("no scripted answer left")
        return Runtime(self, self.answers.pop(0))

    def _on_forget(self, plan: Any) -> None:
        self.ev("memory.on_forget", {"items": list(plan.items), "trail": list(plan.trail)})
        for item_id in plan.items:
            self.learning.drop_fact_index("memory:" + item_id, self.memory)

    def _on_ended(self, ids: list[str], new: Any) -> None:
        self.ev("memory.ended", {"ids": list(ids), "new": new})
        self.profile.follow(ids, new)

    def _on_saved(self, it: Any) -> None:
        self.ev("memory.saved", {"id": it.id})
        self.profile.saved(it)

    def _on_set_aside(self, ids: list[str]) -> None:
        self.ev("memory.set_aside", {"ids": list(ids)})
        self.profile.remove(ids, by="code")


TABLES = ["memory_items", "memory_sources", "memory_context", "memory_set_aside", "memory_forgotten",
          "memory_profile", "known_destinations", "trail", "journal", "learning_nodes", "learning_sources",
          "learning_changes", "forgotten_refs", "learning_card_inputs", "learning_digests",
          "learning_search_version", "learning_fts", "learning_titles", "distill_runs", "memory_import_runs",
          "memory_import_done", "memory_import_items", "memory_questions"]


def dump(env: Env) -> dict[str, Any]:
    out = {}
    for t in TABLES:
        cur = env.db.conn._conn.execute(f"SELECT * FROM {t} ORDER BY rowid")
        out[t] = [list(r) for r in cur.fetchall()]
    idx = env.db.conn._conn.execute("SELECT 1 FROM sqlite_master WHERE name = 'learning_source_ref'").fetchone()
    out["source_ref_index"] = [[idx is not None]]
    return out


def j(x: Any) -> Any:
    """A result as JSON (Python's own types written out)."""
    if isinstance(x, items.Saved):
        return {"item": item_json(x.item), "replaced": [item_json(i) for i in x.replaced],
                "not_replaced": list(x.not_replaced)}
    if isinstance(x, items.Capture):
        return {"origin": x.origin, "content": x.content, "refused": x.refused}
    if hasattr(x, "source_ids") and hasattr(x, "set_aside"):
        return item_json(x)
    if isinstance(x, profile.Entry):
        return {"section": x.section, "position": x.position, "item": x.item.id, "added_by": x.added_by,
                "seq": x.seq}
    if isinstance(x, summary.DayEvidence):
        return {"day": x.day, "digest": x.digest, "lines": x.lines,
                "handles": {k: [list(s) for s in v] for k, v in x.handles.items()},
                "owner_words": {k: list(v) for k, v in x.owner_words.items()}}
    if isinstance(x, import_extract.SaveCounts):
        return {"saved": x.saved, "credited": x.credited, "kept_owner": x.kept_owner, "older": x.older,
                "history": x.history, "refused": x.refused}
    if hasattr(x, "data") and hasattr(x, "kind") and hasattr(x, "origin") and hasattr(x, "at"):
        return {"id": x.id, "at": iso(x.at), "kind": x.kind, "data": x.data, "origin": x.origin}
    if isinstance(x, sqlite3.Row):
        return list(x)
    if isinstance(x, (set, frozenset)):
        return sorted(j(v) for v in x)
    if isinstance(x, tuple):
        return [j(v) for v in x]
    if isinstance(x, list):
        return [j(v) for v in x]
    if isinstance(x, dict):
        return {str(k): j(v) for k, v in x.items()}
    if isinstance(x, datetime):
        return x.isoformat()
    return x


def when(text: str | None) -> datetime | None:
    return datetime.fromisoformat(text) if text else None


async def op(env: Env, name: str, a: dict[str, Any]) -> Any:
    m, L, P = env.memory, env.learning, env.profile
    if name == "clock":
        env.clock.t = datetime.fromisoformat(a["at"])
        return None
    if name == "sql":
        env.db.conn._conn.execute(a["sql"], a.get("params", []))
        return None
    if name == "answers":
        env.answers = list(a["answers"])
        return None
    if name == "busy":
        env.busy_polls = a["polls"]
        return None
    # ---------- journal ----------
    if name == "journal.append":
        return env.journal.append(a["kind"], a.get("data"), a.get("origin"))
    if name == "journal.entries":
        return env.journal.entries(when(a.get("since")), a.get("limit"))
    if name == "journal.entries_of":
        return env.journal.entries_of(*a["kinds"])
    if name == "journal.kinds_after":
        return env.journal.kinds_after(tuple(a["kinds"]), a["id"])
    if name == "journal.of_kind":
        return env.journal.of_kind(a["kind"], a.get("field", ""), a.get("equals", ""), a.get("missing"))
    if name == "journal.recent_values":
        return env.journal.recent_values(a["kind"], a["field"], a["limit"])
    if name == "journal.by_ids":
        return env.journal.by_ids(a["ids"])
    if name == "journal.latest_kind":
        return env.journal.latest_kind(tuple(a["kinds"]), a["field"], a["equals"])
    if name == "journal.of_running_tasks":
        return env.journal.of_running_tasks(a["kind"])
    # ---------- memory items ----------
    if name == "memory.save":
        return m.save(a["kind"], a["subject"], a["key"], a["content"], a["origin"],
                      [tuple(s) for s in a["sources"]], a.get("confidence", 0.7), replaces=a.get("replaces", ()),
                      about=a.get("about", ""), from_owner=a.get("from_owner"))
    if name == "memory.get":
        return m.get(a["id"])
    if name == "memory.items":
        return m.items(a.get("status", "active"), a.get("subject"), a.get("valid_now", True))
    if name == "memory.current_items":
        return m.current_items()
    if name == "memory.history":
        return m.history(a["id"])
    if name == "memory.telling_words":
        return m.telling_words(a["text"], when(a.get("at")))
    if name == "memory.current_ids":
        return m.current_ids(a.get("origin"))
    if name == "memory.evidence_days":
        return m.evidence_days(a["id"])
    if name == "memory.from_mac_scan":
        return m.from_mac_scan(a["id"])
    if name == "memory.commit":
        return m.commit(a["ids"], a["reason"])
    if name == "memory.set_about":
        return m.set_about(a["id"], a["about"])
    if name == "memory.set_review":
        return m.set_review(a["id"], when(a.get("at")))
    if name == "memory.due_for_review":
        return m.due_for_review()
    if name == "memory.retract":
        return m.retract(a["id"], a.get("reason", ""), never_again=a.get("never_again", False))
    if name == "memory.expire_at":
        return m.expire_at(a["id"], datetime.fromisoformat(a["at"]))
    if name == "memory.retire":
        return m.retire(a["id"], a.get("reason", ""))
    if name == "memory.looks_over":
        return m.looks_over(a["id"], a.get("why", ""))
    if name == "memory.restore":
        return m.restore(a["id"])
    if name == "memory.edit":
        return m.edit(a["id"], a["content"], tuple(a["source"]))
    if name in ("memory.plan_forget", "memory.forget"):
        plan = (m.plan_forget if name == "memory.plan_forget" else m.forget)(a["id"], a.get("keep_sources", False))
        return {"items": plan.items, "trail": plan.trail, "rebuild_days": plan.rebuild_days,
                "delete_summaries": plan.delete_summaries, "kept": plan.kept,
                "destinations": [list(d) for d in plan.destinations],
                "describe": plan.describe(env.db), "plain": plan.describe(env.db, plain=True), "keeps": plan.keeps()}
    if name == "memory.forgets_on_record":
        return items.forgets_on_record(env.db)
    if name == "export":
        folder = env.folder / "export"
        written = export.export(m, folder)
        return {p.name: p.read_text() for p in written} | {"_files": sorted(p.name for p in folder.iterdir())}
    # ---------- profile ----------
    if name == "profile.entries":
        return P.entries()
    if name == "profile.render":
        return list(P.render())
    if name == "profile.prompt_text":
        return P.prompt_text()
    if name == "profile.pending_text":
        return P.pending_text()
    if name == "profile.settle":
        return P.settle(a["why"])
    if name == "profile.used_tokens":
        return P.used_tokens()
    if name == "profile.view":
        return P.view()
    if name == "profile.due":
        return P.due()
    if name == "profile.holds_up":
        return P.holds_up(m.get(a["id"]))
    if name == "profile.eligible":
        return [i.id for i in P.eligible()]
    if name == "profile.add":
        return P.add(a["id"], a.get("section", ""), by=a.get("by", "owner"))
    if name == "profile.remove":
        return P.remove(a["ids"], by=a.get("by", "code"))
    if name == "profile.seed":
        return P.seed()
    if name == "profile.pass_context":
        return P.pass_context()
    if name == "profile.apply":
        return P.apply(a["answer"])
    if name == "profile.run_pass":
        return await P.run_pass(env.make_runtime, device_id="dev-1", spans=env.spans, usage=env.usage)
    if name == "profile.holding":
        with P.holding_joins() as held:
            for step in a["steps"]:
                await op(env, step["op"], step.get("args", {}))
        return [i.id for i in held]
    # ---------- learning ----------
    if name == "learning.put":
        return L.put(a["kind"], a["key"], a["body"], a.get("sources", []), origin_label=a.get("origin", "inferred"),
                     made_by=a.get("made_by", "test:1"), batch=a.get("batch"), state=a.get("state", "candidate"),
                     owner=a.get("owner", False), labels=set(a["labels"]) if "labels" in a else None,
                     supersede=a.get("supersede", True))
    if name == "learning.get":
        return L.get(a["id"], a.get("sources", True))
    if name == "learning.items":
        return L.items(a.get("kind"), a.get("history", False), sources=a.get("sources", True),
                       kinds=a.get("kinds", ()))
    if name == "learning.versions":
        return L.versions(a["kind"], a["key"])
    if name == "learning.labels":
        return L.labels(a["sources"])
    if name == "learning.forget":
        return L.forget(a["refs"])
    if name == "learning.drop_fact_index":
        return L.drop_fact_index(a["ref"], m)
    if name == "learning.rollback":
        return L.rollback(a["batch"])
    if name == "learning.confirm":
        return L.confirm(a["id"])
    if name == "learning.edit":
        return L.edit(a["id"], a["text"])
    if name == "learning.retire":
        return L.retire(a["id"])
    if name == "learning.digest":
        return L.digest()
    if name == "learning.identity":
        return L.identity()
    if name == "learning.descendants":
        return L.descendants(a["refs"])
    if name == "learning.forgotten":
        return L.forgotten(a["ref"])
    if name == "learning.search":
        found = L.search(a.get("query", ""), kind=a.get("kind"), app=a.get("app"), person=a.get("person"),
                         since=a.get("since"), until=a.get("until"), limit=a.get("limit"),
                         leave_out=frozenset(a.get("leave_out", [])), sources=a.get("sources", True),
                         focus=a.get("focus", ""))
        return found
    if name == "learning.weighted_terms":
        return L.weighted_terms(a["words"])
    if name == "learning.reindex_search":
        n = {"left": a.get("stop_after", -1)}

        def stop() -> bool:
            if n["left"] == 0:
                return True
            n["left"] -= 1
            return False
        return learning.reindex_search(env.db, batch_bytes=a.get("batch_bytes", learning.REINDEX_BATCH_BYTES),
                                       stop=stop)
    if name == "learning.search_indexed":
        return learning.search_indexed(env.db)
    if name == "learning.prepare_episode":
        env.prepared = L.prepare_episode(a["task_id"], a.get("apps"))
        return env.prepared
    if name == "learning.write_episode":
        return L.write_episode(env.prepared)
    if name == "learning.capture_episode":
        return L.capture_episode(a["task_id"], a.get("apps"))
    if name == "learning.card_inputs":
        return L.card_inputs(a.get("task_id"))
    if name == "learning.cards_to_read":
        return L.cards_to_read()
    if name == "learning.card_read":
        return L.card_read(a["task_id"], a["inputs"], a.get("error"))
    if name == "learning.stuck_cards":
        return L.stuck_cards()
    if name == "learning.card_source_refs":
        return L.card_source_refs(a["task_id"])
    if name == "learning.card_source_plan":
        return L.card_source_plan(removing=a.get("removing", ()))
    if name == "learning.prune_card_sources":
        n = {"left": a.get("stop_after", -1)}
        told: list[Any] = []

        def stop() -> bool:
            if n["left"] == 0:
                return True
            n["left"] -= 1
            return False
        out = L.prune_card_sources(stop=stop, progress=lambda d, t: told.append([d, t]),
                                   rebuilding=lambda: told.append("rebuilding"))
        return {"counts": out, "told": told}
    if name == "learning.index_summaries":
        return learning.index_summaries(L)
    if name == "learning.current_card":
        return L.current_card(a["task_id"], a.get("sources", True))
    if name == "learning.generation":
        return L.generation
    if name == "learning.shown":
        node = L.get(a["id"], False)
        return {"text": learning.shown_text(node), "state": learning.shown_state(node)}
    # ---------- summary ----------
    if name == "summary.build":
        return summary.build(env.db, a["day"])
    if name == "summary.rebuild":
        return summary.rebuild(env.db, env.journal, a["day"])
    if name == "summary.summarized":
        return summary.summarized(env.db, a["day"])
    # ---------- distill ----------
    if name == "distill.pending_days":
        return env.distiller.pending_days()
    if name == "distill.run_day":
        return await env.distiller.run_day(a["day"])
    if name == "distill.catch_up":
        return await env.distiller.catch_up()
    if name == "distill.yesterday":
        return distill.yesterday(env.clock)
    # ---------- playbooks ----------
    if name == "playbooks.candidates":
        return [[b["id"] if b else None, [c["key"] for c in g]] for b, g in playbooks.candidates(L, FAKE_EMBEDDER)]
    if name == "playbooks.merge":
        return await playbooks.merge(L, env.make_runtime, FAKE_EMBEDDER, device_id="dev-1", spans=env.spans,
                                     record=lambda k, d: env.journal.append(k, d), usage=env.usage)
    if name == "playbooks.repeat_stats":
        return playbooks.repeat_stats(env.db.conn)
    if name == "playbooks.verified_cards":
        return [c["id"] for c in playbooks.verified_cards(L)]
    # ---------- mac profile ----------
    if name == "mac.interpret":
        return await mac_profile.interpret(env.make_runtime, a["facts"], device_id="dev-1", spans=env.spans,
                                           usage=env.usage)
    if name == "mac.store":
        with P.holding_joins() as held:
            out = mac_profile.store(m, lambda k, d: env.journal.append(k, d), a["facts"], a["answer"])
        return {"out": out, "held": [i.id for i in held]}
    if name == "mac.previous":
        return [i.id for i in mac_profile.previous(m)]
    # ---------- import ----------
    if name == "import.estimate":
        return await env.importer.estimate(a["source"], a["path"])
    if name == "import.create":
        return env.importer.create(a["source"], a["path"])
    if name == "import.run":
        return await env.importer.run(a["run"])
    if name == "import.restart":
        env.importer = env.new_importer()
        return None
    if name == "import.end":
        return env.importer.end(a["run"], a["status"], a.get("error", ""))
    if name == "import.status":
        return env.importer.status()
    if name == "import.running":
        return env.importer.running()
    if name == "import.tell_done":
        return await env.importer.tell_done(a["run"])
    # ---------- questions ----------
    q = questions.MemoryQuestions(env.db)
    if name == "questions.keep":
        return q.keep(a["id"], a["kind"], a["target"], a["reason"], a["request_id"], a["at"])
    if name == "questions.waiting":
        return q.waiting()
    if name == "questions.answered":
        return q.answered(a["id"])
    if name == "questions.drop_about":
        return q.drop_about(a["targets"], tuple(a["kinds"]) if a.get("kinds") is not None else None)
    if name == "questions.drop_kinds":
        return q.drop_kinds(a["kinds"])
    # ---------- upgrade ----------
    if name == "upgrade.clean":
        return upgrade.clean_before_tier1(m, lambda k, d: env.journal.append(k, d))
    if name == "upgrade.close_fact_nodes":
        return upgrade.close_fact_nodes(env.db, env.clock.now().isoformat(), lambda k, d: env.journal.append(k, d))
    # ---------- the memory.learned tool ----------
    if name == "tool.learned":
        tool = LearningRead(lambda: L)
        res = await tool.run(LearningRead.Input(**a), None)
        return {"content": res.content, "is_error": res.is_error}
    raise ValueError(f"no such op {name}")


class FakeEmbedder:
    """tests/support/fake_embedder.py's stand-in, with its concept table."""
    name = "fake-embedder:256"
    dimensions = 256

    def encode(self, text: str, *, query: bool = False) -> Any:
        from tests.support.fake_embedder import FakeEmbedder as F
        return F().encode(text, query=query)


FAKE_EMBEDDER = FakeEmbedder()


# ======================================================================================================================
# scenario data
# ======================================================================================================================

def at(minutes: float = 0, days: float = 0) -> str:
    return (T0 + timedelta(minutes=minutes, days=days)).isoformat()


def S(op_name: str, **args: Any) -> dict[str, Any]:
    return {"op": op_name, "args": args}


def trail(ident: str, when_: str, day: str, actor: str, source: str, data: dict[str, Any],
          task: str | None = None) -> dict[str, Any]:
    return S("sql", sql="INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) "
                        "VALUES (?, ?, ?, 'dev-1', ?, ?, ?, ?)",
             params=[ident, when_, day, actor, task, source, json.dumps(data)])


def jrow(ident: str, when_: str, kind: str, data: dict[str, Any], origin: str | None = None) -> dict[str, Any]:
    return S("sql", sql="INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?)",
             params=[ident, when_, kind, json.dumps(data), origin])


def dest(ident: str, kind: str, value: str, when_: str) -> dict[str, Any]:
    return S("sql", sql="INSERT INTO known_destinations (id, kind, value, how, at) VALUES (?, ?, ?, 'named', ?)",
             params=[ident, kind, value, when_])


def episode(task: str, request: str, goal: str, status: str = "done", verified: Any = 1, ended: str = "",
            how: str = "", seconds: float = 30.0, steps: Any = None, origins: str = "owner") -> list[dict[str, Any]]:
    out = [S("sql", sql="INSERT INTO episodes (task_id, request_id, request_text, goal, status, verified, started_at, "
                        "ended_at, seconds, answer, conditions_json, steps_json, problems_json, tools_json, origins, "
                        "tool_origins, external_read, usage_json, desktop_json, made_by, built_at) VALUES "
                        "(?, 'req', ?, ?, ?, ?, ?, ?, ?, 'Done.', '[]', ?, '[]', '{}', ?, '', 0, '{}', '{}', "
                        "'code:episode/1', ?)",
             params=[task, request, goal, status, verified, ended or at(), ended or at(1), seconds,
                     json.dumps(steps or [{"role": "Worker"}]), origins, ended or at(1)])]
    if how:
        out.append(S("sql", sql="INSERT INTO episode_how (task_id, how) VALUES (?, ?)", params=[task, how]))
    return out


def span(ident: str, task: str, kind: str, inp: dict[str, Any], output: Any = None, error: str | None = None,
         started: str = "") -> dict[str, Any]:
    return S("sql", sql="INSERT INTO spans (id, task_id, kind, started_at, input_json, output_json, error) "
                        "VALUES (?, ?, ?, ?, ?, ?, ?)",
             params=[ident, task, kind, started or at(), json.dumps(inp),
                     None if output is None else json.dumps(output), error])


def task_row(ident: str, status: str = "done") -> dict[str, Any]:
    return S("sql", sql="INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, "
                        "created_at, updated_at) VALUES (?, ?, 'req', 'g', 'owner', 'worker', '[]', ?, ?, ?)",
             params=[ident, ident, status, at(), at()])


DAY = "2026-10-20"          # T0 is 14:30 local on the 20th (FX-05:30)


def sc_items() -> list[dict[str, Any]]:
    return [
        jrow("j-said-1", at(), "chat.message", {"text": "my dentist is Dr. Zorblat", "untrusted_parts": 0}, "owner"),
        jrow("j-said-2", at(5), "memory.added", {"content": "x"}, "owner"),
        S("memory.save", kind="person", subject="owner", key="Dentist", content="  my dentist is Dr. Zorblat ",
          origin="owner_stated", sources=[["journal", "j-said-1"]], confidence=1.0, about="the teeth task"),
        S("memory.save", kind="person", subject="owner", key="dentist", content="My dentist is Dr. Zorblat",
          origin="inferred", sources=[["journal", "j-said-2"]]),
        S("memory.save", kind="fact", subject="Project:Home Move", key="Movers", content="Movers come on the 16th",
          origin="inferred", sources=[["journal", "j-said-1"]]),
        S("clock", at=at(10)),
        S("memory.save", kind="fact", subject="project:home-move", key="movers", content="Movers come on the 18th",
          origin="inferred", sources=[["journal", "j-said-2"]]),
        S("memory.history", id="{first:movers}"),
        S("memory.save", kind="preference", subject="owner", key="tone", content="SECRET-abc123 short replies",
          origin="owner_stated", sources=[["journal", "j-said-1"]], replaces=["{first:dentist}", "nope", "{last:movers}"]),
        S("memory.save", kind="preference", subject="owner", key="tone", content="no buzzwords",
          origin="inferred", sources=[["journal", "j-said-2"]]),
        S("memory.save", kind="fact", subject="owner", key="x", content="", origin="inferred",
          sources=[["journal", "j-said-2"]]),
        S("memory.save", kind="bogus", subject="owner", key="x", content="y", origin="inferred",
          sources=[["journal", "j-said-2"]]),
        S("memory.save", kind="fact", subject="owner", key="x", content="y", origin="inferred", sources=[]),
        S("memory.save", kind="fact", subject="owner", key="x", content="y", origin="third_party",
          sources=[["journal", "j-said-2"]]),
        S("memory.save", kind="fact", subject="world", key="weather", content="It rains in May",
          origin="third_party", sources=[["journal", "j-said-2"]]),
        S("memory.save", kind="fact", subject="owner", key="x", content="y", origin="observed",
          sources=[["journal", "j-said-2"]]),
        trail("t-1", at(), DAY, "owner", "app_focus", {"app": "Piano"}),
        trail("t-2", at(days=1), "2026-10-21", "owner", "app_focus", {"app": "Piano"}),
        trail("t-3", at(days=2), "2026-10-22", "owner", "app_focus", {"app": "Piano"}),
        trail("t-f", at(), DAY, "ferb", "app_focus", {"app": "Piano"}),
        S("memory.save", kind="fact", subject="owner", key="x", content="y", origin="observed",
          sources=[["trail", "t-missing"]]),
        S("memory.save", kind="preference", subject="owner", key="music", content="plays piano daily",
          origin="observed", sources=[["trail", "t-f"]]),
        S("memory.save", kind="preference", subject="owner", key="music", content="plays piano daily",
          origin="inferred", sources=[["trail", "t-f"]]),
        S("memory.save", kind="preference", subject="owner", key="music", content="plays piano daily",
          origin="observed", sources=[["trail", "t-1"]]),
        S("memory.evidence_days", id="{last:music}"),
        S("memory.save", kind="preference", subject="owner", key="music", content="Plays piano most days",
          origin="observed", sources=[["trail", "t-2"]]),
        S("memory.save", kind="preference", subject="owner", key="music", content="piano again",
          origin="observed", sources=[["trail", "t-3"], ["trail", "t-3"]]),
        S("memory.items", status=None, valid_now=False),
        S("memory.items", status="active", subject="owner"),
        S("memory.items", status="proposed"),
        S("memory.current_items"),
        S("memory.current_ids"),
        S("memory.current_ids", origin="owner_stated"),
        S("memory.telling_words", text="my dentist and piano and movers come"),
        S("memory.telling_words", text="", at=at(-100)),
        S("memory.get", id="id-000001"),
        S("memory.get", id="nope"),
        S("memory.set_about", id="{first:movers}", about="  the   move  SECRET-zz9 " + "x" * 300),
        S("memory.set_review", id="{first:tone}", at=at(60)),
        S("memory.set_review", id="{last:movers}", at=(T0 + timedelta(minutes=30)).astimezone(
            timezone(timedelta(hours=-7))).isoformat()),
        S("memory.due_for_review"),
        S("clock", at=at(90)),
        S("memory.due_for_review"),
        S("memory.set_review", id="{first:tone}"),
        S("memory.expire_at", id="{last:weather}", at=at(200)),
        S("memory.expire_at", id="{last:weather}", at=at(300)),
        S("memory.items", status="active"),
        S("clock", at=at(250)),
        S("memory.items", status="active"),
        S("memory.current_ids"),
        S("memory.retire", id="{last:weather}", reason="over"),
        S("memory.retire", id="{last:movers}", reason="the move is done"),
        S("memory.looks_over", id="{first:tone}", why="seems   old SECRET-q1"),
        S("memory.looks_over", id="{last:tone}", why="FERB's check"),
        S("memory.current_items"),
        S("memory.restore", id="{first:tone}"),
        S("memory.restore", id="{first:tone}"),
        S("memory.save", kind="preference", subject="owner", key="tone", content="no buzzwords",
          origin="owner_stated", sources=[["journal", "j-said-1"]]),
        S("memory.retract", id="{first:dentist}", reason="wrong", never_again=True),
        S("memory.retract", id="nope", reason="wrong"),
        S("memory.save", kind="person", subject="owner", key="dentist", content="My dentist is Dr. Zorblat",
          origin="inferred", sources=[["journal", "j-said-2"]]),
        S("memory.edit", id="{first:dentist}", content="my dentist is now Dr. Quell", source=["journal", "j-said-2"]),
        S("memory.edit", id="missing", content="x", source=["journal", "j-said-2"]),
        S("memory.commit", ids=["{last:music}", "nope"], reason="enough days"),
        S("memory.from_mac_scan", id="{first:dentist}"),
        S("memory.history", id="{first:dentist}"),
        S("memory.history", id="nope"),
        S("memory.current_items"),
        S("export"),
    ]


def sc_forget() -> list[dict[str, Any]]:
    d2 = "2026-10-21"
    return [
        trail("t-a", at(), DAY, "owner", "browser_tab", {"host": "www.kaggle.com", "browser": "chrome",
                                                          "profile": "Default"}),
        trail("t-b", at(5), DAY, "owner", "app_focus", {"app": "Mail"}),
        trail("t-c", at(days=1), d2, "owner", "app_focus", {"app": "Mail"}),
        jrow("j-1", at(), "chat.message", {"text": "Ana Ruiz is my lawyer, call +1 (555) 010-7788", "untrusted_parts": 0},
             "owner"),
        jrow("j-sum-1", at(), "trail.day_summary", {"day": DAY, "events": 2}),
        jrow("j-sum-2", at(), "trail.day_summary", {"day": d2, "events": 1}),
        dest("d-1", "name", "Ana Ruiz", at()),
        dest("d-2", "phone", "15550107788", at(1)),
        dest("d-3", "host", "www.kaggle.com", at(2)),
        dest("d-4", "email", "sha256:abc", at(3)),
        dest("d-5", "name", "Bob", at(4)),
        dest("d-6", "phone", "0107", at(5)),
        S("memory.save", kind="person", subject="person:Ana Ruiz", key="role",
          content="Ana Ruiz is my lawyer, call +1 (555) 010-7788", origin="owner_stated",
          sources=[["journal", "j-1"]], about="legal work"),
        S("memory.save", kind="fact", subject="owner", key="site", content="uses kaggle for data",
          origin="inferred", sources=[["trail", "t-a"], ["trail", "t-b"]]),
        S("memory.save", kind="fact", subject="owner", key="mail", content="checks Mail",
          origin="inferred", sources=[["trail", "t-b"], ["trail", "t-c"]]),
        S("learning.put", kind="fact", key="site", body={"content": "uses kaggle"}, sources=["memory:{last:site}"],
          made_by="code:fact/1", state="active"),
        S("clock", at=at(1)),
        S("learning.put", kind="fact", key="site", body={"content": "uses kaggle a lot"},
          sources=["memory:{last:site}"], made_by="code:fact/1", state="active"),
        S("learning.put", kind="episode", key="task-9", body={"intent": "check kaggle"},
          sources=["memory:{last:site}"], made_by="code:evidence-card/4", origin="ferb", state="active"),
        S("memory.plan_forget", id="{last:site}"),
        S("memory.plan_forget", id="{last:site}", keep_sources=True),
        S("memory.plan_forget", id="{last:role}"),
        S("memory.plan_forget", id="nope"),
        S("memory.forget", id="{last:site}"),
        S("memory.save", kind="fact", subject="owner", key="site", content="uses kaggle for data",
          origin="inferred", sources=[["trail", "t-b"]]),
        S("memory.forget", id="{last:role}"),
        S("memory.forget", id="{last:mail}"),
        S("memory.forgets_on_record"),
        S("summary.summarized", day=DAY),
        S("learning.get", id="m-nope"),
        S("learning.items", history=True),
        S("export"),
    ]


def sc_profile() -> list[dict[str, Any]]:
    steps = [
        jrow("j-1", at(), "chat.message", {"text": "hello", "untrusted_parts": 0}, "owner"),
        jrow("j-2", at(days=-3), "chat.message", {"text": "older", "untrusted_parts": 0}, "owner"),
        S("profile.seed"),
        S("profile.seed"),
        S("memory.save", kind="preference", subject="owner", key="tone", content="short   replies, no buzzwords",
          origin="owner_stated", sources=[["journal", "j-1"]], about="emails"),
        S("memory.save", kind="fact", subject="project:ferb", key="lang", content="FERB moves to Rust",
          origin="owner_stated", sources=[["journal", "j-1"]]),
        S("memory.save", kind="fact", subject="owner", key="city", content="lives in Pune",
          origin="inferred", sources=[["journal", "j-1"]]),
        S("profile.entries"),
        S("profile.prompt_text"),
        S("profile.pending_text"),
        S("profile.settle", why="start"),
        S("profile.pending_text"),
        S("profile.prompt_text"),
        S("memory.save", kind="fact", subject="owner", key="city", content="lives in Pune",
          origin="inferred", sources=[["journal", "j-2"]]),
        S("profile.pending_text"),
        S("profile.holds_up", id="{last:city}"),
        S("profile.eligible"),
        S("profile.view"),
        S("profile.used_tokens"),
        S("profile.due"),
        S("profile.add", id="{last:lang}", section="  Work stuff  "),
        S("profile.add", id="nope"),
        S("memory.save", kind="preference", subject="owner", key="tone", content="always short replies",
          origin="owner_stated", sources=[["journal", "j-1"]]),
        S("profile.entries"),
        S("memory.forget", id="{last:city}"),
        S("profile.render"),
        S("profile.pass_context"),
        jrow("j-so", at(), "memory.search_only", {"keys": [["project:ferb", "lang"]]}),
        S("profile.eligible"),
        S("profile.apply", answer={"sections": [{"title": "  Style  ", "items": ["{last:tone}", "{last:tone}", "zz"]},
                                                {"title": "", "items": ["{last:lang}"]}],
                                   "ended": [{"id": "{last:tone}", "why": "over"}, {"id": "zz", "why": "x"}]}),
        S("profile.entries"),
        S("profile.due"),
        S("clock", at=at(days=8)),
        S("profile.due"),
        S("answers", answers=[{"answer": {"sections": [{"title": "You", "items": ["{last:tone}", "{last:note_0}"]}], "ended": []}},
                              {"error": "model down"}, {"text": "[1, 2]"}, {"invalid": "missing sections"}]),
        S("profile.run_pass"),
        S("profile.due"),
        S("clock", at=at(days=16)),
        S("profile.run_pass"),
        S("profile.due"),
        S("clock", at=at(days=16, minutes=61)),
        S("profile.run_pass"),
        S("clock", at=at(days=16, minutes=130)),
        S("profile.run_pass"),
        S("profile.view"),
    ]
    # a tight budget: what doesn't fit is left out, counted, and makes the pass due early
    for n in range(12):
        steps.append(S("memory.save", kind="fact", subject="owner", key=f"note {n}",
                       content=f"owner note number {n} " + "word " * 30, origin="owner_stated",
                       sources=[["journal", "j-1"]]))
    steps += [S("profile.render"), S("profile.prompt_text"), S("profile.pending_text"), S("profile.due"),
              S("profile.settle", why="fold"), S("profile.render"), S("profile.view"),
              S("clock", at=at(days=17, minutes=200)), S("profile.due")]
    return steps


def sc_profile_seed() -> list[dict[str, Any]]:
    return [
        jrow("j-1", at(), "chat.message", {"text": "hello", "untrusted_parts": 0}, "owner"),
        jrow("j-mac", at(), "memory.mac_profile", {"facts": {}}),
        S("memory.save", kind="env", subject="owner", key="browser", content="uses Chrome", origin="observed",
          sources=[["journal", "j-mac"]]),
        S("memory.save", kind="person", subject="person:ana", key="role", content="Ana is a lawyer",
          origin="owner_stated", sources=[["journal", "j-1"]]),
        S("memory.save", kind="fact", subject="owner", key="setup_persona", content="old dump",
          origin="owner_stated", sources=[["journal", "j-1"]]),
        S("memory.save", kind="project", subject="project:x", key="goal", content="ship x", origin="inferred",
          sources=[["journal", "j-1"]]),
        jrow("j-clean", at(), "memory.cleanup", {"owner_kept": ["{last:goal}"]}),
        S("profile.holds_up", id="{last:goal}"),
        S("profile.seed"),
        S("profile.entries"),
        S("profile.view"),
    ]


def sc_learning() -> list[dict[str, Any]]:
    return [
        jrow("j-o", at(), "chat.message", {"text": "book the dentist", "untrusted_parts": 1}, "owner,external:paste"),
        jrow("j-f", at(), "task.outcome", {"task_id": "t1"}),
        trail("t-w", at(), DAY, "owner", "window", {"title": "x"}),
        trail("t-x", at(), DAY, "owner", "typed_text", {"text": "x"}),
        trail("t-y", at(), DAY, "owner", "file", {"path": "/a/b"}),
        S("learning.labels", sources=["journal:j-o", "trail:t-w", "trail:t-x", "trail:t-y"]),
        S("learning.labels", sources=["journal:nope"]),
        S("learning.labels", sources=["nocolon"]),
        S("learning.put", kind="identity", key="me", body={"text": "x"}, sources=[]),
        S("learning.put", kind="lesson", key="l1", body={"text": "a lesson"}, sources=["journal:j-o"],
          origin="owner"),
        S("learning.put", kind="lesson", key="l1", body={"text": "a lesson", "password": "hunter",
                                                          "n": 12, "flag": True, "note": "SECRET-x1"},
          sources=["journal:j-o", "trail:t-w"], batch="b1"),
        S("learning.put", kind="lesson", key="l1", body={"text": "a lesson", "password": "hunter",
                                                          "n": 12, "flag": True, "note": "SECRET-x1"},
          sources=["journal:j-o", "trail:t-w"], batch="b1"),
        S("clock", at=at(1)),
        S("learning.put", kind="lesson", key="l1", body={"text": "a better lesson " + SEALED}, sources=[]),
        S("learning.put", kind="lesson", key="l1", body={"text": "a better lesson"}, sources=["journal:j-f"],
          batch="b2"),
        S("learning.put", kind="identity_proposal", key="who", body={"text": "the owner likes tea"},
          sources=["journal:j-o"], batch="b2", state="quarantined"),
        S("learning.put", kind="identity_proposal", key="who2", body={"text": "the owner likes coffee"},
          sources=["journal:j-o"], batch="b2"),
        S("learning.put", kind="episode_annotation", key="task-1 SECRET-k2", body={"reflection": "went fine"},
          sources=["m-nope-ref"], labels=["ferb"]),
        S("learning.put", kind="activity_summary", key="trail.day_summary:2026-10-20",
          body={"text": "a quiet day", "day": DAY}, sources=["journal:j-f"], state="active"),
        S("learning.versions", kind="lesson", key="l1"),
        S("learning.items"),
        S("learning.items", kind="lesson", history=True, sources=False),
        S("learning.items", kinds=["lesson", "identity_proposal"]),
        S("learning.digest"),
        S("learning.digest"),
        S("learning.confirm", id="m-nope"),
        S("learning.identity"),
        S("learning.retire", id="nope"),
        S("learning.edit", id="m-nope", text="x"),
        S("learning.descendants", refs=["journal:j-o"]),
        S("learning.rollback", batch="b2"),
        S("learning.items", history=True, sources=False),
        S("learning.rollback", batch="b2"),
        S("learning.weighted_terms", words=["lesson", "quiet", "zzz"]),
        S("learning.search", query="better lesson"),
        S("learning.search", query=""),
        S("learning.search", query="", kind="lesson"),
        S("learning.search", query="quiet day", limit=1, sources=False),
        S("learning.search", query="a", leave_out=["lesson"]),
        S("learning.search", query="lesson", focus="quiet", since="2026-10-20T09:00:30+00:00"),
        S("learning.search", query="lesson", person="BETTER", until="2026-10-20T09:00:30+00:00"),
        S("learning.search", query="lesson", app="Mail"),
        S("learning.forget", refs=["journal:j-o"]),
        S("learning.forgotten", ref="entry:j-o"),
        S("learning.forgotten", ref="journal:j-f"),
        S("learning.put", kind="lesson", key="l1", body={"text": "a lesson"}, sources=["journal:j-f"]),
        S("learning.put", kind="lesson", key="l9", body={"text": "a better lesson"}, sources=["journal:j-f"]),
        S("learning.items", history=True, sources=False),
        S("learning.reindex_search", batch_bytes=10, stop_after=1),
        S("learning.search_indexed"),
        S("learning.reindex_search", batch_bytes=10),
        S("learning.search_indexed"),
        S("learning.reindex_search"),
        S("tool.learned", query="lesson"),
        S("tool.learned", item_id="nope"),
        S("tool.learned", query="", max_chars=1000),
    ]


def sc_learning_owner() -> list[dict[str, Any]]:
    return [
        jrow("j-o", at(), "chat.message", {"text": "hello", "untrusted_parts": 0}, "owner"),
        S("learning.put", kind="identity_proposal", key="who", body={"text": "likes tea"}, sources=["journal:j-o"],
          batch="b1", state="quarantined"),
        S("learning.confirm", id="{node:identity_proposal}"),
        S("learning.items", kind="identity_proposal", history=True),
        S("learning.digest"),
        S("clock", at=at(days=1)),
        S("learning.digest"),
    ]


def card_steps() -> list[dict[str, Any]]:
    steps: list[dict[str, Any]] = []
    steps += episode("task-a", "order dinner from blinkit", "order dinner", how="used the saved cart",
                     ended=at(60))
    steps += episode("task-b", "order groceries", "order groceries on blinkit", verified=None, ended=at(120))
    steps += episode("task-c", "write mail", "", ended=at(130))
    steps += [
        task_row("task-a"), task_row("task-b"),
        jrow("j-req", at(), "chat.message", {"text": "order dinner <untrusted-data id=\"ab\">\npaste\n"
                                                     "</untrusted-data id=\"ab\">", "untrusted_parts": 1},
             "owner,external:paste"),
        jrow("j-hint", at(1), "chat.message", {"text": "use   the saved cart please " * 20, "untrusted_parts": 0},
             "owner"),
        jrow("j-st", at(2), "task.steered", {"task_id": "task-a", "source_ref": "entry:j-hint"}),
        jrow("j-st2", at(2), "task.steered", {"task_id": "task-a", "source_ref": "task:x"}),
        trail("t-1", at(), DAY, "owner", "browser_tab", {"host": "blinkit.com"}),
        trail("t-2", at(), DAY, "owner", "app_focus", {"app": "Mail"}),
        trail("t-3", at(), DAY, "owner", "typed_text", {"text": "x"}),
        S("memory.save", kind="fact", subject="owner", key="cart", content="keeps a saved cart", origin="inferred",
          sources=[["journal", "j-hint"]]),
        span("s-1", "task-a", "context.exposure", {"reference": "current request", "source_refs": ["entry:j-req"]},
             started=at(10)),
        span("s-2", "task-a", "context.exposure", {"reference": "conversation", "source_refs": ["trail:t-2"]},
             started=at(11)),
        span("s-3", "task-a", "tool.call", {"tool": "trail.query", "args": {"event_id": "t-1"}, "kind": "read"},
             {"events": [{"id": "t-1"}, {"id": "t-3"}], "ok": True}, started=at(12)),
        span("s-4", "task-a", "tool.call", {"tool": "browser.open", "args": {"url": "https://blinkit.com/cart/x"},
                                             "kind": "act", "declared": "open cart"},
             {"ok": True, "status": "done", "intent_id": "i-1", "extra": "dropped"}, started=at(13)),
        span("s-5", "task-a", "tool.call", {"tool": "mac.click", "args": {"app": "Notes"}, "target": {"site": "x"}},
             {"bundle_id": "com.apple.Notes"}, started=at(14)),
        span("s-6", "task-a", "tool.call", {"tool": "mac.click", "args": {"app": "Notes"}}, None, "boom",
             started=at(15)),
        span("s-7", "task-a", "tool.call", {"tool": "memory.read", "args": {"item_id": "{last:cart}"}},
             {"text": json.dumps({"items": [{"id": "{last:cart}"}]})}, started=at(16)),
        span("s-8", "task-a", "tool.call", {"tool": "fs.read", "args": {"path": "/tmp/a/notes.txt"}},
             {"ok": True}, started=at(17)),
        span("s-9", "task-a", "tool.call", {"tool": "fs.read", "args": {"path": "/tmp/a/notes.txt"}},
             {"ok": True}, started=at(18)),
        span("s-10", "task-a", "context.source", {"source_refs": ["trail:t-2"]}, started=at(19)),
        span("s-b1", "task-b", "tool.call", {"tool": "task.evidence", "args": {}},
             {"source_refs": ["journal:j-hint"], "records": [{"kind": "context.source",
                                                               "input": {"source_refs": ["trail:t-1"]}}],
              "text": json.dumps({"kind": "tool.call", "input": {"tool": "trail.days"},
                                  "output": {"days": [{"id": "j-hint"}]}})}, started=at(20)),
        S("sql", sql="INSERT INTO owner_verdicts (id, task_id, verdict, note, at) VALUES ('v1','task-b','pass','ok',?)",
          params=[at(30)]),
    ]
    return steps


def sc_cards() -> list[dict[str, Any]]:
    return card_steps() + [
        S("learning.card_inputs"),
        S("learning.cards_to_read"),
        S("learning.prepare_episode", task_id="task-a", apps={"com.apple.Notes": "1.2", "com.google.Chrome": "130"}),
        S("learning.write_episode"),
        S("learning.current_card", task_id="task-a"),
        S("learning.shown", id="{node:episode}"),
        S("learning.cards_to_read"),
        S("learning.capture_episode", task_id="task-b"),
        S("learning.capture_episode", task_id="task-c"),
        S("learning.capture_episode", task_id="nope"),
        S("learning.card_source_refs", task_id="task-a"),
        S("learning.card_source_refs", task_id="task-c"),
        S("learning.card_read", task_id="task-c", inputs="x1", error="boom"),
        S("learning.card_read", task_id="task-c", inputs="x1", error="boom again"),
        S("learning.card_read", task_id="task-b", inputs="x2", error="redaction would still change it"),
        S("learning.stuck_cards"),
        S("learning.cards_to_read"),
        S("learning.card_source_plan"),
        S("learning.prune_card_sources"),
        S("learning.search", query="order dinner blinkit"),
        S("tool.learned", query="dinner"),
        S("learning.generation"),
        S("learning.forget", refs=["trail:t-1"]),
        S("learning.prepare_episode", task_id="task-a"),
        S("learning.write_episode"),
        S("learning.capture_episode", task_id="task-a"),
        S("learning.cards_to_read"),
        S("learning.generation"),
    ]


def sc_cleanout() -> list[dict[str, Any]]:
    steps = card_steps()
    old_refs = ["episode:task-a", "trail:t-1", "trail:t-2", "trail:t-3", "journal:j-req", "trail:t-gone",
                "trail:t-gone2", "journal:j-hint"]
    steps += [
        S("learning.put", kind="episode", key="task-a", body={"intent": "order dinner", "v": 1}, sources=old_refs,
          made_by="code:evidence-card/1", origin="ferb", state="active", labels=["owner", "ferb"]),
        S("clock", at=at(1)),
        S("learning.put", kind="episode", key="task-a", body={"intent": "order dinner", "v": 2}, sources=old_refs,
          made_by="code:evidence-card/1", origin="ferb", state="active", labels=["owner", "ferb"]),
        S("learning.put", kind="episode", key="task-z", body={"intent": "aged out"}, sources=[
            "episode:task-z", "trail:t-1", "trail:t-3", "trail:t-gone", "trail:t-gone2"],
          made_by="code:evidence-card/1", origin="ferb", state="active", labels=["owner"]),
        S("learning.card_source_plan"),
        S("learning.prune_card_sources", stop_after=1),
        S("learning.card_source_plan"),
        S("learning.prune_card_sources"),
        S("learning.card_source_plan"),
        S("learning.prune_card_sources"),
    ]
    return steps


def sc_summary() -> list[dict[str, Any]]:
    d = DAY
    return [
        trail("t-1", "2026-10-20T08:00:00+05:30", d, "owner", "app_focus", {"app": "Mail"}),
        trail("t-2", "2026-10-20T02:40:00+00:00", d, "owner", "app_focus", {"bundle_id": "com.x.Piano"}),
        trail("t-3", "2026-10-20T08:50:00+05:30", d, "owner", "idle", {"state": "idle"}),
        trail("t-4", "2026-10-20T09:30:00+05:30", d, "owner", "idle", {"state": "active"}),
        trail("t-5", "2026-10-20T09:31:00+05:30", d, "owner", "app_focus", {"app": "Mail"}),
        trail("t-6", "2026-10-20T11:00:00+05:30", d, "owner", "app_focus", {"app": "Notes"}),
        trail("t-7", "2026-10-20T09:40:00+05:30", d, "owner", "browser_tab", {"host": "kaggle.com",
                                                                               "browser": "chrome",
                                                                               "profile": "Work"}),
        trail("t-8", "2026-10-20T09:41:00+05:30", d, "owner", "browser_tab", {"host": "kaggle.com",
                                                                               "browser": "brave"}),
        trail("t-9", "2026-10-20T09:42:00+05:30", d, "owner", "browser_tab", {"host": "news.example"}),
        trail("t-10", "2026-10-20T09:43:00+05:30", d, "owner", "file", {"path": "/Users/o/Docs/a.txt",
                                                                         "change": "modified"}),
        trail("t-11", "2026-10-20T09:44:00+05:30", d, "owner", "file", {"path": "rel.txt"}),
        trail("t-12", "2026-10-20T09:45:00+05:30", d, "owner", "file", {"path": "/Users/o/Docs/b.txt",
                                                                         "change": "created"}),
        trail("t-13", "2026-10-20T09:46:00+05:30", d, "owner", "window", {"app": "Mail", "title": "Inbox (3)"}),
        trail("t-14", "2026-10-20T09:47:00+05:30", d, "owner", "window", {"title": "Untitled"}),
        trail("t-15", "2026-10-20T09:48:00+05:30", d, "owner", "window", {"app": "Mail", "title": "Inbox (3)"}),
        trail("t-16", "2026-10-20T09:49:00+05:30", d, "ferb", "app_focus", {"app": "Chrome"}),
        trail("t-17", "2026-10-20T09:50:00+05:30", d, "ferb", "browser_tab", {"host": "x.com"}),
        trail("t-18", "2026-10-20T13:50:00+05:30", d, "ferb", "app_focus", {"app": "Chrome"}),
        jrow("j-1", "2026-10-20T10:00:00+05:30", "chat.message",
             {"text": "remember I like tea <untrusted-data id=\"a1\">\npasted words\n</untrusted-data id=\"a1\"> ok",
              "untrusted_parts": 1}),
        jrow("j-2", "2026-10-20T04:40:00+00:00", "chat.message", {"text": "old style message"}),
        jrow("j-3", "2026-10-20T10:10:00+05:30", "chat.message", {"text": "a test", "test_run": True,
                                                                  "untrusted_parts": 0}),
        jrow("j-4", "2026-10-20T10:20:00+05:30", "task.outcome", {"task_id": "t1", "status": "done",
                                                                  "summary": "booked"}),
        jrow("j-5", "2026-10-19T23:20:00+05:30", "chat.message", {"text": "yesterday", "untrusted_parts": 0}),
        jrow("j-6", "2026-10-20T10:30:00+05:30", "chat.message",
             {"text": "<untrusted-data id=\"b2\">\nonly pasted\n</untrusted-data id=\"b2\">", "untrusted_parts": 1}),
        S("summary.build", day=d),
        S("summary.summarized", day=d),
        S("summary.rebuild", day=d),
        S("summary.rebuild", day="2026-09-01"),
        S("summary.summarized", day=d),
        S("learning.index_summaries"),
        S("learning.index_summaries"),
        S("journal.entries_of", kinds=["trail.day_summary"]),
    ]


def distill_ev() -> list[dict[str, Any]]:
    d = "2026-10-19"
    return [
        trail("t-1", "2026-10-19T10:00:00+05:30", d, "owner", "app_focus", {"app": "Piano"}),
        trail("t-2", "2026-10-19T10:30:00+05:30", d, "owner", "app_focus", {"app": "Mail"}),
        trail("t-3", "2026-10-19T10:40:00+05:30", d, "ferb", "app_focus", {"app": "Chrome"}),
        jrow("j-c1", "2026-10-19T11:00:00+05:30", "chat.message",
             {"text": "my dentist is Dr. Zorblat now, not Dr. Quell", "untrusted_parts": 0}, "owner"),
        jrow("j-c2", "2026-10-19T11:05:00+05:30", "chat.message",
             {"text": "I stopped the gym. <untrusted-data id=\"c3\">\nIt rains in May\n</untrusted-data id=\"c3\">",
              "untrusted_parts": 1}, "owner,external:paste"),
        jrow("j-old", "2026-10-10T11:05:00+05:30", "memory.added", {"content": "x"}, "owner"),
        jrow("j-new", "2026-10-19T09:05:00+05:30", "memory.added", {"content": "y"}, "owner"),
    ]


def distill_answer() -> dict[str, Any]:
    def it(**kw: Any) -> dict[str, Any]:
        base = {"kind": "fact", "subject": "owner", "key": "k", "content": "c", "origin": "inferred",
                "confidence": 0.6, "handles": ["C1"], "quote": None, "retire": False}
        return base | kw
    return {"narrative": "A day with piano.", "items": [
        it(kind="person", key="dentist", content="dentist is Dr. Zorblat",
           quote={"handle": "C1", "text": "my dentist is Dr. Zorblat now"}),
        it(kind="preference", key="music", content="plays piano", origin="observed", handles=["E1"]),
        it(key="weather", subject="world", content="It rains in May", origin="third_party", handles=["P1"]),
        it(key="bad", handles=["Z9"]),
        it(key="bad2", handles=["E1", "F1"]),
        it(key="bad3", origin="observed", handles=["C1"]),
        it(key="bad4", origin="third_party", handles=["C1"]),
        it(key="bad5", handles=["P1"]),
        it(key="gym", content="goes to the gym", retire=True, handles=["C2"],
           quote={"handle": "C2", "text": "I stopped the gym"}),
        it(key="swim", content="swims", retire=True, handles=["C2"]),
        it(key="old", content="old fact", retire=True, handles=["C2"],
           quote={"handle": "C2", "text": "I stopped the gym"}),
        it(key="nothing", content="x", retire=True, handles=["C2"]),
        it(key="tea", content="likes tea", handles=["C1"]),
        it(key="city", content="lives in Pune", handles=["C1"]),
        it(kind="person", key="lawyer", content="my lawyer is Ana", handles=["C1"],
           quote={"handle": "C1", "text": "Dr. Zorblat"}),
    ]}


def distill_setup() -> list[dict[str, Any]]:
    return distill_ev() + [
        S("clock", at="2026-10-18T03:00:00+00:00"),
        S("memory.save", kind="person", subject="owner", key="dentist", content="my dentist is Dr. Quell",
          origin="owner_stated", sources=[["journal", "j-old"]]),
        S("memory.save", kind="fact", subject="owner", key="gym", content="goes to the gym", origin="inferred",
          sources=[["journal", "j-old"]]),
        S("memory.save", kind="fact", subject="owner", key="swim", content="swims", origin="owner_stated",
          sources=[["journal", "j-old"]]),
        S("memory.save", kind="fact", subject="owner", key="city", content="lives in Pune", origin="inferred",
          sources=[["journal", "j-old"]]),
        S("memory.set_review", id="{last:city}", at="2026-10-18T03:00:00+00:00"),
        S("clock", at="2026-10-21T03:00:00+00:00"),
        S("memory.save", kind="fact", subject="owner", key="old", content="old fact", origin="inferred",
          sources=[["journal", "j-new"]]),
    ]


def sc_distill() -> list[dict[str, Any]]:
    return distill_setup() + [
        S("distill.pending_days"),
        S("distill.yesterday"),
        S("answers", answers=[{"error": "model down"}]),
        S("distill.run_day", day="2026-10-19"),
        S("distill.pending_days"),
        S("answers", answers=[{"answer": distill_answer()}]),
        S("distill.run_day", day="2026-10-19"),
        S("distill.run_day", day="2026-10-19"),
        S("distill.pending_days"),
        S("answers", answers=[{"tight": True, "answer": {"narrative": "", "items": []}}, {"text": "[]"}]),
        S("clock", at="2026-10-23T03:00:00+00:00"),
        trail("t-9", "2026-10-21T10:00:00+05:30", "2026-10-21", "owner", "app_focus", {"app": "Piano"}),
        S("distill.catch_up"),
        S("distill.pending_days"),
        S("memory.current_items"),
    ]


def sc_journal() -> list[dict[str, Any]]:
    return [
        task_row("task-run", "running"),
        S("journal.append", kind="chat.message", data={"text": "hi SECRET-a1", "untrusted_parts": 0}),
        S("journal.append", kind="chat.message", data={"text": "pasted", "untrusted_parts": 2}),
        S("journal.append", kind="chat.message", data={"text": "old"}),
        S("journal.append", kind="owner.verdict", data={"task_id": "task-run"}),
        S("journal.append", kind="trail.day_summary", data={"day": DAY, "apps_min": {"x": 1}, "sites": {}}),
        S("journal.append", kind="trail.day_summary", data={"day": DAY}),
        S("journal.append", kind="trail.day_summary", data={"day": DAY, "ferb": {"tasks": [1]}, "hours": {"1": []}}),
        S("journal.append", kind="trail.day_narrative", data={"day": DAY}),
        S("journal.append", kind="task.started", data={"task_id": "task-run", "n": 1.5, "é": "ü"}),
        S("journal.append", kind="task.started", data={"task_id": "task-done"}, origin="ferb,owner"),
        S("journal.append", kind="x", data=None),
        S("clock", at=at(60)),
        S("journal.append", kind="task.started", data={"task_id": "task-run", "chat": "s2"}),
        S("journal.entries"),
        S("journal.entries", limit=2),
        S("journal.entries", since=at(30)),
        S("journal.entries", since="2026-10-20T15:00:00+05:30", limit=1),
        S("journal.entries_of", kinds=["task.started", "x"]),
        S("journal.kinds_after", kinds=["task.started"], id="id-000010"),
        S("journal.of_kind", kind="task.started"),
        S("journal.of_kind", kind="task.started", field="chat", equals="owner", missing="owner"),
        S("journal.of_kind", kind="task.started", field="chat", equals="s2"),
        S("journal.recent_values", kind="task.started", field="task_id", limit=2),
        S("journal.by_ids", ids=["id-000003", "id-000001", "zz"]),
        S("journal.by_ids", ids=[]),
        S("journal.latest_kind", kinds=["task.started", "owner.verdict"], field="task_id", equals="task-run"),
        S("journal.latest_kind", kinds=["x"], field="task_id", equals="task-run"),
        S("journal.of_running_tasks", kind="task.started"),
        S("questions.keep", id="q1", kind="forget:memory", target="id-1", reason="Forget?", request_id="r1",
          at=at(2)),
        S("questions.keep", id="q2", kind="confirm", target="id-2", reason="Keep?", request_id="r1", at=at(1)),
        S("questions.keep", id="q1", kind="x", target="y", reason="z", request_id="r", at=at(0)),
        S("questions.keep", id="q3", kind="forget:learning", target="id-2", reason="Forget?", request_id="r2",
          at=at(3)),
        S("questions.waiting"),
        S("questions.drop_about", targets=["id-2"], kinds=["confirm"]),
        S("questions.drop_about", targets=[]),
        S("questions.waiting"),
        S("questions.drop_kinds", kinds=[]),
        S("questions.drop_kinds", kinds=["forget:learning"]),
        S("questions.answered", id="q1"),
        S("questions.waiting"),
    ]


def sc_upgrade() -> list[dict[str, Any]]:
    ins = ("INSERT INTO memory_items (id, kind, subject, key, content, origin, status, confidence, observed_at, "
           "valid_from, valid_to, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, 0.7, ?, ?, NULL, ?)")
    rows = [("u1", "fact", "Project:Home Move", "movers", "a", "inferred", "proposed"),
            ("u2", "fact", "project:home_move", "movers", "b", "inferred", "active"),
            ("u3", "fact", "project:home-move", "movers", "c", "owner_stated", "active"),
            ("u4", "fact", "owner", "setup_persona", "dump", "owner_stated", "active"),
            ("u5", "fact", "owner", "city", "pune", "inferred", "active"),
            ("u6", "fact", "owner", "city", "pune!", "inferred", "active")]
    steps = [S("sql", sql=ins, params=[*r, at(n), at(n), at(n)]) for n, r in enumerate(rows)]
    steps += [
        S("learning.put", kind="identity", key="me", body={"text": "x"}, sources=[], origin="owner", owner=True,
          made_by="owner:edit/1", state="active"),
        S("learning.put", kind="fact", key="k", body={"content": "c"}, sources=[], state="active"),
        S("upgrade.clean"),
        S("upgrade.clean"),
        S("upgrade.close_fact_nodes"),
        S("upgrade.close_fact_nodes"),
    ]
    return steps


def sc_mac() -> list[dict[str, Any]]:
    facts = {"scanned_at": "2026-10-20T09:00:00+00:00", "apps": [{"name": "Piano", "uses": 9}],
             "browsers": ["Chrome"], "note": "café"}
    answer = {"summary": "  You mostly  play piano. ", "facts": [
        {"kind": "preference", "key": "main app", "content": "Piano is the most used app"},
        {"kind": "env", "key": "browser", "content": "Chrome is the main browser"},
        {"kind": "bogus", "key": "x", "content": "y"},
        {"kind": "fact", "key": "empty", "content": ""}]}
    answer2 = {"summary": "", "facts": [{"kind": "env", "key": "browser", "content": "Safari is the main browser"}]}
    return [
        S("answers", answers=[{"answer": answer}, {"error": "offline"}]),
        S("mac.interpret", facts=facts),
        S("mac.interpret", facts=facts),
        S("mac.store", facts=facts, answer=answer | {"made_by": "model:fake"}),
        S("mac.previous"),
        S("memory.from_mac_scan", id="{last:browser}"),
        S("profile.entries"),
        S("clock", at=at(days=7)),
        S("mac.store", facts=facts, answer=answer2),
        S("mac.previous"),
        S("profile.entries"),
        S("profile.view"),
    ]


def sc_playbooks() -> list[dict[str, Any]]:
    steps: list[dict[str, Any]] = []
    tasks = [("p1", "order groceries on blinkit", "add milk to the blinkit cart", 1),
             ("p2", "order dinner from blinkit", "pick the paneer", 1),
             ("p3", "order groceries again", "use the saved cart", 1),
             ("p4", "send an outreach email", "draft the outreach mail", 1),
             ("p5", "send the mailing", "use the inbox template", 1),
             ("p6", "order groceries, unverified", "x", 0)]
    for n, (t, goal, how, ver) in enumerate(tasks):
        steps += episode(t, goal, goal, verified=ver, ended=at(10 * n), how=how, seconds=30.0 + n,
                         steps=[{"role": "Worker"}, {"role": "Checker"}] + ([{"role": "Worker"}] if n % 2 else []))
        steps.append(S("learning.put", kind="episode", key=t, body={
            "task_id": t, "intent": goal, "request": goal + " please", "verified": bool(ver), "how": how,
            "ended_at": at(10 * n), "status": "done"}, sources=[], made_by="code:evidence-card/4", origin="ferb",
            state="active"))
        steps.append(span(f"sp-{t}", t, "context.exposure", {"source_refs": []}, started=at(10 * n)))
    steps += [
        S("playbooks.verified_cards"),
        S("playbooks.candidates"),
        S("answers", answers=[
            {"answer": {"same_kind": True, "members": ["p1", "p2", "p3", "zz"], "title": "  Grocery   runs ",
                        "text": "1. open blinkit\n2. use the cart"}},
            {"answer": {"same_kind": False, "members": [], "title": "", "text": ""}}]),
        S("playbooks.merge"),
        S("playbooks.candidates"),
        S("playbooks.repeat_stats"),
    ]
    steps += episode("p7", "order groceries weekly", "order groceries weekly", ended=at(100), how="cart again")
    steps += [
        S("learning.put", kind="episode", key="p7", body={
            "task_id": "p7", "intent": "order groceries weekly", "request": "groceries", "verified": True,
            "how": "cart again", "ended_at": at(100)}, sources=[], made_by="code:evidence-card/4", origin="ferb",
          state="active"),
        span("sp-p7", "p7", "context.exposure", {"source_refs": ["PB"]}, started=at(100)),
        S("playbooks.candidates"),
        S("answers", answers=[{"answer": {"same_kind": True, "members": ["p7"], "title": "",
                                          "text": "open blinkit and reuse the cart"}}]),
        S("playbooks.merge"),
        S("playbooks.repeat_stats"),
        S("learning.search", query="groceries blinkit"),
        S("playbooks.candidates"),
    ]
    return steps


# ---------- import ----------

def chatgpt_export(folder: Path, detail: int = 1300) -> str:
    def conv(cid: str, title: str, turns: list[tuple[str, str, float]]) -> dict[str, Any]:
        mapping: dict[str, Any] = {}
        parent = None
        for n, (role, text, t) in enumerate(turns):
            node = f"{cid}-n{n}"
            mapping[node] = {"id": node, "parent": parent, "children": [],
                             "message": {"author": {"role": role}, "create_time": t,
                                         "content": {"content_type": "text", "parts": [text]}}}
            if parent:
                mapping[parent]["children"].append(node)
            parent = node
        return {"id": cid, "title": title, "create_time": turns[0][2], "update_time": turns[-1][2],
                "mapping": mapping, "current_node": parent}
    base = datetime(2026, 9, 1, tzinfo=timezone.utc).timestamp()
    convs = [conv(f"c{n}", f"chat {n}", [
        ("user", f"I live in {CITIES[n % 2]} and my sister Mira works at a bakery. Note {n}: " + "detail " * detail,
         base + n * 86400),
        ("assistant", "Noted. " * 10, base + n * 86400 + 60),
        ("user", f"I prefer tea over coffee, and I am learning the piano {n}.", base + n * 86400 + 120)])
        for n in range(10)]
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "conversations.json").write_text(json.dumps(convs))
    return str(folder)


CITIES = ["Pune", "Mumbai"]


def recall_answer(ctx: str) -> dict[str, Any]:
    """Candidates for each conversation the recall context shows: two that its first owner message holds, one whose
    quote isn't there, one from its second owner message; and one citing no message."""
    owners: dict[str, list[tuple[str, str]]] = {}
    for line in ctx.split("\n"):
        m = re.match(r"^\[(\S+) · owner · [^\]]+\] (.*)$", line)
        if m:
            owners.setdefault(m.group(1).rsplit(".", 1)[0], []).append((m.group(1), m.group(2)))
    out = []
    for refs in owners.values():
        ref, first = refs[0]
        city = "Pune" if "Pune" in first else "Mumbai"
        out += [{"kind": "fact", "subject": "owner", "key": "city", "content": f"lives in {city}",
                 "message": ref, "quote": f"I live in {city}"},
                {"kind": "person", "subject": "person:mira", "key": "work", "content": "Mira works at a bakery",
                 "message": f"[{ref}]", "quote": "my sister Mira works at a bakery"},
                {"kind": "preference", "subject": "owner", "key": "drink", "content": "prefers tea",
                 "message": ref, "quote": "not in the message"}]
        if len(refs) > 1:
            out.append({"kind": "preference", "subject": "owner", "key": "hobby", "content": "learning the piano",
                        "message": refs[1][0], "quote": "I am learning the piano"})
    out.append({"kind": "fact", "subject": "owner", "key": "x", "content": "y", "message": "zz.1", "quote": "q"})
    return {"candidates": out}


def verify_answer(ctx: str) -> dict[str, Any]:
    """The verifier's answer to a verification context: each candidate supported on the message it was quoted from."""
    verdicts = []
    cand: dict[str, Any] = {}
    for line in ctx.split("\n"):
        m = re.match(r"Candidate (v\d+): kind=(\S+) subject=(\S+) key=(\S+)$", line)
        if m:
            cand = {"id": m.group(1), "kind": m.group(2), "subject": m.group(3), "key": m.group(4)}
            continue
        m = re.match(r"  says: (.*)$", line)
        if m and cand:
            cand["content"] = m.group(1)
            continue
        m = re.match(r"  quoted: “(.*)” \(from (.+)\)$", line)
        if m and cand:
            key = cand["key"]
            verdicts.append(cand | {"supported": key != "work" or cand["id"] != "v2", "message": m.group(2),
                                    "quote": m.group(1), "same_as": "", "ended": False,
                                    "until": "2026-10-10" if key == "work" else "",
                                    "review_in_days": 30 if key == "hobby" else 0})
            cand = {}
    return {"verdicts": verdicts}


def import_steps(path: str) -> list[dict[str, Any]]:
    return [
        S("clock", at="2026-10-25T09:00:00+00:00"),
        S("import.estimate", source="chatgpt", path=path),
        S("import.estimate", source="nope", path=path),
        S("import.estimate", source="chatgpt", path=""),
        S("import.create", source="chatgpt", path=path),
        S("import.create", source="chatgpt", path=path),
        S("import.running"),
        S("import.status"),
    ]


def import_answers(chunks: list[Any]) -> list[Any]:
    return [{"dynamic": "import"}] * (6 * len(chunks))


# ======================================================================================================================

def resolve(env: Env, value: Any) -> Any:
    """Ids only the run knows, named in a step: {first:KEY} / {last:KEY} (a memory item by its key, oldest or newest),
    {node:KIND} (the oldest learned node of a kind). The fixture records the resolved step."""
    if isinstance(value, dict):
        return {k: resolve(env, v) for k, v in value.items()}
    if isinstance(value, list):
        return [resolve(env, v) for v in value]
    if not isinstance(value, str) or "{" not in value:
        return value
    c = env.db.conn._conn

    def one(m: Any) -> str:
        which, what = m.group(1), m.group(2)
        if which == "node":
            r = c.execute("SELECT id FROM learning_nodes WHERE kind = ? ORDER BY recorded_at, id", (what,)).fetchone()
        else:
            r = c.execute(f"SELECT id FROM memory_items WHERE key = ? ORDER BY seq {'' if which == 'first' else 'DESC'}",
                          (what,)).fetchone()
        return str(r[0]) if r else "missing"
    return re.sub(r"\{(first|last|node):([^}]+)\}", one, value)


async def scenario(name: str, steps: list[dict[str, Any]]) -> dict[str, Any]:
    env = Env(name)
    resolved: list[dict[str, Any]] = []
    out = []
    before = dump(env)
    for step in steps:
        args = resolve(env, json.loads(json.dumps(step.get("args", {}))))
        step = {"op": step["op"], "args": args}
        resolved.append(step)
        env.events = []
        try:
            got: Any = {"ok": j(await op(env, step["op"], args))}
        except Exception as e:
            got = raised(e)
        after = dump(env)
        changed = {t: rows for t, rows in after.items() if before.get(t) != rows}
        before = after
        out.append({"op": step["op"], "args": args, "out": got, "events": j(env.events), "tables": changed})
    return {"name": name, "steps": out}


async def import_scenario(name: str, busy: int = 0, broken: bool = False) -> tuple[dict[str, Any], Env]:
    path = chatgpt_export(ROOT / name / "export")
    env = Env(name)
    convs = env.importer.read("chatgpt", path)
    plan = env.importer.plan("chatgpt", convs)
    answers = import_answers(plan.chunks)
    if broken:
        answers = [{"error": "e1"}, {"error": "e2"}, {"error": "e3"}] + answers
    steps = import_steps(path) + [S("busy", polls=busy), S("answers", answers=answers),
                                  S("import.run", run="id-000001"), S("import.status"),
                                  S("import.tell_done", run="id-000001"),
                                  S("import.run", run="id-000001"),
                                  S("import.create", source="chatgpt", path=path),
                                  S("answers", answers=import_answers(plan.chunks)),
                                  S("import.run", run="id-NEXT"),
                                  S("import.estimate", source="chatgpt", path=path),
                                  S("memory.current_items")]
    # the second create's id is the next sequential id at that point: found by running
    out = []
    before = dump(env)
    for step in steps:
        args = dict(step.get("args", {}))
        if args.get("run") == "id-NEXT":
            args["run"] = env.importer.running()[0] if env.importer.running() else "none"
        env.events = []
        try:
            got: Any = {"ok": j(await op(env, step["op"], args))}
        except Exception as e:
            got = raised(e)
        after = dump(env)
        changed = {t: rows for t, rows in after.items() if before.get(t) != rows}
        before = after
        out.append({"op": step["op"], "args": args, "out": got, "events": j(env.events), "tables": changed})
    return {"name": name, "steps": out, "export": json.loads((Path(path) / "conversations.json").read_text())}, env


def normalized(env: Env) -> dict[str, Any]:
    """What a restart run must end with, ids aside: the facts, the parts done, the run's counts and status."""
    c = env.db.conn._conn
    facts = sorted([list(r) for r in c.execute(
        "SELECT kind, subject, key, content, origin, status, valid_to IS NULL FROM memory_items")])
    done = sorted([list(r) for r in c.execute("SELECT source, part FROM memory_import_done")])
    runs = [[r[0], json.loads(r[1])] for r in c.execute("SELECT status, counts_json FROM memory_import_runs")]
    for _, counts in runs:
        counts.pop("saved_ids", None)
    said = sorted([list(r) for r in c.execute(
        "SELECT m.content, i.source, i.said_at FROM memory_import_items i JOIN memory_items m ON m.id = i.item_id")])
    kinds = sorted([r[0] for r in c.execute("SELECT kind FROM journal")])
    return {"facts": facts, "done": done, "runs": runs, "import_items": said, "journal_kinds": kinds}


RESTART_CHUNK_CHARS = 4000


async def import_restarts() -> dict[str, Any]:
    """The import run once counting its writes, then failing at each write in turn and started again."""
    # smaller conversations and chunks than the import scenarios: the same boundaries, fewer tokens to count
    path = chatgpt_export(ROOT / "imp-count" / "export", detail=250)

    async def setup(name: str) -> Env:
        env = Env(name)
        env._answers = [{"dynamic": "import"}] * 400    # type: ignore[attr-defined]
        env.clock.t = datetime.fromisoformat("2026-10-25T09:00:00+00:00")
        env.answers = list(env._answers)    # type: ignore[attr-defined]
        env.importer.chunk_chars = RESTART_CHUNK_CHARS
        env.importer.create("chatgpt", path)
        return env

    env = await setup("imp-count")
    start = env.writes
    await env.importer.run("id-000001")
    total = env.writes - start
    clean = normalized(env)
    runs = []
    for k in range(1, total + 1):
        env = await setup(f"imp-{k}")
        env.fail_at = env.writes + k
        try:
            await env.importer.run("id-000001")
            first = "finished"
        except sqlite3.OperationalError as e:
            first = str(e)
        env.fail_at = None
        env.importer = env.new_importer()
        env.importer.chunk_chars = RESTART_CHUNK_CHARS
        env.answers = list(env._answers)    # type: ignore[attr-defined]
        try:
            await env.importer.run("id-000001")
            second = "finished"
        except Exception as e:
            second = f"{type(e).__name__}: {e}"
        runs.append({"fail_at": k, "first": first, "second": second, "state": normalized(env)})
    return {"export": json.loads((Path(path) / "conversations.json").read_text()), "chunk_chars": RESTART_CHUNK_CHARS,
            "writes": total,
            "clean": clean, "runs": runs}


async def distill_restarts() -> dict[str, Any]:
    """A day's distill failing at each of its writes, then run again (the next hourly pass)."""
    steps: list[dict[str, Any]] = []

    async def setup(name: str) -> Env:
        env = Env(name)
        done = []
        for step in distill_setup():
            args = resolve(env, json.loads(json.dumps(step["args"])))
            done.append({"op": step["op"], "args": args})
            await op(env, step["op"], args)
        steps[:] = done
        return env

    env = await setup("dr-count")
    env.answers = [{"answer": distill_answer()}]
    start = env.writes
    await env.distiller.run_day("2026-10-19")
    total = env.writes - start
    clean = dump(env)
    runs = []
    for k in range(1, total + 1):
        env = await setup(f"dr-{k}")
        env.answers = [{"answer": distill_answer()}, {"answer": distill_answer()}]
        env.fail_at = env.writes + k
        steps_out = []
        for attempt in range(2):
            env.events = []
            try:
                got: Any = {"ok": j(await env.distiller.run_day("2026-10-19"))}
            except Exception as e:
                got = raised(e)
            env.fail_at = None
            steps_out.append({"out": got, "events": j(env.events)})
        tables = dump(env)
        runs.append({"fail_at": k, "attempts": steps_out,
                     "tables": {t: rows for t, rows in tables.items() if rows != clean[t]}})
    return {"writes": total, "clean": clean, "runs": runs, "setup": steps, "answer": {"answer": distill_answer()}}


# ======================================================================================================================
# pure helpers
# ======================================================================================================================

OWNER_TEXTS = [
    "my dentist is Dr. Zorblat",
    "Never share my links publicly, just summarize them",
    "If it's a work email, sign it as Sam; otherwise just Sam.",
    "remember: my prof. Zorblat teaches on Mondays!\nand my sister is Mira",
    "I LIKE TEA\n\nand coffee",
    "Straße and STRASSE, İstanbul or ıstanbul, ſoft",
    "the the the",
    "call me at 555-0100 (cell)",
    "",
    "  spaced   out\twords  ",
    "Ελληνικά ΣΟΦΙΑ σοφια, ﬁle",
    "मेरा नाम राम है, okay?",
]
PARTS = ["my dentist", "Dr. Zorblat", "the", "Never share my links publicly", "sign it as Sam", "Sam",
         "prof. Zorblat teaches", "I like tea", "coffee", "I like tea\n\ncoffee", "straße", "STRASSE", "istanbul",
         "ıstanbul", "soft", "the the", "555-0100", "(cell)", "spaced out words", "σοφια", "ΣΟΦΙΑ", "file",
         "राम", "नाम राम", "", "  ", "“Dr. Zorblat”", "mira", "and my sister is Mira", "Zorbla", "tea\n  \ncoffee"]


def pure() -> dict[str, Any]:
    out: dict[str, Any] = {"text": [], "fingerprint": [], "capture": [], "lineage": [], "notes": [], "origin": [],
                           "steps": [], "batches": []}
    for t in OWNER_TEXTS:
        for p in PARTS:
            out["text"].append({"text": t, "part": p, "contains": items.contains_words(t, p),
                                "found": items.owner_span_at(t, p), "clauses": items.owner_clauses(t, p),
                                "whole": items.whole_clauses(t, p), "quoted": items.quoted(t, p),
                                "quoted_clause": items.quoted(t, p, clause=True)})
            out["capture"].append({"own": t, "content": p, "quote": p, **j(items.capture(t, p, p))})
            out["capture"].append({"own": t, "content": "likes tea", "quote": p, **j(items.capture(t, "likes tea", p))})
    for args in [("fact", "owner", "Key", "  Some   Content "), ("preference", "Project:X", "k", "ÄBC"),
                 ("fact", "", "", "")]:
        out["fingerprint"].append({"args": list(args), "out": items.fingerprint(*args)})
    refs = ["entry:j1", "task:t1", "journal:x", "trail:y", "memory:z", "entry:", "e"]
    out["lineage"] = [{"ref": r, "direct": lineage.direct(r)} for r in refs]
    calls = [
        ({"tool": "memory.read", "args": {"item_id": "i1"}}, {"source_refs": ["a"], "items": [{"id": "i2"}, 3],
                                                              "text": json.dumps({"items": [{"id": "i3"}]})}),
        ({"tool": "memory.learned", "args": {}}, {"candidates": [{"id": "n1"}, {"x": 1}], "text": "not json"}),
        ({"tool": "memory.read"}, None),
        ({"tool": "trail.query", "args": {"event_id": "e1"}}, {"events": [{"id": "e2"}, {"id": ""}]}),
        ({"tool": "trail.days"}, {"days": [{"id": "d1"}, {}], "source_refs": ["s1"]}),
        ({"tool": "task.evidence"}, {"source_refs": ["s2"], "records": [
            {"kind": "context.source", "input": {"source_refs": ["r1"]}},
            {"kind": "tool.call", "input": {"tool": "trail.query", "args": {"event_id": "e9"}}, "output": {}},
            {"kind": "other"}], "text": json.dumps({"kind": "context.exposure", "input": {"source_refs": ["r2"]}})}),
        ({"tool": "fs.read"}, {"source_refs": ["ignored"]}),
    ]
    out["lineage_calls"] = [{"input": i, "result": r, "out": sorted(lineage.call_sources(i, r))} for i, r in calls]
    bodies = [
        ("fact", "k", {"content": "  likes   tea ", "subject": "project:home-move", "key": "bookings_username"}),
        ("identity", "me", {"text": "owner text"}),
        ("episode", "t1", {"intent": "order", "narrative": "done it", "status": "done", "how": "used   cart",
                           "owner_hints": ["  hard refresh ", ""], "verified": True, "task_id": "t1",
                           "ended_at": "2026-10-20T10:00:00+00:00",
                           "actions": [
                               {"tool": "ferb_browser.open", "args": {"url": "https://www.x.com/a/b?q=1"}},
                               {"tool": "browser.open", "args": {"url": "https://www.x.com/a/b?q=2"}},
                               {"tool": "browser.open", "args": {"url": "notaurl"}},
                               {"tool": "fs.read", "args": {"path": "/a/b/c.txt"}},
                               {"tool": "fs.read", "args": {"path": "/"}},
                               {"tool": "mac.click", "args": {"app": "  "}},
                               {"tool": "x", "error": "boom"},
                               {"tool": "y", "result": {"ok": False}},
                               {"tool": ""},
                               {"tool": "browser.open", "args": {"url": "https://HOST.example:8080/"}},
                           ] + [{"tool": f"t{n}"} for n in range(14)]}),
        ("episode", "t2", {"intent": "x", "verified": False, "started_at": "2026-10-19", "how": "h" * 700}),
        ("episode", "t3", {"verified": True, "actions": [{"tool": "a", "error": "e"}]}),
        ("playbook", "pb", {"title": "Groceries", "text": "do it", "tasks": ["a", "b"]}),
        ("episode_annotation", "t1", {"reflection": "fine", "narrative": "n"}),
        ("activity_summary", "trail.day_summary:d", {"text": "a day"}),
        ("activity_summary", "trail.day_summary:d", {"day": "d"}),
        ("summary", "s", {"text": "x"}),
        ("lesson", "l", {"text": "a lesson"}),
        ("lesson", "l", {}),
    ]
    for kind, key, body in bodies:
        node = {"kind": kind, "key": key, "body": body, "recorded_at": "2026-10-20T09:00:00+00:00",
                "state": "active"}
        out["notes"].append({"kind": kind, "key": key, "body": body,
                             "search_text": learning.search_text(kind, key, body),
                             "shown_text": learning.shown_text(node), "how_note": learning.how_note(body),
                             "shown_state": learning.shown_state(node),
                             "encoded": learning.encoded(body), "digest": learning.digest([kind, key, body])})
    out["notes"].append({"slug_words": [[s, learning.slug_words(s)] for s in
                                        ["bookings_username", "project:home-move", "a:b:c", ""]]})
    out["origin"] = [[lbl, learning.memory_origin(lbl)] for lbl in ["owner_stated", "observed", "inferred",
                                                                    "third_party", "x"]]
    out["journal_origin"] = [[k, d, journal_origin(k, d)] for k, d in [
        ("chat.message", {"untrusted_parts": 0}), ("chat.message", {"untrusted_parts": 3}),
        ("chat.message", {}), ("memory.added", {}), ("trail.day_summary", {}),
        ("trail.day_summary", {"folders": {"a": 1}, "windows": ["w"]}), ("trail.day_narrative", {}),
        ("trail.day_summary", {"ferb": {"tasks": []}}), ("trail.day_summary", {"ferb": {"tasks": [1]}}),
        ("x", {})]]
    out["attempts"] = [[s, playbooks.attempts(s)] for s in [[], [{"role": "Worker"}, {"role": "Worker"}],
                                                             [{"role": "Checker"}]]]
    out["group_key"] = [[t, playbooks._group_key(t)] for t in [["b", "a"], [], ["x"]]]
    return out


async def main() -> None:
    scenarios = []
    for name, fn in [("items", sc_items), ("forget", sc_forget), ("profile", sc_profile),
                     ("profile_seed", sc_profile_seed), ("learning", sc_learning),
                     ("learning_owner", sc_learning_owner), ("cards", sc_cards), ("cleanout", sc_cleanout),
                     ("summary", sc_summary), ("distill", sc_distill), ("journal", sc_journal),
                     ("upgrade", sc_upgrade), ("mac", sc_mac), ("playbooks", sc_playbooks)]:
        scenarios.append(await scenario(name, fn()))
    imports = []
    for name, busy, broken in [("import", 2, False), ("import_broken", 0, True)]:
        sc, _ = await import_scenario(name, busy, broken)
        imports.append(sc)
    restarts = {"import": await import_restarts(), "distill": await distill_restarts()}
    write("scenarios.json", {"t0": T0.isoformat(), "sealed": SEALED, "digests": DIGESTS, "scenarios": scenarios,
                             "imports": imports})
    write("restarts.json", restarts)
    write("pure.json", pure())


def write(name: str, data: Any) -> None:
    text = json.dumps(data, ensure_ascii=True, separators=(",", ":"))
    (HERE / name).write_text(text.replace(str(ROOT), "/FX") + "\n")


if __name__ == "__main__":
    asyncio.run(main())
