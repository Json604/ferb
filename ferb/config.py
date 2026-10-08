"""Own configuration parsing and persistence. Must not infer task intent or change owner values silently.

- Refuses a config (or ~/.ferb) that others can read or write: allowed origins and policy live here.
- Writes are atomic: temp file in the same folder (0600) -> fsync -> rename over the original.
"""

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


class InsecureConfig(FerbError):
    pass


def _check_owner_only(path: Path) -> None:
    st = path.stat()
    if st.st_uid != os.getuid():
        raise InsecureConfig(f"{path} is not owned by this user")
    if stat.S_IMODE(st.st_mode) & 0o077:
        raise InsecureConfig(f"{path} has mode {oct(stat.S_IMODE(st.st_mode))}; others must have no access")


class Config:
    def __init__(self, home: Path) -> None:
        self.home = home
        self.path = home / "config.toml"

    def load(self) -> TOMLDocument:
        if not self.path.exists():
            return tomlkit.document()
        _check_owner_only(self.home)
        _check_owner_only(self.path)
        return tomlkit.parse(self.path.read_text())

    def save(self, doc: TOMLDocument) -> None:
        self.home.mkdir(parents=True, exist_ok=True)
        os.chmod(self.home, 0o700)
        fd, tmp = tempfile.mkstemp(prefix=".config.", suffix=".toml", dir=self.home)   # created 0600
        try:
            with os.fdopen(fd, "w") as f:
                f.write(tomlkit.dumps(doc))
                f.flush()
                os.fsync(f.fileno())
            os.chmod(tmp, 0o600)
            os.replace(tmp, self.path)             # atomic on the same filesystem
        except BaseException:
            Path(tmp).unlink(missing_ok=True)
            raise

    def get(self, section: str, key: str, default: Any = None) -> Any:
        doc = self.load()
        tbl = doc.get(section)
        return default if tbl is None else tbl.get(key, default)

    def set(self, section: str, key: str, value: Any) -> None:
        doc = self.load()
        if section not in doc:
            doc[section] = tomlkit.table()
        doc[section][key] = value              # type: ignore[index]
        self.save(doc)

    def exclude_apps(self) -> frozenset[str]:
        return frozenset(self.get("privacy", "exclude_apps", DEFAULT_EXCLUDE_APPS))

    def ensure_privacy_defaults(self) -> None:
        """Write the defaults into config.toml once, so the owner sees and edits them there; a value the owner set
        (an emptied list included) is never replaced."""
        from ferb.sense.exclusions import DEFAULT_EXCLUDE_PATHS
        for section, key, value in (("privacy", "exclude_apps", list(DEFAULT_EXCLUDE_APPS)),
                                    ("privacy", "exclude_domains", []),
                                    ("privacy", "exclude_paths", list(DEFAULT_EXCLUDE_PATHS)),
                                    ("sense", "window_titles", True), ("sense", "watch_folders", []),
                                    ("sense", "idle_after_s", 120), ("sense", "trail_retention_days", 30),
                                    ("retention", "trace_days", 30), ("retention", "decision_days", 30),
                                    ("retention", "evidence_days", 7), ("retention", "recovery_days", 7)):
            if self.get(section, key) is None:
                self.set(section, key, value)
