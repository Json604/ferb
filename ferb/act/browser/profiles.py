"""Owns Chromium profile metadata discovery, candidate matching and explicit profile launch.
Must not choose between plausible names/accounts or launch a visible URL as a background action.
Discovery reads Local State only; exact configured folders prevent accidental creation of profiles.
Name, account and saved-alias matches are returned together, never resolved by tier priority.
BROWSERS maps each Chromium-family browser FERB knows to its user-data folder. One counts as installed only while
Launch Services knows its app on this Mac and it has created its user-data folder (its Local State): a folder an
uninstalled browser left behind is ignored everywhere (HAND-414). Profiles come only from that browser's own Local
State. Profile settings (Preferences, Secure Preferences) are only ever read."""

import json
import plistlib
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Literal

SUPPORT = Path.home() / "Library" / "Application Support"


def _launch_services(bundle_id: str) -> Path | None:
    """The executable of the app Launch Services knows for this bundle id, wherever it is installed (/Applications,
    ~/Applications, another volume); None when there is none."""
    import objc
    from AppKit import NSWorkspace
    from Foundation import NSBundle
    with objc.autorelease_pool():
        url = NSWorkspace.sharedWorkspace().URLForApplicationWithBundleIdentifier_(bundle_id)
        bundle = NSBundle.bundleWithURL_(url) if url is not None else None
        executable = bundle.executableURL() if bundle is not None else None
        return Path(str(executable.path())) if executable is not None else None


locate: Callable[[str], Path | None] = _launch_services     # tests replace it
_located: dict[str, Path] = {}


def _app_installed(bundle_id: str) -> bool:
    """Whether Launch Services knows an app with this bundle id on this Mac (HAND-414): its bundle is there and isn't
    in a Trash. A Mac where Launch Services can't be asked (PyObjC missing) falls back to the app folders."""
    try:
        import objc
        from AppKit import NSWorkspace
        with objc.autorelease_pool():
            url = NSWorkspace.sharedWorkspace().URLForApplicationWithBundleIdentifier_(bundle_id)
            path = Path(str(url.path())) if url is not None else None
    except Exception:
        b = next((b for b in BROWSERS.values() if b.bundle_id == bundle_id), None)
        return b is not None and any((root / f"{b.app}.app").is_dir()
                                     for root in (Path("/Applications"), Path.home() / "Applications"))
    return path is not None and path.is_dir() and ".Trash" not in path.parts


has_app: Callable[[str], bool] = _app_installed              # tests replace it


def app_installed(browser: str) -> bool:
    """The browser's app is installed on this Mac (HAND-414): only then is it offered, discovered, paired, listed or
    asked about."""
    b = BROWSERS.get(browser)
    if b is None:
        return False
    try:
        return bool(has_app(b.bundle_id))
    except Exception:
        return False


@dataclass(frozen=True)
class Browser:
    label: str          # as the owner knows it
    data: str           # its user-data folder, relative to ~/Library/Application Support
    bundle_id: str
    app: str            # <app>.app as the browser installs it; also its executable's and process's name

    @property
    def binary(self) -> str:
        """The browser's executable, found by bundle id through Launch Services (HAND-247: a Standard user's Chrome is
        often in ~/Applications), remembered while it is there; /Applications/<app>.app when Launch Services knows
        none."""
        known = _located.get(self.bundle_id)
        if known is None or not known.is_file():
            _located.pop(self.bundle_id, None)
            try:
                found = locate(self.bundle_id)
            except Exception:                    # PyObjC missing or Launch Services unavailable: the usual place
                found = None
            if found is None:
                return f"/Applications/{self.app}.app/Contents/MacOS/{self.app}"
            known = _located[self.bundle_id] = found
        return str(known)


