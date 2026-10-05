"""Current docs must not point to deleted source files or renamed Python entry points."""

from pathlib import Path

from scripts.check_doc_sources import DEFAULT_DOCS, check


def test_current_documentation_references_exist():
    assert check(Path(__file__).resolve().parents[1], list(DEFAULT_DOCS)) == []


def test_checker_detects_missing_path_and_qualified_symbol(tmp_path):
    folder = tmp_path / 'ferb'
    folder.mkdir()
    (folder / 'module.py').write_text('class Owner:\n    def run(self): pass\n')
    (tmp_path / 'guide.md').write_text(
        '<!-- source: ferb/module.py#Owner.run -->\n'
        '<!-- source: ferb/module.py#Owner.deleted -->\n'
        '[source](ferb/removed.py)\n')
    errors = check(tmp_path, ['guide.md'])
    assert any('Owner.deleted' in error for error in errors)
    assert any('ferb/removed.py' in error for error in errors)
    assert not any('Owner.run' in error for error in errors)
