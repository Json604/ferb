"""cdp_identity against real headless Chrome and local pages (ported from spike 3): CDP over a pipe with no port,
origin-bound fill_secret / fill_otp (safety test 21.4), stale element refs (21.13), sanitized snapshots (21.5).

A temporary keychain is created and deleted here; the login keychain and the search list are never touched.
http://localhost:P and http://127.0.0.1:P are different origins (and sites, so the iframe is out-of-process).
"""

import json
import subprocess
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Iterator

import pytest

from ferb.act.browser.cdp_identity import CdpIdentityDriver, Refused, StaleRef, check_url
from ferb.act.browser.snapshot import for_model
from ferb.core.types import BrowserAction, SecretRef, SecretSpec, WaitCondition
from ferb.identity.mailbox import Message, extract_code, parse_feed
from ferb.identity.secrets import KeychainSecretStore
from tests.browserkit import PASSWORD, REF, Loop, feed

@pytest.fixture(scope="module")
def drv(loop: Loop, keychain: Path, site: dict[str, Any], tmp_path_factory: pytest.TempPathFactory) \
        -> Iterator[tuple[CdpIdentityDriver, dict[str, SecretSpec], list[str]]]:
    udd = tmp_path_factory.mktemp("identity")
    (udd / "Default").mkdir()
    specs: dict[str, SecretSpec] = {}
    scrubbed: list[str] = []
    store = KeychainSecretStore(lambda: specs, keychain)

    async def make() -> CdpIdentityDriver:
        return CdpIdentityDriver(udd, "dev", store, scrubbed.append, allow_local=True, headless=True,
                                 mailbox_address=lambda: "ferb@test.example", feed_url=f"{site['L']}/feed")
    d = loop.run(make())
    yield d, specs, scrubbed
    loop.run(d.shutdown())


def allow(specs: dict[str, SecretSpec], *origins: str) -> None:
    specs[REF.key] = SecretSpec(REF, list(origins), "test account")


def find(snapshot: Any, **match: Any) -> str:
    for e in snapshot.elements:
        if all(getattr(e, k) == v for k, v in match.items()):
            return str(e.id)
    raise AssertionError(f"no element {match} in {[(e.role, e.label) for e in snapshot.elements]}")


async def value_of(d: CdpIdentityDriver, el: Any) -> str:
    t, session, node, _ = d._resolve(el)
    return str(await d._call(t, session, el.frame_id, node, "function() { return this.value; }"))


def test_pipe_only_no_tcp_port(loop: Loop, drv: Any, site: dict[str, Any]) -> None:
    d, _, _ = drv
    tab = loop.run(d.open(f"{site['L']}/login"))
    assert d.pipe is not None
    pids = subprocess.run(["pgrep", "-g", str(d.pipe.proc.pid)], capture_output=True, text=True).stdout.split()
    listening = subprocess.run(["lsof", "-a", "-nP", "-iTCP", "-sTCP:LISTEN", "-p", ",".join(pids)],
                               capture_output=True, text=True).stdout
    assert pids and listening == ""
    loop.run(d.close(tab))


def test_fill_secret_logs_in_and_the_value_never_leaves(loop: Loop, drv: Any, site: dict[str, Any]) -> None:
    d, specs, scrubbed = drv
    allow(specs, site["L"])
    tab = loop.run(d.open(f"{site['L']}/login"))
    s = loop.run(d.snapshot(tab))
    pw = d.element_ref(tab, find(s, input_type="password"))
    user = d.element_ref(tab, find(s, label="", input_type="text"))
    assert loop.run(d.act(user, BrowserAction("type", "ferb-test"), "i1")).ok
    r = loop.run(d.fill_secret(pw, REF))
    assert r.ok and PASSWORD not in r.detail and REF.key in r.detail
    assert loop.run(value_of(d, pw)) == PASSWORD
    assert scrubbed == [PASSWORD]                                   # handed to the redactor, nothing else
    s2 = loop.run(d.snapshot(tab))
    model_view = json.dumps(for_model(s2))
    assert PASSWORD not in model_view and '"filled": true' in model_view
    loop.run(d.act(d.element_ref(tab, find(s2, role="button")), BrowserAction("click"), "i2"))
    assert loop.run(d.wait(tab, WaitCondition("text", "Welcome back"), 10))
    assert f"password={PASSWORD.replace('-', '-')}" in site["posted"][-1]   # the site really got it
    loop.run(d.close(tab))


