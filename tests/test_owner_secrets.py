import json
from pathlib import Path
from typing import Any

import pytest
from ferb.core.errors import Refused
from ferb.core.types import Identity
from ferb.identity.owner_secrets import WHOLE_BROWSER, OwnerSecrets, login_place, same_site
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
    assert OwnerSecrets(tmp_path, keychain).records()['https://accounts.example']['entries'] == {
        '0': 'https://accounts.example · 0', '1': 'https://accounts.example · 1'}


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
    assert looked == [] and 'app:' + bundle not in store.records()
    assert store.reveal('notes', 'app:com.apple.Notes') == 'private-value'


def test_every_browser_ferb_pairs_with_counts_as_a_web_browser() -> None:
    from ferb.act.browser.profiles import BROWSERS
    from ferb.act.mac.browsers import known as is_web_browser
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
    from ferb.act.mac.browsers import known as is_web_browser
    assert is_web_browser(bundle)


@pytest.mark.parametrize('bundle', ['com.apple.mail', 'com.apple.Notes', 'org.mozilla.thunderbird',
                                    'com.google.ChromeRemoteDesktopHost', 'com.google.drivefs'])
def test_other_apps_of_a_browser_maker_are_not_browsers(bundle: str) -> None:
    from ferb.act.mac.browsers import known as is_web_browser
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
    from ferb.act.mac import apps, browsers
    now, calls = [100.0], []

    def enumerate_handlers():
        calls.append(now[0])
        return frozenset({'com.example.newbrowser'}), frozenset({'new browser'})
    monkeypatch.setattr(browsers, '_handlers', None)
    monkeypatch.setattr(browsers, '_clock', lambda: now[0])
    monkeypatch.setattr(browsers, '_enumerate', enumerate_handlers)
    assert not apps.known_opens_web_links('com.example.NewBrowser')      # nothing enumerated yet: never enumerates
    assert not browsers.is_browser('com.example.NewBrowser')
    assert calls == []
    assert apps.opens_web_links('com.example.NewBrowser') and not apps.opens_web_links('com.apple.Notes')
    assert apps.known_opens_web_links('com.example.NewBrowser') and browsers.is_browser('com.example.NewBrowser')
    assert calls == [100.0]                                              # two calls within the TTL: one enumeration
    now[0] += browsers.HANDLERS_TTL_S + 1
    assert not apps.known_opens_web_links('com.example.NewBrowser')      # stale: the sync check uses the list only
    assert browsers.is_browser('com.example.NewBrowser')                 # the one list keeps the last one it got
    assert apps.opens_web_links('com.example.NewBrowser') and len(calls) == 2

    def broken():
        raise OSError('Launch Services unavailable')
    monkeypatch.setattr(browsers, '_handlers', None)
    monkeypatch.setattr(browsers, '_enumerate', broken)
    assert not apps.opens_web_links('com.example.NewBrowser')           # any error: FERB's list alone applies
    assert browsers._handlers is None                                   # and nothing is kept


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


# ---------- one login record per place (L36 §5) ----------

VISA = 'https://www.usvisascheduling.com'
ATLAS = 'https://atlasauth.b2clogin.com'           # the visa site's single sign-on form (I14)


def fake_keychain() -> tuple[dict[str, str], Any]:
    values: dict[str, str] = {}

    def keychain(account: str, value: str | None = None) -> str | None:
        if value is not None:
            values[account] = value
        return values.get(account)
    return values, keychain


def test_a_login_place_is_an_https_origin_or_an_app() -> None:
    assert login_place(VISA + '/en-US/niv/login?x=1') == VISA
    assert login_place('app:com.apple.Notes') == 'app:com.apple.Notes'
    assert login_place('http://www.usvisascheduling.com') == '' and login_place('usvisascheduling.com') == ''
    assert login_place('https://x.test:99999/login') == '' and login_place('app:') == ''
    assert login_place('https://') == ''


def test_saving_a_name_again_for_a_place_replaces_it(tmp_path: Path) -> None:
    values, keychain = fake_keychain()
    store = OwnerSecrets(tmp_path, keychain)
    store.remember('password', VISA, 'first-value')
    store.remember('password', VISA, 'second-value')
    assert store.records()[VISA]['entries'] == {'password': f'{VISA} · password'}
    assert store.reveal('password', VISA) == 'second-value' and len(values) == 1


