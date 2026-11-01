"""config.toml: atomic writes that stay 0600, other sections and comments preserved, loose permissions refused."""

import os
import stat
from pathlib import Path
from typing import Any

import pytest

from ferb.config import FEEDBACK_DECIDED, FERB_WRITTEN, LIMITS_DECIDED, Config, InsecureConfig

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


# decision: HAND-210, HAND-236
def test_fresh_defaults_are_the_owners_limits_with_no_time_based_download_clearing(home: Path) -> None:
    """HAND-210 (2026-10-31): no per-file cap, 300 files and 32 GB per task; decimal GB throughout."""
    c = Config(home)
    c.ensure_privacy_defaults()
    keys = ("download_task_max_files", "download_task_max_gb", "download_total_gb", "download_min_free_gb",
            "task_max_tabs")
    assert [c.get("browser", k) for k in keys] == [300, 32, 20, 10, 50]
    assert c.get("browser", "download_max_mb") is None and c.get("browser", "download_task_max_mb") is None
    assert c.get("retention", "download_days") is None


# decision: HAND-236
def test_the_2026_10_31_download_decision_replaces_the_old_values_and_keeps_an_owners_own(home: Path) -> None:
    """An older config (the 2026-10-09 values FERB wrote): the per-file key goes, 50 files becomes 300, the MB keys go;
    an owner's own MB value carries over in decimal GB. All journaled as changes."""
    c = Config(home)
    for key, value in (("download_max_mb", 512), ("download_task_max_files", 50), ("download_task_max_mb", 4096),
                       ("download_total_mb", 8192)):                  # 8192 MB: the owner's own folder budget
        c.set("browser", key, value)
    c.ensure_privacy_defaults()
    changes = c.upgrade_downloads()
    assert c.get("browser", "download_max_mb") is None and c.get("browser", "download_task_max_files") == 300
    assert c.get("browser", "download_task_max_mb") is None and c.get("browser", "download_task_max_gb") == 32
    assert c.get("browser", "download_total_mb") is None and c.get("browser", "download_total_gb") == 8.59
    assert {"key": "browser.download_max_mb", "from": 512, "to": None} in changes
    assert {"key": "browser.download_task_max_files", "from": 50, "to": 300} in changes
    assert c.upgrade_downloads() == []                              # nothing left to change
    c.set("browser", "download_task_max_files", 80)                  # the owner's own value afterwards stays
    assert c.upgrade_downloads() == [] and c.get("browser", "download_task_max_files") == 80


# decision: DMN-8, DMN-31
def test_the_relay_decision_drops_the_old_feedback_address(home: Path) -> None:
    """Owner 2026-11-01: reports go to the relay; an older config's `[feedback] address` goes (its value unjournaled),
    a relay address the owner set stays, and a table left empty goes."""
    c = Config(home)
    c.set("feedback", "address", "dev@example.org")
    assert c.upgrade(FEEDBACK_DECIDED) == [{"key": "feedback.address", "to": None}]
    assert "feedback" not in (home / "config.toml").read_text()
    assert c.upgrade(FEEDBACK_DECIDED) == []
    c.set("feedback", "address", "dev@example.org")
    c.set("feedback", "relay_url", "https://relay.example.org/report")
    c.upgrade(FEEDBACK_DECIDED)
    assert c.section("feedback") == {"relay_url": "https://relay.example.org/report"}


def test_the_limits_ferb_wrote_take_the_decided_values_and_an_owner_value_stays(home: Path) -> None:
    c = Config(home)
    for (section, key), (old, _) in FERB_WRITTEN.items():
        c.set(section, key, old)
    c.set("browser", "download_total_mb", 1000)                 # the owner's own value
    c.set("retention", "download_days", 30)
    changes = c.upgrade_ferb_written()
    assert (c.get("browser", "download_max_mb"), c.get("browser", "download_task_max_files"),
            c.get("browser", "download_task_max_mb"), c.get("browser", "task_max_tabs")) == (512, 50, 4096, 50)
    assert c.get("browser", "download_total_mb") == 1000 and c.get("retention", "download_days") is None
    assert {"key": "browser.download_max_mb", "from": 100, "to": 512} in changes
    assert {"key": "retention.download_days", "from": 30, "to": None} in changes
    assert not any(ch["key"] == "browser.download_total_mb" for ch in changes)
    assert "# keep this comment" in (home / "config.toml").read_text()


# decision: STORE-403
def test_the_daemon_applies_the_limits_decision_once_and_journals_it() -> None:
    import shutil
    import tempfile
    from tests.conftest import RunningDaemon
    base = Path(tempfile.mkdtemp(prefix="ferbt-"))              # short: AF_UNIX socket paths are limited

    async def upgraded(d: Any) -> list[dict[str, Any]]:
        return [e.data for e in d.daemon.journal.of_kind("config.upgraded", "upgrade", LIMITS_DECIDED)]
    try:
        Config(base / "home").set("browser", "task_max_tabs", 20)             # what F4's daemon wrote
        d = RunningDaemon(base / "home")
        try:
            assert d.daemon.config.get("browser", "task_max_tabs") == 50
            [row] = d.run(upgraded(d))
            assert {"key": "browser.task_max_tabs", "from": 20, "to": 50} in row["changes"]
            d.daemon.config.set("browser", "task_max_tabs", 20)               # now the owner's own choice
        finally:
            d.stop()
        d = RunningDaemon(base / "home")
        try:
            assert d.daemon.config.get("browser", "task_max_tabs") == 20       # never replaced again
            assert len(d.run(upgraded(d))) == 1
        finally:
            d.stop()
    finally:
        shutil.rmtree(base, ignore_errors=True)


