"""config.toml (section 6): load and save with tomlkit so comments and other sections survive.

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
