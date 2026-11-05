"""Owns FERB's Chrome Web Store install request, its native-host folders, per-profile extension status and the
store package, for every installed Chromium-family browser (L25).
Must not write a browser's Preferences or Secure Preferences, open, restart or quit a browser, or approve its prompt.

The request is the browser's documented per-user preference file (developer.chrome.com/docs/extensions/how-to/
distribute/install-extensions): `<user data>/External Extensions/<store id>.json` naming the Web Store's update URL.
The browser installs the extension at its next start, for every profile of that user-data folder, and asks the owner
to turn it on in each. macOS refuses off-store files (a local CRX) since Chrome 44, so only the Web Store id is ever
requested. Removing the file uninstalls it; one the owner removed is never added again by the browser.
FERB records each request file it writes in its own home (REQUEST_RECORD), never in a browser folder, so a changed or
cleared store id, and `ferb uninstall`, remove exactly the files FERB wrote."""

import json
import os
import re
import tempfile
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable

from ferb.act.browser import profiles
from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID
from ferb.config import Config

WEB_STORE_UPDATE_URL = "https://clients2.google.com/service/update2/crx"
STORE_EXTENSION_ID = "plbfkifcmgkgnlfgikdeaaglgncdhkma"   # FERB on the Chrome Web Store (live 2026-10-20); `[browser] store_extension_id` overrides
REQUEST_RECORD = "extension-requests.json"      # in FERB's home: the request files FERB wrote
_EXTENSION_ID = re.compile(r"[a-p]{32}")

# How FERB's extension reaches each browser FERB knows (HAND-247; one row per profiles.BROWSERS entry):
# - "request": the documented install request (the L25 report has the sources). Chrome documents the folder, and
#   Chromium's code reads `<user data>/External Extensions` on macOS for every build that keeps it (chrome_paths.cc,
#   external_provider_impl.cc).
# - "unconfirmed": the request is written, but no source confirms the browser honours it (the note says why).
# - "store": no request; the owner adds FERB from its Chrome Web Store page in that browser (Add to <browser>).
# Every one of them can add FERB from the Web Store page, so Load unpacked is only for when no store id is set.
_EDGE = ("Microsoft documents this folder with its own store's address; the Chrome Web Store's only for Windows. "
         "Edge adds FERB from the Chrome Web Store page once you allow extensions from other stores")
INSTALL: dict[str, tuple[str, str]] = {
    "chrome": ("request", ""), "chrome-beta": ("request", ""), "chrome-dev": ("request", ""),
    "chrome-canary": ("request", ""), "chromium": ("request", ""),
    "brave": ("unconfirmed", "Brave doesn't document this folder; it loads extensions as Chromium does"),
    "edge": ("unconfirmed", _EDGE), "edge-beta": ("unconfirmed", _EDGE), "edge-dev": ("unconfirmed", _EDGE),
    "edge-canary": ("unconfirmed", _EDGE),
    "vivaldi": ("store", "Vivaldi adds extensions straight from the Chrome Web Store"),
    "arc": ("store", "Arc adds extensions straight from the Chrome Web Store"),
    "opera": ("store", "Opera adds Chrome Web Store extensions once its “Install Chrome Extensions” add-on is on"),
}
DOCUMENTED = frozenset(b for b, (way, _) in INSTALL.items() if way == "request")
UNCONFIRMED = {b: note for b, (way, note) in INSTALL.items() if way == "unconfirmed"}

# Browsers that look for native hosts in Google Chrome's folder: Brave (github.com/brave/brave-browser/issues/5074),
# Opera (help.opera.com/en/extensions/message-passing/ gives Chrome's folder for macOS).
ALSO_READS = {"brave": "chrome", "opera": "chrome"}


def valid_id(value: str) -> bool:
    return bool(_EXTENSION_ID.fullmatch(value))


def store_extension_id(config: Config) -> str:
    """The Web Store id as configured; callers check it with valid_id before using it."""
    return str(config.get("browser", "store_extension_id", STORE_EXTENSION_ID) or "").strip()


def accepted_extension_ids(config: Config) -> tuple[str, ...]:
    """The owner's `[browser] extension_ids` that are extension ids, the pinned unpacked id (always), then the Web
    Store id when it's valid. They become the native host's allowed origins and the daemon's accepted callers."""
    configured = config.get("browser", "extension_ids", [])
    ids = [str(i) for i in (configured if isinstance(configured, list) else []) if valid_id(str(i))]
    store = store_extension_id(config)
    return tuple(dict.fromkeys(ids + [FERB_EXTENSION_ID] + ([store] if valid_id(store) else [])))


