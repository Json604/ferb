"""Group B, area `browsers` (ferb/daemon/areas/browsers.py): the browser, pairing and identity-profile calls with Full
Disk Access granted and invented browser folders, recorded from the real Python daemon (rust/fixtures/daemon/make.py's
harness through common.py).

    cd <repo> && uv run python rust/fixtures/daemon-b/browsers.py

Writes browsers.json. Each scenario's `setup` holds what a Rust test rebuilds: config.toml's text, every file of the
invented browser folders (`files`, relative to the browser support folder), the pairing rows (`installs`), the
extension hub's state (`bridges`, `unpaired`). `launched` lists every browser command FERB ran (the launcher is
faked and, where a step says so, answers the pairing page as the extension would: `answers`); `bridge_calls` what FERB
sent a connected profile; `after` the config text, the browser folders' files and the pairing rows at the end.

Stand-ins beyond the harness's: the pairing tokens (`pairing.secrets`: 24 bytes of one counter value each, as a Rust
`Random` gives them), the wake page's address, `pair_load_wait_s` 0, the handoff id (`uuid.uuid4`), the router's tab
look, the notifier and the supervisor's activity clock (both recorded)."""

import asyncio
import base64
import json
import sys
from pathlib import Path
from types import SimpleNamespace
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common  # noqa: E402

H = common.load("browsers")

import ferb.act.browser.pairing as pairing  # noqa: E402
import ferb.act.browser.profiles as browser_profiles  # noqa: E402
from ferb.act.browser.autoinstall import STORE_EXTENSION_ID  # noqa: E402
from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID  # noqa: E402

WAKE = "http://127.0.0.1:9/wake#w1"
SUPPORT_REL = "user/Library/Application Support"       # under the scenario's root (its own invented HOME)
DIRS = {"chrome": "Google/Chrome", "brave": "BraveSoftware/Brave-Browser"}


class Tokens:
    """`secrets.token_urlsafe(24)`: 24 bytes of the counter's value (1, 2, ...), urlsafe base64 without padding."""
    n = 0

    def token_urlsafe(self, n: int) -> str:
        Tokens.n += 1
        return base64.urlsafe_b64encode(bytes([Tokens.n]) * n).rstrip(b"=").decode()

    @staticmethod
    def token_bytes(n: int) -> bytes:
        return bytes(n)


H.stub(pairing, "secrets", Tokens(), "pairing tokens: a counter (24 bytes of one value each)")

LAUNCHED: list[list[str]] = []
STATE: dict[str, Any] = {"answer": False, "installs": 0, "d": None, "bridge_calls": []}


def record_launch(argv: list[str]) -> None:
    """The browser launcher (both `browser_launch` and `open_known_profile`'s default): records the command; while
    `answer` is on, a pairing page opened in a profile is answered as the extension would: its token pairs a new
    install and that profile's bridge connects (version 0.19.5, one more hello)."""
    LAUNCHED.append(list(argv))
    url = argv[-1]
    d = STATE["d"]
    if not STATE["answer"] or "/pair.html#token=" not in url or d is None:
        return
    token = url.split("#token=", 1)[1]
    browser, folder, _identity, _expires = d.pairings._tokens[token]
    STATE["installs"] += 1
    install = f"inst-pair-{STATE['installs']:04d}"
    verdict = d.pairings._pair(SimpleNamespace(install_id=install, body=lambda: {"token": token}))
    assert verdict.accepted, verdict
    connect(d, browser, folder, "0.19.5")


def connect(d: Any, browser: str, folder: str, version: str) -> Any:
    """That profile's bridge with a live connection (what an extension's accepted hello leaves)."""
    br = d.hub.bridge(browser, folder)
    br.conn = SimpleNamespace(live=True)
    br.version = version
    br.hellos += 1
    name = f"{browser}/{folder}"

    async def call(op: str, args: dict[str, Any], timeout: float = 30, cid: Any = None) -> Any:
        STATE["bridge_calls"].append([name, op, args, timeout])
        return {}
    br.call = call
    return br


H.stub(browser_profiles, "_detached", record_launch, "no browser is ever launched: the command is recorded")
_REAL_OPEN = browser_profiles.open_known_profile


def _open_known(browser: str, folder: str, url: Any, known: Any, launch: Any = None, **kw: Any) -> None:
    """`open_known_profile` always through the recording launcher: its `launch` default was bound to the real
    `_detached` when the module was imported, so stubbing `_detached` alone doesn't reach a call that passes none
    (`_browsers_pair`'s chrome://extensions page)."""
    _REAL_OPEN(browser, folder, url, known, launch=record_launch, **kw)


