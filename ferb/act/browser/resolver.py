"""Owns matching an explicit profile selection to paired candidates; must not infer an account from browsing history.
Ambiguous matches remain evidence for agent judgment and, when unresolved, owner clarification."""

from dataclasses import dataclass
from typing import Callable

from ferb.act.browser.pairing import Binding
from ferb.act.browser.profiles import BrowserProfile, resolve, with_paired
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
    # the owner's own pick (Settings, `browsing.owner_profile`) for browsing as them while FERB has no profile of its own
    # (APP-1): an explicit owner setting, never inferred
    owner_default: Callable[[], tuple[str, str] | None] = lambda: None
    # why profile discovery can't see the owner's profiles now (daemon supplied: Full Disk Access is off), else None
    unseen: Callable[[], str | None] = lambda: None

    def known(self) -> list[BrowserProfile]:
        """Discovered profiles, plus paired ones of a browser discovery found none in, by folder (HAND-407)."""
        return with_paired(self.profiles(), [(b.browser, b.folder) for b in self.bindings()])

    def owner_paired(self) -> list[Binding]:
        ferb = self.identity_profile()
        return [b for b in self.bindings()
                if b.identity is Identity.OWNER and (b.browser, b.folder) != ferb]

    def choose(self, named: str | None) -> Choice:
        paired = {(b.browser, b.folder) for b in self.owner_paired()}
        ferb = self.identity_profile()
        if named:
            pool = [p for p in self.known() if (p.browser, p.folder) != ferb]
            r = resolve(named, pool, self.aliases())
            if r.kind != "one":
                why = self.unseen() if r.kind == "none" and not self.profiles() else None
                raise NeedsOwner(r.message(named, pool) + (f" ({why})" if why else ""))
            p = r.candidates[0]
            if (p.browser, p.folder) not in paired:
                account = f", signed in as {p.email}" if p.email else ""
                raise NeedsOwner(f"{p.browser} profile '{p.display_name}' ({p.browser}/{p.folder}{account}) isn't paired "
                                 f"with FERB yet: pair it with browser_pair profile={p.browser}/{p.folder} (FERB asks "
                                 "the owner once)")
            return Choice(p.browser, p.folder, f"named: {named}")
        chosen = self.owner_default() if ferb is None else None
        if chosen is not None and chosen in paired:
            return Choice(chosen[0], chosen[1], "the owner's chosen profile for browsing (FERB has no profile of its own)")
        if chosen is not None and chosen in {(p.browser, p.folder) for p in self.known()}:
            # APP-777: the owner picked it for FERB's web work, and it isn't paired yet: pairing it is the next step
            p = next(p for p in self.known() if (p.browser, p.folder) == chosen)
            account = f", signed in as {p.email}" if p.email else ""
            raise NeedsOwner(f"the owner picked {p.browser} profile '{p.display_name}' ({p.browser}/{p.folder}{account}) "
                             f"for FERB's web work and it isn't paired with FERB yet: pair it with browser_pair "
                             f"profile={p.browser}/{p.folder} (FERB asks the owner once)")
        if not paired:
            raise NeedsOwner("no browser profile is paired with FERB yet: pick the owner's profile from browser_profiles "
                             "and pair it with browser_pair (FERB asks the owner once)")
        raise NeedsOwner("select a profile from the task context; available: " +
                         ", ".join(self._name(b, f) for b, f in sorted(paired)) +
                         ". Use ferb_browser for ordinary browsing in FERB's own profile.")

    def _name(self, browser: str, folder: str) -> str:
        p = next((p for p in self.known() if (p.browser, p.folder) == (browser, folder)), None)
        return f"{browser}/{p.display_name if p else folder}" + (f" ({p.email})" if p and p.email else "")
