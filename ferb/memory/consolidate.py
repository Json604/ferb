"""Idle, incremental learning from redacted evidence. No tools; every model proposal is checked against storage.

A pass works on what changed (memory review I3): it reads again only the cards whose inputs changed
(`Learning.cards_to_read`), gives each card without an intent label one (L29, `_label`: a re-read card keeps its
task's label; any other, one call; the model decides the intent, `memory/intents.py` checks the answer), and then
consolidates by intent (L29), one call per intent: an intent is first learned once it has FIRST_LEARNING_RUNS verified
runs, and from then on (or once it has a procedure) again whenever its runs change. So the number of calls follows what
changed; each call carries the intent's runs again, up to the model's window. A call carries the intent, its procedure
and lessons, the owner's refusals, and its runs: the procedure's example runs first, then the newest others, as many as
fit the model's real window and per-input cap, each whole (a run is never shortened). A call that failed (the service,
the connection, a timeout, the runtime) is tried on the next pass; an answer refused by code (for an intent: every
procedure it proposed) or outside the schema, or input too large, keeps its error and waits for its inputs to change
(a label: its card; an intent: its runs) or `ferb memory consolidate` (`force`). No count or size limit of FERB's own
decides any of this; three runs is the owner's."""
from __future__ import annotations

import asyncio
import hashlib
import json
from typing import Any, Awaitable, Callable

from ferb.core import origin
from ferb.core.context import count_tokens, overhead_chars, overhead_tokens, runtime_limits
from ferb.core.errors import Deferred, InputTooLarge
from ferb.core.prompts import load_prompt
from ferb.core.structured import InvalidAnswer
from ferb.core.types import Identity, Task
from ferb.memory import intents
from ferb.memory.calls import learning_call
from ferb.memory.intents import INTENT_SCHEMA
from ferb.memory.learning import Learning, LearningRefused, card_digest, digest, encoded
from ferb.memory.strict import JSON_TEXT, pairs, to_map


PROPOSAL_SCHEMA: dict[str, Any] = {
    'type': 'object', 'additionalProperties': False, 'required': ['procedures', 'lessons', 'identity', 'episodes'],
    'properties': {
        'episodes': {'type': 'array', 'items': {'type': 'object', 'additionalProperties': False,
            'required': ['episode', 'intent_signature', 'slots', 'reflection', 'narrative'], 'properties': {
                'episode': {'type': 'string'}, 'intent_signature': {'type': 'string'},
                'slots': pairs(JSON_TEXT), 'reflection': {'type': 'string'}, 'narrative': {'type': 'string'}}}},
        'procedures': {'type': 'array', 'items': {'type': 'object', 'additionalProperties': False,
            'required': ['intent', 'slots', 'preconditions', 'steps', 'ui_hints', 'examples'],
            'properties': {
                'intent': {'type': 'string'},
                'slots': pairs({'type': 'string', 'description': "the slot's JSON type: string, number, boolean, "
                                                                  "array or object"}),
                'preconditions': {'type': 'object', 'required': ['apps'], 'additionalProperties': False,
                                  'properties': {'apps': pairs({'type': 'string',
                                                                 'description': 'the captured app version'})}},
                'steps': {'type': 'array', 'items': {'type': 'object', 'additionalProperties': False,
                    'required': ['tool', 'args', 'expect', 'optional', 'evidence'], 'properties': {
                        'tool': {'type': 'string'}, 'args': pairs(JSON_TEXT), 'expect': {'type': 'string'},
                        'optional': {'type': 'boolean', 'description': 'true when only some examples needed this step'},
                        'evidence': {'type': 'array', 'items': {'type': 'string'}}}}},
                'ui_hints': {'type': 'string'},
                'examples': {'type': 'array', 'items': {'type': 'object', 'additionalProperties': False,
                    'required': ['episode', 'slots'], 'properties': {
                        'episode': {'type': 'string'}, 'slots': pairs(JSON_TEXT)}}}}}},
        'lessons': {'type': 'array', 'items': {'type': 'object', 'additionalProperties': False,
            'required': ['scope', 'trigger', 'text', 'failure', 'success'], 'properties': {
                'scope': {'type': 'object', 'additionalProperties': False,
                          'required': ['app', 'site', 'intent'], 'properties': {
                              'app': {'type': 'string'}, 'site': {'type': 'string'}, 'intent': {'type': 'string'}}},
                'trigger': {'type': 'string'}, 'text': {'type': 'string'},
                'failure': {'type': 'string'}, 'success': {'type': 'string'}}}},
        'identity': {'type': 'array', 'items': {'type': 'object', 'additionalProperties': False,
            'required': ['key', 'text', 'evidence'], 'properties': {'key': {'type': 'string'}, 'text': {'type': 'string'},
                'evidence': {'type': 'array', 'items': {'type': 'string'}}}}}}}


