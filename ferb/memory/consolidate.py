"""Idle, incremental learning from redacted evidence. No tools; every model proposal is checked against storage.

A pass works on what changed (memory review I3): it reads again only the cards whose inputs changed
(`Learning.cards_to_read`), and sends the episodes not yet consolidated to the model grouped by structure (the same
tools and apps: `card_shape`), one call per group, with the group's own procedures. A group the model's real window
can't hold is split into calls that fit; a single card too large for it is never shortened or sent, and its job says
why. A failed job keeps its error and is tried again when its group changes or on `ferb memory consolidate`
(`force`), never on every idle hour. No count or size limit of FERB's own decides any of this."""
from __future__ import annotations

import asyncio
import hashlib
import json
from typing import Any, Awaitable, Callable

from ferb.core.context import ContextBlock, assemble_parts, count_tokens, overhead_tokens, runtime_limits
from ferb.core.errors import Deferred
from ferb.core.prompts import load_prompt
from ferb.core.structured import check_final
from ferb.core.types import Identity, ModelRequest, Task
from ferb.memory.learning import Learning, LearningRefused, card_digest, card_shape, digest, encoded


def _pairs(value: dict[str, Any]) -> dict[str, Any]:
    """An open map in strict form: Codex strict output allows no open object, so a map is a list of {name, value}."""
    return {'type': 'array', 'items': {'type': 'object', 'additionalProperties': False, 'required': ['name', 'value'],
                                       'properties': {'name': {'type': 'string'}, 'value': value}}}


JSON_TEXT = {'type': 'string', 'description': 'the value as JSON text: a string in double quotes, a number, '
                                              'true/false, a list or an object'}
