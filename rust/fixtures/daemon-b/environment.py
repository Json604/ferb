"""Fixtures for the Rust port of ferb/daemon/areas/environment.py (ENG-3): what the planner and workers are told about
this Mac and FERB, recorded from the real mixin methods over a fake daemon (`Fake`) whose every other part is a seam
read from the case's `state` (the same state a Rust test hands its fakes).

    cd <repo> && uv run python rust/fixtures/daemon-b/environment.py

Writes environment.json beside this file: `cases`, each `{name, method, args, state, files, out, journal}`. `files`
are the browser settings files the case wrote under the support folder (written `/SUPPORT/<data dir>/<folder>/<name>`);
`journal` the rows the method appended (`[kind, data]`). Real Python runs for everything the methods compose:
browser profile naming, the extension's state in Preferences, services.summary, availability.build, browsing's facts,
devtools.missing_fact, the untrusted-data wrapper (its id fixed `0123456789ab`), trail_sources, origin.join,
without_queries and the token count. Invented data only; the same bytes on every run.
"""

import asyncio
import json
import os
import shutil
import sys
import tempfile
import time
from pathlib import Path
from types import SimpleNamespace
from typing import Any

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
os.environ["TZ"] = "UTC"
time.tzset()
sys.path.insert(0, str(REPO))

import platform  # noqa: E402

import ferb.act.browser.profiles as bprofiles  # noqa: E402
import ferb.act.mac.apps as appsm  # noqa: E402
import ferb.sanitize.untrusted as untrusted  # noqa: E402
from ferb.act.browser.pairing import Binding  # noqa: E402
from ferb.config import Config  # noqa: E402
from ferb.core.types import Identity  # noqa: E402
from ferb.daemon.areas.environment import Environment  # noqa: E402
from ferb.sanitize.redact import Redactor  # noqa: E402

untrusted.secrets = SimpleNamespace(token_hex=lambda n: "0123456789ab")      # type: ignore[attr-defined]
ROOT = Path(tempfile.gettempdir()) / "fxb-environment"
SUPPORT = ROOT / "support"
FERB_ID = "jaclnphefmmgmpofnabffahdkbjjompm"
STORE_ID = "plbfkifcmgkgnlfgikdeaaglgncdhkma"


class Driver:
    def __init__(self, st: dict[str, Any], log: list[Any], name: str) -> None:
        self.st, self.log, self.name = st, log, name
        self.wake = object() if st.get("wake") else None

    def task_tabs(self, task_id: str) -> list[dict[str, Any]]:
        return [dict(t) for t in self.st.get("tabs", {}).get(task_id, [])]

    def urls_of(self, task_id: str) -> list[str]:
        return list(self.st.get("urls", {}).get(task_id, []))

    def keep_tabs_open(self, task_id: str) -> None:
        self.log.append([self.name, "keep_tabs_open", task_id])

    async def rest(self, task_id: str, resting: bool) -> None:
        self.log.append([self.name, "rest", task_id, resting])


class Journal:
    def __init__(self, st: dict[str, Any], rows: list[Any]) -> None:
        self.st, self.rows = st, rows

    def of_running_tasks(self, kind: str) -> list[Any]:
        assert kind == "security.alignment"
        return [SimpleNamespace(data=d) for d in self.st.get("alignment", [])]

    def append(self, kind: str, data: dict[str, Any]) -> None:
        self.rows.append([kind, json.loads(json.dumps(data))])


def profile(p: dict[str, Any]) -> Any:
    return bprofiles.BrowserProfile(p["browser"], p["folder"], p["display_name"], p["email"])


