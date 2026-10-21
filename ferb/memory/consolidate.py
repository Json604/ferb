"""Idle, incremental learning from redacted evidence. No tools; every model proposal is checked against storage.

A pass works on what changed (memory review I3): it reads again only the cards whose inputs changed
(`Learning.cards_to_read`), and sends the episodes not yet consolidated to the model grouped by structure (the same
tools and apps: `card_shape`), one call per group, with the group's own procedures and only the consolidated cards
related to the new ones by structure (`_examples`): a cost that follows what changed. A group the model's real window
can't hold is split into calls that fit; a single card too large for it is never shortened or sent, and its job says
why. A failed job keeps its error and is tried again when its group changes or on `ferb memory consolidate`
(`force`), never on every idle hour. No count or size limit of FERB's own decides any of this."""
from __future__ import annotations

import asyncio
import hashlib
import json
from typing import Any, Awaitable, Callable

from ferb.core.context import count_tokens, overhead_chars, overhead_tokens, runtime_limits
from ferb.core.errors import Deferred
from ferb.core.prompts import load_prompt
from ferb.core.types import Identity, Task
from ferb.memory.calls import learning_call
from ferb.memory.learning import Learning, LearningRefused, card_digest, card_shape, digest, encoded
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
            jobs = {r[0]: (r[1], r[2]) for r in l.db.conn.execute('SELECT source_ref, status, inputs FROM learning_jobs')}
            for members in self._groups():
                done = await self._consolidate(members, jobs, force, results)
                if not done:
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

    def _groups(self) -> list[list[dict[str, Any]]]:
        """The current episode cards by structure (`card_shape`), each group oldest first."""
        groups: dict[Any, list[dict[str, Any]]] = {}
        for node in self.learning.items('episode'):
            groups.setdefault(card_shape(node), []).append(node)
        return list(groups.values())

    async def _consolidate(self, members: list[dict[str, Any]], jobs: dict[str, tuple[str, str]], force: bool,
                           results: list[dict[str, Any]]) -> bool:
        """One group: its cards not yet consolidated, with the already consolidated cards related to them by structure
        (`_examples`), in as many calls as the window needs. False when a task started (the next pass goes on)."""
        l = self.learning
        group = digest(sorted(n['id'] for n in members))
        pending = [n for n in members if n['id'] not in jobs or
                   (jobs[n['id']][0] == 'failed' and (force or jobs[n['id']][1] != group))]
        if not pending:
            return True
        waiting = {n['id'] for n in pending}
        done = [n for n in reversed(members) if jobs.get(n['id'], ('',))[0] == 'done']
        examples = self._examples(pending, done, members)
        procedures = self._procedures(members)
        system = load_prompt('memory/consolidate.md')
        try:
            rt = self.make_runtime('worker', mode='supervisor', tool_names=[], schema=PROPOSAL_SCHEMA)
            limits = runtime_limits(rt)
            window = limits.context_budget
        except asyncio.CancelledError:
            raise
        except Exception as e:                       # no runtime or no documented window: every pending job says so
            self._record(pending, group, f'{type(e).__name__}: {e}', results)
            return True
        room = (window - count_tokens(system) - overhead_tokens(window, count_tokens(system)) -
                count_tokens(encoded({'episodes': [], **procedures})))
        if room <= 0:                                # the group's procedures alone fill the window: say that
            self._record(pending, group, "the group's procedures and the prompt alone don't fit the model's window",
                         results)
            return True
        chunks, too_big = _chunks(pending, examples, room)
        for node in too_big:
            self._record([node], group, "too large for the model's window; sent whole or not at all", results)
        if limits.max_input_chars is not None:       # and the runtime's per-input character cap (Codex, L28)
            chunks, too_long = _within_chars(chunks, waiting, limits.max_input_chars -
                                             overhead_chars(window, count_tokens(system)) -
                                             len(encoded({'episodes': [], **procedures})))
            for node in too_long:
                self._record([node], group, f"too long for one {rt.name} input ({limits.max_input_chars} characters "
                             "at most); sent whole or not at all", results)
        for chunk in chunks:
            if not self.idle():
                return False
            batch = 'sleep-' + digest(sorted(n['id'] for n in chunk))
            try:
                proposals, maker = await self._model(chunk, procedures, system, rt)
                accepted, refused = self.apply(proposals, {n['id'] for n in chunk}, batch, maker)
                results.append({'episodes': [n['id'] for n in chunk], 'accepted': accepted, 'refused': refused})
                error = None
            except asyncio.CancelledError:
                raise
            except Exception as e:
                error = l.redact(f'{type(e).__name__}: {e}')
                results.append({'episodes': [n['id'] for n in chunk], 'error': error})
            for node in chunk:
                if node['id'] in waiting:
                    self._job(node['id'], group, error)
        return True

    def _examples(self, pending: list[dict[str, Any]], done: list[dict[str, Any]],
                  members: list[dict[str, Any]]) -> list[dict[str, Any]]:
        """The already consolidated cards a call carries beside the new ones, chosen by structure, never to fill the
        window (fix round, review Q5): those with a new card's action shape (its tools in order, with their argument
        names) whose request shares a term with that card that most of the group doesn't have (the data-driven rule of
        `Memory.recall`), and those the group's procedures and lessons are built on."""
        from ferb.memory.items import query_terms

        def shape(node: dict[str, Any]) -> tuple[Any, ...]:
            return tuple((str(a.get('tool')), tuple(sorted(a.get('args') or {}))) for a in node['body'].get('actions')
                         or [] if isinstance(a, dict))

        def terms(node: dict[str, Any]) -> set[str]:
            return set(query_terms(f"{node['body'].get('request', '')} {node['body'].get('intent', '')}"))
        counts: dict[str, int] = {}
        for node in members:
            for term in terms(node):
                counts[term] = counts.get(term, 0) + 1
        telling = {t for t, n in counts.items() if n <= max(2, len(members) / 2)}
        ids = {n['id'] for n in members}
        cited = {s for kind in ('procedure', 'lesson') for p in self.learning.items(kind) for s in p['sources']} & ids
        wanted = [(shape(p), terms(p) & telling) for p in pending]
        return [n for n in done if n['id'] in cited or any(shape(n) == s and terms(n) & t for s, t in wanted)]

    def _record(self, nodes: list[dict[str, Any]], group: str, error: str, results: list[dict[str, Any]]) -> None:
        results.append({'episodes': [n['id'] for n in nodes], 'error': error})
        for node in nodes:
            self._job(node['id'], group, error)

    def _job(self, source: str, group: str, error: str | None) -> None:
        l = self.learning
        l.db.conn.execute('INSERT INTO learning_jobs (source_ref, status, at, error, inputs) VALUES (?,?,?,?,?) '
                          'ON CONFLICT(source_ref) DO UPDATE SET status=excluded.status, at=excluded.at, '
                          'error=excluded.error, inputs=excluded.inputs',
                          (source, 'failed' if error else 'done', l.now(), error, group))

    def _procedures(self, members: list[dict[str, Any]]) -> dict[str, Any]:
        """The group's own procedures: those built on its episodes or whose steps use only its tools, current ones to
        revise and the ones the owner turned down, never to be proposed again (M3)."""
        ids = {n['id'] for n in members}
        tools = {t for n in members for t in card_shape(n)[0]}
        current, rejected = [], []
        for p in self.learning.items('procedure', history=True):
            steps = {str(s.get('tool')) for s in p['body'].get('steps') or [] if isinstance(s, dict)}
            if not (ids & set(p['sources']) or (steps and steps <= tools)):
                continue
            if p['state'] == 'retired':
                rejected.append({'intent': p['key']})
            elif not p['valid_to'] and p['state'] != 'rolled_back':
                current.append({'id': p['id'], 'state': p['state'], 'body': p['body']})
        rejected += [{'identity': p['body'].get('text', '')} for p in self.learning.items('identity_proposal', history=True)
                     if p['state'] == 'retired']
        return {'existing_procedures': current, 'rejected_by_owner': rejected}

    def apply(self, proposals: dict[str, Any], available: set[str], batch: str, maker: str) -> tuple[list[str], list[str]]:
        """Check each proposal of a PROPOSAL_SCHEMA answer against storage; a refused one never stops the others."""
        accepted, refused = [], []
        l = self.learning
        for p in proposals.get('episodes', []):
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
                accepted.append(l.propose_procedure(p, batch, maker)['id'])
            except (LearningRefused, KeyError, TypeError) as e:
                refused.append(str(e))
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
                body = p | {'helpful': 0, 'harmful': 0}
                accepted.append(l.put('lesson', encoded(p['scope']) + ':' + p['trigger'], body,
                    [a['id'], b['id']], made_by=maker, batch=batch)['id'])
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

    async def _model(self, cards: list[dict[str, Any]], procedures: dict[str, Any], system: str,
                     rt: Any) -> tuple[dict[str, Any], str]:
        ctx = encoded({'episodes': [card_digest(c) for c in cards], **procedures})
        task = Task('sleep-' + digest(sorted(c['id'] for c in cards)), None, 'memory consolidation', 'local',
                    Identity.OWNER, set(), 'verifier', 'sleep', [], None)
        # within the model's window and the runtime's character cap (L28); recorded as a model.learning span
        out = json.loads(await learning_call(rt, task, system, ctx, 'redacted learning evidence', PROPOSAL_SCHEMA,
                                             role='consolidate', spans=self.spans, usage=self.usage))
        maker = f'model:{rt.name}:{getattr(rt, "model", None) or "default"}|prompt:consolidate@{hashlib.sha256(system.encode()).hexdigest()}'
        return out, maker


