"""browser.* tools through a real daemon + gateway + headless Chrome on local pages.

Safety tests: 21.3 (identity separation of tool sets), 21.5 (a filled password appears nowhere FERB writes),
intent gating of submits (10.2), tabs private to their task and closed with it, `ferb stop` closes FERB's browser.
"""

import json
import time
from datetime import timedelta
from typing import Any, Iterator

import pytest

from ferb.act.browser.tools import act_kind
from ferb.cli.client import Client
from ferb.core.types import Budget, Identity, PageElement, SecretSpec, Task
from ferb.identity.secrets import KeychainSecretStore
from ferb.orchestrator.tree import TaskRecord
from ferb.policy.intent import RequestIntent, site_matches
from ferb.policy.rules import Kind
from tests.browserkit import PASSWORD, REF
from tests.conftest import RunningDaemon

FERB_SCOPES = {"identity:browse", "secrets:fill", "mailbox:read"}


@pytest.fixture
def bd(daemon: RunningDaemon, keychain: Any, site: dict[str, Any]) -> Iterator[RunningDaemon]:
    drv = daemon.daemon.identity_browser
    (drv.user_data_dir / "Default").mkdir(parents=True)
    drv.allow_local = True
    specs = {REF.key: SecretSpec(REF, [site["L"]], "test account")}
    drv.secrets = KeychainSecretStore(lambda: specs, keychain)
    yield daemon


def task(d: RunningDaemon, tid: str, identity: Identity, scopes: set[str], intent: RequestIntent | None) -> Any:
    t = Task(tid, None, "g", "dev", identity, scopes, "actor", f"req-{tid}", [],
             d.daemon.clock.now() + timedelta(minutes=10), Budget())

    async def go() -> Any:
        root = d.daemon.spans.start("task", task_id=t.id, input={})
        d.daemon.tasks.add(TaskRecord(t, d.daemon.clock.now(), root.id))
        if intent:
            d.daemon.engine.intents.record(RequestIntent(f"req-{tid}", intent.kinds, sites=intent.sites))
        token = d.daemon.tokens.issue(t.id, f"w-{tid}", scopes, identity, ttl_s=600)
        return d.daemon.tokens.validate(token)
    return d.run(go())


def call(d: RunningDaemon, grant: Any, name: str, **args: Any) -> dict[str, Any]:
    return d.run(d.daemon.gateway.call(grant, name, args), timeout=60)          # type: ignore[no-any-return]


def el(snapshot: dict[str, Any], **match: Any) -> str:
    for e in snapshot["elements"]:
        if all(e.get(k) == v for k, v in match.items()):
            return str(e["id"])
    raise AssertionError(f"no {match} in {snapshot['elements']}")


def test_identity_separates_tool_sets(bd: RunningDaemon) -> None:
    """21.3: FERB tasks get FERB's browser/secrets/mailbox; OWNER tasks never do (and can't call them)."""
    ferb = task(bd, "a", Identity.FERB, FERB_SCOPES, None)
    owner = task(bd, "b", Identity.OWNER, {"browser:read", "browser:act", "fs:read"}, None)
    ferb_tools = {t["name"] for t in bd.daemon.gateway.tools_for(ferb)}
    assert {"browser.open", "browser.fill_secret", "browser.fill_otp"} <= ferb_tools
    assert not {n for n in (t["name"] for t in bd.daemon.gateway.tools_for(owner)) if n.startswith("browser.")}
    r = call(bd, owner, "browser.fill_secret", tab="t1", element="s1e1", secret=REF.key)
    assert r["is_error"] and "not available" in r["content"]["error"]
    browse_only = task(bd, "c", Identity.FERB, {"identity:browse"}, None)
    assert "browser.fill_secret" not in {t["name"] for t in bd.daemon.gateway.tools_for(browse_only)}
    assert REF.key in next(t["description"] for t in bd.daemon.gateway.tools_for(ferb)
                           if t["name"] == "browser.fill_secret")          # names, never values


def test_sign_in_through_the_gateway_and_the_password_is_nowhere(bd: RunningDaemon, site: dict[str, Any]) -> None:
    """21.5: after a real sign-in, the password is in no tool result, span, journal row, ledger row or log."""
    g = task(bd, "s", Identity.FERB, FERB_SCOPES, RequestIntent.from_plan("req-s", {"sign_in"}))
    results = [call(bd, g, "browser.open", url=f"{site['L']}/login")]
    tab = results[0]["content"]["tab"]
    results.append(call(bd, g, "browser.snapshot", tab=tab))
    snap = results[-1]["content"]
    results.append(call(bd, g, "browser.act", tab=tab, element=el(snap, role="textbox", label=""), action="type",
                        text="ferb-test"))
    results.append(call(bd, g, "browser.fill_secret", tab=tab, element=el(snap, type="password"), secret=REF.key))
    results.append(call(bd, g, "browser.snapshot", tab=tab))
    snap2 = results[-1]["content"]
    assert el(snap2, type="password", sensitive=True, filled=True)
    results.append(call(bd, g, "browser.act", tab=tab, element=el(snap2, role="button"), action="click"))
    results.append(call(bd, g, "browser.wait", tab=tab, until="text", value="Welcome back"))
    assert all(not r["is_error"] for r in results), [r for r in results if r["is_error"]]
    assert results[-1]["content"]["met"] is True
    assert PASSWORD in site["posted"][-1]                                    # the site got it...
    assert PASSWORD not in json.dumps(results)                               # ...the worker never did
    bd.run(bd.daemon.identity_browser.release("s"))
    db = bd.daemon.home / "ferb.db"
    for f in (db, db.with_name("ferb.db-wal")):
        if f.exists():
            assert PASSWORD.encode() not in f.read_bytes(), f.name          # spans, journal, ledger
    for line in (bd.daemon.home / "logs").glob("*") if (bd.daemon.home / "logs").exists() else []:
        assert PASSWORD not in line.read_text(errors="ignore")


