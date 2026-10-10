"""Owns FERB's protection lists (S2, SQ3 revised, SQ5, D27, D31): where FERB's shell may never read or write, what
code always protects (FERB's own state), and which rules the escape hatch may lift. Must not interpret commands or
judge intent: these are paths and ids (rule 5), and nothing here decides what a command means (rule 6).

The owner edits [protection] in config.toml (written once with these defaults). Code always adds FERB's own home
except its workspace, and the workspace's upload staging (a copy the browser is about to read): no entry can remove
them and no escape hatch lifts them (rule 10). A path nobody may read is a path nobody may write.

FERB's own tools that name files (`fs.*`, uploads, recordings: each declares `file_paths`) never touch FERB's own
state. They also stay out of the never-read stores while `protection.fs_reads_never_read` is false (OPEN QUESTION 6,
built as recommended: until F5b's redaction is measured, a secret in a credential store would otherwise reach a model
or a site). That refusal is never lifted by the escape hatch, which is for FERB's shell: a secret a task needs comes
through the sealed secret path (D8, D12, D28), and a file from a store is the owner's to send.

A path is checked as typed and as the OS reaches it now (links first, then `..`), against each entry as written and as
it resolved when FERB started: a rule matching any of them applies. FERB's workspace is the literal folder in FERB's
home: a link planted there never moves the exception (and the daemon won't start with one)."""

import os
import unicodedata
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Iterable

from ferb.act.fs.files import real_path
from ferb.core.errors import FerbError
from ferb.policy.rules import Kind

# SQ3 revised: the sensitive stores FERB's shell never reads. ~ is the owner's home.
_PLAN_NEVER_READ = (
    "~/Library/Keychains", "/Library/Keychains",
    "~/Library/Messages", "~/Library/Mail", "~/Library/Safari", "~/Library/Cookies",
    "~/Library/Group Containers/group.com.apple.usernoted",                       # the notification database
    "~/Library/Application Support/Google/Chrome", "~/Library/Application Support/BraveSoftware",
    "~/Library/Application Support/Chromium", "~/Library/Application Support/Microsoft Edge",
    "~/Library/Application Support/Arc", "~/Library/Application Support/Vivaldi",
    "~/Library/Application Support/Firefox",
    "~/Library/Application Support/1Password", "~/Library/Group Containers/2BUA8C4S2C.com.1password",
    "~/Library/Application Support/Bitwarden",
    "~/.ssh", "~/.aws", "~/.config/gh", "~/.netrc", "~/.npmrc", "~/.docker",
)
# OPEN QUESTION 2 (asked 2026-10-09), built as recommended: more credential stores of the same class. If the owner
# says no, delete this tuple (a config.toml already written keeps them until the owner edits [protection]).
EXTRA_NEVER_READ = (
    "~/.gnupg", "~/.kube", "~/.config/gcloud", "~/.azure", "~/.git-credentials", "~/.pypirc",
    "~/.cargo/credentials.toml",
)
DEFAULT_NEVER_READ = _PLAN_NEVER_READ + EXTRA_NEVER_READ
# SQ5: where FERB's shell never writes (changes there go through FERB's fs tools, with S1 copies).
DEFAULT_NEVER_WRITE = (
    "~/.ssh", "~/Library/Keychains", "~/Library/LaunchAgents", "/Library/LaunchAgents", "/Library/LaunchDaemons",
    "~/.zshrc", "~/.zprofile", "~/.zshenv", "~/.zlogin", "~/.bash_profile", "~/.bashrc", "~/.profile",
    "/System", "/Library", "/Applications",
)
# D31: FERB never types into terminal apps (bundle ids; the owner may edit the list).
DEFAULT_TERMINAL_APPS = ("com.apple.Terminal", "com.googlecode.iterm2", "dev.warp.Warp-Stable",
                         "com.mitchellh.ghostty", "org.alacritty", "net.kovidgoyal.kitty", "com.github.wez.wezterm",
                         "co.zeit.hyper")
FERB_STATE, NETWORK, ADMIN, TRUSTD = "ferb_state", "network", "admin", "trustd"
WORKSPACE, UPLOADS = "workspace", "uploads"
READ, WRITE, REMOVE = "read", "write", "remove"         # FileAccess (ferb/core/types.py): what a call does to a file
# A call that takes a path away with everything inside it: a folder holding protected content is protected for it.
_REMOVES = frozenset({REMOVE, Kind.TRASH, Kind.DELETE_PERMANENT})