class Fake(Environment):
    """The daemon as these methods read it; every other part's answer comes from `state`."""

    def __init__(self, st: dict[str, Any], home: Path) -> None:
        self.st = st
        self.log: list[Any] = []
        self.rows: list[Any] = []
        self.home = home
        self.config = Config(home)
        self.redact = Redactor()
        self.journal = Journal(st, self.rows)
        self.pairings = None if st.get("pairings") is None else SimpleNamespace(
            all=lambda: [Binding(b["install_id"], b["browser"], b["folder"], Identity(b["identity"]))
                         for b in st["pairings"]],
            version=lambda install_id: st.get("pairing_versions", {}).get(install_id, ""))
        self.browser_data_dirs = None
        self.browser_support = SUPPORT
        drivers = st.get("drivers", {})
        self.router = Driver(drivers["router"], self.log, "router") if drivers.get("router") is not None else None
        self.ferb_identity = Driver(drivers["ferb_identity"], self.log, "ferb_identity") \
            if drivers.get("ferb_identity") is not None else None
        self.identity_browser = object() if st.get("identity_browser") else None
        tasks = st.get("tasks", {})
        self.tasks = SimpleNamespace(get=lambda tid: None if tid not in tasks else SimpleNamespace(
            tree_id=tasks[tid].get("tree_id"), task=SimpleNamespace(
                id=tid, role=tasks[tid]["role"], parent_id=tasks[tid].get("parent_id"),
                scopes=frozenset(tasks[tid].get("scopes", [])), request_id=tasks[tid].get("request_id"),
                identity=tasks[tid].get("identity", "owner"), device_id=tasks[tid].get("device_id", "dev"))))
        readable = st.get("readable")
        self.supervisor = None if readable is None else SimpleNamespace(continuity=SimpleNamespace(
            readable_tasks=lambda tree: set(readable.get(tree, [tree]))))
        self.daily_activity = SimpleNamespace(search=lambda words: json.loads(json.dumps(st.get("activity", []))))
        self.registry = SimpleNamespace(capabilities={
            str(i): SimpleNamespace(scopes=frozenset(s)) for i, s in enumerate(st.get("registered", []))})
        self._fresh = SimpleNamespace(get=lambda fn, *a: fn())
        self.secrets = SimpleNamespace(specs=lambda: [
            SimpleNamespace(ref=SimpleNamespace(key=s["key"]), account_hint=s["account_hint"],
                            allowed_origins=tuple(s["allowed_origins"])) for s in st.get("secrets", [])])
        self.missing_dev_tools = list(st.get("missing_dev_tools", []))
        self.schedules = None if st.get("schedule_facts") is None else object()
        self.timers = None if st.get("reminder_facts") is None else object()
        hub = st.get("hub")
        self.hub = None if hub is None else SimpleNamespace(
            bridges={tuple(k.split("/", 1)): SimpleNamespace(conn=object() if v.get("conn") else None,
                                                             version=v.get("version", ""))
                     for k, v in hub.get("bridges", {}).items()},
            unpaired=dict(hub.get("unpaired", {})),
            problem=lambda b: hub.get("problems", {}).get(f"{b.browser}/{b.folder}"))

    # the other parts' answers
    def _identity_profile(self) -> Any:
        p = self.st.get("identity_profile")
        return tuple(p) if p else None

    def _profiles(self) -> list[Any]:
        return [profile(p) for p in self.st.get("profiles", [])]

    _fresh_profiles = _profiles

    def _installed_apps(self) -> list[tuple[str, str]]:
        return [tuple(a) for a in self.st.get("installed_apps", [])]  # type: ignore[misc]

    def _profile_state(self, p: Any, ext: Any) -> str:
        return str(self.st.get("profile_states", {}).get(f"{p.browser}/{p.folder}", f"ext:{ext}"))

    def _worker_types(self) -> Any:
        return self.st.get("worker_types", [])

    def _browsing(self, ask: bool, words: str) -> dict[str, Any]:
        assert ask is False and words == ""
        return dict(self.st.get("browsing", {}))

    def _permissions(self) -> dict[str, bool]:
        return dict(self.st.get("permissions", {}))

    def _runtime_for(self, role: str) -> str:
        return str(self.st["runtime_for"][role])

    def _journal(self) -> Any:
        return self.journal

    def _about_summary(self) -> str:
        return str(self.st.get("about", "FERB about."))

    def dev_tools_installed(self) -> bool:
        return bool(self.st.get("dev_tools_installed", False))

    def _time_meanings(self) -> list[tuple[str, str]]:
        return [tuple(x) for x in self.st.get("time_meanings", [])]  # type: ignore[misc]

    def _models(self) -> dict[str, str]:
        return dict(self.st.get("models", {"planner": "model-a"}))

    def _schedule_facts(self, last_runs: bool) -> list[str]:
        return list(self.st["schedule_facts"]["last_runs" if last_runs else "plain"])

    def _reminder_facts(self) -> list[str]:
        return list(self.st["reminder_facts"])

    def _unpaired_note(self, browser: str) -> str:
        return str(self.st.get("unpaired_notes", {}).get(browser, ""))


