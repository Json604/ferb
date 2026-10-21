"""What FERB's own records say of a forgotten item goes with it (memory review I2, fix round Q1/Q2).

An explicit forget deletes the item, its versions and its sources (items.py). FERB's own records can still state it:
the code line "Remembered: …", FERB's answer to the forgotten message, a reply or a task answer written from the item,
the task's stored goal, outcome, saved spec and permanent record, its episode card, and the planner's and workers'
stored turns and exposures that copied those. After the deletion (`scrub`, on a connection of its own when the daemon
runs it):
- FERB's whole answer to a forgotten chat message is removed, in the chat and in the planner turn that wrote it (it
  answered words that are gone);
- a task given the item (`task_memory_refs`: its exposures or memory reads cited the item, or a node built on it that
  holds the item's text or a rare word of it; or its card cited it; or its exposures cited a forgotten entry) loses each line, in its answer and outcome rows, its stored goal, outcome,
  saved spec and permanent record, that holds the item's text word for word or a word of the item nothing left holds
  (no remaining owner message, memory item or FERB row not given the item: data-driven, no word list); its episode card
  is deleted, and never rebuilt, when its body does (`Scrubbed.cards`; no partial card);
- a chat reply whose turn was given the item (its recorded source refs cite it) loses a line that holds the item's
  text word for word, or two of its words in a row of which one nothing left holds: one shared word is not a mention;
- any other FERB row loses a line holding the item's or a forgotten message's text word for word;
- the given tasks' spans lose those lines; every span loses the item's text and each removed line of two words or
  more, matched as whole words, case and spacing aside (`scrub_told`; never a plain substring, never a single word).
A forgotten learned item has its own, scoped pass (`finish_node`).
Every row is written only while it still holds what was read (`swap`, spans through `rewrite_recorded`), and each
span pass runs again while a span running when it started took a live write meanwhile (`rewrite_until_still`): a span
finishing during the scrub keeps its output and loses the item; one recorded after the scrub is not reached (fix wave
6a). Journal rows are append-only (they are only ever deleted, for a forget): a changed row is deleted and written again
with the same sequence number, id, time, kind and origin. The owner's own messages are never rewritten."""
import json
import re
import unicodedata
from dataclasses import dataclass, field
from datetime import datetime, timezone
from functools import partial
from typing import Any, Callable

from ferb.memory.items import contains_words, query_terms
from ferb.store.db import swap
from ferb.store.spans import SpanWatch, rewrite_recorded, rewrite_until_still

REMOVED = "[removed: it held something the owner asked FERB to forget]"
CHAT_KINDS = ("chat.reply", "chat.clarify")
TASK_KINDS = ("task.answer", "task.outcome")
FERB_KINDS = CHAT_KINDS + TASK_KINDS
OWNER_KINDS = ("chat.message", "memory.added", "memory.edited")
FIELDS = ("text", "display_text", "summary", "answer")
IDS = ("task_id", "request_id", "session_id", "status")
EPISODE_FIELDS = ("answer", "goal", "conditions_json", "steps_json", "problems_json")


@dataclass
class Forgotten:
    """What a forget deletes, read before it does: the items' texts, the forgotten entries' texts, the requests of the
    forgotten chat messages, every ref that names something forgotten, and the tasks that were given it."""
    contents: list[str] = field(default_factory=list)
    said: list[str] = field(default_factory=list)
    requests: set[str] = field(default_factory=set)
    refs: set[str] = field(default_factory=set)
    tasks: set[str] = field(default_factory=set)
    # each candidate task -> the refs it was shown or cited; the texts of the learned nodes among them, read before the
    # forget deletes them: a task was given the item only through the item itself or a node whose body restates it
    through: dict[str, set[str]] = field(default_factory=dict)
    node_texts: dict[str, list[str]] = field(default_factory=dict)

    @property
    def phrases(self) -> list[str]:
        return [t for t in [*self.contents, *self.said] if t.strip()]


@dataclass
class Scrubbed:
    rewritten: list[str] = field(default_factory=list)   # journal rows rewritten
    cards: list[str] = field(default_factory=list)       # episode cards whose body states the item: to forget


def _root(task_id: str) -> str:
    return task_id.split(".", 1)[0]


def _with_refs(db: Any, refs: set[str], sql: str, params: tuple[Any, ...] = ()) -> list[Any]:
    """Run `sql` with the temporary table `forgetting_refs` holding `refs`."""
    db.conn.execute("CREATE TEMP TABLE IF NOT EXISTS forgetting_refs (ref TEXT PRIMARY KEY)")
    try:
        db.conn.executemany("INSERT OR IGNORE INTO forgetting_refs VALUES (?)", [(r,) for r in refs])
        return list(db.conn.execute(sql, params).fetchall())
    finally:
        db.conn.execute("DELETE FROM forgetting_refs")


