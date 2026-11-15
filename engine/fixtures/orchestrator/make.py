"""Fixtures for orchestration's building blocks (ferb/orchestrator except supervisor.py and worker.py, ferb/decide):
the Python build run on invented data, with its returns, the rows it wrote and the text it composed.

    cd <repo> && uv run python rust/fixtures/orchestrator/make.py

Writes texts.json (pure functions: their arguments and what they returned or raised) and stores.json (scenarios run
on a migrated store in a temporary FERB home: each step's call, return and the tables at the end). The pydantic
models have their own script (make_models.py). Same bytes every run: a FakeClock at 2026-01-01T09:00:00Z,
SequentialIds, a fixed untrusted-data id and a seeded random walk; paths under the temporary folder are written
"/FX" (`fx`).
"""

import asyncio
import json
import os
import random
import shutil
import sys
import tempfile
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from types import SimpleNamespace
from typing import Any

if os.environ.get("PYTHONHASHSEED") != "0":       # set order (a forget's refs) must not change between runs
    os.environ["PYTHONHASHSEED"] = "0"
    os.execv(sys.executable, [sys.executable, *sys.argv])

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
ROOT = Path(tempfile.gettempdir()) / "ferborch"         # a fixed folder: paths are written /FX
shutil.rmtree(ROOT, ignore_errors=True)
ROOT.mkdir(parents=True)
ROOT = ROOT.resolve()
os.environ["FERB_HOME"] = str(ROOT / "home")
os.environ["HOME"] = str(ROOT / "user")
os.environ["TZ"] = "UTC"
time.tzset()
(ROOT / "user").mkdir()
sys.path.insert(0, str(REPO))

import ferb.sanitize.untrusted as untrusted  # noqa: E402
from ferb.clock.clock import FakeClock  # noqa: E402
from ferb.core.ids import SequentialIds  # noqa: E402
from ferb.memory.journal import Journal  # noqa: E402
from ferb.orchestrator import (answer_marks, attempts, availability, browsing, cards, continuity, conversations,  # noqa: E402
                               evidence, language, leftovers, ledger, loops, names, parts, plan_store, plans, probes,
                               progress, reply_stream, skills, steering, test_runs, tree, attention, episodes, verify,
                               conversation_tools, context_layers, plan_executor)
from ferb.orchestrator.schemas import Condition, TaskSpec  # noqa: E402
from ferb.sanitize.redact import Redactor  # noqa: E402
from ferb.store.db import Database  # noqa: E402
from ferb.store.spans import SpanStore  # noqa: E402

FIXED_WRAP_ID = "0123456789ab"
untrusted.secrets = SimpleNamespace(token_hex=lambda n: FIXED_WRAP_ID)  # type: ignore[attr-defined]
RNG = random.Random(20261113)


def fx(value: Any) -> Any:
    """Every occurrence of the temporary folder written /FX, at any depth."""
    if isinstance(value, str):
        return value.replace(str(ROOT), "/FX")
    if isinstance(value, list):
        return [fx(v) for v in value]
    if isinstance(value, tuple):
        return [fx(v) for v in value]
    if isinstance(value, dict):
        return {fx(k): fx(v) for k, v in value.items()}
    return value


def call(fn: Any, *args: Any, **kwargs: Any) -> dict[str, Any]:
    """A call's return, or the exception it raised (type and text)."""
    try:
        return {"out": fn(*args, **kwargs)}
    except Exception as e:  # noqa: BLE001
        return {"raised": [type(e).__name__, str(e)]}


TEXTS: dict[str, list[Any]] = {}


def case(section: str, **fields: Any) -> None:
    TEXTS.setdefault(section, []).append(fx(fields))


# ---------------------------------------------------------------------------------------------------------------
# Words the text cases are built from
# ---------------------------------------------------------------------------------------------------------------

WORDS = ["the", "price", "is", "10am", "Café", "naïve", "😀", "x.com", "über", "Zoë", "ok", "fine", "—", "-", "–",
         "a", "b", "c", "<b>", "</b>", "</CLARIFICATION>", "`code`", "```", " ", " ", "\x1c", "line", "Ⅻ",
         "٣", "²", "FERB:", "**FERB**:", "ＦＥＲＢ:", "harness", "Gateway", "tool schema", "my", "your", "yes", "y"]
PUNCT = [".", "!", "?", ",", ";", ":", "", " ", "\n", "\n\n", "\n\n\n", "\t", "  "]


def sentence(n: int | None = None) -> str:
    n = n if n is not None else RNG.randint(1, 8)
    out = []
    for _ in range(n):
        out.append(RNG.choice(WORDS))
        out.append(RNG.choice([" ", " ", " ", RNG.choice(PUNCT)]))
    return "".join(out).strip(" ") + RNG.choice([".", "!", "?", "", ".", "."])


