"""Own explicit capability and runtime registration. Must not choose methods, targets or authorization."""

from dataclasses import dataclass, field
from typing import Any, Callable

from ferb.act.browser.tools import (BrowserRecord, BrowserKeepOpen, BrowserAct, BrowserScreenshot, BrowserPoint, BrowserLoginCode, BrowserClose, BrowserDownloads, BrowserFillOtp, BrowserFillSecret, BrowserMedia, BrowserNavigate,
                                   BrowserFind, BrowserRead, BrowserDialog, BrowserHandoff, BrowserPair, BrowserProfiles, BrowserOpen, BrowserScroll, BrowserSnapshot, BrowserTabs, BrowserUseTab, BrowserWait,
                                   FERB_BROWSER, IdentityDriver,
                                   BrowserOtpCandidates, OWNER_BROWSER)
from ferb.act.ask import OwnerAsk
from ferb.act.fs.files import FsEdit, FsList, FsRead, FsTrash, FsWrite
from ferb.act.mac.driver import MacComputerDriver
from ferb.act.mac.tools import (MacRecord, MacApps, MacAct, MacAppleScript, MacClick, MacPointer, MacMenu, MacOpen, MacScreenshot,
                                MacSnapshot)
from ferb.act.shell.manager import ShellManager
from ferb.act.shell.tools import ShellKill, ShellList, ShellRead, ShellRules, ShellRun, ShellSend, ShellStart, ShellWait
from ferb.core.interfaces import Capability
from ferb.abilities.catalog import AbilityCatalog


@dataclass
class Registry:
    """FERB's tools. Model runtimes are in their own table, ferb/runtimes/catalog.py."""
    capabilities: dict[str, Capability] = field(default_factory=dict)
    abilities: AbilityCatalog = field(init=False)

    def __post_init__(self) -> None:
        self.abilities = AbilityCatalog(capabilities=lambda: self.capabilities.keys())

    def add(self, cap: Any) -> None:
        if cap.name in self.capabilities:
            raise ValueError(f"capability {cap.name} registered twice")
        self.capabilities[cap.name] = cap


def build() -> Registry:
    r = Registry()
    from ferb.act.catalog import ActionsSearch
    r.add(ActionsSearch(lambda: r.capabilities))
    from ferb.abilities.tools import AbilitiesList, AbilitiesRead
    r.add(AbilitiesList(r.abilities))
    r.add(AbilitiesRead(r.abilities))
    # capabilities — one line each
    r.add(FsList())
    r.add(FsRead())
    from ferb.act.fs.usage import FsDiskUsage
    r.add(FsDiskUsage())
    r.add(FsWrite())
    r.add(FsEdit())
    r.add(FsTrash())
    r.add(OwnerAsk())                 # a worker's question to the owner (every identity, no scopes)
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
    r.add(BrowserDownloads(driver, f))                 # D35: downloads are facts, never a dialog
    r.add(BrowserMedia(driver, f))                     # L26: play/pause/mute/unmute/seek a page's video or audio
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
    r.add(BrowserPair(router, f))
    r.add(BrowserHandoff(router, f))
    r.add(BrowserScreenshot(router, f))
    r.add(BrowserPoint(router, f))
    r.add(BrowserAct(router, f))
    r.add(BrowserWait(router, f))
    r.add(BrowserClose(router, f))
    r.add(BrowserKeepOpen(router, f))
    r.add(BrowserScroll(router, f))
    r.add(BrowserFind(router, f))
    r.add(BrowserDownloads(router, f))                 # D35: downloads are facts, never a dialog
    r.add(BrowserMedia(router, f))                     # L26: play/pause/mute/unmute/seek a page's video or audio
    r.add(BrowserTabs(router, f))                      # the owner's open tabs (owner decision, 2026-09-30)
    r.add(BrowserUseTab(router, f))
    r.add(BrowserLoginCode(router, f))
    from ferb.act.browser.tools import BrowserSecret
    r.add(BrowserSecret(router, f))
    if recordings is not None:
        r.add(BrowserRecord(router, f, recordings))


