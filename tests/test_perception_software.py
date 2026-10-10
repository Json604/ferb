import plistlib
from pathlib import Path
from types import SimpleNamespace

from ferb.sense.software import app_security_facts
from ferb.device.persona import usage_summary, confirm_persona


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


def test_persona_uses_usage_counts_not_installed_app_stereotypes() -> None:
    rows = [{"app": "Code", "bundle_id": "code", "uses": 3733}, {"app": "Blender", "bundle_id": "blender", "uses": 0}]
    text = usage_summary(rows)
    assert "Code" in text and "3733" in text and "Blender" not in text
    assert "developer" not in text
    assert confirm_persona(text, "I code and make music") == "I code and make music"
    assert confirm_persona(text, "yes") == text
    assert confirm_persona(text, "skip") is None


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
    facts = login_items(tmp_path / 'OwnerTool.app', 'owner.tool', run=lambda *a, **k: SimpleNamespace(returncode=0, stdout=dump))
    assert [item['UUID'] for item in facts['items']] == ['one']


def test_persona_confirmation_is_once_and_preserves_the_owners_edit(tmp_path: Path) -> None:
    from ferb.config import Config
    from ferb.device.persona import setup_persona
    config = Config(tmp_path / 'home')
    calls, messages = [], []
    env = SimpleNamespace(say=messages.append, ask=lambda prompt: 'I code and make music')
    profile = SimpleNamespace(apps=[])
    client = SimpleNamespace(call=lambda method, params: calls.append((method, params)))
    setup_persona(env, config, profile, client, usage=lambda apps: [{'app': 'Code', 'uses': 3733}])
    setup_persona(env, config, profile, client, usage=lambda apps: [])
    assert len(calls) == 1 and calls[0][1]['content'] == 'I code and make music'
    assert config.get('setup', 'persona_reviewed') and '3733' in messages[0]
