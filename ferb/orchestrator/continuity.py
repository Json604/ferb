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
PREVIEW_SESSION = "preview:"     # the intro's plan-only preview (`Supervisor.preview`), not an owner chat
# FERB's own planner sessions: `chat.send` refuses them, so no client message lands in one
RESERVED_SESSIONS = (SCHEDULE_SESSION, PREVIEW_SESSION)


def in_conversation(kind: str, data: dict[str, Any], conversation: str) -> bool:
    """Whether a journaled chat entry (a message, reply, question or notice; never a task answer) belongs to a
    conversation: by its session id, a missing one being the daily "owner" conversation. A notice is posted to the
    owner, not to one chat, so it belongs to every conversation the owner chats in (the notch's, voice, `ferb chat`'s),
    never to one of FERB's own planner sessions (a scheduled run's, final review M3; the intro's preview)."""
    if kind == "chat.notice" and "session_id" not in data:
        return not conversation.startswith(RESERVED_SESSIONS)
    return bool(data.get("session_id", "owner") == conversation)


class Continuity:
    def __init__(self, db: Database, redact: Callable[..., Any], task_origins: Callable[[str], set[str]]) -> None:
        self.db, self.redact = db, redact
        self.task_origins = task_origins           # SpanStore.task_origins: what a task's models were shown (S3)
        self._lent: dict[str, str] = {}            # a planner turn's lookup record -> its chat (`lend`)

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
        return spec, Request(row["request_id"], words, spec.targets.targets(),
                             payload=row["request_payload"] or row["owner_words"]), state

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
        """The chat a registered task belongs to (or a planner turn's, while it looks things up: `lend`); None when it
        is unknown."""
        if task_id in self._lent:
            return self._lent[task_id]
        row = self.db.conn.execute("SELECT conversation_id FROM task_context WHERE task_id = ?", (task_id,)).fetchone()
        return str(row[0]) if row is not None else None

    def lend(self, task_id: str, conversation: str) -> None:
        """The planner's turn reads its own chat while it runs (ORCH-516): its lookup record `task_id` belongs to
        `conversation` until `unlend`. Never stored: no task row exists for it."""
        self._lent[task_id] = conversation

    def unlend(self, task_id: str) -> None:
        self._lent.pop(task_id, None)

    def task_ids(self, conversation: str) -> set[str]:
        return {row[0] for row in self.db.conn.execute(
            "SELECT task_id FROM task_context WHERE conversation_id = ?", (conversation,))}

    def reaches_schedules(self, task_id: str) -> bool:
        """ORCH-593: whether a lookup by this record also reads the owner's scheduled runs (their `schedule:` sessions):
        only the owner's planner turn in one of the owner's chats (its lookup record is lent the chat, `lend`, and is
        always the owner's, Supervisor._lookup_grant); a worker's or a scheduled run's own lookups read as before."""
        conversation = self._lent.get(task_id)
        return conversation is not None and not conversation.startswith(RESERVED_SESSIONS)

    def readable_tasks(self, task_id: str) -> set[str]:
        """The task ids a record's task.evidence task_id may read (ORCH-517, ORCH-593): its conversation's tasks, and the
        owner's scheduled runs' for the owner's planner turn."""
        conversation = self.conversation_of(task_id)
        if conversation is None:
            return set()
        return self.task_ids(conversation) | (self._schedule_tasks() if self.reaches_schedules(task_id) else set())

    # The conversation kinds chat.history reads (supervisor.CONVERSATION_KINDS), and what FERB posted in the owner's
    # chat by itself (supervisor.SELF_POSTS, ORCH-588): a scheduled run's result and a reminder belong to every owner
    # chat, a part posted as it landed to its task's chat
    KINDS = ("chat.message", "chat.reply", "chat.clarify", "task.answer", "chat.notice")
    SELF_POSTS = ("schedule.posted", "timer.delivered", "chat.part")

    def _where(self, conversation: str, schedules: bool = False) -> tuple[str, list[Any]]:
        """SQL for the journal rows that belong to a conversation (`in_conversation`, plus its tasks' answers and, for an
        owner chat, FERB's own posts there, ORCH-593). `schedules`: the owner's planner turn also reads the owner's
        scheduled runs, their planner replies and their tasks' answers (`reaches_schedules`)."""
        owner_chat = not conversation.startswith(RESERVED_SESSIONS)
        kinds = self.KINDS + (self.SELF_POSTS if owner_chat else ())
        marks = ",".join("?" * len(kinds))
        ran = " OR conversation_id LIKE 'schedule:%'" if schedules else ""
        tasks = f"(SELECT task_id FROM task_context WHERE conversation_id = ?{ran})"
        session = ("json_extract(data_json, '$.session_id') = ?" +
                   (" OR json_extract(data_json, '$.session_id') LIKE 'schedule:%'" if schedules else ""))
        posts = ("" if not owner_chat else
                 " OR (kind = 'chat.notice' AND json_extract(data_json, '$.session_id') IS NULL)"
                 " OR kind IN ('schedule.posted', 'timer.delivered')"
                 f" OR (kind = 'chat.part' AND json_extract(data_json, '$.task_id') IN {tasks})")
        args: list[Any] = [*kinds, conversation, *([conversation] if owner_chat else []), conversation, conversation]
        own = "'task.answer', 'schedule.posted', 'timer.delivered', 'chat.part'"    # rows placed by the lines above
        return (f"kind IN ({marks}) AND ((kind = 'task.answer' AND json_extract(data_json, '$.task_id') IN "
                f"{tasks}){posts} OR (kind NOT IN ({own}) AND json_extract(data_json, '$.session_id') IS NOT NULL "
                f"AND ({session})) OR (kind NOT IN ({own}, 'chat.notice') "
                f"AND json_extract(data_json, '$.session_id') IS NULL AND ? = 'owner'))", args)

    @staticmethod
    def _record(row: Any) -> dict[str, Any]:
        data = json.loads(row["data_json"])
        return {"id": row["id"], "at": row["at"], "kind": row["kind"], "data": data,
                "origin": row["origin"] or journal_origin(row["kind"], data)}

    def entries(self, task_id: str, *, since: str | None = None, until: str | None = None, after: int = 0,
                limit: int = 20) -> tuple[list[dict[str, Any]], int]:
        """One page of the registered task's conversation, oldest first, read from the database (never the whole
        journal), and how many entries it has in all (within `since`/`until`, ISO bounds on the time). Never picks a
        conversation from words."""
        conversation = self.conversation_of(task_id)
        if conversation is None:
            return [], 0
        where, args = self._where(conversation, self.reaches_schedules(task_id))
        if since:
            where, args = where + " AND at >= ?", args + [since]
        if until:
            where, args = where + " AND at <= ?", args + [until]
        total = int(self.db.conn.execute(f"SELECT COUNT(*) FROM journal WHERE {where}", args).fetchone()[0])
        rows = self.db.conn.execute(f"SELECT * FROM journal WHERE {where} ORDER BY seq LIMIT ? OFFSET ?",
                                    [*args, limit, after]).fetchall()
        return [self._record(r) for r in rows], total

    def entry(self, task_id: str, entry_id: str) -> dict[str, Any] | None:
        """One entry of the registered task's conversation by its id, or None when it isn't one of its entries."""
        conversation = self.conversation_of(task_id)
        if conversation is None:
            return None
        where, args = self._where(conversation, self.reaches_schedules(task_id))
        row =self.db.conn.execute(f"SELECT * FROM journal WHERE id = ? AND {where}", [entry_id, *args]).fetchone()
        return self._record(row) if row is not None else None

    def result(self, conversation: str, entry_id: str, kinds: tuple[str, ...]) -> bool:
        """ORCH-619: whether `entry_id` is a result (a row of `kinds`) shown in this conversation, read as the owner's
        planner turn reads it there (ORCH-593: an owner chat also reaches the owner's scheduled runs and FERB's own
        posts; FERB's own planner sessions only their own tasks)."""
        where, args = self._where(conversation, not conversation.startswith(RESERVED_SESSIONS))
        marks = ",".join("?" * len(kinds))
        return self.db.conn.execute(f"SELECT 1 FROM journal WHERE id = ? AND kind IN ({marks}) AND {where}",
                                    [entry_id, *kinds, *args]).fetchone() is not None

    def part_order(self, task_id: str) -> list[str]:
        """The ids of a registered task's parts in the owner's order (its saved spec); [] when it has none."""
        row = self.db.conn.execute("SELECT spec_json FROM task_context WHERE task_id = ?", (task_id,)).fetchone()
        return [p.id for p in TaskSpec.model_validate_json(row[0]).parts] if row is not None else []

    def belongs(self, conversation: str, kind: str, session: str, request: str, doc: str = "",
                schedules: bool = False) -> bool:
        """Whether an indexed conversation entry (its kind, chat and request or task id, its `journal:` doc) is in this
        conversation. A task's answer belongs by its task, as `_where` reads it (ORCH-584): the index keeps its request
        id, so the task id is read from its journal row. `schedules` (ORCH-593): the owner's scheduled runs count too."""
        if kind == "task.answer":
            row = self.db.conn.execute("SELECT json_extract(data_json, '$.task_id') FROM journal WHERE id = ?",
                                       (doc.removeprefix("journal:"),)).fetchone() if doc else None
            held = self.task_ids(conversation) | (self._schedule_tasks() if schedules else set())
            return bool({request, row[0] if row else None} & held)
        if kind == "chat.notice" and not session:
            return not conversation.startswith(RESERVED_SESSIONS)
        return session == conversation or (schedules and session.startswith(SCHEDULE_SESSION))

    def _schedule_tasks(self) -> set[str]:
        return {row[0] for row in self.db.conn.execute(
            "SELECT task_id FROM task_context WHERE conversation_id LIKE ?", (SCHEDULE_SESSION + "%",))}

    def tasks(self, task_id: str) -> list[dict[str, Any]]:
        """The tasks of the registered task's conversation, as chat.history lists them; for the owner's planner turn,
        the owner's scheduled runs too (ORCH-593), newest first after the chat's own."""
        conversation = self.conversation_of(task_id)
        if conversation is None:
            return []
        rows = self.candidates(conversation)
        if self.reaches_schedules(task_id):
            rows += [dict(row) for row in self.db.conn.execute(
                "SELECT c.task_id, c.prior_task, t.status, t.goal, t.updated_at, t.outcome_json FROM task_context c "
                "JOIN tasks t ON t.id = c.task_id WHERE c.conversation_id LIKE ? ORDER BY t.rowid DESC",
                (SCHEDULE_SESSION + "%",)).fetchall()]
        out = []
        for row in rows:
            made = origin.join({origin.INFERRED} | origin.kinds(self.task_origins(row["task_id"])))   # MP2, GATE-519
            out.append({"id": row["task_id"], "kind": "task", "data": row, "origin": made})
        return out


