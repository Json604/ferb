"""L25: `ferb setup` asks every installed Chromium-family browser to install FERB's Chrome Web Store extension (the
browser's documented per-user External Extensions file), registers FERB's native host for both the pinned unpacked
id and the store id, and reports each profile as installed / enabled / paired from its Preferences, which it only
reads. Everything runs in a temporary Application Support folder: no real browser folder, no browser opened."""

import io
import json
import zipfile
from pathlib import Path
from typing import Any

import pytest

from ferb.act.browser import autoinstall, profiles
from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID
from ferb.cli import setup as S
from ferb.config import Config
from ferb.device.install import NATIVE_HOST_MANIFEST
from tests.browserkit import bd  # noqa: F401  (bd is a fixture)
from tests.conftest import FakeRunner
from tests.fake_extension import FakeExtension

STORE = "abcdefghijklmnopabcdefghijklmnop"            # a Web Store id: 32 letters a-p
PINNED = FERB_EXTENSION_ID
URL = "https://clients2.google.com/service/update2/crx"

# browser -> its user-data folder under Application Support, and folder -> (name, Secure Preferences, Preferences)
FIXTURE: dict[str, tuple[str, dict[str, tuple[str, dict[str, Any], dict[str, Any]]]]] = {
    "chrome": ("Google/Chrome", {
        "Default": ("Personal", {"extensions": {"settings": {STORE: {"location": 1, "disable_reasons": []}}}},
                    {"browser": {"window": "kept"}}),
        "Profile 1": ("Work", {"extensions": {"settings": {STORE: {"location": 1}}}},
                      {"extensions": {"settings": {STORE: {"disable_reasons": [1]}}}}),   # waiting for the owner
    }),
    "brave": ("BraveSoftware/Brave-Browser", {
        "Default": ("Brave home", {}, {"extensions": {"settings": {PINNED: {"location": 4, "state": 1}}}}),
    }),
    "edge": ("Microsoft Edge", {
        "Default": ("Edge default", {}, {"extensions": {"settings": {}}}),
        "Profile 1": ("Edge old", {"extensions": {"settings": {}}},
                      {"extensions": {"external_uninstalls": [STORE]}}),                 # the owner removed it
    }),
    "vivaldi": ("Vivaldi", {"Default": ("Viv", {}, {})}),
}


def fake_support(root: Path) -> Path:
    support = root / "Application Support"
    for rel, folders in FIXTURE.values():
        d = support / rel
        d.mkdir(parents=True)
        cache = {f: {"name": name, "user_name": ""} for f, (name, _, _) in folders.items()}
        (d / "Local State").write_text(json.dumps({"profile": {"info_cache": cache}}))
        for folder, (_, secure, prefs) in folders.items():
            (d / folder).mkdir()
            (d / folder / "Secure Preferences").write_text(json.dumps(secure))
            (d / folder / "Preferences").write_text(json.dumps(prefs))
    (support / "Slack").mkdir()                      # not a browser: never touched
    (support / "Slack" / "Local State").write_text("{}")
    return support


def browser_files(support: Path) -> dict[Path, bytes]:
    names = {"Preferences", "Secure Preferences", "Local State"}
    return {p: p.read_bytes() for p in support.rglob("*") if p.is_file() and p.name in names}


class Daemon:
    """Answers what `ferb setup --update` asks the running daemon."""

    def __init__(self, paired: set[tuple[str, str]]) -> None:
        self.paired = paired

    def client(self) -> "Daemon":
        return self

    def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        if method == "ping":
            return {"pid": 4242}
        if method == "doctor.check":
            return {"permissions": {p: "granted" for p in S.SETUP_PERMISSIONS}}
        if method == "setup.persona":
            return {"state": "reviewed"}
        if method == "browsers.list":
            return {"profiles": [{"browser": b, "folder": f, "paired": True} for b, f in sorted(self.paired)]}
        raise AssertionError(method)

    def close(self) -> None:
        pass