def capture(db: Any, items: list[str], journal: list[str], trail: list[str]) -> Forgotten:
    """Read before the deletion: the items' contents (every version), the forgotten entries' texts and requests, the
    refs naming them with the learning nodes built on them (a planner turn cites a fact by its node), and the tasks
    given any of them."""
    out = Forgotten()
    for ident in items:
        row = db.conn.execute("SELECT content FROM memory_items WHERE id=?", (ident,)).fetchone()
        if row is not None:
            out.contents.append(str(row[0]))
    for ident in journal:
        row = db.conn.execute("SELECT kind, data_json FROM journal WHERE id=?", (ident,)).fetchone()
        if row is None:
            continue
        data = json.loads(row["data_json"])
        out.said.append(str(data.get("text") or data.get("content") or ""))
        if row["kind"] == "chat.message" and data.get("request_id"):
            out.requests.add(str(data["request_id"]))
    entries = {"journal:" + j for j in journal} | {"entry:" + j for j in journal}
    out.refs = {"memory:" + i for i in items} | entries | {"trail:" + t for t in trail}
    frontier = set(out.refs)
    while frontier:                                   # the nodes built on them, at any depth
        found = {r[0] for ref in frontier for r in db.conn.execute(
            "SELECT node_id FROM learning_sources WHERE source_ref=?", (ref,))} - out.refs
        out.refs |= found
        frontier = found
    if out.refs:
        given = _with_refs(db, out.refs, "SELECT DISTINCT t.task_id, t.ref FROM task_memory_refs t JOIN forgetting_refs f "
                                         "ON t.ref = f.ref")
        cited = _with_refs(db, out.refs, "SELECT DISTINCT n.key, s.source_ref FROM learning_nodes n JOIN learning_sources s "
                                         "ON s.node_id = n.id JOIN forgetting_refs f ON s.source_ref = f.ref "
                                         "WHERE n.kind = 'episode'")
        shown = _with_refs(db, entries, "SELECT DISTINCT s.task_id, r.value FROM spans s, json_each(s.input_json, "
                                        "'$.source_refs') r JOIN forgetting_refs f ON r.value = f.ref WHERE "
                                        "s.kind = 'context.exposure' AND s.task_id IS NOT NULL") if entries else []
        for task, ref in [*given, *cited, *shown]:
            if task:
                out.through.setdefault(_root(str(task)), set()).add(str(ref))
        for ref in {r for refs in out.through.values() for r in refs if not _direct(r)}:
            node = db.conn.execute("SELECT key, body_json FROM learning_nodes WHERE id=?", (ref,)).fetchone()
            if node is not None:
                out.node_texts[ref] = [str(node["key"]), *_strings(json.loads(node["body_json"]))]
        out.tasks = set(out.through)                  # narrowed once the item's rare words are known (`settle`)
    return out


def _direct(ref: str) -> bool:
    """A ref naming the forgotten thing itself (the item, its source entries or trail rows), not a node built on it."""
    return ref.startswith(("memory:", "journal:", "entry:", "trail:"))


def given(db: Any, forgotten: Forgotten) -> set[str]:
    """FERB's conversation rows written with the item: chat rows whose turn was given it (their recorded source refs
    cite it), and the answer and outcome rows of the tasks given it."""
    rows: set[str] = set()
    if forgotten.refs:
        rows |= {r[0] for r in _with_refs(
            db, forgotten.refs,
            f"SELECT DISTINCT j.id FROM journal j, json_each(j.data_json, '$.source_refs') r JOIN forgetting_refs f "
            f"ON r.value = f.ref WHERE j.kind IN ({','.join('?' * len(CHAT_KINDS))}) "
            "AND json_type(j.data_json, '$.source_refs') = 'array'", CHAT_KINDS)}
    if forgotten.tasks:
        rows |= {r[0] for r in db.conn.execute(f"SELECT id, data_json FROM journal WHERE kind IN "
                                                f"({','.join('?' * len(TASK_KINDS))})", TASK_KINDS)
                 if _root(str(json.loads(r[1]).get("task_id", ""))) in forgotten.tasks}
    return rows


def stating(db: Any, forgotten: Forgotten) -> set[str]:
    """FERB's conversation rows that hold an item's or a forgotten message's text word for word."""
    phrases = forgotten.phrases
    if not phrases:
        return set()
    marks = ','.join('?' * len(FERB_KINDS))
    return {row[0] for row in db.conn.execute(f"SELECT id, data_json FROM journal WHERE kind IN ({marks})", FERB_KINDS)
            if any(contains_words(text, p) for text in _texts(json.loads(row[1])) for p in phrases)}


