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
