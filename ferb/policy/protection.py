"""Owns FERB's protection lists (S2, SQ3 revised, SQ5, D27, D31): where FERB's shell may never read or write, what
code always protects (FERB's own state), and which rules the escape hatch may lift. Must not interpret commands or
judge intent: these are paths and ids (rule 5), and nothing here decides what a command means (rule 6).

The owner edits [protection] in config.toml (written once with these defaults). Code always adds FERB's own home
except its workspace, and the workspace's upload staging (a copy the browser is about to read): no entry can remove
them and no escape hatch lifts them (rule 10). A path nobody may read is a path nobody may write.

FERB's own `fs` tools never touch FERB's own state. They also stay out of the never-read stores while
`protection.fs_reads_never_read` is false (OPEN QUESTION 6, built as recommended: until F5b's redaction is measured, a
secret in a credential store would otherwise reach a model). That refusal is never lifted by the escape hatch, which
is for FERB's shell: a secret a task needs comes through the sealed secret path (D8, D12, D28)."""

import os
import unicodedata
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable

from ferb.act.fs.files import real_path
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
FERB_STATE, NETWORK, ADMIN = "ferb_state", "network", "admin"
WORKSPACE, UPLOADS = "workspace", "uploads"
# Kinds that take a path away with everything inside it: a folder holding FERB's state is FERB's state for them.
_REMOVES = frozenset({Kind.TRASH, Kind.DELETE_PERMANENT})


@dataclass(frozen=True)
class Entry:
    id: str          # read:~/.ssh | write:~/.zshrc
    path: Path       # resolved
    shown: str       # as the owner wrote it, the home as ~


def _real(path: Path) -> Path:
    """The path as the OS reaches it (files.real_path: links followed first, then `..`), so a store reached through a
    link is still the store; its textual form when it can't be resolved (a link loop, a folder macOS won't look into),
    which keeps the rule rather than dropping it."""
    path = path if path.is_absolute() else Path.cwd() / path
    try:
        return real_path(str(path), os.sep)
    except (OSError, RuntimeError, ValueError):
        return Path(os.path.normpath(path))


def _absolute(home: Path, entry: str) -> Path:
    """An entry as a path: ~ is the owner's home, and a relative entry is the home's."""
    if entry == "~" or entry.startswith("~/"):
        return home / entry[2:]
    return Path(entry) if Path(entry).is_absolute() else home / entry


def _shown(home: Path, path: Path) -> str:
    """The rule id's path: the entry as the owner writes it, the home as ~ (textual, never resolved)."""
    p, h = Path(os.path.normpath(path)), Path(os.path.normpath(home))
    if p == h:
        return "~"
    return "~/" + str(p.relative_to(h)) if p.is_relative_to(h) else str(p)


def _key(path: Path) -> tuple[str, ...]:
    """A path as a Mac volume compares it: APFS ignores case and Unicode normalization by default, so ~/.FERB/run is
    ~/.ferb/run there. On a case-sensitive volume this can only close more, never less."""
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
    """A [protection] list as written: one string would otherwise be read letter by letter (a "~" entry closes the
    whole home), so anything but a list of non-empty strings is refused, never guessed."""
    items = None if isinstance(value, (str, bytes)) or not isinstance(value, Iterable) else list(value)
    if items is None or not all(isinstance(v, str) and v.strip() for v in items):
        raise ValueError(f"protection.{key} in config.toml must be a list of {what}, each in quotes")
    return items