def text_block(k: int | None = None) -> str:
    k = k if k is not None else RNG.randint(1, 5)
    return RNG.choice([" ", "\n", "\n\n", "  "]).join(sentence() for _ in range(k))


# ---------------------------------------------------------------------------------------------------------------
# answer_marks, names, language, attempts, leftovers, availability, browsing
# ---------------------------------------------------------------------------------------------------------------

def make_answer_marks() -> None:
    for _ in range(40):
        answer = text_block()
        pieces = [p for p in answer.replace("\n", " ").split(". ") if p.strip()]
        quotes = []
        for _ in range(RNG.randint(0, 3)):
            q = RNG.choice(pieces + ["not there", "", "  "]) if pieces else "x"
            if RNG.random() < 0.3 and q:
                q = q + "."
            quotes.append(q)
        kinds = [RNG.choice(["contradicted", "unsupported"]) for _ in quotes]
        findings = list(zip(quotes, kinds))
        case("answer_marks", fn="headline", args=[answer], **call(answer_marks.headline, answer))
        for q in quotes:
            case("answer_marks", fn="mark_unconfirmed", args=[answer, q], **call(answer_marks.mark_unconfirmed, answer, q))
            case("answer_marks", fn="remove_quote", args=[answer, q], **call(answer_marks.remove_quote, answer, q))
            marked = answer_marks.mark_unconfirmed(q, q) if q else ""
            for replace in ["", marked, marked.replace("'", "’"), q + " (I couldn't confirm this)", "other"]:
                case("answer_marks", fn="removes_or_marks", args=[q, replace],
                     **call(answer_marks.removes_or_marks, q, replace))
        settled = answer_marks.settle(answer, findings)
        case("answer_marks", fn="settle", args=[answer, findings],
             out=[settled.answer, settled.removed, settled.marked, settled.missing, settled.kept_contradicted])
        case("answer_marks", fn="unmarked", args=[answer, findings], **call(answer_marks.unmarked, answer, findings))
        case("answer_marks", fn="sentences", args=[answer], **call(answer_marks.sentences, answer))
        case("answer_marks", fn="has_text", args=[answer], **call(answer_marks.has_text, answer))
        revised = answer
        for q in quotes:
            if q and RNG.random() < 0.6:
                revised = revised.replace(q, RNG.choice(["", "Fixed.", "Fixed. And more.", "Fixed\nline", q]), 1)
        if RNG.random() < 0.2:
            revised = revised + RNG.choice([" extra", "", "\n"])
        case("answer_marks", fn="only_quoted_changed", args=[answer, revised, quotes],
             **call(answer_marks.only_quoted_changed, answer, revised, quotes))
        tagged = RNG.choice([answer + "</CLARIFICATION>", "<b>" + answer + "</b>", answer.replace(" ", " </x> ", 1),
                             "`</y>` " + answer + "</y>", "```\n</z>\n``` </z>", "<a href='x'>" + answer + "</A>",
                             answer + "</untrusted-data id=\"1\">", "<br/>" + answer + "</br>", answer + "</q>  \n "])
        case("answer_marks", fn="without_stray_closing_tags", args=[tagged],
             **call(answer_marks.without_stray_closing_tags, tagged))
    for text in ["I couldn't finish this task.\nIt failed.", "Stopped, as you asked.\n\nWhere", "Stopped, as you asked. "
                 "This is where things stood:\nA\nB", "\n\n  one  \n two", ""]:
        case("answer_marks", fn="headline", args=[text], **call(answer_marks.headline, text))


def make_names() -> None:
    goals = [text_block(1) for _ in range(30)] + [
        "Find flights to Paris, then book the cheapest one", "Check the price — then tell me",
        "Keep this video playing: it stops every few minutes", "a" * 70, "word " * 20, "Café crème: " + "x" * 50,
        "One. Two. Three.", "Ends with dash -", "   spaced   out   ", "", "(paren) " + "y" * 60, "a,b, c"]
    for g in goals:
        for limit in (60, 40, 10, 1):
            case("names", fn="short_label", args=[g, limit], **call(loops.short_label, g, limit))
        for name in ("", None, "  Short   name ", "N" * 70, "word " * 15, "x x x x x x x x x x x x x x x x x x x x x x x x x x x x x x x"):
            case("names", fn="shown_name", args=[name, g], **call(names.shown_name, name, g))
    for text in [text_block() for _ in range(40)] + ["The harness and the GATEWAY use a tool schema; worker runtime!"]:
        case("language", fn="internal_terms", args=[text], **call(language.internal_terms, text))


