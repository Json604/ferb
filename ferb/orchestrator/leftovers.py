"""Own the owner's lines for what a task's cleanup couldn't release (HAND-419): one plain, specific line per leftover,
saying what is left and what the owner can do. Must not show raw errors or internal names: those stay in the
task.cleanup span of the trace.

`leftover_lines(resources, errors)` reads the release records the daemon returned (`resources`, as the task.cleanup
span holds them) and the cleanup's own errors (`errors`: a record's error, or a cleanup that was cancelled or raised
before any record came back). An app FERB left running already has its own line (launched.left_running) and is not
said again here."""

from typing import Any

UNSAID = "I couldn't finish tidying up after this task. The details are in the task's record."
OPENED_TAB = "I couldn't close one tab I opened: {where}. Close it if it's still open."
OWNERS_TAB = ("I couldn't fully let go of one of your tabs: {where}. It's still open as you left it; if {browser} "
              "says FERB started debugging it, press Cancel there.")
BY_RESOURCE = {
    "browser": "I couldn't confirm that the browser tabs this task used were closed or let go of. Close any FERB tab "
               "group that's still open.",
    "app": "I couldn't confirm that the apps this task opened were closed. Quit any you don't need.",
    "shell": "I couldn't confirm that this task's terminal commands all stopped. Activity Monitor shows anything "
             "still running.",
    "listen": "I couldn't confirm that listening stopped when this task ended.",
    "recording": "A screen recording this task made didn't end cleanly. The details are in the task's record.",
}
BY_RESOURCE["ferb_browser"] = BY_RESOURCE["browser"]


def leftover_lines(resources: list[dict[str, Any]], errors: list[str]) -> list[str]:
    """The owner's lines for the cleanup's leftovers, each once, in the order they came."""
    lines: list[str] = []
    said: set[str] = set()
    for r in resources:
        if not isinstance(r, dict) or not r.get("error"):
            continue
        said.add(str(r["error"]))
        kind = str(r.get("resource") or "")
        if kind == "app" and r.get("left_running"):
            continue                                      # its own line says the app is still open, and why
        tabs = [t for t in r.get("tabs") or [] if isinstance(t, dict)]
        lines += [_tab_line(t) for t in tabs] or [BY_RESOURCE.get(kind, UNSAID)]
    if any(e not in said for e in errors):                # a cleanup that stopped before any record came back
        lines.append(UNSAID)
    return list(dict.fromkeys(lines))


def _tab_line(tab: dict[str, Any]) -> str:
    browser = str(tab.get("browser") or "the browser")
    profile, title = str(tab.get("profile") or ""), str(tab.get("title") or "")
    account = str(tab.get("account") or "")                       # the profile's signed-in account (HAND-424)
    where = browser + (f" “{profile}”" if profile else "") + (f" ({account})" if profile and account else "") \
        + (f", {title}" if title else "")
    return (OWNERS_TAB if tab.get("borrowed") else OPENED_TAB).format(where=where, browser=browser)
