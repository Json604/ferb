"""Owns matching an explicit profile selection to paired candidates; must not infer an account from browsing history.
Ambiguous matches remain evidence for agent judgment and, when unresolved, owner clarification."""

from dataclasses import dataclass
from typing import Callable

from ferb.act.browser.pairing import Binding
from ferb.act.browser.profiles import BrowserProfile, resolve
from ferb.core.errors import Refused
from ferb.core.types import Identity


class NeedsOwner(Refused):
    """Selection needs clarification by the agent; only unresolved owner choices go to chat."""


@dataclass(frozen=True)
class Choice:
    browser: str
    folder: str
    reason: str


@dataclass
class Resolver:
    profiles: Callable[[], list[BrowserProfile]]
    bindings: Callable[[], list[Binding]]
    identity_profile: Callable[[], tuple[str, str] | None]       # FERB's own profile, never for OWNER tasks
    aliases: Callable[[], dict[str, tuple[str, str]]] = lambda: {}

    def owner_paired(self) -> list[Binding]:
        ferb = self.identity_profile()
        return [b for b in self.bindings()
                if b.identity is Identity.OWNER and (b.browser, b.folder) != ferb]

    def choose(self, named: str | None) -> Choice:
        paired = {(b.browser, b.folder) for b in self.owner_paired()}
        ferb = self.identity_profile()
        if named:
            pool = [p for p in self.profiles() if (p.browser, p.folder) != ferb]
            r = resolve(named, pool, self.aliases())
            if r.kind != "one":
                raise NeedsOwner(r.message(named, pool))
            p = r.candidates[0]
            if (p.browser, p.folder) not in paired:
                raise NeedsOwner(f"{p.browser} profile '{p.display_name}' ({p.browser}/{p.folder}) isn't paired "
                                 f"with FERB yet: pair it with browser_pair profile={p.browser}/{p.folder} (FERB asks "
                                 "the owner once)")
            return Choice(p.browser, p.folder, f"named: {named}")
        if not paired:
            raise NeedsOwner("no browser profile is paired with FERB yet: pick the owner's profile from browser_profiles "
                             "and pair it with browser_pair (FERB asks the owner once)")
        raise NeedsOwner("select a profile from the task context; available: " +
                         ", ".join(self._name(b, f) for b, f in sorted(paired)) +
                         ". Use ferb_browser for ordinary browsing in FERB's own profile.")

    def _name(self, browser: str, folder: str) -> str:
        p = next((p for p in self.profiles() if (p.browser, p.folder) == (browser, folder)), None)
        return f"{browser}/{p.display_name if p else folder}"
