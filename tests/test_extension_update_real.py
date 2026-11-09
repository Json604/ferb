"""Windowless update/pairing check in a disposable Chrome; no owner profile or installed daemon is used."""
import json
import shutil
import tempfile
from pathlib import Path

import pytest

from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID, MIN_EXTENSION_VERSION
from ferb.clock.clock import SystemClock
from ferb.device.extensions import extension_source, install_extension
from tests.chromelab import ChromeLab, Frontmost, chrome_available, write_native_host
from tests.conftest import RunningDaemon, without_owner_mac
from tests.chromelab import wait_for as _wait

pytestmark = pytest.mark.real


def test_local_bundle_reload_keeps_pairing_and_reconnects_without_a_window():
    if not chrome_available():
        pytest.skip('Chrome required')
    base = Path(tempfile.mkdtemp(prefix='ferbext-'))
    profile = base / 'chrome'
    profile.mkdir()
    (profile / 'Local State').write_text(json.dumps({'profile': {'info_cache': {'Default': {'name': 'Lab'}}}}))
    source = base / 'source'
    shutil.copytree(extension_source(), source)
    target = install_extension(base / 'managed', source)
    d = RunningDaemon(base / 'home', clock=SystemClock())
    without_owner_mac(d)
    d.daemon.browser_data_dirs = {'chrome': profile}
    d.daemon.data_access, d.daemon.browser_support = (lambda: True), base / 'Support'   # its own folders only
    write_native_host(profile, d.sock, FERB_EXTENSION_ID)
    front, chrome = Frontmost(), ChromeLab(profile)
    try:
        # Use Chrome's own settings API in a hidden lab-only page. Protected prefs cannot be seeded by editing JSON.
        settings = chrome.send('Target.createTarget', {'url': 'chrome://extensions/', 'hidden': True,
                                                       'background': True})['targetId']
        settings_session = chrome.send('Target.attachToTarget', {'targetId': settings, 'flatten': True})['sessionId']
        _wait(lambda: chrome.evaluate(settings_session, "typeof chrome.developerPrivate !== 'undefined'"),
              10, 'extension settings not ready')
        chrome.evaluate(settings_session, "chrome.developerPrivate.updateProfileConfiguration({inDeveloperMode:true})")
        assert chrome.evaluate(settings_session, 'chrome.developerPrivate.getProfileConfiguration().then(p => p.inDeveloperMode)')
        chrome.send('Target.closeTarget', {'targetId': settings})
        chrome.load_extension(source)
        _wait(lambda: 'chrome' in d.daemon.hub.unpaired.values(), 15, 'no unpaired hello')
        async def token():
            return d.daemon.pairings.issue_token('chrome', 'Default')
        worker = chrome.worker_session(FERB_EXTENSION_ID)
        # as pair.js's message does: a `paired` is taken only while the pairing is pending (HAND-222)
        chrome.evaluate(worker, f"pairingPending = Date.now(); send('pair', {{token:{json.dumps(d.run(token()))}}})")
        _wait(lambda: (br := d.daemon.hub.bridges.get(('chrome', 'Default'))) and br.conn, 15, 'not paired')
        bridge = d.daemon.hub.bridges[('chrome', 'Default')]
        original_id, original_gen = bridge.binding.install_id, bridge.gen
        assert bridge.version == MIN_EXTENSION_VERSION
        # A one-time Load unpacked from the managed path must retain the existing install identity/key.
        chrome.load_extension(target)
        _wait(lambda: bridge.conn and bridge.gen > original_gen, 10, 'managed-path migration lost pairing')
        assert bridge.binding.install_id == original_id
        loaded = next(x for x in chrome.send('Extensions.getExtensions')['extensions'] if x['id'] == FERB_EXTENSION_ID)
        assert Path(loaded['path']).resolve() == target.resolve()
        original_gen = bridge.gen
        manifest = json.loads((source / 'manifest.json').read_text())
        version = manifest['version'].split('.')
        version[-1] = str(int(version[-1]) + 1)
        manifest['version'] = '.'.join(version)
        (source / 'manifest.json').write_text(json.dumps(manifest))
        install_extension(base / 'managed', source)
        # Use the real 30-second alarm, not a test-only call to the update function.
        _wait(lambda: bridge.conn and bridge.version == manifest['version'] and bridge.gen > original_gen,
              45, 'managed extension did not automatically reload and reconnect')
        assert bridge.binding.install_id == original_id
        assert not front.took_focus(chrome.pid)
        assert not [t for t in chrome.targets() if t['type'] == 'page']
        print(f'Managed-path migration and update {MIN_EXTENSION_VERSION} -> {bridge.version}; pairing preserved; no visible window')
    except Exception:
        print('extension diagnostics:', chrome.send('Extensions.getExtensions'))
        raise
    finally:
        chrome.close()
        d.stop()
        front.stop()
