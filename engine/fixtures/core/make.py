"""Fixtures for the Rust core port (rust/crates/ferb-core), made by the Python code (ENG-3).

    cd <repo> && uv run python rust/fixtures/core/make.py

runs ferb/core (types, ids, errors, timing, call_facts), ferb/paths.py, ferb/config.py and ferb/bus.py on invented
data, in temporary folders only (never ~/.ferb), and writes next to this file:

    types.json    each shared type: invented instances as Python serializes them, and ActionFacts.recorded()
    ids.json      id shapes and the deterministic ids
    errors.json   each error's class, bases and message
    timing.json   Steps and CallTimer scripts on an injected clock
    paths.json    FERB's home and socket under invented FERB_HOME values; app bundle and installed-app answers
    config.json   config.toml inputs, the operations on them, each answer and the file's text after each write
    bus.json      the topics, and scripts of subscribe / publish / read on a fake clock

Everything here is invented; nothing is read from the owner's Mac.
"""

import asyncio
import dataclasses
import json
import os
import plistlib
import stat
import sys
import tempfile
from datetime import datetime, timedelta, timezone
from enum import Enum
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO))
HERE = Path(__file__).resolve().parent

from ferb import bus as busmod  # noqa: E402
from ferb import config as configmod  # noqa: E402
from ferb import paths  # noqa: E402
from ferb.clock.clock import FakeClock  # noqa: E402
from ferb.core import call_facts, errors, ids, timing  # noqa: E402
from ferb.core import types as t  # noqa: E402

UTC = timezone.utc
IST = timezone(timedelta(hours=5, minutes=30))
WEST = timezone(-timedelta(hours=8))


def write(name: str, value: Any) -> None:
    (HERE / name).write_text(json.dumps(value, indent=1, ensure_ascii=False) + "\n")


# ---------- types ----------

def iso(value: Any) -> Any:
    """json.dumps `default` for dataclasses.asdict output: ferb/store/spans.py node_json's form for datetimes
    (isoformat), an enum by its value, a set as a sorted list."""
    if isinstance(value, datetime):
        return value.isoformat()
    if isinstance(value, Enum):
        return value.value
    if isinstance(value, (set, frozenset)):
        return sorted(value)
    raise TypeError(type(value))


def as_json(obj: Any) -> str:
    return json.dumps(dataclasses.asdict(obj), default=iso)


