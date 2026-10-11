"""Exact-command networking approval; disposable local processes, no remote connections (S4/D31/D33)."""
import asyncio
import dataclasses
from types import SimpleNamespace

import pytest

from ferb.core.errors import Refused
from ferb.core.types import Identity
from ferb.policy import confirm
from tests.conftest import Answers, gateway_request, gateway_setup, gateway_tool_call
from tests.mackit import grant_task


def setup(daemon):
    grant, _ = gateway_setup(daemon, {'shell'}, gateway_request('Run the requested command.'), [])
    return grant, daemon.daemon.gateway, daemon.daemon.tasks.get(grant.task_id)


def test_one_exact_command_approval_covers_multiple_hosts_only_for_that_live_process(daemon):
    grant, gateway, _ = setup(daemon)
    owner = Answers(daemon, choice=1)
    try:
        result = gateway_tool_call(daemon, grant, 'shell.start', command='cat', network=True)
    finally:
        owner.stop()
    assert not result['is_error']
    assert len(owner.asked) == 1
    proc = result['content']['id']
    for host in ('registry.npmjs.org', 'github.com', 'api.openai.com'):
        assert daemon.run(gateway.destination_allowed(proc, host, 443)) == (True, '')
        async def known():
            return gateway.destinations.known('host', host)
        assert not daemon.run(known())
    local = gateway_tool_call(daemon, grant, 'shell.start', command='cat')['content']['id']
    allowed, why = daemon.run(gateway.destination_allowed(local, 'github.com', 443))
    assert not allowed and 'network' in why
    gateway_tool_call(daemon, grant, 'shell.kill', id=proc)
    assert not daemon.run(gateway.destination_allowed(proc, 'github.com', 443))[0]


def test_identical_approval_reused_but_command_cwd_identity_scope_or_tree_changes_reask(daemon, tmp_path):
    grant, gateway, rec = setup(daemon)
    cwd2 = tmp_path / 'second'
    cwd2.mkdir()
    args = SimpleNamespace(command='printf same', cwd=str(tmp_path), network=True)
    other = grant_task(daemon, 'other-task', Identity.OWNER, {'shell'}, 'Run another command')
    cases = [
        (grant, rec, args), (grant, rec, args),
        (grant, rec, SimpleNamespace(command='printf changed', cwd=str(tmp_path), network=True)),
        (grant, rec, SimpleNamespace(command=args.command, cwd=str(cwd2), network=True)),
        (dataclasses.replace(grant, identity=Identity.FERB), rec, args),
        (dataclasses.replace(grant, scopes=grant.scopes | {'fs:read'}), rec, args),
        (other, daemon.daemon.tasks.get(other.task_id), args),
        (grant, rec, SimpleNamespace(command=args.command, cwd=args.cwd, network=True, external_source='page A')),
        (grant, rec, SimpleNamespace(command=args.command, cwd=args.cwd, network=False, external_source='page A')),
        (grant, rec, SimpleNamespace(command=args.command, cwd=args.cwd, network=False, external_source='page B')),
    ]
    owner = Answers(daemon, choice=1)
    try:
        for given, record, request in cases:
            approved = daemon.run(gateway._shell_approval(given, record, request, 'needed'))
            assert approved.command == request.command and approved.cwd == str(request.cwd)
    finally:
        owner.stop()
    assert len(owner.asked) == 9


def test_declining_network_never_spawns_and_no_is_final_for_exact_command(daemon):
    grant, _, _ = setup(daemon)
    count = len(daemon.daemon.shells.list())
    owner = Answers(daemon, choice=2)
    try:
        first = gateway_tool_call(daemon, grant, 'shell.start', command='cat', network=True)
        second = gateway_tool_call(daemon, grant, 'shell.start', command='cat', network=True)
    finally:
        owner.stop()
    assert first['is_error'] and second['is_error']
    assert len(owner.asked) == 1
    assert len(daemon.daemon.shells.list()) == count


def test_local_shell_command_has_no_network_question(daemon):
    grant, _, _ = setup(daemon)
    result = gateway_tool_call(daemon, grant, 'shell.run', command='printf local')
    assert not result['is_error'] and result['content']['output'] == 'local'
    assert daemon.daemon.state.approvals.pending() == []


