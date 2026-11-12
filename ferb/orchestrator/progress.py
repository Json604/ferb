"""Owns the live progress line (L8): what a task is doing right now, in a few plain words, built by code from the tool
and its target. Must not use model text: the worker's `why`, typed text, message bodies, commands, queries, page
content and its own names for apps never appear, so injected text can't speak through the line. The exceptions are a
file's name the worker chose (its basename) and, while a call waits on or reads one of the task's own commands, that
command's first line, cut and quoted as the task's own words (ORCH-628, `waiting_for`). Describes; never gates or judges a call. (Outside these lines, a task asleep
in time.sleep_until shows its worker's reason quoted as the task's own words, one clipped line: ORCH-598.)

The target is the page's host (parsed as the browser opens it), an app's name as FERB knows it (`app_name`: the Mac's
app list, never the worker's argument) or a file's name; every other argument is ignored. A tool not named here is
described by the verb of its registry kind (W2)."""

from pathlib import PurePosixPath
from typing import Any, Callable, Iterable, Mapping

from ferb.policy import urls
from ferb.policy.rules import Kind
from ferb.sanitize.terminal import printable

BROWSERS = ("browser", "ferb_browser")       # FERB's own browser and the owner's share their tools' verbs
WORKING = "working"
AN_APP = "an app"                            # an app FERB couldn't match to the Mac's apps
FILE_KINDS = frozenset({Kind.WRITE_FILE, Kind.TRASH, Kind.DELETE_PERMANENT})   # what they change is a file

# Known tools: (with a target, without one). "{}" is the target.
TOOLS: dict[str, tuple[str, str]] = {
    "browser.open": ("opening {}", "opening a page"), "browser.navigate": ("opening {}", "opening a page"),
    "browser.read": ("reading {}", "reading a page"), "browser.snapshot": ("reading {}", "reading a page"),
    "browser.find": ("reading {}", "reading a page"), "browser.scroll": ("reading {}", "reading a page"),
    "browser.html": ("reading {}", "reading a page"),
    "browser.screenshot": ("looking at {}", "looking at a page"),
    "browser.wait": ("waiting for {}", "waiting for a page"),
    "mac.snapshot": ("looking at {}", "looking at an app"), "mac.screenshot": ("looking at {}", "looking at an app"),
    "mac.open": ("opening {}", "opening an app"),
    "fs.read": ("reading {}", "reading a file"), "fs.list": ("looking in {}", "looking in a folder"),
    "fs.look": ("looking at {}", "looking at a file"),
    "fs.write": ("writing {}", "writing a file"), "fs.edit": ("editing {}", "editing a file"),
    "fs.trash": ("moving {} to the Trash", "moving a file to the Trash"),
    "shell.run": ("running a command", "running a command"),
    "shell.start": ("starting a command", "starting a command"),
    "shell.wait": ("waiting for a command", "waiting for a command"),
    "time.sleep_until": ("waiting", "waiting"),
    "wait.for": ("waiting", "waiting"),          # its own line follows at once (wait.py, Supervisor.task_sleeping)
    "owner.ask": ("asking you", "asking you"),
}

# Every other tool: the verb of the kind its describe() reports (W2).
KINDS: dict[str, tuple[str, str]] = {
    Kind.READ: ("reading {}", "reading"), Kind.NAVIGATE: ("opening {}", "opening a page"),
    Kind.SEARCH: ("searching {}", "searching"), Kind.OPEN_TAB: ("opening {}", "opening"),
    Kind.DRAFT: ("drafting in {}", "drafting"), Kind.FILL_FORM: ("filling in a form in {}", "filling in a form"),
    Kind.SUBMIT: ("submitting in {}", "submitting"), Kind.SEND: ("sending from {}", "sending"),
    Kind.POST: ("posting in {}", "posting"), Kind.REPLY: ("replying in {}", "replying"),
    Kind.PAY: ("paying in {}", "paying"), Kind.DELETE_PERMANENT: ("deleting {}", "deleting"),
    Kind.TRASH: ("moving {} to the Trash", "moving to the Trash"), Kind.WRITE_FILE: ("writing {}", "writing a file"),
    Kind.RUN_SHELL: ("running a command", "running a command"),
    Kind.APPLESCRIPT: ("running a script in {}", "running a script"), Kind.MAC_INPUT: ("using {}", "using the Mac"),
    Kind.CLICK: ("clicking in {}", "clicking"), Kind.FILL_SECRET: ("filling a saved secret in {}", "filling a saved secret"),
    Kind.MAILBOX_READ: ("reading mail", "reading mail"), Kind.MAILBOX_SEND: ("sending mail", "sending mail"),
    Kind.TIMER: ("setting a reminder", "setting a reminder"),
    Kind.ACCOUNT_SECURITY: ("changing account security in {}", "changing account security"),
    Kind.CALL: ("starting a call in {}", "starting a call"),
    Kind.SNAPSHOT: ("saving a recovery point", "saving a recovery point"),
    Kind.STOP_PROCESS: ("stopping a command", "stopping a command"),
    "owner_question": ("asking you", "asking you"),
}


