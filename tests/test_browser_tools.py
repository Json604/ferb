"""browser.* tools through a real daemon + gateway, on the in-memory identity driver (tests/browserkit.py).

Safety tests: 21.3 (identity separation of tool sets), 21.5 (a filled password or code appears nowhere FERB
writes, and never in a worker's results), the effect gate on form submits (10.2), tabs private to their task and closed
with it, `ferb stop` closes FERB's browser, local/private URLs refused before any driver sees them.
"""

import json
import time
from datetime import datetime, timedelta, timezone
from typing import Any, Iterator

import pytest

from ferb.act.browser.tools import act_kind, check_url
from ferb.cli.client import Client
from ferb.core.errors import Refused
from ferb.core.types import Identity, PageElement, SecretSpec, Task
from ferb.orchestrator.tree import TaskRecord
from ferb.policy.rules import Kind
from ferb.policy.targets import Request, site_matches
from tests.browserkit import PASSWORD, REF, FakeIdentityDriver, fake_driver, feed
from tests.conftest import RunningDaemon

FERB_SCOPES = {"identity:browse", "secrets:fill", "mailbox:read"}
SITE = "https://login.example.com"


@pytest.fixture
def bd(daemon: RunningDaemon, keychain: Any) -> Iterator[tuple[RunningDaemon, FakeIdentityDriver]]:
    specs = {REF.key: SecretSpec(REF, [SITE], "test account")}
    drv = fake_driver(keychain, specs, [])
    drv.add_secret_value = daemon.daemon._add_secret_value                   # the daemon's redactor learns it
    daemon.daemon.use_identity_browser(drv)
    yield daemon, drv


def task(d: RunningDaemon, tid: str, identity: Identity, scopes: set[str], words: str | None = None) -> Any:
    """A running task; `words` are the owner's own words for its request (what counts as named at the gateway)."""
    t = Task(tid, None, "g", "dev", identity, scopes, "worker", f"req-{tid}", [],
             d.daemon.clock.now() + timedelta(minutes=10))

    async def go() -> Any:
        root = d.daemon.spans.start("task", task_id=t.id, input={})
        d.daemon.tasks.add(TaskRecord(t, d.daemon.clock.now(), root.id))
        d.daemon.engine.requests.record(Request(f"req-{tid}", words or ""))
        token = d.daemon.tokens.issue(t.id, f"w-{tid}", scopes, identity, ttl_s=600)
        return d.daemon.tokens.validate(token)
    return d.run(go())


def call(d: RunningDaemon, grant: Any, name: str, **args: Any) -> dict[str, Any]:
    return d.run(d.daemon.gateway.call(grant, name, args), timeout=30)          # type: ignore[no-any-return]


def el(snapshot: dict[str, Any], **match: Any) -> str:
    for e in snapshot["elements"]:
        if all(e.get(k) == v for k, v in match.items()):
            return str(e["id"])
    raise AssertionError(f"no {match} in {snapshot['elements']}")


def test_identity_separates_tool_sets(bd: Any) -> None:
    """21.3: FERB tasks get FERB's browser/secrets/mailbox; OWNER tasks never do (and can't call them)."""
    d, _ = bd
    ferb = task(d, "a", Identity.FERB, FERB_SCOPES, None)
    owner = task(d, "b", Identity.OWNER, {"browser:read", "browser:act", "fs:read"}, None)
    ferb_tools = {t["name"] for t in d.daemon.gateway.tools_for(ferb)}
    owner_tools = {t["name"] for t in d.daemon.gateway.tools_for(owner)}
    assert {"ferb_browser.open", "ferb_browser.fill_secret", "ferb_browser.fill_otp"} <= ferb_tools
    assert not {n for n in ferb_tools if n.startswith("browser.")}              # never the owner's profiles
    assert "browser.open" in owner_tools and not {n for n in owner_tools if n.startswith("ferb_browser.")}
    assert not {n for n in owner_tools if "fill_" in n}                         # no FERB secrets for owner tasks
    r = call(d, owner, "ferb_browser.fill_secret", tab="t1", element="s1e1", secret=REF.key)
    assert r["is_error"] and "not available" in r["content"]["error"]
    browse_only = task(d, "c", Identity.FERB, {"identity:browse"}, None)
    assert "ferb_browser.fill_secret" not in {t["name"] for t in d.daemon.gateway.tools_for(browse_only)}
    desc = next(t["description"] for t in d.daemon.gateway.tools_for(ferb) if t["name"] == "ferb_browser.fill_secret")
    assert REF.key in desc and PASSWORD not in desc                          # names, never values


def test_no_browser_tools_until_a_driver_is_attached(daemon: RunningDaemon) -> None:
    g = task(daemon, "n", Identity.FERB, FERB_SCOPES, None)
    assert not [t for t in daemon.daemon.gateway.tools_for(g) if t["name"].startswith("ferb_browser.")]


