"""Shared browser-test kit: a temporary keychain holding a test password (created and deleted here; the owner's
keychains and the search list are never touched), an Atom feed builder standing in for FERB's inbox, and an
in-memory identity driver that follows the driver contract (task-owned tabs, generation-scoped element ids,
origin check before the Keychain is read, codes straight from the feed into the field).

Real extension tests use disposable ChromeLab profiles; installed profiles and native-host settings stay untouched.
"""

import json
import secrets
import subprocess
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Callable, Iterator
from urllib.parse import urlsplit

import pytest

from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID
from ferb.cli.client import Client
from ferb.core.errors import Refused, StaleRef
from ferb.core.types import (ActionResult, BrowserAction, DriverFeatures, ElementRef, Identity, PageElement,
                             PageSnapshot, SecretRef, TabRef, Task, WaitCondition)
from ferb.identity.mailbox import GmailFeedMailbox
from ferb.identity.secrets import KeychainSecretStore, normalize_origin, origin_allowed
from ferb.orchestrator.tree import TaskRecord
from ferb.policy.targets import Request, Targets
from tests.conftest import RunningDaemon
from tests.fake_extension import FakeExtension

PASSWORD = "Corr3ct-Horse-" + secrets.token_hex(4)
REF = SecretRef("test/site/password")
ADDRESS = "ferb@test.example"


def feed(entries: list[tuple[str, str, str, datetime]], address: str = ADDRESS) -> str:
    items = "".join(f"<entry><title>{t}</title><summary>{s}</summary><issued>{d.strftime('%Y-%m-%dT%H:%M:%SZ')}"
                    f"</issued><id>tag:mail,{secrets.token_hex(4)}</id><author><email>{a}</email></author></entry>"
                    for a, t, s, d in entries)
    return (f'<?xml version="1.0" encoding="UTF-8"?><feed version="0.3" xmlns="http://purl.org/atom/ns#">'
            f"<title>Gmail - Inbox for {address}</title><fullcount>{len(entries)}</fullcount>{items}</feed>")


@pytest.fixture(scope="module")
def keychain(tmp_path_factory: pytest.TempPathFactory) -> Iterator[Path]:
    path = tmp_path_factory.mktemp("kc") / "ferb-test.keychain-db"
    before = subprocess.run(["security", "list-keychains"], capture_output=True, text=True).stdout
    kc_pass = secrets.token_hex(16)
    subprocess.run(["security", "create-keychain", "-p", kc_pass, str(path)], check=True)
    try:
        subprocess.run(["security", "unlock-keychain", "-p", kc_pass, str(path)], check=True)
        subprocess.run(["security", "add-generic-password", "-s", "ferb", "-a", REF.key, "-w", PASSWORD, str(path)],
                       check=True)
        yield path
    finally:
        subprocess.run(["security", "delete-keychain", str(path)], check=True)
        after = subprocess.run(["security", "list-keychains"], capture_output=True, text=True).stdout
        assert after == before, "temporary keychain left the search list changed"


# ---------- in-memory pages ----------

@dataclass
class El:
    role: str
    label: str = ""
    input_type: str | None = None
    href: str | None = None
    form: str | None = None
    secret: bool = False
    submits: str | None = None               # clicking / Enter posts the page's fields and goes here
    origin: str | None = None                # frame origin when inside an iframe
    value: str = ""


def pages() -> dict[str, tuple[str, str, list[El]]]:
    return {
        "/login": ("Login", "Sign in", [El("textbox", form="login"), El("textbox", input_type="password",
                                                                         form="login", secret=True),
                                        El("button", "Sign in", form="login", submits="/welcome"),
                                        El("link", "Search page", href="/search")]),
        "/search": ("Search", "", [El("searchbox", form="search", submits="/results"),
                                   El("button", "Go", form="search", submits="/results"),
                                   El("textbox", "email", form="other"),
                                   El("button", "Join", form="other", submits="/joined")]),
        "/otp": ("Code", "", [El("textbox", input_type="text", form="other", secret=True),
                              El("button", "Verify", form="other", submits="/welcome")]),
        "/framed": ("Framed", "", [El("textbox", input_type="password", secret=True, origin="https://evil.example")]),
        "/welcome": ("Welcome", "Welcome back", []),
        "/results": ("Results", "results page", []),
    }


