"""Install and register the packaged browser extension/native host. Must not choose an owner browser profile.

FERB's managed unpacked extension. Publishing files never opens or edits a browser profile."""
import json
import shutil
import tempfile
from pathlib import Path
from typing import Any

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


# ---------- L34 C: the setup window's Browsers step ----------

def store_page(store_id: str) -> str:
    """FERB's Chrome Web Store page (Add to <browser>), or "" when no valid store id is set."""
    from ferb.act.browser.autoinstall import valid_id
    return f"https://chromewebstore.google.com/detail/{store_id}" if valid_id(store_id) else ""


def profile_state(p: Any) -> str:
    """A profile as the setup window shows it: not_installed, removed (the owner removed the store copy, so the browser
    won't add it again), installed (waiting to be turned on), on, or paired."""
    if not p.extension_id:
        return "removed" if p.removed else "not_installed"
    if not p.enabled:
        return "installed"
    return "paired" if p.paired else "on"


def browser_rows(states: list[Any], store_id: str, extension_dir: Path, ferb_identity: tuple[str, str] | None,
                 connected: set[tuple[str, str]]) -> dict[str, Any]:
    """`setup.browsers`'s reply: per browser whether FERB asked it to install the extension and whether Load unpacked is
    the way there (as `autoinstall.describe` says it), and per profile its state."""
    from ferb.act.browser import autoinstall, profiles
    valid = autoinstall.valid_id(store_id)
    return {"store_url": store_page(store_id) or None, "extension_dir": str(extension_dir),
            "browsers": [{"browser": s.browser, "label": profiles.BROWSERS[s.browser].label,
                          "requested": s.request is not None,
                          "manual": not valid,          # HAND-247: every browser adds FERB from its store page
                          "unconfirmed": autoinstall.UNCONFIRMED.get(s.browser, ""),
                          "note": autoinstall.INSTALL.get(s.browser, ("store", ""))[1],
                          "profiles": [{"folder": p.folder, "name": p.name, "state": profile_state(p),
                                        "ferb_identity": (p.browser, p.folder) == ferb_identity,
                                        "connected": (p.browser, p.folder) in connected} for p in s.profiles]}
                         for s in states]}
