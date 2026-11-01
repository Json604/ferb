"""Owns reading other AI assistants' records into one normalized form, in memory only (MEM-404:
the raw chats are never kept). Must not persist or log what it reads, mark anything as the owner's own authority, or
let an unredacted text out of this module.

Five sources (MEM-403), each read from where the owner keeps it:
- `chatgpt`: the account's data export (Settings → Data controls → Export data), a zip holding `conversations.json`
  (larger exports: `conversations-000.json`, `-001`, …): each conversation's `mapping` of message nodes, the shown
  branch followed from `current_node` up its parents. `author.role` user → owner, assistant → assistant, tool → tool;
  system and messages hidden from the conversation are left out. Custom instructions (`user_editable_context`, or
  `metadata.user_context_message_data`: the owner's own "about me" text) are read once as the owner's. The export has
  no memory file: ChatGPT's saved memory shows as the newest `model_editable_context` snapshot and the assistant's
  memory writes (recipient "bio"), read as its claims.
- `claude`: the claude.ai data export (Settings → Privacy → Export data): one zip, or since 2026 one zip per kind
  (`conversations-000.zip`, `projects-000.zip` with `projects/<uuid>.json`, `memories-000.zip`), given as a folder;
  `chat_messages` with `sender` human / assistant (text from the `content` text blocks); a project's description and
  instructions are the owner's; the saved memory (`conversations_memory`, `project_memories`, `memory_files`) is the
  assistant's claims.
- `claude_code`: the local transcripts `~/.claude/projects/<project>/<session>.jsonl`. Only the owner's typed turns are
  the owner's: `user` records that are not meta, not a compact summary, not a sidechain and not a tool result, whose
  `turnOrigin` (when recorded) is human. Subagent transcripts (`subagents/`) and SDK sessions (`entrypoint` sdk-*: a
  program's prompts) are left out. Its saved memory notes (`<project>/memory/*.md`) are the assistant's claims.
- `codex`: Codex CLI's local sessions `~/.codex/sessions/**/rollout-*.jsonl` (and `archived_sessions/`; a rollout Codex
  compressed, `rollout-*.jsonl.zst`, is read the same way through `zstandard`): only
  interactive sessions (`session_meta.source` "cli" or "vscode"; `codex exec` runs and subagent threads carry prompts
  programs wrote). Owner turns are `response_item` messages of role user that Codex didn't inherit from a parent thread.
- `gemini`: Google Takeout, "My Activity" → "Gemini Apps": `MyActivity.json` (each record a prompt: `title` "Prompted
  …", `time` in UTC, the reply's HTML in `safeHtmlItem`; or the variant with `details` Request / Response) or
  `MyActivity.html` (the same in `outer-cell` blocks, the time written in a local zone). No thread ids: a day's prompts
  read as one conversation. English Takeout's "Prompted" marks a prompt. Other languages write their own marker
  word(s) in the same title with nothing structural around them (no separate field, no markup), so they aren't read:
  telling the marker from the prompt would take a list of words per language (step 12c ruling).

What every reader shares (`finish`): only messages from the last three months by their own time (MEM-401); text
blocks a client injected into a turn (`<tag>…</tag>`: command output, environment context, pasted content) are not the
owner's words and are dropped, pasted ones named as such; every text is cut at a word boundary to its cap (said in the
text) and redacted before it leaves this module (rule 7), so nothing a model, a trace or the database sees holds a
secret. Tool output is not read."""

import html
import json
import re
import zipfile
from collections.abc import Callable, Iterable, Iterator
from dataclasses import dataclass, field
from datetime import UTC, datetime, timedelta
from html.parser import HTMLParser
from pathlib import Path
from typing import Any

SOURCES = {"chatgpt": "ChatGPT", "claude": "Claude", "claude_code": "Claude Code", "codex": "Codex CLI",
           "gemini": "Gemini"}
