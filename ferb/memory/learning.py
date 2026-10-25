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
from typing import Any, Callable, Iterable, Iterator, NamedTuple

from ferb.core import origin
from ferb.core.errors import FerbError
from ferb.memory.items import query_terms, weighted
from ferb.memory.lineage import PROCEDURE_LABEL, REQUEST_REFERENCE, call_sources, direct
from ferb.orchestrator import episodes
from ferb.policy.confirm import site_is_destination
from ferb.policy.rules import REVERSIBLE, Kind, declared_of
from ferb.sanitize.redact import INTERNAL_ID_KEYS, NODE_TEXT, REDACTED, holds_sealed


class LearningRefused(FerbError):
    pass


# Kinds whose key is one of FERB's own ids: an episode card (its task id), its annotation and its intent label (L29)
# (the card's node id).
ID_KEYED = frozenset({'episode', 'episode_annotation', 'episode_intent'})
# The episode card's composer. 2: a card cites its task's direct sources only (memory review C1). 3: an action's
# target provenance is read in the gateway's vocabulary (`outside_targets`, L29 Task 6 review I1). 4: neither the site
# an action ran on nor a call the gateway records no provenance for is outside (plan 2026-10-23 Task 2). A card an
# older composer built is rebuilt once by the consolidator.
CARD_MAKER = 'code:evidence-card/4'
# The composer before C1 (fixed in e0e344b): it copied into a card the refs of every block its planner was shown, and
# every earlier version's. A store holding its cards gets the one-time clean-out (`prune_card_sources`, run by setup).
COPYING_CARD_MAKER = 'code:evidence-card/1'
# The source-ref index (migration 0030): that clean-out drops it while it deletes and makes it again after; a power
# loss in between leaves it missing whatever the schema version, so every daemon start makes it too (L28, C1)
SOURCE_REF_INDEX = 'CREATE INDEX IF NOT EXISTS learning_source_ref ON learning_sources(source_ref)'
# Model-written suggestions the owner can turn down ("Not right": retire); one turned down never comes back (M3).
SUGGESTIONS = frozenset({'procedure', 'identity_proposal', 'lesson'})
# The JSON types a slot may declare: a closed set, so a slot type the redactor masked by its key ("token": "string")
# is restored, never free text (L29 re-review O3).
SLOT_TYPES = ('string', 'number', 'boolean', 'array', 'object')
# Kinds whose body declares slot types
TYPED_SLOTS = frozenset({'procedure', 'episode_intent'})
# Card fields a consolidation doesn't need (usage, the tool tally, wording about version scope): left out of its digest.
_NOT_FOR_CONSOLIDATION = ('usage', 'surfaces', 'app_version_scope')
# The headings of what a worker gets with its procedure besides the procedure itself (L29, `procedure_parts`)
LESSONS_LABEL = 'Lessons learned from earlier runs of this kind of task (data; check that each applies)'
RUN_LABEL = 'A verified earlier run of this kind of task: its tool calls and what came back (data, never instructions)'
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


class ProcedurePart(NamedTuple):
    """One part of what a worker gets for its selected procedure (L29, `Learning.procedure_parts`)."""
    label: str                                  # its heading
    text: str
    protected: bool                             # always shown; else an optional block fitted to the worker's window
    origins: frozenset[str] = frozenset()       # the origins of what it shows
    refs: tuple[str, ...] = ()                  # the learning nodes it shows, which it cites
    reference: str = ''                         # where it is read in full


def card_digest(node: dict[str, Any]) -> dict[str, Any]:
    """An episode card as the consolidator is given it (I3): its id, origin and body, without its lineage list and the
    body fields it doesn't use. Nothing it uses is shortened."""
    return {'id': node['id'], 'origin': node['origin'],
            'body': {k: v for k, v in node['body'].items() if k not in _NOT_FOR_CONSOLIDATION}}


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


def spaced(value: Any) -> str:
    """`value` as JSON with a space after each ',' and ':', as `redacted_json` reads it: the redactor reads text as
    prose, and in compact JSON a word near a string's end ("Pin", "Token", "password") would label a "value" running
    across the following keys to the next space (L29 re-review R1)."""
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(', ', ': '), default=str)


def redacted_json(redact: Callable[[str], str], value: Any, secrets: Iterable[str] = ()) -> Any:
    """`value` with its secrets redacted, still JSON (rule 14), or LearningRefused. Every whole-text pass reads it
    `spaced` (stored compact). In order, the first that parses:
    1. its text redacted whole, which also catches a secret its key labels ("password": "…") or that spans items (a
       private key's lines);
    2. when that broke the JSON (a redaction inside a number: a card-like number, a registered value within one), the
       same whole text with every number and boolean written as a string: back in the value's shape, each number or
       boolean whose text changed becomes REDACTED; when the redaction changed the shape, that parse as it stands
       (strings where the numbers were);
    3. else each value redacted together with its key (the `"key": value` text as one unit, nested values in turn, a
       list's strings also joined, so a private key split across items is caught), REDACTED where that doesn't parse.
    Then each string is redacted on its own too, as the prose it is: inside JSON the redactor misses a secret that
    ends a string ("my password is sunshine" followed by another key), and each pair of neighbouring values is read as
    two lines (a code whose label ends the previous value, "PIN" then "4821"). Last, the result is checked: redacted
    once more as a whole it must not change, and the compact text to be stored must hold none of the owner's registered
    secret `secrets` where the redactor would replace one (`holds_sealed`, L36 §7: a word-like value as a whole word,
    never strictly inside one of FERB's id bodies; one spanning JSON punctuation, e.g. `1234,5678` in `[1234,5678]`,
    matches only there). Else it is refused, as before this helper existed (L29 re-review N1, R1, Q1)."""
    try:
        out = json.loads(redact(spaced(value)))
    except ValueError:
        try:
            seen = json.loads(redact(spaced(_quoted(value))))
        except ValueError:
            out = _units(redact, value, None)
        else:
            try:
                out = _restored(value, seen)
            except ValueError:
                out = seen
    out = _siblings(redact, _strings_alone(redact, out))
    text, stored = spaced(out), encoded(out)
    if redact(text) != text or holds_sealed(stored, secrets):
        raise LearningRefused('could not be stored safely: redaction would still change it')
    return out