class BadProtection(FerbError, ValueError):
    """[protection] or FERB's workspace isn't as FERB can enforce it: FERB doesn't start, and says why."""


@dataclass(frozen=True)
class Entry:
    id: str          # read:~/.ssh | write:~/.zshrc
    path: Path       # resolved when FERB started
    shown: str       # as the owner wrote it, the home as ~
    typed: Path      # as written, absolute, `..` folded: still matched if the entry later becomes a link


@dataclass(frozen=True)
class LocalPort:
    """An owner-approved TCP port and the resolved executable its listener must run."""
    port: int
    executable: Path


def _local_ports(value: Any) -> tuple[LocalPort, ...]:
    error = ("protection.local_ports in config.toml must be a list of {port = <TCP port>, "
             "executable = <absolute executable path>} entries, with one executable per port")
    if not isinstance(value, (list, tuple)):
        raise BadProtection(error)
    ports: dict[int, LocalPort] = {}
    for item in value:
        if not isinstance(item, dict) or set(item) != {"port", "executable"}:
            raise BadProtection(error)
        port, executable = item["port"], item["executable"]
        if (not isinstance(port, int) or isinstance(port, bool) or not 1 <= port <= 65535
                or not isinstance(executable, str) or "\x00" in executable or not Path(executable).is_absolute()):
            raise BadProtection(error)
        pin = LocalPort(port, Path(os.path.realpath(executable)))
        if port in ports and ports[port] != pin:
            raise BadProtection(error)
        ports[port] = pin
    return tuple(ports.values())


def _absolute(path: Path) -> Path:
    return path if path.is_absolute() else Path.cwd() / path


def _typed(path: Path) -> Path:
    """A path as typed: absolute, `.` and `..` folded as text, no link followed."""
    return Path(os.path.normpath(_absolute(path)))


def _real(path: Path) -> Path:
    """The path as the OS reaches it (files.real_path: links followed first, then `..`), so a store reached through a
    link is still the store; its typed form when it can't be resolved (a link loop, a folder macOS won't look into),
    which keeps the rule rather than dropping it."""
    try:
        return real_path(str(_absolute(path)), os.sep)
    except (OSError, RuntimeError, ValueError):
        return _typed(path)


def _forms(path: Path) -> tuple[Path, ...]:
    return tuple(dict.fromkeys((_typed(path), _real(path))))


def _entry_path(home: Path, entry: str) -> Path:
    """An entry as a path: ~ is the owner's home, and a relative entry is the home's."""
    if entry == "~" or entry.startswith("~/"):
        return home / entry[2:]
    return Path(entry) if Path(entry).is_absolute() else home / entry


def _shown(home: Path, path: Path) -> str:
    """The rule id's path: the entry as the owner writes it, the home as ~ (textual, never resolved)."""
    p, h = _typed(path), _typed(home)
    if p == h:
        return "~"
    return "~/" + str(p.relative_to(h)) if p.is_relative_to(h) else str(p)


def _key(path: Path) -> tuple[str, ...]:
    """A path as a Mac volume compares it: APFS ignores case and Unicode normalization by default, so ~/.FERB/run is
    ~/.ferb/run there. On a case-sensitive volume, folding widens every rule, and the workspace exception also covers
    a case-variant sibling of the workspace (~/.ferb/WORKSPACE): a folder FERB never makes, holding no FERB state;
    upload staging stays closed in every spelling."""
    return tuple(unicodedata.normalize("NFC", part).casefold() for part in path.parts)


def _under(path: Path, root: Path) -> bool:
    p, r = _key(path), _key(root)
    return p[:len(r)] == r


def _folder_or_missing(path: Path) -> bool:
    """For the profile builder: False only for an existing non-folder (a file). A missing entry counts as a folder,
    so whatever is created there later stays covered."""
    try:
        return path.is_dir() or not path.exists()
    except OSError:
        return True