EXPORTS = ("chatgpt", "claude", "gemini")          # read from a file the owner gives; the other two from this Mac
WINDOW_MONTHS = 3                                  # MEM-401 (owner, 2026-10-30)
OWNER_CHARS = 6000                                 # one owner message as the model reads it
ASSISTANT_CHARS = 700                              # an assistant reply corroborates: its start is enough
CLAIM_CHARS = 6000                                 # one saved memory note
MEMBER_LIMIT = 1 << 30                             # one file inside an export: 1 GiB uncompressed at most
CODEX_INTERACTIVE = frozenset({"cli", "vscode"})   # SessionSource of a session the owner typed into
_BLOCK = re.compile(r"<([a-z][a-z0-9_\-]{1,40})(?:\s[^<>]*)?>.*?</\1>", re.DOTALL)
_OPEN_TAG = re.compile(r"\s*<[a-z][a-z0-9_\-]{1,40}(?:\s[^<>]*)?>")


class ImportUnreadable(ValueError):
    """The file isn't the export it should be (said in the owner's terms)."""


@dataclass(frozen=True)
class Message:
    role: str                     # "owner" | "assistant" | "tool"
    text: str
    at: datetime | None


@dataclass
class Conversation:
    source: str
    id: str
    title: str
    messages: list[Message] = field(default_factory=list)
    claims: bool = False          # an assistant's saved memory: claims to check against the owner's words

    @property
    def last(self) -> datetime | None:
        return max((m.at for m in self.messages if m.at is not None), default=None)


def window_start(now: datetime) -> datetime:
    """Three calendar months before `now` (the same day of the month, or the month's last day)."""
    month, year = now.month - WINDOW_MONTHS, now.year
    while month < 1:
        month, year = month + 12, year - 1
    day = now.day
    while True:
        try:
            return now.replace(year=year, month=month, day=day)
        except ValueError:
            day -= 1


def cut(text: str, limit: int) -> str:
    """`text` within `limit` characters, cut before the word that crosses it (a secret is never cut in half), with what
    was left out said."""
    text = text.strip()
    if len(text) <= limit:
        return text
    head = text[:limit]
    space = max(head.rfind(" "), head.rfind("\n"), head.rfind("\t"))
    head = head[:space] if space > limit // 2 else ""
    return f"{head.rstrip()} [… {len(text) - len(head)} more characters not read]"


def owner_words(text: str) -> str:
    """The owner's typed words of a turn: blocks a client injected (`<tag>…</tag>`) dropped, pasted ones named. A turn
    that only opens such a block (an unclosed injected tag) holds none of their words."""
    def gone(m: re.Match[str]) -> str:
        return " [pasted text, not read] " if "paste" in m.group(1) else " "
    out = _BLOCK.sub(gone, text)
    if _OPEN_TAG.match(out) and not out.strip().startswith("[pasted"):
        return ""
    return re.sub(r"[ \t]{2,}", " ", out).strip()


def finish(conversations: Iterable[Conversation], since: datetime, redact: Callable[[str], str]) -> list[Conversation]:
    """Shared by every reader: the window (MEM-401), the caps, redaction, and only conversations holding an owner
    message (or saved claims) in the window; oldest first by their last message."""
    out = []
    for conv in conversations:
        kept = []
        for m in conv.messages:
            if m.role == "tool" or m.at is None or m.at < since or not m.text.strip():
                continue
            limit = OWNER_CHARS if m.role == "owner" else CLAIM_CHARS if conv.claims else ASSISTANT_CHARS
            text = redact(cut(m.text, limit)).strip()
            if text:
                kept.append(Message(m.role, text, m.at))
        if not kept or not (conv.claims or any(m.role == "owner" for m in kept)):
            continue
        out.append(Conversation(conv.source, conv.id, redact(cut(conv.title, 200)), kept, conv.claims))
    return sorted(out, key=lambda c: (c.last or since, c.id))