def test_the_listing_names_the_username_and_entries_never_a_value_or_account(tmp_path: Path) -> None:
    _, keychain = fake_keychain()
    store = OwnerSecrets(tmp_path, keychain)
    store.remember('password', VISA, 'first-value')
    store.remember('What is your favorite food?', VISA, 'second-value')
    store.set_username(VISA, 'Asha Rao', 'memory:item-1')
    listing = store.listing(VISA)
    assert listing == {'place': VISA, 'username': 'Asha Rao', 'entries': ['What is your favorite food?', 'password'],
                       'unusable': []}
    text = json.dumps(listing)
    assert 'first-value' not in text and 'second-value' not in text and ' · ' not in text


def test_an_old_handle_file_is_read_by_place_and_a_browser_bound_one_is_unusable(tmp_path: Path) -> None:
    """Review Focus: the live store's flat handle -> place map keeps working; the app-bound visa password, which
    `refuse_whole_browser` refuses, is listed as unusable ("ask again")."""
    values, keychain = fake_keychain()
    values.update({'US visa password (Chrome)': 'old-chrome', 'US visa password': 'old-web', 'notes': 'old-notes'})
    store = OwnerSecrets(tmp_path, keychain)
    old = {'US visa password (Chrome)': 'app:com.google.Chrome', 'US visa password': VISA,
           'notes': 'app:com.apple.Notes'}
    store.path.write_text(json.dumps(old))
    empty = {'username': None, 'username_source': None, 'sites': []}
    assert store.records() == {VISA: empty | {'entries': {'US visa password': 'US visa password'}},
                               'app:com.apple.Notes': empty | {'entries': {'notes': 'notes'}}}
    assert store.listing(VISA)['unusable'] == [{'name': 'US visa password (Chrome)', 'why': WHOLE_BROWSER}]
    assert 'ask the owner again' in WHOLE_BROWSER
    assert store.reveal('US visa password', VISA) == 'old-web'
    with pytest.raises(Refused, match='browser'):
        store.reveal('US visa password (Chrome)', 'app:com.google.Chrome')
    store.clear_usernames({'memory:none'})                           # a change that changes nothing writes nothing
    assert json.loads(store.path.read_text()) == old                 # a read never rewrites the file
    store.remember('password', VISA, 'new-web')
    saved = json.loads(store.path.read_text())
    assert saved['version'] == 2 and saved['unusable'][0]['place'] == 'app:com.google.Chrome'
    assert store.reveal('US visa password', VISA) == 'old-web'       # its Keychain item is unchanged
    assert store.listing('app:com.apple.Notes')['unusable'] == [{'name': 'US visa password (Chrome)',
                                                                 'why': WHOLE_BROWSER}]   # in every answer


UNREADABLE = ['{not json', '["a", "b"]', '{"version": 3, "records": {}}',
              '{"version": 2, "records": {"https://x.test": {"entries": ["password"]}}}',
              '{"version": 2, "records": {"https://x.test": {"entries": {}, "sites": [1]}}}',
              '{"version": 2, "records": {"https://x.test": {"entries": {}, "username": 5}}}']


@pytest.mark.parametrize('content', UNREADABLE)
def test_an_unreadable_handle_file_is_never_overwritten(tmp_path: Path, content: str) -> None:
    _, keychain = fake_keychain()
    store = OwnerSecrets(tmp_path, keychain)
    store.path.write_text(content)
    for attempt in (lambda: store.reveal('password', VISA), lambda: store.remember('password', VISA, 'x'),
                    lambda: store.listing(VISA), lambda: store.set_username(VISA, 'Asha Rao', 'memory:x'),
                    lambda: store.clear_usernames({'memory:x'})):
        with pytest.raises(Refused, match='owner-secret-handles.json'):
            attempt()
    assert store.path.read_text() == content