@pytest.fixture
def world(tmp_path: Path) -> dict[str, Any]:
    home = tmp_path / ".ferb"
    support = fake_support(tmp_path)
    (home / "venv" / "bin").mkdir(parents=True)
    (home / "venv" / "bin" / "python").write_text("")          # an existing installation: --update applies
    return {"home": home, "support": support, "tmp": tmp_path}


def run_setup(world: dict[str, Any], paired: set[tuple[str, str]] | None = None) -> str:
    home, support = world["home"], world["support"]
    inst = S.installer(home, support, label="com.ferb.test", run=FakeRunner(home),
                       launch_agents=world["tmp"] / "LaunchAgents")
    out = io.StringIO()
    daemon = Daemon(paired or set())
    env = S.Env(inst, daemon.client, input, out, sleep=lambda s: None, running_apps=set,  # type: ignore[arg-type]
                browser_support=support)
    assert S.setup(env, update=True) == 0, out.getvalue()
    return out.getvalue()


def manifest(d: Path) -> dict[str, Any]:
    result: dict[str, Any] = json.loads((d / "NativeMessagingHosts" / NATIVE_HOST_MANIFEST).read_text())
    return result


def test_setup_writes_the_store_request_and_native_host_in_chrome_brave_and_edge(world: dict[str, Any]) -> None:
    Config(world["home"]).set("browser", "store_extension_id", STORE)
    text = run_setup(world)
    support = world["support"]
    origins = [f"chrome-extension://{PINNED}/", f"chrome-extension://{STORE}/"]
    for browser in ("chrome", "brave", "edge"):
        d = support / FIXTURE[browser][0]
        assert json.loads((d / "External Extensions" / f"{STORE}.json").read_text()) == {"external_update_url": URL}
        assert manifest(d)["allowed_origins"] == origins
        assert manifest(d)["path"] == str(world["home"] / "bin" / "ferb-native-host")
    # Vivaldi: no source supports the request there, so none is written; its native host is (Load unpacked works)
    assert not (support / "Vivaldi" / "External Extensions").exists()
    assert manifest(support / "Vivaldi")["allowed_origins"] == origins
    assert not (support / "Slack" / "External Extensions").exists() and not (support / "Opera").exists()
    assert not (support / "com.operasoftware.Opera").exists()      # not installed: nothing created
    assert "Load unpacked" in text and "Vivaldi" in text            # the manual path stays where it's needed
    assert "Reopen" in text and "Microsoft Edge" in text            # which browsers must be reopened


def test_setup_never_changes_a_browsers_preferences(world: dict[str, Any]) -> None:
    Config(world["home"]).set("browser", "store_extension_id", STORE)
    before = browser_files(world["support"])
    run_setup(world, {("brave", "Default")})
    assert browser_files(world["support"]) == before
    assert len(before) == 17                        # 5 Local State files, 2 settings files in each of 6 profiles


def test_an_empty_store_id_writes_no_request_and_prints_the_manual_steps(world: dict[str, Any]) -> None:
    Config(world["home"]).set("browser", "store_extension_id", "")   # the owner can still opt out of the store copy
    text = run_setup(world)
    assert not list(world["support"].rglob("External Extensions"))
    assert manifest(world["support"] / "Google" / "Chrome")["allowed_origins"] == [f"chrome-extension://{PINNED}/"]
    assert "listing is pending" in text and "Load unpacked" in text and "Developer mode" in text
    assert str(world["home"] / "extensions" / "chromium") in text
    assert "ferb browsers pair" in text


def test_a_store_id_that_isnt_an_extension_id_is_never_used_as_a_file_name(world: dict[str, Any]) -> None:
    Config(world["home"]).set("browser", "store_extension_id", "../../../evil")
    text = run_setup(world)
    assert not list(world["tmp"].rglob("*evil*")) and not list(world["support"].rglob("External Extensions"))
    assert "isn't a Chrome extension id" in text and "Load unpacked" in text
    assert autoinstall.accepted_extension_ids(Config(world["home"])) == (PINNED,)


