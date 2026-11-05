"""HAND-424 (owner 2026-11-05): the account a browser profile is signed in to Chrome with (Local State `user_name`) is
shown beside its name wherever FERB shows that profile, and nothing after the name when it isn't signed in. It is read
fresh each time and never stored or journaled; without Full Disk Access FERB can't read it, so none is shown. The
emails here are invented."""

import json
from pathlib import Path
from types import SimpleNamespace
from typing import Any

from ferb.act.browser.autoinstall import STORE_EXTENSION_ID
from ferb.act.browser.profiles import BrowserProfile, shown
from ferb.policy.confirm import PAIR, key
from tests.conftest import RunningDaemon

ON: dict[str, Any] = {"location": 1, "path": f"{STORE_EXTENSION_ID}/0.19.0_0"}
OFF: dict[str, Any] = {**ON, "disable_reasons": [1]}


def _chrome(support: Path, profiles: dict[str, tuple[str, str, dict[str, Any] | None]]) -> Path:
    """Chrome's user-data folder: folder -> (name, signed-in account or "", FERB's extension entry or None)."""
    data = support / "Google" / "Chrome"
    data.mkdir(parents=True)
    (data / "Local State").write_text(json.dumps({"profile": {"info_cache": {
        f: {"name": name, **({"user_name": account} if account else {})}
        for f, (name, account, _) in profiles.items()}}}))
    for f, (_, _, entry) in profiles.items():
        (data / f).mkdir()
        (data / f / "Preferences").write_text(json.dumps({"extensions": {
            "settings": {STORE_EXTENSION_ID: entry} if entry is not None else {}}}))
    return data


def _daemon(daemon: RunningDaemon, tmp_path: Path, access: bool = True) -> Any:
    support = tmp_path / "Support"
    chrome = _chrome(support, {"Default": ("Work", "Work.Person@Example.com", ON),
                               "Profile 1": ("Home", "", ON),
                               "Profile 2": ("Work", "other@example.test", OFF)})
    d = daemon.daemon
    d.browser_support, d.browser_data_dirs = support, {"chrome": chrome}
    d.data_access = lambda: access

    async def no_files() -> dict[str, Any]:
        return {"state": "connected"}
    d._connect_browsers = no_files
    return d


def _pair(daemon: RunningDaemon, folder: str) -> None:
    async def go() -> None:
        daemon.daemon.db.conn.execute(
            "INSERT INTO ext_installs (install_id, browser, folder, identity, key, paired_at, ext_version) "
            "VALUES (?, 'chrome', ?, 'owner', x'00', 'now', '0.19.0')", (f"i-{folder}", folder))
    daemon.run(go())


# decision: HAND-424
def test_shown_names_a_profile_with_its_account_and_nothing_when_unsigned() -> None:
    assert shown(BrowserProfile("chrome", "Default", "Work", "work@example.com")) == \
        "Google Chrome “Work” (work@example.com)"
    assert shown(BrowserProfile("chrome", "Profile 1", "Home", "")) == "Google Chrome “Home”"
    assert shown(BrowserProfile("brave", "Default", "Work", "w@example.com"), "") == "“Work” (w@example.com)"
    from ferb.orchestrator.leftovers import leftover_lines
    [line] = leftover_lines([{"resource": "browser", "error": "x", "tabs": [
        {"browser": "Google Chrome", "profile": "Work", "account": "w@example.com", "title": "Inbox"}]}], ["x"])
    assert "Google Chrome “Work” (w@example.com), Inbox" in line