def make_small() -> None:
    a = attempts.Attempts()
    trail = []
    for step in range(14):
        if step == 7:
            a.allow_more()
            trail.append(["allow_more", a.failed, a.allowed])
        trail.append(["failed_one", a.failed_one(), a.failed, a.allowed])
    case("attempts", fn="sequence", out=trail)
    for n, goal, unattended in [(5, "find the file", False), (10, "Café", True), (1, "", False)]:
        case("attempts", fn="check_in_question", args=[n, goal], **call(attempts.check_in_question, n, goal))
        case("attempts", fn="stopped_after", args=[n, unattended], **call(attempts.stopped_after, n, unattended=unattended))
    tabs = [{"browser": "Chrome", "profile": "Work", "account": "me@example.com", "title": "Inbox", "borrowed": True},
            {"browser": "", "title": "A page"}, {"profile": "P", "account": ""}, {"browser": "Arc", "borrowed": False},
            "not a dict"]
    resource_sets = [
        [], [{"resource": "browser", "error": "e1", "tabs": tabs}], [{"resource": "app", "error": "x", "left_running": True}],
        [{"resource": "shell", "error": "boom"}, {"resource": "listen", "error": "boom"}],
        [{"resource": "ferb_browser", "error": "e"}, {"resource": "recording", "error": "e2"}, {"resource": "odd", "error": 3}],
        [{"resource": "app", "error": "x"}, {"error": ""}, "x", {"resource": "browser", "error": "e", "tabs": []}],
    ]
    for resources in resource_sets:
        for errors in ([], ["e1"], ["other"], ["boom", "e"]):
            case("leftovers", fn="leftover_lines", args=[resources, errors],
                 **call(leftovers.leftover_lines, resources, errors))
    regs = [set(), {"fs:read"}, {"fs:read", "fs:write", "browser:read", "time", "shell", "applescript", "mac:read"},
            {"identity:browse", "mailbox:send", "screen", "audio", "trail:read", "mac:act"}]
    probs = [{}, {"ferb_browser": "not connected — Settings → Browsers"}, {"mac": "no Accessibility", "hearing": "x {scopes}"}]
    for reg in regs:
        for prob in probs:
            for profiles in ("", "Chrome “Work” (me@example.com)", "{scopes}"):
                av = availability.build(reg, prob, profiles)
                case("availability", fn="build", args=[sorted(reg), list(prob.items()), profiles],
                     out=[sorted(av.scopes), av.text, av.missing, av.unavailable_line()])
    for label in ("Chrome “Work”", "{x}", ""):
        for name in ("GONE", "CHOSEN", "GONE_RUN", "CHOSEN_FACT"):
            case("browsing", fn="with_label", args=[name, label], out=getattr(browsing, name).format(label=label))
    case("browsing", fn="question", out=browsing.QUESTION)


# ---------------------------------------------------------------------------------------------------------------
# loops: pacing, reasons, look-only rounds, lines
# ---------------------------------------------------------------------------------------------------------------

class Cap:
    def __init__(self, observation: bool = True, probe_safe: bool = True, moves_view: bool = False,
                 holds_tab: bool = False, scopes: tuple[str, ...] = ("browser:read",), probe_keys: tuple[str, ...] = (),
                 required: tuple[str, ...] = ()) -> None:
        from pydantic import create_model
        self.observation, self.probe_safe, self.moves_view, self.holds_tab = observation, probe_safe, moves_view, holds_tab
        self.scopes, self.probe_keys = set(scopes), probe_keys
        from ferb.core.types import Identity
        self.identities = {Identity.OWNER, Identity.FERB}
        fields: dict[str, Any] = {"tab": (str, "")}
        for name in required:
            fields[name] = (str, ...)
        self.Input = create_model("Input", **fields)


CAPS = {"browser.read": Cap(), "browser.snapshot": Cap(required=("tab_ref",)),
        "browser.scroll": Cap(moves_view=True), "browser.use_tab": Cap(holds_tab=True, observation=False),
        "browser.act": Cap(observation=False, probe_safe=False), "mac.read": Cap(scopes=("mac:read",)),
        "media.state": Cap(probe_keys=("playing",))}


