from pathlib import Path
import pytest
from ferb.core.errors import Refused
from ferb.identity.owner_secrets import OwnerSecrets
from tests.support.mac_secret import secret_driver


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


# ---------- no secret bound to a whole browser (owner 2026-10-24: "at places only ive approved") ----------

BROWSER_BUNDLES = ['com.google.Chrome', 'com.brave.Browser', 'company.thebrowser.Browser', 'com.microsoft.edgemac',
                   'com.apple.Safari', 'org.mozilla.firefox', 'com.operasoftware.Opera', 'com.vivaldi.Vivaldi',
                   'org.chromium.Chromium', 'COM.GOOGLE.CHROME']


@pytest.mark.parametrize('bundle', BROWSER_BUNDLES)
def test_mac_secret_refuses_a_web_browser_before_any_hidden_question(bundle: str) -> None:
    """Live 2026-10-24, visa login: mac.secret asked for the password with target app:com.google.Chrome, so a saved
    handle would have been filled into any page Chrome shows. A web page's password goes through the browser
    connection, bound to the page's HTTPS origin; mac.secret never binds one to the whole browser."""
    import asyncio
    from types import SimpleNamespace
    from ferb.act.mac.tools import MacSecret
    asked: list[dict] = []
    cap = MacSecret(secret_driver(bundle, 'Google Chrome', asked))
    args = cap.Input(app='Google Chrome', element='e1', remember='US Visa Scheduling password')
    refusal = cap.precheck(args)                       # the gateway refuses here, before the ledger and the span
    assert refusal is not None and 'browser connection' in refusal and "all of Google Chrome" in refusal
    result = asyncio.run(cap.run(args, SimpleNamespace(task_id='t1')))
    assert result.is_error and 'browser connection' in result.content['error']
    with pytest.raises(Refused, match='browser connection'):        # the resolved app, even past precheck
        asyncio.run(cap._run(args, SimpleNamespace(task_id='t1')))
    assert asked == []


def test_mac_secret_still_fills_a_native_app() -> None:
    import asyncio
    from types import SimpleNamespace
    from ferb.act.mac.tools import MacSecret
    asked: list[dict] = []
    cap = MacSecret(secret_driver('com.example.Bank', 'Bank', asked))
    args = cap.Input(app='Bank', element='e1')
    assert cap.precheck(args) is None
    assert not asyncio.run(cap.run(args, SimpleNamespace(task_id='t1'))).is_error
    assert [a['target'] for a in asked] == ['app:com.example.Bank']


@pytest.mark.parametrize('bundle', ['com.google.Chrome', 'com.apple.Safari', 'org.mozilla.firefox'])
def test_a_handle_bound_to_a_whole_browser_is_never_revealed_or_saved(tmp_path: Path, bundle: str) -> None:
    """Defense in depth: a handle saved before this rule with target app:<browser> is refused before the Keychain is
    read, and no new one is saved that way."""
    import json
    looked: list[str] = []

    def keychain(handle, value=None):
        looked.append(handle)
        return 'private-value'
    store = OwnerSecrets(tmp_path, keychain)
    store.path.write_text(json.dumps({'visa': 'app:' + bundle, 'notes': 'app:com.apple.Notes'}))
    with pytest.raises(Refused, match='browser'):
        store.reveal('visa', 'app:' + bundle)
    assert looked == []
    with pytest.raises(Refused, match='browser'):
        store.remember('other', 'app:' + bundle, 'private-value')
    assert looked == [] and 'other' not in store.handles()
    assert store.reveal('notes', 'app:com.apple.Notes') == 'private-value'


def test_every_browser_ferb_pairs_with_counts_as_a_web_browser() -> None:
    from ferb.act.browser.profiles import BROWSERS, is_web_browser
    assert all(is_web_browser(b.bundle_id) for b in BROWSERS.values())
    assert is_web_browser('com.apple.Safari') and is_web_browser('org.mozilla.firefox')
    assert not is_web_browser('com.apple.Notes') and not is_web_browser('com.apple.mail')


@pytest.mark.parametrize('bundle', ['com.google.Chrome.app.kjgfgldnnfoeklkmfkjfagphfepbbdan',   # a Chrome web app
                                    'com.apple.Safari.WebApp.5D8C1B2A-0F3E-4C1A',              # a Safari web app
                                    'com.brave.Browser.app.mpnpojknpmmopombnjdcgaaiekajbnjb',
                                    'com.google.chrome.for.testing', 'ru.yandex.desktop.yandex-browser',
                                    'one.ablaze.floorp', 'net.mullvad.mullvadbrowser', 'org.chromium.Thorium',
                                    'com.operasoftware.OperaCryptoBrowser', 'company.thebrowser.dia'])