PROPOSAL_SCHEMA: dict[str, Any] = {
    'type': 'object', 'additionalProperties': False, 'required': ['procedures', 'lessons', 'identity', 'episodes'],
    'properties': {
        'episodes': {'type': 'array', 'items': {'type': 'object', 'additionalProperties': False,
            'required': ['episode', 'intent_signature', 'slots', 'reflection', 'narrative'], 'properties': {
                'episode': {'type': 'string'}, 'intent_signature': {'type': 'string'},
                'slots': _pairs(JSON_TEXT), 'reflection': {'type': 'string'}, 'narrative': {'type': 'string'}}}},
        'procedures': {'type': 'array', 'items': {'type': 'object', 'additionalProperties': False,
            'required': ['intent', 'slots', 'preconditions', 'steps', 'ui_hints', 'examples'],
            'properties': {
                'intent': {'type': 'string'},
                'slots': _pairs({'type': 'string', 'description': "the slot's JSON type: string, number, boolean, "
                                                                  "array or object"}),
                'preconditions': {'type': 'object', 'required': ['apps'], 'additionalProperties': False,
                                  'properties': {'apps': _pairs({'type': 'string',
                                                                 'description': 'the captured app version'})}},
                'steps': {'type': 'array', 'items': {'type': 'object', 'additionalProperties': False,
                    'required': ['tool', 'args', 'expect', 'evidence'], 'properties': {
                        'tool': {'type': 'string'}, 'args': _pairs(JSON_TEXT), 'expect': {'type': 'string'},
                        'evidence': {'type': 'array', 'items': {'type': 'string'}}}}},
                'ui_hints': {'type': 'string'},
                'examples': {'type': 'array', 'items': {'type': 'object', 'additionalProperties': False,
                    'required': ['episode', 'slots'], 'properties': {
                        'episode': {'type': 'string'}, 'slots': _pairs(JSON_TEXT)}}}}}},
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


def _map(pairs: list[dict[str, Any]], *, json_text: bool = False) -> dict[str, Any]:
    """The map a strict {name, value} list stands for. A repeated name is refused; a value that isn't JSON text is
    the literal string (a model often writes a bare string where JSON text was asked for)."""
    out: dict[str, Any] = {}
    for pair in pairs:
        name, value = pair['name'], pair['value']
        if name in out:
            raise LearningRefused(f'{name} is given twice')
        if json_text:
            try:
                value = json.loads(value)
            except ValueError:
                pass                        # not JSON text: the string as written
        out[name] = value
    return out


def _procedure(p: dict[str, Any]) -> dict[str, Any]:
    """A proposed procedure in the shape Learning checks and stores: its slots, apps and args as maps."""
    return p | {'slots': _map(p['slots']),
                'preconditions': p['preconditions'] | {'apps': _map(p['preconditions']['apps'])},
                'steps': [s | {'args': _map(s['args'], json_text=True)} for s in p['steps']],
                'examples': [e | {'slots': _map(e['slots'], json_text=True)} for e in p['examples']]}


class Consolidator:
    def __init__(self, learning: Learning, memory: Any, make_runtime: Callable[..., Any], *,
                 idle: Callable[[], bool] = lambda: True,
                 usage: Callable[[str, dict[str, Any]], None] = lambda *args: None,
                 capture: Callable[[str], Awaitable[Any]] | None = None,
                 cards_refreshed: Callable[[], None] = lambda: None) -> None:
        """`capture`: refreshes one task's episode card. The daemon's reads it in the card reader process, off the event
        loop (L28); without one, the card is refreshed in-process. `cards_refreshed`: told once every card has been
        refreshed (the daemon then records procedure receipts a shutdown deferred)."""
        self.learning, self.memory, self.make_runtime = learning, memory, make_runtime
        self.idle, self.usage, self.lock = idle, usage, asyncio.Lock()
        self.capture = capture or self._capture_here
        self.cards_refreshed = cards_refreshed

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
                except LearningRefused as e:         # waits for its inputs to change (or `force`)
                    l.card_read(task_id, l.card_inputs(task_id).get(task_id, ''), l.redact(str(e)) or 'refused')
                except Deferred:                     # FERB is stopping: the next start's pass reads the rest
                    return {'status': 'stopping'}
                await asyncio.sleep(0)               # the loop runs between episodes
            self.cards_refreshed()
            results: list[dict[str, Any]] = []
            for members in self._groups():
                done = await self._consolidate(members, force, results)
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

    async def _consolidate(self, members: list[dict[str, Any]], force: bool, results: list[dict[str, Any]]) -> bool:
        """One group: its cards not yet consolidated, with its already consolidated cards as examples where the window
        has room, in as many calls as the window needs. False when a task started (the next pass goes on)."""
        l = self.learning
        jobs = {r[0]: (r[1], r[2]) for r in l.db.conn.execute('SELECT source_ref, status, inputs FROM learning_jobs')}
        group = digest(sorted(n['id'] for n in members))
        pending = [n for n in members if n['id'] not in jobs or
                   (jobs[n['id']][0] == 'failed' and (force or jobs[n['id']][1] != group))]
        if not pending:
            return True
        waiting = {n['id'] for n in pending}
        examples = [n for n in reversed(members) if jobs.get(n['id'], ('',))[0] == 'done']
        procedures = self._procedures(members)
        system = load_prompt('memory/consolidate.md')
        try:
            rt = self.make_runtime('worker', mode='supervisor', tool_names=[], schema=PROPOSAL_SCHEMA)
            window = runtime_limits(rt).context_budget
        except asyncio.CancelledError:
            raise
        except Exception as e:                       # no runtime or no documented window: every pending job says so
            self._record(pending, group, f'{type(e).__name__}: {e}', results)
            return True
        room = (window - count_tokens(system) - overhead_tokens(window, count_tokens(system)) -
                count_tokens(encoded({'episodes': [], **procedures})))
        chunks, too_big = _chunks(pending, examples, room)
        for node in too_big:
            self._record([node], group, "too large for the model's window; sent whole or not at all", results)
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
                p = p | {'slots': _map(p['slots'], json_text=True)}
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

        def work() -> None:
            from ferb.store.db import Database
            db = Database(l.db.path)
            try:
                own = Learning(db, l.clock, lambda *event: events.append(event), l.redact, l.redact_id)
                self.rollups(own, stop=lambda: not self.idle())
            finally:
                db.close()
        await asyncio.to_thread(work)
        for event in events:
            l.publish(*event)

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
        task = Task('sleep-' + digest([c['id'] for c in cards]), None, 'memory consolidation', 'local',
                    Identity.OWNER, set(), 'verifier', 'sleep', [], None)
        parts, cacheable = assemble_parts([ContextBlock(ctx, 'redacted learning evidence', protected=True)],
                                          budget_tokens=runtime_limits(rt).context_budget,   # the model's window (L28)
                                          reserved_tokens=count_tokens(system))
        run = await rt.start(task, ModelRequest(system, tuple(parts), cacheable), '')
        final = ''
        try:
            async for event in rt.events(run):
                if event.kind == 'final':
                    final = str(event.data.get('text', ''))
                elif event.kind == 'usage':
                    self.usage(rt.name, dict(event.data))
                elif event.kind in ('error', 'violation'):
                    raise RuntimeError(str(event.data))
        finally:
            await rt.cancel(run)
        out = json.loads(check_final(final, PROPOSAL_SCHEMA))
        maker = f'model:{rt.name}:{getattr(rt, "model", None) or "default"}|prompt:consolidate@{hashlib.sha256(system.encode()).hexdigest()}'
        return out, maker


def _chunks(pending: list[dict[str, Any]], examples: list[dict[str, Any]],
            room: int) -> tuple[list[list[dict[str, Any]]], list[dict[str, Any]]]:
    """Pending cards in calls that each fit `room` tokens, the last one filled with examples while they fit; and the
    pending cards no call can hold whole."""
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
