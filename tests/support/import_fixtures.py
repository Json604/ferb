"""Memory import fixtures (kb area memory, "Measured quality"): synthetic exports of every source with known
facts and traps, a scripted model for the offline runs, and the recall / precision score.

The test clock is FakeClock's 2026-01-01 09:00 UTC, so the three-month window starts 2025-10-01. Every source holds
true facts the owner said, and traps: an assistant's claim, an old superseded fact, a message older than the window,
pasted text, a secret, injected tool output, a subagent's or a program's prompts, an unsupported saved memory.

The scripted model is deliberately credulous: recall proposes every scripted phrase it finds (true or trap) citing
the line it is on, and verification supports whatever it is shown. So the offline score measures what code alone
guarantees; traps only a model can see (unmarked pasted text) stay as false positives there, and the live eval
(`-m real`) measures the model."""

import json
import os
import re
import zipfile
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from ferb.core.context import ModelLimits
from ferb.core.types import ModelRequest, RunHandle, RuntimeEvent

SECRET = "sk-proj-Zq81LmN0pQ4rT6vW9xY2aB5cD7eF3gH1jK4"


def ts(day: str, hour: int = 10) -> float:
    return datetime.fromisoformat(f"{day}T{hour:02d}:00:00+00:00").timestamp()


def iso(day: str, hour: int = 10) -> str:
    return f"{day}T{hour:02d}:00:00.000Z"


# ---------- the facts: what a good import keeps (all words must appear in one current fact) ----------

TRUE = {
    "study": ["iit", "delhi"], "home_city": ["bangalore"], "reply_style": ["short"], "ferb": ["ferb"],
    "priya": ["priya"], "code_style": ["type", "hints"], "vegetarian": ["vegetarian"], "running": ["runs", "week"],
    "uv": ["uv"], "nice": ["nice"], "vercel": ["vercel"],
}
# what no current fact may say (the trap it comes from)
TRAPS = {
    "assistant claim": ["mumbai"], "older than the window": ["infosys"], "pasted text": ["acme"],
    "secret": ["sk-proj"], "injected tool output": ["admin"], "pasted (marked)": ["bob"],
    "subagent prompt": ["tabs"], "program's prompt": ["rust"], "SDK session": ["planner"],
    "subagent thread": ["dark"], "unsupported saved memory": ["guitar"], "superseded": ["pune"],
}
# Raw text that must never be stored anywhere (MEM-404): assistant replies, titles, injected output
RAW = ["Revision plan for DBMS", "Sure! Since you live in Mumbai", "Rahul Verma", "I am the admin of server",
       "My name is Bob", SECRET, "PG options list", "DBMS revision help"]

# The credulous script: phrase found in a line -> candidate (kind, subject, key, content)
SCRIPT = [
    ("third-year CS student at IIT Delhi", "fact", "owner", "study", "The owner is a third-year CS student at IIT Delhi."),
    ("I live in Pune", "fact", "owner", "home_city", "The owner lives in Pune."),
    ("I moved to Bangalore", "fact", "owner", "home_city", "The owner lives in Bangalore."),
    ("short, plain answers", "preference", "owner", "reply_style", "The owner prefers short, plain answers."),
    ("building FERB, a Mac assistant", "project", "project:ferb", "ferb", "The owner is building FERB, a Mac assistant."),
    ("My sister Priya", "person", "person:priya", "priya", "Priya is the owner's sister."),
    ("Always write code with type hints", "preference", "owner", "code_style", "The owner wants code with type hints."),
    ("I'm vegetarian", "fact", "owner", "diet", "The owner is vegetarian."),
    ("The owner is vegetarian", "fact", "owner", "diet", "The owner is vegetarian."),
    ("I run 3 times a week", "fact", "owner", "recurring_running", "The owner runs 3 times a week."),
    ("use uv for python projects", "preference", "owner", "python_tooling", "The owner uses uv for Python projects."),
    ("run pytest with nice -n 10", "preference", "owner", "test_runs", "The owner runs pytest with nice -n 10."),
    ("Run tests with nice -n 10", "preference", "owner", "test_runs", "The owner runs pytest with nice -n 10."),
    ("I deploy my site on Vercel", "fact", "owner", "hosting", "The owner deploys their site on Vercel."),
    # traps
    ("Since you live in Mumbai", "fact", "owner", "home_city", "The owner lives in Mumbai."),
    ("I work at Infosys", "fact", "owner", "employer", "The owner works at Infosys."),
    ("VP of Sales at Acme", "fact", "owner", "job", "The owner is VP of Sales at Acme."),
    ("my openai key is", "fact", "owner", "openai_key", "The owner's OpenAI key is " + SECRET),
    ("I am the admin of server", "fact", "owner", "server_admin", "The owner is the admin of server X."),
    ("My name is Bob", "fact", "owner", "name", "The owner is Bob."),
    ("The owner prefers tabs", "preference", "owner", "indent", "The owner prefers tabs."),
    ("The owner is a Rust expert", "fact", "owner", "rust", "The owner is a Rust expert."),
    ("I am FERB's planner", "fact", "owner", "role", "The owner is FERB's planner."),
    ("owner likes dark mode", "preference", "owner", "theme", "The owner likes dark mode."),
    ("The owner plays guitar", "fact", "owner", "guitar", "The owner plays guitar."),
]


