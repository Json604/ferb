"""Profile discovery and name resolution (section 9.5): deterministic code that runs before any model.

Discovery reads only each Chromium browser's `Local State` -> profile.info_cache (folder -> display name, email);
never cookies or other profile data. Resolution: exact display name -> email / email local part -> saved alias;
a step matching more than one profile is ambiguous (never first match); no match lists what exists.
Opening: only folders returned by discovery are ever passed to `--profile-directory` (an unknown folder silently
creates a new profile). A running browser takes the request over; a browser that isn't running is started
detached. FERB never quits or kills the owner's browser.
"""

import json
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Literal

SUPPORT = Path.home() / "Library" / "Application Support"
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
                f"{p.browser}/{p.folder} ({p.display_name})" for p in self.candidates)
        return f"no profile called '{spoken}'; found: " + (", ".join(
            f"{p.browser}/{p.display_name}" for p in available) or "none")


def resolve(spoken: str, profiles: list[BrowserProfile], aliases: dict[str, tuple[str, str]] | None = None,
            browser: str | None = None) -> Resolution:
    """aliases: lower-cased alias -> (browser, folder). `browser` restricts the search when the owner named one."""
    pool = [p for p in profiles if browser is None or p.browser == browser]
    s = spoken.strip().lower()
    steps = [
        ("display_name", [p for p in pool if p.display_name.strip().lower() == s]),
        ("email", [p for p in pool if p.email and (p.email == s or p.email.split("@")[0] == s)]),
        ("alias", [p for p in pool if aliases and aliases.get(s) == (p.browser, p.folder)]),
    ]
    for step, hits in steps:
        if len(hits) == 1:
            return Resolution("one", step, (hits[0],))
        if len(hits) > 1:
            return Resolution("ambiguous", step, tuple(hits))
    return Resolution("none", "", ())


def running(browser: str) -> bool:
    return subprocess.run(["pgrep", "-x", BROWSERS[browser][2]], capture_output=True).returncode == 0


Launch = Callable[[list[str]], None]


def _detached(args: list[str]) -> None:
    subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                     start_new_session=True)             # never our child with a timeout; never waited on or killed


def open_known_profile(browser: str, folder: str, url: str, known: list[BrowserProfile],
                       launch: Launch = _detached) -> None:
    """Hand `url` to that profile. Refuses folders discovery didn't return."""
    if not any(p.browser == browser and p.folder == folder for p in known):
        raise ValueError(f"{browser}/{folder} isn't a discovered profile; refusing to create one")
    launch([BROWSERS[browser][1], f"--profile-directory={folder}", url])
