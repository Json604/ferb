"""Own FERB's waiting questions about memory (`memory_questions`, migration 0041): a forget's Yes / No kept until it is
answered, so a restart asks it again (rule 9). Must not decide what an answer does (the daemon's memory area does)."""

from typing import Any, Iterable

from ferb.store.db import Database


class MemoryQuestions:
    def __init__(self, db: Database) -> None:
        self.db = db

    def keep(self, approval_id: str, kind: str, target: str, reason: str, request_id: str, at: str) -> None:
        self.db.conn.execute("INSERT OR IGNORE INTO memory_questions VALUES (?,?,?,?,?,?)",
                             (approval_id, kind, target, reason, request_id, at))

    def waiting(self) -> list[Any]:
        """Every question still waiting, oldest first."""
        return self.db.conn.execute("SELECT * FROM memory_questions ORDER BY at").fetchall()

    def answered(self, approval_id: str) -> None:
        self.db.conn.execute("DELETE FROM memory_questions WHERE approval_id = ?", (approval_id,))

    def drop_about(self, targets: Iterable[str], kinds: tuple[str, ...] | None = None) -> None:
        """Questions (of `kinds`; None: every kind) about these targets, which stopped waiting."""
        gone = sorted(set(targets))
        if not gone:
            return
        which = f" AND kind IN ({','.join('?' * len(kinds))})" if kinds is not None else ""
        self.db.conn.execute(f"DELETE FROM memory_questions WHERE target IN ({','.join('?' * len(gone))}){which}",
                             [*gone, *(kinds or ())])

    def drop_kinds(self, kinds: Iterable[str]) -> None:
        kinds = tuple(kinds)
        if kinds:
            self.db.conn.execute(f"DELETE FROM memory_questions WHERE kind IN ({','.join('?' * len(kinds))})", kinds)
