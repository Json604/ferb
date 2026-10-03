"""FERB's managed unpacked extension. Publishing files never opens or edits a browser profile."""
import json
import shutil
import tempfile
from pathlib import Path

import ferb


def extension_source() -> Path:
    package = Path(ferb.__file__).resolve().parent
    checkout = package.parent / 'extensions' / 'chromium'
    return checkout if (checkout / 'manifest.json').is_file() else package / '_extension'


def extension_path(home: Path) -> Path:
    return home / 'extensions' / 'chromium'


def install_extension(home: Path, source: Path | None = None) -> Path:
    """Publish a complete bundle at a stable path, with the manifest/version becoming visible last."""
    source = source or extension_source()
    manifest = json.loads((source / 'manifest.json').read_text())
    if not manifest.get('version') or not (source / 'background.js').is_file():
        raise ValueError('incomplete FERB extension bundle')
    target = extension_path(home)
    target.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.extension-', dir=target.parent) as temp:
        staged = Path(temp)
        files = sorted(p for p in source.rglob('*') if p.is_file() and not p.is_symlink())
        files.sort(key=lambda p: p.name == 'manifest.json')
        for src in files:
            relative = src.relative_to(source)
            copy = staged / relative
            copy.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(src, copy)
        for src in files:
            relative = src.relative_to(source)
            dest = target / relative
            dest.parent.mkdir(parents=True, exist_ok=True)
            (staged / relative).replace(dest)
    return target