def native_host_dirs(support: Path, browsers: Iterable[str]) -> tuple[Path, ...]:
    """Each browser's per-user NativeMessagingHosts folder (inside its user-data folder, chrome_paths.cc), plus the
    folder it actually reads when that's another browser's."""
    dirs = [profiles.data_dir(b, support) / "NativeMessagingHosts"
            for browser in browsers for b in (browser, ALSO_READS.get(browser)) if b]
    return tuple(dict.fromkeys(dirs))


def request_file(browser: str, support: Path, store_id: str) -> Path:
    if not valid_id(store_id):
        raise ValueError(f"{store_id!r} isn't a Chrome extension id")
    return profiles.data_dir(browser, support) / "External Extensions" / f"{store_id}.json"


def write_request(browser: str, support: Path, store_id: str) -> Path:
    path = request_file(browser, support, store_id)
    body = json.dumps({"external_update_url": WEB_STORE_UPDATE_URL}, indent=2) + "\n"
    path.parent.mkdir(exist_ok=True)                 # inside an existing user-data folder; never creates one
    if not path.is_file() or path.read_text() != body:
        fd, tmp = tempfile.mkstemp(prefix=".ferb-", suffix=".json", dir=path.parent)
        try:
            with os.fdopen(fd, "w") as f:
                f.write(body)
            os.chmod(tmp, 0o644)
            os.replace(tmp, path)
        except BaseException:
            Path(tmp).unlink(missing_ok=True)
            raise
    return path


def _recorded(home: Path) -> list[Path]:
    try:
        data = json.loads((home / REQUEST_RECORD).read_text())
    except (OSError, ValueError):
        return []
    return [Path(p) for p in data if isinstance(p, str)] if isinstance(data, list) else []


def _save_record(home: Path, paths: Iterable[Path]) -> None:
    record = home / REQUEST_RECORD
    unique = sorted({str(p) for p in paths})
    if not unique:
        record.unlink(missing_ok=True)
        return
    fd, tmp = tempfile.mkstemp(prefix=".extension-requests.", suffix=".json", dir=home)      # created 0600
    try:
        with os.fdopen(fd, "w") as f:
            f.write(json.dumps(unique, indent=2) + "\n")
        os.replace(tmp, record)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise


def _is_request(path: Path) -> bool:
    """Only a file shaped like a request is ever removed: `.../External Extensions/<extension id>.json`."""
    return path.is_absolute() and path.parent.name == "External Extensions" and path.suffix == ".json" \
        and valid_id(path.stem)


def withdraw(home: Path, keep: str = "") -> list[Path]:
    """Remove the request files FERB recorded, except those for `keep` (the current valid store id). One that can't
    be removed stays recorded for the next run. Returns what was removed."""
    removed, kept = [], []
    for path in _recorded(home):
        if not _is_request(path):
            continue
        if path.stem == keep:
            kept.append(path)
            continue
        try:
            if path.is_file():
                path.unlink()
                removed.append(path)
        except OSError:
            kept.append(path)
    if (home / REQUEST_RECORD).exists():
        _save_record(home, kept)
    return removed


def remove_requests(home: Path, support: Path, store_id: str) -> list[Path]:
    """Uninstall: every request FERB recorded, plus the current id's in every known browser. The browser removes the
    extension at its next start once the file is gone."""
    removed = withdraw(home)
    for browser in profiles.BROWSERS if valid_id(store_id) else ():
        path = request_file(browser, support, store_id)
        if path.is_file():
            path.unlink()
            removed.append(path)
    return removed


@dataclass(frozen=True)
class ProfileStatus:
    browser: str
    folder: str
    name: str
    extension_id: str        # which FERB copy is installed here; "" when none
    enabled: bool
    removed: bool            # the owner removed the store copy earlier: the browser won't add it again
    paired: bool | None      # None: not checked (FERB's daemon didn't answer)
    version: str = ""        # the installed copy's version from the profile's files (HAND-412); "" when unknown
    disabled_for: frozenset[int] = frozenset()   # Chromium's reason codes for a copy that is off (HAND-412)
    email: str = ""          # the account the profile is signed in to the browser with, read fresh (HAND-424); ""


@dataclass(frozen=True)
class BrowserState:
    browser: str
    request: Path | None     # the install request written, if any
    profiles: tuple[ProfileStatus, ...]
    error: str = ""          # why the request couldn't be written