class Protection:
    def __init__(self, home: Path, ferb_home: Path, never_read: Iterable[str], never_write: Iterable[str],
                 terminal_apps: Iterable[str], fs_reads_never_read: bool = False) -> None:
        if not isinstance(fs_reads_never_read, bool):
            raise ValueError("protection.fs_reads_never_read in config.toml must be true or false")
        self.home = home
        self.ferb_home = _real(ferb_home)
        self.workspace = _real(self.ferb_home / WORKSPACE)
        self.uploads = _real(self.workspace / UPLOADS)
        self.reads = self._entries("read", _strings(never_read, "never_read", "paths"))
        self.writes = self._entries("write", _strings(never_write, "never_write", "paths"))
        self.terminal_apps = frozenset(a.lower() for a in _strings(terminal_apps, "terminal_apps", "app bundle ids"))
        self.fs_reads_never_read = fs_reads_never_read

    def _entries(self, prefix: str, items: list[str]) -> list[Entry]:
        out: dict[str, Entry] = {}
        for item in items:                               # one rule per id: the first spelling of an entry is kept
            raw = _absolute(self.home, item)
            shown = _shown(self.home, raw)
            out.setdefault(f"{prefix}:{shown}", Entry(f"{prefix}:{shown}", _real(raw), shown))
        return list(out.values())

    def _check(self, path: Path) -> Path:
        """A checked path, resolved as the OS reaches it; a relative one is the workspace's (as FERB's fs tools)."""
        return _real(path if path.is_absolute() else self.workspace / path)

    def _state(self, real: Path) -> bool:
        return (_under(real, self.ferb_home) and not _under(real, self.workspace)) or _under(real, self.uploads)

    def ferb_state(self, path: Path) -> bool:
        """FERB's own state: its home minus the workspace, and the upload staging inside the workspace."""
        return self._state(self._check(path))

    def read_denied(self, path: Path) -> str | None:
        real = self._check(path)
        if self._state(real):
            return FERB_STATE
        return next((e.id for e in self.reads if _under(real, e.path)), None)

    def write_denied(self, path: Path) -> str | None:
        denied = self.read_denied(path)
        if denied:
            return denied
        real = self._check(path)
        return next((e.id for e in self.writes if _under(real, e.path)), None)

    def fs_denied(self, real: Path, kind: str) -> str | None:
        """FERB's own fs tools (any kind): FERB's own state is closed to them (rule 10), and so is a folder holding it
        for a call that takes the folder away (trash). The never-read stores are closed to them too while
        fs_reads_never_read is false (OPEN QUESTION 6), and readable, labelled private, when the owner sets it true
        (SQ3 revised). Never-write entries don't bind them: changes there go through them, with S1 copies (SQ5)."""
        p = self._check(real)
        if self._state(p) or (kind in _REMOVES and (_under(self.ferb_home, p) or _under(self.uploads, p))):
            return FERB_STATE
        if self.fs_reads_never_read:
            return None
        return next((e.id for e in self.reads if _under(p, e.path)), None)

    def liftable(self, rule_id: str) -> bool:
        return rule_id == ADMIN or any(e.id == rule_id for e in (*self.reads, *self.writes))

    def entry(self, rule_id: str) -> Entry | None:
        return next((e for e in (*self.reads, *self.writes) if e.id == rule_id), None)

    def read_entries(self) -> list[tuple[Path, bool]]:
        """Resolved (path, is_dir) pairs of the never-read list, for the shell profile (Task 2)."""
        return [(e.path, _folder_or_missing(e.path)) for e in self.reads]

    def write_entries(self) -> list[tuple[Path, bool]]:
        """Resolved (path, is_dir) pairs of the never-write list, for the shell profile (Task 2)."""
        return [(e.path, _folder_or_missing(e.path)) for e in self.writes]

    def rules(self) -> list[dict[str, Any]]:
        out: list[dict[str, Any]] = [
            {"id": FERB_STATE, "kind": "read and write", "path": "FERB's own files (~/.ferb except its workspace, and "
             "the workspace's uploads)", "liftable": False},
            {"id": NETWORK, "kind": "network", "path": "every connection goes through FERB's proxy; nothing else "
             "leaves", "liftable": False},
            {"id": ADMIN, "kind": "admin", "path": "a command that needs your Mac password (sudo)", "liftable": True}]
        out += [{"id": e.id, "kind": "read", "path": e.shown, "liftable": True} for e in self.reads]
        out += [{"id": e.id, "kind": "write", "path": e.shown, "liftable": True} for e in self.writes]
        return out