@pytest.mark.parametrize("page,allowed", [
    ("I/login", "L"),                 # lookalike: 127.0.0.1 vs allowed localhost
    ("L/login", "https://localhost"),  # scheme (and port) mismatch
    ("L/top", "L"),                    # the field is in a cross-origin iframe
])
def test_fill_secret_refuses_origins_not_allowed(loop: Loop, drv: Any, site: dict[str, Any], page: str,
                                                 allowed: str) -> None:
    d, specs, scrubbed = drv
    scrubbed.clear()
    allow(specs, site.get(allowed, allowed))
    tab = loop.run(d.open(site[page[0]] + page[1:]))
    s = loop.run(d.snapshot(tab))
    pw = d.element_ref(tab, find(s, input_type="password"))
    with pytest.raises(Refused, match="isn't allowed"):
        loop.run(d.fill_secret(pw, REF))
    assert scrubbed == []                                            # refused before the secret was fetched
    assert loop.run(value_of(d, pw)) == ""
    loop.run(d.close(tab))


def test_fill_into_an_allowed_cross_origin_iframe_works(loop: Loop, drv: Any, site: dict[str, Any]) -> None:
    """Control: the refusal above is the origin check, not failure to reach an out-of-process frame."""
    d, specs, _ = drv
    allow(specs, site["I"])
    tab = loop.run(d.open(f"{site['L']}/top"))
    s = loop.run(d.snapshot(tab))
    pw = d.element_ref(tab, find(s, input_type="password"))
    assert loop.run(d.fill_secret(pw, REF)).ok
    assert loop.run(value_of(d, pw)) == PASSWORD
    user = d.element_ref(tab, find(s, input_type="text"))           # trusted click lands inside the OOPIF
    loop.run(d.act(user, BrowserAction("type", "typed-in-frame"), "i3"))
    assert loop.run(value_of(d, user)) == "typed-in-frame"
    loop.run(d.close(tab))


def test_focus_stealing_cannot_redirect_the_secret(loop: Loop, drv: Any, site: dict[str, Any]) -> None:
    d, specs, _ = drv
    allow(specs, site["L"], site["I"])
    tab = loop.run(d.open(f"{site['L']}/steal_top"))
    s = loop.run(d.snapshot(tab))
    pw = d.element_ref(tab, find(s, input_type="password"))
    grab = d.element_ref(tab, find(s, input_type="text"))
    assert loop.run(d.fill_secret(pw, REF)).ok
    assert loop.run(value_of(d, pw)) == PASSWORD and loop.run(value_of(d, grab)) == ""
    loop.run(d.close(tab))


def test_unknown_secret_and_typing_into_secret_fields_refused(loop: Loop, drv: Any, site: dict[str, Any]) -> None:
    d, specs, _ = drv
    allow(specs, site["L"])
    tab = loop.run(d.open(f"{site['L']}/login"))
    s = loop.run(d.snapshot(tab))
    pw = d.element_ref(tab, find(s, input_type="password"))
    with pytest.raises(Refused, match="no secret"):
        loop.run(d.fill_secret(pw, SecretRef("test/other")))
    with pytest.raises(Refused, match="fill_secret"):
        loop.run(d.act(pw, BrowserAction("type", "guess"), "i4"))
    loop.run(d.close(tab))


