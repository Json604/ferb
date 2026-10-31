"""Owns FERB's protection lists (S2, SQ3 revised, SQ5, D27, D31): where FERB's shell may never read or write, what
code always protects (FERB's own state), and which rules the escape hatch may lift. Must not interpret commands or
judge intent: these are paths and ids (rule 5), and nothing here decides what a command means (rule 6).

The owner edits [protection] in config.toml (written once with these defaults). Code always adds FERB's own home
except its workspace, and the workspace's upload staging (a copy the browser is about to read): no entry can remove
them and no escape hatch lifts them (rule 10). A path nobody may read is a path nobody may write.

FERB's own tools that name files (`fs.*`, uploads, recordings: each declares `file_paths`) never touch FERB's own
state, and follow the never-read and never-write lists like the shell (gateway decision 2, 2026-10-30): a never-read
store is closed to their reads, writes and removals, a never-write place to their writes and removals. A task that
needs one asks the owner (the gateway): a read once per protected folder per task, a change per action (hands decision
1). The old `protection.fs_reads_never_read` switch is gone; an old config.toml that still has it is read harmlessly.

FERB's own service definitions (its launchd LaunchAgents, `SERVICE_LABELS`, and the background service's definition
inside FERB.app, `BUNDLE_SERVICE`, which the app registers with macOS) are FERB's own like its state: never written or
removed by its tools or its shell, and no lift reopens them (rule 10). Reading them stays open.

FERB.app (L27), when FERB runs from one, holds the daemon's own code, its interpreter and its secret helper: it is
closed to writes and removal by FERB's tools and its shell, wherever the app sits, and no escape hatch lifts that
(rule 10). A daemon run from ~/.ferb/venv or a checkout closes the installed FERB.app the same way (its signature
decides which process is FERB.app, and its service definition is FERB's). Reading it stays open.

A path is checked as typed, as the OS reaches it now (links first, then `..`) and as the kernel names the file it
reaches (F_GETPATH on its deepest existing folder or regular file, opened for its name only: the Data volume's
/System/Volumes/Data/… and volfs's /.vol/<dev>/<ino> spellings become FERB's own), against each entry as written, as
it resolved when FERB started and in its firmlinked spelling: a rule matching any of them applies. Entries are never
opened (another app's data folder can raise a macOS prompt); FERB's own home and FERB.app are. A volfs path FERB can't
map is closed. FERB's workspace is the literal folder in FERB's
home: a link planted there never moves the exception (and the daemon won't start with one)."""

import os
import stat
import unicodedata
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Iterable

from ferb import paths
from ferb.act.fs.files import opened_path, real_path
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
# SQ5: where FERB's shell and file tools never write (gateway decision 2: the owner can lift it for one change).
DEFAULT_NEVER_WRITE = (
    "~/.ssh", "~/Library/Keychains", "~/Library/LaunchAgents", "/Library/LaunchAgents", "/Library/LaunchDaemons",
    "~/.zshrc", "~/.zprofile", "~/.zshenv", "~/.zlogin", "~/.bash_profile", "~/.bashrc", "~/.profile",
    "/System", "/Library", "/Applications",
)
# D31: terminal apps (bundle ids). FERB types into a terminal only after the owner's Yes to that exact input, every
# time (policy/terminal.py, which always counts these and adds the owner's [protection] terminal_apps); mac.applescript
# and mac.secret never run in one. L17: typed text is never captured in these apps either (sense/watchers/text_input.py
# uses this constant).
DEFAULT_TERMINAL_APPS = ("com.apple.Terminal", "com.googlecode.iterm2", "dev.warp.Warp-Stable", "dev.warp.Warp-Preview",
                         "com.mitchellh.ghostty", "org.alacritty", "net.kovidgoyal.kitty", "com.github.wez.wezterm",
                         "co.zeit.hyper", "org.tabby", "com.termius-dmg.mac", "com.termius.mac",
                         "com.raphaelamorim.rio", "org.contourterminal.contour")