def _line(value: Any) -> str | None:
    """A scalar as a line of prose: a string as written, a number as JSON; None for anything else."""
    if isinstance(value, str):
        return value
    return encoded(value) if isinstance(value, (int, float)) and not isinstance(value, bool) else None


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
    if kind == 'procedure':
        return words(body.get('intent'), body.get('ui_hints'))
    if kind == 'lesson':
        return words(body.get('trigger'), body.get('text'))
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
    if kind == 'procedure':
        return _one_line(body.get('intent') or key)
    if kind == 'lesson':
        return f"when {_one_line(body.get('trigger'))}: {_one_line(body.get('text'))}"
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


def _ratings(lesson: dict[str, Any]) -> str:
    """A lesson's helpful/harmful ratings from the owner as a worker is shown them, when it has any (review M4); a count
    that isn't a number counts as none."""
    helpful, harmful = (v if isinstance(v := lesson.get(k), int) and not isinstance(v, bool) else 0
                        for k in ('helpful', 'harmful'))
    return f' (you rated it: {helpful} helpful, {harmful} harmful)' if helpful or harmful else ''


def _node_text(item: Any) -> str | None:
    """A list item's own text: a node's first text field (`NODE_TEXT`, as the redactor reads a page's nodes)."""
    if not isinstance(item, dict):
        return None
    return next((k for k in NODE_TEXT if isinstance(item.get(k), str) and item[k].strip()), None)


# Keys whose values are FERB's own ids (an action's id, a label's intent id, a procedure example's card, step evidence):
# never read as a neighbour or masked by that reading (L29 re-review 5, V1); the other passes still redact them.
_SIBLING_ID_KEYS = frozenset(INTERNAL_ID_KEYS | {'id', 'intent_id', 'episode', 'evidence'})


def _siblings(redact: Callable[[str], str], value: Any) -> Any:
    """`value` with each pair of neighbouring scalar values (an object's in key order, a list's items, a list's nodes by
    their text) also read as two lines of prose, as the redactor reads a page's neighbouring nodes: a value that
    reading masks becomes REDACTED (a code whose label ends the previous value: "PIN" then "4821"; L29 re-review Q1)."""
    ids: set[int] = set()
    if isinstance(value, dict):
        keys: list[Any] | None = sorted(value, key=str)
        ids = {i for i, k in enumerate(keys or ()) if k in _SIBLING_ID_KEYS}
        values = [value[k] if i in ids else _siblings(redact, value[k]) for i, k in enumerate(keys or ())]
        texts: list[str | None] = [None] * len(values)
    elif isinstance(value, list):
        keys, values = None, [_siblings(redact, v) for v in value]
        texts = [_node_text(v) for v in values]
    else:
        return value
    lines = [None if i in ids else values[i][t] if t is not None else _line(values[i])
             for i, t in enumerate(texts)]
    # a string is already redacted on its own (`_strings_alone`); a number is read alone here
    alone = [redact(line) if isinstance(values[i], (int, float)) and texts[i] is None and line is not None else line
             for i, line in enumerate(lines)]
    out = list(values)
    for i in range(1, len(values)):
        a, b = lines[i - 1], lines[i]
        if a is None or b is None:
            continue
        pair = redact(a + '\n' + b)
        if pair == f'{alone[i - 1]}\n{alone[i]}':
            continue
        parts = pair.split('\n')
        if len(parts) != a.count('\n') + b.count('\n') + 2:
            out[i - 1], out[i] = _masked(out[i - 1], texts[i - 1]), _masked(out[i], texts[i])
            continue
        cut = a.count('\n') + 1
        if '\n'.join(parts[cut:]) != alone[i]:
            out[i] = _masked(out[i], texts[i])
        if '\n'.join(parts[:cut]) != alone[i - 1]:
            out[i - 1] = _masked(out[i - 1], texts[i - 1])
    return dict(zip(keys, out)) if keys is not None else out


def _masked(value: Any, text: str | None) -> Any:
    """A value the sibling reading masked: REDACTED, or for a node its text field."""
    return value | {text: REDACTED} if text is not None else REDACTED


def slot_types_restored(given: dict[str, Any], body: dict[str, Any]) -> dict[str, Any]:
    """`body` (`given` redacted) with each slot type the redactor masked put back: a type is one of `SLOT_TYPES`, never
    free text, so a slot named like a credential ("token", "pin") keeps its type (L29 re-review O3)."""
    declared, kept = given.get('slots'), body.get('slots')
    if not isinstance(declared, dict) or not isinstance(kept, dict):
        return body
    return body | {'slots': {k: declared[k] if declared.get(k) in SLOT_TYPES else v for k, v in kept.items()}}


def _strings_alone(redact: Callable[[str], str], value: Any) -> Any:
    """`value` with each string redacted on its own."""
    if isinstance(value, dict):
        return {k: _strings_alone(redact, v) for k, v in value.items()}
    if isinstance(value, list):
        return [_strings_alone(redact, v) for v in value]
    return redact(value) if isinstance(value, str) else value