def test_status_per_profile_is_read_from_its_preferences(world: dict[str, Any]) -> None:
    states = autoinstall.prepare(world["support"], STORE, (PINNED, STORE), {("brave", "Default")})
    by = {(s.browser, p.folder): p for s in states for p in s.profiles}
    assert sorted({s.browser for s in states}) == ["brave", "chrome", "edge", "vivaldi"]
    chrome, work = by[("chrome", "Default")], by[("chrome", "Profile 1")]
    assert (chrome.extension_id, chrome.enabled, chrome.paired) == (STORE, True, False)
    assert (work.extension_id, work.enabled) == (STORE, False)              # installed, waiting to be turned on
    brave = by[("brave", "Default")]
    assert (brave.extension_id, brave.enabled, brave.paired) == (PINNED, True, True)
    assert by[("edge", "Default")].extension_id == "" and not by[("edge", "Default")].removed
    assert by[("edge", "Profile 1")].removed                                # the browser won't add it again
    lines = "\n".join(autoinstall.describe(states, STORE, Path("/x/chromium")))
    assert "“Personal” (Default): installed from the Web Store, enabled, not paired" in lines
    assert "“Work” (Profile 1): installed from the Web Store, waiting for you to turn it on" in lines
    assert "“Brave home” (Default): installed unpacked, enabled, paired" in lines
    assert f"chromewebstore.google.com/detail/{STORE}" in lines
    assert "Pair all or Pair in FERB's Settings → Browsers" in lines     # pairing on the owner's click (2026-10-27)
    assert "by itself" not in lines
    unchecked = autoinstall.prepare(world["support"], "", (PINNED,), None)  # FERB isn't running: not checked
    assert all(p.paired is None for s in unchecked for p in s.profiles)


def test_discovery_finds_every_installed_chromium_browser_and_its_profiles(world: dict[str, Any]) -> None:
    support = world["support"]
    assert profiles.installed(support) == ["chrome", "brave", "edge", "vivaldi"]
    dirs = {b: profiles.data_dir(b, support) for b in profiles.installed(support)}
    found = {(p.browser, p.folder, p.display_name) for p in profiles.discover_all(dirs)}
    assert ("edge", "Profile 1", "Edge old") in found and ("vivaldi", "Default", "Viv") in found
    assert {"chrome", "brave", "edge", "chromium", "vivaldi", "arc", "opera"} <= set(profiles.BROWSERS)


def test_uninstall_removes_the_store_requests(world: dict[str, Any]) -> None:
    Config(world["home"]).set("browser", "store_extension_id", STORE)
    run_setup(world)
    home, support = world["home"], world["support"]
    before = browser_files(support)
    inst = S.installer(home, support, every_browser=True, label="com.ferb.test", run=FakeRunner(home),
                       launch_agents=world["tmp"] / "LaunchAgents")
    out = io.StringIO()
    env = S.Env(inst, Daemon(set()).client, input, out, browser_support=support)  # type: ignore[arg-type]
    assert S.uninstall(env, purge=False) == 0
    assert not list(support.rglob(f"{STORE}.json")) and not list(support.rglob(NATIVE_HOST_MANIFEST))
    assert browser_files(support) == before


def test_package_writes_a_store_ready_zip(tmp_path: Path) -> None:
    source = tmp_path / "ext"
    (source / "lib").mkdir(parents=True)
    (source / "manifest.json").write_text(json.dumps({"manifest_version": 3, "name": "FERB", "version": "1.2.3",
                                                      "description": "d", "key": "DEVKEY"}))
    (source / "background.js").write_text("// worker")
    (source / "lib" / "x.js").write_text("// x")
    (source / ".DS_Store").write_text("junk")
    (source / "STORE_LISTING.md").write_text("listing text, not extension code")
    path, notes = autoinstall.package(source, tmp_path / "out")
    assert path == tmp_path / "out" / "ferb-extension-1.2.3.zip"
    with zipfile.ZipFile(path) as z:
        assert sorted(z.namelist()) == ["background.js", "lib/x.js", "manifest.json"]
        m = json.loads(z.read("manifest.json"))
    assert "key" not in m and m["version"] == "1.2.3"           # the Web Store assigns the listing's own key
    assert any("icons" in n for n in notes)


