"""Owns Chromium profile metadata discovery, candidate matching and explicit profile launch.
Must not choose between plausible names/accounts or launch a visible URL as a background action.
Discovery reads Local State only; exact configured folders prevent accidental creation of profiles.
Name, account and saved-alias matches are returned together, never resolved by tier priority."""

import json
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Literal

SUPPORT = Path.home() / "Library" / "Application Support"
BUNDLES = {"chrome": "com.google.Chrome", "brave": "com.brave.Browser"}
BROWSERS: dict[str, tuple[Path, str, str]] = {   # browser -> (data dir, binary, process name)
    "chrome": (SUPPORT / "Google" / "Chrome", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
               "Google Chrome"),
    "brave": (SUPPORT / "BraveSoftware" / "Brave-Browser",
              "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser", "Brave Browser"),
}


@dataclass(frozen=True)
class BrowserProfile:
    browser: str
    folder: str          # identity: "Default", "Profile 3"
    display_name: str
    email: str           # "" when not signed in


def discover(browser: str, data_dir: Path | None = None) -> list[BrowserProfile]:
    local_state = (data_dir or BROWSERS[browser][0]) / "Local State"
    try:
        cache = json.loads(local_state.read_text()).get("profile", {}).get("info_cache", {})
    except (OSError, ValueError):
        return []
    return sorted((BrowserProfile(browser, folder, str(info.get("name", "")), str(info.get("user_name", "")).lower())
                   for folder, info in cache.items()), key=lambda p: p.folder)


def discover_all(data_dirs: dict[str, Path] | None = None) -> list[BrowserProfile]:
    dirs = data_dirs or {b: v[0] for b, v in BROWSERS.items()}
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


def resolve(spoken: str, profiles: list[BrowserProfile], aliases: dict[str, tuple[str, str]] | None = None,
            browser: str | None = None) -> Resolution:
    """aliases: lower-cased alias -> (browser, folder). `browser` restricts the search when the owner named one."""
    pool = [p for p in profiles if browser is None or p.browser == browser]
    s = spoken.strip().lower()
    steps = [
        ("profile_id", [p for p in pool if s in {
            f"{p.browser}/{p.folder}".lower(), f"{p.browser}/{p.display_name}".lower(),
            f'{p.browser} “{p.display_name}”'.lower(), f'{p.browser} "{p.display_name}"'.lower()}]),
        ("display_name", [p for p in pool if p.display_name.strip().lower() == s]),
        ("email", [p for p in pool if p.email and (p.email == s or p.email.split("@")[0] == s)]),
        ("alias", [p for p in pool if aliases and aliases.get(s) == (p.browser, p.folder)]),
    ]
    # A display name must not hide another profile's account or saved alias.
    hits = list(dict.fromkeys(p for _, candidates in steps for p in candidates))
    matched_steps = ",".join(step for step, candidates in steps if candidates)
    return Resolution("one" if len(hits) == 1 else "ambiguous" if hits else "none", matched_steps, tuple(hits))



def running(browser: str) -> bool:
    return subprocess.run(["pgrep", "-x", BROWSERS[browser][2]], capture_output=True).returncode == 0


Launch = Callable[[list[str]], None]


def _detached(args: list[str]) -> None:
    subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                     start_new_session=True)             # never our child with a timeout; never waited on or killed


def open_known_profile(browser: str, folder: str, url: str | None, known: list[BrowserProfile],
                       launch: Launch = _detached, background: bool = True,
                       is_running: Callable[[str], bool] = running) -> None:
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
    if background and url is not None:
        raise ValueError("needs_foreground: URL launch requires an explicit foreground setup step")
    if background and url is None and is_running(browser):
        launch([BROWSERS[browser][1], f"--profile-directory={folder}", "--no-startup-window"])
    elif background:
        launch(["open", "-g", "-b", BUNDLES[browser], "--args", f"--profile-directory={folder}",
                url or "--no-startup-window"])
    else:
        launch([BROWSERS[browser][1], f"--profile-directory={folder}", url or "--no-startup-window"])


def has_extension(p: BrowserProfile, extension_ids: frozenset[str], data_dir: Path | None = None) -> bool:
    """Is the FERB extension installed in this profile? Read from the profile's own settings on disk (both files
    Chromium uses), so it's known before the extension ever says hello."""
    folder = (data_dir or BROWSERS[p.browser][0]) / p.folder
    for name in ("Secure Preferences", "Preferences"):
        try:
            settings = json.loads((folder / name).read_text()).get("extensions", {}).get("settings", {})
        except (OSError, ValueError):
            continue
        if any(i in settings for i in extension_ids):
            return True
    return False