def _texts(data: dict[str, Any]) -> list[str]:
    return [v for k, v in data.items() if k in FIELDS and isinstance(v, str)]


def _strings(value: Any) -> list[str]:
    if isinstance(value, str):
        return [value]
    if isinstance(value, dict):
        return [s for v in value.values() for s in _strings(v)]
    if isinstance(value, list):
        return [s for v in value for s in _strings(v)]
    return []


class _Lines:
    """Line rules over one forget's texts, and the lines they removed."""

    def __init__(self, phrases: list[str], contents: list[str], rare: set[str]) -> None:
        self.phrases, self.rare = phrases, rare
        self.pairs = {pair for c in contents for pair in zip(query_terms(c), query_terms(c)[1:])}
        self.removed: list[str] = []

    def verbatim(self, line: str) -> bool:
        return any(contains_words(line, p) for p in self.phrases)

    def mentions(self, line: str) -> bool:
        """The item's text word for word, or a word of it nothing left holds (rows of a task given the item)."""
        return self.verbatim(line) or bool(self.rare & set(query_terms(line)))

    def restates(self, line: str) -> bool:
        """The item's text word for word, or two of its words in a row of which one nothing left holds (a chat reply
        given the item among others: one shared word is not a mention)."""
        words = query_terms(line)
        return self.verbatim(line) or any(pair in self.pairs and (pair[0] in self.rare or pair[1] in self.rare)
                                          for pair in zip(words, words[1:]))

    def text(self, value: str, rule: Callable[[str], bool]) -> str:
        lines = value.split("\n")
        kept = [REMOVED if line.strip() and line != REMOVED and rule(line) else line for line in lines]
        self.removed += [line for line, now in zip(lines, kept) if now is REMOVED and line is not now]
        return "\n".join(kept)

    def value(self, value: Any, rule: Callable[[str], bool]) -> Any:
        if isinstance(value, str):
            return self.text(value, rule)
        if isinstance(value, dict):
            return {k: self.value(v, rule) for k, v in value.items()}
        if isinstance(value, list):
            return [self.value(v, rule) for v in value]
        return value


def _rare(db: Any, forgotten: Forgotten, given_rows: set[str], ferb: list[Any]) -> set[str]:
    """The item's words nothing left holds: no remaining owner text or memory item, no FERB row not given the item."""
    words = {w for c in forgotten.contents for w in query_terms(c)}
    others = [str(r[0]) for r in db.conn.execute("SELECT content FROM memory_items")]
    for row in db.conn.execute(f"SELECT data_json FROM journal WHERE kind IN ({','.join('?' * len(OWNER_KINDS))})",
                               OWNER_KINDS):
        data = json.loads(row[0])
        others.append(str(data.get("text") or data.get("content") or ""))
    others += [t for r in ferb if r["id"] not in given_rows for t in _texts(json.loads(r["data_json"]))]
    for text in others:
        if not words:
            break
        words -= set(query_terms(text))
    return words


SUMMARY_KIND = "chat.summary"


def finish(db: Any, forgotten: Forgotten) -> Scrubbed:
    """A forget's work after its deletion: `scrub`, then the running summaries that folded a row written with the item
    or rewritten here (or a summary built on one: they are rebuilt from what remains), then an emptied WAL."""
    lines = settle(db, forgotten)
    written = given(db, forgotten) | stating(db, forgotten)
    out = scrub(db, forgotten, lines)
    _drop_folded(db, written | set(out.rewritten))
    db.conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    return out


def _drop_folded(db: Any, rows: set[str]) -> None:
    """The running summaries that folded one of these rows go, and those built on them (rebuilt from what remains)."""
    folded = {"entry:" + ident for ident in rows}
    while folded:
        doomed = {row[0] for row in db.conn.execute("SELECT id, data_json FROM journal WHERE kind = ?", (SUMMARY_KIND,))
                  if folded & set(json.loads(row[1]).get("source_refs", []))}
        for ident in doomed:
            db.conn.execute("DELETE FROM journal WHERE id = ? AND kind = ?", (ident, SUMMARY_KIND))
        folded = {"entry:" + ident for ident in doomed}


def finish_at(path: Any, forgotten: Forgotten) -> Scrubbed:
    """`finish` on a connection of its own (the daemon runs it in a worker thread, off the event loop)."""
    from ferb.store.db import Database
    db = Database(path)
    try:
        return finish(db, forgotten)
    finally:
        db.close()