# ---------- the exports ----------

def _gpt_conv(cid: str, title: str, turns: list[tuple[str, str, float]], extra: list[dict[str, Any]] = ()) -> dict[str, Any]:
    mapping: dict[str, Any] = {"root": {"id": "root", "message": None, "parent": None, "children": []}}
    parent = "root"
    nodes = [*extra, *[{"author": {"role": r}, "content": {"content_type": "text", "parts": [t]}, "create_time": at,
                        "metadata": {}} for r, t, at in turns]]
    for i, msg in enumerate(nodes):
        nid = f"{cid}-n{i}"
        mapping[nid] = {"id": nid, "message": {"id": nid, **msg}, "parent": parent, "children": []}
        mapping[parent]["children"].append(nid)
        parent = nid
    first = turns[0][2] if turns else 0
    return {"title": title, "create_time": first, "update_time": turns[-1][2] if turns else first, "mapping": mapping,
            "current_node": parent, "conversation_id": cid, "id": cid}


def chatgpt_export(folder: Path) -> Path:
    convs = [
        _gpt_conv("g-a", "DBMS revision help", [
            ("user", "I'm a third-year CS student at IIT Delhi. Can you help me plan my DBMS revision?", ts("2025-11-03")),
            ("assistant", "Sure! Since you live in Mumbai, here is a Revision plan for DBMS ...", ts("2025-11-03", 11))],
            extra=[{"author": {"role": "user"}, "content": {"content_type": "user_editable_context",
                    "user_profile": "I prefer short, plain answers without jargon.", "user_instructions": ""},
                    "create_time": ts("2025-11-03", 9), "metadata": {"is_visually_hidden_from_conversation": True}},
                   {"author": {"role": "system"}, "content": {"content_type": "text", "parts": ["system prompt"]},
                    "create_time": ts("2025-11-03", 9), "metadata": {"is_visually_hidden_from_conversation": True}}]),
        _gpt_conv("g-b", "Moving", [("user", "I live in Pune right now. What are good weekend trips?", ts("2025-10-05")),
                                    ("assistant", "Lonavala is close.", ts("2025-10-05", 11))]),
        _gpt_conv("g-c", "PG options list", [("user", "I moved to Bangalore last week, need a list of PG options",
                                              ts("2025-12-10")), ("assistant", "Here are some.", ts("2025-12-10", 11))]),
        _gpt_conv("g-d", "Old job", [("user", "I work at Infosys as an intern, help me write a leave mail",
                                      ts("2025-07-15"))]),
        _gpt_conv("g-e", "Shorten", [("user", "Can you shorten this? Hi team, I am Rahul Verma, VP of Sales at Acme, "
                                      "and I want to share our Q3 numbers.", ts("2025-11-20"))]),
        _gpt_conv("g-f", "Key", [("user", f"my openai key is {SECRET} please check why it fails", ts("2025-12-01"))]),
    ]
    out = folder / "chatgpt-export.zip"
    with zipfile.ZipFile(out, "w") as z:
        z.writestr("conversations.json", json.dumps(convs))
        z.writestr("chat.html", "<html></html>")
        z.writestr("user.json", json.dumps({"id": "user-1"}))
    return out