def test_stale_refs_after_navigation_or_newer_snapshot(loop: Loop, drv: Any, site: dict[str, Any]) -> None:
    d, _, _ = drv
    tab = loop.run(d.open(f"{site['L']}/login"))
    s1 = loop.run(d.snapshot(tab))
    old = d.element_ref(tab, find(s1, role="link"))
    loop.run(d.snapshot(tab))
    with pytest.raises(StaleRef):                                    # a newer snapshot supersedes
        loop.run(d.act(old, BrowserAction("click"), "i5"))
    s3 = loop.run(d.snapshot(tab))
    link = d.element_ref(tab, find(s3, role="link"))
    loop.run(d.act(link, BrowserAction("click"), "i6"))               # navigates to /search
    assert loop.run(d.wait(tab, WaitCondition("url", "/search"), 10))
    with pytest.raises(StaleRef):
        loop.run(d.act(link, BrowserAction("click"), "i7"))
    loop.run(d.close(tab))
    with pytest.raises(StaleRef):
        loop.run(d.snapshot(tab))


def test_snapshot_marks_search_forms_and_drops_hidden_inputs(loop: Loop, drv: Any, site: dict[str, Any]) -> None:
    d, _, _ = drv
    tab = loop.run(d.open(f"{site['L']}/search"))
    s = loop.run(d.snapshot(tab))
    forms = {e.label or e.role: e.form for e in s.elements}
    assert forms["Go"] == "search" and forms["Join"] == "other"
    assert "HIDDEN-TOKEN" not in json.dumps(for_model(s))
    q = d.element_ref(tab, find(s, form="search", role="textbox"))
    loop.run(d.act(q, BrowserAction("type", "weather"), "i8"))
    loop.run(d.act(q, BrowserAction("press", "Enter"), "i9"))
    assert loop.run(d.wait(tab, WaitCondition("url", "q=weather"), 10))
    loop.run(d.close(tab))


def test_fill_otp_takes_one_fresh_code_once(loop: Loop, drv: Any, site: dict[str, Any]) -> None:
    d, specs, scrubbed = drv
    allow(specs, site["L"])
    now = datetime.now(timezone.utc)
    site["feed"] = feed([("noreply@site.example", "Your code", "Your sign-in code is 482913. It expires in 2025.", now),
                         ("old@site.example", "Old code", "Code 111111", now - timedelta(hours=2))])
    tab = loop.run(d.open(f"{site['L']}/otp"))
    s = loop.run(d.snapshot(tab))
    code = d.element_ref(tab, find(s, secret=True))
    r = loop.run(d.fill_otp(code, REF, now - timedelta(minutes=1), "site.example", timeout=1, poll=0.2))
    assert r.ok and "482913" not in r.detail and loop.run(value_of(d, code)) == "482913"
    assert "482913" in scrubbed
    assert "482913" not in json.dumps(for_model(loop.run(d.snapshot(tab))))
    s = loop.run(d.snapshot(tab))
    again = loop.run(d.fill_otp(d.element_ref(tab, find(s, secret=True)), REF, now - timedelta(minutes=1), None,
                                timeout=0.5, poll=0.2))
    assert not again.ok and "no new code" in again.detail             # a used message is never reused
    loop.run(d.close(tab))


def test_fill_otp_refuses_ambiguous_mail_and_disallowed_origins(loop: Loop, drv: Any, site: dict[str, Any]) -> None:
    d, specs, _ = drv
    now = datetime.now(timezone.utc)
    site["feed"] = feed([("x@site.example", "Codes", "Use 123456 or 654321", now)])
    allow(specs, site["L"])
    tab = loop.run(d.open(f"{site['L']}/otp"))
    s = loop.run(d.snapshot(tab))
    r = loop.run(d.fill_otp(d.element_ref(tab, find(s, secret=True)), REF, now - timedelta(minutes=1), None,
                            timeout=0.5, poll=0.2))
    assert not r.ok and "refusing to guess" in r.detail
    allow(specs, site["I"])
    with pytest.raises(Refused, match="isn't allowed"):
        loop.run(d.fill_otp(d.element_ref(tab, find(s, secret=True)), REF, now, None, timeout=0.5))
    loop.run(d.close(tab))


