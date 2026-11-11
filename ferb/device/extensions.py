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


# Chromium's reason code for a copy it turned off until the owner accepts the new permissions an update asks for
# (disable_reason::DISABLE_PERMISSIONS_INCREASE); the others are told as "turned off"
PERMISSIONS_INCREASE = 4


def not_paired_step(p: Any, label: str, requested: bool, store_url: str, extension_dir: Path) -> tuple[str, str] | None:
    """HAND-411: why Pair all can't pair this profile (`autoinstall.ProfileStatus`) as its extension stands, and the
    owner's step, from facts FERB already reads: removed by the owner (the browser won't add it again), not added yet
    with FERB's install request waiting for the browser's next start (`requested`), not added and no request, or added
    but turned off. HAND-412: a copy turned off says Chromium's own reason when it is the new-permissions one, and a
    copy whose installed version (from the profile's files) is below MIN_EXTENSION_VERSION is too old: its pairing
    page would only be refused, so none is opened. None: FERB's extension is on there and new enough."""
    from ferb.act.browser import autoinstall
    from ferb.act.browser.ext_protocol import MIN_EXTENSION_VERSION, version_ok
    add = (f"add FERB from its Chrome Web Store page in {label} ({store_url})" if store_url
           else f"add FERB's extension in {label} with Load unpacked ({extension_dir})")
    if not p.extension_id:
        if p.removed:
            return "removed", f"FERB's extension was removed there, so {label} won't add it again: {add}"
        if requested:
            step = f"{label} adds FERB the next time it starts (then turn FERB on when it asks)"
            if store_url:                       # item 7: the store page is the faster way, one click away
                return "requested", step + f"; faster: {add}"
            return "requested", step + (f"; if FERB isn't there then, {add}"
                                        if p.browser in autoinstall.UNCONFIRMED else "")
        return "not_added", add
    old = bool(p.version) and not version_ok(p.version)
    update = (f"FERB's extension there is version {p.version} and FERB needs {MIN_EXTENSION_VERSION} or later: open "
              f"chrome://extensions in that profile, turn on Developer mode and press Update, or quit and reopen "
              f"{label}")
    if not p.enabled:
        if PERMISSIONS_INCREASE in p.disabled_for:
            step = (f"{label} turned FERB off until you accept its new permissions: open chrome://extensions in that "
                    f"profile and turn FERB on")
        else:
            step = f"turn FERB on in {label}'s extensions (chrome://extensions in that profile)"
        return "off", step + (f"; {update[0].lower()}{update[1:]}" if old else "")
    if old:
        return "too_old", update
    return None


def browser_rows(states: list[Any], store_id: str, extension_dir: Path, ferb_identity: tuple[str, str] | None,
                 connected: set[tuple[str, str]] | dict[tuple[str, str], str],
                 unsupported: list[dict[str, str]] | None = None,
                 unpaired: dict[str, str] | None = None) -> dict[str, Any]:
    """`setup.browsers`'s reply: per browser whether FERB asked it to install the extension and whether Load unpacked is
    the way there (as `autoinstall.describe` says it), and per profile its state; `unsupported`: each installed
    Chromium-family browser FERB doesn't know, with its plain line (HAND-415). APP-883: each profile's installed
    extension version (from its files, read afresh) and whether it is below MIN_EXTENSION_VERSION (`too_old`).
    APP-892: `connected` may carry each connected profile's version as its hello said it (the copy that answers), shown
    before the files' one; `unpaired` (browser -> version, Hub.unpaired_copies) names browsers where a FERB copy is
    connected but not paired: a paired profile there that isn't connected `needs_pairing` (a reinstall gives the copy a
    new identity, so opening the profile won't connect it; pairing it again will), with that copy's version."""
    from ferb.act.browser import autoinstall, profiles
    from ferb.act.browser.ext_protocol import version_ok
    valid = autoinstall.valid_id(store_id)
    versions = connected if isinstance(connected, dict) else {}
    unpaired = unpaired or {}

    def row(p: Any) -> dict[str, Any]:
        key = (p.browser, p.folder)
        state = profile_state(p)
        needs = state == "paired" and key not in connected and p.browser in unpaired
        version = (unpaired[p.browser] if needs else versions.get(key, "")) or p.version
        return {"folder": p.folder, "name": p.name, "email": p.email, "state": state,
                "ferb_identity": key == ferb_identity, "connected": key in connected,
                "needs_pairing": needs, "version": version,
                "too_old": bool(version) and not version_ok(version)}
    return {"store_url": store_page(store_id) or None, "extension_dir": str(extension_dir),
            "unsupported": [{"app": u["app"], "bundle_id": u["bundle_id"],
                             "message": autoinstall.unsupported_line(u["app"])} for u in unsupported or []],
            "browsers": [{"browser": s.browser, "label": profiles.BROWSERS[s.browser].label,
                          "requested": s.request is not None,
                          "manual": not valid,          # HAND-247: every browser adds FERB from its store page
                          "unconfirmed": autoinstall.UNCONFIRMED.get(s.browser, ""),
                          "note": autoinstall.INSTALL.get(s.browser, ("store", ""))[1],
                          "profiles": [row(p) for p in s.profiles]}
                         for s in states]}
