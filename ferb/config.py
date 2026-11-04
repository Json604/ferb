"""Own configuration parsing and persistence. Must not infer task intent or change owner values silently.

- Refuses a config (or ~/.ferb) that others can read or write: allowed origins and policy live here.
- Writes are atomic: temp file in the same folder (0600) -> fsync -> rename over the original.
- Reads are cached: `get` parses config.toml once and again only when the file (or its or ~/.ferb's owner or mode)
  changes, so an owner's edit while FERB runs still applies at the next read, and a loosened mode is still refused.
"""

import copy
import os
import stat
import tempfile
from pathlib import Path
from typing import Any

import tomlkit
from tomlkit.toml_document import TOMLDocument

from ferb.core.errors import FerbError


# [privacy] exclude_apps starts with password managers only; the owner may edit or empty it (an empty list is kept).
DEFAULT_EXCLUDE_APPS = ["com.1password.1password", "com.agilebits.onepassword7", "com.bitwarden.desktop",
                        "com.apple.Passwords", "com.apple.keychainaccess"]

# Download and tab limits FERB itself wrote into config.toml in F4 (ensure_privacy_defaults), before the owner chose
# them. The owner's decision of 2026-10-09 replaces them: a key still holding FERB's old value takes the decided one;
# any other value is the owner's and stays. Time-based clearing of downloads was removed, so its key goes. The daemon
# applies this once and journals it (`config.upgraded`), so a value the owner sets later is never replaced.
LIMITS_DECIDED = "download-limits-2026-10-09"
FERB_WRITTEN = {("browser", "download_max_mb"): (100, 512), ("browser", "download_task_max_files"): (20, 50),
                ("browser", "download_task_max_mb"): (500, 4096), ("browser", "download_total_mb"): (2048, 20480),
                ("browser", "task_max_tabs"): (20, 50)}
RETIRED = (("retention", "download_days"),)
# HAND-210 (owner, 2026-10-31) replaces 2026-10-09's download limits: no size cap per file (the key goes: an old
# config still setting it is never read), 300 files and 32 GB per task tree (50 files and 4 GB before), and every
# limit in decimal GB, as Finder shows it (ruling, step 12e: the MB keys were binary, the free-disk floor decimal). A
# key holding the 2026-10-09 value takes the new one; an owner's own MB value is kept, converted to GB.
DOWNLOADS_DECIDED = "download-limits-2026-10-31"
DOWNLOAD_FILES = (("browser", "download_task_max_files"), 50, 300)
DOWNLOAD_MB_TO_GB = {("browser", "download_task_max_mb"): ("download_task_max_gb", 4096),
                     ("browser", "download_total_mb"): ("download_total_gb", 20480)}
DOWNLOAD_RETIRED = (("browser", "download_max_mb"),)
# Reports go to the developer's relay (owner, 2026-11-01), not by email: the feedback email address (`[feedback]
# address`, read by no build since) goes; `[feedback] relay_url` is the new key.
FEEDBACK_DECIDED = "feedback-relay-2026-11-01"
FEEDBACK_RETIRED = (("feedback", "address"),)
# FERB never speaks (VOICE-409, owner 2026-11-04): the speech settings go. `[voice] requirement` (an older setup's
# record of FERB.app's signature for hearing) is not a speech setting and stays.
VOICE_DECIDED = "no-voice-2026-11-04"
VOICE_RETIRED = (("voice", "tts"), ("voice", "voice"), ("voice", "rate"))
# The owner's decisions applied to config.toml once each, in this order, each journaled (`config.upgraded`).
UPGRADES = (LIMITS_DECIDED, DOWNLOADS_DECIDED, FEEDBACK_DECIDED, VOICE_DECIDED)


class InsecureConfig(FerbError):
    pass


def _check_owner_only(path: Path) -> None:
    st = path.stat()
    if st.st_uid != os.getuid():
        raise InsecureConfig(f"{path} is not owned by this user")
    if stat.S_IMODE(st.st_mode) & 0o077:
        raise InsecureConfig(f"{path} has mode {oct(stat.S_IMODE(st.st_mode))}; others must have no access")


def write_private(home: Path, path: Path, text: str) -> None:
    """Write `text` to `path` (inside `home`) atomically and owner-only: a 0600 temp file in the same folder, fsync,
    rename over the original; `home` is made 0700."""
    home.mkdir(parents=True, exist_ok=True)
    os.chmod(home, 0o700)
    fd, tmp = tempfile.mkstemp(prefix=f".{path.stem}.", suffix=path.suffix, dir=home)   # created 0600
    try:
        with os.fdopen(fd, "w") as f:
            f.write(text)
            f.flush()
            os.fsync(f.fileno())
        os.chmod(tmp, 0o600)
        os.replace(tmp, path)                  # atomic on the same filesystem
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise


