"""Own FERB runtime directory locations. Must not resolve an ambiguous task file or change its source."""

import os
from pathlib import Path


def ferb_home() -> Path:
    """~/.ferb, or $FERB_HOME (tests, and running a second FERB side by side)."""
    return Path(os.environ.get("FERB_HOME", Path.home() / ".ferb"))


def socket_path(home: Path | None = None) -> Path:
    return (home or ferb_home()) / "run" / "ferb.sock"
