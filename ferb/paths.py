"""Own FERB runtime directory locations. Must not resolve an ambiguous task file or change its source."""

import os
import sys
from pathlib import Path


def ferb_home() -> Path:
    """~/.ferb, or $FERB_HOME (tests, and running a second FERB side by side)."""
    return Path(os.environ.get("FERB_HOME", Path.home() / ".ferb"))


def socket_path(home: Path | None = None) -> Path:
    return (home or ferb_home()) / "run" / "ferb.sock"


def app_bundle(prefix: Path | None = None) -> Path | None:
    """The FERB.app whose Python runs this code (its prefix is <app>/Contents/Resources/python); None from a checkout
    or ~/.ferb/venv (L27)."""
    p = Path(prefix if prefix is not None else sys.prefix).resolve()
    contents = p.parent.parent
    if (p.name, p.parent.name, contents.name) == ("python", "Resources", "Contents") and contents.parent.suffix == ".app":
        return contents.parent
    return None
