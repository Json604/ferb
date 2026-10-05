"""Owns setup/doctor verification of the explicitly configured dedicated FERB Chrome account profile.
Must not copy credentials, inspect owner cookies or select among multiple eligible profiles.
Local State and Preferences provide account metadata; setup requires exactly the configured FERB account."""

import json
from dataclasses import dataclass
from pathlib import Path

CHROME_DATA = Path.home() / "Library" / "Application Support" / "Google" / "Chrome"


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


def _only_ferb(p: ChromeProfile, address: str) -> ChromeProfile:
    if p.accounts != (address,):
        others = [a for a in p.accounts if a != address]
        raise SeedError(f"profile {p.name} ({p.folder}) " + (
            "also holds other accounts" if others and address in p.accounts else f"isn't signed into {address}")
            + f"; FERB's profile must hold only {address}")
    return p


def choose(all_profiles: list[ChromeProfile], address: str, name: str | None = None) -> ChromeProfile:
    """Setup: the one profile holding FERB's account and nothing else; refuses anything ambiguous or mixed."""
    address = address.lower()
    hits = [p for p in all_profiles if address in p.accounts]
    if name is not None:
        hits = [p for p in hits if name in (p.name, p.folder)]
    if not hits:
        raise SeedError(f"no Chrome profile{f' named {name!r}' if name else ''} is signed into {address}. Make a "
                        "dedicated Chrome profile, sign FERB's account into it (nothing else), and run this again.")
    if len(hits) > 1:
        raise SeedError("several Chrome profiles hold FERB's account: " + ", ".join(f"{p.name} ({p.folder})"
                                                                              for p in hits)
                        + ". Pick one with --profile NAME.")
    return _only_ferb(hits[0], address)


def verify(address: str, folder: str, chrome_data: Path = CHROME_DATA) -> ChromeProfile:
    """Doctor / pairing: the configured folder still exists and holds exactly FERB's account."""
    for p in profiles(chrome_data):
        if p.folder == folder:
            return _only_ferb(p, address.lower())
    raise SeedError(f"Chrome has no profile folder {folder!r} any more")
