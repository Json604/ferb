"""Step 12c: FERB.app's screens and their daemon side (notes/daemon.md decisions 6-8, notes/storage.md decision 4).
FERB's self-description is built from code and config and stays the same between calls; the worker's lookup only
reads; what FERB keeps comes from the retention rule table; FERB's own account is written through the settings path;
a saved secret is deleted without its value ever coming back; the app restores a Trash move; the owner-data methods are
FERB.app's alone; uninstalling everything asks only in the app."""

import io
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.client import Client, RpcError
from ferb.daemon.server import APP_ONLY_REFUSED
from ferb.identity.owner_secrets import OwnerSecrets
from ferb.store import retention
from tests.browserkit import chrome_data
from tests.conftest import RunningDaemon, gateway_request, gateway_setup, gateway_tool_call
from tests.support.trash import use_trash


def _call(d: RunningDaemon, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
    c = Client(d.sock)
    try:
        return c.call(method, params or {})
    finally:
        c.close()


def _kind(d: RunningDaemon, kind: str) -> list[Any]:
    async def read() -> list[Any]:
        return list(d.daemon.journal.of_kind(kind))
    return d.run(read())        # type: ignore[no-any-return]


def _as_app(d: RunningDaemon) -> None:
    d.daemon.server.is_app = lambda token: token is not None        # FERB.app's code signature, stood in for


def _grants(d: RunningDaemon) -> None:
    d.daemon.permission_check = lambda: {"accessibility": "granted", "screen_recording": "granted",   # type: ignore
                                         "input_monitoring": "granted", "full_disk_access": "denied"}


# ---------- About FERB (decision 6) ----------

def test_the_about_summary_is_built_from_code_and_config_and_stays_the_same(daemon: RunningDaemon) -> None:
    import ferb
    d = daemon.daemon

    async def built() -> tuple[str, str, str, str]:
        return d._about_summary(), d._about_summary(), d._facts(), d.supervisor._system("worker.md")
    first, second, facts, system = daemon.run(built())
    assert first == second and ferb.__version__ in first
    assert d._models()["worker"].split()[-1] in first             # the model the config gives the worker
    assert first in facts and first in system                      # in the facts every role's instructions carry
    assert daemon.run(built())[3] == system                        # the cacheable instructions don't change by call
    d.config.set("runtimes", "worker", "claude")                   # the owner's change shows on the next build
    assert d._about_summary() != first and d._models()["worker"].startswith("Claude")


def test_the_lookup_tool_only_reads_and_hides_secrets(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    _grants(daemon)
    cap = d.gateway.caps["about.read"]
    assert cap.observation and cap.describe(cap.Input()).kind == "read"
    d.config.set("privacy", "exclude_apps", ["sk-canary-value-0123456789abcdef"])
    d._add_secret_value("sk-canary-value-0123456789abcdef")
    config_before = (d.home / "config.toml").read_bytes()
    grant, _ = gateway_setup(daemon, set(), gateway_request("what does FERB keep about me?"), [])
    out = gateway_tool_call(daemon, grant, "about.read")
    assert not out["is_error"]
    doc = out["content"]
    assert {"version", "models", "records", "settings", "health", "cost", "app"} <= doc.keys()
    assert "sk-canary-value-0123456789abcdef" not in str(doc)
    assert (d.home / "config.toml").read_bytes() == config_before
    one = gateway_tool_call(daemon, grant, "about.read", section="records")["content"]
    assert list(one) == ["records"]


def test_what_ferb_keeps_comes_from_the_retention_rule_table(daemon: RunningDaemon,
                                                            monkeypatch: pytest.MonkeyPatch) -> None:
    d = daemon.daemon
    _grants(daemon)
    d.config.set("retention", "trace_days", 12)
    doc = _call(daemon, "about.get")
    assert [r["records"] for r in doc["records"]] == [r.records for r in retention.RULES]
    traces = next(r for r in doc["records"] if r.get("setting") == "retention.trace_days")
    assert traces["days"] == 12 and d._windows()["trace_days"] == 12   # the hourly pass reads the same row
    added = retention.Rule("a record kind added later", retention.DB, "after a while", "extra_days",
                           ("retention", "extra_days"), 5)
    monkeypatch.setattr(retention, "RULES", (*retention.RULES, added))
    doc = daemon.run(d._about_document())
    assert doc["records"][-1]["records"] == "a record kind added later" and doc["records"][-1]["days"] == 5


def test_the_app_guide_becomes_the_self_descriptions_how_tos(daemon: RunningDaemon) -> None:
    _grants(daemon)
    _as_app(daemon)
    guide = {"howtos": [{"title": "Delete a saved card", "steps": "Settings → Data"}],
             "shortcuts": [{"action": "Open FERB", "keys": "Double-tap right Option"}],
             "places": {"privacy.exclude_apps": "Settings → Privacy → Apps FERB ignores"}, "other": "dropped"}
    assert _call(daemon, "about.guide", guide) == {"howtos": 1, "shortcuts": 1, "places": 1}
    doc = _call(daemon, "about.get")
    assert doc["app"]["howtos"] == guide["howtos"]
    assert next(s for s in doc["settings"] if s["key"] == "privacy.exclude_apps")["where"] == \
        "Settings → Privacy → Apps FERB ignores"


# ---------- FERB's own account (decision 7) ----------

def test_choosing_ferbs_profile_writes_config_through_the_settings_path(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    d.browser_data_dirs = {"chrome": chrome_data(d.home.parent)}
    _as_app(daemon)
    found = _call(daemon, "identity.find", {"address": "FERB@gmail.com"})
    assert [(c["folder"], c["only"]) for c in found["candidates"]] == [("Profile 7", True)]
    with pytest.raises(RpcError):                                  # a profile signed into another account
        _call(daemon, "identity.choose", {"address": "ferb@gmail.com", "browser": "chrome", "folder": "Default"})
    assert d.config.get("mailbox", "address") is None
    stored = _call(daemon, "identity.choose", {"address": "ferb@gmail.com", "browser": "chrome",
                                               "folder": "Profile 7"})
    assert stored == {"address": "ferb@gmail.com", "browser": "chrome", "profile": "Profile 7"}
    assert (d.config.get("mailbox", "address"), d.config.get("identity", "browser"),
            d.config.get("identity", "profile")) == ("ferb@gmail.com", "chrome", "Profile 7")
    assert any(e.data.get("key") == "identity" for e in _kind(daemon, "settings.changed"))

    async def paired() -> None:                                    # its profile paired as FERB: its tools attach
        d.use_identity_browser(d.ferb_identity)
    daemon.run(paired())
    assert "ferb_browser.open" in d.gateway.caps
    _call(daemon, "identity.choose", {"clear": True})
    assert d.config.get("mailbox", "address") is None and d.config.get("identity", "profile") is None
    # past bug (12e review): the provider stayed and FERB's browser tools stayed registered
    assert d.config.get("mailbox", "provider") is None and d.config.get("identity", "browser") is None
    assert d.identity_browser is None and not any(n.startswith("ferb_browser.") for n in d.gateway.caps)


def test_without_its_own_profile_ferb_browses_in_the_profile_the_owner_picked() -> None:
    from ferb.act.browser.pairing import Binding
    from ferb.act.browser.profiles import BrowserProfile
    from ferb.act.browser.resolver import NeedsOwner, Resolver
    from ferb.core.types import Identity
    profiles = [BrowserProfile("chrome", "Default", "Personal", "me@gmail.com"),
                BrowserProfile("chrome", "Profile 1", "Work", "me@work.com")]
    bindings = [Binding("i1", "chrome", "Default", Identity.OWNER), Binding("i2", "chrome", "Profile 1", Identity.OWNER)]
    picked = Resolver(lambda: profiles, lambda: bindings, lambda: None, owner_default=lambda: ("chrome", "Profile 1"))
    assert (picked.choose(None).browser, picked.choose(None).folder) == ("chrome", "Profile 1")
    with_own = Resolver(lambda: profiles, lambda: bindings, lambda: ("chrome", "Profile 7"),
                        owner_default=lambda: ("chrome", "Profile 1"))
    with pytest.raises(NeedsOwner):                                # FERB's own profile exists: nothing is assumed
        with_own.choose(None)


# ---------- saved secrets and cards (decision 8) ----------

def _fake_keychain() -> tuple[dict[str, str], Any, Any]:
    values: dict[str, str] = {}

    def keychain(handle: str, value: str | None = None) -> str | None:
        if value is not None:
            values[handle] = value
        return values.get(handle)

    def delete(account: str) -> bool:
        return values.pop(account, None) is not None
    return values, keychain, delete


def test_delete_removes_the_keychain_item_and_never_returns_its_value(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    values, keychain, delete = _fake_keychain()
    store = OwnerSecrets(d.home, keychain, delete)
    store.remember("password", "https://shop.example", "hunter2-canary")
    store.remember("permit number", "https://shop.example", "4111111111111111", kind="card_number")
    d._owner_secrets = lambda: OwnerSecrets(d.home, keychain, delete)     # type: ignore[method-assign]
    _as_app(daemon)
    listed = _call(daemon, "secrets.saved")
    assert {(e["name"], e["kind"]) for e in listed["entries"]} == {("password", "secret"), ("permit number", "card_number")}
    assert "hunter2" not in str(listed) and "4111" not in str(listed)
    out = _call(daemon, "secrets.delete", {"place": "https://shop.example", "name": "permit number"})
    assert out == {"place": "https://shop.example", "name": "permit number", "deleted": True}
    assert "4111111111111111" not in values.values() and "hunter2-canary" in values.values()
    assert [e["name"] for e in _call(daemon, "secrets.saved")["entries"]] == ["password"]
    with pytest.raises(RpcError):
        _call(daemon, "secrets.delete", {"place": "https://shop.example", "name": "permit number"})
    rows = _kind(daemon, "secret.deleted")
    assert rows and "4111" not in str([r.data for r in rows])


# ---------- restore (decision 8) ----------

def test_the_app_lists_and_restores_a_trash_move(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    bin_ = daemon.daemon.home.parent / ".Trash"
    bin_.mkdir(parents=True, exist_ok=True)
    use_trash(monkeypatch, bin_)
    ws = daemon.daemon.home / "workspace"
    ws.mkdir(parents=True, exist_ok=True)
    f = ws / "draft.txt"
    f.write_text("keep me")
    scopes = set().union(*(c.scopes for c in daemon.daemon.gateway.caps.values() if c.name == "fs.trash"))
    grant, _ = gateway_setup(daemon, scopes, gateway_request("tidy my workspace"), [])
    gateway_tool_call(daemon, grant, "fs.trash", path=str(f))
    assert not f.exists()
    listed = _call(daemon, "recovery.list")
    trashed = next(r for r in listed["records"] if r["kind"] == "trash")
    back = _call(daemon, "recovery.restore", {"id": trashed["id"]})
    assert back["in_place"] and f.read_text() == "keep me"


# ---------- FERB.app only ----------

@pytest.mark.parametrize("method", ["secrets.saved", "secrets.delete", "identity.find", "identity.choose",
                                    "identity.pair", "about.guide", "setup.finish"])
def test_the_owner_data_methods_refuse_any_other_program(daemon: RunningDaemon, method: str) -> None:
    with pytest.raises(RpcError) as refused:
        _call(daemon, method, {"place": "https://x.example", "name": "n"})
    assert refused.value.code == APP_ONLY_REFUSED


# ---------- uninstall (decision 8) ----------

def test_uninstall_everything_asks_only_in_the_app(tmp_path: Path) -> None:
    from types import SimpleNamespace
    from ferb.cli import setup as setup_cmd
    removed: list[bool] = []
    home = tmp_path / "ferb-home"
    home.mkdir()
    (home / setup_cmd.MARKER).write_text("")
    inst = SimpleNamespace(home=home, granted_binary=lambda: None,
                           uninstall=lambda purge: removed.append(purge) or [str(home)])
    asked: list[str] = []
    env = setup_cmd.Env(inst, lambda: None, lambda q: asked.append(q) or "no", io.StringIO())   # type: ignore[arg-type]
    assert setup_cmd.uninstall(env, purge=True) == 0
    assert asked and removed == [False]                           # the terminal's question, answered no: not purged
    asked.clear()
    assert setup_cmd.uninstall(env, purge=True, confirmed=True) == 0
    assert not asked and removed == [False, True]                 # FERB.app asked once already


def test_a_notices_cards_reach_the_app_after_it_was_away(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    page = {"type": "page", "page": "memory", "filter": {"imported_from": "chatgpt"}, "line": "Open the Memory page"}
    daemon.run(d._tell_owner("memory-import-r1", [], "Imported 2 facts from ChatGPT", "ferb", cards=[page]))
    notice = next(n for n in _call(daemon, "chat.status", {"peek": True})["notices"] if n["id"] == "memory-import-r1")
    assert notice["cards"] == [page]


def test_a_banner_never_carries_a_secret(daemon) -> None:
    """Moved from test_app_notifications (deleted in step 12d): a banner FERB.app posts never holds a sealed value."""
    import json as _json
    import threading as _threading
    import time as _time

    from ferb.cli.client import Client as _Client
    items: list[dict] = []
    client = _Client(daemon.sock)

    def read() -> None:
        try:
            for item in client.stream("notifications.subscribe", {"authorized": True}):
                items.append(item)
        except Exception:
            pass
    reader = _threading.Thread(target=read, daemon=True)
    reader.start()
    try:
        end = _time.monotonic() + 5
        while not daemon.daemon.posters.connected and _time.monotonic() < end:
            _time.sleep(0.05)
        daemon.call_soon(daemon.daemon._add_secret_value, "hunter2-correct-horse")
        daemon.run(daemon.daemon._notify("FERB finished", "Logged in with hunter2-correct-horse",
                                         category="task_finished", target={"kind": "task", "task_id": "t1"}))
        end = _time.monotonic() + 5
        while not items and _time.monotonic() < end:
            _time.sleep(0.05)
        assert items and "hunter2-correct-horse" not in _json.dumps(items)
    finally:
        client.interrupt()
        reader.join(2)
        client.close()