LEGACY_UPGRADE = "continuity_legacy"     # the one-time upgrade below, journaled as store.upgraded once it has run


def upgrade_legacy(db: Database, redact: Callable[..., Any]) -> dict[str, int]:
    """One-time upgrade of chat continuity written by builds before 2026-10-03 (step 8c removed the shims that read it
    on every load and attach): (1) a task's recorded owner words from before the full request was recorded
    (migration 0012, empty `request_payload`) lose FERB's question that old code appended after a plain yes, in exactly
    that form, and keep the words as recorded as the request payload; (2) root tasks recorded before task context
    existed are registered in the owner's daily conversation from their recorded message and task span (never
    inventing an approval; conflicting records are left out). Counts of what changed."""
    continuity = Continuity(db, redact, lambda _task: set())
    questions = {str(json.loads(r[0]).get("text") or "")
                 for r in db.conn.execute("SELECT data_json FROM journal WHERE kind = 'chat.clarify'")}
    rewritten = 0
    for row in db.conn.execute("SELECT task_id, owner_words FROM task_context WHERE COALESCE(request_payload, '') = ''"
                               ).fetchall():
        words = str(row["owner_words"] or "")
        for question in sorted(filter(None, questions), key=len, reverse=True):
            words = _strip_endorsed(words, "\n" + re.sub(r"\byour\b", "my", question, flags=re.I))
        db.conn.execute("UPDATE task_context SET owner_words = ?, request_payload = ? WHERE task_id = ?",
                        (words, str(row["owner_words"] or ""), row["task_id"]))
        rewritten += 1
    continuity.ensure("owner")
    imported = skipped = 0
    rows = db.conn.execute("SELECT t.* FROM tasks t LEFT JOIN task_context c ON c.task_id=t.id "
                           "WHERE t.parent_id IS NULL AND c.task_id IS NULL ORDER BY t.rowid DESC").fetchall()
    for row in rows:
        messages = db.conn.execute(
            "SELECT data_json FROM journal WHERE kind='chat.message' AND json_extract(data_json, '$.request_id')=?",
            (row["request_id"],)).fetchall()
        spans = db.conn.execute("SELECT input_json FROM spans WHERE kind='task' AND task_id=?", (row["id"],)).fetchall()
        if not messages or not spans:
            continue
        if len({message[0] for message in messages}) != 1 or len({span[0] for span in spans}) != 1:
            skipped += 1                    # conflicting records: never guessed between
            continue
        data = json.loads(spans[0][0])
        if data.get("identity") not in {"owner", "ferb"}:
            continue
        spec = TaskSpec(goal=data.get("goal", row["goal"]), identity=data["identity"],
                        targets=data.get("targets", {}), success_conditions=[])
        text = str(json.loads(messages[0][0]).get("text", ""))
        own = re.sub(r'<untrusted-data id="([^"]+)">.*?</untrusted-data id="\1">', " ", text, flags=re.S)
        continuity.save(row["id"], "owner", spec, mark(own).own_words, payload=text)
        imported += 1
    return {"rewritten": rewritten, "imported": imported, "conflicting": skipped}


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
