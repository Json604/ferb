"""Owns Chromium profile metadata discovery, candidate matching and explicit profile launch.
Must not choose between plausible names/accounts or launch a visible URL as a background action.
Discovery reads Local State only; exact configured folders prevent accidental creation of profiles.
Name, account and saved-alias matches are returned together, never resolved by tier priority.
Every Chromium-family browser FERB knows is listed in BROWSERS; one counts as installed once it has created its
user-data folder (its Local State). Profile settings (Preferences, Secure Preferences) are only ever read."""

import json
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Literal

SUPPORT = Path.home() / "Library" / "Application Support"


@dataclass(frozen=True)
class Browser:
    label: str          # as the owner knows it
    data: str           # its user-data folder, relative to ~/Library/Application Support
    bundle_id: str
    app: str            # /Applications/<app>.app; also its executable's and process's name

    @property
    def binary(self) -> str:
        return f"/Applications/{self.app}.app/Contents/MacOS/{self.app}"


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
    """The browsers that have created their user-data folder, in BROWSERS order."""
    return [b for b in BROWSERS if (data_dir(b, support) / "Local State").is_file()]


@dataclass(frozen=True)
class BrowserProfile:
    browser: str
    folder: str          # identity: "Default", "Profile 3"
    display_name: str
    email: str           # "" when not signed in


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
    dirs = data_dirs or {b: SUPPORT / v.data for b, v in BROWSERS.items()}
    return [p for b, d in dirs.items() for p in discover(b, d)]


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
            f"{p.browser}/{p.display_name}" for p in available) or "none")


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


def extension_enabled(entry: dict[str, Any]) -> bool:
    """Chromium records why an extension is off in `disable_reasons` (a bit mask, a list in newer versions); older
    versions also kept `state` (0 = disabled)."""
    return entry.get("state", 1) != 0 and not entry.get("disable_reasons")


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
        if found:
            installed.append((i, {k: v for entry in found for k, v in entry.items()}))
    return next((e for e in installed if extension_enabled(e[1])), installed[0] if installed else None)


def extension_state(p: BrowserProfile, extension_ids: frozenset[str], data_dir: Path | None = None) -> str:
    """"on" (a FERB copy is turned on), "off" (installed but turned off: Chrome keeps an externally requested copy
    off until the owner approves it, "Action Required"; its pages are blocked) or "missing"."""
    entry = extension_entry(p, extension_ids, data_dir)
    if entry is None:
        return "missing"
    return "on" if extension_enabled(entry[1]) else "off"