H.stub(browser_profiles, "open_known_profile", _open_known, "every browser command goes to the recording launcher")


def prepare(d: Any) -> None:
    support = d.home.parent / SUPPORT_REL
    d.browser_support = support
    d.browser_data_dirs = {b: support / rel for b, rel in DIRS.items()}
    d.browser_launch = record_launch
    d.pair_load_wait_s = 0.0
    d.handoff_poll_s = 0.0
    d.wake.url = lambda: WAKE
    STATE["d"] = d


# ---------- the invented browser folders ----------

def on(version: str) -> dict[str, Any]:
    return {"location": 1, "path": f"{STORE_EXTENSION_ID}/{version}_0"}


CHROME: dict[str, Any] = {          # folder -> (name, signed-in account, FERB's extension entry or None, removed)
    "Default": ("Home", "owner@example.test", on("0.19.5"), False),
    "Profile 1": ("Work", "work@example.test", on("0.19.5"), False),
    "Profile 2": ("Spare", "", {**on("0.19.5"), "disable_reasons": [4]}, False),
    "Profile 3": ("Old", "", on("0.15.13"), False),
    "Profile 4": ("Removed", "", None, True),
    "Profile 5": ("FERB", "ferb@example.test", on("0.19.5"), False),
    "Profile 6": ("Stale", "stale@example.test", on("0.19.5"), False),
    "Profile 7": ("Mixed", "ferb@example.test", {"location": 4, "path": "/x/unpacked", "state": 0}, False),
}
BRAVE: dict[str, Any] = {"Default": ("Brave Me", "", None, False)}
ACCOUNTS = {"Profile 7": ["ferb@example.test", "other@example.test"]}


def browser_files(profiles: dict[str, Any], ext_id: str) -> dict[str, str]:
    out = {"Local State": json.dumps({"profile": {"info_cache": {
        f: {"name": name, "user_name": user} for f, (name, user, _e, _r) in profiles.items()}}})}
    for f, (_name, user, entry, removed) in profiles.items():
        ext: dict[str, Any] = {"settings": {ext_id if entry and entry.get("location") == 1 else FERB_EXTENSION_ID:
                                            entry} if entry is not None else {}}
        if removed:
            ext["external_uninstalls"] = [STORE_EXTENSION_ID]
        prefs: dict[str, Any] = {"extensions": ext}
        if f in ACCOUNTS:
            prefs["account_info"] = [{"email": e} for e in ACCOUNTS[f]]
        elif user:
            prefs["account_info"] = [{"email": user}]
        out[f"{f}/Preferences"] = json.dumps(prefs)
    return out


def write_browsers(support: Path) -> dict[str, str]:
    files: dict[str, str] = {}
    for browser, profiles in (("chrome", CHROME), ("brave", BRAVE)):
        for rel, text in browser_files(profiles, STORE_EXTENSION_ID).items():
            files[f"{DIRS[browser]}/{rel}"] = text
    for rel, text in files.items():
        (support / rel).parent.mkdir(parents=True, exist_ok=True)
        (support / rel).write_text(text)
    return files


def tree(support: Path) -> dict[str, str]:
    return {str(p.relative_to(support)): p.read_text() for p in sorted(support.rglob("*")) if p.is_file()}


def installs(d: Any) -> list[list[Any]]:
    rows = H.on_loop(H_HOST[0], lambda: d.db.conn.execute(
        "SELECT install_id, browser, folder, identity, ext_version FROM ext_installs ORDER BY browser, folder")
        .fetchall())
    return [list(r) for r in rows]


H_HOST: list[Any] = []


def insert(d: Any, install: str, browser: str, folder: str, identity: str, version: str) -> None:
    H.on_loop(H_HOST[0], lambda: d.db.conn.execute(
        "INSERT INTO ext_installs (install_id, browser, folder, identity, key, paired_at, ext_version) "
        "VALUES (?, ?, ?, ?, x'00', '2026-01-01T09:00:00+00:00', ?)", (install, browser, folder, identity, version)))


def config_text(d: Any) -> str:
    path = d.home / "config.toml"
    return path.read_text() if path.exists() else ""


# ---------- scenarios ----------

