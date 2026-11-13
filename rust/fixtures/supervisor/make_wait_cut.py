"""Fixture for supervisor.py:5657 `_wait_cut` (ORCH-633): the plain fact a step's next run gets when FERB's restart
ended a wait.for of its tree before it woke. Runs the real method on the real journal (ferb.memory.journal.Journal,
on a temporary SQLite store) with invented rows, in two time zones (the time is shown as the Mac's local time).

    cd <repo> && uv run python rust/fixtures/supervisor/make_wait_cut.py

Writes wait_cut.json beside this file: each case's rows (kind, at, data), the zone, the tree asked about and the text
(null: no fact). Starts no program. Invented data only; the same bytes on every run.
"""

import json
import os
import sys
import tempfile
import time
from datetime import datetime
from pathlib import Path
from types import SimpleNamespace
from typing import Any

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(REPO))

CASES: list[dict[str, Any]] = [
    {"name": "one_cut", "zone": "UTC", "tree": "t1",
     "rows": [["wait.ended", "2026-01-01T09:15:00+00:00", {"tree": "t1", "line": "until the invented parcel arrives"}]]},
    {"name": "latest_of_two_local_time", "zone": "Asia/Kolkata", "tree": "t1",
     "rows": [["wait.ended", "2026-01-01T09:15:00+00:00", {"tree": "t1", "line": "the first wait"}],
              ["wait.ended", "2026-01-01T22:40:00+00:00", {"tree": "t1", "line": "the second wait"}]]},
    {"name": "no_line", "zone": "UTC", "tree": "t1",
     "rows": [["wait.ended", "2026-01-01T09:15:00+00:00", {"tree": "t1", "line": ""}]]},
    {"name": "control_characters", "zone": "UTC", "tree": "t1",
     "rows": [["wait.ended", "2026-01-01T09:15:00+00:00", {"tree": "t1", "line": "a\u0007 wait\u009b with\ttab"}]]},
    {"name": "another_tree", "zone": "UTC", "tree": "t2",
     "rows": [["wait.ended", "2026-01-01T09:15:00+00:00", {"tree": "t1", "line": "not this tree's"}]]},
    {"name": "none", "zone": "UTC", "tree": "t1", "rows": []},
]


def main() -> None:
    from ferb.memory.journal import Journal
    from ferb.orchestrator.supervisor import Supervisor
    from ferb.store.db import Database

    out = []
    for case in CASES:
        os.environ["TZ"] = case["zone"]
        time.tzset()
        with tempfile.TemporaryDirectory() as tmp:
            db = Database(Path(tmp) / "ferb.db")
            db.migrate()
            clock = SimpleNamespace(at=None)
            clock.now = lambda: clock.at
            counter = iter(range(1, 100))
            ids = SimpleNamespace(new_id=lambda: f"id-{next(counter):06d}")
            journal = Journal(db, clock, ids, lambda d: d)  # type: ignore[arg-type]
            for kind, at, data in case["rows"]:
                clock.at = datetime.fromisoformat(at)
                journal.append(kind, data)
            fake = SimpleNamespace(journal=journal)
            text = Supervisor._wait_cut(fake, case["tree"])  # type: ignore[arg-type]
            db.close()
        out.append({**case, "text": text})
    os.environ["TZ"] = "UTC"
    time.tzset()
    (HERE / "wait_cut.json").write_text(json.dumps({"made_by": "rust/fixtures/supervisor/make_wait_cut.py",
                                                    "cases": out}, indent=1, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    main()
