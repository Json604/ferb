"""Seed FERB's identity browser profile (section 8 step 3).

Google refuses sign-in from a browser under remote debugging, so the owner signs FERB's Google account into a
dedicated profile of their normal Chrome, holding no other accounts or site logins. With Chrome quit, setup finds
that profile in `Local State`, checks its Chrome accounts (`Local State` + `Preferences` -> `account_info`) are
exactly FERB's address, shows it for confirmation, and copies it into `~/.ferb/chrome/identity/Default`.

Only `Local State` and `Preferences` are parsed. Cookies and saved passwords are copied as files, never read.
Setup never quits Chrome (it asks the owner to) and never looks at Brave.
"""

import json
import os
import shutil
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path

CHROME_DATA = Path.home() / "Library" / "Application Support" / "Google" / "Chrome"
CHROME_BIN = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
SKIP = shutil.ignore_patterns("Cache", "Code Cache", "GPUCache", "DawnGraphiteCache", "DawnWebGPUCache",
                              "GrShaderCache", "ShaderCache", "Singleton*")


class SeedError(Exception):
    pass


@dataclass(frozen=True)
class ChromeProfile:
    folder: str                 # "Profile 22"
    name: str                   # what the owner calls it ("FERB")
    accounts: tuple[str, ...]   # Chrome accounts signed into it (lowercased emails)


def profiles(chrome_data: Path = CHROME_DATA) -> list[ChromeProfile]:
    try:
        cache = json.loads((chrome_data / "Local State").read_text())["profile"]["info_cache"]
    except (OSError, ValueError, KeyError) as e:
        raise SeedError(f"can't read Chrome's Local State ({type(e).__name__})") from None
    out = []
    for folder, info in cache.items():
        accounts = {str(info.get("user_name") or "").lower()} - {""}
        try:
            prefs = json.loads((chrome_data / folder / "Preferences").read_text())
            accounts |= {str(a.get("email", "")).lower() for a in prefs.get("account_info", [])} - {""}
        except (OSError, ValueError, AttributeError):
            pass
        out.append(ChromeProfile(folder, str(info.get("name") or folder), tuple(sorted(accounts))))
    return out


def choose(all_profiles: list[ChromeProfile], address: str, name: str | None = None) -> ChromeProfile:
    """The one profile holding FERB's account and nothing else; refuses anything ambiguous or mixed."""
    address = address.lower()
    hits = [p for p in all_profiles if address in p.accounts]
    if name is not None:
        hits = [p for p in hits if name in (p.name, p.folder)]
    if not hits:
        raise SeedError(f"no Chrome profile{f' named {name!r}' if name else ''} is signed into {address}. Make a "
                        "dedicated Chrome profile, sign FERB's account into it (nothing else), quit Chrome, and run "
                        "this again.")
    if len(hits) > 1:
        raise SeedError("several Chrome profiles hold FERB's account: " + ", ".join(f"{p.name} ({p.folder})"
                                                                              for p in hits)
                        + ". Pick one with --profile NAME.")
    p = hits[0]
    if p.accounts != (address,):
        raise SeedError(f"profile {p.name} ({p.folder}) also holds other accounts; FERB's profile must hold only "
                        f"{address}")
    return p


def running_chromes(identity_dir: Path) -> tuple[bool, bool]:
    """(owner's Chrome running, FERB's own identity Chrome running) — from process arguments, never by asking
    Chrome. Helpers (`--type=...`) are ignored."""
    ps = subprocess.run(["ps", "-axww", "-o", "args="], capture_output=True, text=True).stdout
    owner = ferb = False
    for line in ps.splitlines():
        if not line.startswith(CHROME_BIN) or "--type=" in line:
            continue
        if f"--user-data-dir={identity_dir}" in line:
            ferb = True
        else:
            owner = True
    return owner, ferb


def copy_profile(p: ChromeProfile, identity_dir: Path, chrome_data: Path = CHROME_DATA) -> Path | None:
    """Copy the profile into identity_dir/Default (owner-only). An existing identity folder is moved aside, not
    deleted; returns where it went."""
    moved = None
    if identity_dir.exists():
        moved = identity_dir.with_name(f"{identity_dir.name}.old-{time.strftime('%Y%m%d-%H%M%S')}")
        identity_dir.rename(moved)
    identity_dir.mkdir(parents=True, mode=0o700)
    shutil.copytree(chrome_data / p.folder, identity_dir / "Default", symlinks=True, ignore=SKIP)
    for root, dirs, _ in os.walk(identity_dir):
        for d in dirs:
            os.chmod(Path(root) / d, 0o700)
    os.chmod(identity_dir, 0o700)
    return moved
