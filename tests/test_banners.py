"""L34 D: banner categories, the owner's switches ([banners] in config.toml) and FERB.app as the poster."""
import asyncio
from pathlib import Path

import pytest

from ferb.config import Config
from ferb.core.errors import FerbError
from ferb.daemon import banners
from ferb.daemon.banners import Banner, Posters


def test_every_category_posts_until_the_owner_switches_it_off(tmp_path: Path) -> None:
    config = Config(tmp_path)
    assert banners.CATEGORIES == ("question", "reminder", "alert", "task_finished", "loop")
    assert banners.categories(config.get) == {c: True for c in banners.CATEGORIES}
    assert banners.set_categories(config, {"loop": False, "reminder": False}) == {
        "question": True, "reminder": False, "alert": True, "task_finished": True, "loop": False}
    assert not banners.enabled(config.get, "loop") and banners.enabled(config.get, "question")
    assert config.get("banners", "loop") is False
    assert banners.enabled(config.get, "")                     # "FERB noticed": no switch governs it


def test_switches_refuse_unknown_categories_and_non_booleans(tmp_path: Path) -> None:
    config = Config(tmp_path)
    for bad in ({"weather": False}, {"loop": "off"}, {}, ["loop"], {"loop": False, "x": True}):
        with pytest.raises(FerbError):
            banners.set_categories(config, bad)
    assert not (tmp_path / "config.toml").exists()             # a refusal writes nothing


def test_a_poster_takes_banners_only_while_connected() -> None:
    async def run() -> None:
        posters = Posters()
        assert not posters.connected and posters.deliver({"id": "x"}) is False
        queue = posters.open()
        assert posters.connected and posters.deliver({"id": "a"}) is True
        assert await queue.get() == {"id": "a"}
        posters.close(queue)
        assert not posters.connected and posters.deliver({"id": "b"}) is False
    asyncio.run(run())


def test_a_poster_that_stopped_reading_falls_back(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(banners, "QUEUE_SIZE", 1)

    async def run() -> None:
        posters = Posters()
        posters.open()
        assert posters.deliver({"id": "1"}) is True
        assert posters.deliver({"id": "2"}) is False           # full: osascript posts this one
    asyncio.run(run())


def test_a_banner_names_its_category_target_and_id() -> None:
    b = Banner("FERB reminder", "call Mom", "reminder", {"kind": "reminder", "timer_id": "t1"}, False, "reminder:t1")
    assert (b.category, b.target["timer_id"], b.ident, b.when_hidden) == ("reminder", "t1", "reminder:t1", False)
    assert Banner("FERB finished", "done") == Banner("FERB finished", "done", "", {}, False, "")