FERB_STATE, NETWORK, ADMIN, TRUSTD = "ferb_state", "network", "admin", "trustd"
FERB_APP = "ferb_app"                                   # the FERB.app the daemon runs from: never written, never lifted
FERB_SERVICE = "ferb_service"                           # FERB's own launchd definitions: never written, never lifted
# FERB's LaunchAgents (ferb/device/install.py LABEL and APP_LOGIN_LABEL; a test keeps them equal), as plists in
# ~/Library/LaunchAgents: the daemon's own (a checkout's, or one from before FERB.app registered its service) and
# FERB.app's login fallback
SERVICE_LABELS = ("com.ferb.daemon", "com.ferb.FERB.login")
# The background service's definition inside FERB.app (ferb/device/install.py BUNDLE_SERVICE; a test keeps them equal)
BUNDLE_SERVICE = Path("Contents/Library/LaunchAgents/com.ferb.daemon.plist")
NEVER_LIFTED = frozenset({FERB_STATE, NETWORK, FERB_APP, FERB_SERVICE})
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
    kernel: Path | None = None   # its firmlinked spelling when written through the Data volume (never opened)


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


_DATA_VOLUME = Path("/System/Volumes/Data")      # every firmlinked folder (/Users, /Applications, /private…) is here too
_VOLFS = Path("/.vol")                            # /.vol/<device>/<inode>/…: any file by its identity
_O_EVTONLY = getattr(os, "O_EVTONLY", 0x8000)     # open for a name only: no read access needed, no data touched


def _opened_name(path: Path, folder: bool, seen: os.stat_result) -> Path | None:
    """F_GETPATH of a folder or regular file, opened for its name only (no data read) and never through a link
    (O_NOFOLLOW): a file swapped for a link to a device since FERB looked at it is never opened. None if it can't be
    opened, or if what opened isn't what FERB looked at (`seen`)."""
    flags = _O_EVTONLY | os.O_NONBLOCK | os.O_CLOEXEC | os.O_NOFOLLOW | (os.O_DIRECTORY if folder else 0)
    try:
        fd = os.open(path, flags)
    except (OSError, ValueError):
        return None
    try:
        st = os.fstat(fd)
        return opened_path(fd) if (st.st_dev, st.st_ino) == (seen.st_dev, seen.st_ino) else None
    except OSError:
        return None
    finally:
        os.close(fd)


def _kernel(path: Path) -> Path | None:
    """The path as the kernel names what it reaches, so another spelling of the same file is the spelling FERB's rules
    use: every link followed by name first (realpath), then F_GETPATH on the deepest part that exists, then the rest.
    Only a folder or a regular file is ever opened (for its name only, never through a link); a device node, FIFO,
    socket, a file FERB can't open, or one swapped for a link meanwhile is named through its folder. None when nothing
    along it can be named."""
    tail: list[str] = []
    try:
        p = Path(os.path.realpath(_absolute(path)))
    except (OSError, ValueError):                     # ValueError: a NUL in the path, which no file can have
        return None
    while True:
        try:
            st = os.lstat(p)
        except (FileNotFoundError, NotADirectoryError, PermissionError):
            st = None
        except (OSError, ValueError):
            return None
        if st is not None and stat.S_ISDIR(st.st_mode):
            named = _opened_name(p, True, st)
        elif st is not None and stat.S_ISREG(st.st_mode):
            named = _opened_name(p, False, st)
        else:
            named = None
        if named is not None:
            return Path(os.path.normpath(named.joinpath(*reversed(tail))))
        if (st is not None and stat.S_ISDIR(st.st_mode)) or p == p.parent:
            return None                               # a folder that can't be opened, or nothing up to the root
        tail.append(p.name)
        p = p.parent


