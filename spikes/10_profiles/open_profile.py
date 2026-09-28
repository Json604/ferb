"""Open a page in a named profile of an already-running browser (--profile-directory), find that exact window via
Accessibility by a one-time title token, and close only that window."""

import subprocess
import time

import ApplicationServices as AX
from AppKit import NSRunningApplication

BINARIES = {"chrome": "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "brave": "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"}
BUNDLES = {"chrome": "com.google.Chrome", "brave": "com.brave.Browser"}


def _attr(el, name):
    err, val = AX.AXUIElementCopyAttributeValue(el, name, None)
    return val if err == 0 else None


def running_pid(browser: str) -> int | None:
    apps = NSRunningApplication.runningApplicationsWithBundleIdentifier_(BUNDLES[browser])
    return apps[0].processIdentifier() if apps else None


def windows(pid: int) -> list:
    return list(_attr(AX.AXUIElementCreateApplication(pid), "AXWindows") or [])


def title(win) -> str:
    return str(_attr(win, "AXTitle") or "")


def open_in_profile(browser: str, folder: str, url: str, user_data_dir: str | None = None) -> float:
    """Launch the browser binary with --profile-directory; a running instance takes over and exits this process.
    Returns how long the forwarding process took."""
    args = [BINARIES[browser], f"--profile-directory={folder}", url]
    if user_data_dir:
        args.insert(1, f"--user-data-dir={user_data_dir}")
    elif running_pid(browser) is None:
        # The launcher would become the owner's browser itself. Never start (or later kill) it.
        raise RuntimeError(f"{browser} is not running; FERB never starts the owner's browser implicitly")
    t0 = time.monotonic()
    p = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
    while p.poll() is None:  # a hand-off exits within ~0.2 s; never kill it (it might be the browser)
        if time.monotonic() - t0 > 10:
            raise RuntimeError("launcher did not hand off to a running instance (left running, not killed)")
        time.sleep(0.05)
    return time.monotonic() - t0


def wait_window(pid: int, token: str, timeout: float = 20):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        for w in windows(pid):
            if token in title(w):
                return w
        time.sleep(0.2)
    return None


def close_window(win) -> None:
    button = _attr(win, "AXCloseButton")
    if button is None or AX.AXUIElementPerformAction(button, "AXPress") != 0:
        raise RuntimeError("could not close the window")


class UnknownProfile(Exception):
    pass


def open_known_profile(browser: str, folder: str, url: str, discovered: list, user_data_dir: str | None = None) -> float:
    """--profile-directory with a folder that doesn't exist silently CREATES a new profile, so only folders
    returned by discovery (Local State) may be opened."""
    if not any(p.browser == browser and p.folder == folder for p in discovered):
        raise UnknownProfile(f"{browser}/{folder} is not a discovered profile")
    return open_in_profile(browser, folder, url, user_data_dir)
