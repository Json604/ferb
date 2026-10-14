"""D30 standing permission: tree-scoped, exact sends, stated in the plan and correctable by the owner. No model verdict
is authority for these grants; code matches a call against a scope's concrete bounds only.

A scope covers a send-class call: one whose own kind is a send, or a generic input (a click, typing, an app's control)
the worker declared to send (W2, ferb/policy/rules.py `declare`), never one with any declared or described kind that
isn't a send (a payment, an account-security change, a permanent delete). Its tool, identity, site, account and
recipients must equal the scope's (sites as the browser names them, `confirm.site_key`; accounts and recipients
exactly but for ASCII case and a leading "@", `confirm.account_key`; a public post has no recipients); its content
must equal the scope's text, unless the scope's content is None (any body: a campaign whose posts are composed
later). A click carries no text, so a scope naming the exact text never covers one: FERB asks."""
import json
from typing import Any
from ferb.core.types import Target
from ferb.orchestrator.plans import SendScope
from ferb.policy import provenance
from ferb.policy.confirm import SEND_CLASS, account_key, site_key
from ferb.policy.rules import GENERIC_INPUT, declared_kinds
from ferb.store.db import Database

SCHEDULE_GRANT = "schedule:"     # a schedule's runs hold its recorded scopes under `schedule:<schedule id>`


def owner_named(scope: SendScope, owner_text: str) -> bool:
    """Whether every destination of a scope (its site, account and recipients) appears in the owner's own words, by
    the same provenance rules the code floor uses (S3): only such a scope may be recorded for unattended runs (D30)."""
    values = provenance.arg_sources(Target(account=scope.account, site=scope.site, recipients=tuple(scope.recipients)),
                                    owner_text, [])
    return bool(values) and all("owner_words" in found for found in values.values())


class StandingPermissions:
    def __init__(self, db: Database) -> None:
        self.db = db

    def grant(self, task_id: str, scopes: list[SendScope], approval_id: str) -> None:
        for index, scope in enumerate(scopes):
            self.db.conn.execute('INSERT OR REPLACE INTO standing_permissions VALUES(?,?,?,?,1)',
                (task_id + ':' + str(index), task_id, scope.model_dump_json(), approval_id))

    @staticmethod
    def sends(spec: Any) -> bool:
        """Whether a call is a send a scope may cover: its own kind is a send (and nothing it declares is another
        kind), or it is a generic input declared to send. Never with an amount or a contact the agent noted."""
        declared = declared_kinds(spec)
        if spec.target.amount is not None or spec.target.contact is not None or declared - SEND_CLASS:
            return False
        return spec.kind in SEND_CLASS or (spec.kind in GENERIC_INPUT and bool(declared & SEND_CLASS))

    def covers(self, task_id: str, capability: str, identity: str, spec: Any) -> bool:
        if not self.sends(spec):
            return False
        recipients = sorted(account_key(r) for r in spec.target.recipients)
        for row in self.db.conn.execute('SELECT scope FROM standing_permissions WHERE task_id=? AND active=1', (task_id,)):
            scope = json.loads(row[0])
            if (scope['capability'] == capability and scope['identity'] == identity and
                account_key(scope['account']) == account_key(spec.target.account) and
                site_key(scope['site']) == site_key(spec.target.site) and
                sorted(account_key(r) for r in scope['recipients']) == recipients and
                (scope['content'] is None or (bool(spec.content) and scope['content'] == spec.content)) and
                bool(scope['recipients'] or scope['site'])):
                return True
        return False

    def scopes(self, task_id: str) -> list[SendScope]:
        """The task's scopes in force, in their stated order."""
        return [SendScope.model_validate_json(row[0]) for row in self.db.conn.execute(
            'SELECT scope FROM standing_permissions WHERE task_id=? AND active=1 ORDER BY rowid', (task_id,))]

    def approvals(self, task_id: str) -> list[str]:
        """The statements (approval ids) the task's scopes in force were granted under."""
        return [row[0] for row in self.db.conn.execute(
            'SELECT DISTINCT approval_id FROM standing_permissions WHERE task_id=? AND active=1 ORDER BY approval_id',
            (task_id,))]

    def revoke(self, task_id: str) -> None:
        self.db.conn.execute('UPDATE standing_permissions SET active=0 WHERE task_id=?', (task_id,))

    def revoke_approval(self, approval_id: str) -> list[str]:
        """Withdraw every scope granted under one statement (a cancelled schedule's `schedule:<id>`), from every task
        holding it, running or suspended; returns those tasks."""
        tasks = [row[0] for row in self.db.conn.execute(
            'SELECT DISTINCT task_id FROM standing_permissions WHERE approval_id=? AND active=1 ORDER BY task_id',
            (approval_id,))]
        self.db.conn.execute('UPDATE standing_permissions SET active=0 WHERE approval_id=?', (approval_id,))
        return tasks