def write_files(files: dict[str, Any]) -> None:
    for rel, doc in files.items():
        p = SUPPORT / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(doc if isinstance(doc, str) else json.dumps(doc))


def run(case: dict[str, Any]) -> dict[str, Any]:
    if ROOT.exists():
        shutil.rmtree(ROOT)
    home = ROOT / "home"
    home.mkdir(parents=True, mode=0o700)
    cfg = home / "config.toml"
    cfg.write_text(case.get("config", ""))
    cfg.chmod(0o600)
    files = case.get("files", {})
    write_files(files)
    st = case.get("state", {})
    bprofiles.SUPPORT = SUPPORT
    running = st.get("running", [])

    def running_apps() -> list[Any]:
        if running == "error":
            raise RuntimeError("no workspace")
        return [SimpleNamespace(bundle_id=b, name=b, pid=i + 100) for i, b in enumerate(running)]
    appsm.running = running_apps
    platform.mac_ver = lambda: (st.get("macos", "26.1"), ("", "", ""), "")   # type: ignore[assignment]
    d = Fake(st, home)
    keys = st.get("keys", {})
    providers: list[Any] = []

    def stored_api_keys(wanted: Any, problem: Any = None) -> list[str]:
        providers.extend(sorted(wanted))
        if problem is not None:
            for x in keys.get("problems", []):
                problem("keys.old_item_not_deleted", x)
        if keys.get("raise"):
            raise RuntimeError("helper gone")
        return list(keys.get("values", []))
    import ferb.runtimes.keys as rkeys
    rkeys.stored_api_keys = stored_api_keys
    method, args = case["method"], case.get("args", [])
    fn = getattr(d, method)
    out = fn(*args)
    if asyncio.iscoroutine(out):
        out = asyncio.run(out)
    if method == "_availability":
        out = {"scopes": sorted(out.scopes), "text": out.text, "missing": list(out.missing)}
    elif method == "_keychain_values":
        out = list(out)
    elif method == "_conversation_tasks":
        out = sorted(out)
    after: dict[str, Any] = {"missing_dev_tools": d.missing_dev_tools} if method == "_facts" else {}
    if keys:
        after["providers"] = providers
        after["redacted_after"] = d.redact(st["probe"])
    return {"name": case["name"], "method": method, "args": args, "config": case.get("config", ""), "state": st,
            "files": files, "out": json.loads(json.dumps(out)), "journal": d.rows, "log": d.log, **after}


def pref(settings: dict[str, Any], **extra: Any) -> dict[str, Any]:
    return {"extensions": {"settings": settings, **extra}}


ON = {"location": 1, "path": "/x/ext", "state": 1}
OFF = {"location": 4, "path": "/x/ext", "disable_reasons": [8192]}
PROFILES = [
    {"browser": "chrome", "folder": "Default", "display_name": "Personal", "email": "owner@example.test"},
    {"browser": "chrome", "folder": "Profile 1", "display_name": "Work", "email": ""},
    {"browser": "chrome", "folder": "Profile 2", "display_name": "", "email": "side@example.test"},
    {"browser": "brave", "folder": "Default", "display_name": "Bravé ü", "email": ""},
    {"browser": "chrome", "folder": "Profile 3", "display_name": "FERB", "email": "ferb.bot@example.test"},
]
PAIRED = [
    {"install_id": "inst-1", "browser": "chrome", "folder": "Default", "identity": "owner"},
    {"install_id": "inst-2", "browser": "brave", "folder": "Default", "identity": "owner"},
    {"install_id": "inst-3", "browser": "chrome", "folder": "Profile 3", "identity": "ferb"},
    {"install_id": "inst-4", "browser": "chrome", "folder": "Profile 9", "identity": "owner"},
]
FERB_CONFIG = '[identity]\nbrowser = "chrome"\nprofile = "Profile 3"\n'