def claude_export(folder: Path) -> Path:
    root = folder / "claude-export"
    root.mkdir(parents=True)

    def msg(sender: str, text: str, day: str) -> dict[str, Any]:
        return {"uuid": f"{sender}-{day}", "text": text, "content": [{"type": "text", "text": text}], "sender": sender,
                "created_at": iso(day), "updated_at": iso(day), "attachments": [], "files": []}
    (root / "conversations.json").write_text(json.dumps([
        {"uuid": "c-1", "name": "FERB plans", "created_at": iso("2025-11-15"), "updated_at": iso("2025-11-15"),
         "chat_messages": [msg("human", "I'm building FERB, a Mac assistant in Python and SwiftUI.", "2025-11-15"),
                           msg("assistant", "Great project!", "2025-11-15")]},
        {"uuid": "c-2", "name": "Visit", "created_at": iso("2025-12-20"), "updated_at": iso("2025-12-20"),
         "chat_messages": [msg("human", "My sister Priya is visiting next week, plan a day out.", "2025-12-20")]},
        {"uuid": "c-3", "name": "Food", "created_at": iso("2025-10-20"), "updated_at": iso("2025-10-20"),
         "chat_messages": [msg("human", "I'm vegetarian, suggest three dinner recipes.", "2025-10-20")]},
    ]))
    (root / "projects.json").write_text(json.dumps([
        {"uuid": "p-1", "name": "Code", "description": "", "prompt_template": "Always write code with type hints.",
         "created_at": iso("2025-11-01"), "updated_at": iso("2025-11-01"), "docs": []}]))
    (root / "memories.json").write_text(json.dumps([
        {"conversations_memory": "The owner is vegetarian.\nThe owner plays guitar.", "project_memories": {}}]))
    (root / "users.json").write_text(json.dumps([{"uuid": "u-1"}]))
    return root


def gemini_export(folder: Path, html: bool = False) -> Path:
    root = folder / "Takeout" / "My Activity" / "Gemini Apps"
    root.mkdir(parents=True)
    if html:
        (root / "MyActivity.html").write_text(
            '<html><body><div class="outer-cell mdl-cell"><div class="mdl-grid">'
            '<div class="header-cell"><p class="mdl-typography--title">Gemini Apps<br></p></div>'
            '<div class="content-cell mdl-cell">Prompted&nbsp;what&#39;s a good 5k plan? I run 3 times a week<br>'
            'Nov 8, 2025, 10:00:00 AM UTC<br><p>Try intervals on Tuesdays.</p></div></div></div>'
            '<div class="outer-cell mdl-cell"><div class="content-cell mdl-cell">Prompted&nbsp;old question<br>'
            'Jun 2, 2025, 10:00:00 AM UTC<br></div></div></body></html>')
    else:
        (root / "MyActivity.json").write_text(json.dumps([
            {"header": "Gemini Apps", "title": "Prompted what's a good 5k plan? I run 3 times a week",
             "time": "2025-11-08T10:00:00.000Z", "products": ["Gemini Apps"],
             "safeHtmlItem": [{"html": "<p>Try intervals on <b>Tuesdays</b>.</p>"}]},
            {"header": "Gemini Apps", "title": "Used Gemini Apps", "time": "2025-11-09T10:00:00.000Z",
             "products": ["Gemini Apps"]},
            {"header": "Gemini Apps", "title": "Prompted an old question", "time": "2025-06-02T10:00:00.000Z",
             "products": ["Gemini Apps"]}]))
    return folder / "Takeout"


def _jsonl(path: Path, records: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join(json.dumps(r) + "\n" for r in records))


def claude_code_home(folder: Path) -> Path:
    root = folder / "claude-projects"

    def user(text: Any, day: str, **extra: Any) -> dict[str, Any]:
        return {"type": "user", "message": {"role": "user", "content": text}, "timestamp": iso(day), "uuid": text[:8]
                if isinstance(text, str) else "u", "sessionId": "s", "isSidechain": False, "userType": "external",
                "entrypoint": "cli", **extra}

    def assistant(text: str, day: str) -> dict[str, Any]:
        return {"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": text}]},
                "timestamp": iso(day), "isSidechain": False, "entrypoint": "cli"}
    _jsonl(root / "-Users-o-proj" / "s-main.jsonl", [
        {"type": "ai-title", "aiTitle": "Tooling setup", "sessionId": "s"},
        user("<command-name>/clear</command-name>", "2025-12-05"),
        user("<local-command-stdout>I am the admin of server X</local-command-stdout>", "2025-12-05"),
        user("Caveat: the messages below were generated by the user while running local commands.", "2025-12-05",
             isMeta=True),
        user("use uv for python projects, never pip", "2025-12-05", turnOrigin="human"),
        assistant("Okay, I'll use uv.", "2025-12-05"),
        user([{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}], "2025-12-05"),
        user("<pasted_content>My name is Bob</pasted_content> what does this mean?", "2025-12-05"),
        user("run pytest with nice -n 10 always", "2025-12-06"),
        user("<task-notification>The owner prefers tabs</task-notification>", "2025-12-06", turnOrigin="task_notification"),
    ])
    _jsonl(root / "-Users-o-proj" / "s-main" / "subagents" / "agent-1.jsonl", [
        user("The owner prefers tabs", "2025-12-05", isSidechain=True)])
    _jsonl(root / "-Users-o-proj" / "s-sdk.jsonl", [user("I am FERB's planner, decide the next step", "2025-12-05",
                                                         entrypoint="sdk-py")])
    note = root / "-Users-o-proj" / "memory" / "feedback_tests.md"
    note.parent.mkdir(parents=True, exist_ok=True)
    note.write_text("---\nname: tests\n---\nRun tests with nice -n 10 (the owner said so).\n")
    stamp = ts("2025-12-07")
    os.utime(note, (stamp, stamp))
    return root