# User-data folders: Chromium's docs/user_data_dir.md (Chrome, its channels, Chromium), Microsoft's Edge docs (Edge
# and its channels); Brave, Vivaldi, Arc and Opera as those browsers create them.
BROWSERS: dict[str, Browser] = {
    "chrome": Browser("Google Chrome", "Google/Chrome", "com.google.Chrome", "Google Chrome"),
    "chrome-beta": Browser("Google Chrome Beta", "Google/Chrome Beta", "com.google.Chrome.beta", "Google Chrome Beta"),
    "chrome-dev": Browser("Google Chrome Dev", "Google/Chrome Dev", "com.google.Chrome.dev", "Google Chrome Dev"),
    "chrome-canary": Browser("Google Chrome Canary", "Google/Chrome Canary", "com.google.Chrome.canary",
                             "Google Chrome Canary"),
    "chromium": Browser("Chromium", "Chromium", "org.chromium.Chromium", "Chromium"),
    "brave": Browser("Brave", "BraveSoftware/Brave-Browser", "com.brave.Browser", "Brave Browser"),
    "edge": Browser("Microsoft Edge", "Microsoft Edge", "com.microsoft.edgemac", "Microsoft Edge"),
    "edge-beta": Browser("Microsoft Edge Beta", "Microsoft Edge Beta", "com.microsoft.edgemac.Beta",
                         "Microsoft Edge Beta"),
    "edge-dev": Browser("Microsoft Edge Dev", "Microsoft Edge Dev", "com.microsoft.edgemac.Dev", "Microsoft Edge Dev"),
    "edge-canary": Browser("Microsoft Edge Canary", "Microsoft Edge Canary", "com.microsoft.edgemac.Canary",
                           "Microsoft Edge Canary"),
    "vivaldi": Browser("Vivaldi", "Vivaldi", "com.vivaldi.Vivaldi", "Vivaldi"),
    "arc": Browser("Arc", "Arc/User Data", "company.thebrowser.Browser", "Arc"),
    "opera": Browser("Opera", "com.operasoftware.Opera", "com.operasoftware.Opera", "Opera"),
}
# Which apps are web browsers at all (these and every other) is ferb/act/mac/browsers.py, the one browser list.


def data_dir(browser: str, support: Path | None = None) -> Path:
    return (support or SUPPORT) / BROWSERS[browser].data


def installed(support: Path | None = None) -> list[str]:
    """The browsers whose app is installed (HAND-414) and that have created their user-data folder, in BROWSERS
    order. A user-data folder whose browser's app is gone is ignored."""
    return [b for b in BROWSERS if (data_dir(b, support) / "Local State").is_file() and app_installed(b)]


def only_installed(dirs: dict[str, Path]) -> dict[str, Path]:
    """The given browser -> user-data folder map, keeping only browsers whose app is installed (HAND-414)."""
    return {b: d for b, d in dirs.items() if app_installed(b)}


# Chromium's own file type for an extension package, which Chromium's app Info.plist declares (chrome/app/
# app-Info.plist: CFBundleDocumentTypes and UTExportedTypeDeclarations) and every Chromium-built browser keeps; seen in
# Google Chrome and Brave. Electron apps, also built on Chromium's engine, don't declare it.
CHROMIUM_EXTENSION_TYPE = "org.chromium.extension"


def _https_handlers() -> list[Path]:
    """Every app bundle Launch Services lists as opening https links, read only; nothing opens."""
    import objc
    from AppKit import NSWorkspace
    from Foundation import NSURL
    with objc.autorelease_pool():
        urls = NSWorkspace.sharedWorkspace().URLsForApplicationsToOpenURL_(NSURL.URLWithString_("https://example.com"))
        return [Path(str(u.path())) for u in urls or ()]


handlers: Callable[[], list[Path]] = _https_handlers         # tests replace it


def _info(app: Path) -> dict[str, Any]:
    try:
        with (app / "Contents" / "Info.plist").open("rb") as f:
            info = plistlib.load(f)
    except (OSError, ValueError, plistlib.InvalidFileException):
        return {}
    return info if isinstance(info, dict) else {}


