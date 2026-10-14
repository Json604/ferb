"""Episode-card fixtures (L28): a FERB home whose database holds finished tasks, and card readers that stand in for
`python -I -m ferb.memory.card_reader` in a test daemon (one that answers late, one that never answers)."""
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable

from ferb.core.ids import RandomIds
from ferb.memory.learning import Learning
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
