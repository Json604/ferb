"""What the planner may use right now (owner decision, 2026-09-30), decided in code.

The planner's "what workers can do" list is built here each turn from the tools actually registered and what is
live: paired and connected browser profiles, FERB's own browser, the Mac permissions. A group that's missing gets one
short "Unavailable: X (reason)" line so FERB can tell the owner plainly what's missing and how to fix it. Plan
validation uses the same scopes, as the backstop.
"""

from dataclasses import dataclass, field


@dataclass(frozen=True)
class Group:
    key: str
    name: str                      # for the Unavailable line
    scopes: frozenset[str]
    text: str                      # the planner's line when usable ({profiles} filled for the owner's browsers)


GROUPS: tuple[Group, ...] = (
    Group("files", "files", frozenset({"fs:read", "fs:write"}),
          "Files: list, read, write, edit, move to Trash — inside FERB's workspace (paths relative to it) and, for "
          "\"owner\" tasks, folders the owner has allowed."),
    Group("owner_browsers", "the owner's browsers", frozenset({"browser:read", "browser:act"}),
          "The owner's browsers (\"owner\" identity), connected now: {profiles} — open pages in their signed-in "
          "sessions, read them, click/type/select, or take over a tab the owner already has open. Only when the "
          "owner's words point at their browser. FERB never signs in as the owner. If the owner names a profile "
          "(\"in my Work profile\"), put it in the goal."),
    Group("ferb_browser", "FERB's own browser", frozenset({"identity:browse", "secrets:fill", "mailbox:send"}),
          "FERB's own browser (\"ferb\" identity only; scopes {scopes} — never browser:read / browser:act, which "
          "are the owner's): websites for requests that don't say \"my …\", FERB's own accounts, passwords from the "
          "Keychain and codes "
          "from FERB's inbox (the worker never sees them), and sending email from FERB's own address (mailbox:send; "
          "only to recipients the owner named, held briefly so the owner can undo it). Never the owner's accounts "
          "or browsers."),
    Group("time", "reminders and timers", frozenset({"time"}),
          "Time (either identity): reminders and timers for the owner, and pausing a task until a time (time)."),
    Group("mac", "the owner's Mac apps", frozenset({"mac:read", "mac:act", "applescript"}),
          "The owner's Mac apps (\"owner\" identity): through Accessibility{and_applescript} ({scopes}). Apps "
          "work from the background — the owner stays in their own app: never plan to bring an app to the front, "
          "activate or switch to it; a goal is what to read or do in it (\"Read the top chat in WhatsApp\")."),
    Group("screen", "screenshots of app windows", frozenset({"screen"}),
          "A screenshot of one app window, with clicks by position (screen, mac:act). Give work in Mac apps the "
          "screen scope along with mac:read / mac:act: its snapshots then include an image of the window with each "
          "control numbered, which makes acting in apps far more reliable."),
    Group("shell", "shell commands", frozenset({"shell"}), "Shell commands on the owner's Mac (shell)."),
    Group("trail", "the owner's activity record", frozenset({"trail:read"}),
          "FERB's record of the owner's recent activity (trail:read)."),
)


@dataclass
class Availability:
    scopes: set[str]               # what a plan may use right now
    text: str                      # the planner's block
    missing: list[str] = field(default_factory=list)   # "FERB's own browser (not connected — run `ferb doctor`)"

    def unavailable_line(self) -> str:
        return ("Unavailable: " + "; ".join(self.missing) + ".") if self.missing else ""


def build(registered: set[str], problems: dict[str, str], profiles: str = "") -> Availability:
    """`registered`: scopes of the tools the daemon has. `problems`: group key -> why it can't be used right now
    (and how to fix it); a group with no tools and no stated problem simply isn't part of this FERB."""
    lines: list[str] = []
    scopes: set[str] = set()
    missing: list[str] = []
    for g in GROUPS:
        if g.key in problems:
            missing.append(f"{g.name} ({problems[g.key]})")
            continue
        present = g.scopes & registered
        if not present:
            continue
        scopes |= present                                 # a line names only the scopes present, never more
        lines.append("- " + g.text.format(profiles=profiles or "the paired profiles", scopes=", ".join(sorted(present)),
                                          and_applescript=" and AppleScript" if "applescript" in present else ""))
    text = "What workers can do right now:\n" + "\n".join(lines)
    a = Availability(scopes, text, missing)
    if missing:
        text += ("\n" + a.unavailable_line() + " If the request needs one of these, don't plan it: tell the owner "
                 "plainly what's missing and how to fix it.")
    else:
        text += "\nIf the owner asks for anything FERB can't do, reply that it can't."
    a.text = text
    return a
