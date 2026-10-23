"""Durable plan state. Completed work is never silently put back in the pending queue."""
import json
from collections.abc import Callable
from typing import Any

from ferb.orchestrator.plans import Plan
from ferb.store.db import Database


# Where a replaced plan's finished results stay (L11): a step-row prefix no planner step id carries (ids are slugs).
EARLIER = 'earlier plan: '


class PlanStore:
    def __init__(self, db: Database, redact: Callable[[Any], Any]) -> None:
        self.db, self.redact = db, redact

    def _json(self, value: Any) -> str:
        return json.dumps(self.redact(value), ensure_ascii=False)

    def create(self, task_id: str, plan: Plan, request: dict[str, Any], conversation: str | None) -> None:
        with self.db.conn:
            self.db.conn.execute('INSERT INTO task_plans(task_id,plan,request,conversation) VALUES(?,?,?,?) '
                                 'ON CONFLICT(task_id) DO UPDATE SET request=excluded.request',
                                 (task_id, self._json(plan.model_dump()), self._json(request), conversation))
            for step in plan.steps:
                keys = [step.id] + ([f'{step.id}:{i}' for i in range(len(step.batches))] if step.kind == 'map' else [])
                for key in keys:
                    self.db.conn.execute('INSERT OR IGNORE INTO plan_steps(task_id,step_id) VALUES(?,?)', (task_id, key))

    def update_plan(self, task_id: str, plan: Plan) -> None:
        """The plan as an owner correction left it (its standing scope, D30); steps and their records stay."""
        self.db.conn.execute('UPDATE task_plans SET plan=? WHERE task_id=?', (self._json(plan.model_dump()), task_id))

    def load(self, task_id: str) -> dict[str, Any]:
        row = self.db.conn.execute('SELECT * FROM task_plans WHERE task_id=?', (task_id,)).fetchone()
        if row is None:
            raise KeyError(task_id)
        return {**dict(row), 'plan': Plan.model_validate_json(row['plan']), 'request': json.loads(row['request'])}

    def steps(self, task_id: str) -> dict[str, dict[str, Any]]:
        return {r['step_id']: {'status': r['status'], 'output': json.loads(r['output'])}
                for r in self.db.conn.execute('SELECT * FROM plan_steps WHERE task_id=?', (task_id,))}

    def set_status(self, task_id: str, status: str) -> None:
        self.db.conn.execute('UPDATE task_plans SET status=? WHERE task_id=?', (status, task_id))

    def begin(self, task_id: str, step_id: str, status: str = 'running') -> None:
        self.db.conn.execute('UPDATE plan_steps SET status=? WHERE task_id=? AND step_id=? AND status != ?',
                             (status, task_id, step_id, 'done'))

    def finish(self, task_id: str, step_id: str, output: dict[str, Any]) -> None:
        self.db.conn.execute('UPDATE plan_steps SET status=?,output=? WHERE task_id=? AND step_id=?',
                             (output.get('status', 'failed'), self._json(output), task_id, step_id))

    def reopen(self, task_id: str) -> None:
        """Explicit owner continuation retries unfinished steps; completed outputs and P19 ledger stay intact."""
        with self.db.conn:
            self.db.conn.execute("UPDATE plan_steps SET status='pending' WHERE task_id=? AND status != 'done'",
                                 (task_id,))
            # Ended trees revoke standing grants. A resumed send plan must display/approve its scope again.
            self.db.conn.execute("UPDATE task_plans SET status='running', approved=0 WHERE task_id=?", (task_id,))

    def replace(self, task_id: str, plan: Plan) -> None:
        """A follow-up with a new goal brought its own plan for this ended task (L11). A finished step the new plan
        keeps unchanged (same id and definition) stays done, so it is never redone; the old plan's other finished
        results are kept as data under EARLIER (see earlier()), and its unfinished steps are dropped, so a new step
        with a reused id starts fresh. The task's action ledger, spans and evidence stay. Like reopen, a send plan
        must be approved again."""
        kept = {step.id for step in plan.steps if step in self.load(task_id)['plan'].steps}
        with self.db.conn:
            for key, state in self.steps(task_id).items():
                if key.startswith(EARLIER) or (key.split(':', 1)[0] in kept and state['status'] == 'done'):
                    continue
                if state['status'] == 'done':
                    self.db.conn.execute('DELETE FROM plan_steps WHERE task_id=? AND step_id=?',
                                         (task_id, EARLIER + key))
                    self.db.conn.execute('UPDATE plan_steps SET step_id=? WHERE task_id=? AND step_id=?',
                                         (EARLIER + key, task_id, key))
                else:
                    self.db.conn.execute('DELETE FROM plan_steps WHERE task_id=? AND step_id=?', (task_id, key))
            self.db.conn.execute("UPDATE task_plans SET plan=?, status='running', approved=0 WHERE task_id=?",
                                 (self._json(plan.model_dump()), task_id))

    def earlier(self, task_id: str) -> dict[str, Any]:
        """Results of an earlier plan's finished steps that a replacing plan didn't keep, by their old step id."""
        return {key[len(EARLIER):]: state['output'] for key, state in self.steps(task_id).items()
                if key.startswith(EARLIER)}

    def pending(self) -> list[str]:
        return [r[0] for r in self.db.conn.execute("SELECT task_id FROM task_plans WHERE status IN ('running','suspended','steps_done')")]

    def event(self, name: str, payload: dict[str, Any]) -> None:
        self.db.conn.execute('INSERT INTO plan_events(event,payload) VALUES(?,?)', (name, self._json(payload)))

    def event_after(self, name: str, after: int, matches: list[Any] | None = None) -> dict[str, Any] | None:
        for row in self.db.conn.execute('SELECT payload FROM plan_events WHERE event=? AND seq>? ORDER BY seq',
                                       (name, after)):
            payload = json.loads(row[0])
            if all(m.field in payload and str(payload[m.field]) == m.value for m in matches or []):
                return payload
        return None

    def event_seq_after(self, name: str, after: int,
                        matches: list[Any] | None = None) -> tuple[int, dict[str, Any]] | None:
        """The first matching event after `after`, with its sequence number (a loop stores it as its cursor)."""
        for row in self.db.conn.execute('SELECT seq,payload FROM plan_events WHERE event=? AND seq>? ORDER BY seq',
                                       (name, after)):
            payload = json.loads(row[1])
            if all(m.field in payload and str(payload[m.field]) == m.value for m in matches or []):
                return int(row[0]), payload
        return None

    def event_cursor(self) -> int:
        return int(self.db.conn.execute('SELECT COALESCE(MAX(seq),0) FROM plan_events').fetchone()[0])