def add_mac(r: Registry, driver: MacComputerDriver, device_profile: Any, recordings: Any = None,
            sandbox: Any = None, protection: Any = None) -> None:
    """Native Mac tools for the installed device profile. AppleScript runs under FERB's sandbox, so without one it is
    left out (never run uncontained: its `do shell script` would be a shell outside the sandbox)."""
    r.add(MacApps(driver, device_profile))
    r.add(MacOpen(driver, device_profile))
    r.add(MacSnapshot(driver, device_profile))
    r.add(MacMenu(driver, device_profile))
    r.add(MacAct(driver, device_profile))
    from ferb.act.mac.tools import MacSecret
    r.add(MacSecret(driver, device_profile))
    if sandbox is not None:
        r.add(MacAppleScript(driver, device_profile, sandbox))
    r.add(MacScreenshot(driver, device_profile))
    r.add(MacClick(driver, device_profile))
    r.add(MacPointer(driver, device_profile))
    if recordings is not None:
        r.add(MacRecord(driver, recordings))
    for cap in r.capabilities.values():
        if cap.name.startswith("mac."):
            cap.protection = protection  # type: ignore[attr-defined]
    if protection is not None:          # D31: the owner's terminal apps, beside policy/terminal.py's built-in list
        driver.terminal_apps = lambda: protection.terminal_apps
    from ferb.act.mac.clipboard import MacClipboard
    r.add(MacClipboard())


def add_trail(r: Registry, trail: Callable[[], Any]) -> None:
    """Search the owner's recorded activity."""
    from ferb.sense.tools import TrailDays, TrailQuery
    r.add(TrailQuery(trail))
    r.add(TrailDays(trail))


def add_evidence(r: Registry, spans: Any, parent: Callable[[str], str | None],
                 root: Callable[[str], str | None] = lambda task_id: None, store: Any = None,
                 secrets: Callable[[], Any] = lambda: ()) -> None:
    from ferb.orchestrator.evidence import TaskEvidence
    r.add(TaskEvidence(spans, parent, root, store, secrets))


def add_context_readers(r: Registry, continuity: Any, root: Callable[[str], str | None],
                        memory: Callable[[], Any], source_origins: Callable[[str], set[str]] | None = None,
                        search: Any = None) -> None:
    from ferb.orchestrator.evidence import ChatHistory, MemoryRead
    r.add(ChatHistory(continuity, root))
    r.add(MemoryRead(memory, source_origins, search))


def add_time(r: Registry, timers: Callable[[], Any], now: Callable[[], Any], sleep: Callable[[], Any],
             limit: Callable[[str], Any]) -> None:
    """Reminders for the owner and a worker's durable sleep."""
    from ferb.clock.tools import TimeRemindAt, TimeSetTimer, TimeSleepUntil
    r.add(TimeRemindAt(timers, now))
    r.add(TimeSetTimer(timers, now))
    r.add(TimeSleepUntil(timers, now, sleep, limit))


def add_shell(r: Registry, shells: ShellManager, protection: Any) -> None:
    """Task-owned shell processes, under FERB's sandbox; shell.rules lists its boundaries."""
    r.add(ShellRun(shells))
    r.add(ShellStart(shells))
    r.add(ShellWait(shells))
    r.add(ShellRead(shells))
    r.add(ShellSend(shells))
    r.add(ShellKill(shells))
    r.add(ShellList(shells))
    r.add(ShellRules(shells, protection))


def add_recovery(r: Registry, snapshots: Any) -> None:
    """S1: an agent's request for a disk snapshot before bulk or destructive work (ferb/act/fs/recovery.py)."""
    from ferb.act.fs.recovery import RecoverySnapshot
    r.add(RecoverySnapshot(snapshots))


def add_hearing(r: Registry, hearing: Any, browsers: Callable[[Any], Any]) -> None:
    """L26: hearing an app's or a task tab's sound as an on-device transcript (`browsers(identity)`: that identity's
    browser driver, for a tab)."""
    from ferb.act.listen import AudioListen
    r.add(AudioListen(hearing, browsers))


def add_recording(r: Registry, manager: Any) -> None:
    from ferb.act.recording import RecordingStop, RecordingInspect
    r.add(RecordingStop(manager))
    r.add(RecordingInspect())