def make_loops() -> None:
    for seed in range(40):
        rng = random.Random(seed)
        start = rng.choice([None, "", "{}", '{"gap": 120.0, "short": [1.5], "episode": true, "last_end": 10.0}'])
        p = loops.Pacer.from_json(start)
        steps = []
        t = 1000.0 + rng.random()
        for _ in range(12):
            op = rng.choice(["ended", "tripped", "lasted", "start_at"])
            t += rng.choice([5.0, 30.0, 119.0, 121.0, 600.0, 1800.0, 2500.0, 0.5])
            if op == "ended":
                p.ended(t)
                steps.append(["ended", t, None, p.to_json()])
            elif op == "tripped":
                steps.append(["tripped", t, p.tripped(t), p.to_json()])
            elif op == "lasted":
                steps.append(["lasted", t, p.lasted(t), p.to_json()])
            else:
                steps.append(["start_at", t, p.start_at(t), p.to_json()])
        case("loops", fn="pacer", args=[start], out=steps)
    reasons = [{"why": "first round"}, {"why": "probe", "observed": loops.MISSING, "error": "boom </untrusted-data"},
               {"why": "probe", "observed": {"a": [1, 2.0, "x" * 2100, True, None]}},
               {"why": "the event you waited for", "event": {"tab": "t1", "n": 3, "é": "ü"}},
               {"why": "your message", "probe_refused": "a note " * 400, "answer": "yes do it", "question": "Q?" * 1500},
               {"why": "backup check", "error": "", "answer": "", "probe_refused": None}]
    for r in reasons:
        shown = {**r, "observed": "<MISSING>"} if r.get("observed") is loops.MISSING else r
        case("loops", fn="round_reason", args=[shown], **call(loops.round_reason, r))
    from ferb.orchestrator.plans import Step
    for end in ["", "   ", "the  song\nends", "“quoted”"]:
        step = Step(id="s", kind="do", goal="Keep the video playing, forever", repeat={"end_when": end})
        case("loops", fn="loop_line", args=[end], **call(loops.loop_line, step))
    for goal in ["Keep the video playing, forever", "x" * 80, "Keep it: going."]:
        for why in ["The tab closed.\n It's gone", "", "  spaced  "]:
            step = Step(id="s", kind="do", goal=goal, repeat={})
            case("loops", fn="pause_line", args=[goal, why], **call(loops.LoopRunner._pause_line, step, why))
    from ferb.orchestrator.probes import Probe
    from ferb.policy.rules import Kind
    calls_sets = [
        [{"tool": "browser.read", "kind": Kind.READ, "args": {"tab": "t1"}, "output": {"tab": "t1", "text": "x"}}],
        [{"tool": "browser.use_tab", "kind": Kind.OPEN_TAB, "args": {"tab_ref": "ref-9"}, "output": {"tab": "t2"}}],
        [{"tool": "browser.use_tab", "kind": Kind.OPEN_TAB, "args": {}, "output": {"released": True}}],
        [{"tool": "browser.scroll", "kind": Kind.READ, "args": {"tab": "t1"}, "output": {}}],
        [{"tool": "browser.read", "kind": Kind.READ, "args": {"tab": "t1"}, "output": {"media": [{"id": "m7", "playing": True}]}},
         {"tool": "browser.read", "kind": Kind.READ, "args": {"tab": "t1"}, "declared": "send", "output": {}}],
        [{"tool": "browser.read", "kind": Kind.READ, "args": {"tab": "t3"}, "output": {"media": [{"id": "m7"}], "page": "t9"}}],
        [{"tool": "nope", "kind": Kind.READ, "args": {}}], [],
    ]
    probes_ = [Probe(tool="browser.read", args_json='{"tab": "t1"}', field="media.0.playing", op="equals", value_json="true"),
               Probe(tool="browser.read", args_json='{"tab": "t9"}', field="text", op="changed"),
               Probe(tool="media.state", args_json='{"tab": "t1"}', field="m7.playing", op="equals"),
               Probe(tool="browser.read", args_json='{}', field="t2.x", op="equals"),
               Probe(tool="browser.snapshot", args_json='{"tab": "t1"}', field="a", op="equals"),
               Probe(tool="nope", args_json='{}', field="a", op="equals"),
               Probe(tool="browser.read", args_json='[1]', field="a", op="equals")]
    for calls in calls_sets:
        for probe in probes_[:4] + [probes_[6], None]:
            for held in ([["t2", "ref-9"]] if probe is None else ([], ["t2", "ref-9"])):
                case("loops", fn="look_only_round",
                     args=[calls, probe.model_dump(exclude_defaults=True) if probe else None, held],
                     **call(loops.look_only_round, calls, probe, CAPS, held))


# ---------------------------------------------------------------------------------------------------------------
# probes, progress
# ---------------------------------------------------------------------------------------------------------------