def _strings(value: Any, key: str, what: str) -> list[str]:
    """A [protection] list as written: only a TOML list of non-empty strings. A single string would be read letter by
    letter (a "~" entry closes the whole home) and an inline table as its keys, so anything else is refused, never
    guessed."""
    items = list(value) if isinstance(value, (list, tuple)) else None
    if items is None or not all(isinstance(v, str) and v.strip() for v in items):
        raise BadProtection(f"protection.{key} in config.toml must be a list of {what}, each in quotes")
    return items


def workspace_problem(ferb_home: Path) -> str | None:
    """Why FERB must not start with this workspace, else None: it must be a plain folder directly in FERB's home (or
    not exist yet). A link there could make FERB's own files look like the workspace to anything that follows it."""
    ws = ferb_home / WORKSPACE
    if ws.is_symlink():
        return (f"FERB's workspace ({ws}) is a link to {os.readlink(ws)}, not a folder. A link there could open FERB's "
                f"own files to its tools, so FERB won't start until you move the link away (FERB then makes a fresh "
                f"workspace).")
    if ws.exists() and (not ws.is_dir() or _real(ws) != _real(ferb_home) / WORKSPACE):
        return (f"FERB's workspace ({ws}) is not a plain folder in FERB's home, so FERB won't start until you move it "
                f"away (FERB then makes a fresh workspace).")
    return None


def load(get: Callable[[str, str, Any], Any], home: Path, ferb_home: Path) -> "Protection":
    """The daemon's lists from config.toml's [protection] (`get` is Config.get), after checking the workspace. Raises
    BadProtection with the reason, which the daemon shows by not starting and `ferb doctor` shows as a failed check."""
    problem = workspace_problem(ferb_home)
    if problem:
        raise BadProtection(problem)
    return Protection(home, ferb_home, get("protection", "never_read", DEFAULT_NEVER_READ),
                      get("protection", "never_write", DEFAULT_NEVER_WRITE),
                      get("protection", "terminal_apps", DEFAULT_TERMINAL_APPS),
                      get("protection", "fs_reads_never_read", False),
                      local_ports=get("protection", "local_ports", []))