def read(source: str, path: Path | None, *, now: datetime, redact: Callable[[str], str],
         home: Path | None = None) -> list[Conversation]:
    """The source's conversations in the window, normalized and redacted. `path`: the export the owner gave (a zip, its
    folder or the JSON/HTML file), or for the local sources a folder in place of the usual one."""
    since, home = window_start(now), home or Path.home()
    if source in EXPORTS and path is None:
        raise ImportUnreadable(f"{SOURCES[source]} is imported from its data export: choose the file")
    readers: dict[str, Callable[[], Iterable[Conversation]]] = {
        "chatgpt": lambda: chatgpt(_files(path, ("conversations",), ".json"), now),
        "claude": lambda: claude(_files(path, ("conversations", "projects", "memories", "memory"), ".json"), now),
        "gemini": lambda: gemini(_files(path, ("myactivity", "my activity"), (".json", ".html")), now),
        "claude_code": lambda: claude_code(path or home / ".claude" / "projects"),
        "codex": lambda: codex(path or home / ".codex"),
    }
    if source not in readers:
        raise ImportUnreadable(f"unknown source {source!r}: one of {', '.join(SOURCES)}")
    return finish(readers[source](), since, redact)


# ---------- files ----------

def _files(path: Path | None, stems: tuple[str, ...], suffix: str | tuple[str, ...]) -> list[tuple[str, bytes]]:
    """(name, bytes) of the export's files whose name starts with one of `stems`: from a zip without extracting it, a
    folder (searched), or the file itself."""
    assert path is not None
    suffixes = (suffix,) if isinstance(suffix, str) else suffix

    def wanted(name: str) -> bool:
        # the file's name, or (claude.ai's 2026 layout: projects/<uuid>.json, memories/<uuid>.json) its folder's
        parts = [x.lower() for x in name.replace("\\", "/").split("/") if x]
        return bool(parts) and parts[-1].endswith(suffixes) and any(x.startswith(stems) for x in parts[-2:])

    def from_zip(z: zipfile.ZipFile, where: str) -> None:
        for info in z.infolist():
            if info.is_dir() or not wanted(info.filename):
                continue
            if info.file_size > MEMBER_LIMIT:
                raise ImportUnreadable(f"{info.filename} in the export is larger than 1 GiB")
            found.append((f"{where}/{info.filename}", z.read(info)))
    path = path.expanduser()
    if not path.exists():
        raise ImportUnreadable(f"{path} doesn't exist")
    found: list[tuple[str, bytes]] = []
    if path.is_file() and zipfile.is_zipfile(path):
        with zipfile.ZipFile(path) as z:
            from_zip(z, path.name)
    elif path.is_dir():                 # an unpacked export, or a folder of its zips (claude.ai sends one per kind)
        for p in sorted(path.rglob("*")):
            if p.is_file() and p.suffix.lower() == ".zip" and zipfile.is_zipfile(p):
                with zipfile.ZipFile(p) as z:
                    from_zip(z, str(p.relative_to(path)))
            elif p.is_file() and wanted(str(p.relative_to(path))):
                found.append((str(p.relative_to(path)), p.read_bytes()))
    elif path.is_file():
        found.append((path.name, path.read_bytes()))
    if not found:
        raise ImportUnreadable(f"no {' / '.join(stems)} file in {path.name}: is it the right export?")
    return found


def _json(name: str, data: bytes) -> Any:
    try:
        return json.loads(data.decode("utf-8-sig"))
    except (UnicodeDecodeError, ValueError):
        raise ImportUnreadable(f"{name} isn't JSON") from None


def _unix(value: Any) -> datetime | None:
    try:
        return datetime.fromtimestamp(float(value), tz=UTC) if value not in (None, "") else None
    except (TypeError, ValueError, OverflowError, OSError):
        return None


def _iso(value: Any) -> datetime | None:
    if not isinstance(value, str) or not value:
        return None
    try:
        at = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    return at if at.tzinfo else at.replace(tzinfo=UTC)


def _lines(path: Path) -> Iterator[str]:
    """A JSON Lines file's lines; a `.zst` one (Codex compresses rollouts) decompressed as it is read."""
    if path.suffix == ".zst":
        import io
        import zstandard
        with path.open("rb") as raw, zstandard.ZstdDecompressor().stream_reader(raw) as unzipped:
            yield from io.TextIOWrapper(unzipped, encoding="utf-8", errors="replace")
        return
    with path.open(encoding="utf-8", errors="replace") as fh:
        yield from fh