def _ferb_rows(db: Any) -> list[Any]:
    marks = ','.join('?' * len(FERB_KINDS))
    return list(db.conn.execute(f"SELECT seq, id, at, kind, data_json, origin FROM journal WHERE kind IN ({marks}) "
                                "ORDER BY seq", FERB_KINDS).fetchall())


def settle(db: Any, forgotten: Forgotten) -> "_Lines":
    """The line rules for this forget, and the tasks it reaches. A candidate task was given the item when it was
    shown or cited the item itself (its text, source entries or rows) or a learned node whose own body holds the
    item's text or one of its rare words (`mentions`); a node merely built on the item's sources that says nothing of
    it (a procedure on a card that cites the source message) doesn't count (re-review F2, f3). The rare words are
    measured once, against every candidate: they never change with which tasks are kept (re-review NEW-2). Being
    given the item scrubs a task's lines; its card still goes only when the body restates the item (`scrub`)."""
    lines = _Lines(forgotten.phrases, forgotten.contents, _rare(db, forgotten, given(db, forgotten), _ferb_rows(db)))
    forgotten.tasks = {task for task, refs in forgotten.through.items() if any(
        _direct(ref) or any(lines.mentions(text) for text in forgotten.node_texts.get(ref, [])) for ref in refs)}
    return lines


def scrub(db: Any, forgotten: Forgotten, lines: "_Lines | None" = None) -> Scrubbed:
    """Remove what FERB's own records say of the forgotten item (module docstring), after its deletion."""
    out = Scrubbed()
    phrases = forgotten.phrases
    if not phrases and not forgotten.requests:
        return out
    lines = lines or settle(db, forgotten)
    ferb = _ferb_rows(db)
    written_with = given(db, forgotten)
    for row in ferb:
        data = json.loads(row["data_json"])
        answers_forgotten = row["kind"] in CHAT_KINDS and data.get("request_id") in forgotten.requests
        of_task = row["kind"] in TASK_KINDS and _root(str(data.get("task_id", ""))) in forgotten.tasks
        rule = (lines.mentions if of_task else lines.restates if row["id"] in written_with else lines.verbatim)
        new = dict(data)
        for key, value in data.items():
            if of_task and key not in IDS:          # a task's outcome: its summary, verification and subtasks too
                new[key] = lines.value(value, rule)
            elif key not in FIELDS or not isinstance(value, str) or not value:
                continue
            elif answers_forgotten:
                new[key] = REMOVED
                lines.removed += [line for line in value.split("\n") if lines.restates(line)]
            else:
                new[key] = lines.text(value, rule)
        if new != data:
            _rewrite(db, row, new)
            out.rewritten.append(row["id"])
    change = (lambda text: lines.text(text, lines.mentions), lambda value: lines.value(value, lines.mentions))
    _scrub_records(db, {task: change for task in forgotten.tasks})     # the tasks' own records
    for task in sorted(forgotten.tasks):                 # its card
        for card in db.conn.execute("SELECT id, body_json FROM learning_nodes WHERE kind = 'episode' AND key = ?",
                                    (task,)).fetchall():
            # a card goes only when its body restates the item, never on one rare word (re-review F2)
            if any(lines.restates(s) for s in _strings(json.loads(card["body_json"]))):
                out.cards.append(card["id"])

    def spans(watch: SpanWatch) -> None:                 # their spans and planner turns, then every span
        index = _Index(db, set())
        _rewrite_spans(db, [{"id": i} for i in index.task_spans(forgotten.tasks)], change[1], watch=watch)
        for request in sorted(forgotten.requests):       # the planner's own answer to a forgotten message goes whole
            for span in db.conn.execute("SELECT id, input_json, output_json FROM spans WHERE kind = 'model.turn' AND "
                                        "json_extract(input_json, '$.request_id') = ?", (request,)).fetchall():
                rewrite_recorded(db, span["id"], _answer_removed, watch=watch,
                                 held=(span["input_json"], span["output_json"]))
        scrub_told(db, {*lines.removed, *phrases}, watch)
    rewrite_until_still(db, spans)
    return out


def _answer_removed(inp: Any, out: Any) -> tuple[Any, Any]:
    return inp, (out | {"text": REMOVED} if isinstance(out, dict) and out.get("text") not in (None, REMOVED) else out)