class Protection:
    def __init__(self, home: Path, ferb_home: Path, never_read: Iterable[str], never_write: Iterable[str],
                 terminal_apps: Iterable[str], fs_reads_never_read: bool = False, *,
                 local_ports: Any = ()) -> None:
        if not isinstance(fs_reads_never_read, bool):
            raise BadProtection("protection.fs_reads_never_read in config.toml must be true or false")
        self.home = home
        self.ferb_home = _real(ferb_home)
        self.workspace = self.ferb_home / WORKSPACE          # literal: never where a link there points
        self.uploads = self.workspace / UPLOADS
        typed = _typed(ferb_home)
        # FERB's own state as (home, workspace, uploads), resolved and as typed
        self._state_roots = tuple(dict.fromkeys(((self.ferb_home, self.workspace, self.uploads),
                                                 (typed, typed / WORKSPACE, typed / WORKSPACE / UPLOADS))))
        self.reads = self._entries("read", _strings(never_read, "never_read", "paths"))
        self.writes = self._entries("write", _strings(never_write, "never_write", "paths"))
        self.terminal_apps = frozenset(a.lower() for a in _strings(terminal_apps, "terminal_apps", "app bundle ids"))
        self.fs_reads_never_read = fs_reads_never_read
        self.local_ports = _local_ports(local_ports)

    def _entries(self, prefix: str, items: list[str]) -> list[Entry]:
        out: dict[str, Entry] = {}
        for item in items:                               # one rule per id: the first spelling of an entry is kept
            raw = _entry_path(self.home, item)
            shown = _shown(self.home, raw)
            out.setdefault(f"{prefix}:{shown}", Entry(f"{prefix}:{shown}", _real(raw), shown, _typed(raw)))
        return list(out.values())

    def _forms(self, path: Path) -> tuple[Path, ...]:
        """A checked path as typed and as resolved now; a relative one is the workspace's (as FERB's fs tools)."""
        return _forms(path if path.is_absolute() else self.workspace / path)

    def _state(self, forms: tuple[Path, ...], removes: bool = False) -> bool:
        """FERB's own state: its home minus the workspace, and the upload staging inside the workspace; for a call
        that removes, also a folder holding any of it."""
        return any((_under(p, home) and not _under(p, ws)) or _under(p, up)
                   or (removes and (_under(home, p) or _under(up, p)))
                   for p in forms for home, ws, up in self._state_roots)

    @staticmethod
    def _hit(entries: list[Entry], forms: tuple[Path, ...], removes: bool = False) -> str | None:
        """The first entry a path is inside (as typed or resolved, against the entry as written or resolved); for a
        call that removes, also an entry inside the path."""
        return next((e.id for e in entries for p in forms for root in (e.path, e.typed)
                     if _under(p, root) or (removes and _under(root, p))), None)

    def ferb_state(self, path: Path) -> bool:
        return self._state(self._forms(path))

    def read_denied(self, path: Path) -> str | None:
        forms = self._forms(path)
        return FERB_STATE if self._state(forms) else self._hit(self.reads, forms)

    def write_denied(self, path: Path) -> str | None:
        return self.read_denied(path) or self._hit(self.writes, self._forms(path))

    def fs_denied(self, path: Path, access: str) -> str | None:
        """FERB's own tools that name files (`access`: read, write, remove; a trash or permanent-delete kind counts as
        remove). FERB's own state is closed to them (rule 10), and so is a folder holding it for a call that takes the
        folder away. The never-read stores are closed to them too while fs_reads_never_read is false (OPEN QUESTION 6),
        including a folder that holds one for a call that removes (trashing ~/.config would hand over ~/.config/gh);
        readable, labelled private, when the owner sets it true (SQ3 revised). Never-write entries don't bind them:
        changes there go through them, with S1 copies (SQ5)."""
        forms, removes = self._forms(path), access in _REMOVES
        if self._state(forms, removes):
            return FERB_STATE
        return None if self.fs_reads_never_read else self._hit(self.reads, forms, removes)

    def liftable(self, rule_id: str) -> bool:
        return rule_id in (ADMIN, TRUSTD) or any(e.id == rule_id for e in (*self.reads, *self.writes))

    def entry(self, rule_id: str) -> Entry | None:
        return next((e for e in (*self.reads, *self.writes) if e.id == rule_id), None)

    def read_entries(self) -> list[tuple[Path, bool]]:
        """(path, is_dir) pairs of the never-read list, resolved and as written, for the shell profile (Task 2)."""
        return [(p, _folder_or_missing(p)) for e in self.reads for p in dict.fromkeys((e.path, e.typed))]

    def write_entries(self) -> list[tuple[Path, bool]]:
        """(path, is_dir) pairs of the never-write list, resolved and as written, for the shell profile (Task 2)."""
        return [(p, _folder_or_missing(p)) for e in self.writes for p in dict.fromkeys((e.path, e.typed))]

    def guarded_parents(self) -> list[Path]:
        """Every folder above a listed entry and above FERB's upload staging (so the workspace and FERB's home too),
        for the shell profile's `(deny file-write* (literal <folder>))`: moving or deleting such a folder would carry
        the protected content to a path no rule names. Files can still be made inside them. The root is left out."""
        roots = [p for e in (*self.reads, *self.writes) for p in (e.path, e.typed)]
        roots += [up for _, _, up in self._state_roots]
        return sorted({parent for r in roots for parent in r.parents if parent != parent.parent})

    def rules(self) -> list[dict[str, Any]]:
        out: list[dict[str, Any]] = [
            {"id": FERB_STATE, "kind": "read and write", "path": "FERB's own files (~/.ferb except its workspace, and "
             "the workspace's uploads)", "liftable": False},
            {"id": NETWORK, "kind": "network", "path": "no direct connection off this Mac, and on it only to "
             "servers started inside FERB's shell, plus owner-approved executable-pinned local ports through "
             "FERB's proxy; the proxy records each host and is the way out "
             "(shell.rules says whether it is running)", "liftable": False},
            {"id": ADMIN, "kind": "admin", "path": "a command that needs your Mac password (sudo)", "liftable": True},
            {"id": TRUSTD, "kind": "service", "path": "macOS certificate trust evaluation (trustd): enabling it "
             "lets the certificate service fetch certificate URLs outside FERB's proxy; Go TLS may need it",
             "liftable": True}]
        out += [{"id": e.id, "kind": "read", "path": e.shown, "liftable": True} for e in self.reads]
        out += [{"id": e.id, "kind": "write", "path": e.shown, "liftable": True} for e in self.writes]
        return out