def test_browser_channels_web_app_shims_and_browser_vendors_count_as_browsers(bundle: str) -> None:
    from ferb.act.browser.profiles import is_web_browser
    assert is_web_browser(bundle)


@pytest.mark.parametrize('bundle', ['com.apple.mail', 'com.apple.Notes', 'org.mozilla.thunderbird',
                                    'com.google.ChromeRemoteDesktopHost', 'com.google.drivefs'])
def test_other_apps_of_a_browser_maker_are_not_browsers(bundle: str) -> None:
    from ferb.act.browser.profiles import is_web_browser
    assert not is_web_browser(bundle)


def test_mac_secret_refuses_an_app_launch_services_lists_as_opening_https_links() -> None:
    """A browser missing from FERB's list is still refused when macOS lists it among the apps that open https links;
    refusing a non-browser that way only blocks a native secret fill there (the safe side)."""
    import asyncio
    from types import SimpleNamespace
    from ferb.act.mac.tools import MacSecret
    asked: list[dict] = []
    cap = MacSecret(secret_driver('com.example.NewBrowser', 'New Browser', asked, web=['com.example.newbrowser']))
    args = cap.Input(app='New Browser', element='e1')
    assert 'all of New Browser' in (cap.precheck(args) or '')
    with pytest.raises(Refused, match='browser connection'):
        asyncio.run(cap._run(args, SimpleNamespace(task_id='t1')))
    assert asked == []
    native = MacSecret(secret_driver('com.example.Bank', 'Bank', asked, web=['com.example.newbrowser']))
    assert native.precheck(native.Input(app='Bank', element='e1')) is None


def test_launch_services_lists_safari_as_opening_https_links_and_not_notes() -> None:
    pytest.importorskip('AppKit')
    from ferb.act.mac import apps
    assert apps.opens_web_links('com.apple.Safari')
    assert not apps.opens_web_links('com.apple.Notes')


def test_the_https_handler_list_is_enumerated_once_per_ttl_and_an_error_is_not_kept(monkeypatch) -> None:
    from ferb.act.mac import apps
    now, calls = [100.0], []

    def enumerate_handlers():
        calls.append(now[0])
        return frozenset({'com.example.newbrowser'})
    monkeypatch.setattr(apps, '_handlers', None)
    monkeypatch.setattr(apps, '_clock', lambda: now[0])
    monkeypatch.setattr(apps, '_enumerate_handlers', enumerate_handlers)
    assert not apps.known_opens_web_links('com.example.NewBrowser')      # nothing enumerated yet: never enumerates
    assert calls == []
    assert apps.opens_web_links('com.example.NewBrowser') and not apps.opens_web_links('com.apple.Notes')
    assert apps.known_opens_web_links('com.example.NewBrowser')
    assert calls == [100.0]                                              # two calls within the TTL: one enumeration
    now[0] += apps.HANDLERS_TTL_S + 1
    assert not apps.known_opens_web_links('com.example.NewBrowser')      # stale: the sync check uses the list only
    assert apps.opens_web_links('com.example.NewBrowser') and len(calls) == 2

    def broken():
        raise OSError('Launch Services unavailable')
    monkeypatch.setattr(apps, '_handlers', None)
    monkeypatch.setattr(apps, '_enumerate_handlers', broken)
    assert not apps.opens_web_links('com.example.NewBrowser')           # any error: FERB's list alone applies
    assert apps._handlers is None                                       # and nothing is kept


def test_mac_secret_asks_launch_services_off_the_event_loop() -> None:
    import asyncio
    import threading
    from types import SimpleNamespace
    from ferb.act.mac.tools import MacSecret
    asked: list[dict] = []
    driver = secret_driver('com.example.NewBrowser', 'New Browser', asked)
    threads: list[str] = []

    def opens(bundle: str) -> bool:
        threads.append(threading.current_thread().name)
        return bundle == 'com.example.NewBrowser'
    driver.apps.opens_web_links = opens
    cap = MacSecret(driver)
    args = cap.Input(app='New Browser', element='e1')
    assert cap.precheck(args) is None                    # no fresh list yet: precheck never enumerates
    loop_thread: list[str] = []

    async def run():
        loop_thread.append(threading.current_thread().name)
        return await cap.run(args, SimpleNamespace(task_id='t1'))
    result = asyncio.run(run())
    assert result.is_error and 'browser connection' in result.content['error'] and asked == []
    assert threads and loop_thread[0] not in threads