def chromium_family(info: dict[str, Any]) -> bool:
    """A structural fact from an app's Info.plist: it declares Chromium's extension file type."""
    declared = [t.get("UTTypeIdentifier") for key in ("UTExportedTypeDeclarations", "UTImportedTypeDeclarations")
                for t in (info.get(key) or []) if isinstance(t, dict)]
    opened = [c for d in (info.get("CFBundleDocumentTypes") or []) if isinstance(d, dict)
              for c in (d.get("LSItemContentTypes") or [])]
    return CHROMIUM_EXTENSION_TYPE in declared + opened


def unsupported() -> list[dict[str, str]]:
    """Each installed app macOS lists as opening https links that is a Chromium-family browser BROWSERS doesn't know
    (HAND-415), as {"app", "bundle_id"}: setup names it as not supported yet instead of passing it over. Read only;
    never raises (no list: none)."""
    known = {b.bundle_id.casefold() for b in BROWSERS.values()}
    try:
        apps = handlers()
    except Exception:
        return []
    out: dict[str, dict[str, str]] = {}
    for app in apps:
        if ".Trash" in app.parts:
            continue
        info = _info(app)
        ident = str(info.get("CFBundleIdentifier") or "")
        if not ident or ident.casefold() in known or ident.casefold() in out or not chromium_family(info):
            continue
        name = str(info.get("CFBundleDisplayName") or info.get("CFBundleName") or app.stem)
        out[ident.casefold()] = {"app": name, "bundle_id": ident}
    return sorted(out.values(), key=lambda r: r["app"].casefold())


@dataclass(frozen=True)
class BrowserProfile:
    browser: str
    folder: str          # identity: "Default", "Profile 3"
    display_name: str
    email: str           # "" when not signed in


def shown(p: BrowserProfile, label: str | None = None) -> str:
    """How FERB's own text names a profile (owner 2026-11-05, HAND-424): its browser, its name in quotes and, when the
    profile is signed in to the browser, that account in brackets — `Google Chrome “Work” (work@example.com)`; nothing
    after the name when it isn't. `label` replaces the browser's name ("" leaves it out). The account is read fresh
    from Local State each time (`discover`), never stored."""
    browser = label if label is not None else (BROWSERS[p.browser].label if p.browser in BROWSERS else p.browser)
    name = f"“{p.display_name or p.folder}”"
    return (f"{browser} {name}" if browser else name) + (f" ({p.email})" if p.email else "")


def named(found: list[BrowserProfile], browser: str, folder: str) -> BrowserProfile:
    """The discovered profile in that folder, else one named by its folder with no account (Full Disk Access off, its
    Local State unreadable: HAND-407 never guesses a name or an account)."""
    return next((p for p in found if (p.browser, p.folder) == (browser, folder)),
                BrowserProfile(browser, folder, folder, ""))


def discover(browser: str, data_dir: Path | None = None) -> list[BrowserProfile]:
    local_state = (data_dir or SUPPORT / BROWSERS[browser].data) / "Local State"
    try:
        cache = json.loads(local_state.read_text()).get("profile", {}).get("info_cache", {})
    except (OSError, ValueError):
        return []
    return sorted((BrowserProfile(browser, folder, str(info.get("name", "")), str(info.get("user_name", "")).lower())
                   for folder, info in cache.items()), key=lambda p: p.folder)


def last_used(browser: str, data_dir: Path | None = None) -> str | None:
    """The profile folder whose window was active last, as the browser records it in Local State (up to about ten
    seconds after a switch); read only. Chromium leaves the key out while only "Default" has been used and reads a
    missing key as "Default", so this does too when that profile exists."""
    try:
        profile = json.loads(((data_dir or SUPPORT / BROWSERS[browser].data) / "Local State").read_text()) \
            .get("profile", {})
        folder = profile.get("last_used")
        if folder is None and "Default" in (profile.get("info_cache") or {}):
            folder = "Default"
    except (OSError, ValueError, AttributeError, TypeError):
        return None
    return folder if isinstance(folder, str) and folder else None