def test_the_package_command_uses_the_real_bundle(tmp_path: Path) -> None:
    from ferb.cli import main
    assert main(["browsers", "extension", "package", "--out", str(tmp_path)]) == 0
    real = json.loads((Path(__file__).parents[1] / "extensions" / "chromium" / "manifest.json").read_text())
    with zipfile.ZipFile(tmp_path / f"ferb-extension-{real['version']}.zip") as z:
        assert "background.js" in z.namelist() and "key" not in json.loads(z.read("manifest.json"))


def test_the_extension_install_command_registers_requests_and_reports(world: dict[str, Any],
                                                                     monkeypatch: pytest.MonkeyPatch,
                                                                     capsys: pytest.CaptureFixture[str]) -> None:
    from ferb.cli import main
    monkeypatch.setattr(profiles, "SUPPORT", world["support"])
    monkeypatch.setenv("FERB_HOME", str(world["home"]))
    Config(world["home"]).set("browser", "store_extension_id", STORE)
    before = browser_files(world["support"])
    assert main(["browsers", "extension", "install"]) == 0
    text = capsys.readouterr().out
    chrome = world["support"] / "Google" / "Chrome"
    assert (chrome / "External Extensions" / f"{STORE}.json").is_file()
    assert manifest(chrome)["allowed_origins"][-1] == f"chrome-extension://{STORE}/"
    assert "pairing not checked" in text and browser_files(world["support"]) == before   # FERB isn't running


def test_the_daemon_accepts_the_store_copy_and_pairs_it_by_its_own_id(bd: Any) -> None:  # noqa: F811
    """The store copy has its own id: the daemon accepts its origin and opens the pairing page at that id."""
    import threading

    from ferb.cli.client import Client
    d, launched = bd
    d.daemon.config.set("browser", "store_extension_id", STORE)
    (d.daemon.browser_data_dirs["chrome"] / "Profile 1" / "Secure Preferences").write_text(json.dumps(
        {"extensions": {"settings": {STORE: {"location": 1}}}}))
    ext = FakeExtension(d.daemon.home, origin=f"chrome-extension://{STORE}/")
    ext.connect()
    c = Client(d.sock, timeout=30)
    out: dict[str, Any] = {}
    try:
        n = len(launched)
        t = threading.Thread(target=lambda: out.update(c.call("browsers.pair", {"profile": "Work", "timeout_s": 10})))
        t.start()
        ext.wait_for(lambda: len(launched) > n and "pair.html#token=" in launched[-1][-1], 10)
        url = launched[-1][-1]
        ext.pair(url.split("#token=")[1])
        t.join(15)
    finally:
        c.close()
    assert url.startswith(f"chrome-extension://{STORE}/pair.html")
    assert out.get("paired") is True, out


# ---------- fix round 1 ----------

OTHER = "ponmlkjihgfedcbaponmlkjihgfedcba"                  # a second valid Web Store id


def requests_for(support: Path, store: str) -> list[Path]:
    return sorted(support.rglob(f"External Extensions/{store}.json"))


def test_a_cleared_store_id_removes_the_requests_ferb_wrote(world: dict[str, Any]) -> None:
    Config(world["home"]).set("browser", "store_extension_id", STORE)
    run_setup(world)
    assert len(requests_for(world["support"], STORE)) == 3
    Config(world["home"]).set("browser", "store_extension_id", "")
    text = run_setup(world)
    assert requests_for(world["support"], STORE) == []
    assert "Removed" in text and f"{STORE}.json" in text


def test_a_changed_store_id_replaces_the_old_requests(world: dict[str, Any]) -> None:
    Config(world["home"]).set("browser", "store_extension_id", STORE)
    run_setup(world)
    Config(world["home"]).set("browser", "store_extension_id", OTHER)
    run_setup(world)
    assert requests_for(world["support"], STORE) == [] and len(requests_for(world["support"], OTHER)) == 3


def uninstall(world: dict[str, Any], **patch: Any) -> tuple[int, str]:
    home, support = world["home"], world["support"]
    inst = S.installer(home, support, every_browser=True, label="com.ferb.test", run=FakeRunner(home),
                       launch_agents=world["tmp"] / "LaunchAgents")
    for name, value in patch.items():
        setattr(inst, name, value)
    out = io.StringIO()
    env = S.Env(inst, Daemon(set()).client, input, out, browser_support=support)  # type: ignore[arg-type]
    return S.uninstall(env, purge=False), out.getvalue()