def types_fixture() -> dict[str, Any]:
    at = datetime(2026, 3, 4, 9, 30, 15, tzinfo=UTC)
    micro = datetime(2026, 3, 4, 15, 0, 15, 250, tzinfo=IST)
    west = datetime(2025, 12, 31, 23, 59, 59, 999999, tzinfo=WEST)
    tab = t.TabRef("dev-1", "chrome", "Profile 7", 3, "tab-41")
    received = t.ReceivedBy("com.example.notes", "Shopping list", None)
    facts_full = t.ActionFacts(
        submits_form=True, form_action_host="pay.example.com", form_method="POST",
        field_kinds=("card_number", "card_csc"), page_amount="€12.50", page_merchant="Example Shop",
        page_amounts=("€12.50", "USD 3"), received_by=received, secure_field=False, terminal_target=False,
        target_role="AXButton", target_subrole="AXSecureTextField", page_url="https://pay.example.com/checkout",
        frame_host="cards.example.net", tab_chain="tab-41", deletes=("/tmp/invented/a.txt",),
        writes=("/tmp/invented/b.txt", "/tmp/invented/c.txt"), control_label="Pay now")
    facts_some = t.ActionFacts(submits_form=False, received_by=t.ReceivedBy(), field_kinds=("password",))
    terminal = t.TerminalInput(app="Terminal", bundle_id="com.apple.Terminal", surface="a terminal app",
                               text="echo héllo", enter=True, window="invented — zsh",
                               files=(("/tmp/invented/run.command", "ab" * 32, "echo hi"),), named=("notes.txt",))
    submit = t.FormSubmit(True, False, "example.com", "POST", ("password",), "Sign in")
    element = t.PageElement("e1", "f0", "textbox", "Email", None, (1.5, 2.0, 300.25, 40.0), True,
                            input_type="email", state=("disabled",), level=None, submit=submit, backend_node=77)
    instances: dict[str, list[Any]] = {
        "SecretRef": [t.SecretRef("ferb/email/password")],
        "SecretSpec": [t.SecretSpec(t.SecretRef("ferb/email/password"), ["https://accounts.example.com"], "f@example.com")],
        "Task": [
            t.Task("t1", None, "Tidy the invented folder", "dev-1", t.Identity.FERB, {"fs.write", "browser"}, "worker",
                   "r1", ["the folder holds 3 files"], None),
            t.Task("t2", "t1", "Check it — ✓", "dev-1", t.Identity.OWNER, set(), "verifier", "r1", [], micro),
            t.Task("t3", "t1", "", "dev-2", t.Identity.OWNER, {"a"}, "mixed", "r2", ["x", "y"], west),
        ],
        "Outcome": [t.Outcome("t1", status, "It is done.", ["/tmp/invented/out.pdf"], ["span:s1"])
                    for status in ("done", "failed", "unknown", "cancelled", "needs_user", "blocked")],
        "ActionIntent": [t.ActionIntent("i1", "t1", "browser.click", {"site": "example.com", "n": 2}, "ab" * 32, state)
                         for state in ("intended", "executing", "done", "failed", "unknown")],
        "Span": [
            t.Span("s1", None, "task", None, None, at, None, {}, None, None, None),
            t.Span("s2", "s1", "tool.call", "t1", "dev-1", micro, west, {"tool": "fs.read", "args": {"path": "a"}},
                   {"ok": True, "n": [1, 2.5, None]}, 0.0125, "it failed: naïve", "owner,external:web", "read a"),
        ],
        "TrailEvent": [t.TrailEvent("e1", at, "dev-1", actor, "t1" if actor == "ferb" else None, "ax",
                                    {"app": "com.example.notes"}) for actor in ("owner", "ferb", "unknown")],
        "Target": [t.Target(), t.Target("gmail:someone@example.com", "mail.example.com", ("a@example.com", "b@example.com"),
                                        "/tmp/invented/x", 12.5, "EUR", "Sam Example", "Checkout – Example", "3 notes")],
        "ReceivedBy": [t.ReceivedBy(), received],
        "ActionFacts": [t.ActionFacts(), facts_some, facts_full],
        "ActionSpec": [t.ActionSpec("read"),
                       t.ActionSpec("send", t.Target(recipients=("a@example.com",)), "hello", "send", "Mail", "pay",
                                    facts_full)],
        "ModelLimits": [t.ModelLimits("invented-1", 200000, 8000, "https://example.com/models"),
                        t.ModelLimits("invented-2", 128000, 4096, "docs", False, 1048576)],
        "RuntimeEvent": [t.RuntimeEvent(kind, {"text": "hi", "n": 1} if kind == "final" else {})   # type: ignore[arg-type]
                         for kind in ("message", "text_delta", "tool_call", "usage", "final", "error", "violation",
                                      "invalid_answer", "retrying", "progress", "compacted")],
        "TerminalInput": [t.TerminalInput("iTerm", "com.googlecode.iterm2", "a terminal app"), terminal],
        "TerminalGrant": [t.TerminalGrant(terminal, "c-1")],
        "TabRef": [tab],
        "ElementRef": [t.ElementRef(tab, "f0", 12, "e9")],
        "DriverFeatures": [t.DriverFeatures(True, False, True, True, False)],
        "WaitCondition": [t.WaitCondition("load"), t.WaitCondition("text", "Thanks"), t.WaitCondition("url", "https://example.com/done")],
        "ActionResult": [t.ActionResult(True), t.ActionResult(False, "no such element", {"tries": 2}, facts_some)],
        "FormSubmit": [submit, t.FormSubmit(False, True, None, None)],
        "PageElement": [t.PageElement("e0", "f0", "button", "Go", "v", (0.5, 0.25, 10.75, 20.5), False), element],
        "PageSnapshot": [
            t.PageSnapshot(tab, 4, "https://example.com/", "Example"),
            t.PageSnapshot(tab, 5, "https://example.com/a", "A", [element], "text", True, ["alert: hi"],
                           {"y": 0, "height": 900, "viewport_h": 600, "at_end": False}, [{"id": "s1", "y": 3}],
                           [{"index": 0, "playing": True}], ["https://frames.example.com/x"], True, "application/pdf"),
        ],
        "ComputerAction": [
            t.ComputerAction("press", "el-3"),
            t.ComputerAction("edit", "el-4", "new text", [2, 5], None, None, None, True, "ax_value"),
            t.ComputerAction("edit", "el-4", "x", "all"),
            t.ComputerAction("scroll", None, None, None, "down", 3, "Notes", False, "foreground"),
        ] + [t.ComputerAction(kind, mechanism=mech)   # type: ignore[arg-type]
             for kind, mech in (("set_value", "ax"), ("focus", "pid"), ("click", None), ("type", None), ("key", None),
                                ("select", None), ("cancel", None))],
    }
    out: dict[str, Any] = {name: [as_json(i) for i in items] for name, items in instances.items()}
    out["Identity"] = [json.dumps(i.value) for i in t.Identity]
    out["RiskLevel"] = [json.dumps(r.value) for r in t.RiskLevel]
    # A memory item goes to a model as json.dumps(asdict(item), ensure_ascii=False, default=str)
    # (ferb/orchestrator/evidence.py:360,501): its datetimes are str(datetime), with a space.
    memory = [
        t.MemoryItem("m1", "fact", "owner", "dentist", "The dentist is Dr Example — on Elm St", ["s1", "s2"], at, at, None,
                     None, 0.9, "owner_stated", "active"),
        t.MemoryItem("m2", "preference", "owner", "tea", "Prefers green tea", [], micro, west, at, "m1", 0.5, "inferred",
                     "proposed", "a chat about tea", micro, True),
    ] + [t.MemoryItem(f"m{i}", kind, "s", "k", "c", [], at, at, None, None, 1.0, origin, status)   # type: ignore[arg-type]
         for i, (kind, origin, status) in enumerate((("person", "observed", "retracted"), ("project", "third_party", "active"),
                                                     ("env", "observed", "active")), 3)]
    out["MemoryItem"] = [json.dumps(dataclasses.asdict(m), ensure_ascii=False, default=str) for m in memory]
    recorded = [{"facts": as_json(f), "recorded": json.dumps(f.recorded())}
                for f in (t.ActionFacts(), facts_some, facts_full, t.ActionFacts(secure_field=False, deletes=("a",)))]
    image = t.ModelImage(bytes(range(40)), "evidence:7")
    limits = [{"limits": as_json(m), "context_budget": m.context_budget} for m in instances["ModelLimits"]]
    request = t.ModelRequest("be brief", ("stable part", "second", "the request"), 2)
    datetimes = [{"iso": d.isoformat(), "str": str(d)} for d in (
        at, micro, west, datetime(2026, 1, 1, 0, 0, tzinfo=UTC), datetime(1999, 7, 8, 1, 2, 3, 40000, tzinfo=IST),
        datetime(2026, 6, 1, 12, 0, 0, 7, tzinfo=timezone(timedelta(hours=-3, minutes=-30))))]
    return {"asdict": out, "recorded": recorded, "limits": limits, "datetimes": datetimes,
            "image": {"hex": image.data.hex(), "ref": image.ref, "mime": image.mime, "base64": image.base64(),
                      "data_url": image.data_url(), "repr": repr(image)},
            "request": {"system": request.system, "parts": list(request.parts), "context": request.context},
            "escape_grant_repr": repr(t.EscapeGrant("rule", "c-1", "ls", "/tmp/invented", "hidden-value"))}