@dataclass
class _Tab:
    ref: TabRef
    url: str
    els: list[El]
    title: str = ""
    text: str = ""
    owner: str | None = None
    snap_gen: int = 0


@dataclass
class FakeIdentityDriver:
    secrets: KeychainSecretStore
    add_secret_value: Callable[[str], None]
    feed_xml: str = field(default_factory=lambda: feed([]))
    name: str = "ext:chrome"
    identity: Identity = Identity.FERB
    features: DriverFeatures = DriverFeatures(True, True, True, True, False)
    running: bool = False
    posted: list[dict[str, str]] = field(default_factory=list)
    used_otps: set[str] = field(default_factory=set)
    generation: int = 1
    _tabs: dict[str, _Tab] = field(default_factory=dict)
    _n: int = 0

    def _load(self, t: _Tab, url: str) -> None:
        title, text, els = pages().get(urlsplit(url).path, ("Blank", "", []))
        t.url, t.title, t.text, t.els, t.snap_gen = url, title, text, els, -1   # navigation: old ids stale

    async def tabs(self) -> list[TabRef]:
        return [t.ref for t in self._tabs.values()]

    async def open(self, url: str, background: bool = True) -> TabRef:
        self.running = True
        self._n += 1
        ref = TabRef("dev", "chrome", "Profile 7", self.generation, f"t{self._n}")
        t = _Tab(ref, url, [])
        self._load(t, url)
        self._tabs[ref.tab_id] = t
        return ref

    async def open_for(self, url: str, owner: str, profile: str | None = None, *,
                       window_id: int | None = None, allow_foreground: bool = False) -> TabRef:
        ref = await self.open(url)
        self.claim(ref, owner)
        return ref

    async def navigate(self, tab: TabRef, url: str) -> None:
        self._load(self._tab(tab), url)

    async def snapshot(self, tab: TabRef) -> PageSnapshot:
        t = self._tab(tab)
        t.snap_gen = max(t.snap_gen, 0) + 1
        els = [PageElement(f"s{t.snap_gen}e{i}", "f0", e.role, e.label,
                           ("filled" if e.value else "") if e.secret else e.value, (0, 0, 10, 10), True,
                           e.input_type, e.href, e.form, e.secret) for i, e in enumerate(t.els)]
        return PageSnapshot(tab, t.snap_gen, t.url, t.title, els, t.text)

    async def act(self, el: ElementRef, action: BrowserAction, intent_id: str) -> ActionResult:
        t, e = self._resolve(el)
        if action.kind == "type":
            if e.secret:
                raise Refused("that's a password/code field: use fill_secret or fill_otp")
            e.value = action.text or ""
            return ActionResult(True, "typed")
        if action.kind == "click" and e.href:
            self._load(t, urlsplit(t.url)._replace(path=e.href).geturl())
            return ActionResult(True, "clicked")
        if action.kind in ("click", "press") and (action.text or "Enter") == "Enter":
            target = e.submits or next((x.submits for x in t.els if x.form == e.form and x.submits), None)
            if target:
                self.posted.append({x.label or x.role: x.value for x in t.els if x.form == e.form})
                self._load(t, urlsplit(t.url)._replace(path=target).geturl())
            return ActionResult(True, "done")
        return ActionResult(True, action.kind)

    async def wait(self, tab: TabRef, until: WaitCondition, timeout: float) -> bool:
        t = self._tab(tab)
        return until.kind == "load" or (until.value or "") in (t.text if until.kind == "text" else t.url)

    async def screenshot(self, tab: TabRef) -> bytes:
        return b""

    async def close(self, tab: TabRef) -> None:
        self._tabs.pop(tab.tab_id, None)

    def claim(self, tab: TabRef, owner: str) -> None:
        self._tab(tab).owner = owner

    def owned_by(self, tab_id: str, owner: str) -> TabRef:
        t = self._tabs.get(tab_id)
        if t is None or t.owner != owner:
            raise Refused(f"no tab {tab_id} open for this task")
        return t.ref

    def tab_url(self, tab_id: str) -> str | None:
        t = self._tabs.get(tab_id)
        return t.url if t else None

    def element_info(self, tab_id: str, element_id: str) -> PageElement | None:
        t = self._tabs.get(tab_id)
        if t is None or not element_id.startswith(f"s{t.snap_gen}e"):
            return None
        i = int(element_id.split("e")[-1])
        e = t.els[i] if i < len(t.els) else None
        return PageElement(element_id, "f0", e.role, e.label, None, (0, 0, 1, 1), True, e.input_type, e.href,
                           e.form, e.secret) if e else None

    def element_ref(self, tab: TabRef, element_id: str) -> ElementRef:
        gen = int(element_id[1:].split("e")[0]) if element_id.startswith("s") else -1
        return ElementRef(tab, "f0", gen, element_id)

    def _tab(self, ref: TabRef) -> _Tab:
        t = self._tabs.get(ref.tab_id)
        if t is None or ref.generation != self.generation:
            raise StaleRef(f"tab {ref.tab_id} is gone")
        return t

    def _resolve(self, el: ElementRef) -> tuple[_Tab, El]:
        t = self._tab(el.tab)
        i = int(el.element_id.split("e")[-1])
        if el.snapshot_gen != t.snap_gen or i >= len(t.els):
            raise StaleRef(f"element {el.element_id} is from an older snapshot; take a new one")
        return t, t.els[i]

    def _origin(self, t: _Tab, e: El) -> str:
        return normalize_origin(e.origin or t.url)

    async def fill_secret(self, el: ElementRef, ref: SecretRef, intent_id: str = "") -> ActionResult:
        if not self.secrets.has(ref):
            raise Refused(f"no secret {ref.key} is set up")
        t, e = self._resolve(el)
        if not origin_allowed(self._origin(t, e), self.secrets.spec(ref)):   # before the Keychain is read
            raise Refused(f"refused: this field's page ({self._origin(t, e)}) isn't allowed for {ref.key}")
        value = await self.secrets._reveal(ref)
        self.add_secret_value(value)
        e.value = value
        return ActionResult(True, f"ok ({ref.key})")

    async def otp_candidates(self, el: ElementRef, ref: SecretRef) -> dict[str, Any]:
        t,e=self._resolve(el)
        if not origin_allowed(self._origin(t,e),self.secrets.spec(ref)):
            raise Refused("origin isn't allowed")
        async def fetch(url: str) -> str:
            return self.feed_xml
        self.mailbox=GmailFeedMailbox(fetch,ADDRESS)
        self.mailbox.used=self.used_otps
        return await self.mailbox.candidates()

    async def fill_otp(self, el: ElementRef, ref: SecretRef, candidate: str, intent_id: str = "") -> ActionResult:
        t,e=self._resolve(el)
        if not origin_allowed(self._origin(t,e),self.secrets.spec(ref)):
            raise Refused("origin isn't allowed")
        try:
            found=self.mailbox.take(candidate)
        except ValueError as error:
            return ActionResult(False,str(error))
        self.add_secret_value(found.code)
        e.value=found.code
        self.used_otps.add(found.message_id)
        return ActionResult(True,"ok (explicit candidate)")

    async def release(self, owner: str) -> None:
        for tid in [k for k, t in self._tabs.items() if t.owner == owner]:
            self._tabs.pop(tid)
        if not self._tabs:
            self.running = False

    async def shutdown(self) -> None:
        self._tabs.clear()
        self.running = False

    def value(self, tab_id: str, index: int) -> str:            # test-only peek at a field
        return self._tabs[tab_id].els[index].value


