"""FERB's identity profile (section 8 step 3, safety test 20) against a fake Chrome data folder: only a profile
holding exactly FERB's account is accepted, the owner confirms it, nothing is copied, cookies are never read."""

import json
from pathlib import Path
from typing import Any

import pytest

from ferb.identity.seed import ChromeProfile, SeedError, choose, profiles, verify

ADDR = "ferb.test@gmail.com"


def chrome_data(tmp_path: Path, accounts: dict[str, tuple[str, list[str]]]) -> Path:
    root = tmp_path / "Chrome"
    cache = {}
    for folder, (name, emails) in accounts.items():
        (root / folder).mkdir(parents=True)
        (root / folder / "Cookies").write_text("encrypted cookie db")
        (root / folder / "Login Data").write_text("saved passwords")
        (root / folder / "Preferences").write_text(json.dumps({"account_info": [{"email": e} for e in emails]}))
        cache[folder] = {"name": name, "user_name": emails[0] if emails else ""}
    (root / "Local State").write_text(json.dumps({"profile": {"info_cache": cache}}))
    return root


def test_choose_and_verify_accept_only_a_profile_holding_just_ferbs_account(tmp_path: Path) -> None:
    root = chrome_data(tmp_path, {"Default": ("Me", ["owner@gmail.com"]), "Profile 7": ("FERB", [ADDR]),
                                  "Profile 3": ("Mixed", [ADDR, "owner@work.com"])})
    ps = profiles(root)
    assert ChromeProfile("Profile 7", "FERB", (ADDR,)) in ps
    with pytest.raises(SeedError, match="several"):
        choose(ps, ADDR)
    assert choose(ps, ADDR, "FERB").folder == "Profile 7"
    with pytest.raises(SeedError, match="also holds other accounts"):
        choose(ps, ADDR, "Mixed")
    with pytest.raises(SeedError, match="no Chrome profile"):
        choose(ps, "nobody@gmail.com")
    assert verify(ADDR, "Profile 7", root).name == "FERB"
    with pytest.raises(SeedError, match="also holds other accounts"):
        verify(ADDR, "Profile 3", root)                         # doctor flags a profile that gained an account
    with pytest.raises(SeedError, match="isn't signed into"):
        verify(ADDR, "Default", root)
    with pytest.raises(SeedError, match="no profile folder"):
        verify(ADDR, "Profile 99", root)


def test_cookie_and_password_stores_are_never_opened(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    root = chrome_data(tmp_path, {"Profile 7": ("FERB", [ADDR])})
    opened: list[str] = []
    real = Path.read_text

    def spy(self: Path, *a: Any, **k: Any) -> str:
        opened.append(self.name)
        return real(self, *a, **k)
    monkeypatch.setattr(Path, "read_text", spy)
    verify(ADDR, "Profile 7", root)
    assert set(opened) <= {"Local State", "Preferences"}
