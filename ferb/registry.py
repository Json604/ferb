"""THE explicit list of what FERB loads (section 6, rule 3). No auto-discovery, no import-time side effects."""

from dataclasses import dataclass, field
from typing import Any

from ferb.act.browser.tools import (BrowserAct, BrowserClose, BrowserFillOtp, BrowserFillSecret, BrowserNavigate,
                                   BrowserOpen, BrowserSnapshot, BrowserWait, IdentityDriver)
from ferb.act.fs.files import FsEdit, FsList, FsRead, FsTrash, FsWrite
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
    """FERB's own browser (identity FERB only): the extension in Chrome's "FERB" profile, from build step 6."""
    r.add(BrowserOpen(driver, allow_local))
    r.add(BrowserNavigate(driver, allow_local))
    r.add(BrowserSnapshot(driver))
    r.add(BrowserAct(driver))
    r.add(BrowserWait(driver))
    r.add(BrowserClose(driver))
    r.add(BrowserFillSecret(driver))
    r.add(BrowserFillOtp(driver))