def make_probes() -> None:
    from ferb.orchestrator.probes import Probe
    registered = ["browser.read", "ferb_browser.read", "mac.read", "media.state", "a_b.c", "a.b_c"]
    for written in ["browser.read", "browser_read", "mcp__ferb__browser_read", "mcp__ferb.browser.read", "ferb__mac_read",
                    "mcp__ferb__nothing", "a_b_c", "media_state", "", "ferb__ferb__mac_read"]:
        case("probes", fn="resolve_tool", args=[written, registered], **call(probes.resolve_tool, written, registered))
    probe_list = [Probe(tool=t, args_json=a, field=f, op=o, value_json=v)
                  for t in ("browser.read", "browser.act", "mac.read", "nope", "browser.snapshot")
                  for a, f, o, v in [('{"tab": "t1"}', "media.0.x", "equals", "true"), ("", " ", "changed", ""),
                                     ("{bad", "a", "equals", "1"), ("[1]", "a", "equals", "1"),
                                     ('{"tab": 5}', "a", "not_equals", "{nope"), ('{"tab": "t"}', "a", "changed", "{nope")]]
    for p in probe_list:
        for scopes in (["browser:read"], ["browser:read", "mac:read"]):
            case("probes", fn="refusal", args=[p.model_dump(), scopes], **call(probes.refusal, p, CAPS.get(p.tool), scopes,
                                                                                   __import__("ferb.core.types").core.types.Identity.OWNER))
        case("probes", fn="args", args=[p.args_json], **call(lambda p=p: p.args()))
        case("probes", fn="value", args=[p.value_json], **call(lambda p=p: p.value()))
    content = {"media": [{"id": "m1", "playing": True, "t": 1.0}, {"id": "m2"}], "a": {"b": None, "0": "zero"}, "n": 1,
               "f": 1.0, "s": "1", "deep": [[[["x"]]]]}
    for path in ["media.0.playing", "media.1.playing", "media.2", "a.b", "a.0", "n", "f", "s", "deep.0.0.0.0", "media.²",
                 "media.01", "", "media.-1", "missing.x", "media.0.t"]:
        got = call(probes.field_value, content, path)
        if "out" in got and got["out"] is probes.MISSING:
            got = {"out": "<MISSING>"}
        case("probes", fn="field_value", args=[content, path], **got)
    values = [1, 1.0, True, "1", None, [1], {"a": 1}, {"a": 1.0}, "true", False, 0, "<MISSING>"]
    for a in values:
        for b in values:
            av = probes.MISSING if a == "<MISSING>" else a
            bv = probes.MISSING if b == "<MISSING>" else b
            case("probes", fn="same", args=[a, b], out=probes.same(av, bv))
            for op, vj in [("equals", "1"), ("changed", "null")]:
                p = Probe(tool="x", field="f", op=op, value_json=vj)
                case("probes", fn="tripped", args=[p.model_dump(), a, b], **call(probes.tripped, p, av, bv))


def make_progress() -> None:
    tools = ["browser.open", "ferb_browser.read", "browser.act", "mac.snapshot", "mac.act", "fs.read", "fs.trash",
             "shell.run", "wait.for", "owner.ask", "x.unknown", "browser.screenshot", "mac.open", "fs.list", "plain"]
    kinds = ["", "read", "send", "write_file", "trash", "pay", "owner_question", "nonsense", "mac_input", "click"]
    argsets = [{}, {"url": "https://www.Example.com/a?b=1"}, {"path": "/tmp/notes/report.txt"}, {"path": "/a/b/"},
               {"path": "/"}, {"path": "   "}, {"path": "a/./b/."}, {"app": "WhatsApp"}, {"bundle_id": "com.x.y"},
               {"url": "not a url"}, {"path": "evil\x1b[2Jname\nline.txt"}, {"url": "https://xn--bcher-kva.example/"},
               {"path": 5}, {"app": 0}]
    for tool in tools:
        for kind in kinds:
            for args in argsets:
                for site, app in [(None, None), ("docs.example.org", None), (None, "Notes"), ("", "")]:
                    if RNG.random() < 0.05:
                        case("progress", fn="describe", args=[tool, args, kind, site, app],
                             **call(progress.describe, tool, args, kind=kind, site=site, app=app))
    commands = ["", "  \n  ", "codex exec -s danger-full-access --model gpt-6 'do the thing please now'",
                "echo `hi` && sleep 1", "\x1b[31mred\x1b[0m\nsecond line", "a" * 60, "word " * 20, "x"]
    for tool in ("shell.wait", "shell.read"):
        for command in commands:
            for minutes in (0, 1, 14):
                case("progress", fn="waiting_for", args=[tool, command, minutes],
                     **call(progress.waiting_for, tool, command, minutes))
    installed = [("com.apple.Notes", "Notes"), ("net.whatsapp.WhatsApp", "WhatsApp"), ("com.x.dup", "Notes"),
                 ("", "Empty"), ("com.no.name", "")]
    running = [("com.apple.Safari", "Safari", 42), ("com.run.noname", "", 7)]
    for args in [{}, {"app": "notes"}, {"bundle_id": "COM.APPLE.NOTES"}, {"app": "pid:42"}, {"app": "Unknown"},
                 {"app": 5}, {"app": "  "}, {"bundle_id": "com.apple.safari"}, {"app": "pid:7"}]:
        for site in (None, "net.whatsapp.WhatsApp", ""):
            case("progress", fn="app_name", args=[args, site, installed, running],
                 **call(progress.app_name, args, site, installed, lambda: running))


# ---------------------------------------------------------------------------------------------------------------
# plans: checks, parts, inventory
# ---------------------------------------------------------------------------------------------------------------

