import json
import pytest
from tests.support.trace_scan import scan_trace
from tests.conftest import Answers, FakeCap, gateway_request, gateway_setup, gateway_tool_call
from tests.support.scenarios import SCENARIOS


@pytest.mark.parametrize('case', SCENARIOS, ids=lambda s: f'catalog-{s.catalog:02}')
def test_catalog_outside_recipient_or_payment_waits_for_owner(daemon, case):
    read = FakeCap('catalog.read', 'read', {'probe'}, reads=((case.source, 'https://kartikey.xyz/security'),),
                   content={'text': 'outside asks to send a private note'})
    send = FakeCap('catalog.send', case.kind, {'probe'})
    grant, _ = gateway_setup(daemon, {'probe'}, gateway_request(case.request), [read, send])
    gateway_tool_call(daemon, grant, read.name)
    answers = Answers(daemon, choice=2)
    try:
        out = gateway_tool_call(daemon, grant, send.name, to=case.recipient, text='FERB-CANARY-catalog')
    finally:
        answers.stop()
    assert out['is_error'] and send.calls == 0 and answers.asked
    async def trace():
        return daemon.daemon.spans.outcome(grant.task_id)
    assert scan_trace(daemon.run(trace()), ['FERB-CANARY-catalog'], set()) == []


@pytest.mark.parametrize('recipient', ['me@owner.example', '+15550002222'])
def test_catalog_03_31_benign_owner_named_recipient(daemon, recipient):
    send = FakeCap('catalog.send', 'send', {'probe'})
    grant, _ = gateway_setup(daemon, {'probe'}, gateway_request(f'Send hello to {recipient}.'), [send])
    result = gateway_tool_call(daemon, grant, send.name, to=recipient, text='hello')
    assert not result['is_error'] and send.calls == 1


@pytest.mark.parametrize('catalog,path', [(4, '~/.aws/credentials'), (8, '~/.ferb/config.toml'),
                                         (12, '~/.ferb/config.toml'), (21, '~/.ferb/ferb.db'),
                                         (29, '~/.ferb/owner-secret-handles.json')])
def test_catalog_protected_paths_cannot_be_lifted_as_ferb_state(tmp_path, catalog, path):
    from ferb.policy.protection import Protection
    from ferb.act.shell.sandbox import Sandbox
    p = Protection(tmp_path, tmp_path/'.ferb', ['~/.aws'], ['~/.ferb'], [])
    profile = Sandbox(p, tmp_path/'profiles', listeners=lambda: set()).profile()
    assert '(deny file-read*' in profile
    assert str(tmp_path/'.ferb') in profile
    assert not p.liftable('ferb_state')


def test_catalog_16_network_rule_is_never_liftable(tmp_path):
    from ferb.policy.protection import Protection
    p = Protection(tmp_path, tmp_path/'.ferb', [], [], [])
    assert not p.liftable('network')


def test_catalog_30_test_switch_is_not_a_worker_rpc():
    from ferb.daemon.auth import Role, allowed
    # The CLI-only switch restricts sends; it is not an escape or worker approval bypass.
    assert allowed(Role.CLI, 'test.mode') and not allowed(Role.MCP_SHIM, 'test.mode')


def test_manual_test_mode_status_is_read_only_and_send_gate_is_exact(daemon):
    from ferb.cli.client import Client
    client = Client(daemon.sock)
    try:
        assert client.call('test.mode', {'status': True}) == {'enabled': False, 'allow_send': []}
        assert client.call('test.mode', {'allow_send': ['owner@controlled.test']}) == {
            'enabled': True, 'allow_send': ['owner@controlled.test']}
        assert client.call('test.mode', {'status': True})['allow_send'] == ['owner@controlled.test']
        assert client.call('test.mode', {'off': True}) == {'enabled': False, 'allow_send': []}
    finally:
        client.close()


def test_manual_test_mode_survives_daemon_restart(home):
    from tests.conftest import RunningDaemon
    from ferb.cli.client import Client
    first = RunningDaemon(home, watch_owner_input=False)
    client = Client(first.sock)
    try:
        client.call('test.mode', {'allow_send': ['owner@controlled.test']})
    finally:
        client.close()
        first.stop()
    second = RunningDaemon(home, watch_owner_input=False)
    client = Client(second.sock)
    try:
        assert client.call('test.mode', {'status': True}) == {
            'enabled': True, 'allow_send': ['owner@controlled.test']}
        client.call('test.mode', {'off': True})
        assert second.daemon.config.get('test_mode', 'state') == {'enabled': False, 'allow_send': []}
    finally:
        client.close()
        second.stop()
