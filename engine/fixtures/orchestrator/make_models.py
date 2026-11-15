"""Fixtures for the answer models' validation (ferb/orchestrator/schemas.py, plans.py, conversations.py, probes.py,
ferb/clock/when.py): what pydantic makes of a model's answer, valid or not, and the exact text of its errors, which
FERB sends back to the model (supervisor.py:1515, 2444) or reads (worker.py:223).

    cd <repo> && uv run python rust/fixtures/orchestrator/make_models.py

Writes models.json: {"pins", "seeds", "cases"}. Each case: `model`, `mode` ("python": `model_validate(value)`;
"json": `model_validate_json(json.dumps(value, ensure_ascii=ascii))`), the input as either `input` (the exact text)
or `seed` (an index into `seeds`) with `ops` (mutations, `apply` below) and `ascii`, and one of
- `ok`: {"dump": json.dumps(model_dump(), ensure_ascii=False) (over 1,000 characters: "dump_sha"/"dump_len"),
  "json_sha"/"json_len": the SHA-256 (first 24 hex)
  and length of model_dump_json()} (TaskSpec also "json_noplan": model_dump_json(exclude={"plan"}));
- `error`: str(e) of the pydantic ValidationError (its items' type, loc and msg are each in it), packed: each
  item's link line ("For further information visit https://errors.pydantic.dev/2.13/v/<type>") left out (`unpack`);
- `raised`: [exception type, str(e)] (a validator raised something else, which escapes pydantic).

Cases are valid answers (hand-written here and the scripted finals of rust/fixtures/daemon/chat_scenarios.json),
each mutated by a seeded random walk: fields dropped, nulled, given another type, numbers as text, big numbers,
long and non-ASCII text, extra and legacy keys, invalid JSON. Same bytes every run (fixed seed, no clock).
"""

import hashlib
import os
import json
import re
import random
import sys
from pathlib import Path

import pydantic
import pydantic_core
from pydantic import ValidationError

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))

from ferb.clock.when import When  # noqa: E402
from ferb.orchestrator.conversations import ConversationScope  # noqa: E402
from ferb.orchestrator.plans import Plan  # noqa: E402
from ferb.orchestrator.probes import Probe  # noqa: E402
from ferb.orchestrator.schemas import (  # noqa: E402
    CheckerOutcome, FixOutcome, QuestionOutcome, RecheckOutcome, SummaryOutcome, SupervisorOutcome, TaskSpec,
    WorkerReport)

# A wider run for checking (not the tracked file): FERB_MODELS_SEED, FERB_MODELS_SCALE, FERB_MODELS_OUT
OUT = Path(os.environ.get("FERB_MODELS_OUT") or Path(__file__).with_name("models.json"))
SCALE = int(os.environ.get("FERB_MODELS_SCALE") or 1)
MODELS = {"SupervisorOutcome": SupervisorOutcome, "TaskSpec": TaskSpec, "WorkerReport": WorkerReport,
          "CheckerOutcome": CheckerOutcome, "Plan": Plan, "Probe": Probe, "ConversationScope": ConversationScope,
          "QuestionOutcome": QuestionOutcome, "FixOutcome": FixOutcome, "RecheckOutcome": RecheckOutcome,
          "SummaryOutcome": SummaryOutcome, "When": When}

# ---------------------------------------------------------------------------------------------------------------
# Valid answers
# ---------------------------------------------------------------------------------------------------------------

TARGETS = {"apps": [], "sites": ["example.com"], "paths": ["~/Desktop/notes.txt"], "recipients": ["sam@example.com"]}
NOCHECK = None
SCOPE = {"capability": "browser.act", "identity": "owner", "account": "me@example.com", "site": "x.com",
         "recipients": [], "content": None}
CONV = {"identity": "owner", "account": "me@example.com", "site": "web.whatsapp.com", "app": "WhatsApp",
        "thread": "Book club", "recipients": ["Ana"], "topic": "the next meeting date",
        "handover_when": "a date is agreed", "window_id": None}
STEP = {"id": "find", "kind": "do", "label": "Find prices", "goal": "Find the prices of the three plans",
        "depends_on": [], "inputs": [], "worker_type": "default", "conditions": ["three prices found"],
        "until": None, "event": None, "event_match": [], "question": "", "options": [], "conversation": None,
        "repeat": None, "earlier_results": []}
