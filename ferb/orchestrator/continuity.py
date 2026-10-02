"""Durable task context. Context is data; approval grants, run tokens and login codes never live here."""
import json
import re
import secrets
from typing import Any, Callable

from ferb.orchestrator.schemas import TaskSpec
from ferb.policy.targets import Request
from ferb.sanitize.untrusted import mark
from ferb.store.db import Database


class Continuity:
    def __init__(self, db: Database, redact: Callable[..., Any]) -> None:
        self.db, self.redact = db, redact

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
            row = self.db.conn.execute("SELECT conversation_id FROM current_conversation WHERE singleton = 1").fetchone()
            conversation = row[0] if row else "owner"
        self.ensure(conversation)
        self.db.conn.execute("INSERT OR REPLACE INTO current_conversation VALUES (1, ?)", (conversation,))
        if conversation == "owner":
            self._import_legacy(conversation)
        return conversation

    def pending(self, conversation: str) -> tuple[str, str]:
        self.ensure(conversation)
        row = self.db.conn.execute("SELECT pending, asked FROM conversations WHERE id = ?", (conversation,)).fetchone()
        return row[0], row[1]

    def save_pending(self, conversation: str, words: str, asked: str) -> None:
        clean = self.redact({"words": words, "asked": asked})
        self.db.conn.execute("UPDATE conversations SET pending = ?, asked = ? WHERE id = ?",
                             (clean["words"], clean["asked"], conversation))

    def save(self, task_id: str, conversation: str, spec: TaskSpec, words: str, prior_task: str | None = None) -> None:
        self.ensure(conversation)
        clean = self.redact({"spec": spec.model_dump(), "words": words})
        self.db.conn.execute("INSERT OR REPLACE INTO task_context VALUES (?, ?, ?, ?, ?)",
                             (task_id, conversation, json.dumps(clean["spec"]), clean["words"], prior_task))

    def load(self, task_id: str, conversation: str) -> tuple[TaskSpec, Request, str] | None:
        row = self.db.conn.execute(
            "SELECT c.*, t.request_id, t.status, t.outcome_json FROM task_context c JOIN tasks t ON t.id = c.task_id "
            "WHERE c.task_id = ? AND c.conversation_id = ?", (task_id, conversation)).fetchone()
        if row is None:
            return None
        spec = TaskSpec.model_validate_json(row["spec_json"])
        result = json.loads(row["outcome_json"])
        state = f"{row['status']}: {result.get('summary', '')}"
        return spec, Request(row["request_id"], row["owner_words"], spec.targets.targets()), state[:1500]

    def candidates(self, conversation: str) -> list[dict[str, Any]]:
        rows = self.db.conn.execute(
            "SELECT c.task_id, c.prior_task, t.status, t.goal, t.updated_at, t.outcome_json FROM task_context c "
            "JOIN tasks t ON t.id = c.task_id WHERE c.conversation_id = ? ORDER BY t.rowid DESC LIMIT 32",
            (conversation,)).fetchall()
        return [dict(row) for row in rows]

    def task_ids(self, conversation: str) -> set[str]:
        return {row[0] for row in self.db.conn.execute(
            "SELECT task_id FROM task_context WHERE conversation_id = ?", (conversation,))}

    def _import_legacy(self, conversation: str) -> None:
        """Recover older chat tasks from recorded owner messages and task spans; never invent missing approvals."""
        rows = self.db.conn.execute(
            "SELECT t.* FROM tasks t LEFT JOIN task_context c ON c.task_id=t.id "
            "WHERE t.parent_id IS NULL AND c.task_id IS NULL ORDER BY t.rowid DESC LIMIT 100").fetchall()
        for row in rows:
            message = self.db.conn.execute(
                "SELECT data_json FROM journal WHERE kind='chat.message' AND json_extract(data_json, '$.request_id')=? "
                "ORDER BY seq DESC LIMIT 1", (row["request_id"],)).fetchone()
            span = self.db.conn.execute("SELECT input_json FROM spans WHERE kind='task' AND task_id=? LIMIT 1",
                                        (row["id"],)).fetchone()
            if not message or not span:
                continue
            data = json.loads(span[0])
            if data.get("identity") not in {"owner", "ferb"}:
                continue
            spec = TaskSpec(goal=data.get("goal", row["goal"]), identity=data["identity"],
                            targets=data.get("targets", {}), success_conditions=[])
            text = str(json.loads(message[0]).get("text", ""))
            text = re.sub(r'<untrusted-data id="([^"]+)">.*?</untrusted-data id="\1">', " ", text, flags=re.S)
            words = mark(text).own_words
            self.save(row["id"], conversation, spec, words)
