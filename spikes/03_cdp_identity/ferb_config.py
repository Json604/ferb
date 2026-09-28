"""SecretSpecs from ~/.ferb/config.toml, refusing a config others could read or change.

Allowed origins decide where a secret may be filled, so a group/world-writable (or readable) config or
directory is refused rather than trusted.
"""

import os
import stat
import tomllib
from pathlib import Path

from secrets_kc import SecretRef, SecretSpec

FERB_HOME = Path.home() / ".ferb"


class InsecureConfig(Exception):
    pass


def _check_owner_only(path: Path, mode: int) -> None:
    st = path.stat()
    if st.st_uid != os.getuid():
        raise InsecureConfig(f"{path} is not owned by this user")
    if stat.S_IMODE(st.st_mode) & 0o077:
        raise InsecureConfig(f"{path} has mode {oct(stat.S_IMODE(st.st_mode))}; expected {oct(mode)}")


def load_secret_specs(home: Path = FERB_HOME) -> list[SecretSpec]:
    config = home / "config.toml"
    _check_owner_only(home, 0o700)
    _check_owner_only(config, 0o600)
    data = tomllib.loads(config.read_text())
    return [SecretSpec(SecretRef(key), list(v["allowed_origins"]), v["account_hint"])
            for key, v in data.get("secrets", {}).items()]