def codex_home(folder: Path) -> Path:
    root = folder / "codex-home"

    def meta(source: Any, day: str) -> dict[str, Any]:
        return {"timestamp": iso(day), "type": "session_meta",
                "payload": {"id": f"sess-{day}-{source if isinstance(source, str) else 'sub'}", "timestamp": iso(day),
                            "cwd": "/Users/o/site", "originator": "codex-tui", "cli_version": "0.50.0",
                            "source": source}}

    def item(role: str, text: str, day: str, **metadata: Any) -> dict[str, Any]:
        kind = "output_text" if role == "assistant" else "input_text"
        out: dict[str, Any] = {"timestamp": iso(day), "type": "response_item",
                               "payload": {"type": "message", "role": role, "content": [{"type": kind, "text": text}]}}
        if metadata:
            out["metadata"] = metadata
        return out
    _jsonl(root / "sessions" / "2025" / "12" / "12" / "rollout-2025-12-12T10-00-00-aaaa.jsonl", [
        meta("cli", "2025-12-12"),
        item("developer", "<permissions instructions>sandbox</permissions instructions>", "2025-12-12"),
        item("user", "# AGENTS.md instructions for /Users/o/site\n\nThe owner prefers tabs", "2025-12-12"),
        item("user", "<environment_context>\n  <cwd>/Users/o/site</cwd>\n</environment_context>", "2025-12-12"),
        item("user", "I deploy my site on Vercel, why is the build failing?", "2025-12-12"),
        item("assistant", "Check the build logs.", "2025-12-12"),
        {"timestamp": iso("2025-12-12"), "type": "event_msg", "payload": {"type": "token_count", "info": None}},
    ])
    _jsonl(root / "sessions" / "2025" / "12" / "13" / "rollout-2025-12-13T10-00-00-bbbb.jsonl", [
        meta("exec", "2025-12-13"), item("user", "You are a code reviewer. The owner is a Rust expert.", "2025-12-13")])
    _jsonl(root / "sessions" / "2025" / "12" / "14" / "rollout-2025-12-14T10-00-00-cccc.jsonl", [
        meta({"subagent": {"thread_spawn": {"parent_thread_id": "p", "depth": 1}}}, "2025-12-14"),
        item("user", "Note: owner likes dark mode", "2025-12-14", inherited_user_message=True)])
    return root


@dataclass
class Exports:
    chatgpt: Path
    claude: Path
    gemini: Path
    claude_code: Path
    codex: Path

    def path(self, source: str) -> str:
        return str(getattr(self, source))


def make_exports(folder: Path) -> Exports:
    folder.mkdir(parents=True, exist_ok=True)
    return Exports(chatgpt_export(folder), claude_export(folder), gemini_export(folder), claude_code_home(folder),
                   codex_home(folder))


# ---------- the scripted model ----------

_LINE = re.compile(r"^\s*\[([^\]·]+?) · ([^·\]]+?) · ([^\]]+)\] (.*)$")


def _messages(text: str) -> list[tuple[str, str, str]]:
    """(ref, who, text) of each message line in a context, continuation lines joined."""
    out: list[list[str]] = []
    for line in text.split("\n"):
        m = _LINE.match(line)
        if m:
            out.append([m.group(1).strip(), m.group(2).strip(), m.group(4)])
        elif out and line.strip() and not line.startswith(("Candidate ", "  says:", "  quoted:", "  evidence:",
                                                            "What FERB", "  [", "Conversation", "Saved memory")):
            out[-1][2] += "\n" + line
    return [(a, b, c) for a, b, c in out]