def with_access() -> dict[str, Any]:
    """Full Disk Access, Chrome (eight profiles in every state) and Brave (one without FERB): list, installed, the
    native-host files, Pair all and its stream, pairing one profile (answered, unanswered, off, missing, too old, a bad
    timeout), FERB's own profile found, chosen, paired and cleared, Add to Chrome, unpair, a handoff."""
    Tokens.n = 0
    LAUNCHED.clear()
    STATE.update(answer=False, installs=0, bridge_calls=[])
    scn = common.BScenario("with_access", with_access.__doc__ or "", ["ferb/daemon/areas/browsers.py"],
                           extra={"data_access": lambda: True}, before_start=[prepare])
    d = scn.d
    H_HOST[:] = [scn.host]
    support = d.browser_support
    files = write_browsers(support)
    insert(d, "inst-default", "chrome", "Default", "owner", "0.19.5")
    insert(d, "inst-stale", "chrome", "Profile 6", "owner", "0.1.0")
    H.on_loop(scn.host, lambda: connect(d, "chrome", "Default", "0.19.5"))

    def unpaired() -> None:
        d.hub.unpaired["inst-unpaired"] = "brave"
        d.hub.unpaired_version["inst-unpaired"] = "0.19.4"
    H.on_loop(scn.host, unpaired)
    scn.setup = {"config": config_text(d), "files": files, "installs": installs(d),
                 "bridges": [["chrome", "Default", "0.19.5", 1]], "unpaired": [["inst-unpaired", "brave", "0.19.4"]]}
    c = scn.conn("app")
    scn.calls(c, ("browsers.list", {}),
              ("browsers.installed", {"profile": "Work"}), ("browsers.installed", {"profile": "chrome/Profile 4"}),
              ("browsers.installed", {"profile": "Brave Me"}), ("browsers.installed", {"profile": "Nobody"}),
              ("browsers.installed", {"profile": "chrome", "browser": "chrome"}),
              ("setup.browsers", {}), ("setup.browsers", {}),
              ("identity.find", {"address": "FERB@example.test"}),
              ("identity.choose", {"address": "ferb@example.test", "browser": "chrome", "folder": "Default"}),
              ("identity.choose", {"address": "ferb@example.test", "browser": "chrome", "folder": "Profile 7"}),
              ("identity.choose", {"address": "ferb@example.test", "browser": "opera", "folder": "Default"}),
              ("identity.choose", {"address": "ferb@example.test", "browser": "chrome", "folder": "Profile 5"}),
              ("identity.find", {}),
              ("setup.browsers_pair_all", {"timeout_s": 0}))
    s = scn.s.stream(c, "setup.browsers_pairing", {"timeout_s": 0})
    scn.s.end(s)
    s = scn.s.stream(c, "setup.browsers_pairing", {"timeout_s": "soon"})
    scn.s.end(s)
    scn.calls(c, ("browsers.pair", {"browser": "chrome", "folder": "Profile 2", "timeout_s": 0}),
              ("browsers.pair", {"profile": "Brave Me"}),
              ("browsers.pair", {"profile": "FERB"}),
              ("browsers.pair", {"browser": "chrome", "folder": "Profile 3", "timeout_s": "soon"}),
              ("browsers.pair", {"profile": "Old", "timeout_s": 0}),
              ("setup.browser_pair", {"browser": "chrome", "folder": "Profile 1", "timeout_s": 0}),
              ("setup.browser_pair", {"browser": "chrome", "folder": "Profile 1", "timeout_s": [1]}),
              ("identity.pair", {"timeout_s": 0}),
              ("identity.pair", {"timeout_s": "soon"}))
    STATE["answer"] = True
    scn.calls(c, ("browsers.pair", {"browser": "chrome", "folder": "Profile 1", "timeout_s": 5}),
              ("identity.pair", {"timeout_s": 5}),
              ("identity.pair", {}),
              ("identity.find", {}),
              ("browsers.list", {}))
    STATE["answer"] = False
    STATE["attached"] = H.on_loop(scn.host, lambda: d.identity_browser is not None)
    scn.calls(c, ("setup.browser_open", {"browser": "chrome", "folder": "Profile 4"}),
              ("setup.browser_open", {"browser": "chrome", "folder": "Profile 9"}),
              ("browsers.unpair", {"profile": "Spare"}), ("browsers.unpair", {"profile": "Work"}),
              ("setup.browsers", {}),
              ("identity.choose", {"clear": True}),
              ("identity.find", {}))
    handoff = record_handoff(scn, c)
    H.settle(scn.host)
    after = {"config": config_text(d), "files": tree(support), "installs": installs(d),
             "identity_attached": [STATE["attached"], d.identity_browser is not None]}
    out = scn.finish()
    out["launched"] = list(LAUNCHED)
    out["bridge_calls"] = STATE["bridge_calls"]
    out["handoff"] = handoff
    out["after"] = after
    return json.loads(H.fx(json.dumps(out)))