def make_plans() -> None:
    from ferb.orchestrator.plans import Part, Plan, Step
    base = [{"id": "a", "kind": "do", "goal": "Find it"}, {"id": "b", "kind": "do", "goal": "Use it", "depends_on": ["a"]}]
    variants = [
        base, [], [{"id": "a", "kind": "do"}, {"id": "a", "kind": "do"}], [{"id": "x:y", "kind": "do"}, {"id": "", "kind": "do"}],
        [{"id": "a", "kind": "do", "depends_on": ["z", "y"]}],
        [{"id": "w", "kind": "wait"}, {"id": "w2", "kind": "wait", "until": "2026-01-02T09:00:00", "event": "e"},
         {"id": "w3", "kind": "wait", "until": "2026-01-02T09:00:00Z"}, {"id": "w4", "kind": "wait", "until": "tomorrow"}],
        [{"id": "c", "kind": "converse"}, {"id": "q", "kind": "ask", "question": "  "}],
        [{"id": "r", "kind": "wait", "repeat": {"end_when": "x"}, "until": "2026-01-02T09:00:00+00:00"},
         {"id": "l", "kind": "do", "repeat": {}, "until": "2026-01-02T09:00:00+00:00"},
         {"id": "d", "kind": "do", "depends_on": ["l"]}, {"id": "i", "kind": "ask", "question": "Q", "inputs": ["/x"]}],
        [{"id": "p", "kind": "do", "depends_on": ["q"]}, {"id": "q", "kind": "do", "depends_on": ["p"]},
         {"id": "r", "kind": "do", "depends_on": ["r"]}, {"id": "s", "kind": "do"}],
        [{"id": "in1", "kind": "do", "inputs": ["/FX/a.txt", "/FX/b.txt", "/FX/a.txt"]},
         {"id": "in2", "kind": "do", "inputs": ["/FX/b.txt", "/FX/zzz"]}],
    ]
    facts = [{"path": "/FX/a.txt", "bytes": 10, "text": True, "tokens_upper_bound": 900},
             {"path": "/FX/b.txt", "bytes": 5, "text": True, "tokens_upper_bound": 200},
             {"path": "/FX/c.bin", "bytes": 5, "text": False, "tokens_upper_bound": 0}]
    for steps in variants:
        for big, time_e, cost in [(False, "", ""), (True, " ", "x"), (True, "1h", "2c")]:
            raw = {"steps": steps, "big_job": big, "time_estimate": time_e, "cost_estimate": cost}
            plan = Plan.model_validate(raw)
            for f in ([], facts):
                case("plans", fn="problems", args=[raw, f, 1000], **call(plans.problems, plan, f, context_tokens=1000))
            case("plans", fn="answer_steps", args=[raw, []], **call(plans.answer_steps, plan, []))
    part_sets = [
        [], [{"id": "p1", "quote": "q", "label": "One", "direct": "4"}],
        [{"id": "p1", "quote": "q", "label": "One", "direct": "4"}, {"id": "p2", "quote": "r", "label": "", "answer_step": "b",
                                                                       "conditions": [0, 1]}],
        [{"id": "p1", "quote": "q", "label": "One", "answer_step": "a", "conditions": [0]},
         {"id": "p1", "quote": "r", "label": "Two", "answer_step": "a", "conditions": [0, 5, -1]}],
        [{"id": "p:1", "quote": "q", "label": "L", "direct": "x", "answer_step": "a", "conditions": [2]},
         {"id": " ", "quote": "q", "label": "L"}],
        [{"id": "p1", "quote": "q", "label": "L", "answer_step": "worker", "conditions": [0]},
         {"id": "p2", "quote": "q", "label": "L", "answer_step": "zz", "conditions": [1]}],
        [{"id": "p1", "quote": "q", "label": "L", "answer_step": "w", "conditions": []},
         {"id": "p2", "quote": "q", "label": "L", "answer_step": "b", "conditions": [1000000000000]}],
    ]
    raw_plans = [None, {"steps": [{"id": "a", "kind": "do"}, {"id": "b", "kind": "do", "depends_on": ["a"]},
                                  {"id": "w", "kind": "wait"}, {"id": "idle", "kind": "do"}]},
                 {"steps": [{"id": "a", "kind": "do"}, {"id": "b", "kind": "do"}]}]
    for ps in part_sets:
        parts_ = [Part(**p) for p in ps]
        for raw in raw_plans:
            plan = Plan.model_validate(raw) if raw else None
            for conditions in (0, 2, 3):
                case("plans", fn="part_problems", args=[ps, raw, conditions],
                     **call(plans.part_problems, parts_, plan, conditions))
            case("plans", fn="answer_steps", args=[raw, ps], **call(plans.answer_steps, plan, parts_))
            case("plans", fn="card", args=[ps], out=plans.card(parts_))
    for label, goal in [("", "Find the file, then tell me"), ("Short", "x"), ("", ""), ("  ", "  ")]:
        step = Step(id="s1", kind="do", label=label, goal=goal)
        case("plans", fn="step_label", args=[label, goal], out=plans.step_label(step))
    for checklist in [{}, {"steps": [{"id": "a"}, {"id": "b"}, {}], "states": {"a": {"status": "done"}, "b": None}},
                      {"steps": None, "states": None}, {"steps": [{"id": "a"}], "states": {"a": {"status": "failed"}}}]:
        case("plans", fn="steps_done", args=[checklist], out=plans.steps_done(checklist))
    home = str(Path.home())
    for paths in [[home + "/Desktop/notes"], [home, "/tmp/x", home + "x"], [home + "/a", home + "/b", "/c"]]:
        case("plans", fn="looking_at", args=[paths], out=plans.looking_at(paths))
    # the inventory, on invented trees
    tree = ROOT / "tree"
    (tree / "notes" / "deep" / "deeper").mkdir(parents=True)
    (tree / "notes" / "empty").mkdir()
    (tree / "media").mkdir()
    (tree / "notes" / "a.txt").write_text("hello " * 50)
    (tree / "notes" / "b.md").write_text("é" * 100)
    (tree / "notes" / "deep" / "c.txt").write_text("x" * 300)
    (tree / "notes" / "deep" / "deeper" / "d.txt").write_text("y" * 50)
    (tree / "notes" / "cut.txt").write_bytes(b"a" * 8191 + "é".encode())
    (tree / "notes" / "bad.txt").write_bytes(b"ok\xff\xfe")
    (tree / "media" / "v.bin").write_bytes(b"\x00" * 64)
    (tree / "media" / "z.txt").write_text("z" * 2000)
    (tree / "link.txt").symlink_to(tree / "notes" / "a.txt")
    (tree / "notes" / "linkdir").symlink_to(tree / "media")
    (tree / "single.txt").write_text("single")
    roots = [[str(tree / "notes")], [str(tree)], [str(tree / "single.txt"), str(tree / "notes")], ["~/nothing"],
             [str(tree / "link.txt")], [str(tree / "media"), str(tree / "notes")]]
    for paths in roots:
        for inputs in ([], [str(tree / "notes" / "deep")], [str(tree / "notes"), "~/x"]):
            for steps, ctx in [(1, None), (2, 400), (1, 1000)]:
                measured = asyncio.run(plans.inventory(paths, lambda p: p.name == "b.md", inputs=inputs, steps=steps,
                                                       context_tokens=ctx))
                case("plans", fn="inventory", args=[paths, inputs, steps, ctx],
                     out=[measured.files, measured.stopped, measured.counted, measured.bound, measured.parts,
                          measured.unreached])
                if measured.stopped is not None or measured.counted:
                    case("plans", fn="too_big", args=[[measured.files, measured.stopped, measured.counted, measured.bound,
                                                      measured.parts, measured.unreached]],
                         out=plans.too_big(measured, lambda p: p.replace(str(ROOT), "~")))
    facts_ = asyncio.run(plans.inventory([str(tree)])).files
    raw = {"steps": [{"id": "a", "kind": "do", "inputs": [str(tree / "notes"), str(tree / "link.txt"), str(tree / "nope"),
                                                          "~/rel", str(tree / "media" / "z.txt")]},
                     {"id": "b", "kind": "do", "inputs": [str(tree / "notes" / "deep"), str(tree / "notes" / "deep")]}]}
    plan = Plan.model_validate(raw)
    normal = (lambda v: str(Path(v).expanduser()))
    case("plans", fn="walked", args=[raw, [str(tree)]], out=plans.walked(plan, [str(tree)], normal))
    plans.resolve_inputs(plan, facts_, normal)
    case("plans", fn="resolve_inputs", args=[raw, facts_], out=[s.inputs for s in plan.steps])