def test_secrets_list_is_an_owner_observation_that_normalizes_the_place(daemon: Any) -> None:
    from tests.mackit import gateway_call as call, grant_task as grant
    from tests.support.memory_items import run
    _, keychain = fake_keychain()
    run(daemon, lambda: OwnerSecrets(daemon.daemon.home, keychain).remember('password', VISA, 'v'))
    owner = grant(daemon, 'q', Identity.OWNER, {'task:read'}, None)
    r = call(daemon, owner, 'secrets.list', place=VISA + '/en-US/niv/login?x=1')
    assert not r['is_error'] and r['content']['place'] == VISA and r['content']['entries'] == ['password']
    assert call(daemon, owner, 'secrets.list', place='http://www.usvisascheduling.com')['is_error']
    ferb = grant(daemon, 'f', Identity.FERB, {'task:read'}, None)
    assert 'secrets.list' not in {t['name'] for t in daemon.daemon.gateway.tools_for(ferb)}
    OwnerSecrets(daemon.daemon.home, keychain).path.write_text('{not json')
    r = call(daemon, owner, 'secrets.list', place=VISA)
    assert r['is_error'] and 'owner-secret-handles.json' in json.dumps(r['content'])    # the tool says why


# ---------- a login record serves the sites of the task it was saved in (I14) ----------

def test_a_record_saved_during_a_task_is_listed_for_the_tasks_sites_but_fills_only_at_its_own_place(
        tmp_path: Path) -> None:
    """Live 2026-10-24: the visa site's sign-in form is on atlasauth.b2clogin.com while the owner names
    usvisascheduling.com. The record of the form's origin lists the task's sites, so a listing for the site finds it;
    its entries still fill only at the form's own origin (D12)."""
    _, keychain = fake_keychain()
    store = OwnerSecrets(tmp_path, keychain)
    store.remember('password', ATLAS, 'v', sites=['usvisascheduling.com', ATLAS + '/login', 'USVisaScheduling.com.'])
    assert store.records()[ATLAS]['sites'] == ['usvisascheduling.com']      # hosts; its own host is its place
    for site in (VISA, 'https://usvisascheduling.com'):                # the same site, a leading "www." aside
        assert store.listing(site) == {'place': site, 'username': None, 'entries': [], 'unusable': [],
                                       'linked': [{'place': ATLAS, 'username': None, 'entries': ['password']}]}
    for other in ('https://ais.usvisascheduling.com', 'https://evilusvisascheduling.com',
                  'https://usvisascheduling.com.evil.example', 'https://x.test'):
        assert 'linked' not in store.listing(other)                      # never a subdomain, sibling or look-alike
    assert 'linked' not in store.listing(ATLAS)
    with pytest.raises(Refused, match='exact destination'):
        store.reveal('password', VISA)
    store.remember('What is your favorite food?', ATLAS, 'w', sites=['ais.usvisascheduling.com'])
    assert store.records()[ATLAS]['sites'] == ['ais.usvisascheduling.com', 'usvisascheduling.com']


def test_a_listed_site_never_serves_another_tenant_of_shared_hosting(tmp_path: Path) -> None:
    """Fix round 1: with subdomain matching, a record listing b2clogin.com served evil.b2clogin.com, another tenant's
    page, which got the owner's username and entry names (and the question would name the account beside it)."""
    _, keychain = fake_keychain()
    store = OwnerSecrets(tmp_path, keychain)
    store.remember('password', ATLAS, 'v', sites=['b2clogin.com', 'github.io'])
    store.set_username(ATLAS, 'Asha Rao', 'memory:1')
    for tenant in ('https://evil.b2clogin.com', 'https://x.github.io'):
        assert store.listing(tenant) == {'place': tenant, 'username': None, 'entries': [], 'unusable': []}
        assert store.username(tenant) is None


def test_a_new_entry_never_reuses_another_entrys_keychain_item(tmp_path: Path) -> None:
    """A version-1 handle can be any text, "<place> · <name>" included: a new entry never takes its Keychain item,
    whether it is another place's entry or an unusable one."""
    values, keychain = fake_keychain()
    taken = f'{VISA} · password'
    values[taken] = 'old-value'
    store = OwnerSecrets(tmp_path, keychain)
    for old in ({taken: 'https://other.example'}, {taken: 'app:com.google.Chrome'}):
        store.path.write_text(json.dumps(old))
        with pytest.raises(Refused, match='Keychain item'):
            store.remember('password', VISA, 'new-value')
        assert values == {taken: 'old-value'} and json.loads(store.path.read_text()) == old