def test_submits_need_the_intent_but_searches_do_not(bd: RunningDaemon, site: dict[str, Any]) -> None:
    g = task(bd, "r", Identity.FERB, FERB_SCOPES, RequestIntent.from_plan("req-r", {"research"}))
    tab = call(bd, g, "browser.open", url=f"{site['L']}/search")["content"]["tab"]
    snap = call(bd, g, "browser.snapshot", tab=tab)["content"]
    q = el(snap, form="search")
    assert not call(bd, g, "browser.act", tab=tab, element=q, action="type", text="weather")["is_error"]
    assert not call(bd, g, "browser.act", tab=tab, element=q, action="press", text="Enter")["is_error"]
    call(bd, g, "browser.navigate", tab=tab, url=f"{site['L']}/login")
    snap = call(bd, g, "browser.snapshot", tab=tab)["content"]
    r = call(bd, g, "browser.act", tab=tab, element=el(snap, role="button"), action="click")
    assert r["is_error"] and "'submit' was not requested" in r["content"]["error"]
    r = call(bd, g, "browser.fill_secret", tab=tab, element=el(snap, type="password"), secret=REF.key)
    assert r["is_error"] and "'fill_secret' was not requested" in r["content"]["error"]
    assert site["posted"] == [] or "weather" not in site["posted"][-1]


def test_tabs_are_private_to_their_task_and_close_with_it(bd: RunningDaemon, site: dict[str, Any]) -> None:
    a = task(bd, "ta", Identity.FERB, {"identity:browse"}, None)
    b = task(bd, "tb", Identity.FERB, {"identity:browse"}, None)
    tab = call(bd, a, "browser.open", url=f"{site['L']}/login")["content"]["tab"]
    r = call(bd, b, "browser.snapshot", tab=tab)
    assert r["is_error"] and "no tab" in r["content"]["error"]
    bd.run(bd.daemon.identity_browser.release("ta"))
    assert not bd.daemon.identity_browser.running                            # nothing left open: Chrome stopped
    assert call(bd, a, "browser.snapshot", tab=tab)["is_error"]


def test_stop_closes_ferbs_browser(bd: RunningDaemon, site: dict[str, Any]) -> None:
    g = task(bd, "st", Identity.FERB, {"identity:browse"}, None)
    call(bd, g, "browser.open", url=f"{site['L']}/login")
    assert bd.daemon.identity_browser.running
    c = Client(bd.sock)
    try:
        c.call("stop.engage")
        end = time.monotonic() + 10
        while bd.daemon.identity_browser.running and time.monotonic() < end:
            time.sleep(0.1)
        assert not bd.daemon.identity_browser.running
    finally:
        c.call("stop.lift")
        c.close()


def test_private_addresses_refused_in_the_real_config(daemon: RunningDaemon) -> None:
    g = task(daemon, "p", Identity.FERB, {"identity:browse"}, None)
    r = call(daemon, g, "browser.open", url="http://127.0.0.1:9/")
    assert r["is_error"] and "local" in r["content"]["error"]


def test_site_matching_covers_subdomains_only() -> None:
    sites = frozenset({"example.com", "https://www.shop.io/cart"})
    assert site_matches("example.com", sites) and site_matches("accounts.example.com", sites)
    assert site_matches("www.shop.io", sites) and site_matches("shop.io", sites)
    assert not site_matches("example.com.evil.io", sites) and not site_matches("badexample.com", sites)


def test_action_kinds_follow_what_the_element_does() -> None:
    def e(role: str, form: str | None = None, href: str | None = None) -> PageElement:
        return PageElement("s1e1", "f", role, "x", None, (0, 0, 1, 1), True, href=href, form=form)
    assert act_kind("click", None, e("link", href="https://x")) == Kind.NAVIGATE
    assert act_kind("click", None, e("button", "search")) == Kind.SEARCH
    assert act_kind("click", None, e("button", "other")) == Kind.SUBMIT
    assert act_kind("click", None, e("button")) == Kind.SUBMIT
    assert act_kind("click", None, e("checkbox", "other")) == Kind.FILL_FORM
    assert act_kind("type", "hi", e("textbox", "search")) == Kind.SEARCH
    assert act_kind("type", "hi", e("textbox", "login")) == Kind.FILL_FORM
    assert act_kind("press", "Enter", e("textbox", "other")) == Kind.SUBMIT
    assert act_kind("press", "Tab", e("textbox", "other")) == Kind.READ
    assert act_kind("click", None, None) == Kind.SUBMIT                     # unknown element: strictest
