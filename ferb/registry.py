"""THE explicit list of what FERB loads (section 6, rule 3). No auto-discovery, no import-time side effects."""

from dataclasses import dataclass, field, replace
from typing import Any, Callable

from ferb.act.browser.tools import (BrowserAct, BrowserScreenshot, BrowserPoint, BrowserLoginCode, BrowserClose, BrowserFillOtp, BrowserFillSecret, BrowserNavigate,
                                   BrowserFind, BrowserOpen, BrowserScroll, BrowserSnapshot, BrowserTabs, BrowserUseTab, BrowserWait,
                                   FERB_BROWSER, IdentityDriver,
                                   MailboxSend, OWNER_BROWSER)
from ferb.act.ask import OwnerAsk
from ferb.act.fs.files import FsEdit, FsList, FsRead, FsTrash, FsWrite
from ferb.act.mac.driver import MacComputerDriver
from ferb.act.mac.tools import (MacApps, MacAct, MacAppleScript, MacClick, MacMenu, MacOpen, MacScreenshot,
                                MacSnapshot)
from ferb.act.shell.manager import ShellManager
from ferb.act.shell.tools import ShellKill, ShellList, ShellRead, ShellRun, ShellSend
from ferb.core.interfaces import Capability


@dataclass
class Registry:
    capabilities: dict[str, Capability] = field(default_factory=dict)
    runtimes: dict[str, str] = field(default_factory=dict)            # name -> module path (built on demand)
    unregistered: dict[str, str] = field(default_factory=dict)        # runtime -> why it was unregistered

    def add(self, cap: Any) -> None:
        if cap.name in self.capabilities:
            raise ValueError(f"capability {cap.name} registered twice")
        self.capabilities[cap.name] = cap


def build() -> Registry:
    r = Registry()
    # capabilities — one line each
    r.add(FsList())
    r.add(FsRead())
    r.add(FsWrite())
    r.add(FsEdit())
    r.add(FsTrash())
    r.add(OwnerAsk())                 # a worker's question to the owner (every identity, no scopes)
    # runtimes — one line each (section 5 AgentRuntime); config chooses per role
    r.runtimes["codex"] = "ferb.runtimes.codex_exec"
    r.runtimes["claude"] = "ferb.runtimes.claude_sdk"
    return r


def add_identity_browser(r: Registry, driver: IdentityDriver, allow_local: bool = False) -> None:
    """FERB's own browser (identity FERB only): the extension in Chrome's "FERB" profile (build step 6b)."""
    f = replace(FERB_BROWSER, allow_local=allow_local)
    r.add(BrowserOpen(driver, f))
    r.add(BrowserNavigate(driver, f))
    r.add(BrowserSnapshot(driver, f))
    r.add(BrowserScreenshot(driver, f))
    r.add(BrowserPoint(driver, f))
    r.add(BrowserAct(driver, f))
    r.add(BrowserWait(driver, f))
    r.add(BrowserClose(driver, f))
    r.add(BrowserScroll(driver, f))
    r.add(BrowserFind(driver, f))
    r.add(BrowserFillSecret(driver, f))
    r.add(BrowserFillOtp(driver, f))
    r.add(MailboxSend(driver))


def add_owner_browsers(r: Registry, router: IdentityDriver, allow_local: bool = False) -> None:
    """The owner's paired Chrome/Brave profiles through the extension (identity OWNER only; build step 6a)."""
    f = replace(OWNER_BROWSER, allow_local=allow_local)
    r.add(BrowserOpen(router, f))
    r.add(BrowserNavigate(router, f))
    r.add(BrowserSnapshot(router, f))
    r.add(BrowserScreenshot(router, f))
    r.add(BrowserPoint(router, f))
    r.add(BrowserAct(router, f))
    r.add(BrowserWait(router, f))
    r.add(BrowserClose(router, f))
    r.add(BrowserScroll(router, f))
    r.add(BrowserFind(router, f))
    r.add(BrowserTabs(router, f))                      # the owner's open tabs (owner decision, 2026-09-30)
    r.add(BrowserUseTab(router, f))
    r.add(BrowserLoginCode(router, f))


def add_mac(r: Registry, driver: MacComputerDriver, device_profile: Any,
            extension_ready: Callable[[str], bool] = lambda browser: False,
            unpaired: Callable[[str], str | None] = lambda browser: None) -> None:
    """The owner's Mac (identity OWNER only; build steps 7 and 9)."""
    r.add(MacApps(driver, device_profile))
    r.add(MacOpen(driver, device_profile))
    r.add(MacSnapshot(driver, device_profile))
    r.add(MacMenu(driver, device_profile))
    r.add(MacAct(driver, device_profile))
    r.add(MacAppleScript(driver, device_profile))
    r.add(MacScreenshot(driver, device_profile, extension_ready, unpaired))
    r.add(MacClick(driver, device_profile, extension_ready, unpaired))


def add_trail(r: Registry, trail: Callable[[], Any]) -> None:
    """The owner's recorded activity (identity OWNER only; build step 11)."""
    from ferb.sense.tools import TrailQuery
    r.add(TrailQuery(trail))


def add_time(r: Registry, timers: Callable[[], Any], now: Callable[[], Any], sleep: Callable[[], Any],
             limit: Callable[[str], Any]) -> None:
    """Reminders for the owner and a worker's durable sleep (build step 14)."""
    from ferb.clock.tools import TimeRemindAt, TimeSetTimer, TimeSleepUntil
    r.add(TimeRemindAt(timers, now))
    r.add(TimeSetTimer(timers, now))
    r.add(TimeSleepUntil(timers, now, sleep, limit))


def add_shell(r: Registry, shells: ShellManager) -> None:
    """Shell commands (identity OWNER only, lost when exposed; build step 7)."""
    r.add(ShellRun(shells))
    r.add(ShellRead(shells))
    r.add(ShellSend(shells))
    r.add(ShellKill(shells))
    r.add(ShellList(shells))