def _firmlinked(path: Path) -> Path | None:
    """The firmlinked spelling of a path written through the Data volume (/System/Volumes/Data/Users/… is /Users/…),
    found from the text alone: for entries in other apps' folders, which FERB never opens to learn their names."""
    typed = _typed(path)
    return Path("/") / typed.relative_to(_DATA_VOLUME) if typed.is_relative_to(_DATA_VOLUME) and typed != _DATA_VOLUME \
        else None


def _forms(path: Path) -> tuple[Path, ...]:
    typed = _typed(path)
    forms = [typed, _real(path)]
    kernel = _kernel(path)
    if kernel is not None:
        forms.append(kernel)
    firmlinked = _firmlinked(typed)
    if firmlinked is not None:
        forms.append(firmlinked)                      # the firmlinked spelling, even when missing
    return tuple(dict.fromkeys(forms))


def _unmapped(path: Path) -> bool:
    """A volfs path the kernel can't name (it stays a /.vol path): which file it means is unknown, so it is treated as
    FERB's own."""
    if not _typed(path).is_relative_to(_VOLFS):
        return False
    kernel = _kernel(path)
    return kernel is None or kernel.is_relative_to(_VOLFS)


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
    """The daemon's lists from config.toml's [protection] (`get` is Config.get), after checking the workspace, plus
    FERB.app: the one this code runs from, else the installed one (a daemon run from ~/.ferb/venv or a checkout: its
    signature is what FERB.app's own requests are checked against, and its service definition is FERB's, rule 6).
    Raises BadProtection with the reason, which the daemon shows by not starting and `ferb doctor` shows as a failed
    check."""
    problem = workspace_problem(ferb_home)
    if problem:
        raise BadProtection(problem)
    return Protection(home, ferb_home, get("protection", "never_read", DEFAULT_NEVER_READ),
                      get("protection", "never_write", DEFAULT_NEVER_WRITE),
                      get("protection", "terminal_apps", DEFAULT_TERMINAL_APPS),
                      local_ports=get("protection", "local_ports", []),
                      app_bundle=paths.app_bundle() or paths.installed_app())