def discover_all(data_dirs: dict[str, Path] | None = None) -> list[BrowserProfile]:
    """Every profile of every browser whose app is installed (HAND-414), from its own Local State."""
    dirs = only_installed(data_dirs if data_dirs is not None else {b: data_dir(b) for b in installed()})
    return [p for b, d in dirs.items() for p in discover(b, d)]


def with_paired(discovered: list[BrowserProfile], paired: list[tuple[str, str]]) -> list[BrowserProfile]:
    """The discovered profiles, plus each paired (browser, folder) of a browser discovery found no profile in (Full
    Disk Access off, its Local State unreadable), from FERB's own pairing record, named by its folder (HAND-407). A
    browser whose profiles were read keeps only those: a paired folder missing there is gone, and starting it would
    create it. A browser whose app isn't installed adds nothing (HAND-414)."""
    read = {p.browser for p in discovered}
    extra = [BrowserProfile(b, f, f, "") for b, f in dict.fromkeys(paired)
             if b in BROWSERS and b not in read and app_installed(b)]
    return discovered + extra


@dataclass(frozen=True)
class Resolution:
    kind: Literal["one", "ambiguous", "none"]
    step: str
    candidates: tuple[BrowserProfile, ...]

    def message(self, spoken: str, available: list[BrowserProfile]) -> str:
        if self.kind == "one":
            p = self.candidates[0]
            return f"{p.browser}/{p.folder}"
        if self.kind == "ambiguous":
            return f"'{spoken}' matches several profiles: " + ", ".join(
                f"{p.browser}/{p.folder} (name={p.display_name!r}, account={p.email or 'unknown'})" for p in self.candidates)
        return f"no profile called '{spoken}'; found: " + (", ".join(
            f"{p.browser}/{p.display_name}" + (f" ({p.email})" if p.email else "") for p in available) or "none")


def _norm(text: str) -> str:
    """A name as compared: invisible marks gone, quotes and spaces trimmed, inner whitespace one space, casefolded."""
    from ferb.act.mac.browsers import clean
    return " ".join(clean(text).strip("“”\"' ").split())


def _browser_names(key: str) -> set[str]:
    """What a browser may be called before a profile's name: its key, its label and its app's name ("chrome",
    "google chrome", "brave", "brave browser")."""
    b = BROWSERS.get(key)
    return {_norm(key)} | ({_norm(b.label), _norm(b.app)} if b else set())


def resolve(spoken: str, profiles: list[BrowserProfile], aliases: dict[str, tuple[str, str]] | None = None,
            browser: str | None = None) -> Resolution:
    """aliases: lower-cased alias -> (browser, folder). `browser` restricts the search when the owner named one.
    A profile is named by its folder or display name, alone or after its browser's name with any of "/", ":", a space
    or quotes between (live: 46 "no profile called 'chrome work'" / 'chrome:work' errors, 10-01 to 10-14), by its
    account, or by a saved alias; a browser's name alone names its profiles (one profile: that one; several: they are
    listed). Names compare without case, invisible marks or extra spaces; never fuzzily."""
    pool = [p for p in profiles if browser is None or p.browser == browser]
    s = _norm(spoken)
    named: list[BrowserProfile] = []
    for p in pool:
        for b in _browser_names(p.browser):
            if s.startswith(b) and len(s) > len(b) and s[len(b)] in "/: “\"":
                rest = _norm(s[len(b) + 1:])
                if rest in (_norm(p.folder), _norm(p.display_name)):
                    named.append(p)
    steps = [
        ("profile_id", named),
        ("display_name", [p for p in pool if _norm(p.display_name) == s]),
        ("email", [p for p in pool if p.email and (p.email == s or p.email.split("@")[0] == s)]),
        ("alias", [p for p in pool if aliases and aliases.get(s) == (p.browser, p.folder)]),
        ("browser", [p for p in pool if s in _browser_names(p.browser)]),
    ]
    # A display name must not hide another profile's account or saved alias.
    hits = list(dict.fromkeys(p for _, candidates in steps for p in candidates))
    matched_steps = ",".join(step for step, candidates in steps if candidates)
    return Resolution("one" if len(hits) == 1 else "ambiguous" if hits else "none", matched_steps, tuple(hits))