PLAN = {"steps": [
    STEP,
    {**STEP, "id": "wait", "kind": "wait", "label": "", "goal": "Wait until morning", "depends_on": ["find"],
     "conditions": [], "until": "2026-01-02T09:00:00+00:00"},
    {**STEP, "id": "ask", "kind": "ask", "goal": "", "depends_on": ["wait"], "question": "Which plan?",
     "options": ["Basic", "Pro"]},
    {**STEP, "id": "chat", "kind": "converse", "goal": "Agree a date", "depends_on": ["ask"], "conversation": CONV},
    {**STEP, "id": "watch", "kind": "do", "goal": "Keep the music playing", "depends_on": [],
     "repeat": {"end_when": "", "event": "song.ended", "event_match": [{"field": "tab", "value": "t1"}]}},
], "input_paths": [], "big_job": True, "time_estimate": "about 10 minutes", "cost_estimate": "a few cents",
    "standing_sends": [SCOPE]}
CONDITIONS = [
    {"text": "The note says hello", "owner_text": "says hello", "layer": "outcome", "check": NOCHECK},
    {"text": "notes.txt exists", "owner_text": "", "layer": "artifact",
     "check": {"kind": "file_exists", "path": "~/Desktop/notes.txt", "text": None, "url_contains": None,
               "site": None, "expected": None, "tool": None}},
    {"text": "Sam got it", "owner_text": "", "layer": "external",
     "check": {"kind": "sent", "path": None, "text": None, "url_contains": None, "site": "mail.example.com",
               "expected": "sam@example.com", "tool": None}},
]
TASK = {"goal": "Write hello in my notes and send them to Sam", "name": "Hello note", "identity": "owner",
        "worker_type": "default", "targets": TARGETS, "success_conditions": CONDITIONS, "plan": None,
        "folders": ["/Users/someone/Desktop"], "keep_tabs_open": False,
        "parts": [{"id": "p1", "quote": "write hello", "label": "Hello", "direct": None, "answer_step": "worker",
                   "conditions": [0, 1]},
                  {"id": "p2", "quote": "send them", "label": "Send", "direct": None, "answer_step": "worker",
                   "conditions": [2]}],
        "earlier_results": []}
TASK_PLAN = {**TASK, "parts": [], "plan": PLAN, "name": "Plan prices"}
WHEN = {"in_minutes": None, "in_days": None, "day": "tomorrow", "date": None, "time": "09:30"}
UNTIL = {"quote": "until Friday", "day": "friday", "date": None, "in_days": None}
S_UNTIL = {"quote": "for 2 hours", "day": "", "date": "", "in_days": 0, "time": "", "in_minutes": 120}
SCHEDULE = {"request": "Check the weather and tell me", "name": "Weather check", "when": WHEN, "repeat": "daily",
            "weekday": None, "every_minutes": None, "times": ["18:00"],
            "random": [{"count": 2, "start": "10:00", "end": "12:00"}], "quote": "every day at 9:30",
            "targets": {"apps": [], "sites": ["weather.example"], "paths": [], "recipients": []},
            "standing_sends": [SCOPE], "watch": "none", "until": S_UNTIL, "when_active": False, "folders": [],
            "identity": "ferb", "worker_type": "default", "profile": "", "account": ""}
SIDE = {
    "answer_question": {"approval_id": "ap-1", "answer": "Yes"},
    "remember": [{"kind": "preference", "subject": "coffee", "key": "milk", "content": "oat milk",
                  "quote": "I take oat milk", "login_origin": None, "until": UNTIL, "replaces": ["m-1"],
                  "review_in_days": 30}],
    "forget": [{"item_id": "m-2", "quote": "forget my old address"}],
    "timers": [{"when": {**WHEN, "day": None, "time": None, "in_minutes": 5}, "message": "Tea", "quote": "5 minutes",
                "when_active": False, "name": "Tea timer", "kind": "timer"}],
    "schedules": [SCHEDULE],
    "schedule_changes": [{"id": "s-1", "action": "edit", "quote": "make it 7am", "request": "",
                          "timing": {"repeat": "daily", "times": ["07:00"], "every_minutes": 0, "weekday": "",
                                     "random": []},
                          "until": None, "name": ""}],
    "show_schedules": True,
    "reminder_cancels": [{"id": "r-1", "quote": "cancel the tea one"}],
    "conversation_changes": [{"id": "c-1", "action": "pause", "quote": "stop chatting"}],
}
EMPTY_SIDE = {"answer_question": None, "remember": [], "forget": [], "timers": [], "schedules": [],
              "schedule_changes": [], "show_schedules": False, "reminder_cancels": [], "conversation_changes": []}