# ---------- ids, errors ----------

def ids_fixture() -> dict[str, Any]:
    seq = ids.SequentialIds("task")
    plain = ids.SequentialIds()
    return {"random": [ids.RandomIds().new_id() for _ in range(8)],
            "sequential_task": [seq.new_id() for _ in range(12)],
            "sequential_default": [plain.new_id() for _ in range(2)]}


def errors_fixture() -> list[dict[str, Any]]:
    made: list[BaseException] = [
        errors.FerbError("base"), errors.RuntimeUnavailable("codex isn't installed"), errors.Unsupported("can't"),
        errors.ModelLimitsUnknown("no window"), errors.InputTooLarge(1200000, 1048576),
        errors.InputTooLarge(5, 4, "the plan"), errors.PlanRejected(["Inventory paths are protected: ['~/x']", "two"]),
        errors.PlanRejected([]), errors.AuthError("bad token"), errors.NotRunning("gone"),
        errors.AlreadyRunning("there"), errors.Deferred("stopping"), errors.Refused("no"), errors.StaleRef("old tab"),
        errors.NoTarget("no window"), errors.Withheld("unpainted"), errors.DriverError("driver failed"),
        errors.DriverError("needs a route", facts={"route": "foreground", "n": 2}),
    ]
    rows = []
    for e in made:
        row: dict[str, Any] = {"class": type(e).__name__, "message": str(e),
                               "ferb_error": isinstance(e, errors.FerbError), "value_error": isinstance(e, ValueError),
                               "refused": isinstance(e, errors.Refused), "unsupported": isinstance(e, errors.Unsupported)}
        if isinstance(e, errors.InputTooLarge):
            row["args"] = {"chars": e.chars, "max_chars": e.max_chars, "runtime": e.runtime}
        if isinstance(e, errors.PlanRejected):
            row["args"] = {"problems": e.problems}
        if isinstance(e, errors.DriverError):
            row["args"] = {"facts": e.facts}
        rows.append(row)
    return rows


# ---------- timing, call facts ----------

class Ticks:
    def __init__(self) -> None:
        self.t = 100.0

    def __call__(self) -> float:
        return self.t


def timing_fixture() -> dict[str, Any]:
    step_scripts = [
        [["mark", 0.0104, "plan"], ["mark", 0.2505, "call"], ["mark", 0.2505, "call"], ["mark", 1.0, "plan"]],
        [["mark", 0.0005, "a"], ["mark", 0.0015, "b"], ["mark", 0.0025, "c"], ["mark", 0.001, "back"]],
        [],
        [["mark", 3.14159, "π"], ["mark", 2.0, "earlier"], ["mark", 3.2, "later"]],
    ]
    steps_out = []
    for script in step_scripts:
        now = Ticks()
        steps = timing.Steps(now)
        took = []
        for _, at, name in script:
            now.t = 100.0 + at
            took.append(steps.mark(name))
        steps_out.append({"script": script, "took": took, "facts": json.dumps(steps.facts()), "total_ms": steps.total_ms})
    call_scripts = [
        # tests/test_turn_timing.py: Codex's usage report after its tool already ran
        ["runtime", [["begin", 0], ["sent", 0.4], ["output", 4.4], ["output", 4.6], ["handed_back", 4.605],
                     ["sent", 4.61], ["done", 4.625], ["output", 9.0], ["done", 9.5]]],
        ["arrival", [["output", 1.0], ["output", 2.5], ["done", 2.5], ["done", 3.0]]],
        ["runtime", [["done", 0.0]]],
        ["runtime", [["begin", 1.0], ["handed_back", 1.5], ["sent", 1.75], ["output", 2.0005], ["done", 2.0015],
                     ["handed_back", 3.0], ["output", 3.5], ["done", 4.0]]],
        ["runtime", [["sent", 0.5], ["done", 0.75], ["begin", 1.0], ["output", 1.2], ["sent", 1.3], ["done", 1.4],
                     ["output", 1.9], ["done", 2.0]]],
    ]
    calls_out = []
    for by, script in call_scripts:
        now = Ticks()
        timer = timing.CallTimer(now, by)
        done = []
        for op, at in script:
            now.t = 100.0 + at
            result = getattr(timer, op)()
            if op == "done":
                done.append(json.dumps(result))
        calls_out.append({"by": by, "script": script, "done": done})
    with call_facts.recording() as held:
        call_facts.note("point", "ax")
        call_facts.note("point", "ocr")
        call_facts.note("route", "background")
        with call_facts.recording() as inner:
            call_facts.note("point", "inner")
        call_facts.note("point", "after")
    call_facts.note("point", "nobody records this")
    return {"steps": steps_out, "calls": calls_out,
            "call_facts": {"outer": json.dumps(held), "inner": json.dumps(inner)}}


# ---------- paths ----------

