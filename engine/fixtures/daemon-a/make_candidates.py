"""Fixture for ferb/daemon/areas/memory.py `_learning_candidates` (MEM-27, MEM-12): the memory the attention check
(`memory_search.ATTENTION`) and a worker (`memory_search.WORKER`) are given, from the real Memory, Learning and
CoreProfile on a migrated store in a temporary folder, with a fixed clock and sequential ids, no local model (the
keyword and recency ranking; the vector path is ferb-memory's, verified there).

    cd <repo> && uv run python rust/fixtures/daemon-a/make_candidates.py

Writes candidates.json beside this file: `now`, the rows the seeding left in the tables the search reads (`tables`:
columns and rows in rowid order; a Rust test inserts them into its own store), and each query's `out` (the candidate
dicts). Starts no program; reads nothing outside its temporary folder. Invented data only; the same bytes on every run.
"""

import asyncio
import json
import os
import sys
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path
from types import MethodType, SimpleNamespace
from typing import Any

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(REPO))

NOW = datetime(2026, 1, 10, 9, 0, tzinfo=timezone.utc)
TABLES = ["journal", "memory_items", "memory_sources", "memory_context", "memory_profile", "memory_set_aside",
          "learning_nodes", "learning_sources", "learning_fts", "learning_titles", "learning_search_version"]

QUERIES = [
    {"words": "where do the garden beans go", "limits": "attention", "focus": ""},
    {"words": "garden beans watering", "limits": "worker", "focus": "garden"},
    {"words": "the invented bicycle shop", "limits": "worker", "focus": ""},
    {"words": "dentist", "limits": "attention", "focus": ""},
    {"words": "   ", "limits": "worker", "focus": ""},
    {"words": "garden", "limits": "worker", "focus": "beans"},
    {"words": "nothing matches zzqx", "limits": "attention", "focus": ""},
]


def main() -> None:
    from ferb.clock.clock import FakeClock
    from ferb.core.ids import SequentialIds
    from ferb.daemon.areas.memory import MemoryGlue
    from ferb.memory import search as memory_search
    from ferb.memory.items import Memory
    from ferb.memory.learning import Learning
    from ferb.memory.profile import CoreProfile
    from ferb.store.db import Database

    with tempfile.TemporaryDirectory() as tmp:
        os.environ["FERB_HOME"] = tmp
        os.environ["HOME"] = tmp
        db = Database(Path(tmp) / "ferb.db")
        db.migrate()
        clock = FakeClock(NOW - timedelta(days=5))
        ids = SequentialIds("id")
        memory = Memory(db, clock, ids, lambda t, d: None)
        learning = Learning(db, clock, lambda *a, **k: None, lambda s: s)
        profile = CoreProfile(memory, clock)

        def said(ident: str, text: str, origin: str = "owner") -> None:
            db.conn.execute("INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?)",
                            (ident, clock.now().isoformat(), "chat.message",
                             json.dumps({"session_id": "s1", "request_id": "r-" + ident, "text": text,
                                         "untrusted_parts": 0}), origin))

        said("j-1", "the garden beans go in the north bed")
        said("j-2", "water the garden twice a week")
        said("j-3", "my dentist is Dr. Zorblat")
        said("j-4", "the invented bicycle shop opens at nine", "web")
        beans = memory.save("fact", "project:garden", "beans", "The garden  beans go in the\nnorth bed", "owner_stated",
                            [("journal", "j-1")], 1.0, about="the garden plan").item
        clock.advance(3600)
        memory.save("preference", "owner", "watering", "Water the garden twice a week", "owner_stated",
                    [("journal", "j-2")], 1.0)
        clock.advance(3600)
        dentist = memory.save("person", "owner", "dentist", "My dentist is Dr. Zorblat", "owner_stated",
                              [("journal", "j-3")], 1.0).item
        memory.save("fact", "world", "bicycle shop", "The invented bicycle shop opens at nine", "third_party",
                    [("journal", "j-4")], 0.5)
        memory.save("fact", "project:garden", "squash", "Garden squash goes in the south bed", "inferred",
                    [("journal", "j-1")], 0.6)
        for i in range(12):                       # more garden facts than a worker's 10 items: a closing count line
            clock.advance(60)
            memory.save("fact", "project:garden", f"row {i}", f"Garden row {i} holds invented beans variety {i}",
                        "inferred", [("journal", "j-1")], 0.6)
        profile.add(dentist.id)                   # in every prompt already: takes no room
        clock.advance(3600)
        learning.put("episode", "task-7", {"task_id": "task-7", "intent": "plant the garden beans",
                                            "status": "done", "verified": True, "ended_at": clock.now().isoformat()},
                     ["journal:j-1"], made_by="test", state="active")
        learning.put("playbook", "garden-beans", {"goal": "plant garden beans", "steps": ["dig", "sow"],
                                                   "tasks": ["task-7"]}, ["journal:j-1"], made_by="test",
                     state="candidate")
        learning.put("activity_summary", "trail.day_summary:2026-01-08",
                      {"text": "The owner read about garden beans and the invented bicycle shop."}, ["journal:j-2"],
                      made_by="test", state="active")
        clock.set_wall(NOW)

        glue = SimpleNamespace(learning=learning, memory=memory, profile=profile, db=db, clock=clock,
                               _reindex_task=None)
        for name in ("_learning_candidates", "_search_ready", "_query_vector", "_memory_search", "_fact_origin"):
            setattr(glue, name, MethodType(getattr(MemoryGlue, name), glue))
        glue._journal = lambda: SimpleNamespace(append=lambda *a, **k: None)
        glue._redact_text = lambda s: s

        cases = []
        for q in QUERIES:
            limits = memory_search.ATTENTION if q["limits"] == "attention" else memory_search.WORKER
            out = asyncio.run(glue._learning_candidates(q["words"], limits, focus=q["focus"]))
            cases.append({**q, "out": out})

        tables = {}
        for t in TABLES:
            cur = db.conn.execute(f"SELECT rowid AS _rowid, * FROM {t} ORDER BY rowid")
            cols = [c[0] for c in cur.description]
            tables[t] = {"columns": cols, "rows": [list(r) for r in cur.fetchall()]}
        db.close()
    (HERE / "candidates.json").write_text(json.dumps({"made_by": "rust/fixtures/daemon-a/make_candidates.py",
                                                      "now": NOW.isoformat(), "tables": tables, "cases": cases},
                                                     indent=1, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    main()