SUPERVISOR = [
    {"kind": "reply", "answers_question": False, "task": None, "control": None, "side_entries": None,
     "text": "Hello! How can I help?"},
    {"kind": "clarify", "answers_question": False, "task": None, "control": None, "side_entries": None,
     "text": "Which folder?"},
    {"kind": "task", "answers_question": False, "task": TASK, "control": None, "side_entries": None,
     "text": "I'll write it and send it."},
    {"kind": "task", "answers_question": True, "task": TASK_PLAN, "control": None, "side_entries": SIDE,
     "text": "Here's the plan."},
    {"kind": "task", "answers_question": False, "task": TASK,
     "control": {"task_id": "t-1", "action": "continue", "new_goal": True, "standing_sends": [SCOPE],
                 "supersedes": ["ap-2"]}, "side_entries": None, "text": "Updated."},
    {"kind": "reply", "answers_question": False, "task": None,
     "control": {"task_id": "t-1", "action": "pause", "new_goal": False, "standing_sends": "unchanged",
                 "supersedes": []}, "side_entries": {**EMPTY_SIDE, "timers": SIDE["timers"]}, "text": "Paused."},
    # before ORCH-630 (side entries at the top level) and the single `schedule` before T-1066 (STORE-403)
    {"kind": "task", "text": "Done.", "remember": SIDE["remember"], "schedule": SCHEDULE, "show": ["schedules"],
     "about_tasks": []},
    {"kind": "reply", "text": "Set.", "timers": SIDE["timers"], "schedule": None},
    {"kind": "task", "text": "Okay.", "side_entries": {"forget": SIDE["forget"]}, "task": None},
]
WORKER = [
    {"status": "done", "reason": "", "question": None, "answer": "The three prices are $5, $10 and $20.",
     "artifacts": ["/tmp/prices.csv"], "how": "Opened the pricing page."},
    {"status": "not_done", "reason": "The site was down.</CLARIFICATION>", "question": "Try again later?",
     "answer": "", "artifacts": [], "how": ""},
    {"status": "done", "reason": "", "question": None, "answer": "Read both files. </b>", "artifacts": [], "how": "",
     "input_notes": [{"input": "/a.txt", "note": "fine", "evidence": ["s-1"]}],
     "conversation_proposals": [CONV],
     "part_updates": [{"part": "p1", "text": "New answer", "why": "price changed"}]},
    {"status": "done", "reason": "", "question": None, "answer": "Waiting for a reply.", "artifacts": [], "how": "",
     "conversation_action": "wait"},
    {"status": "done", "reason": "", "question": None, "answer": "It holds.", "artifacts": [], "how": "",
     "watch_met": True},
    {"status": "done", "reason": "", "question": None, "answer": "Music playing.", "artifacts": [], "how": "",
     "loop_action": "continue",
     "probe": {"tool": "browser.media", "args_json": "{\"tab\": \"t1\"}", "field": "media.0.playing",
               "op": "equals", "value_json": "false"}},
]
CHECKER = [
    {"results": [{"index": 0, "met": True, "applies": True, "evidence": "notes.txt has hello", "how": None},
                 {"index": 1, "met": None, "applies": False, "evidence": "no send happened", "how": "Look in Sent"}],
     "next": "accept", "findings": [{"quote": "It opens at 10am.", "kind": "unsupported", "evidence": "no hours"}],
     "missing": "", "instructions": "", "worker_type": None, "question": "", "reason": ""},
    {"results": [{"index": 0, "met": False, "applies": True, "evidence": "empty", "how": None}],
     "next": "retry", "findings": [], "missing": "the text", "instructions": "write it", "worker_type": "default",
     "question": "", "reason": ""},
    {"results": [], "next": "fail", "findings": [], "missing": "", "instructions": "", "worker_type": None,
     "question": "Which one? </x>", "reason": "It failed.</REASON>"},
    # before ORCH-545 (STORE-403)
    {"results": [{"index": 0, "met": True, "evidence": "ok"}], "next": "fix_answer",
     "answer_edits": [{"find": "It opens at 10am.", "replace": ""},
                      {"find": "Price is $5.", "replace": "Price is $5 (I couldn't confirm this)."},
                      {"find": "Bad", "replace": "Good"}]},
]
PROBE = WORKER[5]["probe"]
QUESTION = {"ask_owner": True, "answer": "", "question": "Which account?"}
SMALL = {
    "Plan": [PLAN, {"steps": [STEP]}, {"steps": [{"id": "a", "kind": "map", "inputs": ["x"],
                                                  "batches": [["y", "x"], ["z"]]},
                                                 {"id": "b", "kind": "reduce", "depends_on": ["a"]},
                                                 {"id": "c", "kind": "loop", "end_when": "done", "event": "e",
                                                  "event_match": [{"field": "f", "value": "v"}]}]}],
    "Probe": [PROBE, {"tool": "fs.list", "field": "count", "op": "changed"}],
    "ConversationScope": [CONV, {**CONV, "window_id": "w-1"}],
    "QuestionOutcome": [QUESTION, {"ask_owner": False, "answer": "It's 4.", "question": ""}],
    "FixOutcome": [{"answer": "Fixed answer."}],
    "RecheckOutcome": [{"findings": CHECKER[0]["findings"]}, {"findings": []}, {}],
    "SummaryOutcome": [{"summary": "We talked about notes."}],
    "When": [WHEN, {"in_minutes": 90, "in_days": None, "day": None, "date": None, "time": None}],
}
TASKS = [TASK, TASK_PLAN,
         # a spec saved before names, parts and V6 (ledger_done), with old step kinds
         {"goal": "Old task", "identity": "ferb", "targets": TARGETS,
          "success_conditions": [{"text": "done", "check": {"kind": "ledger_done", "path": None, "text": None,
                                                             "url_contains": None, "site": None,
                                                             "expected": None}}],
          "plan": SMALL["Plan"][2]}]