def _jsonl(path: Path) -> Iterator[dict[str, Any]]:
    try:
        for line in _lines(path):
            if line.strip():
                try:
                    record = json.loads(line)
                except ValueError:
                    continue
                if isinstance(record, dict):
                    yield record
    except OSError:
        return
    except Exception as e:                       # a damaged compressed file: what was read stays, the rest is skipped
        if type(e).__name__ != "ZstdError":
            raise
        return


# ---------- ChatGPT ----------

def _chatgpt_path(conv: dict[str, Any]) -> list[dict[str, Any]]:
    """The shown branch: from `current_node` up the parents (the export keeps every edit and regeneration)."""
    mapping = conv.get("mapping") or {}
    node_id, path, seen = conv.get("current_node"), [], set()
    while isinstance(node_id, str) and node_id in mapping and node_id not in seen:
        seen.add(node_id)
        path.append(mapping[node_id])
        node_id = mapping[node_id].get("parent")
    if path:
        return list(reversed(path))
    return sorted((n for n in mapping.values() if isinstance(n, dict)),
                  key=lambda n: float((n.get("message") or {}).get("create_time") or 0))


def _chatgpt_text(content: dict[str, Any]) -> str:
    kind = content.get("content_type")
    if kind in ("text", "multimodal_text"):
        return "\n".join(p for p in content.get("parts") or [] if isinstance(p, str))
    if kind == "code" or kind == "execution_output":
        return str(content.get("text") or "")
    return ""


def chatgpt(files: list[tuple[str, bytes]], now: datetime) -> Iterator[Conversation]:
    instructions: dict[str, Message] = {}
    # ChatGPT's saved memory isn't a file of the export: it shows in conversations as the memory snapshot
    # (`model_editable_context`) and the assistant's memory writes (recipient "bio"); the newest snapshot and every
    # write are its claims
    snapshot: Message | None = None
    writes: list[Message] = []
    for name, data in files:
        convs = _json(name, data)
        if isinstance(convs, dict) and isinstance(convs.get("conversations"), list):
            convs = convs["conversations"]
        if not isinstance(convs, list):
            raise ImportUnreadable(f"{name} isn't ChatGPT's list of conversations")
        for conv in convs:
            if not isinstance(conv, dict):
                continue
            at_conv = _unix(conv.get("update_time")) or _unix(conv.get("create_time"))
            out = Conversation("chatgpt", str(conv.get("conversation_id") or conv.get("id") or ""),
                               str(conv.get("title") or ""))
            for node in _chatgpt_path(conv):
                msg = node.get("message") or {}
                if not isinstance(msg, dict):
                    continue
                meta, content = msg.get("metadata") or {}, msg.get("content") or {}
                role = (msg.get("author") or {}).get("role")
                at = _unix(msg.get("create_time")) or at_conv
                about = meta.get("user_context_message_data") or {}
                if content.get("content_type") == "model_editable_context":
                    text = str(content.get("model_set_context") or "").strip()
                    if text and at is not None and (snapshot is None or (snapshot.at or at) < at):
                        snapshot = Message("assistant", text, at)
                    continue
                if role == "assistant" and msg.get("recipient") == "bio":
                    if at is not None and _chatgpt_text(content).strip():
                        writes.append(Message("assistant", _chatgpt_text(content), at))
                    continue
                if content.get("content_type") == "user_editable_context" or (
                        isinstance(about, dict) and (about.get("about_user_message") or about.get("about_model_message"))):
                    # custom instructions: the owner's own words
                    text = "\n".join(str(content.get(k) or "") for k in ("user_profile", "user_instructions")).strip() \
                        or "\n".join(str(about.get(k) or "") for k in ("about_user_message", "about_model_message")).strip()
                    if text and at is not None and (text not in instructions or instructions[text].at < at):  # type: ignore[operator]
                        instructions[text] = Message("owner", text, at)
                    continue
                if meta.get("is_visually_hidden_from_conversation") or role not in ("user", "assistant", "tool"):
                    continue
                text = _chatgpt_text(content)
                role_ = {"user": "owner", "assistant": "assistant", "tool": "tool"}[role]
                if role_ == "assistant" and content.get("content_type") == "execution_output":
                    role_ = "tool"
                out.messages.append(Message(role_, text, at))
            if out.messages:
                yield out
    if snapshot is not None or writes:
        yield Conversation("chatgpt", "saved-memory", "ChatGPT's saved memory",
                           ([snapshot] if snapshot else []) + sorted(writes, key=lambda m: m.at or now), claims=True)
    if instructions:
        yield Conversation("chatgpt", "custom-instructions", "ChatGPT custom instructions (about you)",
                           sorted(instructions.values(), key=lambda m: m.at or now))


