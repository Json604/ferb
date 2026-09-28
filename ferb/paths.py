"""FERB's runtime locations (section 3): one place so the daemon and the CLI always agree."""

import os
from pathlib import Path


def ferb_home() -> Path:
    """~/.ferb, or $FERB_HOME (tests, and running a second FERB side by side)."""
    return Path(os.environ.get("FERB_HOME", Path.home() / ".ferb"))


def socket_path(home: Path | None = None) -> Path:
    return (home or ferb_home()) / "run" / "ferb.sock"
