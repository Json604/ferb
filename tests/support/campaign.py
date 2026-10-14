"""Shared support for the launch-campaign tests (D30, L3, L22): the owner's browser as BrowserAct reaches it (tabs,
element lookups, every delivered action recorded), the campaign's standing scope, a worker that posts through the
daemon's socket, and what FERB recorded (its own questions, a tool's calls, the day's security digest)."""
import json
from typing import Any

from ferb.act.browser.ext_bridge import OutcomeUnknown
from ferb.act.browser.tools import OWNER_BROWSER, BrowserAct, BrowserNavigate, BrowserOpen, Family
from ferb.core.types import ActionResult, BrowserAction, ElementRef, PageElement, TabRef
from ferb.orchestrator.plans import SendScope
from ferb.policy import security_digest
from tests.conftest import RunningDaemon, gateway_request, gateway_setup, tool
from tests.support.browser_sink import FakeBrowser

STORY = "FERB is live: a personal assistant that works across your Mac. https://ferb.kartikey.xyz"
X_SCOPE = SendScope(capability="browser.act", identity="owner", site="x.com", recipients=[], content=None)


class PostingBrowser(FakeBrowser):
    """FakeBrowser plus what BrowserAct needs to act on a page: every element is a plain control, and `act` records
    each action delivered as (page URL, action, text); a call FERB refuses never reaches it. `lose_next` makes the
    next action's outcome unknown, as when the extension disconnects right after Post."""

    def __init__(self) -> None:
        super().__init__()
        self.acts: list[tuple[str, str, str | None]] = []
        self.lose_next = False

    def element_info(self, tab_id: str, element_id: str) -> PageElement | None:
        return PageElement(element_id, "f0", "button", "", None, (0, 0, 1, 1), True)

    def element_ref(self, tab: TabRef, element_id: str) -> ElementRef:
        return ElementRef(tab, "f0", 1, element_id)

    async def act(self, el: ElementRef, action: BrowserAction, intent_id: str) -> ActionResult:
        if self.lose_next:
            self.lose_next = False
            raise OutcomeUnknown("the extension disconnected right after the click; outcome unknown")
        self.acts.append((self.tab_url(el.tab.tab_id) or "", action.kind, action.text))
        return ActionResult(True, action.kind)

    def clicks(self) -> list[str]:
        """The page URLs a click was delivered on, in order."""
        return [url for url, kind, _ in self.acts if kind == "click"]


def browser_caps(browser: PostingBrowser, family: Family = OWNER_BROWSER) -> list[Any]:
    return [BrowserOpen(browser, family), BrowserNavigate(browser, family), BrowserAct(browser, family)]


def campaign_grant(d: RunningDaemon, words: str, extra: list[Any] | None = None) -> tuple[Any, PostingBrowser]:
    """A worker in the owner's browser for a request in the owner's `words`, with any `extra` capabilities."""
    browser = PostingBrowser()
    grant, _ = gateway_setup(d, {OWNER_BROWSER.read, OWNER_BROWSER.act, "probe"}, gateway_request(words),
                             browser_caps(browser) + (extra or []), identity=OWNER_BROWSER.identity)
    return grant, browser


def stand(d: RunningDaemon, tree_id: str, *scopes: SendScope) -> None:
    """The standing scope as the plan stated it, granted for this tree (D30)."""
    async def go() -> None:
        d.daemon.gateway.standing.grant(tree_id, list(scopes), "stated:test")
    d.run(go())


def ferb_questions(d: RunningDaemon) -> list[dict[str, Any]]:
    """The args of every question FERB itself asked (owner.ask spans marked `ferb_question`), in order."""
    async def go() -> list[dict[str, Any]]:
        rows = d.daemon.db.conn.execute(
            "SELECT input_json FROM spans WHERE kind = 'tool.call' AND "
            "json_extract(input_json, '$.ferb_question') = 1 ORDER BY rowid").fetchall()
        return [json.loads(r["input_json"])["args"] for r in rows]
    return d.run(go())                                   # type: ignore[no-any-return]


def calls_of(d: RunningDaemon, name: str) -> list[dict[str, Any]]:
    """The recorded inputs of a tool's calls (tool.call spans), in order."""
    async def go() -> list[dict[str, Any]]:
        return [json.loads(r["input_json"]) for r in d.daemon.db.conn.execute(
            "SELECT input_json FROM spans WHERE kind = 'tool.call' AND json_extract(input_json, '$.tool') = ? "
            "ORDER BY rowid", (name,)).fetchall()]
    return d.run(go())                                   # type: ignore[no-any-return]


def digest(d: RunningDaemon) -> dict[str, Any]:
    """Today's code-built security digest (S10), as the daily summary keeps it."""
    async def go() -> dict[str, Any]:
        return security_digest.build(d.daemon.db, d.daemon.clock.now().date().isoformat())
    return d.run(go())                                   # type: ignore[no-any-return]


def posts_on(sock: Any, url: str, text: str = STORY, **declared: Any) -> Any:
    """A worker that opens `url` in the owner's browser, types `text` into the composer and clicks Post, declaring
    the click's effect and destination (`declared`: effect, destination). Returns its report."""
    def worker(token: str) -> str:
        tab = tool(sock, token, "browser.open", url=url)["content"]["tab"]
        typed = tool(sock, token, "browser.act", tab=tab, element="s1e1", action="type", text=text)
        clicked = tool(sock, token, "browser.act", tab=tab, element="s1e2", action="click", **declared)
        ok = not typed["is_error"] and not clicked["is_error"]
        return json.dumps({"status": "done" if ok else "failed", "summary": "posted" if ok else str(clicked),
                           "artifacts": [], "evidence": []})
    return worker
