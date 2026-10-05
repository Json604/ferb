"""Record extension-reported browser-tab observations. Must not select tabs, profiles or accounts.

Browser tabs: page loads and tab switches, as metadata (browser, profile, host, masked URL, title).

- Paired Chrome / Brave profiles: the extension's `tab_event`s, handed over by the Hub (`TabSink`). Tabs FERB opened
  for a task are FERB's; everything in FERB's own identity profile is FERB's; private windows are never recorded.
- Safari, and Chrome / Brave while no profile of theirs is connected: an AppleScript read of the front tab, only
  while that browser is frontmost and only if the owner has already granted Automation for it (never asked for
  here). The profile is unknown there."""

import asyncio
import subprocess
from typing import Any, Callable
from urllib.parse import urlsplit

from ferb.sense.trail import Observation
from ferb.sense.watchers.base import Emit, Focused, focused_app, poll

RECORDED = ("activated", "updated")          # a tab switched to, a page finished loading


def tab_observation(browser: str, folder: str, ferb_identity: bool, event: str, url: str | None, title: str | None,
                    incognito: bool | None, tab_task: str | None) -> Observation | None:
    if event not in RECORDED or not url:
        return None
    host = urlsplit(url).hostname or ""
    return Observation("browser_tab", {"browser": browser, "profile": folder, "host": host, "url": url,
                                       "title": title or "", "incognito": bool(incognito)},
                       {"ferb_identity": ferb_identity, "tab_task": tab_task})


SCRIPTS = {"com.apple.Safari": ("safari", "get {URL, name} of current tab of front window"),
           "com.google.Chrome": ("chrome", "get {URL, title} of active tab of front window"),
           "com.brave.Browser": ("brave", "get {URL, title} of active tab of front window")}


def applescript_front_tab(bundle_id: str) -> tuple[str, str] | None:
    _, body = SCRIPTS[bundle_id]
    script = f'tell application id "{bundle_id}"\n{body}\nend tell'
    r = subprocess.run(["osascript", "-e", script, "-s", "s"], capture_output=True, text=True, timeout=5)
    if r.returncode != 0:
        return None
    out = r.stdout.strip().strip("{}")
    parts = [p.strip().strip('"') for p in out.split('", "')]
    return (parts[0], parts[1] if len(parts) > 1 else "") if parts and parts[0] else None


class ScriptedTabsWatcher:
    """The AppleScript read, for browsers the extension doesn't cover."""
    source = "browser_tab"

    def __init__(self, automation_granted: Callable[[str], bool], extension_connected: Callable[[str], bool],
                 interval_s: float = 3.0, front: Callable[[], Focused | None] = focused_app,
                 read: Callable[[str], tuple[str, str] | None] = applescript_front_tab) -> None:
        self.granted, self.connected = automation_granted, extension_connected
        self.interval_s, self.front, self.read = interval_s, front, read
        self._last: tuple[str, str] | None = None

    def check(self) -> Observation | None:
        f = self.front()
        if f is None or f.bundle_id not in SCRIPTS:
            return None
        browser = SCRIPTS[f.bundle_id][0]
        if (browser != "safari" and self.connected(browser)) or not self.granted(f.bundle_id):
            return None
        tab = self.read(f.bundle_id)
        if tab is None or (f.bundle_id, tab[0]) == self._last:
            return None
        self._last = (f.bundle_id, tab[0])
        return Observation(self.source, {"browser": browser, "profile": None, "host": urlsplit(tab[0]).hostname or "",
                                         "url": tab[0], "title": tab[1]})

    async def run(self, emit: Emit, stop: asyncio.Event) -> None:
        self._last = None
        loop = asyncio.get_running_loop()

        def step() -> Any:
            try:
                o = self.check()
            except (OSError, subprocess.SubprocessError):
                return
            if o is not None:
                loop.call_soon_threadsafe(emit, o)
        await poll(self.interval_s, stop, step)