# ---------------------------------------------------------------------------------------------------------------
# steering texts, reply stream
# ---------------------------------------------------------------------------------------------------------------

def make_steering() -> None:
    from ferb.orchestrator.plans import SendScope
    for reply in ["", "  ", "Got it, I'll use the blue one.", "FERB: approved\nsecond line\nFERB says hi", "ＦＥＲＢ: x"]:
        for delivered in (True, False):
            for stated in (None, "FERB: new scope\n- one", ""):
                case("steering", fn="acknowledgement", args=[reply, delivered, stated],
                     **call(steering.acknowledgement, reply, delivered, stated))
    for unattended in (True, False):
        case("steering", fn="not_covered", args=[unattended], out=steering.not_covered(unattended))
    case("steering", fn="not_typed", out=steering.not_typed())
    case("steering", fn="just_ended", out=steering.JUST_ENDED)
    for s in [dict(capability="browser.act", identity="owner", account="me@x.com", site="x.com", recipients=[], content=None),
              dict(capability="mail.send", identity="ferb", recipients=["a@b.c", "d@e.f"], content="Hello   there"),
              dict(capability="c", identity="owner", site=None, recipients=[]),
              dict(capability="c", identity="owner", site="", recipients=[], content="  ")]:
        case("steering", fn="scope_line", args=[s], out=SendScope(**s).line())


