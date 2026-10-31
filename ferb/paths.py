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


APP_ID = "com.ferb.FERB"                         # FERB.app's bundle id (ui/mac/FERBNotch/Info.plist)


def installed_app(candidates: tuple[Path, ...] | None = None) -> Path | None:
    """The installed FERB.app, for a daemon that doesn't run from one (setup records no location): /Applications/FERB.app,
    then ~/Applications/FERB.app, the first with FERB's bundle id; None when neither is there."""
    import plistlib
    for app in candidates if candidates is not None else (Path("/Applications/FERB.app"),
                                                          Path.home() / "Applications" / "FERB.app"):
        try:
            with (app / "Contents" / "Info.plist").open("rb") as f:
                if plistlib.load(f).get("CFBundleIdentifier") == APP_ID:
                    return app
        except (OSError, ValueError, plistlib.InvalidFileException):
            continue
    return None


BUNDLE_PYTHON = Path("Contents/Resources/python/bin/python3.12")   # FERB.app's plain interpreter (L27)


def helper_python(prefix: Path | None = None) -> list[str]:
    """How the daemon starts a Python child that runs FERB's own fixed code (`-c`): with its own interpreter, or, in
    FERB.app, with the app's plain python3.12 (never writing bytecode into the sealed bundle), because the daemon's own
    executable there is the agent app, which runs only FERB's entry points (L27)."""
    bundle = app_bundle(prefix)
    return [str(bundle / BUNDLE_PYTHON), "-B"] if bundle else [sys.executable]
