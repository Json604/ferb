"""Record extension-reported browser-tab observations. Must not select tabs, profiles or accounts.

Browser tabs: page loads and tab switches, as metadata (browser, profile, host, masked URL, title).

- Paired Chrome / Brave profiles: the extension's `tab_event`s, handed over by the Hub (`TabSink`). Tabs FERB opened
  for a task are FERB's; everything in FERB's own identity profile is FERB's; private windows are never recorded.
- Safari, and a Chromium browser with tab scripting (Chrome and its channels, Chromium, Brave, Edge and its
  channels, Vivaldi, Arc) while no profile of it is connected: an AppleScript read of the front tab, only while that
  browser is frontmost and only if the owner has already granted Automation for it (never asked for here). The
  profile is unknown there.

The front window at send (L12) reads every window instead (`applescript_windows`) and takes the active tab of the one
FERB captured (`window_tab`), or none."""

import asyncio
import subprocess
from dataclasses import dataclass
from typing import Any, Callable
from urllib.parse import urlsplit

from ferb.act.browser.profiles import BROWSERS
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


# The front tab as two lines, URL then title (M1): Safari's tab, and the active tab of every Chromium browser whose
# AppleScript dictionary has tabs (Opera's has none). Keys are the profile table's browser names.
_CHROMIUM_TAB = "set t to active tab of front window\nreturn (URL of t) & linefeed & (title of t)"
SCRIPTS = {"com.apple.Safari": ("safari", "set t to current tab of front window\nreturn (URL of t) & linefeed & (name of t)"),
           **{b.bundle_id: (key, _CHROMIUM_TAB) for key, b in BROWSERS.items() if key != "opera"}}


def applescript_front_tab(bundle_id: str) -> tuple[str, str] | None:
    _, body = SCRIPTS[bundle_id]
    script = f'tell application id "{bundle_id}"\n{body}\nend tell'
    r = subprocess.run(["osascript", "-e", script], capture_output=True, text=True, timeout=5)
    if r.returncode != 0:
        return None
    url, _, title = r.stdout.removesuffix("\n").partition("\n")
    return (url.strip(), title) if url.strip() else None


# Every window of the browser, for the window FERB captured (L12): AppleScript's `front window` is the browser's
# frontmost window in its own order, which can be another window than the one in front of the owner (live case
# 2026-10-19). One record per window: its bounds (left, top, right, bottom: screen points from the top-left of the
# main display, as the window server's), its title, its active tab's URL and title; fields apart by the ASCII unit
# separator, records by the record separator (no title or URL holds them). A part the browser won't answer is empty.
_TABS = {"chromium": "set tb to active tab of w\n      set u to URL of tb\n      set ti to title of tb",
         "safari": "set tb to current tab of w\n      set u to URL of tb\n      set ti to name of tb"}
_WINDOWS = """set fs to character id 31
set rs to character id 30
set gs to character id 29
tell application id "{bundle}"
  set out to ""
  repeat with w in windows
    set b to ""
    try
      set {{x1, y1, x2, y2}} to bounds of w
      set b to (x1 as text) & gs & (y1 as text) & gs & (x2 as text) & gs & (y2 as text)
    end try
    set n to ""
    try
      set n to name of w
    end try
    set u to ""
    set ti to ""
    try
      {tab}
    end try
    set out to out & b & fs & n & fs & u & fs & ti & rs
  end repeat
  return out
end tell"""
WINDOW_SCRIPTS = {bundle: _TABS["safari" if key == "safari" else "chromium"] for bundle, (key, _) in SCRIPTS.items()}
NO_WINDOW = ("none of the browser's windows is where the captured window is, so FERB can't tell which tab it shows: "
             "no URL or tab title")
AMBIGUOUS = ("more than one of the browser's windows could be the captured one and its title doesn't single one out, "
             "so FERB can't tell which tab it shows: no URL or tab title")


@dataclass(frozen=True)
class ScriptedWindow:
    """One browser window as its AppleScript answers: where it is (x, y, w, h in screen points; None when the browser
    won't say), its title, and its active tab's URL and title ("" when they can't be read)."""
    bounds: tuple[float, float, float, float] | None
    name: str
    url: str
    title: str


def _bounds(field: str) -> tuple[float, float, float, float] | None:
    try:
        x1, y1, x2, y2 = (float(v.strip().replace(",", ".")) for v in field.split("\x1d"))
    except ValueError:
        return None
    return x1, y1, x2 - x1, y2 - y1


def applescript_windows(bundle_id: str) -> list[ScriptedWindow] | None:
    """Every window of this browser with its bounds, title and active tab (the AppleScript read; Automation must be
    granted already). None when the browser didn't answer."""
    script = _WINDOWS.format(bundle=bundle_id, tab=WINDOW_SCRIPTS[bundle_id])
    r = subprocess.run(["osascript", "-e", script], capture_output=True, text=True, timeout=5)
    if r.returncode != 0:
        return None
    out = []
    for record in r.stdout.removesuffix("\n").split("\x1e"):
        fields = record.split("\x1f")
        if len(fields) == 4:
            out.append(ScriptedWindow(_bounds(fields[0]), fields[1], fields[2].strip(), fields[3]))
    return out


def window_tab(windows: list[ScriptedWindow], bounds: tuple[float, float, float, float],
               title: str) -> ScriptedWindow:
    """The browser window FERB captured (at these bounds, with this window-server title), among those its AppleScript
    lists: the one at the same place (within 2 points, the capture path's tolerance); when several could be (maximized
    windows share bounds; a window whose bounds the browser won't give could be anywhere), the one among them whose
    title, or its active tab's, is the captured window's. LookupError (why, in FERB's words) when no single window is:
    no tab rather than another window's."""
    at = [w for w in windows if w.bounds is not None and all(abs(a - b) <= 2 for a, b in zip(w.bounds, bounds))]
    anywhere = [w for w in windows if w.bounds is None]
    if len(at) == 1 and not anywhere:
        return at[0]
    if not at and not anywhere:
        raise LookupError(NO_WINDOW)
    named = [w for w in at + anywhere if title and title in (w.name, w.title)]
    if len(named) != 1:
        raise LookupError(AMBIGUOUS)
    return named[0]


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
