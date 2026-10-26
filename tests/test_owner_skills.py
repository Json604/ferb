"""P21 owner-written skill bodies use one contained reader for every runtime."""
from pathlib import Path
import pytest
from ferb.orchestrator.skills import Skills
from ferb.core.errors import Refused


def test_catalog_lists_metadata_and_body_is_read_on_demand(tmp_path):
    path = tmp_path / 'skills' / 'sessions'; path.mkdir(parents=True)
    (path / 'SKILL.md').write_text('---\nname: Sessions\ndescription: Rank coding sessions\n---\nRead every note.')
    skills = Skills(tmp_path / 'skills')
    assert skills.catalog() == [{'id': 'sessions', 'name': 'Sessions', 'description': 'Rank coding sessions'}]
    assert 'Read every note.' in skills.read('sessions')['body']
    assert 'Read every note.' not in str(skills.catalog())


def test_skill_symlinks_and_parent_paths_cannot_read_ferb_state(tmp_path):
    folder = tmp_path / 'skills'; folder.mkdir()
    (tmp_path / 'config.toml').write_text('private state')
    (folder / 'bad').symlink_to(tmp_path)
    skills = Skills(folder)
    assert skills.catalog() == []
    for name in ('../config.toml', 'bad'):
        with pytest.raises(Refused): skills.read(name)


def test_owner_skill_cannot_shadow_builtin_ability_or_acquire_its_origin(tmp_path):
    from ferb.abilities.catalog import AbilityCatalog
    from ferb.abilities.tools import AbilitiesRead
    from ferb.orchestrator.skills import SkillsRead
    folder = tmp_path / 'browser_verification'
    folder.mkdir()
    (folder / 'SKILL.md').write_text('---\nname: browser_verification\ndescription: My own notes\n---\nOwner notes.')
    skills = Skills(tmp_path)
    abilities = AbilityCatalog()
    assert skills.read('browser_verification')['body'] == 'Owner notes.'
    assert 'Owner notes.' not in abilities.read('browser_verification')['body']
    assert abilities.catalog()[0]['source_kind'] == 'builtin'
    assert SkillsRead(skills).origin == 'owner'
    assert AbilitiesRead(abilities).origin == 'ferb'
