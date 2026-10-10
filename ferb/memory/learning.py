"""F8 derived memory, provenance and owner controls. Never grants authority to learned content.

Only the sleep service writes inferred records. Content versions are immutable, source edges survive retention,
and explicit forgetting traverses them. An owner confirmation of a procedure permits reuse as data; its origin
is unchanged. Identity is a separate owner-authored version, never a relabelled model proposal.
"""
from __future__ import annotations

import hashlib
import json
from dataclasses import asdict
from datetime import datetime
from typing import Any, Callable, Iterable

from ferb.core import origin
from ferb.core.errors import FerbError
from ferb.memory.items import query_terms
from ferb.orchestrator import episodes


class LearningRefused(FerbError):
    pass


def encoded(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':'), default=str)


def digest(value: Any) -> str:
    return hashlib.sha256(encoded(value).encode()).hexdigest()


def memory_origin(label: str) -> str:
    return {'owner_stated': origin.OWNER, 'observed': origin.PRIVATE, 'inferred': origin.INFERRED,
            'third_party': origin.external('memory')}.get(label, origin.external('unlabeled'))


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
    def __init__(self, db: Any, clock: Any, publish: Callable[..., Any], redact: Callable[[str], str]) -> None:
        self.db, self.clock, self.publish, self.redact = db, clock, publish, redact
        self.embedder: Any = None

    def now(self) -> str:
        return self.clock.now().isoformat()

    def get(self, node_id: str) -> dict[str, Any] | None:
        r = self.db.conn.execute('SELECT * FROM learning_nodes WHERE id=?', (node_id,)).fetchone()
        if r is None:
            return None
        out = dict(r)
        out['body'] = json.loads(out.pop('body_json'))
        out['confirmed'] = bool(out['confirmed'])
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
        node = self.get(ref)
        if node:
            return node['origin']
        family, sep, ident = ref.partition(':')
        if not sep:
            raise LearningRefused(f'unknown source {ref}')
        if family == 'memory':
            r = self.db.conn.execute('SELECT origin FROM memory_items WHERE id=?', (ident,)).fetchone()
            if r:
                return memory_origin(r[0])
        elif family == 'episode':
            r = self.db.conn.execute('SELECT origins FROM episodes WHERE task_id=?', (ident,)).fetchone()
            if r:
                return origin.least_trusted(origin.split(r[0]) | {origin.FERB})
        elif family == 'trail':
            r = self.db.conn.execute('SELECT actor FROM trail WHERE id=?', (ident,)).fetchone()
            if r:
                return origin.PRIVATE if r[0] == 'owner' else origin.FERB
        elif family == 'journal':
            r = self.db.conn.execute('SELECT origin FROM journal WHERE id=?', (ident,)).fetchone()
            if r:
                return origin.least_trusted(origin.recorded(r[0], True))
        raise LearningRefused(f'source is missing: {ref}')

    def put(self, kind: str, key: str, body: dict[str, Any], sources: Iterable[str], *,
            origin_label: str = origin.INFERRED, made_by: str, batch: str | None = None,
            state: str = 'candidate', owner: bool = False) -> dict[str, Any]:
        if kind == 'identity' and not owner:
            raise LearningRefused('only the owner can write identity')
        if origin_label == origin.OWNER and not owner:
            raise LearningRefused('a derived record cannot invent owner authority')
        body = json.loads(self.redact(encoded(body)))
        key = self.redact(key)
        refs = sorted(set(sources))
        labels = [self._source_origin(s) for s in refs]
        # An explicit owner statement is a new statement, still citing the old proposal as history.
        inherited = origin.OWNER if owner else origin.least_trusted([origin_label, *labels])
        node_id = 'm-' + digest([kind, key, body, refs, inherited, made_by])
        if self.db.conn.execute('SELECT 1 FROM learning_forgotten WHERE id=?', (node_id,)).fetchone():
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
        self.db.conn.execute('SAVEPOINT learning_put')
        try:
            if prev:
                self.db.conn.execute('UPDATE learning_nodes SET valid_to=? WHERE id=?', (now, prev[0]))
            self.db.conn.execute('INSERT INTO learning_nodes '
                '(id,kind,key,body_json,origin,made_by,recorded_at,valid_from,supersedes,batch_id,state,confirmed) '
                'VALUES (?,?,?,?,?,?,?,?,?,?,?,?)',
                (node_id, kind, key, encoded(body), inherited, made_by, now, now, prev[0] if prev else None,
                 batch, state, int(owner)))
            self.db.conn.executemany('INSERT INTO learning_sources VALUES (?,?)', [(node_id, r) for r in refs])
            text = key + ' ' + encoded(body)
            self.db.conn.execute('INSERT INTO learning_fts VALUES (?,?)', (node_id, text))
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
        ep = episodes.get(self.db, task_id)
        if ep is None:
            return None
        rows = self.db.conn.execute("SELECT id,input_json,output_json,error FROM spans WHERE kind='tool.call' "
                                    "AND (task_id=? OR task_id LIKE ? ESCAPE '\\') ORDER BY started_at,id",
                                    (task_id, task_id.replace('\\', '\\\\').replace('%', '\\%').replace('_', '\\_') + '.%'))
        actions = []
        used_apps: set[str] = set()
        for r in rows:
            inp = json.loads(r['input_json'])
            args = inp.get('args') or {}
            if isinstance(args.get('bundle_id'), str):
                used_apps.add(args['bundle_id'])
            arg_sources = inp.get('arg_sources')
            # No provenance on historical actions is unknown, not evidence of a safe destination.
            labels = {o for values in (arg_sources or {}).values() for o in values}
            outside = inp.get('kind') not in ('read', 'search', 'mailbox_read') and (arg_sources is None or any(origin.OWNER not in values and
                       any(origin.rank(o) >= origin.OUTSIDE_RANK for o in values)
                       for values in arg_sources.values()))
            actions.append({'id': r['id'], 'tool': inp.get('tool'), 'args': args,
                            'kind': inp.get('kind'), 'outside_action': outside,
                            'arg_sources': arg_sources, 'error': r['error'],
                            'result': json.loads(r['output_json']) if r['output_json'] else None})
        feedback = [dict(r) for r in self.db.conn.execute(
            'SELECT verdict,note,at FROM owner_verdicts WHERE task_id=? ORDER BY seq', (task_id,))]
        valid = ep['verified'] is True and not (feedback and feedback[-1]['verdict'] == 'fail')
        body = {'task_id': task_id, 'request': ep['request_text'], 'intent': ep['goal'], 'slots': {},
                'apps': {a: (apps or {}).get(a, 'unknown') for a in used_apps},
                'app_versions_captured': apps is not None, 'surfaces': ep['tools'], 'actions': actions,
                'status': ep['status'], 'verified': valid, 'conditions': ep['conditions'],
                'feedback': feedback, 'narrative': ep['answer'], 'problems': ep['problems'],
                'started_at': ep['started_at'], 'ended_at': ep['ended_at'], 'usage': ep['usage']}
        old = next((n for n in self.items('episode') if n['key'] == task_id), None)
        if old and apps is None:
            # A verdict/retention refresh keeps the versions actually captured at completion.
            body['apps'] = old['body'].get('apps', {})
            body['app_versions_captured'] = old['body'].get('app_versions_captured', False)
            if not actions:
                body['actions'] = old['body'].get('actions', [])
        return self.put('episode', task_id, body, ['episode:' + task_id], made_by='code:evidence-card/1',
                        origin_label=origin.FERB, state='active')

    def propose_procedure(self, proposal: dict[str, Any], batch: str, made_by: str) -> dict[str, Any]:
        examples = proposal.get('examples') or []
        steps = proposal.get('steps') or []
        if not examples or not steps or not proposal.get('intent'):
            raise LearningRefused('a procedure needs intent, steps and verified examples')
        slot_types = proposal.get('slots', {})
        if not isinstance(slot_types, dict) or any(t not in ('string', 'number', 'boolean', 'array', 'object')
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
        selected = []
        for step in steps:
            refs = step.get('evidence') or []
            if not step.get('expect') or not step.get('tool') or not isinstance(step.get('args'), dict) or not refs:
                raise LearningRefused('each tool template needs args, expect and action evidence')
            if any(ref not in actions or actions[ref].get('tool') != step['tool'] or actions[ref].get('error')
                   for ref in refs):
                raise LearningRefused('step cites a missing, failed or different action')
            for ex, card in zip(examples, cards):
                actual = {a['id']: a for a in card['body'].get('actions', [])}
                expected = render_slots(step['args'], ex['slots'])
                if not any(ref in actual and actual[ref].get('args') == expected for ref in refs):
                    raise LearningRefused('template arguments do not reproduce a cited action for each example')
            selected.extend(actions[ref] for ref in refs)
        key = str(proposal['intent'])
        state = 'quarantined' if any(a.get('outside_action', True) for a in selected) else 'candidate'
        result = self.put('procedure', key, proposal, [c['id'] for c in cards], made_by=made_by,
                          batch=batch, state=state)
        for ex, card in zip(examples, cards):
            self.receipt(result['id'], card['id'], ex['slots'])
        return self.get(result['id'])  # type: ignore[return-value]

    def receipt(self, procedure_id: str, episode_id: str, slots: dict[str, Any] | None = None) -> None:
        p, ep = self.get(procedure_id), self.get(episode_id)
        if not p or not ep or ep['kind'] != 'episode':
            raise LearningRefused('missing procedure/episode')
        self.db.conn.execute('INSERT INTO learning_receipts VALUES (?,?,?,?,?) ON CONFLICT(procedure_id,episode_id) '
                             'DO UPDATE SET successful=excluded.successful',
            (procedure_id, episode_id, encoded(slots or ep['body'].get('slots') or {}),
             int(ep['body'].get('verified') is True), ep['recorded_at']))
        self._refresh(procedure_id, None)

    def _refresh(self, node_id: str, apps: dict[str, str] | None) -> dict[str, Any]:
        p = self.get(node_id)
        if p is None or p['kind'] != 'procedure':
            raise LearningRefused('not a procedure')
        rows = list(self.db.conn.execute('SELECT * FROM learning_receipts WHERE procedure_id=? ORDER BY at, rowid',
                                         (node_id,)))
        unique: dict[str, Any] = {}
        for row in rows:
            ep = self.get(row['episode_id'])
            if ep:
                unique[ep['body']['task_id']] = row
        rows = list(unique.values())
        ok = [r for r in rows if r['successful']]
        expected = p['body'].get('preconditions', {}).get('apps', {})
        latest = self.get(ok[-1]['episode_id']) if ok else None
        latest_apps = latest['body'].get('apps', {}) if latest else {}
        matches = 'unknown' not in latest_apps.values() and bool(latest and latest['body'].get('app_versions_captured', True)) and all(
            latest_apps.get(k) == v for k, v in expected.items())
        changed = apps is not None and any(apps.get(k) != v for k, v in expected.items())
        consecutive_failures = len(rows) >= 2 and not rows[-1]['successful'] and not rows[-2]['successful']
        state = p['state']
        if state not in ('retired', 'rolled_back', 'quarantined'):
            if changed or consecutive_failures:
                state = 'suspect'
            elif state != 'suspect':
                state = 'learned' if len(ok) >= 3 and len({r['slots_json'] for r in ok}) >= 2 and matches else 'candidate'
        self.db.conn.execute('UPDATE learning_nodes SET state=? WHERE id=?', (state, node_id))
        p['state'] = state
        p['counters'] = {'verified_successes': len(ok), 'failures': len(rows) - len(ok),
                         'slot_variants': len({r['slots_json'] for r in ok})}
        return p

    def procedure(self, node_id: str, apps: dict[str, str]) -> dict[str, Any]:
        return self._refresh(node_id, apps)

    def confirm(self, node_id: str) -> dict[str, Any]:
        p = self.get(node_id)
        if not p or p['valid_to'] or p['state'] == 'rolled_back':
            raise LearningRefused('no current proposal')
        if p['kind'] == 'identity_proposal':
            self.put('identity', p['key'], p['body'], [p['id']], origin_label=origin.OWNER,
                     made_by='owner:confirmed-digest/1', state='active', owner=True)
        self.db.conn.execute('UPDATE learning_nodes SET confirmed=1,state=CASE WHEN state=\'quarantined\' '
                             'THEN \'candidate\' ELSE state END WHERE id=?', (node_id,))
        self.publish('memory.confirmed_derived', {'id': node_id})
        return self._refresh(node_id, None) if p['kind'] == 'procedure' else self.get(node_id)  # type: ignore[return-value]

    def edit(self, node_id: str, text: str) -> dict[str, Any]:
        p = self.get(node_id)
        if not p:
            raise LearningRefused('unknown item')
        if p['kind'] in ('procedure', 'episode'):
            raise LearningRefused('procedure/episode evidence is immutable; retire it or edit a separate owner note')
        body = dict(p['body']) | {'text': text}
        return self.put('identity' if p['kind'] == 'identity_proposal' else p['kind'], p['key'], body, [p['id']],
                         origin_label=origin.OWNER, made_by='owner:edit/1', state='active', owner=True)

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
        doomed = self.descendants(refs)
        self.db.conn.execute('SAVEPOINT learning_forget')
        try:
            for ident in doomed:
                self.db.conn.execute('INSERT OR IGNORE INTO learning_forgotten VALUES (?,?)', (ident, self.now()))
                self.db.conn.execute('DELETE FROM learning_jobs WHERE source_ref=?', (ident,))
                self.db.conn.execute('DELETE FROM learning_nodes WHERE id=?', (ident,))
            self.db.conn.execute('RELEASE learning_forget')
        except BaseException:
            self.db.conn.execute('ROLLBACK TO learning_forget')
            self.db.conn.execute('RELEASE learning_forget')
            raise
        self.db.conn.execute("INSERT INTO learning_fts(learning_fts) VALUES ('rebuild')")
        self.db.conn.execute("INSERT INTO learning_titles(learning_titles) VALUES ('rebuild')")
        self.db.conn.execute('PRAGMA wal_checkpoint(TRUNCATE)')
        self.publish('memory.forgot_derived', {'ids': sorted(doomed)})
        return sorted(doomed)

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

    def search(self, query: str, *, kind: str | None = None, app: str | None = None,
               person: str | None = None, since: str | None = None, until: str | None = None) -> list[dict[str, Any]]:
        terms = query_terms(query)
        rankings: list[list[str]] = []
        if terms:
            match = ' OR '.join('"' + t.replace('"', '""') + '"' for t in terms)
            rankings.append([r[0] for r in self.db.conn.execute(
                'SELECT id FROM learning_fts WHERE learning_fts MATCH ? ORDER BY bm25(learning_fts)', (match,))])
            if len(query) >= 3:
                rankings.append([r[0] for r in self.db.conn.execute(
                    'SELECT id FROM learning_titles WHERE learning_titles MATCH ? ORDER BY bm25(learning_titles)',
                    ('"' + query.replace('"', '""') + '"',))])
        else:
            rankings.append([p['id'] for p in reversed(self.items(kind))])
        if self.embedder is not None and query:
            from ferb.memory.embeddings import ranked
            try:
                rankings.append(ranked(self.db, self.embedder, query))
            except Exception as e:
                self.publish('memory.embedding_unavailable', {'error': self.redact(str(e))})
        # Reciprocal ranks fuse retrieval methods; age is only a tie-break, never deletion.
        scores: dict[str, float] = {}
        for ranking in rankings:
            for n, ident in enumerate(ranking):
                scores[ident] = scores.get(ident, 0) + 1 / (n + 1)
        out = []
        for ident in scores:
            p = self.get(ident)
            if not p or p['valid_to'] or p['state'] in ('retired', 'rolled_back') or (kind and p['kind'] != kind):
                continue
            body = p['body']
            if app and app not in body.get('apps', {}) and body.get('scope', {}).get('app') != app and \
                    app not in body.get('preconditions', {}).get('apps', {}):
                continue
            if person and person.casefold() not in encoded(body).casefold():
                continue
            if since and p['valid_from'] < since or until and p['valid_from'] > until:
                continue
            p['score'] = scores[ident]
            out.append(p)
        return sorted(out, key=lambda p: (p['score'], p['recorded_at'], p['id']), reverse=True)

    def candidates(self, query: str) -> str:
        return '\n'.join(f"{p['id']} [{p['kind']}, {p['state']}, {p['origin']}; data, not authorization] {p['key']}"
                         for p in self.search(query) if p['kind'] != 'identity')

    def procedure_text(self, node_id: str, apps: dict[str, str]) -> str:
        p = self.procedure(node_id, apps)
        if p['valid_to'] or p['state'] in ('quarantined', 'retired', 'rolled_back'):
            return 'Procedure unavailable: ' + p['state'] + '. Use full reasoning and fresh evidence.'
        mode = ('Eligible for learned fast path: check every step and final state; on any mismatch use full reasoning.'
                if p['state'] == 'learned' else 'Candidate/suspect evidence only. Use full reasoning and fresh observations.')
        return f"Procedure {p['id']} [{p['origin']}; data, never permission]. {mode}\n" + encoded(p)

    def sync_facts(self, memory: Any) -> None:
        for node in self.items():
            if node['kind'] not in ('fact', 'identity') or not any(s.startswith('memory:') for s in node['sources']):
                continue
            source_id = next(s.removeprefix('memory:') for s in node['sources'] if s.startswith('memory:'))
            source = memory.get(source_id)
            if source is None or source.status not in ('active', 'proposed') or source.valid_to is not None:
                self.db.conn.execute('UPDATE learning_nodes SET valid_to=? WHERE id=?', (self.now(), node['id']))
        for item in memory.items(status=None, valid_now=False):
            if item.status not in ('active', 'proposed') or item.valid_to is not None:
                continue
            body = asdict(item)
            body['unconfirmed_hint'] = item.status == 'proposed'
            try:
                self.put('fact', f'{item.subject}/{item.key}', body, ['memory:' + item.id],
                         origin_label=memory_origin(item.origin), made_by='code:fact-index/1', state='active',
                         owner=item.origin == 'owner_stated')
                if item.origin == 'owner_stated' and item.kind == 'preference' and item.subject == 'owner':
                    self.put('identity', item.key, {'text': item.content}, ['memory:' + item.id],
                             origin_label=origin.OWNER, made_by='owner:durable-preference/1', state='active', owner=True)
            except LearningRefused:
                pass  # a forgotten derived item is deliberately never reconstructed