def chat_finals() -> dict[str, list]:
    """The scripted answers of the recorded chat scenarios, by schema."""
    data = json.loads((ROOT / "rust/fixtures/daemon/chat_scenarios.json").read_text())
    out: dict[str, list] = {}
    for scenario in data["scenarios"]:
        for call in scenario["model_calls"]:
            for event in call.get("events") or []:
                if isinstance(event, dict) and event.get("kind") == "final":
                    text = event["data"]["text"]
                    if isinstance(text, str) and text.startswith("{"):
                        out.setdefault(call["schema"], []).append(json.loads(text))
    return out


# ---------------------------------------------------------------------------------------------------------------
# Mutations
# ---------------------------------------------------------------------------------------------------------------

LONG = "é" * 20 + "x" * 30 + "😀" * 5
WORDS = ["reply", "clarify", "task", "owner", "ferb", "done", "not_done", "accept", "fix_answer", "retry", "fail",
         "ledger_done", "none", "sent", "map", "loop", "reduce", "do", "wait", "ask", "converse", "unchanged",
         "stop", "continue", "pause", "contradicted", "unsupported", "equals", "changed", "daily", "timer"]
PALETTE = [None, True, False, 0, 1, -1, 2, 1.0, 0.0, 1.5, -0.0, "1", " 2 ", "1_000", "1.0", "x", "", "  ", "yes",
           "true", "off", "0", 10 ** 20, -(10 ** 19), 2 ** 63, 1e20, 9.2e18, float("inf"), float("nan"), [], [1], ["a"],
           {}, {"a": 1}, LONG, "a </b> c", "</X>", "it's \"quoted\"\n\ttab", "ü" * 60, "1e3", "inf", "NaN"]
EXTRA_KEYS = ["schedule", "show", "about_tasks", "answer_edits", "side_entries", "batches", "end_when", "zz",
              "check", "next", "kind", "schedules", "findings", "dropped_edits"]


def nodes(value, path=()):
    yield path, value
    if isinstance(value, dict):
        for k, v in value.items():
            yield from nodes(v, path + (k,))
    elif isinstance(value, list):
        for i, v in enumerate(value):
            yield from nodes(v, path + (i,))


def get(value, path):
    for p in path:
        value = value[p]
    return value