def _rewrite(db: Any, row: Any, data: dict[str, Any]) -> None:
    """An append-only journal row changed by a forget: deleted and written again with the same seq, id, time, kind
    and origin, only while it still holds what was read (a row the loop deleted meanwhile, such as a notice's chat
    entry the purge removed, is never written back: fix wave 6a)."""
    if db.conn.execute("DELETE FROM journal WHERE seq=? AND data_json=?", (row["seq"], row["data_json"])).rowcount:
        db.conn.execute("INSERT INTO journal (seq, id, at, kind, data_json, origin) VALUES (?,?,?,?,?,?)",
                        (row["seq"], row["id"], row["at"], row["kind"], json.dumps(data), row["origin"]))


def _rewrite_spans(db: Any, spans: list[Any], change: Callable[[Any], Any], needles: Any = None,
                   watch: SpanWatch | None = None) -> None:
    """Each span once, read as it is now and written only while it still holds what was read (`rewrite_recorded`: a
    span listed twice, or one a live write changed meanwhile, is never rewritten from an old copy). `needles`: a span
    whose stored text (lowercased) holds none of them can't change, and isn't decoded."""
    for ident in dict.fromkeys(span["id"] for span in spans):
        rewrite_recorded(db, ident, lambda inp, out: (change(inp), change(out)), watch=watch, needles=needles)


Change = tuple[Callable[[str], str], Callable[[Any], Any]]      # how one task's records change: a text, a JSON value


def _scrub_records(db: Any, changes: dict[str, Change]) -> None:
    """The tasks' own records, each changed by its root task's rule, one pass per table (never a scan per task): their
    stored goal and outcome, saved spec, permanent record and "while you were away" result. Each row is written only
    while it still holds what was read (`swap`): an outcome the loop wrote meanwhile is changed, never lost."""
    if not changes:
        return

    def task(change: Change, held: tuple[Any, ...]) -> tuple[Any, ...]:
        goal, outcome = change[0](held[0]), json.dumps(change[1](json.loads(held[1] or "{}")))
        return held if goal == held[0] and outcome == (held[1] or "{}") else (goal, outcome)

    def spec(change: Change, held: tuple[Any, ...]) -> tuple[Any, ...]:       # its saved plan, to resume
        return (json.dumps(change[1](json.loads(held[0]))),)

    def episode(change: Change, held: tuple[Any, ...]) -> tuple[Any, ...]:
        return tuple(change[0](v) if not k.endswith("_json") else
                     json.dumps(change[1](json.loads(v)), ensure_ascii=False) for k, v in zip(EPISODE_FIELDS, held))

    def result(change: Change, held: tuple[Any, ...]) -> tuple[Any, ...]:
        return change[0](held[0]), change[0](held[1])
    for table, key, columns, root, rule in (
            ("tasks", "id", ("goal", "outcome_json"), _root, task),
            ("task_context", "task_id", ("spec_json",), str, spec),
            ("episodes", "task_id", EPISODE_FIELDS, str, episode),
            ("task_results", "task_id", ("goal", "answer"), str, result)):
        for row in db.conn.execute(f"SELECT {key}, {', '.join(columns)} FROM {table}").fetchall():
            if (change := changes.get(root(str(row[key])))) is not None:
                swap(db, table, key, row[key], columns, partial(rule, change), tuple(row[c] for c in columns))


def _wordlike(ch: str) -> bool:
    return unicodedata.category(ch)[0] in "LNM" or unicodedata.category(ch) == "Co"


_ESCAPED = re.compile(r'\\(?:u([0-9a-fA-F]{4})|[nrtbf"/\\])$')


def _before(value: str, at: int) -> str:
    """The character before `at` as written: inside a JSON document held in a string, an escape (`\\n`, `\\u201c`)
    stands for one character, which is what decides a word's edge."""
    if at == 0:
        return " "
    m = _ESCAPED.search(value, max(0, at - 6), at)
    if m is None:
        return value[at - 1]
    return chr(int(m.group(1), 16)) if m.group(1) else " "