def _removed(sections: list[dict[str, Any]], store_id: str) -> bool:
    """The owner removed the store copy (HAND-416): Chromium lists an external extension the owner uninstalled in
    `extensions.external_uninstalls` and deletes its settings (ExtensionPrefs::OnExtensionUninstalled); older builds
    left its settings with `state` 2 instead. Installing it again clears the record."""
    for section in sections:
        listed = section.get("external_uninstalls")
        if isinstance(listed, list) and store_id in listed:
            return True
        entry = (section.get("settings") or {}).get(store_id) if isinstance(section.get("settings"), dict) else None
        if isinstance(entry, dict) and entry.get("state") == profiles.EXTERNAL_EXTENSION_UNINSTALLED:
            return True
    return False


def profile_status(p: profiles.BrowserProfile, ids: tuple[str, ...], data_dir: Path,
                   paired: set[tuple[str, str]] | None, store_id: str) -> ProfileStatus:
    found = profiles.extension_entry(p, ids, data_dir)
    removed = valid_id(store_id) and _removed(profiles.extension_prefs(data_dir / p.folder), store_id)
    return ProfileStatus(p.browser, p.folder, p.display_name, found[0] if found else "",
                         bool(found and profiles.extension_enabled(found[1])), removed,
                         None if paired is None else (p.browser, p.folder) in paired,
                         profiles.extension_version(found[1], data_dir / p.folder, found[0]) if found else "",
                         profiles.disable_reasons(found[1]) if found else frozenset(), p.email)


def prepare(support: Path, store_id: str, ids: tuple[str, ...], paired: set[tuple[str, str]] | None,
            home: Path | None = None) -> list[BrowserState]:
    """For every installed browser: write the Web Store request where a source supports it (only when `store_id` is
    a valid id; a browser whose folder refuses it is reported, the others go on), and read each profile's state.
    With `home`, each request written is recorded there."""
    out = []
    for browser in profiles.installed(support):
        request, error = None, ""
        if valid_id(store_id) and (browser in DOCUMENTED or browser in UNCONFIRMED):
            try:
                request = write_request(browser, support, store_id)
            except OSError as e:
                error = e.strerror or str(e)
        data = profiles.data_dir(browser, support)
        out.append(BrowserState(browser, request, tuple(profile_status(p, ids, data, paired, store_id)
                                                        for p in profiles.discover(browser, data)), error))
    written = [s.request for s in out if s.request is not None]
    if home is not None and written:
        _save_record(home, _recorded(home) + written)
    return out


def _profile_line(p: ProfileStatus, store_id: str) -> str:
    if not p.extension_id:
        state = (f"removed earlier, so the browser won't add it again: add it from "
                 f"https://chromewebstore.google.com/detail/{store_id}" if p.removed else "not installed")
    else:
        state = "installed " + ("from the Web Store" if p.extension_id == store_id else "unpacked")
        state += ", enabled" if p.enabled else ", waiting for you to turn it on"
        if p.enabled:
            state += (", pairing not checked (FERB isn't running)" if p.paired is None else ", paired" if p.paired
                      else ", not paired")
    where = f"{p.email}, {p.folder}" if p.email else p.folder
    return f"    “{p.name}” ({where}): {state}"


def read_states(support: Path, store_id: str, ids: tuple[str, ...], paired: set[tuple[str, str]] | None,
                data_dirs: dict[str, Path] | None = None) -> list[BrowserState]:
    """Each installed browser's state as `prepare` reports it, read only: nothing is written (L34 C: the setup window
    polls it). `request` is the install request file when one is already there. `data_dirs` (browser -> user-data
    folder) replaces discovery under `support` (the daemon's own mapping; tests); either way only browsers whose app
    is installed are read (HAND-414)."""
    dirs = profiles.only_installed(data_dirs) if data_dirs is not None \
        else {b: profiles.data_dir(b, support) for b in profiles.installed(support)}
    out = []
    for browser, data in dirs.items():
        request = data / "External Extensions" / f"{store_id}.json" if valid_id(store_id) else None
        out.append(BrowserState(browser, request if request is not None and request.is_file() else None,
                                tuple(profile_status(p, ids, data, paired, store_id)
                                      for p in profiles.discover(browser, data))))
    return out


def unsupported_line(app: str) -> str:
    """What setup says about an installed Chromium-family browser FERB doesn't know (HAND-415)."""
    return f"{app} isn't supported by FERB yet"