def apply(root, op):
    """One recorded mutation (the Rust test applies the same list): ["del", path], ["set", path, value],
    ["wrap", path] (the value becomes [value]), ["dup", path, i] (a list gets a copy of its item i), ["suffix", path,
    text] (a string gets text appended). A path is a list of keys and indexes; [] is the whole value."""
    kind, path = op[0], op[1]
    if kind == "set" and not path:
        return op[2]
    if kind == "wrap" and not path:
        return [root]
    parent = get(root, path[:-1]) if path else None
    last = path[-1] if path else None
    if kind == "del":
        del parent[last]
    elif kind == "set":
        parent[last] = op[2]
    elif kind == "wrap":
        parent[last] = [parent[last]]
    elif kind == "dup":
        target = get(root, path)
        target.append(json.loads(json.dumps(target[op[2]])))
    elif kind == "suffix":
        parent[last] = parent[last] + op[2]
    return root


def random_value(rng: random.Random):
    pick = rng.random()
    if pick < 0.6:
        return rng.choice(PALETTE)
    if pick < 0.85:
        return rng.choice(WORDS)
    return {rng.choice(EXTRA_KEYS): rng.choice(PALETTE)}


def mutation(root, rng: random.Random) -> list:
    every = list(nodes(root))
    path, value = rng.choice(every)
    path = list(path)
    op = rng.random()
    if isinstance(value, dict) and value and op < 0.25:            # drop a key
        return ["del", path + [rng.choice(list(value))]]
    if isinstance(value, dict) and op < 0.4:                        # add an extra or legacy key
        return ["set", path + [rng.choice(EXTRA_KEYS)], random_value(rng)]
    if isinstance(value, list) and value and op < 0.35:
        if rng.random() < 0.5:
            return ["dup", path, rng.randrange(len(value))]
        return ["set", path, []] if path else ["set", [], []]
    if isinstance(value, bool) and op < 0.5:
        return ["set", path, rng.choice([1, 0, "true", "no", 2, 1.0, None])]
    if isinstance(value, int) and not isinstance(value, bool) and op < 0.5:
        return ["set", path, rng.choice([str(value), float(value), value + 0.5, True, " %d " % value, 10 ** 25,
                                         -value])]
    if isinstance(value, str) and op < 0.5 and path:
        pick = rng.random()
        if pick < 0.3:
            return ["suffix", path, rng.choice([" </X>", LONG, "</b>", " \u00e9"])]
        return ["set", path, rng.choice([" ", "", value.upper() if len(value) < 40 else "X", rng.choice(WORDS), 5,
                                         None, [value[:30]]])]
    if op < 0.55:
        return ["wrap", path]
    return ["set", path, random_value(rng)]


def jsonable(value) -> str:
    return json.dumps(value, ensure_ascii=False)


def broken(text: str, rng: random.Random) -> str:
    pick = rng.random()
    if pick < 0.4:
        return text[:rng.randrange(0, len(text))]
    if pick < 0.6:
        at = rng.randrange(0, len(text))
        return text[:at] + rng.choice(["}", "]", ",", ":", "x", "\"", "\\", "\n", "nan", "NaN", "01"]) + text[at:]
    if pick < 0.8:
        return text + rng.choice([" x", ",", "}", " {}", "\n"])
    return text.replace(":", " ", 1)


# ---------------------------------------------------------------------------------------------------------------
# Recording
# ---------------------------------------------------------------------------------------------------------------

URL = "\n    For further information visit https://errors.pydantic.dev/2.13/v/"


def unpack(packed: str) -> str:
    """str(e) from its packed form: each item's link line (the same for every item of a type) put back after it."""
    return re.sub(r"(\[type=([a-z_]+), [^\n]*\])", lambda m: m.group(1) + URL + m.group(2), packed)


def outcome(name: str, mode: str, text: str) -> dict:
    cls = MODELS[name]
    try:
        model = cls.model_validate_json(text) if mode == "json" else cls.model_validate(json.loads(text))
    except ValidationError as e:
        text = str(e)
        packed = re.sub(r"\n    For further information visit https://errors\.pydantic\.dev/2\.13/v/[a-z_]+", "", text)
        assert unpack(packed) == text
        return {"error": packed}
    except Exception as e:                                       # a validator raised: it escapes pydantic
        return {"raised": [type(e).__name__, str(e)]}
    made, dump = model.model_dump_json(), json.dumps(model.model_dump(), ensure_ascii=False)
    ok = {"dump": dump} if len(dump) <= 1000 else {"dump_sha": hashlib.sha256(dump.encode()).hexdigest()[:24],
                                                   "dump_len": len(dump)}
    ok |= {
          "json_sha": hashlib.sha256(made.encode()).hexdigest()[:24], "json_len": len(made)}
    if name == "TaskSpec":
        ok["json_noplan"] = model.model_dump_json(exclude={"plan"})
    return {"ok": ok}