# decision: STORE-403
def test_the_upgrade_is_all_or_nothing_and_journaled_only_once_it_is_saved(monkeypatch: pytest.MonkeyPatch) -> None:
    """A failure partway (the one save raises): config.toml is unchanged and nothing is journaled; the next start
    applies the whole decision once."""
    import asyncio
    import shutil
    import tempfile
    from ferb.daemon.main import Daemon
    from ferb.memory.journal import Journal
    from ferb.store.db import Database
    from tests.conftest import RunningDaemon
    base = Path(tempfile.mkdtemp(prefix="ferbt-"))              # short: AF_UNIX socket paths are limited
    home = base / "home"
    try:
        c = Config(home)
        c.ensure_privacy_defaults()                                 # an F4 config: every key present, FERB's values
        for (section, key), (old, _) in FERB_WRITTEN.items():
            c.set(section, key, old)
        c.set("retention", "download_days", 30)
        before = (home / "config.toml").read_text()

        save = Config.save

        def refused(self: Config, doc: Any) -> None:                # fails partway: the write carrying the tabs change
            if doc.get("browser", {}).get("task_max_tabs") == 50:
                raise OSError("disk full")
            save(self, doc)
        monkeypatch.setattr(Config, "save", refused)
        with pytest.raises(OSError):
            c.upgrade_ferb_written()
        assert (home / "config.toml").read_text() == before

        async def start() -> None:
            dm = Daemon(home=home, watchers=False, distill=False, retention=False, resources=False,
                        notifications=False, snapshots=False)
            try:
                await dm.start()
            finally:
                if dm.db is not None:
                    dm.db.close()
        with pytest.raises(OSError):
            asyncio.run(start())
        assert (home / "config.toml").read_text() == before
        db = Database(home / "ferb.db")
        try:
            assert Journal(db, None, None, lambda x: x).of_kind("config.upgraded") == []   # type: ignore[arg-type]
        finally:
            db.close()

        monkeypatch.undo()
        d = RunningDaemon(home)

        async def upgraded() -> list[dict[str, Any]]:
            return [e.data for e in d.daemon.journal.of_kind("config.upgraded", "upgrade", LIMITS_DECIDED)]
        try:
            [row] = d.run(upgraded())
            assert len(row["changes"]) == len(FERB_WRITTEN) + 1
            assert d.daemon.config.get("browser", "task_max_tabs") == 50
            assert d.daemon.config.get("retention", "download_days") is None
        finally:
            d.stop()
    finally:
        shutil.rmtree(base, ignore_errors=True)


# decision: DMN-12
def test_get_parses_once_and_again_only_when_the_file_changes(home: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Daemon step 2: `get` used to re-read and re-parse config.toml on every call (2.36 ms, 59 call sites)."""
    import tomlkit
    parses: list[int] = []
    real = tomlkit.parse
    monkeypatch.setattr(tomlkit, "parse", lambda text: (parses.append(1), real(text))[1])
    c = Config(home)
    assert c.get("secrets", "nope") is None and c.get("secrets", "nope", 3) == 3
    for _ in range(5):
        assert c.section("secrets")["ferb/email/password"]["account_hint"] == "ferb@example.com"
    assert len(parses) == 1
    (home / "config.toml").write_text(EXISTING + '\n[policy]\nautonomy = "careful"\n')   # the owner edits it
    assert c.get("policy", "autonomy") == "careful" and len(parses) == 2
    c.set("policy", "autonomy", "balanced")                       # FERB's own write updates the cache
    parses.clear()
    assert c.get("policy", "autonomy") == "balanced" and parses == []


def test_cached_values_are_plain_copies(home: Path) -> None:
    c = Config(home)
    c.set("privacy", "exclude_domains", ["a.example"])
    got = c.get("privacy", "exclude_domains")
    assert type(got) is list
    got.append("b.example")                                        # a caller changing its copy changes nothing
    assert c.get("privacy", "exclude_domains") == ["a.example"]


# decision: DMN-12
def test_a_loosened_mode_is_refused_even_with_a_cached_parse(home: Path) -> None:
    c = Config(home)
    assert c.get("policy", "autonomy") is None
    (home / "config.toml").chmod(0o644)
    with pytest.raises(InsecureConfig):
        c.get("policy", "autonomy")


def test_the_persona_proposal_moves_out_of_config_toml_on_first_read(home: Path) -> None:
    from ferb.device.persona import PersonaState
    c = Config(home)
    c.set("setup", "persona_pending", {"summary": "Apps you use: Code", "used_apps": ["Code"]})
    c.set("setup", "persona_reviewed", False)
    state = PersonaState(c)
    assert state.pending()["summary"] == "Apps you use: Code" and state.reviewed() is False
    assert c.get("setup", "persona_pending") is None and c.get("setup", "persona_reviewed") is None
    assert "# keep this comment" in (home / "config.toml").read_text()       # the rest of the owner's file stays
    assert stat.S_IMODE((home / "persona.json").stat().st_mode) == 0o600
    state.set_reviewed(True)
    assert PersonaState(c).reviewed() and PersonaState(c).pending()["used_apps"] == ["Code"]
    assert "persona" not in (home / "config.toml").read_text()
