import plistlib
from pathlib import Path
from types import SimpleNamespace

from ferb.sense.software import app_security_facts
from ferb.device.persona import mac_facts


def test_software_facts_are_observations_not_suspicion_labels(tmp_path: Path) -> None:
    calls = []
    def run(args, **kwargs):
        calls.append(args)
        if args[0] == "/usr/sbin/spctl":
            return SimpleNamespace(returncode=0, stdout=b"", stderr=b"accepted\nsource=Notarized Developer ID")
        if "--entitlements" in args:
            return SimpleNamespace(returncode=0, stdout=plistlib.dumps({"com.apple.security.network.client": True}), stderr=b"")
        return SimpleNamespace(returncode=0, stdout=b"", stderr=b"Authority=Developer ID Application: Example\nTeamIdentifier=TEAM")
    facts = app_security_facts(tmp_path / "OwnerTool.app", "owner.tool", run=run, xattr=lambda *args: b"", privacy=lambda *args: {"status": "unavailable"}, jobs=lambda *args: [])
    assert facts["assessment"]["accepted"] and facts["signature"]["TeamIdentifier"] == "TEAM"
    assert facts["entitlements"]["com.apple.security.network.client"] is True
    assert "suspicious" not in facts and all(command[0] != "open" for command in calls)


def test_mac_facts_use_usage_counts_not_installed_app_stereotypes() -> None:
    rows = [{"app": "Code", "bundle_id": "code", "uses": 3733}, {"app": "Blender", "bundle_id": "blender", "uses": 0}]
    facts = mac_facts(rows, "2026-10-31", [], [])
    assert [a["app"] for a in facts["apps"]] == ["Code"] and facts["apps"][0]["uses"] == 3733
    assert "developer" not in str(facts)


def test_login_item_association_uses_exact_identifiers_and_bundle_paths(tmp_path: Path) -> None:
    from ferb.sense.software import login_items
    dump = b'''UUID: one
Name: Owner Tool Helper
Identifier: helper
Parent Identifier: owner.tool
UUID: two
Identifier: owner.tool.lookalike
Parent Identifier: other
'''
    facts = login_items(tmp_path / 'OwnerTool.app', 'owner.tool', inventory=dump)
    assert [item['UUID'] for item in facts['items']] == ['one']


def test_macos_binary_xattr_uses_argument_vector(monkeypatch, tmp_path):
    import ferb.sense.software as software
    calls = []
    def run(args, **kwargs):
        calls.append(args)
        return SimpleNamespace(returncode=0, stdout=b'62 70 6c 69 73 74 00 0a\n')
    monkeypatch.setattr(software.subprocess, 'run', run)
    path = tmp_path / 'Owner tool.app'
    assert software.read_xattr(path, 'com.apple.metadata:kMDItemWhereFroms') == b'bplist\x00\n'
    assert calls == [['/usr/bin/xattr', '-px', 'com.apple.metadata:kMDItemWhereFroms', str(path)]]


def test_passive_software_facts_never_launch_an_authorization_tool(tmp_path: Path) -> None:
    calls = []
    def run(args, **kwargs):
        calls.append(args)
        return SimpleNamespace(returncode=0, stdout=b'', stderr=b'')
    facts = app_security_facts(tmp_path / 'OwnerTool.app', 'owner.tool', run=run,
                               xattr=lambda *args: b'', privacy=lambda *args: {'status': 'read', 'grants': []},
                               jobs=lambda *args: [{'label': 'owner.helper'}])
    assert all(command[0] != '/usr/bin/sfltool' for command in calls)
    assert facts['login_items']['status'] == 'unavailable'
    assert facts['login_items']['reason'] == 'no_supported_noninteractive_source'
    assert facts['privacy']['status'] == 'read'
    assert facts['launch_jobs'] == [{'label': 'owner.helper'}]
    assert len(calls) == 3  # signature, Gatekeeper assessment and entitlements still run


def test_software_watcher_reuses_initial_persistence_facts(monkeypatch, tmp_path: Path) -> None:
    import asyncio
    from ferb.sense.watchers import software
    facts = {'bundle_id': 'owner.tool', 'path': str(tmp_path / 'OwnerTool.app'),
             'privacy': {'status': 'read'}, 'launch_jobs': [], 'login_items': {'status': 'unavailable'}}
    probes = []
    def initial(*args):
        probes.append('initial')
        return facts
    def duplicate(*args):
        raise AssertionError('initial persistence facts must not be read twice')
    monkeypatch.setattr(software, 'app_security_facts', initial)
    monkeypatch.setattr(software, 'privacy_facts', duplicate)
    monkeypatch.setattr(software, 'associated_jobs', duplicate)
    monkeypatch.setattr(software, 'login_items', duplicate)
    async def run():
        stop = asyncio.Event()
        observations = []
        def emit(observation):
            observations.append(observation)
            stop.set()
        watcher = software.SoftwareWatcher(lambda: [('owner.tool', facts['path'])], lambda value: value)
        await watcher.run(emit, stop)
        assert [observation.data for observation in observations] == [facts]
    asyncio.run(run())
    assert probes == ['initial']
