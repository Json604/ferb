"""Browser profile discovery (Chromium `Local State`, read-only) and the deterministic name resolver (ARCH 9.5).

Resolution runs before any model: exact display name -> email / email local part -> saved alias. A step that
matches more than one profile is *ambiguous* (never first-match); no match lists what exists.
"""

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

SUPPORT = Path.home() / "Library" / "Application Support"
DATA_DIRS = {"chrome": SUPPORT / "Google" / "Chrome", "brave": SUPPORT / "BraveSoftware" / "Brave-Browser"}


@dataclass(frozen=True)
class BrowserProfile:
    browser: str
    folder: str          # identity: "Default", "Profile 3" (display names change, folders don't)
    display_name: str
    email: str           # "" when not signed in


def discover(browser: str, data_dir: Path | None = None) -> list[BrowserProfile]:
    """Reads only `Local State` -> profile.info_cache. Never touches profile folders, cookies or other data."""
    local_state = (data_dir or DATA_DIRS[browser]) / "Local State"
    if not local_state.exists():
        return []
    cache = json.loads(local_state.read_text()).get("profile", {}).get("info_cache", {})
    return sorted((BrowserProfile(browser, folder, str(info.get("name", "")), str(info.get("user_name", "")))
                   for folder, info in cache.items()), key=lambda p: (p.browser, p.folder))


@dataclass(frozen=True)
class Resolution:
    kind: Literal["one", "ambiguous", "none"]
    step: Literal["display_name", "email", "alias", ""]
    candidates: tuple[BrowserProfile, ...]

    def message(self, spoken: str, available: list[BrowserProfile]) -> str:
        if self.kind == "one":
            p = self.candidates[0]
            return f"{p.browser}/{p.folder}"
        if self.kind == "ambiguous":
            return f"'{spoken}' matches several profiles: " + ", ".join(
                f"{p.browser}/{p.folder} ({p.display_name})" for p in self.candidates)
        return f"no profile called '{spoken}'; found: " + ", ".join(
            f"{p.browser}/{p.display_name}" for p in available)


def resolve(spoken: str, profiles: list[BrowserProfile],
            aliases: dict[str, tuple[str, str]] | None = None, browser: str | None = None) -> Resolution:
    """aliases: lower-cased alias -> (browser, folder). `browser` restricts the search when the owner named one."""
    pool = [p for p in profiles if browser is None or p.browser == browser]
    s = spoken.strip().lower()
    steps = [
        ("display_name", [p for p in pool if p.display_name.strip().lower() == s]),
        ("email", [p for p in pool if p.email and (p.email.lower() == s or p.email.lower().split("@")[0] == s)]),
        ("alias", [p for p in pool if aliases and aliases.get(s) == (p.browser, p.folder)]),
    ]
    for step, hits in steps:
        if len(hits) == 1:
            return Resolution("one", step, (hits[0],))
        if len(hits) > 1:
            return Resolution("ambiguous", step, tuple(hits))
    return Resolution("none", "", ())
