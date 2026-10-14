"""D30 owner-approved, tree-scoped exact sends. No model verdict is authority for these grants."""
import json
from typing import Any
from ferb.core.types import Target
from ferb.orchestrator.plans import SendScope
from ferb.policy import provenance
from ferb.policy.confirm import SEND_CLASS
from ferb.store.db import Database


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

    def covers(self, task_id: str, capability: str, identity: str, spec: Any) -> bool:
        if spec.kind not in SEND_CLASS or spec.target.amount is not None or not spec.content:
            return False
        if spec.declared and spec.declared not in SEND_CLASS:
            return False
        for row in self.db.conn.execute('SELECT scope FROM standing_permissions WHERE task_id=? AND active=1', (task_id,)):
            scope = json.loads(row[0])
            if (scope['capability'] == capability and scope['identity'] == identity and
                scope['account'] == spec.target.account and scope['site'] == spec.target.site and
                sorted(scope['recipients']) == sorted(spec.target.recipients) and
                (scope['content'] is None or scope['content'] == spec.content) and bool(scope['recipients'] or scope['site']) and
                spec.target.contact is None):
                return True
        return False

    def scopes(self, task_id: str) -> list[SendScope]:
        """The task's scopes in force, in their stated order."""
        return [SendScope.model_validate_json(row[0]) for row in self.db.conn.execute(
            'SELECT scope FROM standing_permissions WHERE task_id=? AND active=1 ORDER BY rowid', (task_id,))]

    def revoke(self, task_id: str) -> None:
        self.db.conn.execute('UPDATE standing_permissions SET active=0 WHERE task_id=?', (task_id,))