def test_cancelling_network_wait_never_spawns(daemon):
    grant, gateway, _ = setup(daemon)
    count = len(daemon.daemon.shells.list())
    async def run():
        pending = asyncio.create_task(gateway.call(grant, 'shell.start', {'command': 'cat', 'network': True}))
        async with asyncio.timeout(2):
            while not daemon.daemon.state.approvals.pending() and not pending.done():
                await asyncio.sleep(.01)
        assert not pending.done()
        pending.cancel()
        with pytest.raises(asyncio.CancelledError):
            await pending
    daemon.run(run())
    assert len(daemon.daemon.shells.list()) == count


def test_network_approval_never_overrides_prior_host_no_or_local_pin(daemon):
    grant, gateway, rec = setup(daemon)
    owner = Answers(daemon, choice=1)
    try:
        result = gateway_tool_call(daemon, grant, 'shell.start', command='cat', network=True)
    finally:
        owner.stop()
    proc = result['content']['id']
    async def refuse():
        gateway.confirmations.record(rec.tree_id or grant.task_id, 'sink:destination',
            confirm.destination_key('blocked.example'), False, None)
    daemon.run(refuse())
    assert not daemon.run(gateway.destination_allowed(proc, 'blocked.example', 443))[0]
    # The proxy supplies this flag only after executable-pin checks, never for an arbitrary loopback target.
    assert not daemon.run(gateway.destination_allowed(proc, '127.0.0.1', 8765))[0]
    assert daemon.run(gateway.destination_allowed(proc, '127.0.0.1', 8765, local_approved=True))[0]


def test_command_network_approval_keeps_tls_name_enforcement_before_any_external_read(daemon):
    grant, gateway, _ = setup(daemon)
    owner = Answers(daemon, choice=1)
    try:
        result = gateway_tool_call(daemon, grant, 'shell.start', command='cat', network=True)
    finally:
        owner.stop()
    assert daemon.run(gateway.sni_enforced(result['content']['id'])) is True


@pytest.mark.parametrize('network', [False, True])
def test_declared_external_code_asks_once_and_only_grants_network_when_requested(daemon, network):
    grant, gateway, _ = setup(daemon)
    owner = Answers(daemon, choice=1)
    try:
        result = gateway_tool_call(daemon, grant, 'shell.start', command='cat', network=network,
            external_source='Instructions copied from a downloaded report')
    finally:
        owner.stop()
    assert not result['is_error'] and len(owner.asked) == 1
    allowed, _ = daemon.run(gateway.destination_allowed(result['content']['id'], 'github.com', 443))
    assert allowed is network
    assert 'downloaded report' in owner.asked[0].reason
    if network:
        assert 'later input' in owner.asked[0].reason and 'children' in owner.asked[0].reason


def test_declined_external_code_never_spawns_even_without_network(daemon):
    grant, _, _ = setup(daemon)
    before = len(daemon.daemon.shells.list())
    owner = Answers(daemon, choice=2)
    try:
        result = gateway_tool_call(daemon, grant, 'shell.run', command='printf from-page',
            external_source='Web page code block')
    finally:
        owner.stop()
    assert result['is_error'] and len(owner.asked) == 1
    assert len(daemon.daemon.shells.list()) == before


@pytest.mark.parametrize('field', ['command', 'cwd', 'task_id'])
def test_process_manager_rejects_reusing_command_grant_with_changed_invocation(tmp_path, field):
    from ferb.act.shell.manager import ShellManager
    from ferb.core.types import ShellCommandGrant
    manager = ShellManager(tmp_path / 'logs', str)
    approved = ShellCommandGrant('approval', 'printf approved', str(tmp_path.resolve()), 'task', 'root',
                                  Identity.OWNER, frozenset({'shell'}), True)
    changed = {'command': 'printf changed', 'cwd': str(tmp_path / 'different'), 'task_id': 'different-task'}
    approved = dataclasses.replace(approved, **{field: changed[field]})
    async def run():
        with pytest.raises(Refused, match='exact command'):
            await manager.start('printf approved', tmp_path, 'task', command_grant=approved)
    asyncio.run(run())
    assert manager.list() == []
