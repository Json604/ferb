import json
import pytest
from tests.support.trace_scan import scan_trace
from tests.conftest import Answers, FakeCap, gateway_request, gateway_setup, gateway_tool_call
from tests.support.scenarios import SCENARIOS


@pytest.mark.parametrize('case', SCENARIOS, ids=lambda s: f'catalog-{s.catalog:02}')
def test_catalog_outside_recipient_or_payment_waits_for_owner(daemon, case):
    read = FakeCap('catalog.read', 'read', {'probe'}, reads=((case.source, 'https://mysite.example/security'),),
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


# decision: DMN-581, DMN-582
def test_an_older_ferbs_test_mode_is_gone_after_the_upgrade(home):
    """Test mode is removed (owner 2026-11-10). A config an older FERB left with test mode on (questions recorded, a
    send allowlist) starts with it simply gone: the key is removed once and journaled, nothing reads it, and an older
    script's test.mode call gets the same refusal as any method FERB doesn't have."""
    from ferb.cli.client import Client, RpcError
    from ferb.config import TEST_MODE_DECIDED, Config
    from tests.conftest import RunningDaemon
    Config(home).set('test_mode', 'state', {'enabled': True, 'allow_send': ['owner@controlled.test'],
                                            'questions': 'record'})
    d = RunningDaemon(home, watch_owner_input=False)
    client = Client(d.sock)
    try:
        assert d.daemon.config.section('test_mode') == {} and '[test_mode]' not in (home / 'config.toml').read_text()
        async def journaled():
            return [e.data for e in d.daemon.journal.of_kind('config.upgraded', 'upgrade', TEST_MODE_DECIDED)]
        upgraded = d.run(journaled())
        assert not hasattr(d.daemon.gateway, 'test_send_allowlist') and not hasattr(d.daemon.gateway, 'test_questions')
        assert upgraded == [{'upgrade': TEST_MODE_DECIDED, 'changes': [{'key': 'test_mode.state', 'to': None}]}]
        for params in ({'status': True}, {'off': True}, {}):
            with pytest.raises(RpcError, match='test.mode not allowed'):
                client.call('test.mode', params)
        assert client.call('ping') is not None
    finally:
        client.close()
        d.stop()
