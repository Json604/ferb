import subprocess
from pathlib import Path
import pytest
from ferb.core.errors import Refused
from ferb.identity.helper import request


def test_secret_helper_never_puts_value_in_argv(tmp_path: Path, monkeypatch):
    (tmp_path/'bin').mkdir(); (tmp_path/'bin/ferb-secrets').write_text('binary')
    (tmp_path/'bin/ferb-secrets.requirement').write_text('identifier "com.ferb.secrets" and certificate leaf = H"abc"')
    calls = []
    def run(argv, **kwargs):
        calls.append((argv, kwargs))
        return subprocess.CompletedProcess(argv, 0, b'{"ok":true}', b'')
    monkeypatch.setattr(subprocess, 'run', run)
    assert request({'operation': 'store', 'value': 'private canary'}, tmp_path) == {'ok': True}
    assert calls[0][0][3].startswith('-R=identifier '), calls[0][0]
    assert all('private canary' not in ' '.join(argv) for argv, _ in calls)
    assert b'private canary' in calls[-1][1]['input']


def test_missing_signed_helper_fails_without_unsigned_fallback(tmp_path):
    with pytest.raises(Refused, match='signed secret helper'):
        request({'operation':'lookup'}, tmp_path)


def test_helper_value_has_no_cli_transport_newline_to_strip(monkeypatch):
    import asyncio
    from ferb.core.types import SecretRef, SecretSpec
    from ferb.identity.secrets import KeychainSecretStore
    monkeypatch.setattr('ferb.identity.helper.request', lambda body: {'value': 'password\n'})
    ref = SecretRef('saved')
    store = KeychainSecretStore(lambda: {'saved': SecretSpec(ref, ['https://accounts.example'], '')})
    assert asyncio.run(store._reveal(ref)) == 'password\n'


def test_helper_with_mismatched_signature_never_executes(tmp_path, monkeypatch):
    (tmp_path/'bin').mkdir()
    (tmp_path/'bin/ferb-secrets').write_text('binary')
    (tmp_path/'bin/ferb-secrets.requirement').write_text('identifier "com.ferb.secrets"')
    calls = []
    def run(argv, **kwargs):
        calls.append(argv)
        return subprocess.CompletedProcess(argv, 1, b'', b'')
    monkeypatch.setattr(subprocess, 'run', run)
    with pytest.raises(Refused, match='signature does not match'):
        request({'operation': 'store', 'value': 'private canary'}, tmp_path)
    assert len(calls) == 1 and calls[0][0] == '/usr/bin/codesign'


def _helper_home(tmp_path: Path) -> Path:
    (tmp_path/'bin').mkdir()
    (tmp_path/'bin/ferb-secrets').write_text('binary')
    (tmp_path/'bin/ferb-secrets.requirement').write_text('identifier "com.ferb.secrets"')
    return tmp_path


# decision: HAND-410
@pytest.mark.parametrize('replies, said', [
    ([(1, b'{"error":"keychain","status":-25308,"message":"User interaction is not allowed."}')],
     'the Keychain refused (status -25308: User interaction is not allowed.)'),
    ([(1, b'{"error":"secret operation failed"}'), (0, b'{"present":true}')], "has this item but didn't hand it"),
    ([(1, b'{"error":"secret operation failed"}'), (0, b'{"present":false}')], "didn't show this item"),
])
def test_a_helper_failure_says_why_without_the_value_or_account(tmp_path, monkeypatch, replies, said):
    """Live 2026-10-31/11-01: ten browser.secret fills failed with only "the secret operation failed". The reason comes
    through (the Keychain's status, or the helper's own present check) to the error and the journal hook."""
    import ferb.identity.helper as helper
    home, calls, told = _helper_home(tmp_path), [], []
    queue = list(replies)

    def run(argv, **kwargs):
        calls.append(kwargs.get('input', b''))
        if argv[0] == '/usr/bin/codesign':
            return subprocess.CompletedProcess(argv, 0, b'', b'')
        code, out = queue.pop(0)
        return subprocess.CompletedProcess(argv, code, out, b'')
    monkeypatch.setattr(subprocess, 'run', run)
    monkeypatch.setattr(helper, 'on_failure', told.append)
    account = 'https://login.example · password'
    with pytest.raises(Refused) as caught:
        request({'operation': 'lookup', 'service': 'ferb.owner-secrets', 'account': account}, home)
    assert said in str(caught.value) and account not in str(caught.value)
    assert told and told[0]['operation'] == 'lookup' and account not in str(told)
    assert not queue and all(b'"value"' not in (c or b'') for c in calls)   # the check never stores or reads one
