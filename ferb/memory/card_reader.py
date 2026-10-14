"""Reads one episode card in a process of its own (L28). Must not write: the daemon writes the card.

The lineage resolution behind a card is CPU-bound (1.3-1.6 s on the owner's data). In a thread it would still hold the
interpreter lock against the daemon's event loop, so it runs here, on a read-only SQLite connection of its own.
usage: python -m ferb.memory.card_reader DB TASK_ID < apps-json; prints {"prepared": ...} or {"error": "..."}."""
import json
import sqlite3
import sys
from pathlib import Path
from types import SimpleNamespace
from typing import Any


def read(db_path: Path, task_id: str, apps: dict[str, str] | None) -> dict[str, Any] | None:
    from ferb.memory.learning import Learning
    conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    conn.row_factory = sqlite3.Row
    try:
        reader = Learning(SimpleNamespace(conn=conn, path=db_path), None, lambda *a, **k: None, lambda text: text)
        return reader.prepare_episode(task_id, apps)
    finally:
        conn.close()


def main() -> None:
    db_path, task_id = Path(sys.argv[1]), sys.argv[2]
    apps = json.loads(sys.stdin.read() or "null")
    try:
        out: dict[str, Any] = {"prepared": read(db_path, task_id, apps)}
    except Exception as exc:                 # the daemon journals it; no traceback with record contents
        out = {"error": f"{type(exc).__name__}: {exc}"}
    sys.stdout.write(json.dumps(out))


if __name__ == "__main__":
    main()