# ---------- claude.ai ----------

def _claude_text(msg: dict[str, Any]) -> str:
    blocks = msg.get("content")
    if isinstance(blocks, list) and blocks:
        text = "\n".join(str(b.get("text") or "") for b in blocks if isinstance(b, dict) and b.get("type") == "text")
        if text.strip():
            return text
    return str(msg.get("text") or "")


def claude(files: list[tuple[str, bytes]], now: datetime) -> Iterator[Conversation]:
    for name, data in files:
        base = name.rsplit("/", 1)[-1].lower()
        value = _json(name, data)
        if base.startswith("conversations"):
            if not isinstance(value, list):
                raise ImportUnreadable(f"{name} isn't Claude's list of conversations")
            for conv in value:
                if not isinstance(conv, dict):
                    continue
                out = Conversation("claude", str(conv.get("uuid") or ""), str(conv.get("name") or ""))
                for msg in conv.get("chat_messages") or []:
                    if not isinstance(msg, dict):
                        continue
                    role = {"human": "owner", "assistant": "assistant"}.get(str(msg.get("sender")))
                    if role is None:
                        continue
                    text = _claude_text(msg)
                    if role == "owner" and (msg.get("attachments") or msg.get("files")):
                        text += "\n[attached file, not read]"
                    out.messages.append(Message(role, text, _iso(msg.get("created_at"))))
                if out.messages:
                    yield out
        elif "projects" in name.lower() and isinstance(value, (list, dict)):
            for project in value if isinstance(value, list) else [value]:                        # its description and instructions are the owner's words
                if not isinstance(project, dict):
                    continue
                at = _iso(project.get("updated_at")) or _iso(project.get("created_at"))
                text = "\n".join(str(project.get(k) or "") for k in ("description", "prompt_template")).strip()
                if text:
                    yield Conversation("claude", "project-" + str(project.get("uuid") or ""),
                                       "Claude project: " + str(project.get("name") or ""), [Message("owner", text, at)])
        elif "memor" in name.lower():
            # the assistant's saved memory (conversations_memory, project_memories, memory_files): its claims, dated
            # as of the export unless a memory file carries its own date
            claims: list[Message] = []
            for record in value if isinstance(value, list) else [value]:
                if not isinstance(record, dict):
                    continue
                for text in [record.get("conversations_memory"), *dict(record.get("project_memories") or {}).values()]:
                    if isinstance(text, str) and text.strip():
                        claims.append(Message("assistant", text, now))
                for f in record.get("memory_files") or []:
                    if isinstance(f, dict) and isinstance(f.get("content"), str) and f["content"].strip():
                        claims.append(Message("assistant", f["content"], _iso(f.get("updated_at")) or now))
            if claims:
                yield Conversation("claude", "memory-" + name.rsplit("/", 1)[-1], "Claude's saved memory", claims,
                                   claims=True)


# ---------- Gemini (Google Takeout, My Activity) ----------