CASES: list[dict[str, Any]] = []


def case(name: str, method: str, state: dict[str, Any], args: list[Any] | None = None, config: str = "",
         files: dict[str, Any] | None = None) -> None:
    CASES.append({"name": name, "method": method, "state": state, "args": args or [], "config": config,
                  "files": files or {}})


# ---- _environment (and _environment_files)
ENV_FILES = {
    "Google/Chrome/Profile 1/Preferences": pref({FERB_ID: ON}),
    "Google/Chrome/Profile 2/Secure Preferences": pref({STORE_ID: OFF}),
    "Google/Chrome/Profile 2/Preferences": pref({}),
    "BraveSoftware/Brave-Browser/Default/Preferences": pref({}, external_uninstalls=[STORE_ID]),
}
case("env_full", "_environment", {
    "identity_profile": ["chrome", "Profile 3"], "pairings": PAIRED[:1], "profiles": PROFILES,
    "installed_apps": [["com.google.Chrome", "Google Chrome"], ["com.apple.TextEdit", "TextEdit"]],
    "running": ["com.google.Chrome", "com.apple.finder"], "macos": "26.1",
    "alignment": [{"task_id": "t1", "advice": "stay on example.test"}],
    "profile_states": {"chrome/Default": "connected"}, "worker_types": ["general", "browser"]},
    ["open my mail"], FERB_CONFIG, ENV_FILES)
case("env_no_profiles", "_environment", {"pairings": [], "profiles": [], "running": "error", "macos": "",
                                         "worker_types": []}, [""])
case("env_no_pairings_store", "_environment", {"pairings": None, "profiles": PROFILES[:2], "running": [],
                                               "worker_types": {"a": 1}}, ["x"],
     '[browser]\nstore_extension_id = ""\n',
     {"Google/Chrome/Default/Preferences": pref({STORE_ID: {"location": 1, "path": "/p", "state": 2}}),
      "Google/Chrome/Profile 1/Preferences": "not json"})
case("env_removed_no_store_page", "_environment", {
    "pairings": PAIRED[1:2], "profiles": PROFILES, "running": ["com.brave.Browser"],
    "profile_states": {"brave/Default": "connected", "chrome/Default": "removed"}}, ["w"],
    '[browser]\nstore_extension_id = "not-valid"\n', ENV_FILES)
case("env_removed_store_page", "_environment", {
    "pairings": [], "profiles": PROFILES[3:4], "running": [], "profile_states": {"brave/Default": "removed"}}, ["w"], "", ENV_FILES)
case("env_custom_store", "_environment", {
    "pairings": PAIRED, "profiles": PROFILES, "running": ["b", "a"], "identity_profile": ["chrome", "Profile 3"],
    "alignment": [{"x": "ü"}, {"y": [1, 2]}]}, [""],
    FERB_CONFIG + '[browser]\nstore_extension_id = " abcdefghijklmnopabcdefghijklmnop "\n'
    'extension_ids = ["abcdefghijklmnopabcdefghijklmnop", 7, "bad"]\n',
    {"Google/Chrome/Default/Preferences": pref({"abcdefghijklmnopabcdefghijklmnop": ON}),
     "Google/Chrome/Profile 1/Preferences": pref({}, external_uninstalls=["abcdefghijklmnopabcdefghijklmnop"])})