# The owner's "counter 3" (L29, 2026-10-21): an intent is first learned from three verified runs.
FIRST_LEARNING_RUNS = 3

# A job's inputs (L29): LABEL_JOB for a label job (`learning_jobs.source_ref` 'label:<card id>'), its runs' digest for
# an intent job ('intent:<intent id>'). An answer refused by code or outside the schema, or input too large, waits for
# them to change or `force`; a failed call is tried on the next pass (RETRY appended, so they never match).
RETRY = ':retry'
LABEL_JOB, LABEL_RETRY = 'label/1', 'label/1' + RETRY


def _waits(e: Exception) -> bool:
    """The model's answer, not the call, failed (refused by code or outside the schema), or the input is too large for
    any call: tried again on a change or `force`. Anything else failed the call itself (the service, the connection, a
    timeout, the runtime): tried on the next pass."""
    return isinstance(e, (LearningRefused, InvalidAnswer, InputTooLarge))


def _procedure(p: dict[str, Any]) -> dict[str, Any]:
    """A proposed procedure in the shape Learning checks and stores: its slots, apps and args as maps."""
    return p | {'slots': to_map(p['slots']),
                'preconditions': p['preconditions'] | {'apps': to_map(p['preconditions']['apps'])},
                'steps': [s | {'args': to_map(s['args'], json_text=True)} for s in p['steps']],
                'examples': [e | {'slots': to_map(e['slots'], json_text=True)} for e in p['examples']]}