class _ActivityHTML(HTMLParser):
    """Takeout's MyActivity.html: each `outer-cell` holds content cells; the first holds "Prompted <prompt>", the
    time, then the reply."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.cells: list[list[str]] = []      # per outer cell: its text pieces, line by line
        self._depth = 0
        self._outer: int | None = None

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag == "div":
            self._depth += 1
            if "outer-cell" in (dict(attrs).get("class") or "").split():
                self._outer = self._depth
                self.cells.append([])
        elif tag in ("br", "p") and self._outer is not None and self.cells:
            self.cells[-1].append("\n")

    def handle_endtag(self, tag: str) -> None:
        if tag == "div":
            if self._outer == self._depth:
                self._outer = None
            self._depth -= 1

    def handle_data(self, data: str) -> None:
        if self._outer is not None and self.cells:
            self.cells[-1].append(data)


_HTML_TIME = re.compile(r"([A-Z][a-z]{2,8} \d{1,2}, \d{4}, \d{1,2}:\d{2}:\d{2}\s?[AP]M)"
                        r"(?:\s+(GMT[+-]\d{1,2}(?::\d{2})?|[A-Z]{2,5}))?")


class _Text(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.parts: list[str] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag in ("br", "p", "li", "div"):
            self.parts.append("\n")

    def handle_data(self, data: str) -> None:
        self.parts.append(data)


def _strip_html(text: str) -> str:
    parser = _Text()
    parser.feed(text)
    return re.sub(r"\n\s*\n+", "\n", "".join(parser.parts)).strip()


def _prompt(title: str) -> str | None:
    """The owner's prompt of a Gemini activity: the title after Takeout's "Prompted" marker, else None (not a prompt)."""
    head, _, rest = title.strip().partition(" ")
    return rest.strip() if head == "Prompted" and rest.strip() else None


def gemini(files: list[tuple[str, bytes]], now: datetime) -> Iterator[Conversation]:
    entries: list[tuple[datetime, str, str]] = []
    for name, data in files:
        if name.lower().endswith(".json"):
            records = _json(name, data)
            if not isinstance(records, list):
                raise ImportUnreadable(f"{name} isn't Takeout's activity list")
            for r in records:
                if not isinstance(r, dict):
                    continue
                prompt, at = _prompt(str(r.get("title") or "")), _iso(r.get("time"))
                details = {str(x.get("name")): str(x.get("value") or "") for x in r.get("details") or []
                           if isinstance(x, dict)}
                if prompt is None and details.get("Request"):       # the variant with Request / Response details
                    prompt = details["Request"]
                if prompt is None or at is None:
                    continue
                reply = " ".join(_strip_html(str(h.get("html") or "")) for h in r.get("safeHtmlItem") or []
                                 if isinstance(h, dict)) or _strip_html(details.get("Response", ""))
                entries.append((at, html.unescape(prompt), reply))
        else:
            parser = _ActivityHTML()
            parser.feed(data.decode("utf-8", errors="replace"))
            for cell in parser.cells:
                text = html.unescape("".join(cell)).replace("\xa0", " ")
                m = _HTML_TIME.search(text)
                lines = [line.strip() for line in text.split("\n") if line.strip()]
                if not m or not lines:
                    continue
                prompt = next((p for line in lines if (p := _prompt(line)) is not None), None)
                if prompt is None:                      # the header cell names the product; the prompt follows
                    continue
                try:
                    at = datetime.strptime(m.group(1).replace("\u202f", " "), "%b %d, %Y, %I:%M:%S %p")
                except ValueError:
                    continue
                # UTC / GMT as written; another zone's name is ambiguous, so this Mac's zone (a day's error at most)
                at = at.replace(tzinfo=UTC) if m.group(2) in ("UTC", "GMT") else at.astimezone()
                reply = text[m.end():].strip()
                entries.append((at, prompt, reply))
    by_day: dict[str, Conversation] = {}
    for at, prompt, reply in sorted(entries):
        day = at.astimezone().date().isoformat()
        conv = by_day.setdefault(day, Conversation("gemini", "gemini-" + day, f"Gemini prompts on {day}"))
        conv.messages.append(Message("owner", prompt, at))
        if reply:
            conv.messages.append(Message("assistant", reply, at + timedelta(seconds=1)))
    yield from by_day.values()


# ---------- Claude Code ----------

def _cc_text(content: Any) -> tuple[str, bool]:
    """A turn's text, and whether it is a tool result (not the owner's)."""
    if isinstance(content, str):
        return content, False
    if isinstance(content, list):
        if any(isinstance(b, dict) and b.get("type") == "tool_result" for b in content):
            return "", True
        return "\n".join(str(b.get("text") or "") for b in content
                         if isinstance(b, dict) and b.get("type") == "text"), False
    return "", False


def claude_code(root: Path) -> Iterator[Conversation]:
    root = root.expanduser()
    if not root.is_dir():
        raise ImportUnreadable(f"no Claude Code transcripts at {root}")
    for path in sorted(root.glob("*/*.jsonl")):
        if path.stem.startswith("agent-"):               # an older build's subagent transcript beside the sessions
            continue
        out = Conversation("claude_code", path.stem, "")
        programmatic = False
        for r in _jsonl(path):
            if str(r.get("entrypoint") or "").startswith("sdk"):
                programmatic = True               # a program drove this session: its prompts aren't the owner's
                break
            kind = r.get("type")
            if kind == "ai-title" and isinstance(r.get("aiTitle"), str):
                out.title = r["aiTitle"]
            if kind not in ("user", "assistant") or r.get("isSidechain") or r.get("isMeta"):
                continue
            text, tool = _cc_text((r.get("message") or {}).get("content"))
            at = _iso(r.get("timestamp"))
            if kind == "assistant":
                out.messages.append(Message("assistant", text, at))
            elif tool:
                out.messages.append(Message("tool", "", at))
            elif not r.get("isCompactSummary") and r.get("turnOrigin", "human") == "human":
                out.messages.append(Message("owner", owner_words(text), at))
        if not programmatic and out.messages:
            yield out
    for note in sorted(root.glob("*/memory/*.md")):         # Claude Code's saved memory notes: its claims
        try:
            at = datetime.fromtimestamp(note.stat().st_mtime, tz=UTC)
            text = note.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        yield Conversation("claude_code", f"memory-{note.parent.parent.name}-{note.stem}",
                           f"Claude Code memory note {note.stem}", [Message("assistant", text, at)], claims=True)


# ---------- Codex CLI ----------

def codex(root: Path) -> Iterator[Conversation]:
    root = root.expanduser()
    def found(pattern: str) -> list[Path]:
        return sorted([*root.glob(pattern + ".jsonl"), *root.glob(pattern + ".jsonl.zst")])
    files = found("sessions/**/rollout-*") + found("archived_sessions/**/rollout-*")
    if not files and root.is_dir():
        files = found("**/rollout-*")
    if not root.is_dir():
        raise ImportUnreadable(f"no Codex sessions at {root}")
    for path in files:
        stem = path.name.removesuffix(".zst").removesuffix(".jsonl").removeprefix("rollout-")
        out, interactive = Conversation("codex", stem, ""), None
        for r in _jsonl(path):
            raw = r.get("payload")
            payload: dict[str, Any] = raw if isinstance(raw, dict) else {}
            if r.get("type") == "session_meta":
                source = payload.get("source")
                if interactive is None:
                    interactive = isinstance(source, str) and source in CODEX_INTERACTIVE
                    out.id = str(payload.get("id") or out.id)
                continue
            if r.get("type") != "response_item" or payload.get("type") != "message":
                continue
            role = payload.get("role")
            text = "\n".join(str(b.get("text") or "") for b in payload.get("content") or []
                             if isinstance(b, dict) and b.get("type") in ("input_text", "output_text"))
            at = _iso(r.get("timestamp"))
            if role == "assistant":
                out.messages.append(Message("assistant", text, at))
            elif role == "user" and not (r.get("metadata") or {}).get("inherited_user_message"):
                if text.lstrip().startswith("# AGENTS.md instructions"):     # Codex's injected project instructions
                    continue
                out.messages.append(Message("owner", owner_words(text), at))
        if interactive and out.messages:
            yield out
