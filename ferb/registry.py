"""THE explicit list of what FERB loads (section 6, rule 3). No auto-discovery, no import-time side effects."""

from dataclasses import dataclass, field, replace
from typing import Any

from ferb.act.browser.tools import (BrowserAct, BrowserClose, BrowserFillOtp, BrowserFillSecret, BrowserNavigate,
                                   BrowserOpen, BrowserSnapshot, BrowserWait, FERB_BROWSER, IdentityDriver,
                                   OWNER_BROWSER)
from ferb.act.fs.files import FsEdit, FsList, FsRead, FsTrash, FsWrite
from ferb.act.mac.driver import MacComputerDriver
from ferb.act.mac.tools import MacApps, MacAct, MacAppleScript, MacClick, MacOpen, MacScreenshot, MacSnapshot
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
    r.add(BrowserAct(driver, f))
    r.add(BrowserWait(driver, f))
    r.add(BrowserClose(driver, f))
    r.add(BrowserFillSecret(driver, f))
    r.add(BrowserFillOtp(driver, f))


def add_owner_browsers(r: Registry, router: IdentityDriver, allow_local: bool = False) -> None:
    """The owner's paired Chrome/Brave profiles through the extension (identity OWNER only; build step 6a)."""
    f = replace(OWNER_BROWSER, allow_local=allow_local)
    r.add(BrowserOpen(router, f))
    r.add(BrowserNavigate(router, f))
    r.add(BrowserSnapshot(router, f))
    r.add(BrowserAct(router, f))
    r.add(BrowserWait(router, f))
    r.add(BrowserClose(router, f))


def add_mac(r: Registry, driver: MacComputerDriver, device_profile: Any) -> None:
    """The owner's Mac (identity OWNER only; build step 7)."""
    r.add(MacApps(driver, device_profile))
    r.add(MacOpen(driver, device_profile))
    r.add(MacSnapshot(driver, device_profile))
    r.add(MacAct(driver, device_profile))
    r.add(MacAppleScript(driver, device_profile))
    r.add(MacScreenshot(driver, device_profile))
    r.add(MacClick(driver, device_profile))


def add_shell(r: Registry, shells: ShellManager) -> None:
    """Shell commands (identity OWNER only, lost when exposed; build step 7)."""
    r.add(ShellRun(shells))
    r.add(ShellRead(shells))
    r.add(ShellSend(shells))
    r.add(ShellKill(shells))
    r.add(ShellList(shells))
