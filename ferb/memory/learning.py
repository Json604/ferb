"""F8 derived memory, provenance and owner controls. Never grants authority to learned content.

Only the sleep service writes inferred records. Content versions are immutable, source edges survive retention,
and explicit forgetting traverses them. An owner confirmation of a procedure permits reuse as data; its origin
is unchanged. Identity is a separate owner-authored version, never a relabelled model proposal.
"""
from __future__ import annotations

import hashlib
import json
import math
from dataclasses import asdict
from datetime import datetime
from pathlib import Path
from typing import Any, Callable, Iterable, Iterator, NamedTuple

from ferb.core import origin
from ferb.core.errors import FerbError
from ferb.memory.items import query_terms, weighted
from ferb.memory.lineage import REQUEST_REFERENCE, call_sources, direct
from ferb.orchestrator import episodes
from ferb.sanitize.redact import REDACTED, Redactor, first_format, holds_sealed, secret_key


class LearningRefused(FerbError):
    pass


# Kinds whose key is one of FERB's own ids: an episode card (its task id), its annotation and its intent label (L29)
# (the card's node id).
ID_KEYED = frozenset({'episode', 'episode_annotation', 'episode_intent'})
# The episode card's composer. 2: a card cites its task's direct sources only (memory review C1). 3: an action's
# target provenance is read in the gateway's vocabulary (`outside_targets`, L29 Task 6 review I1). 4: neither the site
# an action ran on nor a call the gateway records no provenance for is outside (plan 2026-10-23 Task 2). A card an
# older composer built is rebuilt once by the hourly pass.
CARD_MAKER = 'code:evidence-card/4'
# The error the redactor before step 6a parked cards with (redaction wasn't idempotent; notes/memory.md): such a card
# is read again (`cards_to_read`); a second failure records a new error, so it is tried once.
PARKED_BY_REDACTION = 'redaction would still change it'
# The composer before C1 (fixed in e0e344b): it copied into a card the refs of every block its planner was shown, and
# every earlier version's. A store holding its cards gets the one-time clean-out (`prune_card_sources`, run by setup).
COPYING_CARD_MAKER = 'code:evidence-card/1'
# The source-ref index (migration 0030): that clean-out drops it while it deletes and makes it again after; a power
# loss in between leaves it missing whatever the schema version, so every daemon start makes it too (L28, C1)
SOURCE_REF_INDEX = 'CREATE INDEX IF NOT EXISTS learning_source_ref ON learning_sources(source_ref)'
# Model-written suggestions the owner can turn down ("Not right": retire); one turned down never comes back (M3).
SUGGESTIONS = frozenset({'procedure', 'identity_proposal', 'lesson'})
# The owner's budgets (2026-10-24: "Planner 60, worker 20"): the most relevant items a planner turn, and a worker or
# plan step, get after ranking. The identity layer (the owner's own preferences about themselves) is outside them.
PLANNER_MEMORY_ITEMS = 60
WORKER_MEMORY_ITEMS = 20
# What a prompt's memory never lists: identity is in every system prompt, an intent label is bookkeeping, and the
# procedures the planner may choose are listed apart (L29)
PROMPT_LEAVES_OUT = frozenset({'identity', 'episode_intent', 'procedure'})
# Ruling: recency's share of a node's score, at most RECENCY_WEIGHT (the gap between ranks 4 and 5), halving every
# RECENCY_HALF_LIFE_DAYS, so it reorders near-equal matches only. An owner-stated node always takes the full share: the
# owner's words don't age, so at equal relevance no guess outscores them (review fix 1)
RECENCY_WEIGHT = 0.05
RECENCY_HALF_LIFE_DAYS = 30
# The version of `search_text` the index holds (migration 0048, `reindex_search`)
SEARCH_TEXT_VERSION = 'content/1'
# The stored bodies re-indexed per write transaction (`reindex_search`), in bytes; one node larger than this is a batch
# of its own. A batch's cost is mostly erasing the old JSON text's terms (FTS5 secure-delete, migration 0037). On a
# scratch copy of the owner's database (2026-10-24: 1,150 nodes, 30 MB of JSON-indexed text) the whole re-index took
# 6.6 s in 189 batches, each holding SQLite's write lock 76 ms at most: the daemon's own writes and a forget wait no
# longer than that.
REINDEX_BATCH_BYTES = 64_000


class CardSpan(NamedTuple):
    """One span of a task's tree an episode card is read from (`Learning.card_trace`)."""
    id: str
    kind: str                                   # 'tool.call', 'context.exposure' or 'context.source'
    input: dict[str, Any]
    result: Any                                 # a tool call's output as recorded ({} when none); None for the others
    error: str | None


def encoded(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':'), default=str)


def digest(value: Any) -> str:
    return hashlib.sha256(encoded(value).encode()).hexdigest()


# Line breaks JSON keeps raw (`ensure_ascii=False`) that a reader of a prompt splits lines at (`str.splitlines`; JSON
# already escapes the control characters). Escaped only after redaction, at prompt build: the redactor reads the raw
# text, and an escape would hide a secret's label from it ("otp\x85 482913")
_LINE_BREAKS = str.maketrans({'\u2028': '\\u2028', '\u2029': '\\u2029', '\x85': '\\u0085'})


def line_safe(text: str) -> str:
    """JSON text for one line of a prompt: U+2028, U+2029 and U+0085 written as JSON escapes (they appear only inside
    its strings), so model-written or recorded text never starts a line of its own (Task 5 re-review N3). Prompt only:
    `encoded`, which stored keys, digests and receipts use, keeps them as they are."""
    return text.translate(_LINE_BREAKS)


def prompt_json(value: Any) -> str:
    """`value` as `encoded` JSON, `line_safe` for a prompt."""
    return line_safe(encoded(value))


def redacted_json(redact: Callable[[str], str], value: Any, secrets: Iterable[str] = (),
                  redact_value: Callable[[Any], Any] | None = None) -> Any:
    """`value` with its secrets redacted, still JSON with every key (rule 14): redacted by structure (`redact_value`,
    the daemon's `Redactor` walk: keys, values a label names, numbers, sibling nodes), never as one text, so a
    redaction can't break the JSON or drop a key. Refused only on a specific hit in the text to be stored: one of the
    owner's known values `secrets` (`holds_sealed`) or a secret format, e.g. one spanning JSON punctuation (`1234,5678`
    in `[1234,5678]`). Without `redact_value`, `redact`'s own Redactor walks it when it is one's `text`, else each
    string and number is read by `redact` alone."""
    walk = redact_value
    if walk is None:
        owner = getattr(redact, '__self__', None)
        walk = owner if isinstance(owner, Redactor) else (lambda v: _each(redact, v))
    out = walk(value)
    stored = encoded(out)
    found = 'a saved secret' if holds_sealed(stored, secrets) else first_format(stored)
    if found:
        raise LearningRefused(f'could not be stored safely: it still holds {found}')
    return out