def paths_fixture(tmp: Path) -> dict[str, Any]:
    before = os.environ.get("FERB_HOME")
    homes = []
    try:
        for given in ("/tmp/invented/ferb-home", "/tmp/invented/ferb-home/", "/tmp//invented/./home", "relative/home",
                      "//double/lead", "///triple/lead", "", ".", "/", "/tmp/invented/dir/..", "~/.ferb"):
            os.environ["FERB_HOME"] = given
            homes.append({"given": given, "home": str(paths.ferb_home()), "socket": str(paths.socket_path())})
    finally:
        if before is None:
            os.environ.pop("FERB_HOME", None)
        else:
            os.environ["FERB_HOME"] = before
    explicit = {"home": "/tmp/invented/other", "socket": str(paths.socket_path(Path("/tmp/invented/other")))}
    bundles = []
    for prefix in ("apps/FERB.app/Contents/Resources/python", "apps/Other Name.app/Contents/Resources/python",
                   "apps/FERB.app/Contents/Resources/python/bin", "apps/FERB/Contents/Resources/python",
                   "apps/FERB.app/Contents/Frameworks/python", "venv"):
        (tmp / prefix).mkdir(parents=True, exist_ok=True)
        found = paths.app_bundle(tmp / prefix)
        bundles.append({"prefix": prefix, "bundle": None if found is None else str(found.relative_to(tmp.resolve()))})
    apps = {"good": {"CFBundleIdentifier": paths.APP_ID, "CFBundleName": "FERB"},
            "other": {"CFBundleIdentifier": "com.example.other"}, "no_id": {"CFBundleName": "FERB"}}
    for name, info in apps.items():
        contents = tmp / "installed" / name / "FERB.app" / "Contents"
        contents.mkdir(parents=True)
        with (contents / "Info.plist").open("wb") as f:
            plistlib.dump(info, f, fmt=plistlib.FMT_BINARY if name == "good" else plistlib.FMT_XML)
    broken = tmp / "installed" / "broken" / "FERB.app" / "Contents"
    broken.mkdir(parents=True)
    (broken / "Info.plist").write_text("not a plist")
    installed = []
    for order in (["missing", "good"], ["other", "no_id", "broken"], ["broken", "other", "good"], ["good", "other"], []):
        found = paths.installed_app(tuple(tmp / "installed" / n / "FERB.app" for n in order))
        installed.append({"candidates": order, "found": None if found is None else found.parent.name})
    return {"app_id": paths.APP_ID, "homes": homes, "explicit": explicit, "bundles": bundles,
            "plists": {**apps, "broken": "not a plist"}, "good_is_binary": True, "installed": installed}


# ---------- config ----------

EXISTING = """# FERB runtime config. Owner-only (0600).

[secrets."ferb/email/password"]
allowed_origins = ["https://accounts.example.com"]  # keep this comment
account_hint = "ferb@example.com"
"""

OLD_LIMITS = """[browser]
download_max_mb = 100   # per file
download_task_max_files = 20
download_task_max_mb = 500
download_total_mb = 1000 # the owner's own
task_max_tabs = 20

[retention]
download_days = 30
trace_days = 14
"""

OLD_DOWNLOADS = """# invented older config
[browser]
download_max_mb = 512
download_task_max_files = 50
download_task_max_mb = 4096
download_total_mb = 8192
download_min_free_gb = 10

[voice]
tts = "macos"
voice = "Daniel"
rate = 190

[feedback]
address = "dev@example.org"

[test_mode]
state = "on"
"""

MIXED = """title = "top level"   # a bare key

[privacy]
exclude_apps = [
    "com.example.one",   # first
    "com.example.two",
]
exclude_domains = []

[sense]
window_titles = false
idle_after_s = 45.5

[policy]
autonomy = 'careful'
nested = { a = 1, b = [true, false] }

[[runs]]
name = "first"

[[runs]]
name = "second"
"""

SET_VALUES: list[Any] = ["careful", "", 'identifier "com.example.app" and anchor', "tab\there", "back\\slash",
                         "line\nbreak", "naïve — ✓", "it's", "\x7f\x01 control", True, False, 0, -7, 9007199254740993, 1.5,
                         8.59, 0.1, 100.0, 1e16, 1.5e-7, -0.0, [], ["a"], ["b", "a", "it's \"q\""], [1, 2, 3], [True, 1.5, "x"],
                         [["a"], []]]

PRIVACY = {"never_read": ["~/.ssh", "~/Library/Keychains", "~/invented folder"],
           "never_write": ["~/.ssh", "/System"], "terminal_apps": ["com.apple.Terminal", "com.example.term"],
           "exclude_paths": ["~/.ssh", "~/.ferb"]}