# ---- _task_root, _inspection_parent, _conversation_tasks, _task_tabs, _browser_context, _urls_of, keep, rest
TASKS = {
    "t1": {"tree_id": "t1", "role": "planner", "scopes": ["browser:act"], "request_id": "r1"},
    "t1.w1": {"tree_id": "t1", "role": "worker", "parent_id": "t1", "scopes": ["browser:act", "fs:read"],
              "request_id": "r1"},
    "t1.c1": {"tree_id": "t1", "role": "verifier", "parent_id": "t1.w1", "scopes": ["browser:act"],
              "request_id": "r1"},
    "t1.c2": {"tree_id": "t1", "role": "verifier", "parent_id": "t1.w1", "scopes": ["browser:act", "mac:act"],
              "request_id": "r1"},
    "t1.c3": {"tree_id": "t1", "role": "verifier", "parent_id": "t1.w1", "scopes": [], "request_id": "r2"},
    "t1.c4": {"tree_id": "t1", "role": "verifier", "parent_id": "t1", "scopes": [], "request_id": "r1"},
    "t1.c5": {"tree_id": "t1", "role": "verifier", "parent_id": "gone", "scopes": [], "request_id": "r1"},
    "t1.c6": {"tree_id": "t1", "role": "verifier", "scopes": [], "request_id": "r1"},
    "t1.c7": {"tree_id": "t1", "role": "verifier", "parent_id": "t1.w1", "scopes": [], "request_id": "r1",
              "identity": "ferb"},
    "t1.c8": {"tree_id": "t1", "role": "verifier", "parent_id": "t1.w1", "scopes": [], "request_id": "r1",
              "device_id": "other"},
    "t2.w": {"tree_id": None, "role": "worker", "scopes": [], "request_id": "r3"},
    "t3.w": {"tree_id": "", "role": "worker", "scopes": []},
    "t4.c": {"tree_id": "t9", "role": "verifier", "parent_id": "t1.w1", "scopes": [], "request_id": "r1"},
}
for tid in ["t1", "t1.w1", "t2.w", "t3.w", "x.y.z", "plain"]:
    case(f"root_{tid}", "_task_root", {"tasks": TASKS}, [tid])
for tid in ["t1.c1", "t1.c2", "t1.c3", "t1.c4", "t1.c5", "t1.c6", "t1.c7", "t1.c8", "t4.c", "t1.w1", "nope"]:
    case(f"inspect_{tid}", "_inspection_parent", {"tasks": TASKS}, [tid])
case("conv_known", "_conversation_tasks", {"tasks": TASKS, "readable": {"t1": ["t1", "t0", "s-1"]}}, ["t1.w1"])
case("conv_tree_none", "_conversation_tasks", {"tasks": TASKS, "readable": {}}, ["t2.w"])
case("conv_unknown", "_conversation_tasks", {"tasks": TASKS, "readable": {}}, ["nope"])
case("conv_no_supervisor", "_conversation_tasks", {"tasks": TASKS}, ["t1"])
DRIVERS = {
    "router": {"tabs": {"t1.w1": [{"tab_id": 11, "alias": "tab-1", "url": "https://example.test/a?token=x",
                                   "taken_over_from": "https://example.test/start"}]},
               "urls": {"t1.w1": ["https://example.test/a"]}, "wake": True},
    "ferb_identity": {"tabs": {"t1.w1": [{"tab_id": 12, "title": "Café — mail of owner@example.test",
                                          "password": "hunter2-invented"}]},
                      "urls": {"t1.w1": ["https://mail.example.test/"], "t1": ["https://x.test"]}},
}
case("tabs_both", "_task_tabs", {"drivers": DRIVERS}, ["t1.w1"])
case("tabs_none", "_task_tabs", {"drivers": DRIVERS}, ["t9"])
case("tabs_no_drivers", "_task_tabs", {"drivers": {}}, ["t1.w1"])
case("tabs_router_only", "_task_tabs", {"drivers": {"router": DRIVERS["router"]}}, ["t1.w1"])
case("browser_ctx_checker", "_browser_context", {"tasks": TASKS, "drivers": DRIVERS}, ["t1.c1"])
case("browser_ctx_no_tabs", "_browser_context", {"tasks": TASKS, "drivers": {"router": {}}}, ["t1.c1"])
case("browser_ctx_worker", "_browser_context", {"tasks": TASKS, "drivers": DRIVERS}, ["t1.w1"])
case("urls_both", "_urls_of", {"drivers": DRIVERS}, ["t1.w1"])
case("urls_identity_only", "_urls_of", {"drivers": {"ferb_identity": DRIVERS["ferb_identity"]}}, ["t1"])
case("keep_tabs", "_keep_tabs_open", {"drivers": DRIVERS}, ["t1.w1"])
case("keep_tabs_one", "_keep_tabs_open", {"drivers": {"ferb_identity": {}}}, ["t1.w1"])
case("loop_rest", "_loop_rest", {"drivers": DRIVERS}, ["t1.w1", True])
case("loop_rest_off", "_loop_rest", {"drivers": {"router": {}}}, ["t1.w1", False])

