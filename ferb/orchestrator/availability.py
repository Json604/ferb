"""Owns installed capability facts for planning. Must not infer intent, choose a target or claim semantic success from transport facts.

Installed tool families and observed connection facts, without routing or permission decisions."""

from dataclasses import dataclass, field


@dataclass(frozen=True)
class Group:
    key: str
    name: str                      # for the Unavailable line
    scopes: frozenset[str]
    text: str                      # the planner's line when usable ({profiles} filled for the owner's browsers)


GROUPS: tuple[Group, ...] = (
    Group("files", "files", frozenset({"fs:read", "fs:write"}),
          "Files: list, read, write, edit and move to Trash. Relative paths use FERB's workspace; absolute paths "
          "use the supplied location and operating-system access."),
    Group("owner_browsers", "the owner's browsers", frozenset({"browser:read", "browser:act"}),
          "The owner's browsers, connection observations: {profiles} — open pages in their signed-in "
          "sessions, read them, click/type/select, or take over a tab the owner already has open. For work that "
          "needs the owner's accounts or a profile they name (prompts decision 5); login codes and passwords come "
          "through FERB's hidden input."),
    Group("ferb_browser", "FERB's own browser", frozenset({"identity:browse", "secrets:fill", "mailbox:send"}),
          "FERB's own browser ({scopes}): the default for web work, FERB's own accounts, passwords from the "
          "Keychain and codes from FERB's inbox (the worker never sees them), and sending email from FERB's own "
          "address (mailbox:send)."),
    Group("time", "reminders and timers", frozenset({"time"}),
          "Time (either identity): reminders and timers for the owner, and pausing a task until a time (time)."),
    Group("mac", "the owner's Mac apps", frozenset({"mac:read", "mac:act", "applescript"}),
          "The owner's Mac apps: through Accessibility{and_applescript} ({scopes}), from the background: the owner "
          "stays in their own app. A goal is what to read or do in it (\"Read the top chat in WhatsApp\")."),
    Group("screen", "screenshots of app windows", frozenset({"screen"}),
          "A screenshot of one app window, with screenshot-bound pointer input (screen, mac:act)."),
    Group("hearing", "hearing app sound", frozenset({"audio"}),
          "Hearing (audio): what one app or a task's tab is playing, as a transcript recognized on this Mac only; "
          "recordings can include the sound. The media must be playing and unmuted."),
    Group("shell", "shell commands", frozenset({"shell"}), "Shell commands on the owner's Mac (shell)."),
    Group("trail", "the owner's activity record", frozenset({"trail:read"}),
          "FERB's record of the owner's recent activity (trail:read)."),
)


@dataclass
class Availability:
    scopes: set[str]               # installed scopes, regardless of transient connection state
    text: str                      # the planner's block
    missing: list[str] = field(default_factory=list)   # "FERB's own browser (not connected — Settings → Browsers)"

    def unavailable_line(self) -> str:
        return ("Unavailable: " + "; ".join(self.missing) + ".") if self.missing else ""


def build(registered: set[str], problems: dict[str, str], profiles: str = "") -> Availability:
    """`registered`: scopes of the tools the daemon has. `problems`: group key -> why it can't be used right now
    (and how to fix it); a group with no tools and no stated problem simply isn't part of this FERB."""
    lines: list[str] = []
    scopes: set[str] = set(registered)
    missing: list[str] = []
    for g in GROUPS:
        if g.key in problems:
            missing.append(f"{g.name} ({problems[g.key]})")
        present = g.scopes & registered
        if not present:
            continue
        lines.append("- " + g.text.format(profiles=profiles or "the paired profiles", scopes=", ".join(sorted(present)),
                                          and_applescript=" and AppleScript" if "applescript" in present else ""))
    text = "Installed worker capabilities:\n" + "\n".join(lines)
    a = Availability(scopes, text, missing)
    if missing:
        text += "\nConnection/permission observations: " + "; ".join(missing) + "."
    a.text = text
    return a