def test_sign_in_through_the_gateway_and_the_password_is_nowhere(bd: Any) -> None:
    """21.5: after a sign-in, the password is in no tool result, span, journal row, ledger row or log."""
    d, drv = bd
    g = task(d, "s", Identity.FERB, FERB_SCOPES, "sign in to login.example.com")
    results = [call(d, g, "ferb_browser.open", url=f"{SITE}/login")]
    tab = results[0]["content"]["tab"]
    results.append(call(d, g, "ferb_browser.snapshot", tab=tab))
    snap = results[-1]["content"]
    results.append(call(d, g, "ferb_browser.act", tab=tab, element=el(snap, role="textbox", form="login"),
                        action="type", text="ferb-test"))
    results.append(call(d, g, "ferb_browser.fill_secret", tab=tab, element=el(snap, type="password"), secret=REF.key))
    results.append(call(d, g, "ferb_browser.snapshot", tab=tab))
    snap2 = results[-1]["content"]
    assert el(snap2, type="password", sensitive=True, filled=True)
    results.append(call(d, g, "ferb_browser.act", tab=tab, element=el(snap2, role="button"), action="click",
                        effect="send", why="signing in"))                  # a form submit: send, site named
    results.append(call(d, g, "ferb_browser.wait", tab=tab, until="text", value="Welcome back"))
    assert all(not r["is_error"] for r in results), [r for r in results if r["is_error"]]
    assert results[-1]["content"]["met"] is True
    assert PASSWORD in drv.posted[-1].values()                               # the site got it...
    assert PASSWORD not in json.dumps(results)                               # ...the worker never did
    _nowhere(d, PASSWORD)


def test_fill_otp_code_goes_to_the_field_never_to_the_worker(bd: Any) -> None:
    d, drv = bd
    now = datetime.now(timezone.utc)
    drv.feed_xml = feed([("noreply@login.example.com", "Your code", "Your sign-in code is 482913.", now)])
    g = task(d, "o", Identity.FERB, FERB_SCOPES, "sign in")
    tab = call(d, g, "ferb_browser.open", url=f"{SITE}/otp")["content"]["tab"]
    snap = call(d, g, "ferb_browser.snapshot", tab=tab)["content"]
    r = call(d, g, "ferb_browser.fill_otp", tab=tab, element=el(snap, sensitive=True), secret=REF.key)
    assert not r["is_error"] and "482913" not in json.dumps(r)
    assert drv.value(tab, 0) == "482913"
    snap = call(d, g, "ferb_browser.snapshot", tab=tab)["content"]
    again = call(d, g, "ferb_browser.fill_otp", tab=tab, element=el(snap, sensitive=True), secret=REF.key)
    assert again["is_error"] and "no new code" in again["content"]["detail"]  # a used message is never reused
    _nowhere(d, "482913")


def test_fill_refused_on_a_page_the_secret_isnt_for(bd: Any) -> None:
    d, drv = bd
    g = task(d, "f", Identity.FERB, FERB_SCOPES, "sign in")
    tab = call(d, g, "ferb_browser.open", url=f"{SITE}/framed")["content"]["tab"]
    snap = call(d, g, "ferb_browser.snapshot", tab=tab)["content"]
    r = call(d, g, "ferb_browser.fill_secret", tab=tab, element=el(snap, sensitive=True), secret=REF.key)
    assert r["is_error"] and "isn't allowed" in r["content"]["error"] and drv.value(tab, 0) == ""


def test_a_form_submit_on_a_site_the_owner_did_not_name_asks_but_a_search_runs(bd: Any) -> None:
    """A search is navigation: it just runs. A form's submit button is a send: on a site the owner never named it
    asks them, and a no means nothing is posted."""
    from tests.conftest import Answers
    d, drv = bd
    g = task(d, "r", Identity.FERB, FERB_SCOPES, "look up the weather")
    tab = call(d, g, "ferb_browser.open", url=f"{SITE}/search")["content"]["tab"]
    snap = call(d, g, "ferb_browser.snapshot", tab=tab)["content"]
    q = el(snap, form="search", role="searchbox")
    assert not call(d, g, "ferb_browser.act", tab=tab, element=q, action="type", text="weather")["is_error"]
    assert not call(d, g, "ferb_browser.act", tab=tab, element=q, action="press", text="Enter")["is_error"]
    call(d, g, "ferb_browser.navigate", tab=tab, url=f"{SITE}/search")
    snap = call(d, g, "ferb_browser.snapshot", tab=tab)["content"]
    r = call(d, g, "ferb_browser.act", tab=tab, element=el(snap, label="Join"), action="click", effect="edit")
    assert r["is_error"] and 'effect "send"' in r["content"]["error"]           # the floor: a form submit is a send
    no = Answers(d, granted=False)
    try:
        r = call(d, g, "ferb_browser.act", tab=tab, element=el(snap, label="Join"), action="click", effect="send",
                 why="joining the list")
    finally:
        no.stop()
    assert r["is_error"] and "declined" in r["content"]["error"] and len(no.asked) == 1
    assert "login.example.com" in no.asked[0].reason and "joining the list" in no.asked[0].reason
    assert [p for p in drv.posted if "Join" in p] == []