# decision: HAND-424, DMN-546
def test_setup_browsers_and_pair_all_carry_each_profiles_signed_in_account(
        daemon: RunningDaemon, tmp_path: Path) -> None:
    d = _daemon(daemon, tmp_path)
    rows = {p["folder"]: p for p in daemon.run(d._setup_browsers(None, {}))["browsers"][0]["profiles"]}
    assert rows["Default"]["email"] == "work.person@example.com"        # lower-cased, as discovery reads it
    assert rows["Profile 1"]["email"] == "" and rows["Profile 2"]["email"] == "other@example.test"

    d._extension_stands = lambda p: ("too_old", "update FERB there")    # every pairing ends at once, no page opens

    async def events() -> list[dict[str, Any]]:
        return [e async for e in d._pair_all_events({})]
    plan, *results, done = daemon.run(events())
    assert {r["folder"]: r["email"] for r in plan["todo"]} == {"Default": "work.person@example.com", "Profile 1": ""}
    assert [s["email"] for s in plan["skipped"]] == ["other@example.test"]          # two "Work" profiles told apart
    assert {r["folder"]: r["email"] for r in results} == {"Default": "work.person@example.com", "Profile 1": ""}
    assert {r["folder"]: r["email"] for r in done["results"]} == {"Default": "work.person@example.com",
                                                                 "Profile 1": ""}

    async def journal() -> str:
        return json.dumps([e.data for e in d.journal.of_kind("browsers.pair_all_skipped")])
    assert "@" not in daemon.run(journal())                                          # never journaled


# decision: HAND-424, DMN-546
def test_without_full_disk_access_no_account_is_shown(daemon: RunningDaemon, tmp_path: Path) -> None:
    d = _daemon(daemon, tmp_path, access=False)
    _pair(daemon, "Default")
    assert daemon.run(d._setup_browsers(None, {}))["browsers"] == []
    [row] = daemon.run(d._browsers_list(None, {}))["profiles"]
    assert (row["name"], row["email"]) == ("Default", "")                           # its folder, never a guess

    async def facts() -> str:
        return str(d._availability()) + d._live_facts()
    assert "Default" in daemon.run(facts()) and "@" not in daemon.run(facts())


# decision: HAND-424
def test_the_ais_facts_name_a_paired_profiles_account(daemon: RunningDaemon, tmp_path: Path) -> None:
    d = _daemon(daemon, tmp_path)
    _pair(daemon, "Default")
    _pair(daemon, "Profile 1")

    async def facts() -> tuple[str, str]:
        return str(d._availability()), d._live_facts()
    availability, live = daemon.run(facts())
    assert "“Work” (work.person@example.com)" in availability and "“Home”" in availability
    lines = live.splitlines()
    assert any("“Work” (work.person@example.com)" in line for line in lines)
    assert all("@" not in line for line in lines if "“Home”" in line)              # unsigned: nothing after it


# decision: HAND-424, GATE-511
def test_the_pair_question_shows_the_account_while_the_owners_naming_still_skips_it(
        daemon: RunningDaemon, tmp_path: Path) -> None:
    from tests.browserkit import task
    d = _daemon(daemon, tmp_path)
    gw = d.gateway

    def need(words: str) -> Any:
        task(daemon, "pq", {"browser:act"}, words)
        c = SimpleNamespace(arguments={"profile": "chrome/Default"},
                            rec=SimpleNamespace(task=SimpleNamespace(request_id="req-pq")), root="pq")

        async def go() -> Any:
            return gw._pair_need(c)
        return daemon.run(go())
    asked = need("check my inbox")
    assert asked is not None and asked.values["email"] == "work.person@example.com"
    assert asked.key == key(PAIR, "chrome", "Default")                                # still keyed by the folder
    assert need("pair my Work profile") is None                                       # named by its name: no question


# decision: DMN-546
def test_identity_and_about_name_the_chosen_profiles_with_their_accounts(daemon: RunningDaemon, tmp_path: Path) -> None:
    d = _daemon(daemon, tmp_path)
    d.config.set("browsing", "owner_profile", "chrome/Default")
    current = daemon.run(d._identity_find(None, {}))["current"]
    assert (current["owner_profile_name"], current["owner_profile_email"]) == ("Work", "work.person@example.com")
    account = daemon.run(d._about_get(None, {}))["account"]
    assert account["owner_profile"]["email"] == "work.person@example.com"
    assert "“Work” (work.person@example.com)" in account["note"]
