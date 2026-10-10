from pathlib import Path
import pytest
from ferb.core.errors import Refused
from ferb.identity.owner_secrets import OwnerSecrets


def test_keychain_handles_persist_only_exact_destination(tmp_path: Path) -> None:
    values = {}
    def keychain(handle, value=None):
        if value is not None:
            values[handle] = value
        return values.get(handle)
    store = OwnerSecrets(tmp_path, keychain)
    store.remember('work-login', 'https://accounts.example', 'private-value')
    assert 'private-value' not in store.path.read_text()
    assert store.reveal('work-login', 'https://accounts.example') == 'private-value'
    with pytest.raises(Refused):
        store.reveal('work-login', 'https://attacker.example')


def test_concurrent_prompts_keep_both_handle_bindings(tmp_path: Path) -> None:
    from concurrent.futures import ThreadPoolExecutor
    values = {}
    def keychain(handle, value=None):
        if value is not None:
            values[handle] = value
        return values.get(handle)
    def remember(i):
        OwnerSecrets(tmp_path, keychain).remember(str(i), 'https://accounts.example', f'secret-{i}')
    with ThreadPoolExecutor(max_workers=2) as pool:
        list(pool.map(remember, range(2)))
    assert OwnerSecrets(tmp_path, keychain).handles() == {'0': 'https://accounts.example', '1': 'https://accounts.example'}


@pytest.mark.parametrize('changed', [False, True])
def test_native_hidden_fill_binds_node_and_releases_secret(changed):
    import asyncio
    from types import SimpleNamespace
    from ferb.act.mac.tools import MacSecret
    from ferb.core.errors import StaleRef
    node, other = SimpleNamespace(ref='field'), SimpleNamespace(ref='other')
    app = SimpleNamespace(bundle_id='com.apple.Notes', pid=11)
    current, writes, released = [node], [], []
    async def ask(ctx, target, **options):
        assert target == 'app:com.apple.Notes'
        if changed:
            current[0] = other
        return 'hidden-canary'
    driver = SimpleNamespace(app=lambda _: app, node=lambda *args: current[0], ask_secret=ask,
        ax=SimpleNamespace(set_value=lambda ref, value: writes.append((ref, value)) or True),
        release_secret=released.append)
    cap = MacSecret(driver)
    async def fill():
        return await cap._run(cap.Input(app='Notes', element='e1'), SimpleNamespace())
    if changed:
        with pytest.raises(StaleRef):
            asyncio.run(fill())
        assert not writes
    else:
        result = asyncio.run(fill())
        assert writes == [('field', 'hidden-canary')] and 'hidden-canary' not in str(result)
    assert released == ['hidden-canary']
