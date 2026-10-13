"""Shared support for the browser URL sink tests (S4): a fake identity browser and FERB's recorded questions."""
import json
from typing import Any

from ferb.act.browser.tools import BrowserNavigate, BrowserOpen, Family
from ferb.core.errors import Refused
from ferb.core.types import TabRef
from tests.conftest import FakeCap, RunningDaemon, gateway_request, gateway_setup


class FakeBrowser:
    """A minimal identity-browser driver for the sink tests: it tracks tabs and counts real opens/navigates, so a
    refusal (which returns before the tool runs) leaves the count and the tab untouched."""
    def __init__(self) -> None:
        self.tabs: dict[str, tuple[TabRef, str]] = {}
        self.n = 0
        self.calls = 0

    async def open_for(self, url: str, owner: str, profile: str | None = None, *, window_id: int | None = None,
                       allow_foreground: bool = False) -> TabRef:
        self.calls += 1
        self.n += 1
        ref = TabRef("dev", "chrome", profile or "FERB", 1, f"t{self.n}")
        self.tabs[ref.tab_id] = (ref, url)
        return ref

    def tab_url(self, tab_id: str) -> str | None:
        hit = self.tabs.get(tab_id)
        return hit[1] if hit is not None else None

    def claim(self, tab: TabRef, owner: str) -> None:
        pass

    def owned_by(self, tab_id: str, owner: str, borrow: bool = True) -> TabRef:
        hit = self.tabs.get(tab_id)
        if hit is None:
            raise Refused(f"no tab {tab_id}")
        return hit[0]

    async def navigate(self, tab: TabRef, url: str) -> None:
        self.calls += 1
        self.tabs[tab.tab_id] = (tab, url)

    async def shutdown(self) -> None:
        self.tabs.clear()


def browser_grant(daemon: RunningDaemon, family: Family, words: str, extra: list[FakeCap]) -> tuple[Any, FakeBrowser]:
    fb = FakeBrowser()
    caps: list[Any] = [BrowserOpen(fb, family), BrowserNavigate(fb, family)] + extra   # type: ignore[arg-type]
    grant, _ = gateway_setup(daemon, {family.read, "probe"}, gateway_request(words), caps, identity=family.identity)
    return grant, fb


def asked(daemon: RunningDaemon) -> list[dict[str, Any]]:
    """The args of every question FERB itself asked (owner.ask spans marked `ferb_question`), in order."""
    async def go() -> list[dict[str, Any]]:
        rows = daemon.daemon.db.conn.execute(
            "SELECT input_json FROM spans WHERE kind = 'tool.call' AND "
            "json_extract(input_json, '$.ferb_question') = 1 ORDER BY rowid").fetchall()
        return [json.loads(r["input_json"])["args"] for r in rows]
    return daemon.run(go())    # type: ignore[no-any-return]