def running(browser: str) -> bool:
    """Whether that browser's own binary runs (its full path: `pgrep -x` matches a 16-character process name, so a
    browser with a longer app name, Chrome Beta or Edge Canary, never looked running and was woken as a cold one)."""
    import re
    pattern = "^" + re.escape(BROWSERS[browser].binary) + "( |$)"
    return subprocess.run(["pgrep", "-f", pattern], capture_output=True).returncode == 0


Launch = Callable[[list[str]], None]


def _detached(args: list[str]) -> None:
    subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                     start_new_session=True)             # never our child with a timeout; never waited on or killed


def open_known_profile(browser: str, folder: str, url: str | None, known: list[BrowserProfile],
                       launch: Launch = _detached, background: bool = True,
                       is_running: Callable[[str], bool] = running, window: bool = False) -> None:
    """Hand `url` to that profile — or, with no url, just load the profile with no window. Refuses folders discovery
    didn't return.

    Spike 19 (with the owner watching, 2026-10-01): loading a profile with no window through macOS's `open -g` never
    moved them, browser closed or open, and its extension came online; anything that creates a browser window pulled
    the browser to the front, however it was launched. Running the binary with a URL is only for pages the owner must
    see (pairing): `background=False`.

    A browser that's already running never sees `open`'s arguments (macOS passes them only at launch), so the wake
    did nothing (owner's run 2026-10-01): then the binary itself is run with no startup window, which reaches the
    running browser and loaded FERB's profile without bringing anything forward."""
    if not any(p.browser == browser and p.folder == folder for p in known):
        raise ValueError(f"{browser}/{folder} isn't a discovered profile; refusing to create one")
    if window:          # the profile's own window, no page: the owner's accepted one focus shift (2026-10-26)
        launch([BROWSERS[browser].binary, f"--profile-directory={folder}"])
        return
    if background and url is not None:
        raise ValueError("needs_foreground: URL launch requires an explicit foreground setup step")
    if background and url is None and is_running(browser):
        launch([BROWSERS[browser].binary, f"--profile-directory={folder}", "--no-startup-window"])
    elif background:
        launch(["open", "-g", "-b", BROWSERS[browser].bundle_id, "--args", f"--profile-directory={folder}",
                url or "--no-startup-window"])
    else:
        launch([BROWSERS[browser].binary, f"--profile-directory={folder}", url or "--no-startup-window"])


def extension_prefs(folder: Path) -> list[dict[str, Any]]:
    """The `extensions` sections of a profile's two settings files (Chromium keeps them in both), read only."""
    out = []
    for name in ("Secure Preferences", "Preferences"):
        try:
            doc = json.loads((folder / name).read_text())
        except (OSError, ValueError):
            continue
        section = doc.get("extensions") if isinstance(doc, dict) else None
        if isinstance(section, dict):
            out.append(section)
    return out


# Chromium's old Extension::State value for an external extension the owner uninstalled (kept in profiles written
# before Chromium moved that record to `extensions.external_uninstalls`).
EXTERNAL_EXTENSION_UNINSTALLED = 2


def installed_entry(entry: dict[str, Any]) -> bool:
    """Whether a settings entry is an installed extension as Chromium reads it (HAND-416): Chromium's
    ExtensionPrefs::GetInstalledInfoHelper (extensions/browser/extension_prefs.cc) skips an entry without an integer
    `location` or a string `path`, and older builds marked an external extension the owner removed with `state` 2.
    Anything else left under the id (a few keys written after a removal) is not an installed copy."""
    location, path = entry.get("location"), entry.get("path")
    return isinstance(location, int) and not isinstance(location, bool) and isinstance(path, str) and bool(path) \
        and entry.get("state") != EXTERNAL_EXTENSION_UNINSTALLED


