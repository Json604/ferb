"""Own explicit capability and runtime registration. Must not choose methods, targets or authorization."""

from dataclasses import dataclass, field
from typing import Any, Callable

from ferb.act.browser.tools import (BrowserRecord, BrowserKeepOpen, BrowserAct, BrowserScreenshot, BrowserPoint, BrowserLoginCode, BrowserClose, BrowserFillOtp, BrowserFillSecret, BrowserNavigate,
                                   BrowserFind, BrowserRead, BrowserDialog, BrowserProfiles, BrowserOpen, BrowserScroll, BrowserSnapshot, BrowserTabs, BrowserUseTab, BrowserWait,
                                   FERB_BROWSER, IdentityDriver,
                                   BrowserOtpCandidates, OWNER_BROWSER)
from ferb.act.ask import OwnerAsk
from ferb.act.fs.files import FsEdit, FsList, FsRead, FsTrash, FsWrite
from ferb.act.mac.driver import MacComputerDriver
from ferb.act.mac.tools import (MacRecord, MacApps, MacAct, MacAppleScript, MacClick, MacPointer, MacMenu, MacOpen, MacScreenshot,
                                MacSnapshot)
from ferb.act.shell.manager import ShellManager
from ferb.act.shell.tools import ShellKill, ShellList, ShellRead, ShellRun, ShellSend, ShellStart, ShellWait
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
    from ferb.act.catalog import ActionsSearch
    r.add(ActionsSearch(lambda: r.capabilities))
    # capabilities — one line each
    r.add(FsList())
    r.add(FsRead())
    r.add(FsWrite())
    r.add(FsEdit())
    r.add(FsTrash())
    r.add(OwnerAsk())                 # a worker's question to the owner (every identity, no scopes)
    # runtimes implement AgentRuntime; config chooses per role
    r.runtimes["codex"] = "ferb.runtimes.codex_exec"
    r.runtimes["claude"] = "ferb.runtimes.claude_sdk"
    r.runtimes["anthropic"] = "ferb.runtimes.anthropic_api"
    r.runtimes["openai"] = "ferb.runtimes.openai_api"
    r.runtimes["google"] = "ferb.runtimes.google_api"
    return r


def add_identity_browser(r: Registry, driver: IdentityDriver, recordings: Any = None) -> None:
    """Tools that target FERB's own paired browser profile."""
    f = FERB_BROWSER
    r.add(BrowserOpen(driver, f))
    r.add(BrowserNavigate(driver, f))
    r.add(BrowserSnapshot(driver, f))
    r.add(BrowserRead(driver, f))
    r.add(BrowserDialog(driver, f))
    r.add(BrowserScreenshot(driver, f))
    r.add(BrowserPoint(driver, f))
    r.add(BrowserAct(driver, f))
    r.add(BrowserWait(driver, f))
    r.add(BrowserClose(driver, f))
    r.add(BrowserKeepOpen(driver, f))
    r.add(BrowserScroll(driver, f))
    r.add(BrowserFind(driver, f))
    r.add(BrowserFillSecret(driver, f))
    r.add(BrowserFillOtp(driver, f))
    r.add(BrowserOtpCandidates(driver, f))
    if recordings is not None:
        r.add(BrowserRecord(driver, f, recordings))


def add_owner_browsers(r: Registry, router: IdentityDriver, recordings: Any = None) -> None:
    """Tools that target the owner's paired Chrome/Brave profiles through the extension."""
    f = OWNER_BROWSER
    r.add(BrowserOpen(router, f))
    r.add(BrowserNavigate(router, f))
    r.add(BrowserSnapshot(router, f))
    r.add(BrowserRead(router, f))
    r.add(BrowserDialog(router, f))
    r.add(BrowserProfiles(router, f))
    r.add(BrowserScreenshot(router, f))
    r.add(BrowserPoint(router, f))
    r.add(BrowserAct(router, f))
    r.add(BrowserWait(router, f))
    r.add(BrowserClose(router, f))
    r.add(BrowserKeepOpen(router, f))
    r.add(BrowserScroll(router, f))
    r.add(BrowserFind(router, f))
    r.add(BrowserTabs(router, f))                      # the owner's open tabs (owner decision, 2026-09-30)
    r.add(BrowserUseTab(router, f))
    r.add(BrowserLoginCode(router, f))
    if recordings is not None:
        r.add(BrowserRecord(router, f, recordings))


def add_mac(r: Registry, driver: MacComputerDriver, device_profile: Any, recordings: Any = None) -> None:
    """Native Mac tools for the installed device profile."""
    r.add(MacApps(driver, device_profile))
    r.add(MacOpen(driver, device_profile))
    r.add(MacSnapshot(driver, device_profile))
    r.add(MacMenu(driver, device_profile))
    r.add(MacAct(driver, device_profile))
    r.add(MacAppleScript(driver, device_profile))
    r.add(MacScreenshot(driver, device_profile))
    r.add(MacClick(driver, device_profile))
    r.add(MacPointer(driver, device_profile))
    if recordings is not None:
        r.add(MacRecord(driver, recordings))


def add_trail(r: Registry, trail: Callable[[], Any]) -> None:
    """Search the owner's recorded activity."""
    from ferb.sense.tools import TrailQuery
    r.add(TrailQuery(trail))


def add_evidence(r: Registry, spans: Any, parent: Callable[[str], str | None],
                 root: Callable[[str], str | None] = lambda task_id: None) -> None:
    from ferb.orchestrator.evidence import TaskEvidence
    r.add(TaskEvidence(spans, parent, root))


def add_context_readers(r: Registry, continuity: Any, root: Callable[[str], str | None],
                        memory: Callable[[], Any]) -> None:
    from ferb.orchestrator.evidence import ChatHistory, MemoryRead
    r.add(ChatHistory(continuity, root))
    r.add(MemoryRead(memory))


def add_time(r: Registry, timers: Callable[[], Any], now: Callable[[], Any], sleep: Callable[[], Any],
             limit: Callable[[str], Any]) -> None:
    """Reminders for the owner and a worker's durable sleep."""
    from ferb.clock.tools import TimeRemindAt, TimeSetTimer, TimeSleepUntil
    r.add(TimeRemindAt(timers, now))
    r.add(TimeSetTimer(timers, now))
    r.add(TimeSleepUntil(timers, now, sleep, limit))


def add_shell(r: Registry, shells: ShellManager) -> None:
    """Task-owned shell processes."""
    r.add(ShellRun(shells))
    r.add(ShellStart(shells))
    r.add(ShellWait(shells))
    r.add(ShellRead(shells))
    r.add(ShellSend(shells))
    r.add(ShellKill(shells))
    r.add(ShellList(shells))


def add_recording(r: Registry, manager: Any) -> None:
    from ferb.act.recording import RecordingStop, RecordingInspect
    r.add(RecordingStop(manager))
    r.add(RecordingInspect())
