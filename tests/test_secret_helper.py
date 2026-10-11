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
