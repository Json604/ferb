import json
from pathlib import Path

from ferb.device.extensions import extension_source, install_extension


def test_managed_bundle_updates_in_place_and_publishes_manifest_last(tmp_path, monkeypatch):
    source, home = tmp_path / 'source', tmp_path / 'home'
    source.mkdir()
    (source / 'manifest.json').write_text(json.dumps({'version': '1.0', 'key': 'public-key'}))
    (source / 'background.js').write_text('old code')
    target = install_extension(home, source)
    assert target == home / 'extensions' / 'chromium'
    (source / 'manifest.json').write_text(json.dumps({'version': '1.1', 'key': 'public-key'}))
    (source / 'background.js').write_text('new code')
    replaced = []
    original = Path.replace
    def replace(self, dest):
        replaced.append(Path(dest).name)
        return original(self, dest)
    monkeypatch.setattr(Path, 'replace', replace)
    assert install_extension(home, source) == target
    assert (target / 'background.js').read_text() == 'new code'
    assert json.loads((target / 'manifest.json').read_text())['version'] == '1.1'
    assert replaced[-1] == 'manifest.json'


def test_installed_package_uses_its_bundled_extension(tmp_path, monkeypatch):
    import ferb
    package = tmp_path / 'site-packages' / 'ferb'
    bundle = package / '_extension'
    bundle.mkdir(parents=True)
    (bundle / 'manifest.json').write_text('{"version":"1.0"}')
    monkeypatch.setattr(ferb, '__file__', str(package / '__init__.py'))
    assert extension_source() == bundle


def test_lab_native_host_uses_only_its_own_profile_and_socket(tmp_path):
    from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID
    from tests.chromelab import write_native_host
    profile = tmp_path / 'disposable-chrome'
    socket = tmp_path / 'test.sock'
    launcher = write_native_host(profile, socket, FERB_EXTENSION_ID)
    manifest = json.loads((profile / 'NativeMessagingHosts' / 'com.ferb.native_host.json').read_text())
    assert Path(manifest['path']) == launcher
    assert launcher.is_relative_to(profile)
    assert str(socket) in launcher.read_text()
    assert manifest['allowed_origins'] == [f'chrome-extension://{FERB_EXTENSION_ID}/']
    host = (profile / 'host' / 'ferb_host.py').read_text()
    assert "os.environ['FERB_SOCK']" in host and 'socket_path()' not in host
    compile(host, str(profile / 'host' / 'ferb_host.py'), 'exec')