def config_cases() -> list[tuple[str, str | None, list[list[Any]]]]:
    cases: list[tuple[str, str | None, list[list[Any]]]] = [
        ("missing file: reads and first write", None,
         [["get", "policy", "autonomy", "balanced"], ["section", "policy"], ["exclude_apps"],
          ["remove", "policy", ["autonomy"]], ["add_defaults", "privacy", "exclude_apps", ["x"]],
          ["set", "policy", "autonomy", "careful"], ["get", "policy", "autonomy", None], ["set", "policy", "level", 3],
          ["set", "browser", "task_max_tabs", 50], ["set", "policy", "autonomy", "hands-off"]]),
        ("existing: comments and other tables stay", EXISTING,
         [["get", "secrets", "nope", None], ["get", "secrets", "nope", 3], ["section", "secrets"], ["section", "nope"],
          ["get", "secrets", "ferb/email/password", None], ["set", "policy", "autonomy", "careful"],
          ["set", "policy", "autonomy", "balanced"], ["remove", "policy", ["autonomy"]],
          ["remove", "policy", ["autonomy"]], ["set", "policy", "again", True], ["exclude_apps"]]),
        ("every value kind set", "", [["set", "v", f"k{i}", v] for i, v in enumerate(SET_VALUES)]
         + [["section", "v"], ["set", "v", "k0", 5], ["set", "v", "k22", ["z"]], ["set", "v", "k9", "now text"]]),
        ("mixed file: reads, edits in place", MIXED,
         [["get", "privacy", "exclude_apps", None], ["exclude_apps"], ["section", "sense"], ["section", "policy"],
          ["get", "title", "x", "d"], ["section", "title"], ["section", "runs"], ["get", "runs", "name", "d"],
          ["set", "sense", "window_titles", True], ["set", "sense", "idle_after_s", 120],
          ["set", "policy", "autonomy", "balanced"], ["add_defaults", "privacy", "exclude_apps", ["com.example.two", "com.example.three"]],
          ["add_defaults", "privacy", "exclude_domains", ["example.com", "example.org"]],
          ["add_defaults", "privacy", "exclude_domains", ["example.com"]], ["add_defaults", "privacy", "missing", ["x"]],
          ["add_defaults", "sense", "idle_after_s", ["x"]], ["add_defaults", "nope", "k", ["x"]],
          ["set", "privacy", "exclude_apps", ["only"]], ["remove", "sense", ["window_titles", "nope", "idle_after_s"]],
          ["remove", "policy", ["nested"]], ["set", "new", "k", 1]]),
        ("privacy defaults on a missing file", None, [["ensure_privacy_defaults"], ["ensure_privacy_defaults"],
                                                      ["section", "browser"], ["exclude_apps"]]),
        ("privacy defaults keep the owner's values", "[privacy]\nexclude_apps = []   # none\n\n[retention]\ntrace_days = 3\n"
         "\n[protection]\nlocal_ports = [8080]\n", [["exclude_apps"], ["ensure_privacy_defaults"], ["exclude_apps"],
                                                   ["get", "retention", "trace_days", None]]),
        ("privacy defaults on the existing file", EXISTING, [["ensure_privacy_defaults"]]),
        ("limits decided 2026-10-09", OLD_LIMITS, [["upgrade", configmod.LIMITS_DECIDED], ["upgrade", configmod.LIMITS_DECIDED],
                                                  ["section", "browser"], ["section", "retention"]]),
        ("every upgrade in order", OLD_DOWNLOADS, [["upgrade", name] for name in configmod.UPGRADES]
         + [["upgrade", name] for name in configmod.UPGRADES] + [["upgrade", "no-such-upgrade"]]),
        ("downloads: owner's own values carry over in GB",
         "[browser]\ndownload_task_max_files = 80\ndownload_task_max_mb = 1000\ndownload_total_mb = 2500.5\n"
         "download_task_max_gb = 1\n", [["upgrade", configmod.DOWNLOADS_DECIDED], ["section", "browser"]]),
        ("downloads: values that are not sizes", '[browser]\ndownload_task_max_mb = "big"\ndownload_total_mb = -4\n'
         "download_max_mb = true\n", [["upgrade", configmod.DOWNLOADS_DECIDED], ["section", "browser"]]),
        ("downloads: a boolean and zero", "[browser]\ndownload_task_max_mb = true\ndownload_total_mb = 0\n",
         [["upgrade", configmod.DOWNLOADS_DECIDED], ["section", "browser"]]),
        ("downloads: no browser table", "[other]\nk = 1\n", [["upgrade", configmod.DOWNLOADS_DECIDED]]),
        ("retire leaves a table that still has keys", '[voice]\ntts = "macos"\nrequirement = "identifier x"\n\n[feedback]\n'
         'address = "a@example.org"\nrelay_url = "https://relay.example.org/r"\n',
         [["upgrade", configmod.VOICE_DECIDED], ["upgrade", configmod.FEEDBACK_DECIDED], ["section", "voice"],
          ["section", "feedback"]]),
        ("retire removes a middle table", '[a]\nk = 1\n\n[test_mode]\nstate = "off"\n\n[z]\nk = 2\n',
         [["upgrade", configmod.TEST_MODE_DECIDED]]),
        ("retire removes the only table", '# top comment\n[test_mode]\nstate = "off"\n', [["upgrade", configmod.TEST_MODE_DECIDED]]),
        ("no trailing newline", "[a]\nk = 1", [["set", "a", "j", 2], ["set", "b", "k", 3]]),
        ("top-level keys only", 'name = "x"\n', [["set", "a", "k", 1], ["remove", "a", ["k"]]]),
        ("dotted and quoted keys", '[a]\nb.c = 1\n"quoted key" = 2\n\n[a.sub]\nk = 3\n',
         [["section", "a"], ["set", "a", "plain", 4], ["set", "a", "needs quoting", 5], ["set", "a", "", 6],
          ["remove", "a", ["quoted key"]], ["set", "sec tion", "k", 7]]),
        ("a section that is not a table", "policy = 3\n", [["get", "policy", "autonomy", "d"], ["section", "policy"]]),
    ]
    return cases


ARRAYS = ['[]', '[ ]', '["a"]', '[ "a" ]', '["a",]', '["a", "b"] # c', '[\n  "a",\n  "b"\n]', '[\n  "a",\n  "b",\n]',
          '[\n    "a",   # first\n    "b",\n]', '[\n"a"\n]', '["a",\n "b"]', '[\n]', '[\n  # none yet\n]', "[1, 2]",
          '[\n\t"a",\n]', '[\n  "a", "b",\n  "c",\n]', '[\n  "a",\n  "b" # second\n]', '[\n  "a",\n  "b", # second\n]',
          '[\n "a", "b"\n]', '[\n  "a",\n  "b",\n  # tail\n]', '[ # open\n  "a",\n]']
