"""Episode-card fixtures (L28): a FERB home whose database holds finished tasks, and card readers that stand in for
`python -I -m ferb.memory.card_reader` in a test daemon (one that answers late, one that never answers). Also the
cards the composer before memory review C1 wrote, for the one-time clean-out (plan 2026-10-23 Task 1)."""
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable

from ferb.core import origin
from ferb.core.ids import RandomIds
from ferb.memory.learning import COPYING_CARD_MAKER, Learning
from ferb.memory.lineage import PROCEDURE_LABEL, REQUEST_REFERENCE
from ferb.orchestrator import episodes
from ferb.sanitize.redact import Redactor
from ferb.store.db import Database
from ferb.store.spans import SpanStore
from ferb.trace import compose
from tests.support.trace_builder import TraceBuilder


class Clock:
    def now(self) -> datetime:
        return datetime(2026, 10, 14, 9, tzinfo=timezone.utc)


def card_store(home: Path) -> tuple[Database, Redactor, SpanStore, Learning]:
    """The stores of a FERB home (`home/ferb.db`, where the card reader looks), with real redaction."""
    db = Database(home / "ferb.db")
    db.migrate()
    redactor = Redactor()
    spans = SpanStore(db, Clock(), RandomIds(), redactor)
    return db, redactor, spans, Learning(db, Clock(), lambda *args: None, redactor.text)


def finished_task(db: Database, spans: SpanStore, learning: Learning, task_id: str, refs: Iterable[str] = (),
                  reads: int = 0, shown: Iterable[str] = ()) -> None:
    """A finished task's permanent record, a tool call of its own whose evidence cites `refs` (direct sources of its
    card), a planner exposure citing `shown` (what its planner was only shown: never a card source, C1), and `reads`
    recorded tool calls."""
    trace = TraceBuilder(task_id=task_id, request="check the tea order", goal="answer about the tea order").finish()
    episodes.write(db, compose.compose(trace) | {"made_by": episodes.MADE_BY, "built_at": learning.now()})
    if refs:
        call = spans.start("tool.call", task_id=task_id, input={"tool": "task.evidence", "args": {}, "kind": "read"})
        spans.finish(call, output={"source_refs": list(refs), "records": []}, origin="inferred")
    if shown:
        exposure = spans.start("context.exposure", task_id=task_id,
                               input={"source_refs": list(shown), "reference": "Layer 4 — memory"})
        spans.finish(exposure, output={"text": "held"}, origin="inferred")
    for n in range(reads):
        call = spans.start("tool.call", task_id=task_id, input={"tool": "fs.read", "args": {"path": f"/tmp/{n}"},
                                                                "kind": "read"})
        spans.finish(call, output={"ok": True}, origin="private")


def traced_task(db: Database, spans: SpanStore, learning: Learning, task_id: str,
                shown: int = 40) -> tuple[set[str], set[str]]:
    """A finished task recorded as the supervisor and the gateway record it: the blocks its planner was shown
    (`context.exposure`: the owner's message, today's activity, the conversation), its planner snapshot and its worker's
    procedure (`context.source`), and its own tool calls (`tool.call`: a trail read, an evidence read citing a
    conversation entry and a task by their handles). Returns the refs the current card rule keeps, and those its planner
    was only shown (`shown` activity rows among them), which the composer before memory review C1 also copied."""
    trace = TraceBuilder(task_id=task_id, request="check the tea order", goal="answer about the tea order").finish()
    episodes.write(db, compose.compose(trace) | {"made_by": episodes.MADE_BY, "built_at": learning.now()})

    def record(kind: str, inp: dict[str, Any], out: dict[str, Any], where: str) -> None:
        span = spans.start(kind, task_id=task_id, input=inp)
        spans.finish(span, output=out, origin=where)
    activity = [f"trail:shown-{task_id}-{n}" for n in range(shown)]
    record("context.exposure", {"source_refs": [f"journal:req-{task_id}"], "reference": REQUEST_REFERENCE},
           {"text": "Layer 5 — owner's complete new message:\ncheck the tea order"}, origin.OWNER)
    record("context.exposure", {"source_refs": activity, "reference": "task.evidence query=context.source; memory.read; "
                                "trail.query"}, {"text": "Layer 3 — today's activity (data): …"}, origin.PRIVATE)
    record("context.exposure", {"source_refs": [f"entry:chat-{task_id}", "memory:m-shown"], "reference": "chat.history"},
           {"text": None, "payload": "earlier messages"}, origin.OWNER)
    record("context.source", {"label": "planner-context", "source_refs": [f"trail:snap-{task_id}"]},
           {"text": "[snapshot]"}, origin.PRIVATE)
    record("context.source", {"label": PROCEDURE_LABEL, "source_refs": [f"m-proc-{task_id}", "task:earlier"]},
           {"text": "Procedure …"}, origin.INFERRED)
    record("tool.call", {"tool": "trail.query", "args": {"event_id": f"ev-{task_id}-a"}, "effect": "read",
                         "kind": "read", "target": {"site": None}},
           {"events": [{"id": f"ev-{task_id}-b", "what": "focus"}]}, origin.PRIVATE)
    record("tool.call", {"tool": "task.evidence", "args": {}, "effect": "read", "kind": "read", "target": {}},
           {"source_refs": ["entry:ev-entry", "task:other"], "records": []}, origin.INFERRED)
    kept = {f"episode:{task_id}", f"journal:req-{task_id}", f"m-proc-{task_id}", "episode:earlier",
            f"trail:ev-{task_id}-a", f"trail:ev-{task_id}-b", "journal:ev-entry", "episode:other"}
    # the composer before C1 resolved a handle into the entry and what it cites, and copied every block's refs
    only_shown = set(activity) | {f"journal:chat-{task_id}", "memory:m-shown", f"trail:snap-{task_id}"}
    return kept, only_shown


def copied_cards(learning: Learning, task_id: str, sources: Iterable[str], versions: int = 3) -> list[str]:
    """Versions of a task's card as the composer before C1 wrote them (COPYING_CARD_MAKER), each citing `sources`;
    oldest first, the last one current."""
    return [learning.put("episode", task_id, {"task_id": task_id, "version": n}, sources, made_by=COPYING_CARD_MAKER,
                         origin_label=origin.FERB, state="active", labels={origin.PRIVATE, origin.OWNER})["id"]
            for n in range(versions)]


def sources_of(db: Database, node_id: str) -> set[str]:
    return {r[0] for r in db.conn.execute("SELECT source_ref FROM learning_sources WHERE node_id = ?", (node_id,))}


def slow_reader(seconds: float) -> list[str]:
    """Reads the card exactly as the real reader does, then answers `seconds` later: a capture still in flight."""
    code = ("import json, sys, time\n"
            "from ferb.memory.card_reader import read\n"
            "request = json.loads(sys.stdin.read())\n"
            "out = {'prepared': read(request['task_id'], request['apps'])}\n"
            f"time.sleep({seconds})\n"
            "sys.stdout.write(json.dumps(out))\n")
    return [sys.executable, "-I", "-c", code]


# Never answers: only killing it ends it.
HANGING_READER = [sys.executable, "-I", "-c", "import sys, time\nsys.stdin.read()\ntime.sleep(600)\n"]
