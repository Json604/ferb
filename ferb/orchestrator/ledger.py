"""Action ledger: intended -> executing -> done | failed | unknown (section 7 step 9, section 19).

- The row is written before a consequential action runs; if that write fails, the action must not run.
- After a crash/restart, anything still intended/executing becomes unknown.
- `unknown` is reconciled from a fresh read, never blindly retried: while an identical action (same task,
  capability, payload hash) is unknown, a new intent for it is refused — so a lost ack after Send can't
  become a second send.
"""

import json
from dataclasses import dataclass
from typing import Any, Literal

from ferb.core.errors import FerbError
from ferb.core.interfaces import Clock, IdSource
from ferb.core.types import ActionIntent
from ferb.store.db import Database

State = Literal["intended", "executing", "done", "failed", "unknown"]
TRANSITIONS: dict[str, set[str]] = {
    "intended": {"executing", "failed", "unknown"},
    "executing": {"done", "failed", "unknown"},
    "unknown": {"done", "failed"},             # only via reconcile()
    "done": set(), "failed": set(),
}


class DuplicateUnresolved(FerbError):
    """An identical action is still unknown; reconcile it first."""


class BadTransition(FerbError):
    pass


@dataclass(frozen=True)
class LedgerRow:
    intent: ActionIntent
    reconciled: bool
    evidence: list[str]


class Ledger:
    def __init__(self, db: Database, clock: Clock, ids: IdSource) -> None:
        self._db, self._clock, self._ids = db, clock, ids

    def intend(self, task_id: str, capability: str, target: dict[str, Any], payload_hash: str) -> ActionIntent:
        c = self._db.conn
        c.execute("BEGIN IMMEDIATE")
        try:
            if c.execute("SELECT 1 FROM action_ledger WHERE task_id = ? AND capability = ? AND payload_hash = ? "
                         "AND state = 'unknown'", (task_id, capability, payload_hash)).fetchone():
                raise DuplicateUnresolved(f"an identical {capability} is unknown; reconcile before retrying")
            now = self._clock.now().isoformat()
            intent = ActionIntent(self._ids.new_id(), task_id, capability, target, payload_hash, "intended")
            c.execute("INSERT INTO action_ledger (id, task_id, capability, target_json, payload_hash, state, "
                      "created_at, updated_at) VALUES (?, ?, ?, ?, ?, 'intended', ?, ?)",
                      (intent.id, task_id, capability, json.dumps(target, sort_keys=True), payload_hash, now, now))
            c.execute("COMMIT")
        except BaseException:
            c.execute("ROLLBACK")
            raise
        return intent

    def mark(self, intent_id: str, state: State, evidence: list[str] | None = None) -> None:
        if state in ("done", "failed") and self.get(intent_id).intent.state == "unknown":
            raise BadTransition("an unknown action is settled only by reconcile()")
        self._move(intent_id, state, evidence or [], reconciled=False)

    def reconcile(self, intent_id: str, observed: Literal["done", "failed"], evidence: list[str]) -> None:
        """Settle an unknown action from a fresh read (e.g. 'the message is in Sent'). Evidence is required."""
        if not evidence:
            raise ValueError("reconciliation needs evidence (span ids / snapshot refs)")
        if self.get(intent_id).intent.state != "unknown":
            raise BadTransition("only unknown actions are reconciled")
        self._move(intent_id, observed, evidence, reconciled=True)

    def mark_unknown_after_restart(self) -> list[str]:
        rows = self._db.conn.execute(
            "SELECT id FROM action_ledger WHERE state IN ('intended', 'executing')").fetchall()
        for r in rows:
            self._move(r["id"], "unknown", ["daemon restarted mid-action"], reconciled=False)
        return [r["id"] for r in rows]

    def unknown(self) -> list[LedgerRow]:
        return [self._row(r) for r in self._db.conn.execute(
            "SELECT * FROM action_ledger WHERE state = 'unknown' ORDER BY created_at").fetchall()]

    def get(self, intent_id: str) -> LedgerRow:
        r = self._db.conn.execute("SELECT * FROM action_ledger WHERE id = ?", (intent_id,)).fetchone()
        if r is None:
            raise KeyError(intent_id)
        return self._row(r)

    def _move(self, intent_id: str, state: str, evidence: list[str], reconciled: bool) -> None:
        current = self.get(intent_id)
        if state not in TRANSITIONS[current.intent.state]:
            raise BadTransition(f"{current.intent.state} -> {state} not allowed")
        self._db.conn.execute(
            "UPDATE action_ledger SET state = ?, updated_at = ?, reconciled = ?, evidence_json = ? WHERE id = ?",
            (state, self._clock.now().isoformat(), int(reconciled or current.reconciled),
             json.dumps(current.evidence + evidence), intent_id))

    @staticmethod
    def _row(r: Any) -> LedgerRow:
        return LedgerRow(ActionIntent(r["id"], r["task_id"], r["capability"], json.loads(r["target_json"]),
                                      r["payload_hash"], r["state"]), bool(r["reconciled"]),
                         json.loads(r["evidence_json"]))
