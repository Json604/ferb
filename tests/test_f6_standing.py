"""D30 standing scope never covers another task, another body, payment or a changed destination."""
from ferb.core.types import ActionSpec, Target
from ferb.orchestrator.plans import SendScope
from ferb.policy.standing import StandingPermissions
from ferb.store.db import Database


def test_exact_send_scope_and_revocation(tmp_path):
    db = Database(tmp_path / 'db'); db.migrate()
    permissions = StandingPermissions(db)
    scope = SendScope(capability='mail.send', identity='owner', account='owner@example.test',
                      recipients=['self@example.test'], content='Build passed')
    permissions.grant('tree', [scope], 'owner-yes')
    spec = ActionSpec('send', Target(account=scope.account, recipients=tuple(scope.recipients)), 'Build passed')
    assert permissions.covers('tree', 'mail.send', 'owner', spec)
    assert not permissions.covers('another-tree', 'mail.send', 'owner', spec)
    assert not permissions.covers('tree', 'mail.send', 'ferb', spec)
    assert not permissions.covers('tree', 'mail.send', 'owner', ActionSpec('pay', spec.target, spec.content))
    assert not permissions.covers('tree', 'mail.send', 'owner', ActionSpec('send', spec.target, 'Other message'))
    assert not permissions.covers('tree', 'mail.send', 'owner', ActionSpec('send', Target(recipients=('other',)), spec.content))
    permissions.revoke('tree')
    assert not permissions.covers('tree', 'mail.send', 'owner', spec)
    scope.content = None
    permissions.grant('conversation', [scope], 'explicit-owner-any-body-yes')
    assert permissions.covers('conversation', 'mail.send', 'owner', ActionSpec('reply', spec.target, 'A later reply'))
    assert not permissions.covers('conversation', 'other.send', 'owner', spec)
    db.close()