class Consolidator:
    def __init__(self, learning: Learning, memory: Any, make_runtime: Callable[..., Any], *,
                 idle: Callable[[], bool] = lambda: True,
                 usage: Callable[[str, dict[str, Any]], None] = lambda *args: None,
                 capture: Callable[[str], Awaitable[Any]] | None = None,
                 cards_refreshed: Callable[[], None] = lambda: None, spans: Any = None,
                 forget: Callable[[list[str]], Any] | None = None) -> None:
        """`capture`: refreshes one task's episode card. The daemon's reads it in the card reader process, off the event
        loop (L28); without one, the card is refreshed in-process. `cards_refreshed`: told once every card has been
        refreshed (the daemon then records procedure receipts a shutdown deferred). `spans`: where each model call is
        recorded (`learning_call`). `forget`: how learned nodes are forgotten (the daemon's path also closes FERB's
        questions about them); `Learning.forget` without one."""
        self.learning, self.memory, self.make_runtime = learning, memory, make_runtime
        self.idle, self.usage, self.lock, self.spans = idle, usage, asyncio.Lock(), spans
        self.capture = capture or self._capture_here
        self.cards_refreshed = cards_refreshed
        self.forget: Callable[[list[str]], Any] = forget or learning.forget

    async def _capture_here(self, task_id: str) -> Any:
        return self.learning.capture_episode(task_id)

    async def catch_up(self, *, force: bool = False) -> dict[str, Any]:
        """One pass. `force` (`ferb memory consolidate`): failed reads and jobs are tried again now."""
        if self.lock.locked() or not self.idle():
            return {'status': 'busy'}
        async with self.lock:
            l = self.learning
            l.sync_facts(self.memory)
            for task_id in l.cards_to_read(force):
                if not self.idle():                  # a task started: the next pass reads the rest
                    return {'status': 'busy'}
                try:
                    await self.capture(task_id)
                    if not l.db.conn.execute('SELECT 1 FROM learning_card_inputs WHERE task_id=?', (task_id,)).fetchone():
                        # read without a card to write (nothing to learn from): not read again until it changes
                        l.card_read(task_id, l.card_inputs(task_id).get(task_id, ''))
                except LearningRefused as e:         # waits for its inputs to change (or `force`)
                    l.card_read(task_id, l.card_inputs(task_id).get(task_id, ''), l.redact(str(e)) or 'refused')
                except Deferred:                     # FERB is stopping: the next start's pass reads the rest
                    return {'status': 'stopping'}
                await asyncio.sleep(0)               # the loop runs between episodes
            self.cards_refreshed()
            results: list[dict[str, Any]] = []
            if not await self._label(force, results):
                return {'status': 'busy', 'sources': results}
            jobs = {r[0]: (r[1], r[2]) for r in l.db.conn.execute('SELECT source_ref, status, inputs FROM learning_jobs')}
            for about, members in self._groups():
                if not await self._consolidate(about, members, jobs, force, results):
                    return {'status': 'busy', 'sources': results}
            if not self.idle():                      # I4: never once a task has started
                return {'status': 'busy', 'sources': results}
            await self._rollups_off_loop()
            if l.embedder is not None:
                from ferb.memory.embeddings import index_async
                try:
                    await index_async(l, l.embedder)
                except Exception as e:
                    l.publish('memory.embedding_unavailable', {'error': l.redact(str(e))})
            return {'status': 'done', 'sources': results}

    async def _label(self, force: bool, results: list[dict[str, Any]]) -> bool:
        """L29: each current card without an intent label gets one. A re-read card keeps its task's label without a
        call (`intents.carry_forward`); every other card gets one call, offered the intents already seen (read once a
        pass and counted as labels are stored; most recently used first, as many as fit the window: `_fit_intents`).
        A refused answer, an answer outside the schema or a card too large waits for its card to change (a new card
        node, so a new job) or `force` (job inputs LABEL_JOB); a failed call is tried on the next pass (LABEL_RETRY).
        False when a task started (the next pass goes on)."""
        l = self.learning
        results.extend({'label': n['key'], 'carried': True} for n in intents.carry_forward(l))
        waiting = {r[0] for r in l.db.conn.execute(
            "SELECT source_ref FROM learning_jobs WHERE source_ref LIKE 'label:%' AND status = 'failed' AND inputs = ?",
            (LABEL_JOB,))}
        todo = [c for c in intents.unlabelled(l) if force or 'label:' + c['id'] not in waiting]
        if not todo:
            return True
        system = load_prompt('memory/intent.md')
        try:
            rt = self.make_runtime('worker', mode='supervisor', tool_names=[], schema=INTENT_SCHEMA)
            limits = runtime_limits(rt)
        except asyncio.CancelledError:
            raise
        except Exception as e:                       # no runtime or no documented window: tried on the next pass
            error = l.redact(f'{type(e).__name__}: {e}')
            for c in todo:
                self._job('label:' + c['id'], LABEL_RETRY, error)
                results.append({'label': c['id'], 'error': error})
            return True
        maker = (f'model:{rt.name}:{getattr(rt, "model", None) or "default"}'
                 f'|prompt:intent@{hashlib.sha256(system.encode()).hexdigest()}')
        known = intents.intents(l)
        for c in todo:
            if not self.idle():
                return False
            task = Task('label-' + c['id'], None, 'memory intent label', 'local', Identity.OWNER, set(), 'verifier',
                        'sleep', [], None)
            try:
                ctx = encoded({'episode': card_digest(c), 'intents': _fit_intents(c, known, system, limits, rt.name)})
                answer = json.loads(await learning_call(rt, task, system, ctx, 'redacted learning evidence',
                                                        INTENT_SCHEMA, role='intent', spans=self.spans,
                                                        usage=self.usage))
                node = intents.label(l, c, answer, {i['id']: i for i in known}, maker)
                known = intents.counted(known, node['body'], c)
                self._job('label:' + c['id'], LABEL_JOB, None)
                results.append({'label': c['id']})
            except asyncio.CancelledError:
                raise
            except Exception as e:
                # an answer refused by code or outside the schema, or a card too large for any call: waits; the call
                # itself failed (the service, the connection, a timeout, the runtime): the next pass
                error = l.redact(f'{type(e).__name__}: {e}')
                self._job('label:' + c['id'], LABEL_JOB if _waits(e) else LABEL_RETRY, error)
                results.append({'label': c['id'], 'error': error})
        return True

    def _groups(self) -> list[tuple[dict[str, Any], list[dict[str, Any]]]]:
        """The current episode cards by intent label (L29): the intent (id, name, description and slot types, as
        `intents.intents` gives them: its oldest label's) and its cards (`_newest_first` orders them by when each task
        ended). A card without a label waits for one."""
        labels = intents.labels(self.learning)
        about: dict[str, dict[str, Any]] = {}
        for body in labels.values():
            about.setdefault(body['intent_id'], {'id': body['intent_id'], 'name': body['name'],
                                                 'description': body['description'], 'slots': body['slots']})
        groups: dict[str, list[dict[str, Any]]] = {}
        for node in self.learning.items('episode'):
            if node['id'] in labels:
                groups.setdefault(labels[node['id']]['intent_id'], []).append(node)
        return [(about[intent], members) for intent, members in groups.items()]

    async def _consolidate(self, about: dict[str, Any], members: list[dict[str, Any]],
                           jobs: dict[str, tuple[str, str]], force: bool, results: list[dict[str, Any]]) -> bool:
        """One intent (L29): first learned once it has FIRST_LEARNING_RUNS verified runs, and from then on (or once it
        has a procedure) again whenever its runs change. One call (`_context`): the intent, its procedure and lessons,
        the owner's refusals, and its runs: the procedure's example runs first (a revision must cite them), then the
        newest others that fit, each whole (failed ones too, for lessons). The job (`intent:<intent id>`, inputs: the
        runs' digest) waits for the runs to change or `force` when the answer or the fit failed, or every procedure the
        model proposed was refused; a failed call is tried on the next pass. False when a task started (the next pass
        goes on)."""
        l, intent = self.learning, about['id']
        group = digest(sorted(n['id'] for n in members))
        status, inputs = jobs.get('intent:' + intent, ('', ''))
        if inputs == group and (status == 'done' or not force):
            return True
        procedures = self._procedures(intent)
        if not procedures['existing_procedures'] and \
                sum(n['body'].get('verified') is True for n in members) < FIRST_LEARNING_RUNS:
            return True
        system = load_prompt('memory/consolidate.md')
        try:
            rt = self.make_runtime('worker', mode='supervisor', tool_names=[], schema=PROPOSAL_SCHEMA)
            limits = runtime_limits(rt)
        except asyncio.CancelledError:
            raise
        except Exception as e:                       # no runtime or no documented window: tried on the next pass
            return self._intent_job(intent, group + RETRY, l.redact(f'{type(e).__name__}: {e}'), results)
        context = self._context(about, members, procedures)
        ids = {n['id'] for n in members}
        examples = {e['episode'] for p in context['existing_procedures'] for e in p['body'].get('examples') or []
                    if isinstance(e, dict) and e.get('episode') in ids}
        annotated = {r[0] for r in l.db.conn.execute(
            "SELECT key FROM learning_nodes WHERE kind = 'episode_annotation' AND valid_to IS NULL "
            "AND state NOT IN ('retired', 'rolled_back')")}
        # a run without a current annotation is marked for one (M5): the others are annotated already
        entries = {n['id']: card_digest(n) | ({} if n['id'] in annotated else {'annotate': True}) for n in members}
        window, reserved = limits.context_budget, count_tokens(system)
        base = encoded(context | {'episodes': []})
        room = window - reserved - overhead_tokens(window, reserved) - count_tokens(base)
        chars = (None if limits.max_input_chars is None else
                 limits.max_input_chars - overhead_chars(window, reserved) - len(base))
        cap = f"one {rt.name} input ({limits.max_input_chars} characters at most)"
        limit = "the model's window" + ("" if chars is None else " and " + cap)
        if room <= 0 or (chars is not None and chars <= 0):      # its procedures alone fill the window: say that
            return self._intent_job(intent, group, "the intent's procedures, lessons and the prompt alone don't fit " +
                                    ("the model's window" if room <= 0 else cap), results)
        fitted = _fit_runs(members, examples, entries, room, chars)
        if fitted is None:
            return self._intent_job(intent, group, f"its procedure's example runs (a revision cites them all) don't "
                                    f"fit {limit} beside its procedures, lessons and the prompt", results)
        if not fitted:
            return self._intent_job(intent, group, f"none of its runs fits {limit} beside its procedures, lessons and "
                                    "the prompt; sent whole or not at all", results)
        if not self.idle():
            return False
        # the groups are read once a pass: a run forgotten or read again during another intent's call is not sent (M3)
        fitted = [n for n in fitted if (now := l.get(n['id'])) and not now['valid_to'] and not l.forgotten(n['id'])]
        if not fitted:
            return True                              # its runs changed: the next pass sees them
        sent = [n['id'] for n in fitted]
        batch = 'sleep-' + digest(sorted(sent))
        why: list[str] = []
        try:
            proposals, maker = await self._model(context | {'episodes': [entries[i] for i in sent]}, system, rt)
            accepted, refused = self.apply(proposals, set(sent), batch, maker, intent, procedure_refusals=why,
                                           annotate={i for i in sent if 'annotate' in entries[i]})
        except asyncio.CancelledError:
            raise
        except Exception as e:
            return self._intent_job(intent, group if _waits(e) else group + RETRY, l.redact(f'{type(e).__name__}: {e}'),
                                    results)
        results.append({'intent': intent, 'episodes': sent, 'accepted': accepted, 'refused': refused})
        if proposals.get('procedures') and len(why) == len(proposals['procedures']):
            # every procedure it proposed was refused: the job says why, and waits for the runs to change (I2)
            return self._intent_job(intent, group, l.redact('every procedure proposed was refused: ' + '; '.join(why)),
                                    results)
        return self._intent_job(intent, group, None, results)

    def _context(self, about: dict[str, Any], members: list[dict[str, Any]],
                 procedures: dict[str, Any]) -> dict[str, Any]:
        """What a call carries besides the runs: the intent, its procedure (`procedures`) and current lessons (spec 3),
        their runs named by each run's current card (a card read again has a new id), the owner's refusals, and
        `procedure_forgotten` when the owner forgot the intent's procedure (M2: `Learning.put` never stores another)."""
        l = self.learning
        now = {n['key']: n['id'] for n in members}

        def current(ref: Any) -> Any:
            node = l.get(ref) if isinstance(ref, str) else None
            return now.get(str(node['key']), ref) if node and node['kind'] == 'episode' else ref
        shown = [p | {'body': p['body'] | {'examples': [
            e | {'episode': current(e.get('episode'))} if isinstance(e, dict) else e
            for e in p['body'].get('examples') or []]}} for p in procedures['existing_procedures']]
        lessons = [{'id': n['id'], 'body': n['body'] | {k: current(n['body'].get(k)) for k in ('failure', 'success')
                                                        if k in n['body']}} for n in self._lessons(set(now))]
        out = {'intent': about, **procedures, 'existing_procedures': shown, 'existing_lessons': lessons}
        if l.db.conn.execute('SELECT 1 FROM learning_forgotten WHERE id = ?',
                             ('key-' + digest(['procedure', about['id']]),)).fetchone():
            out['procedure_forgotten'] = True
        return out

    def _lessons(self, tasks: set[str]) -> list[dict[str, Any]]:
        """The current lessons built on runs of these tasks (any version of their cards), through the earlier versions
        of a lesson (the owner's feedback and edits cite the version before): lineage, never words (R2)."""
        l = self.learning
        out = []
        for lesson in l.items('lesson'):
            seen: set[str] = set()
            frontier = list(lesson['sources'])
            while frontier:
                ref = frontier.pop()
                node = None if ref in seen else l.get(ref)
                seen.add(ref)
                if node is not None and node['kind'] == 'episode' and node['key'] in tasks:
                    out.append(lesson)
                    break
                if node is not None and node['kind'] == 'lesson':
                    frontier.extend(node['sources'])
        return out

    def _intent_job(self, intent: str, inputs: str, error: str | None, results: list[dict[str, Any]]) -> bool:
        if error:
            results.append({'intent': intent, 'error': error})
        self._job('intent:' + intent, inputs, error)
        return True

    def _job(self, source: str, group: str, error: str | None) -> None:
        l = self.learning
        l.db.conn.execute('INSERT INTO learning_jobs (source_ref, status, at, error, inputs) VALUES (?,?,?,?,?) '
                          'ON CONFLICT(source_ref) DO UPDATE SET status=excluded.status, at=excluded.at, '
                          'error=excluded.error, inputs=excluded.inputs',
                          (source, 'failed' if error else 'done', l.now(), error, group))

    def _procedures(self, intent: str) -> dict[str, Any]:
        """The intent's own procedure (keyed by its id, L29): the current one to revise, and those the owner turned
        down, by their intent text, never to be proposed again (M3), with the identity suggestions turned down."""
        l = self.learning
        current, rejected = [], []
        for row in l.db.conn.execute("SELECT id FROM learning_nodes WHERE kind = 'procedure' AND key = ? "
                                     "ORDER BY recorded_at, id", (intent,)).fetchall():
            p = l.get(row[0])
            if p is None:
                continue
            if p['state'] == 'retired':
                rejected.append({'intent': p['body'].get('intent', p['key'])})
            elif not p['valid_to'] and p['state'] != 'rolled_back':
                current.append({'id': p['id'], 'state': p['state'], 'body': p['body']})
        rejected += [{'identity': p['body'].get('text', '')} for p in l.items('identity_proposal', history=True)
                     if p['state'] == 'retired']
        return {'existing_procedures': current, 'rejected_by_owner': rejected}

    def apply(self, proposals: dict[str, Any], available: set[str], batch: str, maker: str,
              intent: str | None = None, annotate: set[str] | None = None,
              procedure_refusals: list[str] | None = None) -> tuple[list[str], list[str]]:
        """Check each proposal of a PROPOSAL_SCHEMA answer against storage; a refused one never stops the others. A
        procedure is keyed by `intent`, the intent its runs were labelled with (L29), when given; each procedure's
        refusal is also added to `procedure_refusals`. Only the runs in `annotate`, when given, are annotated (M5: the
        others are annotated already); an annotation of another is ignored. A revised lesson (the same scope and
        trigger) keeps the owner's helpful/harmful counts."""
        accepted, refused = [], []
        l = self.learning
        for p in proposals.get('episodes', []):
            if annotate is not None and isinstance(p, dict) and p.get('episode') not in annotate:
                continue
            try:
                p = p | {'slots': to_map(p['slots'], json_text=True)}
                if p['episode'] not in available:
                    raise LearningRefused('annotation needs a supplied episode')
                accepted.append(l.put('episode_annotation', p['episode'], p, [p['episode']],
                                       made_by=maker, batch=batch, state='active')['id'])
            except (LearningRefused, KeyError, TypeError) as e:
                refused.append(str(e))
        for p in proposals.get('procedures', []):
            try:
                p = _procedure(p)
                if any(e['episode'] not in available for e in p['examples']):
                    raise LearningRefused('procedure cites evidence outside its supplied context')
                accepted.append(l.propose_procedure(p, batch, maker, intent_id=intent)['id'])
            except (LearningRefused, KeyError, TypeError) as e:
                refused.append(str(e))
                if procedure_refusals is not None:
                    procedure_refusals.append(str(e))
        for p in proposals.get('lessons', []):
            try:
                a, b = l.get(p['failure']), l.get(p['success'])
                if p['failure'] not in available or p['success'] not in available or not a or not b:
                    raise LearningRefused('lesson contrast must cite supplied episodes')
                feedback = a['body'].get('feedback') or []
                failed = a['body'].get('status') == 'failed' or bool(feedback and feedback[-1].get('verdict') == 'fail')
                if not failed or a['body'].get('verified') is True or b['body'].get('verified') is not True:
                    raise LearningRefused('lesson needs failed-to-verified-success contrast')
                if not any(p.get('scope', {}).values()) or not p.get('trigger'):
                    raise LearningRefused('lesson needs an app/site/intent scope and trigger')
                key = encoded(p['scope']) + ':' + p['trigger']
                body = p | self._lesson_counts(key)
                accepted.append(l.put('lesson', key, body, [a['id'], b['id']], made_by=maker, batch=batch)['id'])
            except (LearningRefused, KeyError, TypeError) as e:
                refused.append(str(e))
        for p in proposals.get('identity', []):
            try:
                if not p['evidence'] or any(s not in available for s in p['evidence']):
                    raise LearningRefused('identity proposal needs supplied evidence')
                accepted.append(l.put('identity_proposal', p['key'], {'text': p['text']}, p['evidence'],
                                       made_by=maker, batch=batch)['id'])
            except (LearningRefused, KeyError, TypeError) as e:
                refused.append(str(e))
        l.publish('memory.consolidated', {'batch': batch, 'accepted': accepted, 'refused': refused})
        return accepted, refused

    def _lesson_counts(self, key: str) -> dict[str, int]:
        """The owner's helpful/harmful counts of the lesson a proposal with this key revises: the current version
        `Learning.put` supersedes (never an owner's own lesson, which it doesn't); zero for a new lesson."""
        l = self.learning
        row = l.db.conn.execute("SELECT id FROM learning_nodes WHERE kind = 'lesson' AND key = ? AND valid_to IS NULL "
                                "ORDER BY recorded_at DESC, rowid DESC LIMIT 1", (l.redact(key),)).fetchone()
        prev = l.get(row[0]) if row else None
        if prev is None or prev['origin'] == origin.OWNER:
            return {'helpful': 0, 'harmful': 0}
        return {k: int(prev['body'].get(k) or 0) for k in ('helpful', 'harmful')}

    async def _rollups_off_loop(self) -> None:
        """The rollups (I4) in a worker thread on a connection of their own, never on the event loop; they stop between
        days once a task starts. What they publish is published on the loop afterwards."""
        l = self.learning
        events: list[tuple[Any, ...]] = []
        loop = asyncio.get_running_loop()

        async def idle_now() -> bool:
            return self.idle()

        def stop() -> bool:                          # asked on the loop: its task state belongs to it
            try:
                return not asyncio.run_coroutine_threadsafe(idle_now(), loop).result()
            except Exception:
                return True

        def work() -> None:
            from ferb.store.db import Database
            db = Database(l.db.path)
            try:
                own = Learning(db, l.clock, lambda *event: events.append(event), l.redact, l.redact_id)
                self.rollups(own, stop=stop)
            finally:
                db.close()
        await asyncio.to_thread(work)
        for event in events:
            l.publish(*event)
        # A forget on the loop may have deleted a source after the thread read it: what this pass wrote on a forgotten
        # source goes too, with what is built on it (review minor: rollup/forget race)
        written = {e[1]['id'] for e in events if len(e) > 1 and e[0] == 'memory.derived' and isinstance(e[1], dict)}
        doomed = [i for i in sorted(written) if (n := l.get(i)) and any(l.forgotten(s) for s in n['sources'])]
        if doomed:
            self.forget(doomed)

    def rollups(self, learning: Learning | None = None, stop: Callable[[], bool] = lambda: False) -> None:
        """Index each day summary and narrative, roll episode cards up by day, week, month and year, and summarize facts
        by entity. A day summary is indexed again only when its journal row changed (a rebuilt summary has a new one),
        so a pass builds only new days (I4)."""
        l = learning or self.learning
        groups: dict[tuple[str, str], list[dict[str, Any]]] = {}
        from ferb.memory.summary import SUMMARY_KIND, NARRATIVE_KIND, build
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
                l.put('activity_summary', row['kind'] + ':' + day, data, [
                    'journal:' + row['id'], *sorted(raw_refs)], made_by=str(data.get('made_by') or 'code:summary-index/1'),
                    state='active')
            except LearningRefused:
                continue
        for node in l.items('episode'):
            at = node['body'].get('ended_at') or node['recorded_at']
            day = at[:10]
            groups.setdefault(('day', day), []).append(node)
        # Each level explicitly cites its direct children. No raw source deletion breaks the evidence cards.
        for level in ('day', 'week', 'month', 'year'):
            for (kind, key), children in list(groups.items()):
                if kind != level:
                    continue
                body = {'period': level, 'text': '\n'.join(n['key'] for n in children),
                        'children': [n['id'] for n in children], 'count': len(children)}
                try:
                    node = l.put('summary', level + ':' + key, body, body['children'], made_by='code:rollup/1', state='active')
                except LearningRefused:
                    continue
                if level == 'day':
                    from datetime import date
                    year, week, _ = date.fromisoformat(key).isocalendar()
                    groups.setdefault(('week', f'{year}-W{week:02}'), []).append(node)
                    groups.setdefault(('month', key[:7]), []).append(node)
                elif level == 'month':
                    groups.setdefault(('year', key[:4]), []).append(node)
        entities: dict[str, list[dict[str, Any]]] = {}
        for node in l.items('fact'):
            if node['state'] == 'proposed':          # waits for the owner's Yes: in no summary a prompt can reach (F3)
                continue
            entities.setdefault(str(node['body'].get('subject', 'owner')), []).append(node)
        for subject, children in entities.items():
            try:
                l.put('entity', subject, {'text': '\n'.join(str(n['body'].get('content', '')) for n in children)},
                      [n['id'] for n in children], made_by='code:entity-summary/1', state='active')
            except LearningRefused:
                continue

    async def _model(self, context: dict[str, Any], system: str, rt: Any) -> tuple[dict[str, Any], str]:
        ctx = encoded(context)
        task = Task('sleep-' + digest(sorted(e['id'] for e in context['episodes'])), None, 'memory consolidation',
                    'local', Identity.OWNER, set(), 'verifier', 'sleep', [], None)
        # within the model's window and the runtime's character cap (L28); recorded as a model.learning span
        out = json.loads(await learning_call(rt, task, system, ctx, 'redacted learning evidence', PROPOSAL_SCHEMA,
                                             role='consolidate', spans=self.spans, usage=self.usage))
        maker = f'model:{rt.name}:{getattr(rt, "model", None) or "default"}|prompt:consolidate@{hashlib.sha256(system.encode()).hexdigest()}'
        return out, maker