def describe(tool: str, args: Mapping[str, Any], *, kind: str = "", site: str | None = None,
             app: str | None = None) -> str:
    """The live line for one tool call. `kind`: the registry kind its describe() reported; `site`: the host the
    gateway derived for a browser tool's tab; `app`: a Mac tool's app as `app_name` resolved it (without one, an app
    the arguments name shows as "an app")."""
    family, _, verb = tool.rpartition(".")
    name = "browser." + verb if family in BROWSERS else tool
    with_target, alone = TOOLS.get(name) or KINDS.get(kind) or (WORKING, WORKING)
    target = _target(family, args, kind, site, app)
    return with_target.format(target) if target and "{}" in with_target else alone


WAITS_ON_COMMAND = {"shell.wait": "waiting for", "shell.read": "reading the output of"}
COMMAND_CHARS = 40                           # a command's first line in a live line, cut at a word (NAME_CHARS)


def waiting_for(tool: str, command: str, minutes: int) -> str:
    """ORCH-628: the line of a call that waits on or reads one of the task's commands: the command's first line,
    printable, cut at a word, in backticks as the task's own words, and how long it has run ("waiting for `codex exec
    -s danger-full-access …` (14 min)"). The caller redacts it as every live line."""
    said = command_said(command)
    said = f"{WAITS_ON_COMMAND[tool]} {said}" if said else f"{WAITS_ON_COMMAND[tool]} a command"
    return said + (f" ({minutes} min)" if minutes >= 1 else "")


def command_said(command: str) -> str:
    """A command as a live line quotes it (ORCH-628): its first line, printable, cut at a word to COMMAND_CHARS, in
    backticks as the task's own words; "" when it has none."""
    first = " ".join(printable(next((line for line in command.splitlines() if line.strip()), "")).split())
    if len(first) > COMMAND_CHARS:
        first = first[:COMMAND_CHARS - 1].rsplit(" ", 1)[0].rstrip() + " …"
    return f"`{first.replace('`', '')}`" if first else ""


def app_name(args: Mapping[str, Any], site: str | None, installed: Iterable[tuple[str, str]],
             running: Callable[[], Iterable[tuple[str, str, int]]]) -> str | None:
    """A Mac tool's app by the name FERB knows it by. The gateway's resolved bundle id (`site`) or the worker's
    argument must match the Mac's apps exactly: a bundle id or a name in the device profile (`installed`: (bundle id,
    name)), or a bundle id or `pid:` among the running apps (`running()`: (bundle id, name, pid), read only when
    needed). "an app" when nothing matches; None when the call names no app."""
    given = [str(v).strip() for v in (site, args.get("app"), args.get("bundle_id")) if isinstance(v, str) and v.strip()]
    if not given:
        return None
    known = {key.casefold(): name for bundle, name in installed for key in (bundle, name) if key and name}
    for value in given:
        if value.casefold() in known:
            return known[value.casefold()]
    for bundle, name, pid in running():
        if name and any(value.casefold() == bundle.casefold() or value == f"pid:{pid}" for value in given):
            return name
    return AN_APP


def _target(family: str, args: Mapping[str, Any], kind: str, site: str | None, app: str | None) -> str:
    """The call's target as the owner knows it: a host for a page, an app's name as resolved, a file's name; else
    "". A site a non-browser tool reports is not shown: only a browser tool's is a host FERB derived."""
    path = args.get("path")
    file = _plain(PurePosixPath(path.rstrip("/") or "/").name or path) if isinstance(path, str) and path.strip() else ""
    if file and (kind in FILE_KINDS or family == "fs"):
        return file
    if family in BROWSERS:
        return _plain(site or _host(args.get("url")))
    if family == "mac":
        return _plain(app) if app else (AN_APP if args.get("app") or args.get("bundle_id") else "")
    return file or _plain(_host(args.get("url")))


def _host(url: Any) -> str:
    parsed = urls.parse(url) if isinstance(url, str) and url else None
    return parsed.host if parsed is not None else ""


def _plain(value: Any) -> str:
    """One line with no control characters: a target name can't move the cursor or add lines on the owner's screen."""
    return " ".join(printable(value).split()) if isinstance(value, str) else ""