T = configmod.TEST_MODE_DECIDED

# One edit each, on the shapes a config.toml can take: (the file, the operation).
EDITS: list[tuple[str, list[Any]]] = [
    # a new key in a table that is there
    ("[a]\nk = 1\n# c\n\n[b]\nj = 2\n", ["set", "a", "n", 9]),
    ("[a]\nk = 1\n\n# about b\n[b]\nj = 2\n", ["set", "a", "n", 9]),
    ("[a]\n\n[b]\nj = 2\n", ["set", "a", "n", 9]),
    ("[a]\n", ["set", "a", "n", 9]),
    ("[a]", ["set", "a", "n", 9]),
    ("[a]\nk = 1\n[b]\nj = 2\n", ["set", "a", "n", 9]),
    ("[a]\nk = 1\n\n\n[b]\nj = 2\n", ["set", "a", "n", 9]),
    ("[a]\n  k = 1\n\n[b]\n", ["set", "a", "n", 9]),
    ("[a] # hc\nk = 1 # kc\n", ["set", "a", "n", 9]),
    ("[a]\nk = 1\n\n\n", ["set", "a", "n", 9]),
    ("[a.sub]\nk = 1\n\n[a]\nj = 2\n\n[c]\n", ["set", "a", "n", 9]),
    ("[a]\r\nk = 1\r\n\r\n[b]\r\nj = 2\r\n", ["set", "a", "n", 9]),
    ("k = 1\n\n[a]\nj = 1\n", ["set", "a", "n", [1.0, "x"]]),
    ('[a]\nk = """\nx\n\n"""\n\n[b]\n', ["set", "a", "n", 9]),
    # a new table
    ("k = 1\n# end", ["set", "s", "n", 9]),
    ("[a]\nk = 1\n\n\n", ["set", "s", "n", 9]),
    ("# only\n", ["set", "s", "n", 9]),
    ("[a]\r\nk = 1\r\n", ["set", "s", "n", 9]),
    ("[a]\nk = 1", ["set", "s", "n", 9]),
    ("k = 1", ["set", "s", "n", 9]),
    ("\n", ["set", "s", "n", 9]),
    ("\n\n", ["set", "s", "n", 9]),
    ("[a]\nk = 1\n# end\n", ["set", "s", "n", 9]),
    ("[a]\nk = 1\n\n", ["set", "s", "n", 9]),
    ("[a]\n", ["set", "s", "n", 9]),
    ("[[r]]\nk = 1\n", ["set", "s", "n", 9]),
    # a value replaced where it stands
    ("[a]\nk = 1   # c\nj = [\n 1,\n 2, # x\n]  # t\n", ["set", "a", "j", [3]]),
    ("[a]\nb = { c = 1 } # c\nz = 1\n", ["set", "a", "b", "x"]),
    ('[a]\nb = """\nx\ny"""\nz = 1\n', ["set", "a", "b", "x"]),
    ("[a]\nb=1\nc   =   2  \n", ["set", "a", "c", 6]),
    ("[a]\nb=1\nc   =   2  \n", ["set", "a", "b", 5]),
    ("[a]\n'lit key' = 'old'\n", ["set", "a", "lit key", "new"]),
    # a key removed
    ("[a]\n# about k\nk = 1 # kc\nj = 2\n", ["remove", "a", ["k"]]),
    ("[a]\nk = [\n 1,\n]\nj = 2\n", ["remove", "a", ["k"]]),
    ("[a]\nj = 2\nk = 1", ["remove", "a", ["k"]]),
    ("[a]\n  j = 2\n  k = 1\n  l = 3\n", ["remove", "a", ["k", "j"]]),
    ('[secrets."x/y"]\nk = 1\n', ["remove", "secrets", ["nope"]]),
    # a table left empty goes
    ("[a]\nk = 1\n\n# about t\n[test_mode]\nstate = 1\n# inside\n\n# about z\n[z]\nk = 2\n", ["upgrade", T]),
    ("[test_mode]\nstate = 1\n\n[z]\nk = 2\n", ["upgrade", T]),
    ("[test_mode]\nstate = 1\nother = 2\n\n[z]\nk = 2\n", ["upgrade", T]),
    ("[test_mode]\nstate = 1\n\n[test_mode.sub]\nk = 2\n", ["upgrade", T]),
    ("[test_mode]\nother = 2\n", ["upgrade", T]),
] + [(f"[s]\nk = {arr}\nz = 1\n", ["add_defaults", "s", "k", ["x", "y"]]) for arr in ARRAYS] + [
    ('[s]\nk = [\n  "a",\n]\n', ["add_defaults", "s", "k", ["x", "x", "a"]]),
    ('[s]\nk = ["a"]', ["add_defaults", "s", "k", ["it's \"q\"\n"]]),
]