class _Words:
    """Removes phrases from text as whole words, case and spacing aside (final fix: never a plain substring, so
    "Gmail" never takes a piece of "my Gmail inbox", and a single-line JSON output loses the phrase, not the line);
    also as a JSON document inside a recorded string holds them."""

    def __init__(self, phrases: Any) -> None:
        forms = {f for p in phrases if p.strip() for f in (p.strip(), json.dumps(p.strip())[1:-1],
                                                             json.dumps(p.strip(), ensure_ascii=False)[1:-1])}
        self.patterns = [re.compile(r"\s+".join(re.escape(w) for w in f.split()), re.IGNORECASE)
                         for f in sorted(forms, key=len, reverse=True)]
        # what a stored row must hold for any form to be there (its longest word, lowercased, as stored)
        longest = {max(p.split(), key=len).lower() for p in phrases if p.strip()}
        self.needles = sorted({n for w in longest for n in (json.dumps(w)[1:-1], json.dumps(json.dumps(w)[1:-1])[1:-1])})

    def text(self, value: str) -> str:
        for pattern in self.patterns:
            out, pos = [], 0
            for m in pattern.finditer(value):
                a, b = m.start(), m.end()
                if (_wordlike(value[a]) and _wordlike(_before(value, a))) or \
                        (b < len(value) and _wordlike(value[b]) and _wordlike(value[b - 1])):
                    continue                              # part of a longer word
                out += [value[pos:a], REMOVED]
                pos = b
            value = "".join(out) + value[pos:] if out else value
        return value

    def value(self, value: Any) -> Any:
        if isinstance(value, str):
            return self.text(value)
        if isinstance(value, dict):
            return {k: self.value(v) for k, v in value.items()}
        if isinstance(value, list):
            return [self.value(v) for v in value]
        return value


def scrub_told(db: Any, told: set[str], watch: SpanWatch | None = None) -> None:
    """Every span loses each told text of two words or more, matched as whole words (`_Words`); a single word is
    never told (final fix)."""
    texts = {t.strip() for t in told if len(t.split()) >= 2}
    if not texts:
        return
    words = _Words(texts)
    db.conn.execute("CREATE TEMP TABLE IF NOT EXISTS told_needles (needle TEXT PRIMARY KEY)")
    try:
        db.conn.executemany("INSERT OR IGNORE INTO told_needles VALUES (?)", [(n,) for n in words.needles])
        ids = [r[0] for r in db.conn.execute(
            "SELECT id FROM spans s WHERE EXISTS (SELECT 1 FROM told_needles n WHERE instr(lower(s.input_json), "
            "n.needle) > 0 OR instr(lower(COALESCE(s.output_json, '')), n.needle) > 0)").fetchall()]
    finally:
        db.conn.execute("DELETE FROM told_needles")
    _rewrite_spans(db, [{"id": i} for i in ids], words.value, watch=watch)


@dataclass
class NodeEcho:
    """A forgotten learned item's echo (re-review R3, round 5, closing round, final fix): `naming` FERB's naming of each
    named item in its "Forget for good?" question (FERB's own wording, naming only that item); `versions` each
    forgotten version's id and what it states (body text, a procedure's intent, a lesson's trigger); `in_prompts_since`
    an owner identity statement's version id and when it was recorded (from then on it is in every model run's system
    prompt, Layer 1); `statements` a version's id and its statement (a lesson's or an identity's text) when it is two
    words or more: FERB's chat replies and notices lose it word for word wherever they were written."""
    naming: list[str] = field(default_factory=list)
    versions: dict[str, list[str]] = field(default_factory=dict)
    in_prompts_since: dict[str, str] = field(default_factory=dict)
    statements: dict[str, str] = field(default_factory=dict)

    @property
    def refs(self) -> set[str]:
        return set(self.versions)


def _utc(at: str) -> datetime:
    when = datetime.fromisoformat(at)
    return when.replace(tzinfo=timezone.utc) if when.tzinfo is None else when.astimezone(timezone.utc)


NOTICE_KIND = "chat.notice"