# ---- _relevant_context
ROW = {"source": "browser", "source_id": "ev-1", "what": "page", "data": {"url": "https://example.test/p?q=secret#f",
                                                                          "title": "Ünïcode page"}}
MANY = [{"source": "files", "source_id": f"f-{i}", "data": {"path": f"/Users/o/Documents/note-{i}.txt",
                                                            "text": "word " * 40}} for i in range(14)]
case("activity_one", "_relevant_context", {"activity": [ROW]}, ["page"])
case("activity_none", "_relevant_context", {"activity": []}, ["nothing"])
case("activity_mixed", "_relevant_context", {"activity": [
    ROW, {"what": "observed", "data": {"tool": "mac.click"}}, {"source": "messages", "source_id": "m-1",
                                                              "data": {"from": "someone"}},
    {"source": "app", "id": "a-1", "detail": "no data key", "url": "http://x.test/?a=1"},
    {"source": "", "source_id": "z"}]}, ["page"])
case("activity_limit", "_relevant_context", {"activity": MANY + [ROW]}, ["word"])
case("activity_mail", "_relevant_context", {"activity": [{"source": "mail", "source_id": "mail-1",
                                                          "data": {"subject": "Invoice"}}]}, ["invoice"])

# ---- _availability
REG_ALL = [["browser:act", "browser:read"], ["mac:read", "mac:act"], ["screen"], ["audio"], ["fs:read"],
           ["applescript"], ["trail:read"]]
case("avail_all_ok", "_availability", {
    "registered": REG_ALL, "pairings": PAIRED, "identity_profile": ["chrome", "Profile 3"], "identity_browser": True,
    "profiles": PROFILES, "permissions": {"accessibility": True, "screen": True}}, [], FERB_CONFIG)
case("avail_nothing_paired", "_availability", {
    "registered": REG_ALL, "pairings": [], "identity_profile": ["chrome", "Profile 3"], "identity_browser": False,
    "permissions": {"accessibility": False, "screen": False}})
case("avail_no_pairings_store", "_availability", {
    "registered": [["browser:act"]], "pairings": None, "browsing": {"as_owner": True, "label": "Chrome “Work”"},
    "permissions": {}})
case("avail_declined", "_availability", {
    "registered": [["browser:act"], ["audio"]], "pairings": PAIRED[3:], "browsing": {"declined": True},
    "profiles": PROFILES, "permissions": {"screen": False}})
case("avail_unasked", "_availability", {"registered": [["browser:act", "screen"]], "pairings": PAIRED[:2],
                                        "browsing": {}, "profiles": [], "permissions": {"accessibility": False}})
case("avail_no_browser_scope", "_availability", {"registered": [["mac:read"], ["fs:read"]], "pairings": PAIRED,
                                                 "permissions": {"accessibility": False, "screen": False}})
case("avail_empty", "_availability", {"registered": [], "permissions": {}})

# ---- _facts
case("facts_mailbox", "_facts", {"about": "FERB 0.27.3 on this Mac.", "secrets": [
    {"key": "github", "account_hint": "o***@example.test", "allowed_origins": ["https://github.com"]},
    {"key": "bank", "account_hint": "", "allowed_origins": ["https://a.test", "https://b.test"]}],
    "missing_dev_tools": ["git", "make"], "dev_tools_installed": False,
    "time_meanings": [["tonight", "after 18:00"], ["morning", "before 11:00 “local”"]]},
    [], '[mailbox]\naddress = "ferb.invented@example.test"\n')
case("facts_tools_installed", "_facts", {"missing_dev_tools": ["git"], "dev_tools_installed": True},
     [], '[mailbox]\naddress = "ferb.invented@example.test"\n')