def describe(states: list[BrowserState], store_id: str, extension_dir: Path,
             withdrawn: Iterable[Path] = (), unsupported: Iterable[dict[str, str]] = ()) -> list[str]:
    """What setup tells the owner: per browser what was requested (or that FERB is added from its Web Store page there,
    INSTALL), per profile its state, which browsers to reopen, the Load-unpacked steps only when no store id is set,
    each installed Chromium-family browser FERB doesn't support yet (HAND-415), and earlier requests removed."""
    valid = valid_id(store_id)
    removed = [f"  Removed FERB's earlier install request {p}" for p in withdrawn]
    others = [f"  {unsupported_line(u['app'])}." for u in unsupported]
    if not states:
        return ["Browser extension: no Chromium-family browser has been started on this Mac yet; FERB sets it up "
                "the next time it updates after you start one.", *others, *removed]
    asked = [profiles.BROWSERS[s.browser].label for s in states if s.request is not None]
    if valid and asked:
        lines = [f"Browser extension: FERB asked {', '.join(asked)} to install it from the Chrome Web Store. Each "
                 "installs it at its next start, for all its profiles, and asks you to turn it on in each profile. "
                 "FERB never restarts a browser or answers its prompts."]
    elif valid:
        lines = ["Browser extension: no browser here took FERB's Chrome Web Store install request; add FERB from its "
                 "Chrome Web Store page in each (below)."]
    elif store_id:
        lines = [f"Browser extension: [browser] store_extension_id {store_id!r} in config.toml isn't a Chrome "
                 "extension id (32 letters a-p), so no browser was asked to install FERB."]
    else:
        lines = ["Browser extension: the Chrome Web Store listing is pending, so browsers can't be asked to install "
                 "FERB yet."]
    page = f"https://chromewebstore.google.com/detail/{store_id}"
    reopen: list[str] = []
    for s in states:
        label = profiles.BROWSERS[s.browser].label
        if not valid:
            note = ""
        elif s.error:
            note = f": couldn't write the install request ({s.error}); add FERB from {page}"
        elif s.request is None:
            how = INSTALL.get(s.browser, ("store", ""))[1]
            note = f": add FERB from {page}" + (f" ({how})" if how else "")
        elif s.browser in UNCONFIRMED:
            note = (f": install requested (unconfirmed: {UNCONFIRMED[s.browser]}; if FERB isn't offered after you "
                    f"reopen it, add it from {page})")
        else:
            note = ": install requested"
        lines.append(f"  {label}{note}")
        lines += [_profile_line(p, store_id) for p in s.profiles] or ["    no profiles yet"]
        if s.request is not None and any(not p.extension_id and not p.removed for p in s.profiles):
            reopen.append(label)
    if reopen:
        lines.append(f"  Reopen {', '.join(reopen)} to pick up the extension and FERB's native host.")
    if not valid:                            # no store listing to add it from: by hand
        lines.append("  Load unpacked, once per profile: open the browser's extensions page, turn on Developer mode, "
                     f"click “Load unpacked” and choose {extension_dir}")
    if any(p.paired is not True for s in states for p in s.profiles):
        lines.append("  To pair a profile with FERB turned on: Pair all or Pair in FERB's Settings → Browsers, or "
                     '`ferb browsers pair --browser <browser> "<profile>"`.')
    return lines + others + removed


def package(source: Path, out_dir: Path) -> tuple[Path, list[str]]:
    """The Web Store upload: every bundle file with manifest.json at the zip's root, minus the development `key`
    (the store assigns the listing its own) and Markdown (notes such as listing text, never extension code).
    Returns the zip and what the listing still needs."""
    manifest = json.loads((source / "manifest.json").read_text())
    missing = [k for k in ("name", "version", "description") if not manifest.get(k)]
    if missing:
        raise ValueError(f"manifest.json lacks {', '.join(missing)}")
    manifest.pop("key", None)
    notes = [] if manifest.get("icons") else ["manifest.json has no icons; the Chrome Web Store asks for them "
                                              "(at least a 128x128 PNG)"]
    out_dir.mkdir(parents=True, exist_ok=True)
    target = out_dir / f"ferb-extension-{manifest['version']}.zip"
    files = sorted(p for p in source.rglob("*") if p.is_file() and not p.is_symlink() and p.suffix != ".md"
                   and not any(part.startswith(".") for part in p.relative_to(source).parts))
    fd, tmp = tempfile.mkstemp(prefix=".ferb-extension-", suffix=".zip", dir=out_dir)
    os.close(fd)
    try:
        with zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED) as z:
            for f in files:
                name = f.relative_to(source).as_posix()
                if name == "manifest.json":
                    z.writestr(name, json.dumps(manifest, indent=2) + "\n")
                else:
                    z.write(f, name)
        os.replace(tmp, target)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise
    return target, notes