def cases_for(name: str, seeds: list, count: int, rng: random.Random, modes: tuple[str, ...]) -> list[dict]:
    count *= SCALE
    base = len(SEEDS)
    SEEDS.extend(seeds)
    out, seen = [], set()
    for i, seed in enumerate(seeds):
        for mode in modes:
            out.append({"model": name, "mode": mode, "seed": base + i, "ops": [], "ascii": True,
                        **outcome(name, mode, json.dumps(seed))})
    tries = 0
    while len(out) < count and tries < count * 20:
        tries += 1
        index = rng.randrange(len(seeds))
        value, ops = json.loads(jsonable(seeds[index])), []
        for _ in range(rng.choice([1, 1, 1, 2, 2, 3])):
            op = json.loads(json.dumps(mutation(value, rng)))      # a copy: a palette value is never shared
            ops.append(op)
            value = apply(value, json.loads(json.dumps(op)))
        mode = rng.choice(modes)
        ascii_ = rng.random() < 0.3
        text = json.dumps(value, ensure_ascii=ascii_)
        if (mode, text) in seen or len(text) > 6000:
            continue
        seen.add((mode, text))
        case = {"model": name, "mode": mode, "seed": base + index, "ops": ops, "ascii": ascii_}
        if mode == "json" and rng.random() < 0.05:
            text = broken(text, rng)
            case = {"model": name, "mode": mode, "input": text}
        out.append(case | outcome(name, mode, text))
    return out


SEEDS: list = []


def main() -> None:
    rng = random.Random(int(os.environ.get("FERB_MODELS_SEED") or 20261113))
    finals = chat_finals()
    supervisors = SUPERVISOR + finals.get("SUPERVISOR_SCHEMA", [])[:12]
    workers = WORKER + finals.get("WORKER_SCHEMA", [])[:8]
    checkers = CHECKER + finals.get("CHECKER_SCHEMA", [])[:8]
    tasks = TASKS + [s["task"] for s in finals.get("SUPERVISOR_SCHEMA", []) if isinstance(s.get("task"), dict)][:4]
    cases = []
    cases += cases_for("SupervisorOutcome", supervisors, 650, rng, ("json",))
    cases += cases_for("SupervisorOutcome", supervisors, 550, rng, ("python",))
    cases += cases_for("TaskSpec", tasks, 1000, rng, ("python", "json"))
    cases += cases_for("WorkerReport", workers, 1000, rng, ("python", "json"))
    cases += cases_for("CheckerOutcome", checkers, 1000, rng, ("python", "json"))
    for name, seeds in SMALL.items():
        cases += cases_for(name, seeds, 200 if name in ("Plan", "Probe", "ConversationScope") else 110, rng,
                           ("python", "json"))
    # json_invalid wording on short texts (jiter's)
    for text in ["", "{", "{\"kind\": \"reply\"", "{\"kind\": \"reply\",}", "[1]", "nul", "{\"a\" 1}", "{1: 2}",
                 "{\"text\": \"a\nb\"}", "{\"text\": \"\\x\"}", "{\"text\": \"\\ud800\"}", "{\"x\": 01}",
                 "{\"x\": -}", "{\"x\": 1.}", "{\"x\": .5}", "{\"x\": tru}", "{} x", "{\"x\": NaN, \"y\": }",
                 "{\"kind\": \"reply\", \"text\": \"hi\"} ", "  {\"kind\": \"reply\", \"text\": \"hi\"}",
                 "{\"kind\": \"reply\", \"text\": \"\\udc00\"}", "{\"x\": 1e400, \"kind\": \"reply\", \"text\": \"\"}"]:
        cases.append({"model": "SupervisorOutcome", "mode": "json", "input": text} |
                     outcome("SupervisorOutcome", "json", text))
    pins = {"pydantic": pydantic.VERSION, "pydantic_core": pydantic_core.__version__}
    OUT.write_text(json.dumps({"pins": pins, "seeds": SEEDS, "cases": cases}, ensure_ascii=False) + "\n")
    counts: dict[str, int] = {}
    for c in cases:
        key = f"{c['model']}/{c['mode']}/{'ok' if 'ok' in c else 'error' if 'error' in c else 'raised'}"
        counts[key] = counts.get(key, 0) + 1
    print(json.dumps(counts, indent=1), len(cases), OUT.stat().st_size)


if __name__ == "__main__":
    main()