case("facts_no_account_chosen", "_facts", {"profiles": PROFILES},
     [], '[browsing]\nowner_profile = "chrome/Profile 1"\n')
case("facts_no_account_chosen_unknown", "_facts", {"profiles": PROFILES},
     [], '[browsing]\nowner_profile = "brave/Gone"\n')
case("facts_no_account_none", "_facts", {"profiles": PROFILES}, [], '[browsing]\nowner_profile = ""\n')
case("facts_no_account_half", "_facts", {"profiles": PROFILES}, [], '[browsing]\nowner_profile = "chrome/"\n')
case("facts_no_account_unset", "_facts", {"about": "", "missing_dev_tools": []})

# ---- _live_facts
HUB = {"bridges": {"chrome/Default": {"conn": True, "version": "0.19.4"},
                   "brave/Default": {"conn": False, "version": "0.18.0"}},
       "unpaired": {"x": "edge", "y": "arc", "z": "edge"}, "problems": {"chrome/Profile 9": "its FERB extension is too old"}}
LIVE = {"models": {"planner": "gpt-x", "worker": "claude-y", "checker": "gpt-x"},
        "schedule_facts": {"plain": ["- Schedules: 1", "- daily 09:00"], "last_runs": ["- Schedules: 1",
                                                                                     "- daily 09:00 (last run ok)"]},
        "reminder_facts": ["- Reminders: none"], "pairings": PAIRED, "identity_profile": ["chrome", "Profile 3"],
        "profiles": PROFILES, "hub": HUB, "pairing_versions": {"inst-2": "0.17.1", "inst-4": "0.16.0"},
        "drivers": {"router": {"wake": True}}, "unpaired_notes": {"edge": "Microsoft Edge has profiles FERB could pair"}}
case("live_full", "_live_facts", LIVE)
case("live_last_runs", "_live_facts", LIVE, [True, True])
case("live_preview", "_live_facts", LIVE, [False, False])
case("live_no_wake", "_live_facts", {**LIVE, "drivers": {"router": {"wake": False}}})
case("live_no_router", "_live_facts", {**LIVE, "drivers": {}, "schedule_facts": None, "reminder_facts": None})
case("live_no_hub", "_live_facts", {**LIVE, "hub": None})
case("live_minimal", "_live_facts", {"models": {}})

# ---- _keychain_values and _preload_secrets (the key store faked: the providers asked, problems, values, a failure)
RT = {"orchestrator": "openai", "worker": "anthropic", "checker": "openai"}
KEY_A, KEY_B = "invented-key-value-0001-abcdefghij", "invented-key-value-0002-klmnopqrst"
PROBE = f"echo {KEY_A} and {KEY_B}"
case("keys_two", "_keychain_values", {"runtime_for": RT, "keys": {"values": [KEY_A, KEY_B]}, "probe": PROBE})
case("keys_fail", "_keychain_values", {"runtime_for": RT, "keys": {"raise": True}, "probe": PROBE})
case("preload_two", "_preload_secrets", {"runtime_for": RT, "keys": {"values": [KEY_A, KEY_B, KEY_A],
                                                                      "problems": [{"provider": "openai",
                                                                                    "error": "denied"}]},
                                         "probe": PROBE})
case("preload_none", "_preload_secrets", {"runtime_for": {"orchestrator": "codex", "worker": "codex",
                                                          "checker": "claude"}, "keys": {"values": []},
                                          "probe": PROBE})
case("preload_fail", "_preload_secrets", {"runtime_for": RT, "keys": {"raise": True,
                                                                      "problems": [{"provider": "x"}]},
                                          "probe": PROBE})


def main() -> None:
    out = []
    try:
        for c in CASES:
            out.append(run(c))
    finally:
        shutil.rmtree(ROOT, ignore_errors=True)
    text = json.dumps({"cases": out}, ensure_ascii=False, indent=1).replace(str(SUPPORT), "/SUPPORT")
    (HERE / "environment.json").write_text(text + "\n", encoding="utf-8")
    print(f"environment.json: {len(out)} cases, {len(text)} bytes")


if __name__ == "__main__":
    main()
