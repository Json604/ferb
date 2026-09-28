"""config.toml: atomic writes that stay 0600, other sections and comments preserved, loose permissions refused."""

import os
import stat
from pathlib import Path

import pytest

from ferb.config import Config, InsecureConfig

EXISTING = """# FERB runtime config. Owner-only (0600).

[secrets."ferb/email/password"]
allowed_origins = ["https://accounts.google.com"]  # keep this comment
account_hint = "ferb@example.com"
"""


@pytest.fixture
def home(tmp_path: Path) -> Path:
    h = tmp_path / ".ferb"
    h.mkdir(mode=0o700)
    (h / "config.toml").write_text(EXISTING)
    (h / "config.toml").chmod(0o600)
    return h


def test_set_preserves_other_sections_and_comments_and_stays_0600(home: Path) -> None:
    c = Config(home)
    c.set("policy", "autonomy", "careful")
    text = (home / "config.toml").read_text()
    assert "# keep this comment" in text and 'account_hint = "ferb@example.com"' in text
    assert c.get("policy", "autonomy") == "careful"
    assert stat.S_IMODE((home / "config.toml").stat().st_mode) == 0o600
    assert not list(home.glob(".config.*"))                     # no temp files left behind


def test_write_is_atomic_rename_over_the_original(home: Path) -> None:
    before = (home / "config.toml").stat().st_ino
    Config(home).set("policy", "autonomy", "hands-off")
    assert (home / "config.toml").stat().st_ino != before       # replaced by rename, not rewritten in place


def test_failed_write_leaves_the_original_intact(home: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    def boom(*a: object) -> None:
        raise OSError("disk full")
    monkeypatch.setattr(os, "replace", boom)
    with pytest.raises(OSError):
        Config(home).set("policy", "autonomy", "careful")
    assert (home / "config.toml").read_text() == EXISTING
    assert not list(home.glob(".config.*"))


@pytest.mark.parametrize("mode", [0o644, 0o620, 0o604])
def test_loose_permissions_are_refused(home: Path, mode: int) -> None:
    (home / "config.toml").chmod(mode)
    with pytest.raises(InsecureConfig):
        Config(home).load()


def test_missing_config_is_empty_and_first_write_creates_it_0600(tmp_path: Path) -> None:
    h = tmp_path / "fresh"
    c = Config(h)
    assert c.get("policy", "autonomy", "balanced") == "balanced"
    c.set("policy", "autonomy", "careful")
    assert stat.S_IMODE((h / "config.toml").stat().st_mode) == 0o600
    assert stat.S_IMODE(h.stat().st_mode) == 0o700