def _quoted(value: Any) -> Any:
    if isinstance(value, dict):
        return {str(k): _quoted(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_quoted(v) for v in value]
    if value is None or isinstance(value, str):
        return value
    return encoded(value) if isinstance(value, (bool, int, float)) else str(value)


def _restored(value: Any, seen: Any) -> Any:
    """`value` with the redacted forms `seen` holds (`_quoted(value)` encoded, so with its keys sorted, and redacted),
    or ValueError when the shapes differ."""
    if isinstance(value, dict):
        if not isinstance(seen, dict) or len(seen) != len(value):
            raise ValueError('the redacted text changed the shape')
        ordered = sorted(value.items(), key=lambda item: str(item[0]))
        return {k: _restored(v, w) for (_, v), (k, w) in zip(ordered, seen.items())}
    if isinstance(value, (list, tuple)):
        if not isinstance(seen, list) or len(seen) != len(value):
            raise ValueError('the redacted text changed the shape')
        return [_restored(v, w) for v, w in zip(value, seen)]
    if value is None:
        return None if seen is None else REDACTED
    if isinstance(value, (bool, int, float)):
        return value if seen == encoded(value) else REDACTED
    if not isinstance(seen, str):
        raise ValueError('the redacted text changed the shape')
    return seen


def _units(redact: Callable[[str], str], value: Any, key: str | None) -> Any:
    """`redacted_json`'s last resort: each value with its key (`key`, None at the top) as one redacted unit."""
    if isinstance(value, dict):
        return {_unit(redact, None, str(k)): _units(redact, v, str(k)) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        texts = [v for v in value if isinstance(v, str)]
        joined = '\n'.join(texts)
        if len(texts) > 1 and redact(joined) != '\n'.join(redact(t) for t in texts):
            return REDACTED                 # a secret only the items together show (a private key's lines)
        return [_units(redact, v, key) for v in value]
    return _unit(redact, key, value)


def _unit(redact: Callable[[str], str], key: str | None, value: Any) -> Any:
    """One scalar redacted beside its key (`{"key": value}` as one text), and a string alone too; REDACTED when the
    redacted text doesn't parse back to one value, or a number, boolean or null changed."""
    if not isinstance(value, (str, bool, int, float)) and value is not None:
        value = str(value)
    try:
        seen = json.loads(redact(spaced(value if key is None else {key: value})))
    except ValueError:
        return REDACTED
    if key is not None:
        if not isinstance(seen, dict) or len(seen) != 1:
            return REDACTED
        seen = next(iter(seen.values()))
    if isinstance(value, str):
        return redact(seen) if isinstance(seen, str) else REDACTED
    return value if type(seen) is type(value) and seen == value else REDACTED


# Where an action's target value appeared, as the gateway records it (`policy.provenance.arg_sources`, its only
# writer since F3): in the owner's typed words or answers, the same set `policy/confirm.py` reads (OWNER_SOURCES)
_OWNER_SOURCES = frozenset({'owner_words', 'owner_answer'})
# Found in neither the owner's words nor outside content: chosen by FERB or the model, or an outside value code can't
# match (spelled differently, read from a picture); not outside, as confirmations read it (Task 6 re-review 2, N3)
_NEITHER = 'unknown'
_READS = ('read', 'search', 'mailbox_read')
# The target key read as outside only when it is where the call sends (`confirm.site_is_destination`): the site an
# action ran on. Under L2 FERB reaches any site without a question, and the page FERB read names its own address (plan
# 2026-10-23 Task 2, owner); a post or submit to a site only outside content named still holds (fix round 1, SQ7).
_SITE = 'site'


def outside_targets(arg_sources: Any, *, read_site: bool = False) -> bool:
    """Whether an action's targets came from outside content (MP2, F8 quarantine), from the gateway's `arg_sources`
    (key -> labels): a key the owner typed or answered is the owner's; "unknown" is not outside (as confirmations read
    it, `policy/confirm.py`); any other label (an outside origin, or one it doesn't know) on a key without an owner
    label is outside. The site key counts only with `read_site`: when the site is where the call sends. No
    provenance (None) or provenance in another shape (a damaged record) never clears a target: outside
    (`outside_action` reads a call the gateway records none for). Provenance only, never intent (R2; L29 Task 6 review
    I1: this read origin labels, so every target looked outside; plan 2026-10-23 Task 2: the site held nearly every
    browser procedure)."""
    if not isinstance(arg_sources, dict):
        return True
    return any(not (_OWNER_SOURCES & set(labels)) and any(label != _NEITHER for label in labels)
               for labels in (v if isinstance(v, list) else [v] for k, v in arg_sources.items()
                              if k != _SITE or read_site))


def outside_action(action: dict[str, Any]) -> bool:
    """Whether a card's action was built on outside content (F8): any kind but a read, whose targets
    `outside_targets` reads as outside, its site too when the site is where it sends (its kind or declared purpose is
    send-class and it names no recipient or contact: `confirm.site_is_destination`). A call without `arg_sources` that
    the gateway records none for (`gateway.py`: a kind in REVERSIBLE without a declared purpose) is FERB's own choice,
    not outside (plan 2026-10-23 Task 2); any other call without them (recorded before F3, or damaged) is outside, so
    an old send keeps its hold. The declared purpose is read with `rules.declared_of`, the gateway's own reader. A
    damaged record never clears (fail closed): a kind that is not a string (or none, before F0) is read as sending,
    never as a read or a reversible kind, and so is a `declared` in a shape the gateway never writes (it writes one
    kind as a string, several as a list of strings)."""
    kind, sources, declared = action.get('kind'), action.get('arg_sources'), action.get('declared')
    purposes = declared_of(declared)
    if declared is not None and not isinstance(declared, str) and not (
            isinstance(declared, list) and all(isinstance(p, str) for p in declared)):
        purposes.add(Kind.SEND)
    if not isinstance(kind, str):
        kinds = purposes | {Kind.SEND}
    elif kind in _READS or (sources is None and kind in REVERSIBLE and not purposes):
        return False
    else:
        kinds = purposes | {kind}
    return outside_targets(sources, read_site=isinstance(sources, dict) and site_is_destination(kinds, sources))


def memory_origin(label: str) -> str:
    return {'owner_stated': origin.OWNER, 'observed': origin.PRIVATE, 'inferred': origin.INFERRED,
            'third_party': origin.external('memory')}.get(label, origin.external('unlabeled'))


def shown_state(node: dict[str, Any]) -> str:
    """A node's state as a prompt shows it: a fact the owner hasn't confirmed reads as an unconfirmed hint (review M2)."""
    return 'unconfirmed hint' if node['kind'] == 'fact' and node['body'].get('unconfirmed_hint') else str(node['state'])


def render_slots(value: Any, slots: dict[str, Any]) -> Any:
    if isinstance(value, dict):
        return {k: render_slots(v, slots) for k, v in value.items()}
    if isinstance(value, list):
        return [render_slots(v, slots) for v in value]
    if isinstance(value, str):
        for key, replacement in slots.items():
            if value == '{' + key + '}':
                return replacement
        for key, replacement in slots.items():
            value = value.replace('{' + key + '}', str(replacement))
    return value


class Learning:
    def __init__(self, db: Any, clock: Any, publish: Callable[..., Any], redact: Callable[[str], str],
                 redact_id: Callable[[str], str] | None = None) -> None:
        """`redact_id`: redaction for a key that is one of FERB's own ids (ID_KEYED), by structure
        (`Redactor.ident`), so a registered value inside a task id can't alter the card's key; default `redact`."""
        self.db, self.clock, self.publish, self.redact = db, clock, publish, redact
        self.redact_id = redact_id or redact
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
            state: str = 'candidate', owner: bool = False, labels: set[str] | None = None) -> dict[str, Any]:
        """`labels`: the sources' origins when the caller already read them (`labels`), e.g. on a read connection off
        the event loop; otherwise they are read here."""
        if kind == 'identity' and not owner:
            raise LearningRefused('only the owner can write identity')
        if kind in SUGGESTIONS and not owner and self.db.conn.execute(
                "SELECT 1 FROM learning_nodes WHERE kind=? AND state='retired' AND (key=? OR "
                "(json_extract(body_json, '$.text') IS NOT NULL AND json_extract(body_json, '$.text')=?)) LIMIT 1",
                (kind, key, str(body.get('text', '')))).fetchone():
            raise LearningRefused('the owner said this suggestion was not right')
        if origin_label == origin.OWNER and not owner:
            raise LearningRefused('a derived record cannot invent owner authority')
        given, body = body, redacted_json(self.redact, body, self.secret_values())
        if kind in TYPED_SLOTS:
            body = slot_types_restored(given, body)
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
                                    'ORDER BY recorded_at DESC,rowid DESC LIMIT 1', (kind, key)).fetchone()
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
            outside = outside_action(inp)
            outcome_facts = {k: result[k] for k in ('ok', 'status', 'intent_id', 'action_id', 'ledger_id')
                             if isinstance(result, dict) and k in result}
            actions.append({'id': span.id, 'tool': inp.get('tool'), 'args': args,
                            'kind': inp.get('kind'), 'outside_action': outside,
                            'arg_sources': arg_sources, 'error': span.error,
                            'result': outcome_facts}
                           # its declared purpose as the gateway recorded it, so a read from the card judges its site
                           # the same (`outside_action`); absent on cards before composer 4: read as undeclared
                           | ({'declared': inp['declared']} if inp.get('declared') else {}))
        feedback = [dict(r) for r in self.db.conn.execute(
            'SELECT verdict,note,at FROM owner_verdicts WHERE task_id=? ORDER BY seq', (task_id,))]
        valid = ep['verified'] is True and not (feedback and feedback[-1]['verdict'] == 'fail')
        body = {'task_id': task_id, 'request': ep['request_text'], 'intent': ep['goal'], 'slots': {},
                'apps': {a: (apps or {}).get(a, 'unknown') for a in used_apps},
                'app_versions_captured': apps is not None, 'app_version_scope': 'identified apps and browser inventory', 'surfaces': ep['tools'], 'actions': actions,
                'status': ep['status'], 'verified': valid, 'conditions': ep['conditions'],
                'feedback': feedback, 'narrative': ep['answer'], 'problems': ep['problems'],
                'started_at': ep['started_at'], 'ended_at': ep['ended_at'], 'usage': ep['usage']}
        old = self.current_card(task_id)
        if old and apps is None:
            # A verdict/retention refresh keeps the versions actually captured at completion.
            body['apps'] = old['body'].get('apps', {})
            body['app_versions_captured'] = old['body'].get('app_versions_captured', False)
            if not actions:
                # its trace aged out: the actions as stored, each flag read again from its stored provenance (Task 6
                # re-review 2, N2: a card an earlier composer built read it in the wrong vocabulary)
                body['actions'] = [a | {'outside_action': outside_action(a)} if isinstance(a, dict) else a
                                   for a in old['body'].get('actions', [])]
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
        was given (REQUEST_REFERENCE), the procedure its worker was given (PROCEDURE_LABEL), and a tool call's result
        and evidence sources. Nothing else its planner was shown."""
        if span.kind == 'context.exposure':
            if span.input.get('reference') == REQUEST_REFERENCE:         # the owner's message the task was planned from
                return {direct(str(ref)) for ref in span.input.get('source_refs', [])}
            return set()
        if span.kind == 'context.source':
            if span.input.get('label') == PROCEDURE_LABEL:               # the procedure its worker was given
                return {direct(str(ref)) for ref in span.input.get('source_refs', [])}
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
        consolidate`). A card built on forgotten content is never read again."""
        current = self.card_inputs()
        recorded = {r[0]: (r[1], r[2]) for r in self.db.conn.execute(
            'SELECT task_id, inputs, status FROM learning_card_inputs')}
        gone = {r[0] for r in self.db.conn.execute("SELECT id FROM learning_forgotten WHERE id LIKE 'key-%'")}
        order = [r[0] for r in self.db.conn.execute('SELECT task_id FROM episodes ORDER BY ended_at')]
        return [t for t in order if self.card_key(t) not in gone and (
            recorded.get(t, ('', ''))[0] != current.get(t) or recorded[t][1] == 'failed' or
            (force and recorded[t][1] == 'parked'))]

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

    def propose_procedure(self, proposal: dict[str, Any], batch: str, made_by: str,
                          intent_id: str | None = None) -> dict[str, Any]:
        """Check a proposed procedure against its examples' evidence and store it, keyed by `intent_id` (the intent
        its examples were labelled with, L29) when given, else by the proposal's intent. A step without `optional`
        is stored as required (false). A step citing an action built on outside content quarantines it, unless it
        revises a confirmed version and every such step (tool and template arguments) was already one of the version
        the owner confirmed by hand (`_hand_confirmed`): the confirm then carries over, marked as carried (L29 final
        review I1, final fix 2 F1; a code rule, never a model's judgement). A revision of a confirmed version that
        adds such a step, or whose hand-confirmed version was rolled back or is gone, is quarantined again: noted
        (`memory.procedure_requarantined`) and returned with `requarantined` (the reason). A proposal whose content
        matches an existing node changes nothing and is returned with `unchanged` (F3)."""
        examples = proposal.get('examples') or []
        steps = proposal.get('steps') or []
        if not examples or not steps or not proposal.get('intent'):
            raise LearningRefused('a procedure needs intent, steps and verified examples')
        slot_types = proposal.get('slots', {})
        if not isinstance(slot_types, dict) or any(t not in SLOT_TYPES
                                                   for t in slot_types.values()):
            raise LearningRefused('slots must declare JSON types')
        cards, actions = [], {}
        seen_tasks: set[str] = set()
        for ex in examples:
            node = self.get(ex['episode'])
            if node is None or node['kind'] != 'episode' or node['body'].get('verified') is not True:
                raise LearningRefused('procedures learn only from verified episode evidence')
            if set(ex.get('slots', {})) != set(slot_types):
                raise LearningRefused('every example supplies the typed slots')
            types = {'string': str, 'number': (int, float), 'boolean': bool, 'array': list, 'object': dict}
            if any(not isinstance(ex['slots'][k], types[t]) or
                   (t == 'number' and isinstance(ex['slots'][k], bool)) for k, t in slot_types.items()):
                raise LearningRefused('a slot value has the wrong type')
            task_id = str(node['body']['task_id'])
            if task_id in seen_tasks:
                raise LearningRefused('one task cannot count as two successful examples')
            seen_tasks.add(task_id)
            expected_apps = proposal.get('preconditions', {}).get('apps', {})
            if any(expected_apps.get(k) != v for k, v in node['body'].get('apps', {}).items()):
                raise LearningRefused('preconditions must preserve captured app versions')
            cards.append(node)
            actions.update({a['id']: a for a in node['body'].get('actions', [])})
        selected: list[dict[str, Any]] = []
        outside_steps: list[bool] = []                  # per step: it cites an outside-built action
        action_card = {a['id']: card['id'] for card in cards for a in card['body'].get('actions', [])}
        for step in steps:
            refs = step.get('evidence') or []
            if not step.get('expect') or not step.get('tool') or not isinstance(step.get('args'), dict) or not refs:
                raise LearningRefused('each tool template needs args, expect and action evidence')
            if not isinstance(step.get('optional', False), bool):
                raise LearningRefused('a step is optional (true) or required (false)')
            if any(ref not in actions or actions[ref].get('tool') != step['tool'] or actions[ref].get('error')
                   for ref in refs):
                raise LearningRefused('step cites a missing, failed or different action')
            # A required step reproduces a cited action in every example; an optional one (L29) in every example
            # whose actions it cites, and it cites at least one: a step no run did is never learned.
            needed = [(ex, card) for ex, card in zip(examples, cards)
                      if not step.get('optional') or any(action_card[ref] == card['id'] for ref in refs)]
            for ex, card in needed:
                actual = {a['id']: a for a in card['body'].get('actions', [])}
                expected = render_slots(step['args'], ex['slots'])
                if not any(ref in actual and actual[ref].get('args') == expected for ref in refs):
                    raise LearningRefused('template arguments do not reproduce a cited action for each example')
            selected.extend(actions[ref] for ref in refs)
            outside_steps.append(any(actions[ref].get('outside_action', True) for ref in refs))
        key = str(intent_id or proposal['intent'])
        proposal = proposal | {'steps': [s | {'optional': s.get('optional', False)} for s in steps]}
        state = 'quarantined' if any(outside_steps) else 'candidate'
        before = self._current_procedure(key)
        result = self.put('procedure', key, proposal, [c['id'] for c in cards], made_by=made_by,
                          batch=batch, state=state)
        # `put` returns an existing node whose content matches (the current version, or an older one): then the
        # current version is unchanged, and no confirm carries or is lost (final fix 2, F3)
        fresh = not result['valid_to'] and (before is None or result['supersedes'] == before['id'])
        requarantined = None
        if fresh and before is not None and self._confirmed_raw(before['id']):     # it revises a confirmed version
            hand = self._hand_confirmed(before['id'])
            if hand is not None:
                # compared as stored (both redacted alike) with the steps the owner read when they confirmed by hand
                read = {(s.get('tool'), encoded(s.get('args') or {})) for s in hand['body'].get('steps') or []}
                if all((s.get('tool'), encoded(s.get('args') or {})) in read
                       for s, outside in zip(result['body'].get('steps') or [], outside_steps) if outside):
                    self._mark_confirmed(result['id'], carried=True)
                else:
                    requarantined = 'a step built on outside content that the version the owner confirmed did not have'
            elif any(outside_steps):
                requarantined = 'the version the owner confirmed was rolled back or is gone'
            if requarantined:
                self.note('memory.procedure_requarantined', {
                    'procedure_id': result['id'], 'revises': before['id'],
                    'confirmed_id': hand['id'] if hand is not None else None, 'reason': requarantined})
        for ex, card in zip(examples, cards):
            self.receipt(result['id'], card['id'], ex['slots'])
        stored: dict[str, Any] = self.get(result['id'])  # type: ignore[assignment]
        return stored | ({'requarantined': requarantined} if requarantined else {}) | \
            ({} if fresh else {'unchanged': True})

    def _confirmed_raw(self, node_id: str) -> int:
        """A node's stored confirm: 0 none, 1 the owner's own (`confirm`), 2 carried to a revision
        (`propose_procedure`). `get` reports any as True."""
        row = self.db.conn.execute('SELECT confirmed FROM learning_nodes WHERE id = ?', (node_id,)).fetchone()
        return int(row[0]) if row else 0

    def _hand_confirmed(self, node_id: str) -> dict[str, Any] | None:
        """The version of this procedure the owner confirmed by hand: this one, or the nearest earlier one reached
        through versions whose confirm was carried (`supersedes`), so a carried confirm never counts as the owner's
        own (final fix 2, F1, MP2). None when it was rolled back or is gone."""
        ident: str | None = node_id
        seen: set[str] = set()
        while ident and ident not in seen:
            seen.add(ident)
            row = self.db.conn.execute('SELECT confirmed, supersedes, state FROM learning_nodes WHERE id = ?',
                                       (ident,)).fetchone()
            if row is None or row[0] not in (1, 2):
                return None
            if row[0] == 1:
                return None if row[2] == 'rolled_back' else self.get(ident)
            ident = row[1]
        return None

    def _current_procedure(self, key: str) -> dict[str, Any] | None:
        """The current version of the procedure with this key (`key` as given; stored redacted), the one a new
        version supersedes (`put`)."""
        row = self.db.conn.execute("SELECT id FROM learning_nodes WHERE kind = 'procedure' AND key = ? AND valid_to IS "
                                   "NULL ORDER BY recorded_at DESC, rowid DESC LIMIT 1", (self.redact(key),)).fetchone()
        return self.get(row[0]) if row else None

    def receipt(self, procedure_id: str, episode_id: str, slots: dict[str, Any] | None = None) -> None:
        p, ep = self.get(procedure_id), self.get(episode_id)
        if not p or not ep or p['kind'] != 'procedure' or ep['kind'] != 'episode':   # checked before writing (M7)
            raise LearningRefused('missing procedure/episode')
        # The owner's latest verdict counts even when the card couldn't be refreshed after it (a replayed receipt).
        last = self.db.conn.execute('SELECT verdict FROM owner_verdicts WHERE task_id=? ORDER BY seq DESC LIMIT 1',
                                    (ep['key'],)).fetchone()
        successful = ep['body'].get('verified') is True and not (last and last[0] == 'fail')
        # the slot values are model-written (the planner's, L29, or a proposal's examples): redacted like a node's body
        values = redacted_json(self.redact, slots or ep['body'].get('slots') or {}, self.secret_values())
        self.db.conn.execute('INSERT INTO learning_receipts VALUES (?,?,?,?,?) ON CONFLICT(procedure_id,episode_id) '
                             'DO UPDATE SET successful=excluded.successful',
            (procedure_id, episode_id, encoded(values), int(successful), ep['recorded_at']))
        self._refresh(procedure_id, None)
        # a run whose planner chose a version since revised counts for the key's current version too (I2)
        for row in self.db.conn.execute("SELECT id FROM learning_nodes WHERE kind = 'procedure' AND key = ? AND "
                                        "valid_to IS NULL AND id != ?", (p['key'], procedure_id)).fetchall():
            self._refresh(row[0], None)

    def _refresh(self, node_id: str, apps: dict[str, str] | None) -> dict[str, Any]:
        """A procedure's state from the receipts of every version of its key (L29 final review I2: a model revision
        after each run is a new node, and the runs, failures and slot values the key has seen still count), one per
        task (its latest), leaving out a rolled-back version's receipts and a forgotten run's (its card is gone)."""
        p = self.get(node_id)
        if p is None or p['kind'] != 'procedure':
            raise LearningRefused('not a procedure')
        rows = list(self.db.conn.execute(
            "SELECT r.* FROM learning_receipts r JOIN learning_nodes n ON n.id = r.procedure_id WHERE n.kind = "
            "'procedure' AND n.key = ? AND n.state != 'rolled_back' ORDER BY r.at, r.rowid", (p['key'],)))
        unique: dict[str, Any] = {}
        for row in rows:
            ep = self.get(row['episode_id'])
            if ep:
                unique[ep['body']['task_id']] = row
        rows = list(unique.values())
        ok = [r for r in rows if r['successful']]
        expected = p['body'].get('preconditions', {}).get('apps', {})
        declared = sorted(p['body'].get('slots') or {})
        # M1: a slot value counts only when it supplies the procedure's slots; a run's receipt without values doesn't.
        # Only the slots this version declares count (a receipt of an earlier version may hold others: I2).
        variants = set()
        for r in ok if declared else []:
            values = self._receipt_slots(r)
            if set(declared) <= set(values):
                variants.add(encoded([values[k] for k in declared]))
        latest = self.get(ok[-1]['episode_id']) if ok else None
        latest_apps = latest['body'].get('apps', {}) if latest else {}
        # M6: "current" is the versions installed now when known: an update sends a learned procedure back to
        # candidate until it succeeds on the new version; it never makes it suspect by itself
        current = {k: (apps or {}).get(k, v) for k, v in expected.items()}
        matches = 'unknown' not in latest_apps.values() and bool(latest and latest['body'].get('app_versions_captured', True)) and all(
            latest_apps.get(k) == v for k, v in current.items())
        consecutive_failures = len(rows) >= 2 and not rows[-1]['successful'] and not rows[-2]['successful']
        state = p['state']
        if state not in ('retired', 'rolled_back', 'quarantined'):
            if consecutive_failures:                 # only a procedure that actually failed is suspect (M6)
                state = 'suspect'
            elif state != 'suspect':
                state = 'learned' if len(ok) >= 3 and len(variants) >= 2 and matches else 'candidate'
        self.db.conn.execute('UPDATE learning_nodes SET state=? WHERE id=?', (state, node_id))
        p['state'] = state
        p['counters'] = {'verified_successes': len(ok), 'failures': len(rows) - len(ok),
                         'slot_variants': len(variants)}
        return p

    def _receipt_slots(self, row: Any) -> dict[str, Any]:
        """A receipt row's slot values. A row whose values don't parse (written before they were redacted as JSON, L29
        fix round 1) counts without values: it is noted once and its values cleared, never raising on a refresh."""
        try:
            slots = json.loads(row['slots_json'])
        except ValueError:
            slots = None
        if isinstance(slots, dict):
            return slots
        self.db.conn.execute("UPDATE learning_receipts SET slots_json='{}' WHERE procedure_id=? AND episode_id=?",
                             (row['procedure_id'], row['episode_id']))
        self.note('memory.receipt_unreadable', {'procedure_id': row['procedure_id'], 'episode_id': row['episode_id']})
        return {}

    def procedure(self, node_id: str, apps: dict[str, str]) -> dict[str, Any]:
        return self._refresh(node_id, apps)

    def confirm(self, node_id: str) -> dict[str, Any]:
        p = self.get(node_id)
        if not p or p['valid_to'] or p['state'] == 'rolled_back':
            raise LearningRefused('no current proposal')
        if p['kind'] == 'identity_proposal':
            self.put('identity', p['key'], p['body'], [p['id']], origin_label=origin.OWNER,
                     made_by='owner:confirmed-digest/1', state='active', owner=True)
        self._mark_confirmed(node_id)
        self.publish('memory.confirmed_derived', {'id': node_id})
        return self._refresh(node_id, None) if p['kind'] == 'procedure' else self.get(node_id)  # type: ignore[return-value]

    def _mark_confirmed(self, node_id: str, *, carried: bool = False) -> None:
        """The owner's confirm on a node (`confirm`: 1, also over a carried one, since the owner has now read it), or
        one carried to a procedure's revision (`propose_procedure`: 2, never the owner's own, `_hand_confirmed`): a
        quarantined one becomes a candidate; its state is then refreshed by its caller."""
        self.db.conn.execute('UPDATE learning_nodes SET confirmed=?,state=CASE WHEN state=\'quarantined\' '
                             'THEN \'candidate\' ELSE state END WHERE id=?', (2 if carried else 1, node_id))

    def edit(self, node_id: str, text: str) -> dict[str, Any]:
        p = self.get(node_id)
        if not p:
            raise LearningRefused('unknown item')
        if p['kind'] in ('procedure', 'episode'):
            raise LearningRefused('procedure/episode evidence is immutable; retire it or edit a separate owner note')
        body = dict(p['body']) | {'text': text}
        return self.put('identity' if p['kind'] == 'identity_proposal' else p['kind'], p['key'], body, [p['id']],
                         origin_label=origin.OWNER, made_by='owner:edit/1', state='active', owner=True)

    def lesson_feedback(self, node_id: str, helpful: bool) -> dict[str, Any]:
        node = self.get(node_id)
        if not node or node['kind'] != 'lesson' or node['valid_to']:
            raise LearningRefused('feedback needs a current lesson')
        key = 'helpful' if helpful else 'harmful'
        body = dict(node['body'])
        body[key] = int(body.get(key, 0)) + 1
        return self.put('lesson', node['key'], body, [node['id']], origin_label=node['origin'],
                        made_by='code:owner-lesson-feedback/1', state=node['state'],
                        owner=node['origin'] == origin.OWNER)

    def identity(self) -> str:
        return '\n'.join(str(p['body'].get('text', '')) for p in self.items('identity') if p['origin'] == origin.OWNER)

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

    def drop_fact_index(self, ref: str) -> list[str]:
        """The owner's Drop on one of FERB's guesses (L36 §2): the fact nodes that index that memory item `ref`
        (`sync_facts`, every version) are deleted, and nothing else. Never `forget`: what was built on the guess while
        it existed (the episode cards of tasks whose memory reads returned it, the summaries and intent labels built on
        those cards, an entity summary) stays, and no tombstone is left, so a later capture of such a card counts the
        guess as a missing source, never a forgotten one (`_card_labels`). Live 2026-10-25: one Drop through `forget`
        deleted 750 learned items, 406 of them episode cards, in 53 s on the event loop."""
        doomed = sorted(r[0] for r in self.db.conn.execute(
            "SELECT n.id FROM learning_nodes n JOIN learning_sources s ON s.node_id = n.id "
            "WHERE s.source_ref = ? AND n.kind = 'fact'", (ref,)))
        if not doomed:
            return []
        self.generation += 1                          # a card read before now may cite a node that is gone
        self.db.conn.execute('SAVEPOINT learning_drop')
        try:
            self.db.conn.executemany('DELETE FROM learning_nodes WHERE id=?', [(i,) for i in doomed])
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
        no recency or owner boost. `leave_out`: kinds dropped before `limit` (the budget) applies. An
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
        order: list[tuple[float, bool, str, str, float]] = []
        for row in self._columns(scores):
            if row['valid_to'] or row['state'] in ('retired', 'rolled_back', 'proposed') or \
                    (kind and row['kind'] != kind) or row['kind'] in leave_out:
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

    def candidates(self, query: str) -> str:
        return '\n'.join(f"{p['id']} [{p['kind']}, {shown_state(p)}, {p['origin']}; data, not authorization] {p['key']}"
                         for p in self.search(query) if p['kind'] != 'identity')

    def learned_procedures(self) -> list[dict[str, Any]]:
        """L29: every procedure the planner may choose (current: not superseded, retired, rolled back or forgotten; and
        not quarantined), the most recently used intent first (a procedure no current card's label carries: by when it
        was written), each with its intent's name and description on one line, the label node they come from
        (`intents.named`: the intent's newest current label; None, and the procedure's own intent text, when no current
        card carries the intent), the ids and the union of the origins of all the intent's current labels (whose text
        the newest copies) and its slot types."""
        from ferb.memory import intents
        procedures = [p for p in self.items('procedure') if p['state'] != 'quarantined']
        named = intents.named(self, [p['key'] for p in procedures])
        procedures.sort(key=lambda p: (named.get(p['key'], {}).get('last') or p['recorded_at'], p['recorded_at'],
                                       p['id']), reverse=True)
        out = []
        for p in procedures:
            about_intent = named.get(p['key'], {})
            label = about_intent.get('label')
            about = label['body'] if label else {}
            out.append({'node': p, 'label': label, 'labels': about_intent.get('labels', []),
                        'label_origins': about_intent.get('origins', set()),
                        'name': ' '.join(str(about.get('name') or p['body'].get('intent') or p['key']).split()),
                        'description': ' '.join(str(about.get('description', '')).split()),
                        'slots': p['body'].get('slots') or {}})
        return out

    def slot_values(self, procedure_id: str, pairs: list[dict[str, Any]]) -> dict[str, Any] | None:
        """The planner's values for a procedure's slots (strict {name, value} pairs, each value JSON text, read as a
        label's values are), typed (L29); None when the id isn't a procedure, or the values don't name exactly its
        slots or one has the wrong JSON type (`slot_problem` says which)."""
        try:
            return self._typed_slots(procedure_id, pairs)
        except LearningRefused:
            return None

    def slot_problem(self, procedure_id: str, pairs: list[dict[str, Any]]) -> str | None:
        """Why `slot_values` refuses the planner's values for a procedure's slots, or None."""
        try:
            self._typed_slots(procedure_id, pairs)
        except LearningRefused as e:
            return str(e)
        return None

    def _typed_slots(self, procedure_id: str, pairs: list[dict[str, Any]]) -> dict[str, Any]:
        from ferb.memory.intents import typed
        from ferb.memory.strict import to_map
        p = self.get(procedure_id)
        if p is None or p['kind'] != 'procedure':
            raise LearningRefused('not a procedure')
        values = to_map(pairs, json_text=True)
        typed(values, p['body'].get('slots') or {})
        return values

    def built_on(self, node: dict[str, Any], tasks: set[str]) -> bool:
        """Whether a node (a lesson) is built on a run of these tasks (any version of its card), through its earlier
        versions (the owner's feedback and edits cite the version before): lineage, never words (R2)."""
        seen: set[str] = set()
        frontier = list(node['sources'])
        while frontier:
            ref = frontier.pop()
            found = None if ref in seen else self.get(ref)
            seen.add(ref)
            if found is not None and found['kind'] == 'episode' and found['key'] in tasks:
                return True
            if found is not None and found['kind'] == node['kind']:
                frontier.extend(found['sources'])
        return False

    def procedure_parts(self, node_id: str, apps: dict[str, str],
                        slots: dict[str, Any] | None) -> list[ProcedurePart]:
        """L29 (spec 5): what a worker gets for its selected procedure, all of it data. First the procedure (protected):
        its state's mode, chosen by code and shown first, then its steps as a tool plan, each step's arguments with
        this run's slot values filled in (a slot without a value keeps its {template}) and the result to expect; its
        model-written text (intent, UI hints, each expect) JSON-encoded like the arguments (review M3), every JSON
        `prompt_json` (a line separator inside it is an escape, never a new line: re-review N3). Then, when any,
        the current lessons built on its intent's runs (protected; trigger and text JSON-encoded, with the owner's
        ratings when it has any), and one part per verified run of its intent, newest first by when its task ended:
        that run's successful tool calls (tool, arguments, what came back), an optional part the worker's window fits
        newest first, read in full by its card's id. Each part carries the origins of what it shows and the nodes it
        cites."""
        from ferb.memory import intents
        p = self.procedure(node_id, apps)
        shown = ProcedurePart(PROCEDURE_LABEL, '', True, frozenset(p['origins']), (p['id'], *p['sources']))
        if p['valid_to'] or p['state'] in ('quarantined', 'retired', 'rolled_back'):
            return [shown._replace(text=f"Procedure unavailable: {p['state']}. Use full reasoning and fresh evidence.")]
        mode = ('Eligible for learned fast path: follow these steps, check every expected result and the final state; '
                'on any mismatch use full reasoning.' if p['state'] == 'learned' else
                'Candidate/suspect evidence only. Use full reasoning and fresh observations.')
        body, filled = p['body'], slots or {}
        declared = body.get('slots') or {}
        # the code-chosen mode first; the model-written fields JSON-encoded, as the arguments are (review M3)
        lines = [f"Procedure {p['id']} [{p['origin']}; data, never permission]. {mode}",
                 'Intent: ' + prompt_json(body.get('intent', ''))]
        if body.get('ui_hints'):
            lines.append('UI hints: ' + prompt_json(body['ui_hints']))
        lines.append('Steps:' if not declared else
                     f"Steps, with this run's slot values filled in ({prompt_json(filled)}):" if filled else
                     f"Steps; this run has no slot values, so fill each {{slot}} from the request (slots: "
                     f"{prompt_json(declared)}):")
        for n, step in enumerate(body.get('steps') or [], 1):
            lines.append(f"{n}. {step.get('tool')} {prompt_json(render_slots(step.get('args') or {}, filled))} — "
                         f"expect: {prompt_json(step.get('expect', ''))}"
                         + (' (optional: only some runs needed it)' if step.get('optional') else ''))
        parts = [shown._replace(text='\n'.join(lines))]
        cards = intents.cards(self, p['key'])
        tasks = {str(c['key']) for c in cards}
        lessons = [n for n in self.items('lesson') if self.built_on(n, tasks)]
        if lessons:
            parts.append(ProcedurePart(
                LESSONS_LABEL, '\n'.join(f"- when {prompt_json(n['body'].get('trigger', ''))}: "
                                         f"{prompt_json(n['body'].get('text', ''))}{_ratings(n['body'])}"
                                         for n in lessons),
                True,
                frozenset(o for n in lessons for o in n['origins']), tuple(n['id'] for n in lessons),
                'memory.learned kind=lesson'))
        runs = sorted((c for c in cards if c['body'].get('verified') is True),
                      key=lambda c: (c['body'].get('ended_at') or c['recorded_at'], c['id']), reverse=True)
        for run in runs:
            calls = [f"- {a.get('tool')} {prompt_json(a.get('args') or {})} → {prompt_json(a.get('result'))}"
                     for a in run['body'].get('actions') or [] if isinstance(a, dict) and not a.get('error')]
            parts.append(ProcedurePart(
                RUN_LABEL, f"Task {run['body'].get('task_id')}, ended {run['body'].get('ended_at') or run['recorded_at']}"
                           f": {_one_line(run['body'].get('request'))}\n" + ('\n'.join(calls) or 'No successful tool calls.'),
                False, frozenset(run['origins']), (run['id'],), 'memory.learned item_id=' + run['id']))
        return parts

    def procedure_text(self, node_id: str, apps: dict[str, str]) -> str:
        """The procedure itself as a worker gets it (`procedure_parts`' first part)."""
        return self.procedure_parts(node_id, apps, None)[0].text

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
                node = self.put('fact', f'{item.subject}/{item.key}', body, ['memory:' + item.id],
                                origin_label=memory_origin(item.origin), made_by='code:fact-index/1',
                                state=state, owner=item.origin == 'owner_stated')
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