Stamp = tuple[int, ...]


class Config:
    def __init__(self, home: Path) -> None:
        self.home = home
        self.path = home / "config.toml"
        # (what the file and ~/.ferb looked like when parsed, the parsed values): `get` re-parses only on a change
        self._cached: tuple[Stamp, dict[str, Any]] | None = None

    def _stamp(self) -> Stamp | None:
        """The file's identity, size, change time and owner/mode, and ~/.ferb's owner/mode; None: no config file."""
        try:
            st = self.path.stat()
        except FileNotFoundError:
            return None
        home = self.home.stat()
        return (st.st_ino, st.st_size, st.st_mtime_ns, st.st_ctime_ns, st.st_uid, st.st_mode, home.st_uid,
                home.st_mode)

    def _remember(self, doc: TOMLDocument) -> None:
        stamp = self._stamp()
        self._cached = None if stamp is None else (stamp, doc.unwrap())

    def _values(self) -> dict[str, Any]:
        stamp = self._stamp()
        if stamp is None:
            return {}
        cached = self._cached
        if cached is not None and cached[0] == stamp:
            return cached[1]
        doc = self.load()                      # checks the owner and mode, then parses
        values = doc.unwrap()
        self._cached = (stamp, values)
        return values

    def load(self) -> TOMLDocument:
        """A freshly parsed document, for a caller that changes and saves it; reads go through `get`/`section`."""
        if not self.path.exists():
            return tomlkit.document()
        _check_owner_only(self.home)
        _check_owner_only(self.path)
        return tomlkit.parse(self.path.read_text())

    def save(self, doc: TOMLDocument) -> None:
        write_private(self.home, self.path, tomlkit.dumps(doc))
        self._remember(doc)

    def get(self, section: str, key: str, default: Any = None) -> Any:
        """A plain value (never a tomlkit item), copied: changing it never changes the cache."""
        tbl = self._values().get(section)
        if not isinstance(tbl, dict) or key not in tbl:
            return default
        value = tbl[key]
        return copy.deepcopy(value) if isinstance(value, (dict, list)) else value

    def section(self, section: str) -> dict[str, Any]:
        """One table's plain values, copied ({} when there is none)."""
        tbl = self._values().get(section)
        return copy.deepcopy(tbl) if isinstance(tbl, dict) else {}

    def set(self, section: str, key: str, value: Any) -> None:
        doc = self.load()
        if section not in doc:
            doc[section] = tomlkit.table()
        doc[section][key] = value              # type: ignore[index]
        self.save(doc)

    def remove(self, section: str, *keys: str) -> None:
        """Delete these keys of one table in one atomic write (nothing is written when none is there)."""
        doc = self.load()
        tbl = doc.get(section)
        present = [k for k in keys if tbl is not None and k in tbl]
        if not present:
            return
        for key in present:
            del tbl[key]                        # type: ignore[union-attr]
        self.save(doc)

    def upgrade_ferb_written(self) -> list[dict[str, Any]]:
        """The owner's limits decision (LIMITS_DECIDED), for the daemon to apply once: each FERB_WRITTEN key still
        holding FERB's old value takes the decided one; each RETIRED key goes. All or nothing: every change is made on
        one loaded document and saved in one atomic write (a failure leaves config.toml as it was). Returns what
        changed, for the daemon to journal after the write."""
        doc = self.load()
        changed: list[dict[str, Any]] = []
        for (section, key), (old, new) in FERB_WRITTEN.items():
            tbl = doc.get(section)
            if tbl is not None and tbl.get(key) == old:
                tbl[key] = new
                changed.append({"key": f"{section}.{key}", "from": old, "to": new})
        for section, key in RETIRED:
            tbl = doc.get(section)
            if tbl is not None and tbl.get(key) is not None:
                value = tbl[key]
                del tbl[key]
                changed.append({"key": f"{section}.{key}", "from": getattr(value, "unwrap", lambda: value)(),
                                "to": None})
        if changed:
            self.save(doc)
        return changed

    def add_defaults(self, section: str, key: str, entries: list[str]) -> list[str]:
        """Add to a list the owner keeps in config.toml each of `entries` it lacks, in one atomic write; the owner's own
        entries and order stay. Nothing is written when the list isn't there or isn't a list (FERB then reads it as it
        is, or refuses it). Returns what was added."""
        doc = self.load()
        tbl = doc.get(section)
        current = tbl.get(key) if tbl is not None else None
        if not isinstance(current, list):
            return []
        added = [e for e in entries if e not in current]
        if added:
            current.extend(added)
            self.save(doc)
        return added

    def upgrade(self, name: str) -> list[dict[str, Any]]:
        """One of UPGRADES, by name (the daemon applies each once and journals what it changed)."""
        if name == LIMITS_DECIDED:
            return self.upgrade_ferb_written()
        if name == DOWNLOADS_DECIDED:
            return self.upgrade_downloads()
        if name == FEEDBACK_DECIDED:
            return self.retire(FEEDBACK_RETIRED)
        if name == VOICE_DECIDED:
            return self.retire(VOICE_RETIRED)
        raise ValueError(f"no config upgrade {name!r}")

    def retire(self, keys: tuple[tuple[str, str], ...]) -> list[dict[str, Any]]:
        """Remove keys no build reads any more, in one atomic write; a table left empty goes too. Returns what changed
        (the old value is not journaled: a retired key may have held an address)."""
        doc = self.load()
        changed: list[dict[str, Any]] = []
        for section, key in keys:
            tbl = doc.get(section)
            if tbl is not None and key in tbl:
                del tbl[key]
                changed.append({"key": f"{section}.{key}", "to": None})
                if not tbl:
                    del doc[section]
        if changed:
            self.save(doc)
        return changed

    def upgrade_downloads(self) -> list[dict[str, Any]]:
        """HAND-210 (DOWNLOADS_DECIDED), all or nothing in one atomic write: the per-file key goes; the files
        limit still at 2026-10-09's 50 becomes 300; each MB key goes, and an owner's own value (anything but
        2026-10-09's) carries over to its GB key, converted (MiB to decimal GB). Returns what changed."""
        doc = self.load()
        changed: list[dict[str, Any]] = []
        tbl = doc.get("browser")
        if tbl is None:
            return changed
        (_, key), old, new = DOWNLOAD_FILES
        if tbl.get(key) == old:
            tbl[key] = new
            changed.append({"key": f"browser.{key}", "from": old, "to": new})
        for (_, key), (gb_key, decided) in DOWNLOAD_MB_TO_GB.items():
            value = tbl.get(key)
            if value is None:
                continue
            value = getattr(value, "unwrap", lambda: value)()
            del tbl[key]
            changed.append({"key": f"browser.{key}", "from": value, "to": None})
            if value != decided and isinstance(value, (int, float)) and not isinstance(value, bool) and value >= 0:
                gb = round(value * 1024 * 1024 / 1000 ** 3, 3)
                changed.append({"key": f"browser.{gb_key}", "from": tbl.get(gb_key), "to": gb})
                tbl[gb_key] = gb
        for _, key in DOWNLOAD_RETIRED:
            value = tbl.get(key)
            if value is not None:
                del tbl[key]
                changed.append({"key": f"browser.{key}", "from": getattr(value, "unwrap", lambda: value)(),
                                "to": None})
        if changed:
            self.save(doc)
        return changed

    def exclude_apps(self) -> frozenset[str]:
        return frozenset(self.get("privacy", "exclude_apps", DEFAULT_EXCLUDE_APPS))

    def ensure_privacy_defaults(self) -> None:
        """Write the defaults into config.toml once, so the owner sees and edits them there; a value the owner set
        (an emptied list included) is never replaced."""
        from ferb.policy.protection import DEFAULT_NEVER_READ, DEFAULT_NEVER_WRITE, DEFAULT_TERMINAL_APPS
        from ferb.sense.exclusions import DEFAULT_EXCLUDE_PATHS
        for section, key, value in (("privacy", "exclude_apps", list(DEFAULT_EXCLUDE_APPS)),
                                    ("privacy", "exclude_domains", []),
                                    ("privacy", "exclude_paths", list(DEFAULT_EXCLUDE_PATHS)),
                                    ("sense", "window_titles", True), ("sense", "watch_folders", []),
                                    ("sense", "idle_after_s", 120), ("sense", "trail_retention_days", 30),
                                    ("retention", "trace_days", 30), ("retention", "decision_days", 30),
                                    ("retention", "evidence_days", 7), ("retention", "recovery_days", 7),
                                    ("retention", "resource_days", 30), ("retention", "operational_days", 90),
                                    ("browser", "download_task_max_files", 300),
                                    ("browser", "download_task_max_gb", 32), ("browser", "download_total_gb", 20),
                                    ("browser", "download_min_free_gb", 10), ("browser", "task_max_tabs", 50),
                                    ("protection", "never_read", list(DEFAULT_NEVER_READ)),
                                    ("protection", "never_write", list(DEFAULT_NEVER_WRITE)),
                                    ("protection", "terminal_apps", list(DEFAULT_TERMINAL_APPS)),
                                    ("protection", "local_ports", [])):
            if self.get(section, key) is None:
                self.set(section, key, value)
