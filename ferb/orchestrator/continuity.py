"""Owns durable conversations and explicit task references. Must not infer intent, choose a target or claim semantic success from transport facts.

Durable task context. Context is data; approval grants, run tokens and login codes never live here."""
import json
import re
import secrets
from typing import Any, Callable

from ferb.core import origin
from ferb.memory.journal import journal_origin
from ferb.orchestrator.schemas import TaskSpec
from ferb.policy.targets import Request
from ferb.sanitize.untrusted import mark, without_ferb_question
from ferb.store.db import Database

SCHEDULE_SESSION = "schedule:"   # a scheduled run's planner session (`Supervisor.run_scheduled`), not an owner chat


def in_conversation(kind: str, data: dict[str, Any], conversation: str) -> bool:
    """Whether a journaled chat entry (a message, reply, question or notice; never a task answer) belongs to a
    conversation: by its session id, a missing one being the daily "owner" conversation. A notice is posted to the
    owner, not to one chat, so it belongs to every conversation the owner chats in (the notch's, voice, `ferb chat`'s),
    never to a scheduled run's planner session (final review M3)."""
    if kind == "chat.notice" and "session_id" not in data:
        return not conversation.startswith(SCHEDULE_SESSION)
    return bool(data.get("session_id", "owner") == conversation)