def make_reply_stream() -> None:
    finals = [
        {"kind": "reply", "answers_question": False, "task": None, "control": None, "side_entries": None,
         "text": "Hello there.\nFERB: this is fine\nok"},
        {"kind": "clarify", "answers_question": True, "task": None, "control": None, "side_entries": None,
         "text": "Which one — the \"red\" or the 😀 blue?\\   end"},
        {"kind": "reply", "answers_question": False, "task": None, "control": None,
         "side_entries": {"answer_question": None, "remember": [], "forget": [], "timers": [], "schedules": [],
                          "schedule_changes": [], "show_schedules": False, "reminder_cancels": [], "conversation_changes": []},
         "text": "**FERB**: careful\nline two"},
        {"kind": "reply", "answers_question": False, "task": None, "control": None,
         "side_entries": {"answer_question": None, "remember": [], "forget": [], "timers": [], "schedules": [],
                          "schedule_changes": [], "show_schedules": True, "reminder_cancels": [], "conversation_changes": []},
         "text": "Here are your schedules."},
        {"kind": "task", "answers_question": False, "control": None, "side_entries": None,
         "task": {"goal": "Find the file", "name": "Find file", "identity": "owner", "worker_type": "default",
                  "targets": {"apps": [], "sites": [], "paths": [], "recipients": []},
                  "success_conditions": [{"text": "found", "owner_text": "", "layer": "outcome", "check": None}],
                  "plan": None, "folders": [], "keep_tabs_open": False, "parts": [], "earlier_results": []},
         "text": "I'll find it.\nFERB will check."},
        {"kind": "task", "answers_question": False, "control": None, "side_entries": None,
         "task": {"goal": "x", "identity": "owner", "success_conditions": []}, "text": "bad task"},
        {"kind": "reply", "answers_question": False, "task": None, "control": None, "side_entries": None,
         "text": "my password is hunter2 and card 4111 1111 1111 1111"},
    ]
    for final in finals:
        raw = json.dumps(final, ensure_ascii=RNG.random() < 0.5)
        for cut in (1, 3, 7, 50):
            events: list[Any] = []
            draft = reply_stream.ReplyDraft(Redactor(), events.append)
            pieces = [raw[i:i + cut] for i in range(0, len(raw), cut)]
            for p in pieces:
                draft.feed(p)
            draft.finish(raw if cut != 50 else json.dumps(final))
            case("reply_stream", fn="draft", args=[pieces, raw if cut != 50 else json.dumps(final)], out=events)
    # broken drafts: text not last, unknown key, a bad escape, a restart after a lookup
    for raw, finish in [('{"kind": "reply", "text": "hi"', None), ('{"text": "x"}', '{}'),
                        ('{"kind": "reply", "answers_question": false, "task": null, "control": null, "side_entries": null,'
                         ' "text": "a\\qb"}', None),
                        ('{"kind": "reply", "answers_question": false, "task": null, "control": null, "side_entries": null,'
                         ' "text": "hi"} x', None), ('[1]', None), ('{"bogus": 1}', None),
                        ('{"kind": "reply", "answers_question": false, "task": null, "control": null, "side_entries": null,'
                         ' "text": "\\ud83d\\ude00 and \\ud83d"}', None)]:
        events = []
        draft = reply_stream.ReplyDraft(Redactor(), events.append)
        for i in range(0, len(raw), 4):
            draft.feed(raw[i:i + 4])
        if finish is not None:
            draft.finish(finish)
        case("reply_stream", fn="draft", args=[[raw[i:i + 4] for i in range(0, len(raw), 4)], finish], out=events)
    events = []
    draft = reply_stream.ReplyDraft(Redactor(), events.append)
    draft.feed('{"kind": "reply", "answers')
    draft.restart()
    raw = json.dumps(finals[0])
    draft.feed(raw)
    draft.finish(raw)
    draft.restart()
    case("reply_stream", fn="restart", args=[raw], out=events)


def main() -> None:
    make_answer_marks()
    make_names()
    make_small()
    make_loops()
    make_probes()
    make_progress()
    make_plans()
    make_steering()
    make_reply_stream()
    from stores import make_stores  # noqa: E402  (the store scenarios, beside this file)
    stores = make_stores(ROOT, fx, call)
    (HERE / "texts.json").write_text(json.dumps(TEXTS, ensure_ascii=False, sort_keys=False) + "\n")
    (HERE / "stores.json").write_text(json.dumps(stores, ensure_ascii=False) + "\n")
    shutil.rmtree(ROOT, ignore_errors=True)


if __name__ == "__main__":
    sys.path.insert(0, str(HERE))
    main()