class _Index:
    """What a forget's scopes are read from, each read once: the tasks (id, request, created), the spans' metadata
    (task, kind, start, whether they record a model run's system prompt, a planner turn's request, an attention check's
    rows), the spans naming each ref by its id, and the notices with the rows they saw."""

    def __init__(self, db: Any, refs: set[str]) -> None:
        self.tasks = db.conn.execute("SELECT id, request_id, created_at FROM tasks").fetchall()
        self.spans = db.conn.execute(
            "SELECT id, task_id, kind, started_at, json_type(input_json, '$.system_prompt') IS NOT NULL AS prompt, "
            "CASE WHEN kind = 'model.turn' THEN json_extract(input_json, '$.request_id') END AS request, "
            "CASE WHEN kind = 'attention.consider' THEN json_extract(input_json, '$.source_refs') END AS saw "
            "FROM spans").fetchall()
        self.naming: dict[str, set[str]] = {}
        for span, ref in _with_refs(db, refs, "SELECT s.id, f.ref FROM spans s JOIN forgetting_refs f ON "
                                              "instr(s.input_json, f.ref) > 0 OR instr(COALESCE(s.output_json, ''), "
                                              "f.ref) > 0") if refs else []:
            self.naming.setdefault(str(ref), set()).add(str(span))
        self.given: dict[str, set[str]] = {}
        for task, ref in _with_refs(db, refs, "SELECT t.task_id, t.ref FROM task_memory_refs t JOIN forgetting_refs f "
                                              "ON t.ref = f.ref") if refs else []:
            self.given.setdefault(str(ref), set()).add(_root(str(task)))
        self.listed: dict[str, list[tuple[str, str]]] = {}
        for row, request, ref in _with_refs(
                db, refs, f"SELECT j.id, json_extract(j.data_json, '$.request_id'), f.ref FROM journal j, "
                          "json_each(j.data_json, '$.source_refs') r JOIN forgetting_refs f ON r.value = f.ref WHERE "
                          f"j.kind IN ({','.join('?' * len(CHAT_KINDS))}) AND json_type(j.data_json, "
                          "'$.source_refs') = 'array'", CHAT_KINDS) if refs else []:
            self.listed.setdefault(str(ref), []).append((str(row), str(request or "")))
        self.notices = [(str(r["id"]), set(json.loads(r["source_refs_json"])) | set(json.loads(r["considered_refs_json"])))
                        for r in db.conn.execute("SELECT id, source_refs_json, considered_refs_json FROM "
                                                 "attention_notices").fetchall()]

    def turns(self, requests: set[str]) -> set[str]:
        return {str(s["id"]) for s in self.spans if s["request"] is not None and str(s["request"]) in requests}

    def task_spans(self, roots: set[str]) -> set[str]:
        """Every span of these tasks' trees, and the planner turns that planned them."""
        if not roots:
            return set()
        requests = {str(t["request_id"]) for t in self.tasks if _root(str(t["id"])) in roots}
        return {str(s["id"]) for s in self.spans if s["task_id"] and _root(str(s["task_id"])) in roots} | \
            self.turns(requests)

    def shown(self, ref: str, since: str | None) -> tuple[set[str], set[str], set[str], set[str]]:
        """Where one learned item was shown or cited: (tasks, spans, chat rows, notices). The tasks given it
        (`task_memory_refs`); the chat rows whose source refs cite it; spans: those tasks' trees and planner turns, the
        planner turns of those chat rows, and every span naming it by its id (a run with no request, such as the
        attention check, records no exposure). For an identity statement, from `since` on (compared in UTC): every task
        created and every span of a task or recording a model run's system prompt. Notices: those made by an
        attention check among those spans (the rows it saw)."""
        tasks = set(self.given.get(ref, set()))
        rows = {row for row, _ in self.listed.get(ref, [])}
        spans = set(self.naming.get(ref, set())) | self.turns({request for _, request in self.listed.get(ref, [])})
        if since is not None:
            start = _utc(since)
            tasks |= {_root(str(t["id"])) for t in self.tasks if _utc(str(t["created_at"])) >= start}
            spans |= {str(s["id"]) for s in self.spans if (s["task_id"] or s["prompt"]) and
                      _utc(str(s["started_at"])) >= start}
        spans |= self.task_spans(tasks)
        saw = {r for s in self.spans if str(s["id"]) in spans and s["saw"] for r in json.loads(s["saw"])}
        notices = {ident for ident, refs in self.notices if refs & saw}
        return tasks, spans, rows, notices