class Continuity:
    def __init__(self, db: Database, redact: Callable[..., Any], task_origins: Callable[[str], set[str]]) -> None:
        self.db, self.redact = db, redact
        self.task_origins = task_origins           # SpanStore.task_origins: what a task's models were shown (S3)

    def ensure(self, conversation: str) -> None:
        if not re.fullmatch(r"[\w:.-]{1,128}", conversation):
            raise ValueError("invalid conversation id")
        self.db.conn.execute("INSERT OR IGNORE INTO conversations(id) VALUES (?)", (conversation,))

    def attach(self, new: bool = False, resume: str | None = None) -> str:
        if new and resume:
            raise ValueError("choose --new or --resume")
        if resume:
            if not self.db.conn.execute("SELECT 1 FROM conversations WHERE id = ?", (resume,)).fetchone():
                raise ValueError("that conversation does not exist")
            conversation = resume
        elif new:
            conversation = "chat-" + secrets.token_hex(6)
        else:
            conversation = self.current()
        self.ensure(conversation)
        self.db.conn.execute("INSERT OR REPLACE INTO current_conversation VALUES (1, ?)", (conversation,))
        if conversation == "owner":
            self._import_legacy(conversation)
        return conversation

    def current(self) -> str:
        """The conversation `ferb chat` last attached to; the owner's daily conversation until another is chosen."""
        row = self.db.conn.execute("SELECT conversation_id FROM current_conversation WHERE singleton = 1").fetchone()
        return str(row[0]) if row else "owner"

    def pending(self, conversation: str) -> tuple[str, str]:
        self.ensure(conversation)
        row = self.db.conn.execute("SELECT pending, asked FROM conversations WHERE id = ?", (conversation,)).fetchone()
        return row[0], row[1]

    def pending_payload(self, conversation: str) -> str:
        row = self.db.conn.execute("SELECT pending_payload FROM conversations WHERE id = ?", (conversation,)).fetchone()
        return row[0] if row else ""

    def save_pending(self, conversation: str, words: str, asked: str, payload: str = "") -> None:
        clean = self.redact({"words": words, "asked": asked, "payload": payload})
        self.db.conn.execute("UPDATE conversations SET pending = ?, asked = ?, pending_payload = ? WHERE id = ?",
                             (clean["words"], clean["asked"], clean["payload"], conversation))

    def save(self, task_id: str, conversation: str, spec: TaskSpec, words: str, prior_task: str | None = None,
             payload: str = "") -> None:
        self.ensure(conversation)
        clean = self.redact({"spec": spec.model_dump(), "words": words, "payload": payload})
        self.db.conn.execute("INSERT OR REPLACE INTO task_context "
                             "(task_id,conversation_id,spec_json,owner_words,prior_task,request_payload) VALUES (?, ?, ?, ?, ?, ?)",
                             (task_id, conversation, json.dumps(clean["spec"]), clean["words"], prior_task, clean["payload"]))

    def load(self, task_id: str, conversation: str) -> tuple[TaskSpec, Request, str] | None:
        row = self.db.conn.execute(
            "SELECT c.*, t.request_id, t.status, t.outcome_json FROM task_context c JOIN tasks t ON t.id = c.task_id "
            "WHERE c.task_id = ? AND c.conversation_id = ?", (task_id, conversation)).fetchone()
        if row is None:
            return None
        spec = TaskSpec.model_validate_json(row["spec_json"])
        result = json.loads(row["outcome_json"])
        state = f"{row['status']}: {result.get('summary', '')}"
        words = without_ferb_question(row["owner_words"])
        if not row["request_payload"]:          # written before the full request was recorded (0012): maybe older
            words = self._without_endorsed_question(words)
        return spec, Request(row["request_id"], words, spec.targets.targets(),
                             payload=row["request_payload"] or row["owner_words"]), state

    def _without_endorsed_question(self, words: str) -> str:
        """Before 4e0976a (2026-10-03), a plain yes to FERB's question appended that question, unlabeled and with
        "your" read as "my", to the words recorded as the owner's. Removed only in exactly that form: FERB's own
        recorded question (a chat.clarify row), transformed as that code did, on the lines right after a line that
        code read as a yes, ending the text or a line. Anything else stays: nothing is stripped by a guess."""
        rows = self.db.conn.execute("SELECT data_json FROM journal WHERE kind = 'chat.clarify'")
        questions = {str(json.loads(r[0]).get("text") or "") for r in rows}
        for question in sorted(filter(None, questions), key=len, reverse=True):
            words = _strip_endorsed(words, "\n" + re.sub(r"\byour\b", "my", question, flags=re.I))
        return words

    def prior_of(self, task_id: str) -> str | None:
        """The earlier task this task continued, as recorded (None when it continued none)."""
        row = self.db.conn.execute("SELECT prior_task FROM task_context WHERE task_id = ?", (task_id,)).fetchone()
        return row[0] if row else None

    def candidates(self, conversation: str) -> list[dict[str, Any]]:
        rows = self.db.conn.execute(
            "SELECT c.task_id, c.prior_task, t.status, t.goal, t.updated_at, t.outcome_json FROM task_context c "
            "JOIN tasks t ON t.id = c.task_id WHERE c.conversation_id = ? ORDER BY t.rowid DESC",
            (conversation,)).fetchall()
        return [dict(row) for row in rows]

    def conversation_of(self, task_id: str) -> str | None:
        """The chat a registered task belongs to; None when it is unknown."""
        row = self.db.conn.execute("SELECT conversation_id FROM task_context WHERE task_id = ?", (task_id,)).fetchone()
        return str(row[0]) if row is not None else None

    def task_ids(self, conversation: str) -> set[str]:
        return {row[0] for row in self.db.conn.execute(
            "SELECT task_id FROM task_context WHERE conversation_id = ?", (conversation,))}

    def history(self, task_id: str, *, include_tasks: bool = False) -> list[dict[str, Any]]:
        """Complete data for the registered task's conversation; never pick a conversation from words."""
        found = self.db.conn.execute("SELECT conversation_id FROM task_context WHERE task_id = ?", (task_id,)).fetchone()
        if found is None:
            return []
        return self.conversation_history(str(found[0]), include_tasks=include_tasks)

    def conversation_history(self, conversation: str, *, include_tasks: bool = False) -> list[dict[str, Any]]:
        """Host-selected conversation; callers must never accept this ID from a model."""
        tasks = self.task_ids(conversation)
        records = []
        for row in self.db.conn.execute("SELECT * FROM journal ORDER BY seq"):
            data = json.loads(row["data_json"])
            belongs = (data.get("task_id") in tasks if row["kind"] == "task.answer" else
                       in_conversation(row["kind"], data, conversation))
            if belongs and row["kind"] in ("chat.message", "chat.reply", "chat.clarify", "task.answer", "chat.notice"):
                records.append({"id": row["id"], "at": row["at"], "kind": row["kind"], "data": data,
                                "origin": row["origin"] or journal_origin(row["kind"], data)})
        if include_tasks:
            for row in self.candidates(conversation):
                made = origin.join({origin.INFERRED} | self.task_origins(row["task_id"]))   # model-written (MP2)
                records.append({"id": row["task_id"], "kind": "task", "data": row, "origin": made})
        return records

    def _import_legacy(self, conversation: str) -> None:
        """Recover older chat tasks from recorded owner messages and task spans; never invent missing approvals."""
        rows = self.db.conn.execute(
            "SELECT t.* FROM tasks t LEFT JOIN task_context c ON c.task_id=t.id "
            "WHERE t.parent_id IS NULL AND c.task_id IS NULL ORDER BY t.rowid DESC").fetchall()
        for row in rows:
            messages = self.db.conn.execute(
                "SELECT data_json FROM journal WHERE kind='chat.message' AND json_extract(data_json, '$.request_id')=?",
                (row["request_id"],)).fetchall()
            spans = self.db.conn.execute("SELECT input_json FROM spans WHERE kind='task' AND task_id=?",
                                         (row["id"],)).fetchall()
            if not messages or not spans:
                continue
            if len({message[0] for message in messages}) != 1 or len({span[0] for span in spans}) != 1:
                raise ValueError(f"Conflicting legacy request sources for task {row['id']}; inspect the trace before resuming")
            # Identical duplicate records are mechanical duplicates, not alternative request candidates.
            message, span = messages[0], spans[0]
            data = json.loads(span[0])
            if data.get("identity") not in {"owner", "ferb"}:
                continue
            spec = TaskSpec(goal=data.get("goal", row["goal"]), identity=data["identity"],
                            targets=data.get("targets", {}), success_conditions=[])
            text = str(json.loads(message[0]).get("text", ""))
            text = re.sub(r'<untrusted-data id="([^"]+)">.*?</untrusted-data id="\1">', " ", text, flags=re.S)
            words = mark(text).own_words
            self.save(row["id"], conversation, spec, words, payload=str(json.loads(message[0]).get("text", "")))


# The old code's plain-yes check, verbatim (supervisor._affirmative before 4e0976a), to recognise what it wrote.
_OLD_YES = re.compile(r"\s*(yes|y|yeah|yep|yup|sure|ok|okay|go ahead|do it|please do|correct|right)[.! ]*\s*")


def _strip_endorsed(words: str, needle: str) -> str:
    """`words` without each `needle` (newline + endorsed question) that follows a plain-yes line and ends the text or
    a line."""
    at = words.find(needle)
    while at != -1:
        line = words[words.rfind("\n", 0, at) + 1:at]
        end = at + len(needle)
        if _OLD_YES.fullmatch(line.lower()) and (end == len(words) or words[end] == "\n"):
            words = words[:at] + words[end:]
            at = words.find(needle, at)
        else:
            at = words.find(needle, at + 1)
    return words