# Edits tomlkit makes that the Rust port refuses (nothing written), or where Python itself raises.
REFUSED: list[tuple[str, list[Any]]] = [
    (EXISTING, ["set", "secrets", "extra", "x"]),                    # a table known only from its sub-tables
    ("a = { k = 1 }\n", ["set", "a", "n", 9]),                       # an inline table as the section
    ("a.k = 1\nz = 2\n", ["set", "a", "n", 9]),                      # a dotted key as the section
    ("[a]\nk = 1\n[a.s]\nq = 1\n", ["set", "a", "s", 5]),            # a value over a sub-table
    ("[a]\nb.c = 1\nz = 1\n", ["set", "a", "b", 5]),                 # a value over dotted keys
    ("[a]\nb.c = 1\nb.d = 2\nz = 1\n", ["remove", "a", ["b"]]),      # dotted keys removed
    ("[a]\nk = 1\n\n[a.s]\nq = 1\n\n[b]\n", ["remove", "a", ["s"]]),  # a sub-table removed
    ("[a]\nk = 1\n", ["set", "a", "t", {"x": 1, "y": ["z"]}]),      # a table as the value
    ("a = 3\n", ["set", "a", "n", 9]),                               # Python: TypeError
    ("[[a]]\nk = 1\n", ["set", "a", "n", 9]),                        # Python: TypeError
]

# Edits where tomlkit's text is not reproduced by the Rust port; the values match, the text differs: two array
# layouts, and a table between the parts of an array of tables (tomlkit moves the parts together).
LAYOUT_DIFFERS: list[tuple[str, list[Any]]] = [
    (f"[s]\nk = {a}\nz = 1\n", ["add_defaults", "s", "k", ["x", "y"]]) for a in ('[\n  "a"\n  ,\n]', '["a", # x\n]')
] + [("[[r]]\nk = 1\n\n[a]\nj = 1\n\n[[r]]\nk = 2\n", ["set", "a", "n", 9])]


def jsonable(value: Any) -> Any:
    if isinstance(value, frozenset):
        return sorted(value)
    return value


def run_config_case(base: Path, name: str, text: str | None, ops: list[list[Any]]) -> dict[str, Any]:
    home = base / "home"
    home.mkdir(mode=0o700)
    cfg = configmod.Config(home)
    if text is not None:
        cfg.path.write_text(text)
        cfg.path.chmod(0o600)
    steps = []
    for op in ops:
        kind, args = op[0], op[1:]
        error = None
        try:
            if kind == "get":
                result = cfg.get(args[0], args[1], args[2])
            elif kind == "section":
                result = cfg.section(args[0])
            elif kind == "set":
                result = cfg.set(args[0], args[1], args[2])
            elif kind == "remove":
                result = cfg.remove(args[0], *args[1])
            elif kind == "add_defaults":
                result = cfg.add_defaults(args[0], args[1], args[2])
            elif kind == "upgrade":
                result = cfg.upgrade(args[0])
            elif kind == "exclude_apps":
                result = cfg.exclude_apps()
            elif kind == "ensure_privacy_defaults":
                result = ensure_privacy_defaults(cfg)
            else:
                raise AssertionError(kind)
        except (ValueError, TypeError) as e:
            result, error = None, str(e)
        steps.append({"op": op, "result": json.dumps(jsonable(result)), "error": error,
                      "text": cfg.path.read_text() if cfg.path.exists() else None})
    mode = oct(stat.S_IMODE(cfg.path.stat().st_mode)) if cfg.path.exists() else None
    return {"name": name, "input": text, "steps": steps, "mode": mode, "home_mode": oct(stat.S_IMODE(home.stat().st_mode)),
            "values": json.dumps(cfg._values()), "left_behind": sorted(p.name for p in home.iterdir())}


def ensure_privacy_defaults(cfg: configmod.Config) -> None:
    """Config.ensure_privacy_defaults with the policy and sense lists replaced by invented ones (those lists belong to
    later ports; the Rust port takes them as an argument)."""
    from ferb.policy import protection
    from ferb.sense import exclusions
    saved = (protection.DEFAULT_NEVER_READ, protection.DEFAULT_NEVER_WRITE, protection.DEFAULT_TERMINAL_APPS,
             exclusions.DEFAULT_EXCLUDE_PATHS)
    protection.DEFAULT_NEVER_READ = tuple(PRIVACY["never_read"])
    protection.DEFAULT_NEVER_WRITE = tuple(PRIVACY["never_write"])
    protection.DEFAULT_TERMINAL_APPS = tuple(PRIVACY["terminal_apps"])
    exclusions.DEFAULT_EXCLUDE_PATHS = list(PRIVACY["exclude_paths"])
    try:
        cfg.ensure_privacy_defaults()
    finally:
        (protection.DEFAULT_NEVER_READ, protection.DEFAULT_NEVER_WRITE, protection.DEFAULT_TERMINAL_APPS,
         exclusions.DEFAULT_EXCLUDE_PATHS) = saved


def config_fixture(tmp: Path) -> dict[str, Any]:
    cases = []
    for i, (name, text, ops) in enumerate(config_cases()):
        base = tmp / f"config-{i}"
        base.mkdir()
        cases.append(run_config_case(base, name, text, ops))
    for text, op in EDITS:
        base = tmp / f"config-edit-{len(cases)}"
        base.mkdir()
        cases.append(run_config_case(base, f"edit: {op[0]} on {text!r}", text, [op]))
    refused, differs = [], []
    for group, items in ((refused, REFUSED), (differs, LAYOUT_DIFFERS)):
        for text, op in items:
            base = tmp / f"config-other-{len(refused)}-{len(differs)}"
            base.mkdir()
            group.append(run_config_case(base, f"{op[0]} on {text!r}", text, [op]))
    insecure = []
    for target, mode in (("file", 0o644), ("file", 0o620), ("file", 0o604), ("file", 0o640), ("home", 0o755), ("home", 0o710)):
        home = tmp / f"insecure-{len(insecure)}"
        home.mkdir(mode=0o700)
        cfg = configmod.Config(home)
        cfg.path.write_text(EXISTING)
        cfg.path.chmod(0o600)
        (cfg.path if target == "file" else home).chmod(mode)
        try:
            cfg.get("policy", "autonomy")
            message = None
        except configmod.InsecureConfig as e:
            message = str(e).replace(str(cfg.path), "<config>").replace(str(home), "<home>")
        insecure.append({"target": target, "mode": oct(mode), "message": message})
    return {"default_exclude_apps": configmod.DEFAULT_EXCLUDE_APPS, "upgrades": list(configmod.UPGRADES),
            "privacy_defaults": PRIVACY, "cases": cases, "refused": refused, "layout_differs": differs,
            "insecure": insecure}


