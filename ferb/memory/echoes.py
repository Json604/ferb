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
- every span loses those lines and texts.
Journal rows are append-only (they are only ever deleted, for a forget): a changed row is deleted and written again
with the same sequence number, id, time, kind and origin. The owner's own messages are never rewritten."""
import json
from dataclasses import dataclass, field
from typing import Any, Callable

from ferb.memory.items import contains_words, query_terms

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
    for task in sorted(forgotten.tasks):                 # the task's own records, and its card
        _scrub_task(db, task, lines, lines.mentions)
        for card in db.conn.execute("SELECT id, body_json FROM learning_nodes WHERE kind = 'episode' AND key = ?",
                                    (task,)).fetchall():
            # a card goes only when its body restates the item, never on one rare word (re-review F2)
            if any(lines.restates(s) for s in _strings(json.loads(card["body_json"]))):
                out.cards.append(card["id"])
    for request in sorted(forgotten.requests):           # the planner's own answer to a forgotten message goes whole
        for span in db.conn.execute("SELECT id, output_json FROM spans WHERE kind = 'model.turn' AND "
                                    "json_extract(input_json, '$.request_id') = ?", (request,)).fetchall():
            out_value = json.loads(span["output_json"] or "null")
            if isinstance(out_value, dict) and out_value.get("text") not in (None, REMOVED):
                db.conn.execute("UPDATE spans SET output_json = ? WHERE id = ?",
                                (json.dumps(out_value | {"text": REMOVED}), span["id"]))
    scrub_told(db, {*lines.removed, *phrases})
    return out


def _rewrite(db: Any, row: Any, data: dict[str, Any]) -> None:
    """An append-only journal row changed by a forget: deleted and written again with the same seq, id, time, kind
    and origin."""
    db.conn.execute("DELETE FROM journal WHERE seq=?", (row["seq"],))
    db.conn.execute("INSERT INTO journal (seq, id, at, kind, data_json, origin) VALUES (?,?,?,?,?,?)",
                    (row["seq"], row["id"], row["at"], row["kind"], json.dumps(data), row["origin"]))


def _scrub_spans(db: Any, spans: list[Any], lines: "_Lines", rule: Callable[[str], bool]) -> None:
    for span in spans:
        before_in, before_out = json.loads(span["input_json"]), json.loads(span["output_json"] or "null")
        new_in, new_out = lines.value(before_in, rule), lines.value(before_out, rule)
        if new_in != before_in or new_out != before_out:
            db.conn.execute("UPDATE spans SET input_json = ?, output_json = ? WHERE id = ?",
                            (json.dumps(new_in), None if new_out is None else json.dumps(new_out), span["id"]))


def _turns(db: Any, request: str) -> list[Any]:
    """The planner turns of one request."""
    return list(db.conn.execute("SELECT id, input_json, output_json FROM spans WHERE kind = 'model.turn' AND "
                                "json_extract(input_json, '$.request_id') = ?", (request,)).fetchall())


def _scrub_task(db: Any, task: str, lines: "_Lines", rule: Callable[[str], bool]) -> None:
    """A task's own records lose each line `rule` holds: its stored goal and outcome, saved spec, permanent record,
    "while you were away" result, every span of its tree and the planner turn that planned it."""
    for row in db.conn.execute("SELECT id, goal, outcome_json FROM tasks WHERE id = ? OR id LIKE ?",
                               (task, task + ".%")).fetchall():
        goal = lines.text(row["goal"], rule)
        outcome = json.dumps(lines.value(json.loads(row["outcome_json"] or "{}"), rule))
        if goal != row["goal"] or outcome != (row["outcome_json"] or "{}"):
            db.conn.execute("UPDATE tasks SET goal = ?, outcome_json = ? WHERE id = ?", (goal, outcome, row["id"]))
    for row in db.conn.execute("SELECT task_id, spec_json FROM task_context WHERE task_id = ?", (task,)).fetchall():
        spec = json.dumps(lines.value(json.loads(row["spec_json"]), rule))   # its saved plan, to resume
        if spec != row["spec_json"]:
            db.conn.execute("UPDATE task_context SET spec_json = ? WHERE task_id = ?", (spec, row["task_id"]))
    episode = db.conn.execute(f"SELECT {', '.join(EPISODE_FIELDS)} FROM episodes WHERE task_id = ?",
                              (task,)).fetchone()
    if episode is not None:
        new_values = [lines.text(episode[k], rule) if not k.endswith("_json") else
                      json.dumps(lines.value(json.loads(episode[k]), rule), ensure_ascii=False)
                      for k in EPISODE_FIELDS]
        if new_values != [episode[k] for k in EPISODE_FIELDS]:
            db.conn.execute(f"UPDATE episodes SET {', '.join(k + ' = ?' for k in EPISODE_FIELDS)} WHERE task_id = ?",
                            (*new_values, task))
    for row in db.conn.execute("SELECT task_id, goal, answer FROM task_results WHERE task_id = ?", (task,)).fetchall():
        goal, answer = lines.text(row["goal"], rule), lines.text(row["answer"], rule)
        if (goal, answer) != (row["goal"], row["answer"]):      # the "while you were away" result
            db.conn.execute("UPDATE task_results SET goal = ?, answer = ? WHERE task_id = ?", (goal, answer, task))
    spans = list(db.conn.execute("SELECT id, input_json, output_json FROM spans WHERE task_id = ? OR task_id LIKE ?",
                                 (task, task + ".%")).fetchall())
    for (request,) in db.conn.execute("SELECT DISTINCT request_id FROM tasks WHERE id = ? OR id LIKE ?",
                                      (task, task + ".%")).fetchall():
        spans += _turns(db, request)              # the planner turn that planned it
    _scrub_spans(db, spans, lines, rule)          # every record of the task's tree: tool calls, turns, evidence