def recall_answer(text: str) -> dict[str, Any]:
    out = []
    for ref, _who, words in _messages(text):
        for phrase, kind, subject, key, content in SCRIPT:
            if phrase.lower() in words.lower():
                at = words.lower().index(phrase.lower())
                out.append({"kind": kind, "subject": subject, "key": key, "content": content, "message": ref,
                            "quote": words[at:at + len(phrase)]})
    return {"candidates": out}


def verify_answer(text: str) -> dict[str, Any]:
    verdicts = []
    for block in re.split(r"\n(?=Candidate v\d+:)", text):
        head = re.match(r"Candidate (v\d+): kind=(\S+) subject=(\S+) key=(\S+)", block.strip())
        if not head:
            continue
        cid, kind, subject, key = head.groups()
        says = re.search(r"  says: (.*)", block)
        quote = re.search(r"  quoted: “(.*)” \(from (\S+)\)", block)
        q, ref = (quote.group(1), quote.group(2)) if quote else ("", "")
        owners = [(r, w) for r, who, w in _messages(block.split("What FERB")[0]) if who == "owner"]
        hit = next(((r, q) for r, w in owners if q and q.lower() in w.lower()), None)
        if hit is None and owners:                            # credulous: the first owner line, its first words
            r, w = owners[0]
            hit = (r, " ".join(w.split()[:4]))
        msg, qq = hit if hit else (ref, q)
        verdicts.append({"id": cid, "supported": True, "kind": kind, "subject": subject, "key": key,
                         "content": says.group(1) if says else "", "message": msg, "quote": qq, "same_as": "",
                         "ended": False, "until": "", "review_in_days": 0})
    return {"verdicts": verdicts}


class ScriptedImportModel:
    """make_runtime for the import: recall and verification answered from SCRIPT; every context recorded."""
    name = "fake"
    limits = ModelLimits("fake-model", 1_000_000, 200_000, "test double")

    def __init__(self) -> None:
        self.contexts: list[tuple[str, str]] = []           # (role, context)
        self.fail_after: int | None = None                   # calls answered before each call raises
        self.block_after: int | None = None                  # calls answered before each call waits forever
        self.verify_override: Any = None

    def __call__(self, role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any], **k: Any) -> Any:
        model, kind = self, "verify" if "verdicts" in schema.get("properties", {}) else "recall"

        class Run:
            name, limits = "fake", ScriptedImportModel.limits

            async def start(self, task: Any, request: ModelRequest, token: str) -> RunHandle:
                self.text = "\n".join(request.parts)
                return RunHandle("r", "fake", task.id, {})

            async def events(self, run: RunHandle) -> Any:
                import asyncio
                n = len(model.contexts)
                model.contexts.append((kind, self.text))
                if model.block_after is not None and n >= model.block_after:
                    await asyncio.Event().wait()
                if model.fail_after is not None and n >= model.fail_after:
                    raise RuntimeError("the model service is unavailable")
                if kind == "verify" and model.verify_override is not None:
                    answer = model.verify_override(self.text)
                else:
                    answer = verify_answer(self.text) if kind == "verify" else recall_answer(self.text)
                yield RuntimeEvent("final", {"text": json.dumps(answer)})

            async def cancel(self, run: RunHandle) -> None:
                return None
        return Run()


# ---------- the score ----------

def score(contents: list[str]) -> dict[str, Any]:
    """Recall: the share of TRUE facts some current fact says (all its words); precision: the share of current facts
    that say a TRUE fact and no trap."""
    low = [c.lower() for c in contents]

    def says(text: str, words: list[str]) -> bool:
        return all(re.search(r"\b" + re.escape(w) + r"\b", text) for w in words)
    found = {k for k, words in TRUE.items() if any(says(c, words) for c in low)}
    traps = {k for k, words in TRAPS.items() if any(says(c, words) for c in low)}
    good = [c for c in low if any(says(c, w) for w in TRUE.values()) and not any(says(c, w) for w in TRAPS.values())]
    return {"recall": round(len(found) / len(TRUE), 3), "precision": round(len(good) / len(low), 3) if low else 0.0,
            "found": sorted(found), "missed": sorted(set(TRUE) - found), "traps_kept": sorted(traps),
            "facts": len(low)}


def utc(day: str) -> datetime:
    return datetime.fromisoformat(day).replace(tzinfo=UTC)