def finish_node(db: Any, echo: NodeEcho) -> Scrubbed:
    """A learned item's echo pass, after its deletion (final adjudicated fix).
    - FERB's naming of it goes from FERB's conversation rows (each line holding it) and every span.
    - What each forgotten version states goes where that version was shown or cited (`_Index.shown`), never across
      every span: those tasks' answer and outcome rows and records lose each line holding it; the chat rows that
      listed it and the notices of a check shown it lose it (lines in journal rows, whole words in a notice's text);
      those spans lose it as whole words, so a single-line output keeps the rest.
    - A version's statement (a lesson's or an identity's text, two words or more) also goes, word for word, from
      every FERB chat reply and notice wherever written; a trigger, an intent or a key never does.
    - Copies of the FERB lines removed (two words or more) go from every span, as whole words.
    - The cards of those tasks whose body states it are returned (`Scrubbed.cards`), for the daemon to forget."""
    out = Scrubbed()
    naming = [t for t in echo.naming if t.strip()]
    said = {ref: [t for t in texts if t.strip()] for ref, texts in echo.versions.items()}
    statements = sorted({t.strip() for t in echo.statements.values() if len(t.split()) >= 2})
    if not naming and not statements and not any(said.values()):
        return out
    in_tasks, in_spans, in_rows, in_notices = _scopes(db, echo, said)
    lines: dict[frozenset[str], _Lines] = {}

    def lines_of(phrases: set[str]) -> _Lines:
        return lines.setdefault(frozenset(phrases), _Lines(sorted(phrases), [], set()))
    named, stated = _Lines(naming, [], set()), _Lines(statements, [], set())
    for row in db.conn.execute(f"SELECT seq, id, at, kind, data_json, origin FROM journal WHERE kind IN "
                               f"({','.join('?' * (len(FERB_KINDS) + 1))}) ORDER BY seq",
                               (*FERB_KINDS, NOTICE_KIND)).fetchall():
        data = json.loads(row["data_json"])
        chat = row["kind"] in CHAT_KINDS or row["kind"] == NOTICE_KIND
        scoped = (in_tasks.get(_root(str(data.get("task_id", "")))) if row["kind"] in TASK_KINDS else
                  in_notices.get(str(data.get("id"))) if row["kind"] == NOTICE_KIND else in_rows.get(row["id"]))
        own = lines_of(scoped) if scoped else None
        new = {}
        for key, value in data.items():
            if row["kind"] in TASK_KINDS and key not in IDS:   # a task's outcome: its summary and subtasks too
                value = named.value(value, named.verbatim)
                value = own.value(value, own.verbatim) if own is not None else value
            elif key in FIELDS and isinstance(value, str) and value:
                value = named.text(value, named.verbatim)
                value = stated.text(value, stated.verbatim) if chat else value
                value = own.text(value, own.verbatim) if own is not None else value
            new[key] = value
        if new != data:
            _rewrite(db, row, new)
            out.rewritten.append(row["id"])
    _scrub_records(db, {task: ((lambda own: (lambda t: own.text(t, own.verbatim), lambda v: own.value(v, own.verbatim)))
                               (lines_of(phrases))) for task, phrases in in_tasks.items()})
    for task, phrases in sorted(in_tasks.items()):     # its card goes when its body states it (no partial card)
        own = lines_of(phrases)
        for card in db.conn.execute("SELECT id, body_json FROM learning_nodes WHERE kind = 'episode' AND key = ?",
                                    (task,)).fetchall():
            if any(own.verbatim(s) for s in _strings(json.loads(card["body_json"]))):
                out.cards.append(card["id"])
    words: dict[frozenset[str], _Words] = {}

    def words_of(phrases: set[str]) -> _Words:
        return words.setdefault(frozenset(phrases), _Words(phrases))
    for row in db.conn.execute("SELECT id, text FROM attention_notices").fetchall():
        about = set(statements) | in_notices.get(str(row["id"]), set())
        if about:                                   # written only while it holds what was read (a purge may clear it)
            swap(db, "attention_notices", "id", row["id"], ("text",), partial(_notice_text, words_of(about)),
                 (row["text"],))
    told = {*named.removed, *named.phrases, *stated.removed, *(line for own in lines.values() for line in own.removed)}

    def spans(watch: SpanWatch) -> None:            # a span that moved may now name it: its scope is read again
        by_phrases: dict[frozenset[str], list[dict[str, str]]] = {}
        for span, phrases in (_scopes(db, echo, said)[1] if watch.again else in_spans).items():
            by_phrases.setdefault(frozenset(phrases), []).append({"id": span})
        for shown, ids in by_phrases.items():       # whole words, never a whole line (a one-line output keeps the rest)
            own = words_of(set(shown))
            _rewrite_spans(db, ids, own.value, own.needles, watch)
        scrub_told(db, told, watch)
    rewrite_until_still(db, spans)
    _drop_folded(db, set(out.rewritten))
    db.conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    return out


def _notice_text(words: _Words, held: tuple[Any, ...]) -> tuple[Any, ...]:
    return (words.text(held[0]),)


def _scopes(db: Any, echo: NodeEcho, said: dict[str, list[str]]) -> tuple[dict[str, set[str]], ...]:
    """Where each forgotten version was shown or cited (`_Index.shown`), as (tasks, spans, chat rows, notices), each
    with the texts that go from it."""
    index = _Index(db, {ref for ref, texts in said.items() if texts})
    scopes: tuple[dict[str, set[str]], ...] = ({}, {}, {}, {})
    for ref, texts in said.items():
        if not texts:
            continue
        for scope, keys in zip(scopes, index.shown(ref, echo.in_prompts_since.get(ref))):
            for key in keys:
                scope.setdefault(key, set()).update(texts)
    return scopes


def finish_node_at(path: Any, echo: NodeEcho) -> Scrubbed:
    """`finish_node` on a connection of its own (the daemon runs it in a worker thread, off the event loop)."""
    from ferb.store.db import Database
    db = Database(path)
    try:
        return finish_node(db, echo)
    finally:
        db.close()