def _fit_intents(card: dict[str, Any], known: list[dict[str, Any]], system: str, limits: Any,
                 runtime: str) -> list[dict[str, Any]]:
    """The known intents a label call carries beside its card, most recently used first, while they fit the window and
    the runtime's per-input character cap (L28); no count of FERB's own. A card that alone doesn't fit is never
    shortened or sent: LearningRefused says why (its job keeps it)."""
    window = limits.context_budget
    base = encoded({'episode': card_digest(card), 'intents': []})
    room = window - count_tokens(system) - overhead_tokens(window, count_tokens(system)) - count_tokens(base)
    chars = (None if limits.max_input_chars is None else
             limits.max_input_chars - overhead_chars(window, count_tokens(system)) - len(base))
    if room < 0:
        raise LearningRefused("too large for the model's window; sent whole or not at all")
    if chars is not None and chars < 0:
        raise LearningRefused(f"too long for one {runtime} input ({limits.max_input_chars} characters at most); "
                              "sent whole or not at all")
    out, used, used_chars = [], 0, 0
    for intent in known:
        shown = {k: intent[k] for k in ('id', 'name', 'description', 'slots')}
        text = encoded(shown)
        cost, size = count_tokens(text) + count_tokens(','), len(text) + len(',')
        if used + cost > room or (chars is not None and used_chars + size > chars):
            break
        out.append(shown)
        used, used_chars = used + cost, used_chars + size
    return out


def _fit_runs(members: list[dict[str, Any]], required: set[str], entries: dict[str, dict[str, Any]], room: int,
             chars: int | None) -> list[dict[str, Any]] | None:
    """The runs a call carries, as `entries` shows them, within `room` tokens (and `chars`, a runtime's per-input
    character cap, L28): every run in `required` (the procedure's examples), or None when they don't all fit; then the
    others newest first, as many as fit what is left, each whole: a run is left out, never shortened. Returned newest
    first, by when the task ended (a card read again later, e.g. for a verdict, keeps its run's place)."""
    def newest(nodes: list[dict[str, Any]]) -> list[dict[str, Any]]:
        return sorted(nodes, key=lambda n: (n['body'].get('ended_at') or n['recorded_at'], n['id']), reverse=True)
    out, used, used_chars = [], 0, 0
    for node in newest([n for n in members if n['id'] in required]) + newest([n for n in members
                                                                             if n['id'] not in required]):
        text = encoded(entries[node['id']])
        cost, size = count_tokens(text) + count_tokens(','), len(text) + len(',')
        if used + cost > room or (chars is not None and used_chars + size > chars):
            if node['id'] in required:
                return None
            continue
        out.append(node)
        used, used_chars = used + cost, used_chars + size
    return newest(out)