def scrub_told(db: Any, told: set[str]) -> None:
    """Every span loses each told text (`_told_pairs`)."""
    pairs = _told_pairs(told)
    if pairs:
        from ferb.orchestrator.attention import scrub_spans
        scrub_spans(db, pairs, every_kind=True)


def _told_pairs(told: set[str]) -> list[tuple[str, str]]:
    """Each told text and its replacement: as recorded, space-normalised (FERB's questions show a memory's text so:
    re-review R2), and as a JSON document inside a recorded string holds either; longest first."""
    texts = {t.strip() for t in told if t.strip()}
    texts |= {" ".join(t.split()) for t in texts}
    pairs = {(t, REMOVED) for t in texts}
    for ascii_only in (True, False):
        pairs |= {(json.dumps(t, ensure_ascii=ascii_only)[1:-1], json.dumps(REMOVED, ensure_ascii=ascii_only)[1:-1])
                  for t in texts}
    return sorted(pairs, key=lambda p: -len(p[0]))


@dataclass
class NodeEcho:
    """A forgotten learned item's echo (re-review R3, round 5): `refs` the node ids; `naming` FERB's naming of each in
    its "Forget for good?" question (FERB's own wording, naming only that item); `texts` what they state;
    `in_every_turn_since` when the item was in every model turn's prompt from then on (an owner identity statement,
    Layer 1), else None."""
    refs: set[str] = field(default_factory=set)
    naming: list[str] = field(default_factory=list)
    texts: list[str] = field(default_factory=list)
    in_every_turn_since: str | None = None


def finish_node(db: Any, echo: NodeEcho) -> Scrubbed:
    """A learned item's echo pass, after its deletion (round 5 ruling). FERB's naming of it goes from FERB's
    conversation rows (each line holding it) and from every span. What it states goes only where it was shown or
    cited, never by a substring replace across every span: the tasks given it (`task_memory_refs`: their answer and
    outcome rows, records, spans and the planner turn that planned them), the chat turns that listed it (the chat
    rows whose source refs cite it, and those requests' planner turns), and for an owner identity statement every
    model turn since it was recorded (it is in each one's prompt). Copies of the FERB lines it removed go from every
    span, as a memory forget's do."""
    out = Scrubbed()
    naming, texts = [t for t in echo.naming if t.strip()], [t for t in echo.texts if t.strip()]
    named, stated, in_spans = _Lines(naming, [], set()), _Lines(texts, [], set()), _Lines(texts, [], set())
    if not named.phrases and not stated.phrases:
        return out
    tasks = {_root(str(r[0])) for r in _with_refs(
        db, echo.refs, "SELECT DISTINCT t.task_id FROM task_memory_refs t JOIN forgetting_refs f ON t.ref = f.ref")}
    listed = _with_refs(db, echo.refs, f"SELECT DISTINCT j.id, json_extract(j.data_json, '$.request_id') FROM journal j, "
                                       "json_each(j.data_json, '$.source_refs') r JOIN forgetting_refs f ON r.value = "
                                       f"f.ref WHERE j.kind IN ({','.join('?' * len(CHAT_KINDS))}) AND "
                                       "json_type(j.data_json, '$.source_refs') = 'array'", CHAT_KINDS)
    for row in _ferb_rows(db):
        data = json.loads(row["data_json"])
        of_task = row["kind"] in TASK_KINDS and _root(str(data.get("task_id", ""))) in tasks
        scoped = of_task or row["id"] in {r[0] for r in listed}
        new = {}
        for key, value in data.items():
            if of_task and key not in IDS:
                value = stated.value(named.value(value, named.verbatim), stated.verbatim)
            elif key in FIELDS and isinstance(value, str) and value:
                value = named.text(value, named.verbatim)
                value = stated.text(value, stated.verbatim) if scoped else value
            new[key] = value
        if new != data:
            _rewrite(db, row, new)
            out.rewritten.append(row["id"])
    if in_spans.phrases:
        for task in sorted(tasks):
            _scrub_task(db, task, in_spans, in_spans.verbatim)
        for request in sorted({str(r[1]) for r in listed if r[1]}):
            _scrub_spans(db, _turns(db, request), in_spans, in_spans.verbatim)
        if echo.in_every_turn_since is not None:
            _scrub_spans(db, list(db.conn.execute(
                "SELECT id, input_json, output_json FROM spans WHERE kind = 'model.turn' AND started_at >= ?",
                (echo.in_every_turn_since,)).fetchall()), in_spans, in_spans.verbatim)
    scrub_told(db, {*named.removed, *named.phrases, *stated.removed})
    _drop_folded(db, set(out.rewritten))
    db.conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    return out


def finish_node_at(path: Any, echo: NodeEcho) -> Scrubbed:
    """`finish_node` on a connection of its own (the daemon runs it in a worker thread, off the event loop)."""
    from ferb.store.db import Database
    db = Database(path)
    try:
        return finish_node(db, echo)
    finally:
        db.close()