def fake_driver(keychain: Path, specs: dict[str, Any], scrubbed: list[str]) -> FakeIdentityDriver:
    return FakeIdentityDriver(KeychainSecretStore(lambda: specs, keychain), scrubbed.append)


PROFILES = {"Default": ("Personal", "me@gmail.com"), "Profile 1": ("Work", "me@work.com"),
            "Profile 7": ("FERB", "ferb@gmail.com")}


def chrome_data(root: Path) -> Path:
    d = root / "Chrome"
    d.mkdir(parents=True)
    cache = {f: {"name": n, "user_name": e} for f, (n, e) in PROFILES.items()}
    (d / "Local State").write_text(json.dumps({"profile": {"info_cache": cache, "last_used": "Profile 1"}}))
    for folder in PROFILES:                                   # the FERB extension is loaded in each (like real ones)
        (d / folder).mkdir(exist_ok=True)
        (d / folder / "Secure Preferences").write_text(json.dumps(
            {"extensions": {"settings": {FERB_EXTENSION_ID: {"location": 4}}}}))
    return d


@pytest.fixture
def bd(daemon: RunningDaemon) -> Iterator[tuple[RunningDaemon, list[list[str]]]]:
    d = daemon.daemon
    launched: list[list[str]] = []
    d.browser_data_dirs = {"chrome": chrome_data(d.home.parent)}
    d.browser_launch = launched.append
    d.browser_default = lambda: "chrome"                                      # last used in the fake: Profile 1
    d.pair_load_wait_s = 0.0
    d.router.launch = launched.append                                         # type: ignore[union-attr]
    d.router.wake = None                                                      # type: ignore[union-attr]
    d.router.is_running = lambda browser: False                               # type: ignore[union-attr]
    d.config.set("identity", "browser", "chrome")
    d.config.set("identity", "profile", "Profile 7")
    yield daemon, launched