def extension_enabled(entry: dict[str, Any]) -> bool:
    """Chromium records why an extension is off in `disable_reasons` (a bit mask, a list in newer versions); older
    versions also kept `state` (0 = disabled)."""
    return entry.get("state", 1) != 0 and not entry.get("disable_reasons")


def disable_reasons(entry: dict[str, Any]) -> frozenset[int]:
    """Chromium's reasons an extension is off, as its reason codes (HAND-412): newer versions keep a list of codes,
    older ones a bit mask; an old `state` 0 with no reason counts as turned off by the owner (1). Empty: not off."""
    raw = entry.get("disable_reasons")
    if isinstance(raw, list):
        codes = frozenset(int(r) for r in raw if isinstance(r, int) and not isinstance(r, bool) and r > 0)
    elif isinstance(raw, int) and not isinstance(raw, bool) and raw > 0:
        codes = frozenset(1 << bit for bit in range(raw.bit_length()) if raw >> bit & 1)
    else:
        codes = frozenset()
    return codes or (frozenset({1}) if entry.get("state", 1) == 0 else frozenset())


def extension_version(entry: dict[str, Any], folder: Path, extension_id: str) -> str:
    """The FERB copy's installed version as the profile's own files say it (HAND-412): its settings' manifest, else
    the version folder its `path` names ("<id>/<version>_0" for a store copy), else the newest version folder under
    the profile's Extensions/<id>. "" when none says."""
    manifest = entry.get("manifest")
    if isinstance(manifest, dict) and isinstance(manifest.get("version"), str) and manifest["version"]:
        return str(manifest["version"])[:32]
    path = entry.get("path")
    if isinstance(path, str) and path.startswith(extension_id + "/"):
        version = path.rsplit("/", 1)[-1].rsplit("_", 1)[0]
        if _version_like(version):
            return version
    try:
        found = [d.name.rsplit("_", 1)[0] for d in (folder / "Extensions" / extension_id).iterdir() if d.is_dir()]
    except OSError:
        return ""
    found = [v for v in found if _version_like(v)]
    return max(found, key=lambda v: tuple(int(x) for x in v.split("."))) if found else ""


def _version_like(value: str) -> bool:
    parts = value.split(".")
    return 0 < len(parts) <= 4 and all(x.isdigit() and len(x) <= 9 for x in parts)


def extension_entry(p: BrowserProfile, extension_ids: tuple[str, ...] | frozenset[str],
                    data_dir: Path | None = None) -> tuple[str, dict[str, Any]] | None:
    """The FERB copy in this profile, with its settings from both files merged: the first of `extension_ids` that is
    turned on, else the first installed. A profile that had the unpacked copy also takes the Web Store request, so
    it can hold both with one turned off; the one turned on is the one that answers."""
    folder = (data_dir or SUPPORT / BROWSERS[p.browser].data) / p.folder
    sections = [s.get("settings") for s in extension_prefs(folder)]
    installed = []
    for i in extension_ids:
        found = [s[i] for s in sections if isinstance(s, dict) and isinstance(s.get(i), dict)]
        merged = {k: v for entry in found for k, v in entry.items()}
        if found and installed_entry(merged):
            installed.append((i, merged))
    return next((e for e in installed if extension_enabled(e[1])), installed[0] if installed else None)


def extension_state(p: BrowserProfile, extension_ids: frozenset[str], data_dir: Path | None = None) -> str:
    """"on" (a FERB copy is turned on), "off" (installed but turned off: Chrome keeps an externally requested copy
    off until the owner approves it, "Action Required"; its pages are blocked) or "missing"."""
    entry = extension_entry(p, extension_ids, data_dir)
    if entry is None:
        return "missing"
    return "on" if extension_enabled(entry[1]) else "off"