def test_uninstall_after_the_id_was_cleared_or_config_distrusted_still_removes_requests(world: dict[str, Any]) -> None:
    Config(world["home"]).set("browser", "store_extension_id", STORE)
    run_setup(world)
    Config(world["home"]).set("browser", "store_extension_id", "")
    (world["home"] / "config.toml").chmod(0o644)                 # and FERB no longer trusts config.toml
    code, text = uninstall(world)
    assert code == 0 and requests_for(world["support"], STORE) == []


def test_uninstall_reports_removed_requests_even_when_the_installer_then_fails(world: dict[str, Any]) -> None:
    from ferb.device.install import InstallError
    Config(world["home"]).set("browser", "store_extension_id", STORE)
    run_setup(world)

    def fails(purge: bool = False) -> list[str]:
        raise InstallError("launchctl said no")
    code, text = uninstall(world, uninstall=fails)
    assert code == 1 and "launchctl said no" in text
    assert text.count(f"removed ") >= 3 and f"{STORE}.json" in text


def test_one_browser_that_cant_take_the_request_doesnt_stop_setup(world: dict[str, Any]) -> None:
    Config(world["home"]).set("browser", "store_extension_id", STORE)
    (world["support"] / "Google" / "Chrome" / "External Extensions").write_text("not a folder")
    text = run_setup(world)
    assert len(requests_for(world["support"], STORE)) == 2              # Brave and Edge still asked
    assert "Google Chrome: couldn't write the install request" in text and "Load unpacked" in text


def test_a_failed_request_write_leaves_no_temp_file(world: dict[str, Any], monkeypatch: pytest.MonkeyPatch) -> None:
    def refuse(src: Any, dst: Any) -> None:
        raise OSError("disk full")
    monkeypatch.setattr(autoinstall.os, "replace", refuse)
    states = autoinstall.prepare(world["support"], STORE, (PINNED, STORE), None)
    assert all(s.request is None for s in states) and any("disk full" in s.error for s in states)
    assert not [p for p in world["support"].rglob(".ferb-*")]


def test_configured_extension_ids_are_validated_and_the_pinned_id_always_stays(world: dict[str, Any]) -> None:
    c = Config(world["home"])
    c.set("browser", "extension_ids", ["../../x", "chrome-extension://abc/", OTHER])
    c.set("browser", "store_extension_id", STORE)
    assert autoinstall.accepted_extension_ids(c) == (OTHER, PINNED, STORE)
    run_setup(world)
    assert manifest(world["support"] / "Google" / "Chrome")["allowed_origins"] == [
        f"chrome-extension://{i}/" for i in (OTHER, PINNED, STORE)]


def test_the_headline_names_only_the_browsers_actually_asked(tmp_path: Path) -> None:
    support = tmp_path / "Application Support"
    (support / "Vivaldi").mkdir(parents=True)
    (support / "Vivaldi" / "Local State").write_text("{}")
    text = "\n".join(autoinstall.describe(autoinstall.prepare(support, STORE, (PINNED, STORE), None), STORE,
                                          Path("/x")))
    assert "asked" not in text.splitlines()[0] and "Load unpacked" in text
    world = fake_support(tmp_path / "w")
    first = autoinstall.describe(autoinstall.prepare(world, STORE, (PINNED, STORE), None), STORE, Path("/x"))[0]
    assert "asked Google Chrome, Brave, Microsoft Edge to install it" in first


def test_opera_reads_native_hosts_from_chromes_folder(tmp_path: Path) -> None:
    """help.opera.com/en/extensions/message-passing/ gives Chrome's folder for macOS."""
    dirs = autoinstall.native_host_dirs(tmp_path, ["opera"])
    assert tmp_path / "Google" / "Chrome" / "NativeMessagingHosts" in dirs


