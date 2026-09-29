"""Site -> browser + profile for OWNER tasks (section 9.3). Deterministic; asks instead of guessing.

The unit is browser + profile. A profile the owner names wins (resolved with 9.5). Otherwise:
1. a paired profile already has that site open -> use it (several in one browser -> ask; several browsers ->
   `browser.prefer_for_actions`),
2. (trail history: build step 10),
3. the default browser's last-used profile, if paired,
4. ask, listing the paired profiles.
Never routes to FERB's identity profile or any install bound to identity FERB (10.1).
"""

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from ferb.act.browser.pairing import Binding
from ferb.act.browser.profiles import BROWSERS, BrowserProfile, resolve
from ferb.core.errors import Refused
from ferb.core.types import Identity
from ferb.policy.intent import site_matches


class NeedsOwner(Refused):
    """The resolver can't choose without the owner: the message is the question (with the options)."""


@dataclass(frozen=True)
class Choice:
    browser: str
    folder: str
    reason: str


@dataclass
class Resolver:
    profiles: Callable[[], list[BrowserProfile]]
    bindings: Callable[[], list[Binding]]
    open_hosts: Callable[[], list[tuple[str, str, str]]]         # (browser, folder, host) of open tabs
    identity_profile: Callable[[], tuple[str, str] | None]       # FERB's own profile, never for OWNER tasks
    prefer: Callable[[], list[str]] = lambda: ["brave", "chrome"]
    default_browser: Callable[[], str | None] = lambda: None
    last_used: Callable[[str], str | None] = lambda browser: None
    aliases: Callable[[], dict[str, tuple[str, str]]] = lambda: {}

    def owner_paired(self) -> list[Binding]:
        ferb = self.identity_profile()
        return [b for b in self.bindings()
                if b.identity is Identity.OWNER and (b.browser, b.folder) != ferb]

    def choose(self, site: str | None, named: str | None = None) -> Choice:
        paired = {(b.browser, b.folder) for b in self.owner_paired()}
        ferb = self.identity_profile()
        if named:
            pool = [p for p in self.profiles() if (p.browser, p.folder) != ferb]
            r = resolve(named, pool, self.aliases())
            if r.kind != "one":
                raise NeedsOwner(r.message(named, pool))
            p = r.candidates[0]
            if (p.browser, p.folder) not in paired:
                raise NeedsOwner(f"{p.browser} profile '{p.display_name}' isn't connected to FERB yet: run "
                                 f"`ferb browsers pair {p.browser} \"{p.display_name}\"`")
            return Choice(p.browser, p.folder, f"named: {named}")
        if site:
            hits = sorted({(b, f) for b, f, h in self.open_hosts() if (b, f) in paired and h
                           and site_matches(h, frozenset({site}))})
            if len(hits) == 1:
                return Choice(*hits[0], f"{site} is open there")
            if hits:
                order = [b for b in self.prefer() if any(h[0] == b for h in hits)] + \
                        sorted({h[0] for h in hits} - set(self.prefer()))
                top = [h for h in hits if h[0] == order[0]]
                if len(top) == 1:
                    return Choice(*top[0], f"{site} is open in several browsers; preferred {order[0]}")
                raise NeedsOwner(f"{site} is open in several profiles: " + ", ".join(
                    self._name(b, f) for b, f in top) + ". Which one?")
        default = self.default_browser()
        if default:
            folder = self.last_used(default)
            if folder and (default, folder) in paired:
                return Choice(default, folder, "default browser, last-used profile")
        if not paired:
            raise NeedsOwner("no browser profile is connected to FERB yet: run `ferb browsers pair <browser> <profile>`")
        raise NeedsOwner("which browser profile should FERB use? " + ", ".join(self._name(b, f) for b, f in sorted(paired)))

    def _name(self, browser: str, folder: str) -> str:
        p = next((p for p in self.profiles() if (p.browser, p.folder) == (browser, folder)), None)
        return f"{browser}/{p.display_name if p else folder}"


def last_used_profile(browser: str, data_dir: Path | None = None) -> str | None:
    try:
        v = json.loads(((data_dir or BROWSERS[browser][0]) / "Local State").read_text())["profile"]["last_used"]
        return str(v) if v else None
    except (OSError, ValueError, KeyError, TypeError):
        return None


BUNDLES = {"com.google.Chrome": "chrome", "com.brave.Browser": "brave"}


def default_browser() -> str | None:
    try:
        from AppKit import NSWorkspace
        from Foundation import NSBundle, NSURL
        app = NSWorkspace.sharedWorkspace().URLForApplicationToOpenURL_(NSURL.URLWithString_("https://example.com"))
        bundle = NSBundle.bundleWithURL_(app) if app else None
        return BUNDLES.get(str(bundle.bundleIdentifier())) if bundle else None
    except Exception:
        return None