def _chunks(pending: list[dict[str, Any]], examples: list[dict[str, Any]],
            room: int) -> tuple[list[list[dict[str, Any]]], list[dict[str, Any]]]:
    """Pending cards in calls that each fit `room` tokens, the last one given the related examples (`_examples`) while
    they fit (never padded with others); and the pending cards no call can hold whole."""
    chunks: list[list[dict[str, Any]]] = []
    current: list[dict[str, Any]] = []
    used = 0
    too_big = []
    separator = count_tokens(',')
    for node in pending:
        cost = count_tokens(encoded(card_digest(node))) + separator
        if cost > room:
            too_big.append(node)
            continue
        if current and used + cost > room:
            chunks.append(current)
            current, used = [], 0
        current.append(node)
        used += cost
    for node in examples:
        cost = count_tokens(encoded(card_digest(node))) + separator
        if not current or used + cost > room:
            break
        current.append(node)
        used += cost
    if current:
        chunks.append(current)
    return chunks, too_big


def _within_chars(chunks: list[list[dict[str, Any]]], waiting: set[str],
                  room: int) -> tuple[list[list[dict[str, Any]]], list[dict[str, Any]]]:
    """`_chunks`' calls split again so each call's cards also fit `room` characters (a runtime's per-input cap, L28);
    and the pending cards (`waiting`) no call can hold whole. An example that doesn't fit is left out; a call left with
    examples only is not made."""
    out: list[list[dict[str, Any]]] = []
    too_long = []
    for chunk in chunks:
        current: list[dict[str, Any]] = []
        used = 0
        for node in chunk:
            cost = len(encoded(card_digest(node))) + len(',')
            if cost > room:
                if node['id'] in waiting:
                    too_long.append(node)
                continue
            if current and used + cost > room:
                out.append(current)
                current, used = [], 0
            current.append(node)
            used += cost
        if current:
            out.append(current)
    return [c for c in out if any(n['id'] in waiting for n in c)], too_long