class Protection:
    def __init__(self, home: Path, ferb_home: Path, never_read: Iterable[str], never_write: Iterable[str],
                 terminal_apps: Iterable[str], *, local_ports: Any = (), app_bundle: Path | None = None) -> None:
        self.home = home
        self.ferb_home = _real(ferb_home)
        self.workspace = self.ferb_home / WORKSPACE          # literal: never where a link there points
        self.uploads = self.workspace / UPLOADS
        typed = _typed(ferb_home)
        # FERB's own state as (home, workspace, uploads): resolved, as typed, and as the kernel names the home
        homes = [self.ferb_home, typed, *[k for k in (_kernel(ferb_home),) if k is not None]]
        self._state_roots = tuple(dict.fromkeys((h, h / WORKSPACE, h / WORKSPACE / UPLOADS) for h in homes))
        self.reads = self._entries("read", _strings(never_read, "never_read", "paths"))
        self.writes = self._entries("write", _strings(never_write, "never_write", "paths"))
        self.terminal_apps = frozenset(a.lower() for a in _strings(terminal_apps, "terminal_apps", "app bundle ids"))
        # FERB's own launchd definitions, as written and resolved (a link there is followed by the check)
        self.service_files = tuple(home / "Library" / "LaunchAgents" / f"{label}.plist" for label in SERVICE_LABELS) + (
            (app_bundle / BUNDLE_SERVICE,) if app_bundle is not None else ())
        # (never opened, like an entry: as written, resolved and firmlinked)
        self._service_roots = tuple(dict.fromkeys(f for s in self.service_files
                                                  for f in (_typed(s), _real(s), _firmlinked(_real(s)))
                                                  if f is not None))
        self.local_ports = _local_ports(local_ports)
        self.app_bundle = app_bundle
        self._app_roots = _forms(app_bundle) if app_bundle is not None else ()   # resolved and as typed

    def _entries(self, prefix: str, items: list[str]) -> list[Entry]:
        out: dict[str, Entry] = {}
        for item in items:                               # one rule per id: the first spelling of an entry is kept
            raw = _entry_path(self.home, item)
            shown = _shown(self.home, raw)
            out.setdefault(f"{prefix}:{shown}", Entry(f"{prefix}:{shown}", _real(raw), shown, _typed(raw),
                                                      _firmlinked(_real(raw)) or _firmlinked(raw)))
        return list(out.values())

    def _forms(self, path: Path) -> tuple[Path, ...]:
        """A checked path as typed, as resolved now and as the kernel names it; a relative one is the workspace's (as
        FERB's fs tools)."""
        return _forms(path if path.is_absolute() else self.workspace / path)

    def _state(self, forms: tuple[Path, ...], removes: bool = False) -> bool:
        """FERB's own state: its home minus the workspace, and the upload staging inside the workspace; for a call
        that removes, also a folder holding any of it. A volfs path FERB can't map counts as FERB's own."""
        if any(_unmapped(p) for p in forms):
            return True
        return any((_under(p, home) and not _under(p, ws)) or _under(p, up)
                   or (removes and (_under(home, p) or _under(up, p)))
                   for p in forms for home, ws, up in self._state_roots)

    def _app(self, forms: tuple[Path, ...], removes: bool = False) -> bool:
        """Inside FERB.app; for a call that removes, also a folder holding it."""
        return any(_under(p, app) or (removes and _under(app, p)) for p in forms for app in self._app_roots)

    @staticmethod
    def _hit(entries: list[Entry], forms: tuple[Path, ...], removes: bool = False) -> str | None:
        """The first entry a path is inside (as typed or resolved, against the entry as written or resolved); for a
        call that removes, also an entry inside the path."""
        return next((e.id for e in entries for p in forms for root in (e.path, e.typed, e.kernel) if root is not None
                     if _under(p, root) or (removes and _under(root, p))), None)

    def ferb_state(self, path: Path) -> bool:
        return self._state(self._forms(path))

    def read_denied(self, path: Path) -> str | None:
        forms = self._forms(path)
        return FERB_STATE if self._state(forms) else self._hit(self.reads, forms)

    def write_denied(self, path: Path) -> str | None:
        forms = self._forms(path)
        return self.read_denied(path) or (FERB_APP if self._app(forms) else self._hit(self.writes, forms))

    def _service(self, forms: tuple[Path, ...], removes: bool = False) -> bool:
        """One of FERB's own launchd definitions; for a call that removes, also a folder holding one."""
        return any(_under(p, s) or (removes and _under(s, p)) for p in forms for s in self._service_roots)

    def fs_rules(self, path: Path, access: str) -> list[str]:
        """Every rule that closes `path` to FERB's own tools that name files (`access`: read, write, remove, metadata;
        a trash or permanent-delete kind counts as remove), the never-lifted ones alone when one applies:
        - FERB's own state (rule 10), and for a call that takes a folder away, a folder holding it: [FERB_STATE];
        - FERB.app and FERB's launchd definitions, for any change: [FERB_APP] / [FERB_SERVICE];
        - else each never-read entry it is inside (for a removal, also one inside it: trashing ~/.config would hand
          over ~/.config/gh), for reads, writes and removals; and each never-write entry, for writes and removals.
        The entry rules are liftable by the owner (the gateway asks); metadata is never closed by them."""
        forms, removes = self._forms(path), access in _REMOVES
        if self._state(forms, removes):
            return [FERB_STATE]
        if access in (READ, "metadata"):
            return [] if access == "metadata" else self._hits(self.reads, forms)
        if self._service(forms):
            return [FERB_SERVICE]                   # a service definition itself, FERB.app's included (rule 10)
        if self._app(forms, removes):
            return [FERB_APP]                       # the daemon's own code (L27, rule 10): read, never changed
        if self._service(forms, removes):
            return [FERB_SERVICE]                   # a folder holding one
        return list(dict.fromkeys(self._hits(self.reads, forms, removes) + self._hits(self.writes, forms, removes)))

    def fs_denied(self, path: Path, access: str) -> str | None:
        """The first rule `fs_rules` names for the call, or None when nothing closes it."""
        rules = self.fs_rules(path, access)
        return rules[0] if rules else None

    @staticmethod
    def _hits(entries: list[Entry], forms: tuple[Path, ...], removes: bool = False) -> list[str]:
        return list(dict.fromkeys(e.id for e in entries for p in forms
                                  for root in (e.path, e.typed, e.kernel) if root is not None
                                  if _under(p, root) or (removes and _under(root, p))))

    def liftable(self, rule_id: str) -> bool:
        return rule_id not in NEVER_LIFTED and (rule_id in (ADMIN, TRUSTD)
                                                or any(e.id == rule_id for e in (*self.reads, *self.writes)))

    def entry(self, rule_id: str) -> Entry | None:
        return next((e for e in (*self.reads, *self.writes) if e.id == rule_id), None)

    def read_entries(self) -> list[tuple[Path, bool]]:
        """(path, is_dir) pairs of the never-read list, resolved and as written, for the shell profile (Task 2)."""
        return [(p, _folder_or_missing(p)) for e in self.reads for p in dict.fromkeys((e.path, e.typed))]

    def write_entries(self) -> list[tuple[Path, bool]]:
        """(path, is_dir) pairs of the never-write list, resolved and as written, for the shell profile (Task 2)."""
        return [(p, _folder_or_missing(p)) for e in self.writes for p in dict.fromkeys((e.path, e.typed))]

    def app_entries(self) -> list[Path]:
        """FERB.app, resolved and as typed, for the shell profile's last write deny (never lifted)."""
        return list(self._app_roots)

    def guarded_parents(self) -> list[Path]:
        """Every folder above a listed entry, above FERB's upload staging (so the workspace and FERB's home too) and
        above FERB.app, for the shell profile's `(deny file-write* (literal <folder>))`: moving or deleting such a
        folder would carry the protected content to a path no rule names. Files can still be made inside them. The
        root is left out."""
        roots = [p for e in (*self.reads, *self.writes) for p in (e.path, e.typed)]
        roots += [up for _, _, up in self._state_roots]
        roots += list(self._app_roots)
        roots += list(self._service_roots)
        return sorted({parent for r in roots for parent in r.parents if parent != parent.parent})

    def rules(self) -> list[dict[str, Any]]:
        out: list[dict[str, Any]] = [
            {"id": FERB_STATE, "kind": "read and write", "path": "FERB's own files (~/.ferb except its workspace, and "
             "the workspace's uploads)", "liftable": False},
            {"id": NETWORK, "kind": "network", "path": "no direct connection off this Mac, and on it only to "
             "servers started inside FERB's shell, plus owner-approved executable-pinned local ports through "
             "FERB's proxy; the proxy records each host and is the way out "
             "(shell.rules says whether it is running)", "liftable": False},
            *([{"id": FERB_APP, "kind": "write", "path": str(self.app_bundle), "liftable": False}]
              if self.app_bundle is not None else []),
            {"id": FERB_SERVICE, "kind": "write", "path": "FERB's own background-service definitions ("
             + ", ".join(_shown(self.home, f) for f in self.service_files) + ")", "liftable": False},
            {"id": ADMIN, "kind": "admin", "path": "a command that needs your Mac password (sudo)", "liftable": True},
            {"id": TRUSTD, "kind": "service", "path": "macOS certificate trust evaluation (trustd): enabling it "
             "lets the certificate service fetch certificate URLs outside FERB's proxy; Go TLS may need it",
             "liftable": True}]
        out += [{"id": e.id, "kind": "read", "path": e.shown, "liftable": True} for e in self.reads]
        out += [{"id": e.id, "kind": "write", "path": e.shown, "liftable": True} for e in self.writes]
        return out