def pair(d: RunningDaemon, launched: list[list[str]], name: str) -> FakeExtension:
    ext = FakeExtension(d.daemon.home)
    ext.connect()
    c = Client(d.sock, timeout=30)
    try:
        import threading
        out: dict[str, Any] = {}
        t = threading.Thread(target=lambda: out.update(c.call("browsers.pair", {"profile": name, "timeout_s": 10})))
        n = len(launched)
        t.start()
        ext.wait_for(lambda: len(launched) > n and "pair.html#token=" in launched[-1][-1])
        ext.pair(launched[-1][-1].split("#token=")[1])
        t.join(15)
    finally:
        c.close()
    assert out.get("paired") is True, out
    assert launched[-1][1].startswith("--profile-directory=")
    return ext


def task(d: RunningDaemon, tid: str, scopes: set[str], words: Any = None, sites: frozenset[str] = frozenset()) -> Any:
    """A running owner task; the sites (and `words`, when a string) count as named by the owner for its request."""
    t = Task(tid, None, "g", "dev", Identity.OWNER, scopes, "worker", f"req-{tid}", [],
             d.daemon.clock.now() + timedelta(minutes=10))
    own = " ".join([words] if isinstance(words, str) else []) + " " + " ".join(sorted(sites))

    async def go() -> Any:
        root = d.daemon.spans.start("task", task_id=t.id, input={})
        d.daemon.tasks.add(TaskRecord(t, d.daemon.clock.now(), root.id))
        d.daemon.state.requests.record(Request(f"req-{tid}", own.strip(), Targets.of(sites=sites)))
        return d.daemon.tokens.validate(d.daemon.tokens.issue(t.id, f"w-{tid}", scopes, Identity.OWNER, ttl_s=600))
    return d.run(go())


def call(d: RunningDaemon, grant: Any, name: str, **args: Any) -> dict[str, Any]:
    if name == "browser.open":
        args.setdefault("profile", "Work")  # this fixture's test target, explicitly supplied to the driver
    return d.run(d.daemon.gateway.call(grant, name, args), timeout=30)          # type: ignore[no-any-return]


OWNER = {"browser:read", "browser:act"}