def test_stale_element_ids_are_refused(bd: Any) -> None:
    d, _ = bd
    g = task(d, "st", Identity.FERB, {"identity:browse"}, "sign in")
    tab = call(d, g, "ferb_browser.open", url=f"{SITE}/login")["content"]["tab"]
    old = call(d, g, "ferb_browser.snapshot", tab=tab)["content"]
    call(d, g, "ferb_browser.snapshot", tab=tab)
    r = call(d, g, "ferb_browser.act", tab=tab, element=el(old, role="link"), action="click")
    assert r["is_error"] and "older snapshot" in r["content"]["error"]


def test_tabs_are_private_to_their_task_and_close_with_it(bd: Any) -> None:
    d, drv = bd
    a = task(d, "ta", Identity.FERB, {"identity:browse"}, None)
    b = task(d, "tb", Identity.FERB, {"identity:browse"}, None)
    tab = call(d, a, "ferb_browser.open", url=f"{SITE}/login")["content"]["tab"]
    r = call(d, b, "ferb_browser.snapshot", tab=tab)
    assert r["is_error"] and "no tab" in r["content"]["error"]
    d.run(d.daemon._task_end("ta"))
    assert not drv.running and call(d, a, "ferb_browser.snapshot", tab=tab)["is_error"]


def test_stop_closes_ferbs_browser(bd: Any) -> None:
    d, drv = bd
    g = task(d, "sp", Identity.FERB, {"identity:browse"}, None)
    call(d, g, "ferb_browser.open", url=f"{SITE}/login")
    assert drv.running
    c = Client(d.sock)
    try:
        c.call("stop.engage")
        end = time.monotonic() + 5
        while drv.running and time.monotonic() < end:
            time.sleep(0.05)
        assert not drv.running
    finally:
        c.call("stop.lift")
        c.close()


def test_local_and_non_http_urls_refused_before_the_driver(bd: Any) -> None:
    d, drv = bd
    g = task(d, "p", Identity.FERB, {"identity:browse"}, None)
    for url in ("http://127.0.0.1:9/", "file:///etc/passwd", "http://router.local/"):
        r = call(d, g, "ferb_browser.open", url=url)
        assert r["is_error"] and ("local" in r["content"]["error"] or "http" in r["content"]["error"])
    assert not drv.running                                                  # the driver never saw them
    for local in ("http://localhost:8080/", "http://192.168.1.1/", "http://10.0.0.2", "http://[::1]/"):
        with pytest.raises(Refused):
            check_url(local, allow_local=False)
    check_url("https://accounts.google.com/", allow_local=False)


def test_site_matching_covers_subdomains_only() -> None:
    sites = frozenset({"example.com", "https://www.shop.io/cart"})
    assert site_matches("example.com", sites) and site_matches("accounts.example.com", sites)
    assert site_matches("www.shop.io", sites) and site_matches("shop.io", sites)
    assert not site_matches("example.com.evil.io", sites) and not site_matches("badexample.com", sites)


def test_action_kinds_are_facts_about_the_element() -> None:
    def e(role: str, form: str | None = None, href: str | None = None, label: str = "x",
          input_type: str | None = None) -> PageElement:
        return PageElement("s1e1", "f", role, label, None, (0, 0, 1, 1), True, href=href, form=form,
                           input_type=input_type)
    assert act_kind("click", None, e("link", href="https://x")) == Kind.NAVIGATE
    assert act_kind("click", None, e("button", "search")) == Kind.SEARCH
    assert act_kind("click", None, e("button", "other")) == Kind.SUBMIT           # a form's button submits it
    assert act_kind("click", None, e("button", input_type="submit")) == Kind.CLICK   # no form: nothing to submit
    assert act_kind("click", None, e("button")) == Kind.CLICK                     # a plain button: a click
    assert act_kind("click", None, e("row", label="FMI assignment — new post")) == Kind.CLICK   # an email row
    assert act_kind("click", None, e("button", label="Send")) == Kind.SEND          # the floor words
    assert act_kind("click", None, e("button", label="Pay now")) == Kind.PAY
    assert act_kind("click", None, e("button", label="Delete")) == Kind.DELETE_PERMANENT
    assert act_kind("click", None, e("button", label="Archive")) == Kind.CLICK     # not on the floor
    assert act_kind("click", None, e("checkbox", "other")) == Kind.FILL_FORM
    assert act_kind("type", "hi", e("textbox", "search")) == Kind.SEARCH
    assert act_kind("type", "hi", e("textbox", "login")) == Kind.FILL_FORM
    assert act_kind("press", "Enter", e("textbox", "other")) == Kind.SUBMIT
    assert act_kind("press", "Enter", e("textbox", None)) == Kind.SUBMIT          # Return in a message box: a send
    assert act_kind("press", "Tab", e("textbox", "other")) == Kind.READ
    assert act_kind("click", None, None) == Kind.CLICK                             # unknown element: the driver refuses


def _nowhere(d: RunningDaemon, value: str) -> None:
    db = d.daemon.home / "ferb.db"
    for f in (db, db.with_name("ferb.db-wal")):
        if f.exists():
            assert value.encode() not in f.read_bytes(), f.name             # spans, journal, ledger
    logs = d.daemon.home / "logs"
    for f in logs.glob("*") if logs.exists() else []:
        assert value not in f.read_text(errors="ignore")