# ---------- bus ----------

def frame(ev: busmod.Event) -> dict[str, Any]:
    return {"topic": ev.topic, "at": ev.at.isoformat(), "class": ev.cls.value, "data": ev.data}


async def run_bus_script(script: list[list[Any]]) -> list[Any]:
    clock = FakeClock(datetime(2026, 5, 6, 7, 8, 9, tzinfo=UTC))
    bus = busmod.Bus(clock)
    subs: dict[str, busmod.Subscription] = {}
    iters: dict[str, Any] = {}
    out: list[Any] = []
    for op in script:
        kind = op[0]
        if kind == "sub":
            subs[op[1]] = bus.subscribe(None if op[2] is None else set(op[2]), op[3])
            iters[op[1]] = subs[op[1]].__aiter__()
            out.append(None)
        elif kind == "pub":
            try:
                out.append(frame(bus.publish(op[1], op[2])))
            except ValueError as e:
                out.append({"error": str(e)})
        elif kind == "advance":
            clock.advance(op[1])
            out.append(None)
        elif kind == "close":
            subs[op[1]].close()
            out.append(None)
        elif kind == "read":                      # everything the subscriber gets without waiting
            sub, got = subs[op[1]], []
            while sub.dropped > sub._reported_dropped or sub._queue or sub.overflowed or sub.closed:
                try:
                    got.append(frame(await iters[op[1]].__anext__()))
                except busmod.Overflow as e:
                    got.append({"overflow": str(e)})
                    break
                except StopAsyncIteration:
                    got.append({"closed": True})
                    break
            out.append({"events": got, "dropped": sub.dropped, "overflowed": sub.overflowed, "closed": sub.closed,
                        "subscribers": len(bus._subs)})
        else:
            raise AssertionError(kind)
    return out


def bus_fixture() -> dict[str, Any]:
    tel, crit = "tool.called", "task.created"
    scripts: list[list[list[Any]]] = [
        [["sub", "a", None, 1000], ["pub", crit, {"task_id": "t1"}], ["advance", 1.5], ["pub", tel, None],
         ["pub", "task.progress", {"text": "héllo"}], ["read", "a"], ["read", "a"], ["close", "a"], ["read", "a"],
         ["pub", crit, {}]],
        # telemetry dropped oldest first; the count is told before the next event
        [["sub", "a", None, 3], ["pub", tel, {"n": 1}], ["pub", crit, {"n": 2}], ["pub", tel, {"n": 3}],
         ["pub", tel, {"n": 4}], ["pub", crit, {"n": 5}], ["read", "a"], ["pub", tel, {"n": 6}], ["read", "a"]],
        # a queue full of critical events: new telemetry is dropped, a new critical event overflows
        [["sub", "a", None, 2], ["sub", "b", None, 1000], ["pub", crit, {"n": 1}], ["pub", "task.started", {"n": 2}],
         ["pub", tel, {"n": 3}], ["read", "a"], ["pub", crit, {"n": 4}], ["pub", crit, {"n": 5}], ["pub", crit, {"n": 6}],
         ["pub", tel, {"n": 7}], ["read", "a"], ["read", "b"], ["read", "a"]],
        # topic filters: exact names and "prefix.*"
        [["sub", "exact", ["task.created", "tool.called"], 10], ["sub", "wild", ["task.*", "browser.dialog"], 10],
         ["sub", "none", [], 10], ["sub", "star", ["*"], 10], ["sub", "dot", [".*", "memory.import.*"], 10],
         ["pub", "task.created", {}], ["pub", "task.finished", {}], ["pub", "tool.called", {}], ["pub", "browser.dialog", {}],
         ["pub", "memory.import.progress", {}], ["pub", "tasks.nope", {}], ["pub", "bus.dropped", {}],
         ["read", "exact"], ["read", "wild"], ["read", "none"], ["read", "star"], ["read", "dot"]],
        # drops while the count is being read; closing with events still queued; maxsize 0
        [["sub", "a", None, 1], ["pub", tel, {"n": 1}], ["pub", tel, {"n": 2}], ["pub", tel, {"n": 3}], ["close", "a"],
         ["pub", tel, {"n": 4}], ["read", "a"], ["sub", "z", None, 0], ["pub", tel, {"n": 5}], ["read", "z"],
         ["pub", crit, {"n": 6}], ["read", "z"]],
    ]
    return {"critical": sorted(busmod.CRITICAL), "telemetry": sorted(busmod.TELEMETRY), "dropped": busmod.DROPPED,
            "scripts": [{"script": s, "out": asyncio.run(run_bus_script(s))} for s in scripts]}


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="ferb-core-fixtures-") as d:
        tmp = Path(d)
        os.environ["FERB_HOME"] = str(tmp / "home")          # nothing here may reach ~/.ferb
        write("types.json", types_fixture())
        write("ids.json", ids_fixture())
        write("errors.json", errors_fixture())
        write("timing.json", timing_fixture())
        write("paths.json", paths_fixture(tmp))
        write("config.json", config_fixture(tmp))
        write("bus.json", bus_fixture())


if __name__ == "__main__":
    main()