def test_urls_limited_to_http_and_public_hosts() -> None:
    for bad in ("file:///etc/passwd", "javascript:alert(1)", "chrome://settings", "data:text/html,x"):
        with pytest.raises(Refused, match="http"):
            check_url(bad, allow_local=True)
    for local in ("http://localhost:8080/", "http://127.0.0.1/", "http://192.168.1.1/", "http://10.0.0.2",
                  "http://[::1]/", "http://printer.local/"):
        with pytest.raises(Refused, match="local"):
            check_url(local, allow_local=False)
    check_url("https://accounts.google.com/", allow_local=False)


def test_code_extraction_and_feed_parsing() -> None:
    now = datetime.now(timezone.utc)
    msg = lambda s: Message("i", "a@b", "subj", s, now)             # noqa: E731
    assert extract_code(msg("Your code is 482913")) == "482913"
    assert extract_code(msg("G-482913 is your Google verification code")) == "482913"
    assert extract_code(msg("Welcome! Copyright 2026")) is None     # years aren't codes
    assert extract_code(msg("123456 or 654321")) is False
    title, msgs = parse_feed(feed([("A@B.example", "t", "s", now)]).decode())
    assert "ferb@test.example" in title and msgs[0].sender == "a@b.example"
    with pytest.raises(ValueError):
        parse_feed("<html>Sign in</html>")


def test_launch_works_when_the_parent_has_no_fd_3_or_4(tmp_path: Path) -> None:
    """Regression: pass_fds=(3, 4) failed with EBADF in a process without fds 3/4 (e.g. a fresh script)."""
    (tmp_path / "Default").mkdir()
    script = f"""
import asyncio, os, sys
for fd in (3, 4):
    try: os.close(fd)
    except OSError: pass
from pathlib import Path
from ferb.act.browser.cdp_identity import CdpIdentityDriver
from ferb.identity.secrets import KeychainSecretStore
async def main():
    d = CdpIdentityDriver(Path({str(tmp_path)!r}), "dev", KeychainSecretStore(lambda: {{}}), lambda v: None,
                          headless=True)
    await d._ensure()
    print("version" if d.running else "down")
    await d.shutdown()
asyncio.run(main())
"""
    r = subprocess.run([sys.executable, "-c", script], capture_output=True, text=True, timeout=60)
    assert r.stdout.strip() == "version", r.stderr[-500:]


def test_a_failed_load_leaves_no_tab_and_chrome_stops_when_idle(loop: Loop, drv: Any, site: dict[str, Any]) -> None:
    d, _, _ = drv
    with pytest.raises(Refused, match="couldn't load"):
        loop.run(d.open("http://localhost:1/unreachable"))
    assert loop.run(d.tabs()) == []
    loop.run(d.release("nobody"))
    assert not d.running


def test_a_signed_out_inbox_is_reported_not_raised(loop: Loop, drv: Any, site: dict[str, Any]) -> None:
    d, specs, _ = drv
    allow(specs, site["L"])
    tab = loop.run(d.open(f"{site['L']}/otp"))
    s = loop.run(d.snapshot(tab))
    d.feed_url = "http://localhost:1/feed"                               # the feed load is refused
    try:
        r = loop.run(d.fill_otp(d.element_ref(tab, find(s, secret=True)), REF, datetime.now(timezone.utc), None,
                                timeout=0.5, poll=0.2))
        assert loop.run(d.session_ok()) is False
    finally:
        d.feed_url = f"{site['L']}/feed"
    assert not r.ok and "signed out" in r.detail
    loop.run(d.close(tab))