def test_a_username_given_for_the_site_shows_on_the_record_that_serves_it(tmp_path: Path) -> None:
    """The username the owner gave for usvisascheduling.com (that morning) is the sign-in form's account too: a record
    shows its own username, else the one its linked records agree on; never one of two that disagree."""
    _, keychain = fake_keychain()
    store = OwnerSecrets(tmp_path, keychain)
    store.set_username('https://usvisascheduling.com', 'Asha Rao', 'memory:1')
    store.remember('password', ATLAS, 'v', sites=['usvisascheduling.com'])
    assert store.username(ATLAS) == 'Asha Rao' and store.listing(ATLAS) == {
        'place': ATLAS, 'username': 'Asha Rao', 'username_site': 'usvisascheduling.com', 'entries': ['password'],
        'unusable': [], 'linked': [{'place': 'https://usvisascheduling.com', 'username': 'Asha Rao', 'entries': []}]}
    assert store.account(ATLAS) == ('Asha Rao', 'https://usvisascheduling.com')        # review I-2: whose account
    assert store.account('https://usvisascheduling.com') == ('Asha Rao', None)
    assert store.username('https://usvisascheduling.com') == 'Asha Rao'
    assert store.records()[ATLAS]['username'] is None                       # read through the link, never copied
    store.set_username('https://ais.usvisascheduling.com', 'A. Rao', 'memory:2')
    store.remember('password', ATLAS, 'v', sites=['ais.usvisascheduling.com'])
    assert store.username(ATLAS) is None                                    # two accounts: the line names neither
    store.set_username(ATLAS, 'asha.rao', 'memory:3')
    assert store.username(ATLAS) == 'asha.rao'                              # its own comes first
    assert store.clear_usernames({'memory:1', 'memory:3'}) == [ATLAS, 'https://usvisascheduling.com']
    assert store.username(ATLAS) == 'A. Rao'


def test_an_empty_host_is_never_a_site() -> None:
    """Task 6 re-review: an app's place has no host; nothing, or "www." against nothing, is never one site with
    anything."""
    for a, b in (('', ''), ('www.', ''), ('', 'www.'), ('usvisascheduling.com', ''), ('', 'usvisascheduling.com')):
        assert not same_site(a, b)
    assert same_site('www.usvisascheduling.com', 'usvisascheduling.com')


def test_the_records_of_one_site_with_and_without_www_serve_each_other(tmp_path: Path) -> None:
    """L36 §5: the planner writes a username's site with or without "www."; the two records are one site
    (`same_site`), so a username kept for https://usvisascheduling.com names the account of the entries saved at
    https://www.usvisascheduling.com. Entries still fill only at their own place, and two apps' records (no host)
    never serve each other."""
    _, keychain = fake_keychain()
    store = OwnerSecrets(tmp_path, keychain)
    store.remember('US visa password', VISA, 'v')
    store.set_username('https://usvisascheduling.com', 'Asha Rao', 'memory:1')
    assert store.username(VISA) == 'Asha Rao'
    assert store.listing(VISA)['linked'] == [{'place': 'https://usvisascheduling.com', 'username': 'Asha Rao',
                                              'entries': []}]
    assert store.listing('https://usvisascheduling.com')['linked'] == [{'place': VISA, 'username': None,
                                                                         'entries': ['US visa password']}]
    with pytest.raises(Refused, match='exact destination'):
        store.reveal('US visa password', 'https://usvisascheduling.com')
    assert store.username('https://ais.usvisascheduling.com') is None              # never a subdomain
    store.set_username('app:com.apple.Notes', 'asha', 'memory:2')
    assert store.username('app:com.example.Other') is None
    assert 'linked' not in store.listing('app:com.example.Other')


def test_a_username_is_kept_only_for_a_login_place(tmp_path: Path) -> None:
    _, keychain = fake_keychain()
    store = OwnerSecrets(tmp_path, keychain)
    for place in ('http://usvisascheduling.com', 'usvisascheduling.com', VISA + '/login'):
        with pytest.raises(Refused, match='https'):
            store.set_username(place, 'Asha Rao', 'memory:1')
    with pytest.raises(Refused, match='browser'):
        store.set_username('app:com.google.Chrome', 'Asha Rao', 'memory:1')
    assert not store.path.exists()