def _each(redact: Callable[[str], str], value: Any) -> Any:
    """Each key and string redacted by `redact`, and each number whose text it changes masked."""
    if isinstance(value, dict):
        return {redact(k) if isinstance(k, str) else k: REDACTED if secret_key(str(k)) and v not in (None, '', [], {})
                and not isinstance(v, bool) else _each(redact, v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_each(redact, v) for v in value]
    if isinstance(value, str):
        return redact(value)
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return value if redact(encoded(value)) == encoded(value) else REDACTED
    return value


def _one_line(value: Any) -> str:
    """Model-written or recorded text as one line of a prompt (nothing for a missing value)."""
    return ' '.join(str('' if value is None else value).split())


def slug_words(slug: str) -> str:
    """A key or subject slug as the words it is made of (L36 §3): `usvisascheduling_username` -> "usvisascheduling
    username", `project:us-b1b2-visa` -> "us b1b2 visa" (the label before ':' names its kind, not the item)."""
    return ' '.join(query_terms(slug.split(':', 1)[1] if ':' in slug else slug))


def search_text(kind: str, key: str, body: dict[str, Any]) -> str:
    """What search matches a node by (L36 §3): its content (for a fact, with the words of its subject and key slugs, the
    item's own words), never JSON field names or FERB's scaffolding."""
    def words(*values: Any) -> str:
        return ' '.join(_one_line(v) for v in values if isinstance(v, str) and v.strip())
    if kind == 'fact':
        return words(body.get('content'), slug_words(str(body.get('subject') or '')),
                     slug_words(str(body.get('key') or '')))
    if kind in ('entity', 'identity', 'identity_proposal'):
        return words(key, body.get('text'))
    if kind == 'episode':
        return words(body.get('intent'), body.get('narrative'), body.get('status'))
    if kind == 'episode_annotation':
        return words(body.get('reflection'), body.get('narrative'))
    if kind == 'activity_summary':
        return words(body.get('text'))         # a day's narrative; a day digest (code counts) has no text
    return ''                                  # rollups and intent labels: found through what they cite


def shown_text(node: dict[str, Any]) -> str:
    """A node as one line of a prompt or a memory.learned candidate (L36 §3): what it says, never a bare id."""
    kind, key, body = node['kind'], str(node['key']), node['body']
    if kind == 'fact':
        return f"{key} — {_one_line(body.get('content'))}"
    if kind == 'episode':
        when = str(body.get('ended_at') or body.get('started_at') or node['recorded_at'])[:10]
        outcome = str(body.get('status') or 'unknown') + (', verified' if body.get('verified') is True else '')
        return f"past task {body.get('task_id') or key} ({when}): {_one_line(body.get('intent'))} — {outcome}"
    if kind == 'episode_annotation':
        return f"about task {key}: {_one_line(body.get('reflection'))}"
    text = _one_line(body.get('text'))
    return f"{key}: {text}" if text else key


def _tied(rows: Iterable[Any], *, positional: bool = False) -> dict[str, int]:
    """Ranks from rows (id, score) ordered best first; equal scores share a rank, so two equally good matches score
    alike and the owner's statement can come first (L36 §3). Dense by default (the next distinct score takes the next
    rank); `positional`: competition ranks, as standard reciprocal rank fusion uses (the next distinct score takes its
    own position), so a large tied group can't lift the scores after it (the focus ranking, L36 §4 fix round 2)."""
    out: dict[str, int] = {}
    rank, last = -1, None
    for position, (ident, score) in enumerate(rows):
        if score != last:
            rank, last = (position if positional else rank + 1), score
        out.setdefault(ident, rank)
    return out


def _reindexed_through(db: Any) -> str | None:
    """Where the content re-index stands: None once `learning_fts` holds SEARCH_TEXT_VERSION for every node; else the
    id of the last node it re-indexed ('' for none yet: an empty table, or an earlier version's row). Node ids, never
    the implicit rowid, which a compaction (VACUUM) may renumber (review fix 2)."""
    row = db.conn.execute('SELECT version, through FROM learning_search_version').fetchone()
    if row is None or row[0] != SEARCH_TEXT_VERSION:
        return ''
    return None if row[1] is None else str(row[1])


def search_indexed(db: Any) -> bool:
    """Whether `learning_fts` holds SEARCH_TEXT_VERSION for every node (`reindex_search` finished)."""
    return _reindexed_through(db) is None


def reindex_search(db: Any, *, batch_bytes: int = REINDEX_BATCH_BYTES,
                   stop: Callable[[], bool] = lambda: False) -> bool:
    """Index every node by its content (`search_text`) once per SEARCH_TEXT_VERSION (L36 §3): a store indexed by an
    earlier version (JSON bodies, field names and all) is re-indexed in order of its node ids, in batches of about
    `batch_bytes` of stored bodies, each one write transaction that records how far it got
    (`learning_search_version.through`). The write lock is never held for long, and a re-index stopped between
    batches (`stop`, asked before each; a crash, a power loss) keeps what it did: the next call finishes the rest. Each
    batch reads its nodes and writes their text in one transaction, so a forget committed in between is never undone
    (its node is gone, and its index rows with it). A node written meanwhile is indexed by its content already
    (`put`). The last batch also drops index rows whose node is gone. True when it re-indexed any node (an empty store
    only records the version)."""
    did = False
    while not stop():
        db.conn.execute('BEGIN IMMEDIATE')
        try:
            after = _reindexed_through(db)
            if after is None:
                db.conn.execute('COMMIT')
                break
            rows = db.conn.execute('SELECT id, kind, key, body_json FROM learning_nodes WHERE id > ? ORDER BY id',
                                   (after,))
            nodes: list[Any] = []
            size, finished = 0, True
            for r in rows:
                if nodes and size >= batch_bytes:
                    finished = False                # this row and any after it: the next batch
                    break
                nodes.append(r)
                size += len(r[3])
            rows.close()
            ids = [r[0] for r in nodes]
            if ids:
                db.conn.execute(f"DELETE FROM learning_fts WHERE id IN ({','.join('?' * len(ids))})", ids)
                db.conn.executemany('INSERT INTO learning_fts VALUES (?,?)',
                                    [(r[0], search_text(r[1], r[2], json.loads(r[3]))) for r in nodes])
            if finished:
                db.conn.execute('DELETE FROM learning_fts WHERE id NOT IN (SELECT id FROM learning_nodes)')
            db.conn.execute('DELETE FROM learning_search_version')
            db.conn.execute('INSERT INTO learning_search_version VALUES (?,?)',
                            (SEARCH_TEXT_VERSION, None if finished else nodes[-1][0]))
            db.conn.execute('COMMIT')
        except BaseException:
            db.conn.execute('ROLLBACK')
            raise
        did = did or bool(nodes)
    return did


def memory_origin(label: str) -> str:
    return {'owner_stated': origin.OWNER, 'observed': origin.PRIVATE, 'inferred': origin.INFERRED,
            'third_party': origin.external('memory')}.get(label, origin.external('unlabeled'))


def shown_state(node: dict[str, Any]) -> str:
    """A node's state as a prompt shows it: a fact the owner hasn't confirmed reads as an unconfirmed hint (review M2)."""
    return 'unconfirmed hint' if node['kind'] == 'fact' and node['body'].get('unconfirmed_hint') else str(node['state'])


class Learning:
    def __init__(self, db: Any, clock: Any, publish: Callable[..., Any], redact: Callable[[str], str],
                 redact_id: Callable[[str], str] | None = None,
                 redact_value: Callable[[Any], Any] | None = None) -> None:
        """`redact_id`: redaction for a key that is one of FERB's own ids (ID_KEYED), by structure
        (`Redactor.ident`), so a registered value inside a task id can't alter the card's key; default `redact`.
        `redact_value`: a body redacted by structure (the daemon's `Redactor`); default `redacted_json`'s reading."""
        self.db, self.clock, self.publish, self.redact = db, clock, publish, redact
        self.redact_id = redact_id or redact
        self.redact_value = redact_value
        self.home = str(Path.home())   # the owner's home folder (embeddings, tests set it)
        self.embedder: Any = None
        # where a problem the owner should be able to find later is recorded (the daemon: its journal)
        self.note: Callable[[str, dict[str, Any]], Any] = publish
        # the owner's registered secret values (the daemon: its Redactor's), checked in the exact text stored
        self.secret_values: Callable[[], Iterable[str]] = lambda: ()
        # Bumped by every change to an episode card's inputs (a forget, an owner verdict, a write of a card): a card
        # read before one is read again before it is written (L28).
        self.generation = 0

    def now(self) -> str:
        return self.clock.now().isoformat()

    def get(self, node_id: str, sources: bool = True) -> dict[str, Any] | None:
        """A node with its body and origins; with `sources`, its source refs too (a day's summary or an entity can cite
        tens of thousands: a prompt line never needs them)."""
        r = self.db.conn.execute('SELECT * FROM learning_nodes WHERE id=?', (node_id,)).fetchone()
        if r is None:
            return None
        out = dict(r)
        out['body'] = json.loads(out.pop('body_json'))
        out['origins'] = json.loads(out.pop('origins_json'))
        out['confirmed'] = bool(out['confirmed'])
        if sources:
            out['sources'] = [s[0] for s in self.db.conn.execute(
                'SELECT source_ref FROM learning_sources WHERE node_id=? ORDER BY source_ref', (node_id,))]
        return out

    def items(self, kind: str | None = None, history: bool = False) -> list[dict[str, Any]]:
        sql, args = 'SELECT id FROM learning_nodes WHERE 1=1', []
        if kind:
            sql += ' AND kind=?'
            args.append(kind)
        if not history:
            sql += ' AND valid_to IS NULL AND state NOT IN (\'retired\',\'rolled_back\')'
        return [self.get(r[0]) for r in self.db.conn.execute(sql + ' ORDER BY recorded_at,id', args)]  # type: ignore[misc]

    def _source_origin(self, ref: str) -> str:
        return origin.least_trusted(self._source_origins(ref))

    def _source_origins(self, ref: str) -> set[str]:
        node = self.get(ref)
        if node:
            return set(node['origins'])
        family, sep, ident = ref.partition(':')
        if not sep:
            raise LearningRefused(f'unknown source {ref}')
        if family == 'memory':
            r = self.db.conn.execute('SELECT origin FROM memory_items WHERE id=?', (ident,)).fetchone()
            if r:
                labels = {memory_origin(r[0])}
                for source in self.db.conn.execute('SELECT kind,source_id FROM memory_sources WHERE item_id=?', (ident,)):
                    try:
                        labels.update(self._source_origins(source[0] + ':' + source[1]))
                    except LearningRefused:
                        # A retained fact may outlive its raw event. Never infer it lost privacy.
                        labels.update({origin.PRIVATE, origin.external('unlabeled')})
                return labels
        elif family == 'episode':
            r = self.db.conn.execute('SELECT origins FROM episodes WHERE task_id=?', (ident,)).fetchone()
            if r:
                return origin.split(r[0]) | {origin.FERB}
        elif family == 'trail':
            r = self.db.conn.execute('SELECT actor,source FROM trail WHERE id=?', (ident,)).fetchone()
            if r:
                from ferb.sense.tools import trail_sources
                return {origin.from_read_source(k, v) for k, v in trail_sources({r['source']})}
        elif family == 'journal':
            r = self.db.conn.execute('SELECT origin FROM journal WHERE id=?', (ident,)).fetchone()
            if r:
                return origin.recorded(r[0], True)
        retained = self.db.conn.execute('SELECT n.origins_json FROM learning_sources s JOIN learning_nodes n '
            'ON n.id=s.node_id WHERE s.source_ref=? ORDER BY n.recorded_at DESC', (ref,)).fetchall()
        if retained:
            return {label for row in retained for label in json.loads(row[0])}
        raise LearningRefused(f'source is missing: {ref}')

    def inputs_changed(self) -> None:
        """A card input changed outside Learning (an owner verdict): a card read before now is read again."""
        self.generation += 1

    def forgotten(self, ref: str) -> bool:
        """Whether an explicit owner forget deleted this ref: its tombstone (the id, never its content). A missing ref
        without one was removed by retention."""
        ref = direct(ref)
        return self.db.conn.execute('SELECT 1 FROM forgotten_refs WHERE ref=? UNION ALL SELECT 1 FROM learning_forgotten '
                                    'WHERE id=? LIMIT 1', (ref, ref)).fetchone() is not None

    @staticmethod
    def card_key(task_id: str) -> str:
        """The logical id of a task's episode card: its tombstone stops the card from ever being written again."""
        return 'key-' + digest(['episode', task_id])

    def labels(self, sources: Iterable[str]) -> set[str]:
        """Every origin the sources hold (a missing or unknown source raises LearningRefused)."""
        return {label for s in sorted(set(sources)) for label in self._source_origins(s)}

    def put(self, kind: str, key: str, body: dict[str, Any], sources: Iterable[str], *,
            origin_label: str = origin.INFERRED, made_by: str, batch: str | None = None,
            state: str = 'candidate', owner: bool = False, labels: set[str] | None = None,
            supersede: bool = True) -> dict[str, Any]:
        """`labels`: the sources' origins when the caller already read them (`labels`), e.g. on a read connection off
        the event loop; otherwise they are read here. `supersede` False: the node closes no open node of its key (a
        waiting guess's fact node: `sync_facts`)."""
        if kind == 'identity' and not owner:
            raise LearningRefused('only the owner can write identity')
        if kind in SUGGESTIONS and not owner and self.db.conn.execute(
                "SELECT 1 FROM learning_nodes WHERE kind=? AND state='retired' AND (key=? OR "
                "(json_extract(body_json, '$.text') IS NOT NULL AND json_extract(body_json, '$.text')=?)) LIMIT 1",
                (kind, key, str(body.get('text', '')))).fetchone():
            raise LearningRefused('the owner said this suggestion was not right')
        if origin_label == origin.OWNER and not owner:
            raise LearningRefused('a derived record cannot invent owner authority')
        body = redacted_json(self.redact, body, self.secret_values(), self.redact_value)
        key = self.redact_id(key) if kind in ID_KEYED else self.redact(key)
        refs = sorted(set(sources))
        labels = set(labels) if labels is not None else self.labels(refs)
        # An explicit owner statement is a new statement, still citing the old proposal as history.
        labels = {origin.OWNER} if owner else labels | origin.split(origin_label)
        inherited = origin.least_trusted(labels)
        node_id = 'm-' + digest([kind, key, body, refs, sorted(labels), made_by])
        logical_id = 'key-' + digest([kind, key])
        if self.db.conn.execute('SELECT 1 FROM learning_forgotten WHERE id IN (?,?)', (node_id, logical_id)).fetchone():
            raise LearningRefused('this content was forgotten')
        old = self.get(node_id)
        if old:
            return old
        now = self.now()
        prev = self.db.conn.execute('SELECT id FROM learning_nodes WHERE kind=? AND key=? AND valid_to IS NULL '
                                    'ORDER BY recorded_at DESC,rowid DESC LIMIT 1', (kind, key)).fetchone() \
            if supersede else None
        # Never allow a model-written fact to supersede an owner statement.
        if prev and self.get(prev[0])['origin'] == origin.OWNER and not owner:
            prev = None
        if kind == 'episode':
            self.generation += 1
        self.db.conn.execute('SAVEPOINT learning_put')
        try:
            if prev:
                self.db.conn.execute('UPDATE learning_nodes SET valid_to=? WHERE id=?', (now, prev[0]))
            self.db.conn.execute('INSERT INTO learning_nodes '
                '(id,kind,key,body_json,origin,made_by,recorded_at,valid_from,supersedes,batch_id,state,confirmed,origins_json) '
                'VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)',
                (node_id, kind, key, encoded(body), inherited, made_by, now, now, prev[0] if prev else None,
                 batch, state, int(owner), encoded(sorted(labels))))
            self.db.conn.executemany('INSERT INTO learning_sources VALUES (?,?)', [(node_id, r) for r in refs])
            self.db.conn.execute('INSERT INTO learning_fts VALUES (?,?)', (node_id, search_text(kind, key, body)))
            self.db.conn.execute('INSERT INTO learning_titles VALUES (?,?)', (node_id, key))
            self.db.conn.execute('INSERT INTO learning_changes(batch_id,node_id,action,prior_id,at) VALUES (?,?,?,?,?)',
                                 (batch or node_id, node_id, 'add', prev[0] if prev else None, now))
            self.db.conn.execute('RELEASE learning_put')
        except BaseException:
            self.db.conn.execute('ROLLBACK TO learning_put')
            self.db.conn.execute('RELEASE learning_put')
            raise
        self.publish('memory.derived', {'id': node_id, 'kind': kind, 'origin': inherited})
        return self.get(node_id)  # type: ignore[return-value]

    def capture_episode(self, task_id: str, apps: dict[str, str] | None = None) -> dict[str, Any] | None:
        return self.write_episode(self.prepare_episode(task_id, apps))

    def write_episode(self, prepared: dict[str, Any] | None) -> dict[str, Any] | None:
        """Write a card `prepare_episode` read: the only write of a capture. A card built on forgotten content is never
        written, and its tombstone stops every later capture of it (C1: no resurrection)."""
        if prepared is None:
            return None
        if prepared.get('forgotten'):
            self.db.conn.execute('INSERT OR IGNORE INTO learning_forgotten VALUES (?,?)',
                                 (self.card_key(prepared['task_id']), self.now()))
            return None
        card = self.put('episode', prepared['task_id'], prepared['body'], prepared['sources'],
                        made_by=CARD_MAKER, origin_label=origin.FERB, state='active',
                        labels=set(prepared['labels']))
        if prepared.get('inputs'):
            self.card_read(prepared['task_id'], prepared['inputs'])
        return card

    def prepare_episode(self, task_id: str, apps: dict[str, str] | None = None) -> dict[str, Any] | None:
        """Every read of an episode card: its body, its direct sources and their origins, without writing. The daemon
        runs it in `ferb.memory.card_reader`, a process of its own on a read-only connection (L28); `write_episode`
        writes the result.

        Direct sources (memory review C1): the task's own episode, the owner's request entry its planner was given, the
        sources of its own tool results and evidence, and the procedure it used. Never every conversation entry, task or
        activity row its planner was shown: those stay out of the card, so their retention or a forget of one of them
        never touches it. `damaged`/`expired`: the refs skipped as altered by redaction or kept as removed by retention
        (`_card_labels`). A card built on a forgotten source comes back as `{'task_id', 'forgotten'}` only."""
        ep = episodes.get(self.db, task_id)
        if ep is None or self.db.conn.execute('SELECT 1 FROM learning_forgotten WHERE id=?',
                                              (self.card_key(task_id),)).fetchone():
            return None
        actions = []
        source_refs = {'episode:' + task_id}
        used_apps: set[str] = set()
        traced = False
        for span in self.card_trace(task_id):          # one span at a time: an output has no size limit
            traced = True
            source_refs |= self._span_refs(span)
            if span.kind != 'tool.call':
                continue
            inp, result = span.input, span.result
            args = inp.get('args') or {}
            if isinstance(args.get('bundle_id'), str):
                used_apps.add(args['bundle_id'])
            target = inp.get('target') or {}
            if str(inp.get('tool', '')).startswith('mac.'):
                site = target.get('site')
                if site and site in (apps or {}):
                    used_apps.add(site)
                elif args.get('app') in (apps or {}):
                    used_apps.add(args['app'])
                else:
                    bid = result.get('bundle_id') if isinstance(result, dict) else None
                    used_apps.add(str(bid) if bid else 'unresolved-mac-app')
            if str(inp.get('tool', '')).startswith(('browser.', 'ferb_browser.')):
                # The retained trace may not identify a resolved browser bundle. Preserve the actual captured
                # browser inventory conservatively, never invent which browser version ran a historic action.
                browsers = {k for k in (apps or {}) if k in
                    ('com.google.Chrome', 'com.brave.Browser', 'com.apple.Safari', 'com.microsoft.edgemac')}
                used_apps.update(browsers or {'unresolved-browser'})
            arg_sources = inp.get('arg_sources')
            outcome_facts = {k: result[k] for k in ('ok', 'status', 'intent_id', 'action_id', 'ledger_id')
                             if isinstance(result, dict) and k in result}
            actions.append({'id': span.id, 'tool': inp.get('tool'), 'args': args,
                            'kind': inp.get('kind'), 'arg_sources': arg_sources, 'error': span.error,
                            'result': outcome_facts}
                           # its declared purpose as the gateway recorded it
                           | ({'declared': inp['declared']} if inp.get('declared') else {}))
        feedback = [dict(r) for r in self.db.conn.execute(
            'SELECT verdict,note,at FROM owner_verdicts WHERE task_id=? ORDER BY seq', (task_id,))]
        valid = ep['verified'] is True and not (feedback and feedback[-1]['verdict'] == 'fail')
        body = {'task_id': task_id, 'request': ep['request_text'], 'intent': ep['goal'], 'slots': {},
                'apps': {a: (apps or {}).get(a, 'unknown') for a in used_apps},
                'app_versions_captured': apps is not None, 'app_version_scope': 'identified apps and browser inventory', 'surfaces': ep['tools'], 'actions': actions,
                'status': ep['status'], 'verified': valid, 'conditions': ep['conditions'],
                'feedback': feedback, 'narrative': ep['answer'], 'how': ep.get('how') or '', 'problems': ep['problems'],
                'started_at': ep['started_at'], 'ended_at': ep['ended_at'], 'usage': ep['usage']}
        old = self.current_card(task_id)
        if old and apps is None:
            # A verdict/retention refresh keeps the versions actually captured at completion.
            body['apps'] = old['body'].get('apps', {})
            body['app_versions_captured'] = old['body'].get('app_versions_captured', False)
            if not actions:              # its trace aged out: the actions as stored
                body['actions'] = old['body'].get('actions', [])
            if not traced:
                # Its trace reached retention: the card keeps the lineage it was captured with.
                source_refs.update(old['sources'])
        labels, damaged, expired, forgotten = self._card_labels(source_refs)
        inputs = self.card_inputs(task_id).get(task_id, '')
        if forgotten:
            return {'task_id': task_id, 'forgotten': forgotten, 'inputs': inputs}
        return {'task_id': task_id, 'body': body, 'sources': sorted(source_refs - set(damaged)),
                'labels': sorted(labels), 'damaged': damaged, 'expired': expired, 'inputs': inputs}

    def card_trace(self, task_id: str) -> Iterator[CardSpan]:
        """The spans of a task's tree its card is read from, oldest first, each parsed only when the next is asked for:
        its tool calls, the blocks its planner was shown (`context.exposure`) and the sources its models were given
        (`context.source`). It yields nothing once retention removed the task's spans (all of a tree's go at once,
        `store/retention.py`)."""
        from ferb.store.spans import tree_where
        where, params = tree_where(task_id)
        rows = self.db.conn.execute("SELECT id,kind,input_json,output_json,error FROM spans WHERE kind IN "
                                    f"('tool.call','context.exposure','context.source') AND {where} ORDER BY started_at,id",
                                    params)
        for r in rows:
            yield CardSpan(r['id'], r['kind'], json.loads(r['input_json']),
                           (json.loads(r['output_json']) if r['output_json'] else {}) if r['kind'] == 'tool.call'
                           else None, r['error'])

    def _span_refs(self, span: CardSpan) -> set[str]:
        """The card rule for one span (memory review C1), each ref direct (`direct`): the owner's message its planner
        was given (REQUEST_REFERENCE) and a tool call's result and evidence sources. Nothing else its planner was
        shown."""
        if span.kind == 'context.exposure':
            if span.input.get('reference') == REQUEST_REFERENCE:         # the owner's message the task was planned from
                return {direct(str(ref)) for ref in span.input.get('source_refs', [])}
            return set()
        if span.kind == 'context.source':
            return set()
        # Missing evidence stays an edge (`_card_labels`): dropping it would let a forgotten fact's descendants be
        # reconstructed from old task traces without their deletion lineage.
        return {direct(ref) for ref in self._action_sources(span.input, span.result)}

    def card_source_refs(self, task_id: str) -> set[str] | None:
        """The source edges the card rule gives a task's card from its trace: its episode and each span's
        (`_span_refs`, as `prepare_episode` reads them), the trace read one span at a time. None when the task has no
        trace left: its card then keeps the lineage it was captured with (`prepare_episode`)."""
        refs = {'episode:' + task_id}
        traced = False
        for span in self.card_trace(task_id):
            traced = True
            refs |= self._span_refs(span)
        return refs if traced else None

    def prune_card_sources(self, stop: Callable[[], bool] = lambda: False,
                           progress: Callable[[int, int], None] = lambda done, total: None,
                           rebuilding: Callable[[], None] = lambda: None) -> dict[str, Any]:
        """The one-time clean-out of episode cards' source edges (L28, C1; setup runs it with the daemon stopped). The
        composer before C1 (COPYING_CARD_MAKER) copied into each card every ref its planner was shown, and each earlier
        version's. Each version of a card, in any state, now keeps only the edges the card rule gives its task's trace
        (`card_source_refs`); the rest are the rows that rule would never write. A card whose trace aged out is left as
        it is (its lineage can't be read again) and counted. No edge is added and no other kind of node is touched.

        Each node is cleaned in a transaction of its own, so a run stopped halfway (a crash, power loss) leaves every
        node either clean or as it was, and the next run finishes. `stop`, asked before each node (a stop the owner
        asked for), ends the run there. `progress(done, total)` is told the versions dealt with so far (cleaned, or
        left alone) after each node, so setup can say how far it is.

        The deletes run without `learning_source_ref` (migration 0030). It is dropped right before the first node that
        has rows to delete (a run with nothing to delete keeps it), and made again on the way out, first and whatever
        happens, also when it was missing before the run. With it, each node's delete rewrote ~50k random pages of that
        index: about 7 s a node, 47 minutes projected on a synthetic store of the owner's shape (23.3M rows), against
        91.7 s without (the drop's ~40 s included). After a stop or an error the index is made over every row left,
        which can take a while: `rebuilding` is told first, and an error it raises (a closed pipe) still lets the index
        be made, then reaches the caller. A power loss in between leaves it to the daemon's next start (migration 0043
        once, then `SOURCE_REF_INDEX` at every start), and to the next run.

        Returns counts: `tasks` and `nodes` (card versions) seen, `cleaned` (versions that lost rows), `rows` removed,
        `untraced` (versions left alone) and `stopped`."""
        conn = self.db.conn
        cards: dict[str, list[str]] = {}
        for row in conn.execute("SELECT id, key FROM learning_nodes WHERE kind='episode' ORDER BY key, recorded_at, id"
                                ).fetchall():
            cards.setdefault(row[1], []).append(row[0])
        counts: dict[str, Any] = {'tasks': len(cards), 'nodes': sum(map(len, cards.values())), 'cleaned': 0, 'rows': 0,
                                  'untraced': 0, 'stopped': False}
        # the refs to keep, on this connection only (a temp table): each node's rows are checked against it in SQLite
        unkept = 'FROM learning_sources WHERE node_id = ? AND source_ref NOT IN (SELECT ref FROM temp.card_keep)'
        done = 0
        dropped = finished = False
        try:
            conn.execute('CREATE TEMP TABLE IF NOT EXISTS card_keep (ref TEXT PRIMARY KEY)')
            for task_id, nodes in cards.items():
                keep = self.card_source_refs(task_id)
                if keep is None:
                    counts['untraced'] += len(nodes)
                    done += len(nodes)
                    progress(done, counts['nodes'])
                    continue
                conn.execute('DELETE FROM temp.card_keep')
                conn.executemany('INSERT INTO temp.card_keep VALUES (?)', [(ref,) for ref in sorted(keep)])
                for node_id in nodes:
                    if stop():
                        counts['stopped'] = True
                        return counts
                    if not dropped and conn.execute(f'SELECT EXISTS (SELECT 1 {unkept})', (node_id,)).fetchone()[0]:
                        conn.execute('DROP INDEX IF EXISTS learning_source_ref')
                        dropped = True
                    removed = 0
                    if dropped:
                        try:
                            conn.execute('BEGIN IMMEDIATE')
                            removed = conn.execute(f'DELETE {unkept}', (node_id,)).rowcount
                            conn.execute('COMMIT')
                        except BaseException:
                            if conn.in_transaction:  # SQLite may have rolled back already; keep the real error
                                conn.execute('ROLLBACK')
                            raise
                    counts['cleaned'] += removed > 0
                    counts['rows'] += removed
                    done += 1
                    progress(done, counts['nodes'])
            finished = True
        finally:
            try:
                try:
                    if dropped and not finished:
                        rebuilding()                 # told first; its failure (a closed pipe) never skips the remake
                finally:
                    conn.execute(SOURCE_REF_INDEX)
            finally:
                conn.execute('DROP TABLE IF EXISTS temp.card_keep')
        return counts

    def card_inputs(self, task_id: str | None = None) -> dict[str, str]:
        """Each episode's card inputs as one digest (I3): its composer, the episode as recorded and the owner's latest
        verdict. A card read from other inputs is read again; one read from these is not."""
        sql = ('SELECT e.task_id, e.built_at, e.status, e.verified, (SELECT MAX(v.seq) FROM owner_verdicts v '
               'WHERE v.task_id = e.task_id) FROM episodes e' + (' WHERE e.task_id = ?' if task_id else ''))
        return {r[0]: digest([CARD_MAKER, r[1], r[2], r[3], r[4]])
                for r in self.db.conn.execute(sql, (task_id,) if task_id else ())}

    def cards_to_read(self, force: bool = False) -> list[str]:
        """The tasks whose card must be read (again), oldest first: no card yet, or a card read from other inputs (a
        new verdict, a rebuilt episode, an older composer). A failed read is tried once more on the next pass (a reader
        may have crashed); failed twice on the same inputs, it waits for them to change, unless `force` (`ferb memory
        consolidate`), or unless it was parked by the old redactor's idempotence refusal (`PARKED_BY_REDACTION`), which
        the structural redaction no longer makes: those are read again once. A card built on forgotten content is
        never read again."""
        current = self.card_inputs()
        recorded = {r[0]: (r[1], r[2], r[3] or '') for r in self.db.conn.execute(
            'SELECT task_id, inputs, status, error FROM learning_card_inputs')}
        gone = {r[0] for r in self.db.conn.execute("SELECT id FROM learning_forgotten WHERE id LIKE 'key-%'")}
        order = [r[0] for r in self.db.conn.execute('SELECT task_id FROM episodes ORDER BY ended_at')]
        return [t for t in order if self.card_key(t) not in gone and (
            recorded.get(t, ('', '', ''))[0] != current.get(t) or recorded[t][1] == 'failed' or
            (recorded[t][1] == 'parked' and (force or PARKED_BY_REDACTION in recorded[t][2])))]

    def card_read(self, task_id: str, inputs: str, error: str | None = None) -> None:
        """Record the inputs a card was read from, or failed to be read from with the error: 'failed' the first time
        (tried once more), 'parked' when it failed again on the same inputs."""
        status = 'read'
        if error:
            before = self.db.conn.execute('SELECT inputs, status FROM learning_card_inputs WHERE task_id=?',
                                          (task_id,)).fetchone()
            status = 'parked' if before and before[0] == inputs and before[1] in ('failed', 'parked') else 'failed'
        self.db.conn.execute('INSERT INTO learning_card_inputs VALUES (?,?,?,?) ON CONFLICT(task_id) DO UPDATE SET '
                             'inputs=excluded.inputs, status=excluded.status, error=excluded.error',
                             (task_id, inputs, status, error))

    def _action_sources(self, inp: dict[str, Any], result: Any) -> set[str]:
        """The sources of one recorded tool call's result (memory/lineage.py)."""
        return call_sources(inp, result)

    def current_card(self, task_id: str) -> dict[str, Any] | None:
        """A task's current episode card, by its key (the task id)."""
        row = self.db.conn.execute("SELECT id FROM learning_nodes WHERE kind='episode' AND key=? AND valid_to IS NULL "
                                   "AND state NOT IN ('retired','rolled_back') ORDER BY recorded_at DESC,rowid DESC "
                                   "LIMIT 1", (task_id,)).fetchone()
        return self.get(row[0]) if row else None

    def _card_labels(self, refs: Iterable[str]) -> tuple[set[str], list[str], list[str], list[str]]:
        """The origins of a card's sources, and its missing sources by why they are missing (C1):
        - `forgotten`: an explicit owner forget deleted it (its tombstone): the card is built on forgotten content;
        - `damaged`: it holds the redaction marker. No FERB id contains '[' or ']', so an earlier redaction altered it
          (60 old rows in the owner's journal); it never resolves and is dropped;
        - `expired`: any other, removed by retention (aged out); it stays an edge (source edges survive retention).
        A damaged or expired ref adds the labels of a source whose content is unknown: never assume it was less
        private (as for a retained fact's lost event)."""
        labels: set[str] = set()
        damaged: list[str] = []
        expired: list[str] = []
        forgotten: list[str] = []
        for ref in sorted(set(refs)):
            try:
                labels |= self._source_origins(ref)
                continue
            except LearningRefused:
                pass
            if REDACTED in ref:
                damaged.append(ref)
            elif self.forgotten(ref):
                forgotten.append(ref)
                continue
            else:
                expired.append(ref)
            labels |= {origin.PRIVATE, origin.external('unlabeled')}
        return labels, damaged, expired, forgotten

    def confirm(self, node_id: str) -> dict[str, Any]:
        p = self.get(node_id)
        if not p or p['valid_to'] or p['state'] == 'rolled_back':
            raise LearningRefused('no current proposal')
        if p['kind'] == 'identity_proposal':
            self.put('identity', p['key'], p['body'], [p['id']], origin_label=origin.OWNER,
                     made_by='owner:confirmed-digest/1', state='active', owner=True)
        self._mark_confirmed(node_id)
        self.publish('memory.confirmed_derived', {'id': node_id})
        return self.get(node_id)  # type: ignore[return-value]

    def _mark_confirmed(self, node_id: str) -> None:
        """The owner's confirm on a node: a quarantined one becomes a candidate."""
        self.db.conn.execute('UPDATE learning_nodes SET confirmed=1,state=CASE WHEN state=\'quarantined\' '
                             'THEN \'candidate\' ELSE state END WHERE id=?', (node_id,))

    def edit(self, node_id: str, text: str) -> dict[str, Any]:
        p = self.get(node_id)
        if not p:
            raise LearningRefused('unknown item')
        if p['kind'] == 'episode':
            raise LearningRefused('episode evidence is immutable; retire it or edit a separate owner note')
        body = dict(p['body']) | {'text': text}
        return self.put('identity' if p['kind'] == 'identity_proposal' else p['kind'], p['key'], body, [p['id']],
                         origin_label=origin.OWNER, made_by='owner:edit/1', state='active', owner=True)

    def identity(self) -> str:
        return '\n'.join(str(p['body'].get('text', '')) for p in self.items('identity') if p['origin'] == origin.OWNER)

    def retire(self, node_id: str) -> dict[str, Any] | None:
        """The owner's "Not right" on a learned item (the app's digest): the node is retired, kept for its history."""
        self.db.conn.execute("UPDATE learning_nodes SET state='retired' WHERE id=?", (node_id,))
        return self.get(node_id)

    def digest(self) -> dict[str, Any] | None:
        pending = [p for p in self.items('identity_proposal') if not p['confirmed']]
        day = self.clock.now().astimezone().date().isoformat()
        if not pending or self.db.conn.execute('SELECT 1 FROM learning_digests WHERE day=?', (day,)).fetchone():
            return None
        ids = [p['id'] for p in pending]
        self.db.conn.execute('INSERT INTO learning_digests VALUES (?,?,?)', (day, encoded(ids), self.now()))
        out = {'day': day, 'ids': ids, 'proposals': [{'id': p['id'], 'text': p['body'].get('text', '')} for p in pending]}
        self.publish('memory.digest', out)
        return out

    def descendants(self, refs: Iterable[str]) -> set[str]:
        frontier, found = set(refs), set()
        while frontier:
            current, frontier = frontier, set()
            for ref in current:
                if self.get(ref):
                    found.add(ref)
                children = {r[0] for r in self.db.conn.execute('SELECT node_id FROM learning_sources WHERE source_ref=?',
                                                               (ref,))}
                frontier.update(children - found)
        return found

    def forget(self, refs: Iterable[str]) -> list[str]:
        """An explicit owner forget: every version of a requested node, and every node derived from the refs or those
        versions (`descendants`, walked now: cards cite direct sources only). Each requested ref and deleted node leaves
        a tombstone (`forgotten_refs`: its id, never its content), and a deleted episode card's key does too, so a later
        capture never writes it again (C1)."""
        self.generation += 1
        requested = list(refs)
        versions = set(requested)
        logical = []
        for ref in requested:
            node = self.get(ref)
            if node:
                logical.append('key-' + digest([node['kind'], node['key']]))
                versions.update(r[0] for r in self.db.conn.execute('SELECT id FROM learning_nodes WHERE kind=? AND key=?',
                                                                  (node['kind'], node['key'])))
        doomed = self.descendants(versions)
        rows = self.db.conn.execute(f"SELECT kind,key,body_json FROM learning_nodes WHERE id IN "
                                    f"({','.join('?' * len(doomed))})", sorted(doomed)).fetchall() if doomed else []
        jobs = set()
        for row in rows:
            if row['kind'] in ID_KEYED:                       # a card or annotation of one task: never rebuilt
                logical.append('key-' + digest([row['kind'], row['key']]))
            if row['kind'] == 'episode_intent':               # a card's label (L29): its intent's job goes too
                jobs.add('intent:' + str(json.loads(row['body_json']).get('intent_id')))
        now = self.now()
        self.db.conn.execute('SAVEPOINT learning_forget')
        try:
            for ident in logical:
                self.db.conn.execute('INSERT OR IGNORE INTO learning_forgotten VALUES (?,?)', (ident, now))
            self.db.conn.executemany('INSERT OR IGNORE INTO forgotten_refs VALUES (?,?)',
                                     [(direct(ref), now) for ref in sorted(set(requested) | doomed)])
            for ident in doomed:
                self.db.conn.execute('INSERT OR IGNORE INTO learning_forgotten VALUES (?,?)', (ident, now))
                # a card's label job (and a job keyed by the card, from before L29), whose error may name what was
                # forgotten
                self.db.conn.execute('DELETE FROM learning_jobs WHERE source_ref IN (?,?)', (ident, 'label:' + ident))
                self.db.conn.execute('DELETE FROM learning_nodes WHERE id=?', (ident,))
            # the jobs of the intents whose labels go (L29): an error may quote what the call was sent, and the
            # intent's runs changed anyway
            self.db.conn.executemany('DELETE FROM learning_jobs WHERE source_ref=?', [(j,) for j in sorted(jobs)])
            self.db.conn.execute('RELEASE learning_forget')
        except BaseException:
            self.db.conn.execute('ROLLBACK TO learning_forget')
            self.db.conn.execute('RELEASE learning_forget')
            raise
        # The indexes zero a deleted row's terms themselves (FTS5 secure-delete, migration 0037): no rebuild here.
        self.db.truncate_wal('memory.forget')
        self.publish('memory.forgot_derived', {'ids': sorted(doomed)})
        return sorted(doomed)

    def drop_fact_index(self, ref: str, memory: Any) -> list[str]:
        """The owner's Drop on one of FERB's guesses (L36 §2): the fact nodes that index that memory item `ref`
        (`sync_facts`, every version) are deleted, and nothing else. Never `forget`: what was built on the guess while
        it existed (the episode cards of tasks whose memory reads returned it, the summaries and intent labels built on
        those cards, an entity summary) stays, and no tombstone is left, so a later capture of such a card counts the
        guess as a missing source, never a forgotten one (`_card_labels`). Live 2026-10-25: one Drop through `forget`
        deleted 750 learned items, 406 of them episode cards, in 53 s on the event loop.
        A node a deleted one had closed (`supersedes`: a guess indexed before review Q-I3 closed a kept version of its
        fact) is reopened while its memory item still holds (`prompt_ready`) and nothing else indexes that item."""
        from ferb.memory.items import prompt_ready
        rows = self.db.conn.execute(
            "SELECT n.id, n.supersedes FROM learning_nodes n JOIN learning_sources s ON s.node_id = n.id "
            "WHERE s.source_ref = ? AND n.kind = 'fact'", (ref,)).fetchall()
        doomed = sorted(r[0] for r in rows)
        if not doomed:
            return []
        reopen = []
        for closed in sorted({r[1] for r in rows if r[1] and r[1] not in doomed}):
            node = self.get(closed)
            item_refs = [s for s in (node or {}).get('sources', []) if s.startswith('memory:')]
            if node is None or node['kind'] != 'fact' or node['valid_to'] is None or len(item_refs) != 1:
                continue
            item = memory.get(item_refs[0].removeprefix('memory:'))
            if item is None or not prompt_ready(item, self.clock.now()) or self.db.conn.execute(
                    "SELECT 1 FROM learning_nodes n JOIN learning_sources s ON s.node_id = n.id WHERE s.source_ref = ? "
                    "AND n.kind = 'fact' AND n.valid_to IS NULL", (item_refs[0],)).fetchone():
                continue
            reopen.append(closed)
        self.generation += 1                          # a card read before now may cite a node that is gone
        self.db.conn.execute('SAVEPOINT learning_drop')
        try:
            self.db.conn.executemany('DELETE FROM learning_nodes WHERE id=?', [(i,) for i in doomed])
            self.db.conn.executemany('UPDATE learning_nodes SET valid_to=NULL WHERE id=?', [(i,) for i in reopen])
            self.db.conn.execute('RELEASE learning_drop')
        except BaseException:
            self.db.conn.execute('ROLLBACK TO learning_drop')
            self.db.conn.execute('RELEASE learning_drop')
            raise
        self.publish('memory.forgot_derived', {'ids': doomed})
        return doomed

    def rollback(self, batch: str) -> list[str]:
        rows = list(self.db.conn.execute('SELECT * FROM learning_changes WHERE batch_id=? AND rolled_back=0 '
                                         'ORDER BY seq DESC', (batch,)))
        changed = []
        for r in rows:
            if self.get(r['node_id']):
                for ident in self.descendants([r['node_id']]):
                    if ident != r['node_id'] and self.get(ident)['origin'] == origin.OWNER:
                        continue  # a later owner statement is independent authority
                    self.db.conn.execute("UPDATE learning_nodes SET valid_to=?,state='rolled_back' WHERE id=?",
                                         (self.now(), ident))
                if r['prior_id'] and self.get(r['prior_id']):
                    # Reopen only the immediate predecessor this batch closed, and only without a newer edit.
                    prior = self.get(r['prior_id'])
                    newer = self.db.conn.execute('SELECT 1 FROM learning_nodes WHERE kind=? AND key=? '
                        'AND valid_to IS NULL', (prior['kind'], prior['key'])).fetchone()
                    if not newer:
                        self.db.conn.execute('UPDATE learning_nodes SET valid_to=NULL WHERE id=?', (r['prior_id'],))
                changed.append(r['node_id'])
            self.db.conn.execute('UPDATE learning_changes SET rolled_back=1 WHERE seq=?', (r['seq'],))
        self.publish('memory.rollback', {'batch': batch, 'ids': changed})
        return changed

    def weighted_terms(self, words: list[str]) -> list[str]:
        """The query words that carry weight (L36 §3; `items.weighted`, the rule `Memory.recall` uses): a word found in
        more than half of the indexed nodes says nothing about any one of them (data-driven, no word list); the floor of
        2 keeps a shared topic useful in a small store. Nodes indexed with no text (rollups, intent labels) don't
        count."""
        total = int(self.db.conn.execute("SELECT COUNT(*) FROM learning_fts WHERE text != ''").fetchone()[0])

        def matches(term: str) -> int:
            return int(self.db.conn.execute('SELECT COUNT(*) FROM learning_fts WHERE learning_fts MATCH ?',
                                            ('"' + term.replace('"', '""') + '"',)).fetchone()[0])
        return weighted(words, total, matches)

    def search(self, query: str, *, kind: str | None = None, app: str | None = None,
               person: str | None = None, since: str | None = None, until: str | None = None,
               query_vector: list[int] | None = None, embed: bool = True, limit: int | None = None,
               leave_out: frozenset[str] = frozenset(), sources: bool = True,
               focus: str = '') -> list[dict[str, Any]]:
        """The one ranked search (L36 §3): bm25 over each node's content (`search_text`) for the words that carry
        weight (`weighted_terms`), the trigram title match and, when the owner adopted one, embedding similarity,
        fused by reciprocal rank (each ranking adds 1 / (rank + 1), ranks from 0; equal scores share a rank: `_tied`),
        plus a small recency boost; at equal score the owner's own statement first. A query whose every word most nodes
        share finds nothing, by similarity too (m5); a query with no words lists the newest first, with
        no recency or owner boost. `leave_out`: kinds dropped before `limit` (the budget) applies; leaving out
        identity also leaves out the fact nodes of what identity holds (`_identity_twins`, final review M1). An
        episode without a stored goal is left out. `query_vector`: the query's embedding, computed off the event loop
        (L28); `embed` False: keyword ranking only (the caller's encode failed and was reported). `sources` False:
        nodes without their source refs (`get`), for a prompt's lines. `focus`: a task's target words (`target_words`),
        also ranked alone by bm25 with the same weight rule and fused like the others, so a short fact that shares only
        the site's name with a long goal isn't buried by long texts sharing the goal's ordinary words (L36 §4 fix
        round); its ties take competition ranks, so a common target word ("Chrome") can't fill the budget with one big
        tied group (fix round 2); focus words that carry no weight or match nothing change nothing. The filters, the
        owner's order and the budget apply after the fusion, as without it."""
        words = query_terms(query)
        terms = self.weighted_terms(words) if words else []
        if words and not terms:
            return []
        rankings: list[dict[str, int]] = []
        if terms:
            rankings.append(self._bm25(terms))
            focused = self.weighted_terms(query_terms(focus)) if focus else []
            if focused:                   # competition ranks: a common target word's big tied group stays one rank
                rankings.append(self._bm25(focused, positional=True))
            if len(query) >= 3:
                rankings.append(_tied(self.db.conn.execute(
                    'SELECT id, bm25(learning_titles) FROM learning_titles WHERE learning_titles MATCH ? ORDER BY 2',
                    ('"' + query.replace('"', '""') + '"',))))
        else:                                     # no words: the current nodes, newest first (ids only, as `items`)
            listed = self.db.conn.execute("SELECT id FROM learning_nodes WHERE valid_to IS NULL AND state NOT IN "
                                          "('retired','rolled_back') AND (? IS NULL OR kind = ?) "
                                          "ORDER BY recorded_at DESC, id DESC", (kind or None, kind or None)).fetchall()
            rankings.append({r[0]: n for n, r in enumerate(listed)})
        if self.embedder is not None and query and embed:
            from ferb.memory.embeddings import ranked
            try:
                rankings.append({ident: n for n, ident in enumerate(ranked(self.db, self.embedder, query,
                                                                             vector=query_vector))})
            except Exception as e:
                self.publish('memory.embedding_unavailable', {'error': self.redact(str(e))})
        # Reciprocal ranks fuse retrieval methods; age only nudges near-equal matches, never deletes.
        scores: dict[str, float] = {}
        for ranking in rankings:
            for ident, n in ranking.items():
                scores[ident] = scores.get(ident, 0.0) + 1 / (n + 1)
        # Ranked by each node's columns first; bodies are read best first only until the budget is full (review fix 5:
        # a planner query matched 698 of the owner's nodes, and reading every body and its sources took 1.4 s a turn)
        now = self.clock.now()
        twins = self._identity_twins() if 'identity' in leave_out else set()
        order: list[tuple[float, bool, str, str, float]] = []
        for row in self._columns(scores):
            if row['valid_to'] or row['state'] in ('retired', 'rolled_back', 'proposed') or \
                    (kind and row['kind'] != kind) or row['kind'] in leave_out or row['id'] in twins:
                continue                          # 'proposed': a memory item waiting for the owner's Keep
            if row['kind'] == 'episode_intent':   # a card's intent label (L29): bookkeeping, never recalled
                continue
            if since and row['valid_from'] < since or until and row['valid_from'] > until:
                continue
            at = datetime.fromisoformat(row['recorded_at'])
            at = at if (at.tzinfo is None) == (now.tzinfo is None) else at.replace(tzinfo=now.tzinfo)
            age = 0.0 if row['origin'] == origin.OWNER else max(0.0, (now - at).total_seconds() / 86400)
            boost = RECENCY_WEIGHT * math.pow(0.5, age / RECENCY_HALF_LIFE_DAYS) if words else 0.0   # a listing: newest first
            score = scores[row['id']] + boost
            order.append((round(score, 9), bool(words) and row['origin'] == origin.OWNER, row['recorded_at'], row['id'], score))
        order.sort(reverse=True)
        out: list[dict[str, Any]] = []
        for *_, ident, score in order:
            if limit is not None and len(out) >= limit:
                break
            p = self.get(ident, sources)
            if p is None:
                continue
            body = p['body']
            if p['kind'] == 'episode' and not str(body.get('intent') or '').strip():
                continue
            if app and app not in body.get('apps', {}) and body.get('scope', {}).get('app') != app and \
                    app not in body.get('preconditions', {}).get('apps', {}):
                continue
            if person and person.casefold() not in encoded(body).casefold():
                continue
            p['score'] = score
            out.append(p)
        return out

    def _identity_twins(self) -> set[str]:
        """The open fact nodes of the memory items the identity layer already holds (an owner-stated preference about
        the owner: `sync_facts` indexes it as a fact and as identity). A search that leaves identity out leaves these
        out too: identity is in every role's prompt and never takes a slot of the 60/20 (L36 ruling "Identity", final
        review M1)."""
        return {r[0] for r in self.db.conn.execute(
            "SELECT f.node_id FROM learning_sources f JOIN learning_nodes n ON n.id = f.node_id "
            "WHERE n.kind = 'fact' AND n.valid_to IS NULL AND f.source_ref IN ("
            "SELECT s.source_ref FROM learning_sources s JOIN learning_nodes i ON i.id = s.node_id "
            "WHERE i.kind = 'identity' AND i.origin = ? AND i.valid_to IS NULL "
            "AND i.state NOT IN ('retired','rolled_back') AND s.source_ref LIKE 'memory:%')", (origin.OWNER,))}

    def _bm25(self, terms: list[str], *, positional: bool = False) -> dict[str, int]:
        """The nodes whose content holds any of `terms`, ranked by bm25 (ties share a rank: `_tied`)."""
        match = ' OR '.join('"' + t.replace('"', '""') + '"' for t in terms)
        return _tied(self.db.conn.execute(
            'SELECT id, bm25(learning_fts) FROM learning_fts WHERE learning_fts MATCH ? ORDER BY 2', (match,)),
            positional=positional)

    def _columns(self, ids: Iterable[str]) -> list[Any]:
        """The columns search ranks and filters by, for these nodes (no body), in chunks SQLite's variable limit
        allows."""
        ids = list(ids)
        rows: list[Any] = []
        for at in range(0, len(ids), 500):
            chunk = ids[at:at + 500]
            rows += self.db.conn.execute('SELECT id, kind, origin, state, valid_to, valid_from, recorded_at FROM '
                                         f"learning_nodes WHERE id IN ({','.join('?' * len(chunk))})", chunk).fetchall()
        return rows

    def sync_facts(self, memory: Any) -> None:
        """Index the memory items as fact nodes: those that may shape prompts (`prompt_ready`; an unconfirmed hint says
        so in its body), and inferred ones waiting for the owner's Confirm in state 'proposed', which search (so every
        prompt) leaves out but the dashboard lists and confirms. Close the nodes of items that are neither. Only an
        owner_stated preference about the owner becomes identity (whatever was on screen when they typed it: owner
        decision 2026-10-20)."""
        from ferb.memory.items import current, prompt_ready
        now = self.clock.now()                        # an item with an end date holds until then (L36 §6)

        def pending(item: Any) -> bool:
            return current(item, now) and item.status == 'proposed' and item.origin == 'inferred'
        for node in self.items('fact') + self.items('identity'):     # only what it syncs: never every label (L29)
            if not any(s.startswith('memory:') for s in node['sources']):
                continue
            source_id = next(s.removeprefix('memory:') for s in node['sources'] if s.startswith('memory:'))
            source = memory.get(source_id)
            if source is None or not (prompt_ready(source, now) or (node['kind'] == 'fact' and pending(source))):
                self.db.conn.execute('UPDATE learning_nodes SET valid_to=? WHERE id=?', (self.now(), node['id']))
        for item in memory.items(status=None, valid_now=False):
            if not (prompt_ready(item, now) or pending(item)):
                continue
            body = asdict(item)
            body['unconfirmed_hint'] = item.status == 'proposed'
            state = 'proposed' if pending(item) else 'active'
            try:
                # A waiting guess's node closes nothing (review Q-I3: it hid a kept version of its fact from every
                # search); the guess replaces a version only once kept, through its item's own versioning
                node = self.put('fact', f'{item.subject}/{item.key}', body, ['memory:' + item.id],
                                origin_label=memory_origin(item.origin), made_by='code:fact-index/1',
                                state=state, owner=item.origin == 'owner_stated', supersede=state != 'proposed')
                if state == 'proposed':                  # its own older node alone is replaced (more evidence)
                    self.db.conn.execute(
                        "UPDATE learning_nodes SET valid_to=? WHERE kind='fact' AND valid_to IS NULL AND id != ? AND "
                        "id IN (SELECT node_id FROM learning_sources WHERE source_ref=?)",
                        (self.now(), node['id'], 'memory:' + item.id))
                # `put` returns a node already there unchanged: one stored 'active' before pending items were indexed
                # 'proposed' kept reaching every prompt. Its state follows its item (the body fixes which), but an
                # owner's Retire, a rollback or a closed version is never undone here.
                if node['state'] in ('active', 'proposed') and node['state'] != state and node['valid_to'] is None:
                    self.db.conn.execute('UPDATE learning_nodes SET state=? WHERE id=?', (state, node['id']))
                if item.origin == 'owner_stated' and item.kind == 'preference' and item.subject == 'owner':
                    self.put('identity', item.key, {'text': item.content}, ['memory:' + item.id],
                             origin_label=origin.OWNER, made_by='owner:durable-preference/1', state='active', owner=True)
            except LearningRefused:
                pass  # a forgotten derived item is deliberately never reconstructed


def index_summaries(learning: Learning, stop: Callable[[], bool] = lambda: False) -> None:
    """Index each day summary and narrative for search (the daily distill's output as `activity_summary` nodes). A
    day is indexed again only when its journal row changed (a rebuilt summary has a new one), so a pass indexes only
    new days. `stop`, asked before each day, ends the pass there. (No other rollups: the day/week/month episode and
    entity summaries went with procedure learning, notes/memory.md.)"""
    from ferb.memory.summary import NARRATIVE_KIND, SUMMARY_KIND, build
    l = learning
    for row in l.db.conn.execute('SELECT id,kind,data_json FROM journal WHERE kind IN (?,?)',
                                 (SUMMARY_KIND, NARRATIVE_KIND)).fetchall():
        data = json.loads(row['data_json'])
        day = data.get('day')
        if not day or l.db.conn.execute(
                "SELECT 1 FROM learning_nodes n JOIN learning_sources s ON s.node_id = n.id WHERE n.kind = "
                "'activity_summary' AND n.key = ? AND n.valid_to IS NULL AND s.source_ref = ?",
                (row['kind'] + ':' + day, 'journal:' + row['id'])).fetchone():
            continue                             # indexed from this very row already
        if stop():
            return
        raw_refs = {kind + ':' + ident for refs in build(l.db, day).handles.values() for kind, ident in refs}
        try:
            l.put('activity_summary', row['kind'] + ':' + day, data, ['journal:' + row['id'], *sorted(raw_refs)],
                  made_by=str(data.get('made_by') or 'code:summary-index/1'), state='active')
        except LearningRefused:
            continue
