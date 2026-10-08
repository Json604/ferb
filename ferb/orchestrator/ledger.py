"""Owns action intentions and observed terminal states. Must not infer intent, choose a target or claim semantic success from transport facts.

Action ledger: intended -> executing -> done | failed | unknown.

- The row is written before a consequential action runs; if that write fails, the action must not run.
- After a crash/restart, anything still intended/executing becomes unknown.
- `unknown` is reconciled from a fresh read or the owner's decision, never blindly retried: while an identical
  action (same task tree, capability, payload hash) is unknown or still in flight, a new intent for it is refused,
  so a lost ack after Send can't become a second send (P19). Rows keep the worker task id they were written under;
  the tree is the part before the first "."."""

import json
from dataclasses import dataclass
from typing import Any, Literal

from ferb.core.errors import FerbError
from ferb.core.interfaces import Clock, IdSource
from ferb.core.types import ActionIntent
from ferb.store.db import Database
from ferb.store.spans import tree_where

State = Literal["intended", "executing", "done", "failed", "unknown"]
TRANSITIONS: dict[str, set[str]] = {
    "intended": {"executing", "failed", "unknown"},
    "executing": {"done", "failed", "unknown"},
    "unknown": {"done", "failed"},             # only via reconcile()
    "done": set(), "failed": set(),
}


class DuplicateUnresolved(FerbError):
    """An identical action in the same task tree is unknown (reconcile it first) or still in flight (`state`)."""

    def __init__(self, message: str, intent_id: str, state: str) -> None:
        super().__init__(message)
        self.intent_id, self.state = intent_id, state


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

    def intend(self, task_id: str, capability: str, target: dict[str, Any], payload_hash: str, *, reconcile_first: bool = True) -> ActionIntent:
        c = self._db.conn
        c.execute("BEGIN IMMEDIATE")
        try:
            if reconcile_first:
                where, params = tree_where(task_id.split(".", 1)[0])
                same = c.execute(
                    f"SELECT id, state FROM action_ledger WHERE {where} AND capability = ? AND payload_hash = ? AND state "
                    "IN ('unknown', 'intended', 'executing') ORDER BY state <> 'unknown', created_at, rowid",
                    (*params, capability, payload_hash)).fetchone()
                if same is not None:
                    raise DuplicateUnresolved(f"an identical {capability} is {same['state']}; reconcile before retrying",
                                              same["id"], same["state"])
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

    def unresolved(self, tree_id: str) -> list[dict[str, Any]]:
        """The task tree's actions that may or may not have happened (state `unknown` or `executing`), oldest first,
        for recovery. Rows keep the worker task id they were written under; the tree is selected by range, as spans
        are. The ledger is a recovery record, not proof of success: a `done` row says the tool returned, nothing more."""
        where, params = tree_where(tree_id)
        rows = self._db.conn.execute(
            f"SELECT * FROM action_ledger WHERE state IN ('unknown', 'executing') AND {where} ORDER BY created_at, rowid",
            params).fetchall()
        return [dict(id=r["id"], task_id=r["task_id"], capability=r["capability"], target=json.loads(r["target_json"]),
                     payload_hash=r["payload_hash"], state=r["state"], evidence=json.loads(r["evidence_json"]))
                for r in rows]

    def for_task(self, task_id: str) -> list[LedgerRow]:
        return [self._row(r) for r in self._db.conn.execute(
            "SELECT * FROM action_ledger WHERE task_id = ? ORDER BY created_at", (task_id,)).fetchall()]

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