def test_the_native_host_manifest_is_replaced_atomically(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    import ferb.device.install as install_mod
    d = tmp_path / "NativeMessagingHosts"
    d.mkdir()
    (d / NATIVE_HOST_MANIFEST).write_text("OLD")
    inst = install_mod.Installer(tmp_path / ".ferb", native_hosts_dirs=(d,), extension_ids=(PINNED,))

    def refuse(src: Any, dst: Any) -> None:
        raise OSError("disk full")
    monkeypatch.setattr(install_mod.os, "replace", refuse)
    with pytest.raises(OSError):
        inst.write_native_host()
    assert (d / NATIVE_HOST_MANIFEST).read_text() == "OLD" and sorted(p.name for p in d.iterdir()) == [
        NATIVE_HOST_MANIFEST]


def test_profile_paths_follow_support_when_called(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(profiles, "SUPPORT", tmp_path)
    assert profiles.data_dir("chrome") == tmp_path / "Google" / "Chrome"
    assert profiles.installed() == []


def test_the_built_in_store_id_is_ferbs_live_listing() -> None:
    """FERB's Chrome Web Store listing went live on 2026-10-20: a fresh install uses it without any config."""
    assert autoinstall.STORE_EXTENSION_ID == "plbfkifcmgkgnlfgikdeaaglgncdhkma"
    assert autoinstall.valid_id(autoinstall.STORE_EXTENSION_ID)


# ---------- both copies in one profile (owner's update 2026-10-24) ----------

def both_copies(folder: Path, pinned_on: bool, store_on: bool) -> None:
    """A profile that had the unpacked copy and then took the store request: Chromium keeps both entries."""
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "Secure Preferences").write_text(json.dumps({"extensions": {"settings": {
        PINNED: {"location": 4, "disable_reasons": [] if pinned_on else [1]},
        STORE: {"location": 1, "disable_reasons": [] if store_on else [1]}}}}))


def test_a_profile_with_both_copies_reports_the_one_turned_on(tmp_path: Path) -> None:
    p = profiles.BrowserProfile("chrome", "Profile 2", "scaler", "")
    found = {}
    for pinned_on, store_on in ((False, True), (True, False), (True, True), (False, False)):
        both_copies(tmp_path / "Profile 2", pinned_on, store_on)
        s = autoinstall.profile_status(p, (PINNED, STORE), tmp_path, set(), STORE)
        found[(pinned_on, store_on)] = (s.extension_id, s.enabled)
    assert found[(False, True)] == (STORE, True)          # the unpacked copy turned off: the store copy counts
    assert found[(True, False)] == (PINNED, True)
    assert found[(True, True)] == (PINNED, True)          # the copy listed first, as before
    assert found[(False, False)] == (PINNED, False)       # both off: waiting for the owner


def test_pairing_opens_the_copy_turned_on_when_a_profile_has_both(bd: Any) -> None:  # noqa: F811
    """With the unpacked copy turned off, its pairing page can't open: pairing must go to the store copy."""
    import threading

    from ferb.cli.client import Client
    d, launched = bd
    d.daemon.config.set("browser", "store_extension_id", STORE)
    both_copies(d.daemon.browser_data_dirs["chrome"] / "Profile 1", pinned_on=False, store_on=True)
    ext = FakeExtension(d.daemon.home, origin=f"chrome-extension://{STORE}/")
    ext.connect()
    c = Client(d.sock, timeout=30)
    out: dict[str, Any] = {}
    try:
        n = len(launched)
        t = threading.Thread(target=lambda: out.update(c.call("browsers.pair", {"profile": "Work", "timeout_s": 10})))
        t.start()
        ext.wait_for(lambda: len(launched) > n and "pair.html#token=" in launched[-1][-1], 10)
        url = launched[-1][-1]
        ext.pair(url.split("#token=")[1])
        t.join(15)
    finally:
        c.close()
    assert url.startswith(f"chrome-extension://{STORE}/pair.html")
    assert out.get("paired") is True, out

# ---------- L34 C: the setup window's Browsers step ----------

from ferb.cli.client import Client, RpcError  # noqa: E402
from ferb.device.extensions import browser_rows  # noqa: E402
from tests.conftest import RunningDaemon  # noqa: E402


def test_read_states_reports_what_prepare_does_and_writes_nothing(world: dict[str, Any]) -> None:
    before = sorted(world["support"].rglob("*"))
    states = autoinstall.read_states(world["support"], STORE, (PINNED, STORE), {("brave", "Default")})
    assert sorted(world["support"].rglob("*")) == before                    # no request file, no folder
    by = {(s.browser, p.folder): p for s in states for p in s.profiles}
    assert by[("chrome", "Default")].enabled and by[("brave", "Default")].paired
    assert all(s.request is None for s in states)                         # nothing requested yet
    written = autoinstall.prepare(world["support"], STORE, (PINNED, STORE), None)
    again = autoinstall.read_states(world["support"], STORE, (PINNED, STORE), None)
    assert {s.browser for s in again if s.request} == {s.browser for s in written if s.request}


def test_setup_rows_name_each_profiles_state_and_where_load_unpacked_is_the_way(world: dict[str, Any]) -> None:
    autoinstall.prepare(world["support"], STORE, (PINNED, STORE), None)
    states = autoinstall.read_states(world["support"], STORE, (PINNED, STORE), {("brave", "Default")})
    rows = browser_rows(states, STORE, Path("/x/chromium"), ("chrome", "Profile 1"), {("brave", "Default")})
    assert rows["store_url"] == f"https://chromewebstore.google.com/detail/{STORE}"
    assert rows["extension_dir"] == "/x/chromium"
    by = {(b["browser"], p["folder"]): p for b in rows["browsers"] for p in b["profiles"]}
    assert by[("chrome", "Default")]["state"] == "on" and by[("chrome", "Default")]["name"] == "Personal"
    assert by[("chrome", "Profile 1")]["state"] == "installed" and by[("chrome", "Profile 1")]["ferb_identity"]
    assert by[("brave", "Default")]["state"] == "paired" and by[("brave", "Default")]["connected"]
    assert by[("edge", "Default")]["state"] == "not_installed"
    assert by[("edge", "Profile 1")]["state"] == "removed"
    assert {b["browser"]: b["manual"] for b in rows["browsers"]} == {"chrome": False, "brave": True, "edge": True,
                                                                     "vivaldi": True}
    assert {b["browser"]: b["label"] for b in rows["browsers"]}["chrome"] == "Google Chrome"
    assert browser_rows(states, "", Path("/x"), None, set())["store_url"] is None


def _point_daemon_at(daemon: RunningDaemon, world: dict[str, Any]) -> None:
    d = daemon.daemon
    d.config.set("browser", "store_extension_id", STORE)
    d.browser_data_dirs = {b: profiles.data_dir(b, world["support"]) for b in profiles.installed(world["support"])}


def test_the_daemon_lists_browsers_for_the_setup_window_without_writing(daemon: RunningDaemon,
                                                                       world: dict[str, Any]) -> None:
    _point_daemon_at(daemon, world)
    before = browser_files(world["support"])
    client = Client(daemon.sock)
    try:
        rows = client.call("setup.browsers")
    finally:
        client.close()
    assert browser_files(world["support"]) == before
    assert not (world["support"] / "Google" / "Chrome" / "External Extensions").exists()
    assert {b["browser"] for b in rows["browsers"]} == {"chrome", "brave", "edge", "vivaldi"}


def test_add_to_browser_opens_the_store_page_in_that_profile_only(daemon: RunningDaemon, world: dict[str, Any]) -> None:
    _point_daemon_at(daemon, world)
    launched: list[list[str]] = []
    daemon.daemon.browser_launch = launched.append
    page = f"https://chromewebstore.google.com/detail/{STORE}"
    client = Client(daemon.sock)
    try:
        assert client.call("setup.browser_open", {"browser": "chrome", "folder": "Profile 1"}) == {"opened": page}
        with pytest.raises(RpcError):
            client.call("setup.browser_open", {"browser": "chrome", "folder": "Profile 9"})   # not discovered
    finally:
        client.close()
    assert launched == [[profiles.BROWSERS["chrome"].binary, "--profile-directory=Profile 1", page]]
