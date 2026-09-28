"""Seeding FERB's identity profile (section 8 step 3) against a fake Chrome data folder: only FERB's account,
Chrome must be quit by the owner (never by FERB), confirmation before copying, the old profile kept."""

import io
import json
import stat
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.setup import Env, seed_identity
from ferb.identity.seed import ChromeProfile, SeedError, choose, copy_profile, profiles

ADDR = "ferb.test@gmail.com"


def chrome_data(tmp_path: Path, accounts: dict[str, tuple[str, list[str]]]) -> Path:
    root = tmp_path / "Chrome"
    cache = {}
    for folder, (name, emails) in accounts.items():
        (root / folder / "Cache").mkdir(parents=True)
        (root / folder / "Cache" / "big").write_text("x")
        (root / folder / "Cookies").write_text("encrypted cookie db")
        (root / folder / "Preferences").write_text(json.dumps({"account_info": [{"email": e} for e in emails]}))
        cache[folder] = {"name": name, "user_name": emails[0] if emails else ""}
    (root / "Local State").write_text(json.dumps({"profile": {"info_cache": cache}}))
    return root


def test_profiles_and_choose_only_a_profile_holding_just_ferbs_account(tmp_path: Path) -> None:
    root = chrome_data(tmp_path, {"Default": ("Me", ["owner@gmail.com"]), "Profile 22": ("FERB", [ADDR]),
                                  "Profile 3": ("Mixed", [ADDR, "owner@work.com"])})
    ps = profiles(root)
    assert ChromeProfile("Profile 22", "FERB", (ADDR,)) in ps
    with pytest.raises(SeedError, match="several"):
        choose(ps, ADDR)
    assert choose(ps, ADDR, "FERB").folder == "Profile 22"
    with pytest.raises(SeedError, match="also holds other accounts"):
        choose(ps, ADDR, "Mixed")
    with pytest.raises(SeedError, match="no Chrome profile"):
        choose(ps, "nobody@gmail.com")


def test_copy_is_owner_only_skips_caches_and_keeps_the_old_profile(tmp_path: Path) -> None:
    root = chrome_data(tmp_path, {"Profile 22": ("FERB", [ADDR])})
    ident = tmp_path / "home" / "chrome" / "identity"
    (ident / "Default").mkdir(parents=True)
    (ident / "Default" / "old").write_text("previous")
    moved = copy_profile(ChromeProfile("Profile 22", "FERB", (ADDR,)), ident, root)
    assert moved is not None and (moved / "Default" / "old").exists()
    assert (ident / "Default" / "Cookies").exists() and not (ident / "Default" / "Cache").exists()
    assert stat.S_IMODE(ident.stat().st_mode) == 0o700 and stat.S_IMODE((ident / "Default").stat().st_mode) == 0o700


class FakeInst:
    def __init__(self, home: Path) -> None:
        self.home = home


class FakeClient:
    def __init__(self, signed_in: bool) -> None:
        self.signed_in = signed_in

    def call(self, method: str, params: Any = None) -> dict[str, Any]:
        assert method == "identity.check"
        return {"seeded": True, "address": ADDR, "signed_in": self.signed_in, "error": None, "secrets": []}

    def close(self) -> None:
        pass


def env(tmp_path: Path, answers: list[str], signed_in: bool = True) -> tuple[Env, io.StringIO]:
    home = tmp_path / "home"
    home.mkdir(exist_ok=True, mode=0o700)
    out = io.StringIO()
    return Env(FakeInst(home), lambda: FakeClient(signed_in), lambda _q: answers.pop(0), out), out  # type: ignore


def test_seed_asks_the_owner_to_quit_chrome_and_never_quits_it(tmp_path: Path) -> None:
    root = chrome_data(tmp_path, {"Profile 22": ("FERB", [ADDR])})
    e, out = env(tmp_path, [ADDR])
    calls: list[Path] = []

    def running(p: Path) -> tuple[bool, bool]:
        calls.append(p)
        return True, False
    assert seed_identity(e, chrome_data=root, running=running) == 1
    assert "Please quit Google Chrome" in out.getvalue() and "won't quit it" in out.getvalue()
    assert not (tmp_path / "home" / "chrome").exists()


def test_seed_confirms_copies_and_verifies_the_session(tmp_path: Path) -> None:
    root = chrome_data(tmp_path, {"Default": ("Me", ["owner@gmail.com"]), "Profile 22": ("FERB", [ADDR])})
    e, out = env(tmp_path, [ADDR, "y"])
    assert seed_identity(e, chrome_data=root, running=lambda p: (False, False)) == 0
    text = out.getvalue()
    assert "“FERB” (folder Profile 22)" in text and f"Signed in as {ADDR}" in text
    assert (tmp_path / "home" / "chrome" / "identity" / "Default" / "Preferences").exists()
    e2, out2 = env(tmp_path, ["n"])                                     # address now remembered in config
    assert seed_identity(e2, chrome_data=root, running=lambda p: (False, False)) == 1
    assert "replaces FERB's current browser profile" in out2.getvalue() and "Not copied" in out2.getvalue()


def test_seed_refuses_while_ferbs_own_browser_runs(tmp_path: Path) -> None:
    e, out = env(tmp_path, [ADDR])
    assert seed_identity(e, chrome_data=tmp_path, running=lambda p: (False, True)) == 1
    assert "in use right now" in out.getvalue()
