"""Owns the live progress line (L8): what a task is doing right now, in a few plain words, built by code from the tool
and its target. Must not use model text: the worker's `why`, typed text, message bodies, commands, queries and page
content never appear, so injected text can't speak through the line. Describes; never gates or judges a call.

The target is the page's host (parsed as the browser opens it), an app's name or a file's name; every other argument
is ignored. A tool not named here is described by the verb of its registry kind (W2)."""

from pathlib import PurePosixPath
from typing import Any, Mapping

from ferb.policy import urls
from ferb.policy.rules import Kind
from ferb.sanitize.terminal import printable

BROWSERS = ("browser", "ferb_browser")       # FERB's own browser and the owner's share their tools' verbs
WORKING = "working"

# Known tools: (with a target, without one). "{}" is the target.
TOOLS: dict[str, tuple[str, str]] = {
    "browser.open": ("opening {}", "opening a page"), "browser.navigate": ("opening {}", "opening a page"),
    "browser.read": ("reading {}", "reading a page"), "browser.snapshot": ("reading {}", "reading a page"),
    "browser.find": ("reading {}", "reading a page"), "browser.scroll": ("reading {}", "reading a page"),
    "browser.screenshot": ("looking at {}", "looking at a page"),
    "browser.wait": ("waiting for {}", "waiting for a page"),
    "mac.snapshot": ("looking at {}", "looking at an app"), "mac.screenshot": ("looking at {}", "looking at an app"),
    "mac.open": ("opening {}", "opening an app"),
    "fs.read": ("reading {}", "reading a file"), "fs.list": ("looking in {}", "looking in a folder"),
    "fs.write": ("writing {}", "writing a file"), "fs.edit": ("editing {}", "editing a file"),
    "fs.trash": ("moving {} to the Trash", "moving a file to the Trash"),
    "shell.run": ("running a command", "running a command"),
    "shell.start": ("starting a command", "starting a command"),
    "shell.wait": ("waiting for a command", "waiting for a command"),
    "time.sleep_until": ("waiting", "waiting"),
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
    "owner_question": ("asking you", "asking you"),
}


def describe(tool: str, args: Mapping[str, Any], *, kind: str = "", site: str | None = None) -> str:
    """The live line for one tool call. `kind`: the registry kind its describe() reported; `site`: the host the
    gateway derived for it (a tab's page), when the arguments name only a tab."""
    family, _, verb = tool.rpartition(".")
    name = "browser." + verb if family in BROWSERS else tool
    with_target, alone = TOOLS.get(name) or KINDS.get(kind) or (WORKING, WORKING)
    target = _target(family, args, site)
    return with_target.format(target) if target and "{}" in with_target else alone


def _target(family: str, args: Mapping[str, Any], site: str | None) -> str:
    """The call's target as the owner knows it: a host for a page, an app's name, a file's name; else ""."""
    host = site if family in BROWSERS and site else _host(args.get("url"))
    if family in BROWSERS:
        return _plain(host)
    if family == "mac":
        return _plain(args.get("app") or args.get("bundle_id"))
    path = args.get("path")
    if isinstance(path, str) and path.strip():
        return _plain(PurePosixPath(path.rstrip("/") or "/").name or path)
    return _plain(host or site)


def _host(url: Any) -> str:
    parsed = urls.parse(url) if isinstance(url, str) and url else None
    return parsed.host if parsed is not None else ""


def _plain(value: Any) -> str:
    """One line with no control characters: a target name can't move the cursor or add lines on the owner's screen."""
    return " ".join(printable(value).split()) if isinstance(value, str) else ""