def record_handoff(scn: Any, c: Any) -> dict[str, Any]:
    """A human check handed to the owner (`_handoff`, the router's callback) while browser.show_handoff is called: the
    tab comes forward, then (the tab can't) the profile's window opens; the page lets the owner through on two looks."""
    import uuid
    d = scn.d
    H.MP.setattr(uuid, "uuid4", lambda: SimpleNamespace(hex="abcdef0123456789abcdef0123456789"))
    looks: list[Any] = []
    shown: list[Any] = []
    notes: list[Any] = []
    waits: list[Any] = []
    go: dict[str, Any] = {}
    page = {"url": "https://www.example.test/check", "title": "Check"}

    async def shows(ref: Any, message: str) -> dict[str, Any]:
        looks.append([ref, message])
        if len(looks) == 1:
            return {"open": True, "shows": True, **page}
        await go["event"].wait()
        return {"open": True, "shows": False, **page}

    async def show_tab(ref: Any) -> bool:
        shown.append(ref)
        return len(shown) == 1

    async def notify(title: str, text: str, urgent: bool = False, **kw: Any) -> None:
        notes.append({"title": title, "text": text, "urgent": urgent, **kw})

    def setup() -> None:
        go["event"] = asyncio.Event()
        d.router.shows, d.router.show_tab = shows, show_tab
        d.router.tab_profile = lambda ref: ("chrome", "Default")
        d._notify = notify
        if d.supervisor is not None:
            d.supervisor.activity.waiting = lambda worker, on: waits.append([worker, on])
    H.on_loop(scn.host, setup)
    ctx = SimpleNamespace(task_id="task-1", worker_id="w-1")
    fut = asyncio.run_coroutine_threadsafe(d._handoff({"tab": 7}, "Verify you are human", ctx), scn.host.loop)
    scn.s.wait("the handoff's first look", lambda: len(looks) >= 1 and len(notes) == 1)
    H.settle(scn.host)
    scn.calls(c, ("browser.show_handoff", {"handoff_id": "abcdef012345"}),
              ("browser.show_handoff", {"handoff_id": "abcdef012345"}))
    scn.host.loop.call_soon_threadsafe(go["event"].set)
    result = fut.result(10)
    scn.calls(c, ("browser.show_handoff", {"handoff_id": "abcdef012345"}))
    return {"ref": {"tab": 7}, "message": "Verify you are human", "task_id": "task-1", "worker_id": "w-1",
            "result": result, "looks": looks, "shown": shown, "notify": notes, "waiting": waits}


def without_access() -> dict[str, Any]:
    """No Full Disk Access, one profile paired earlier (HAND-407: named by its folder), FERB's account set: the
    calls that read nothing, and clearing FERB's account."""
    Tokens.n = 0
    LAUNCHED.clear()
    STATE.update(answer=False, installs=0, bridge_calls=[])
    scn = common.BScenario("without_access", without_access.__doc__ or "", ["ferb/daemon/areas/browsers.py"],
                           before_start=[prepare])
    d = scn.d
    H_HOST[:] = [scn.host]
    write_browsers(d.browser_support)
    insert(d, "inst-old", "chrome", "Profile 9", "owner", "0.19.5")
    insert(d, "inst-ferb", "chrome", "Profile 5", "ferb", "")
    from ferb.daemon import settings
    H.on_loop(scn.host, lambda: settings.write_identity(d.config, "ferb@example.test", "chrome", "Profile 5"))
    H.on_loop(scn.host, lambda: settings.write(d.config, "browsing.owner_profile", "chrome/Profile 9"))
    scn.setup = {"config": config_text(d), "files": {}, "installs": installs(d), "bridges": [], "unpaired": []}
    c = scn.conn("app")
    scn.calls(c, ("browsers.list", {}), ("browsers.pair", {"profile": "Profile 9"}),
              ("browsers.unpair", {"profile": "Profile 9"}), ("browsers.installed", {"profile": "Profile 9"}),
              ("setup.browsers", {}), ("setup.browser_open", {"browser": "chrome", "folder": "Profile 9"}),
              ("identity.find", {"address": "ferb@example.test"}), ("identity.pair", {}),
              ("identity.pair", {"confirm": True}),
              ("identity.choose", {"address": "ferb@example.test", "browser": "chrome", "folder": "Profile 5"}),
              ("identity.choose", {"clear": True}), ("identity.find", {}), ("identity.pair", {}),
              ("setup.browsers_pair_all", {}))
    H.settle(scn.host)
    after = {"config": config_text(d), "installs": installs(d)}
    out = scn.finish()
    out["launched"] = list(LAUNCHED)
    out["bridge_calls"] = STATE["bridge_calls"]
    out["after"] = after
    return json.loads(H.fx(json.dumps(out)))


if __name__ == "__main__":
    common.run("browsers", "browsers.json", [with_access, without_access])
