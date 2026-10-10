"""Idle, incremental learning from redacted evidence. No tools; every model proposal is checked against storage."""
from __future__ import annotations

import asyncio
import hashlib
import json
from typing import Any, Callable

from ferb.core.context import ContextBlock, assemble_parts, count_tokens
from ferb.core.prompts import load_prompt
from ferb.core.structured import check_final
from ferb.core.types import Identity, ModelRequest, Task
from ferb.memory.learning import Learning, LearningRefused, digest, encoded

PROPOSAL_SCHEMA: dict[str, Any] = {
    'type': 'object', 'additionalProperties': False, 'required': ['procedures', 'lessons', 'identity', 'episodes'],
    'properties': {
        'episodes': {'type': 'array', 'items': {'type': 'object', 'additionalProperties': False,
            'required': ['episode', 'intent_signature', 'slots', 'reflection', 'narrative'], 'properties': {
                'episode': {'type': 'string'}, 'intent_signature': {'type': 'string'},
                'slots': {'type': 'object'}, 'reflection': {'type': 'string'}, 'narrative': {'type': 'string'}}}},
        'procedures': {'type': 'array', 'items': {'type': 'object', 'additionalProperties': False,
            'required': ['intent', 'slots', 'preconditions', 'steps', 'ui_hints', 'examples'],
            'properties': {
                'intent': {'type': 'string'}, 'slots': {'type': 'object', 'additionalProperties': {'type': 'string'}},
                'preconditions': {'type': 'object', 'required': ['apps'], 'additionalProperties': False,
                                  'properties': {'apps': {'type': 'object', 'additionalProperties': {'type': 'string'}}}},
                'steps': {'type': 'array', 'items': {'type': 'object', 'additionalProperties': False,
                    'required': ['tool', 'args', 'expect', 'evidence'], 'properties': {
                        'tool': {'type': 'string'}, 'args': {'type': 'object'}, 'expect': {'type': 'string'},
                        'evidence': {'type': 'array', 'items': {'type': 'string'}}}}},
                'ui_hints': {'type': 'string'},
                'examples': {'type': 'array', 'items': {'type': 'object', 'additionalProperties': False,
                    'required': ['episode', 'slots'], 'properties': {
                        'episode': {'type': 'string'}, 'slots': {'type': 'object'}}}}}}},
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


class Consolidator:
    def __init__(self, learning: Learning, memory: Any, make_runtime: Callable[..., Any], *,
                 idle: Callable[[], bool] = lambda: True,
                 usage: Callable[[str, dict[str, Any]], None] = lambda *args: None) -> None:
        self.learning, self.memory, self.make_runtime = learning, memory, make_runtime
        self.idle, self.usage, self.lock = idle, usage, asyncio.Lock()

    async def catch_up(self) -> dict[str, Any]:
        if self.lock.locked() or not self.idle():
            return {'status': 'busy'}
        async with self.lock:
            l = self.learning
            l.sync_facts(self.memory)
            for row in list(l.db.conn.execute('SELECT task_id FROM episodes ORDER BY ended_at')):
                try:
                    l.capture_episode(row[0])
                except LearningRefused:
                    continue
            pending = [p for p in l.items('episode') if not l.db.conn.execute(
                "SELECT 1 FROM learning_jobs WHERE source_ref=? AND status='done'", (p['id'],)).fetchone()]
            results = []
            for p in pending:
                if not self.idle():
                    break
                # Agent decides same intent. Code retrieves candidate episodes by words, no intent classification.
                pool = {n['id']: n for n in l.search(p['body']['request'], kind='episode')}
                pool[p['id']] = p
                batch = 'sleep-' + digest(sorted(pool))
                try:
                    proposals, maker = await self._model(list(pool.values()))
                    accepted, refused = self.apply(proposals, set(pool), batch, maker)
                    status, error = 'done', None
                    results.append({'source': p['id'], 'accepted': accepted, 'refused': refused})
                except asyncio.CancelledError:
                    raise
                except Exception as e:
                    status, error = 'failed', l.redact(f'{type(e).__name__}: {e}')
                    results.append({'source': p['id'], 'error': error})
                l.db.conn.execute('INSERT INTO learning_jobs VALUES (?,?,?,?) ON CONFLICT(source_ref) DO UPDATE SET '
                                  'status=excluded.status,at=excluded.at,error=excluded.error',
                                  (p['id'], status, l.now(), error))
            self.rollups()
            if l.embedder is not None:
                from ferb.memory.embeddings import index
                try:
                    index(l, l.embedder)
                except Exception as e:
                    l.publish('memory.embedding_unavailable', {'error': l.redact(str(e))})
            return {'status': 'done', 'sources': results}

    def apply(self, proposals: dict[str, Any], available: set[str], batch: str, maker: str) -> tuple[list[str], list[str]]:
        accepted, refused = [], []
        l = self.learning
        for p in proposals.get('episodes', []):
            try:
                if p['episode'] not in available:
                    raise LearningRefused('annotation needs a supplied episode')
                accepted.append(l.put('episode_annotation', p['episode'], p, [p['episode']],
                                       made_by=maker, batch=batch, state='active')['id'])
            except (LearningRefused, KeyError, TypeError) as e:
                refused.append(str(e))
        for p in proposals.get('procedures', []):
            try:
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

    def rollups(self) -> None:
        l = self.learning
        groups: dict[tuple[str, str], list[dict[str, Any]]] = {}
        from ferb.memory.summary import SUMMARY_KIND, NARRATIVE_KIND, build
        for row in l.db.conn.execute('SELECT id,kind,data_json FROM journal WHERE kind IN (?,?)',
                                     (SUMMARY_KIND, NARRATIVE_KIND)):
            data = json.loads(row['data_json'])
            day = data.get('day')
            if not day:
                continue
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

    async def _model(self, cards: list[dict[str, Any]]) -> tuple[dict[str, Any], str]:
        system = load_prompt('memory/consolidate.md')
        ctx = encoded({'episodes': cards, 'existing_procedures': self.learning.items('procedure')})
        rt = self.make_runtime('worker', mode='supervisor', tool_names=[], schema=PROPOSAL_SCHEMA)
        task = Task('sleep-' + digest([c['id'] for c in cards]), None, 'memory consolidation', 'local',
                    Identity.OWNER, set(), 'verifier', 'sleep', [], None)
        parts, cacheable = assemble_parts([ContextBlock(ctx, 'redacted learning evidence', protected=True)],
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
