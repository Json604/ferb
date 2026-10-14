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
from ferb.sanitize.redact import REDACTED


class LearningRefused(FerbError):
    pass


# Kinds whose key is one of FERB's own ids: an episode card (its task id) and its annotation (the card's node id).
ID_KEYED = frozenset({'episode', 'episode_annotation'})


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
    def __init__(self, db: Any, clock: Any, publish: Callable[..., Any], redact: Callable[[str], str],
                 redact_id: Callable[[str], str] | None = None) -> None:
        """`redact_id`: redaction for a key that is one of FERB's own ids (ID_KEYED), by structure
        (`Redactor.ident`), so a registered value inside a task id can't alter the card's key; default `redact`."""
        self.db, self.clock, self.publish, self.redact = db, clock, publish, redact
        self.redact_id = redact_id or redact
        self.embedder: Any = None
        # Bumped by every change to an episode card's inputs (a forget, an owner verdict, a write of a card): a card
        # read before one is read again before it is written (L28).
        self.generation = 0

    def now(self) -> str:
        return self.clock.now().isoformat()

    def get(self, node_id: str) -> dict[str, Any] | None:
        r = self.db.conn.execute('SELECT * FROM learning_nodes WHERE id=?', (node_id,)).fetchone()
        if r is None:
            return None
        out = dict(r)
        out['body'] = json.loads(out.pop('body_json'))
        out['origins'] = json.loads(out.pop('origins_json'))
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
        if origin_label == origin.OWNER and not owner:
            raise LearningRefused('a derived record cannot invent owner authority')
        body = json.loads(self.redact(encoded(body)))
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
        return self.write_episode(self.prepare_episode(task_id, apps))

    def write_episode(self, prepared: dict[str, Any] | None) -> dict[str, Any] | None:
        """Write a card `prepare_episode` read: the only write of a capture."""
        if prepared is None:
            return None
        return self.put('episode', prepared['task_id'], prepared['body'], prepared['sources'],
                        made_by='code:evidence-card/1', origin_label=origin.FERB, state='active',
                        labels=set(prepared['labels']))

    def prepare_episode(self, task_id: str, apps: dict[str, str] | None = None) -> dict[str, Any] | None:
        """Every read of an episode card: its body, its resolved lineage and its sources' origins, without writing. The
        daemon runs it in `ferb.memory.card_reader`, a process of its own on a read-only connection (L28);
        `write_episode` writes the result. `damaged`: the refs skipped as altered by redaction (`_card_labels`)."""
        ep = episodes.get(self.db, task_id)
        if ep is None:
            return None
        from ferb.store.spans import tree_where
        where, params = tree_where(task_id)
        rows = self.db.conn.execute("SELECT id,kind,input_json,output_json,error FROM spans "
                                    f"WHERE kind IN ('tool.call','context.exposure') AND {where} ORDER BY started_at,id",
                                    params)
        actions = []
        source_refs = {'episode:' + task_id}
        used_apps: set[str] = set()
        for r in rows:
            inp = json.loads(r['input_json'])
            if r['kind'] == 'context.exposure':
                source_refs.update(str(ref) for ref in inp.get('source_refs', []))
                continue
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
                    result_app = json.loads(r['output_json']) if r['output_json'] else {}
                    bid = result_app.get('bundle_id') if isinstance(result_app, dict) else None
                    used_apps.add(str(bid) if bid else 'unresolved-mac-app')
            if str(inp.get('tool', '')).startswith(('browser.', 'ferb_browser.')):
                # The retained trace may not identify a resolved browser bundle. Preserve the actual captured
                # browser inventory conservatively, never invent which browser version ran a historic action.
                browsers = {k for k in (apps or {}) if k in
                    ('com.google.Chrome', 'com.brave.Browser', 'com.apple.Safari', 'com.microsoft.edgemac')}
                used_apps.update(browsers or {'unresolved-browser'})
            arg_sources = inp.get('arg_sources')
            # No provenance on historical actions is unknown, not evidence of a safe destination.
            outside = inp.get('kind') not in ('read', 'search', 'mailbox_read') and (arg_sources is None or any(origin.OWNER not in values and
                       any(origin.rank(o) >= origin.OUTSIDE_RANK for o in values)
                       for values in arg_sources.values()))
            source_refs.update(self._action_sources(inp, json.loads(r['output_json']) if r['output_json'] else {}))
            result = json.loads(r['output_json']) if r['output_json'] else {}
            outcome_facts = {k: result[k] for k in ('ok', 'status', 'intent_id', 'action_id', 'ledger_id')
                             if isinstance(result, dict) and k in result}
            actions.append({'id': r['id'], 'tool': inp.get('tool'), 'args': args,
                            'kind': inp.get('kind'), 'outside_action': outside,
                            'arg_sources': arg_sources, 'error': r['error'],
                            'result': outcome_facts})
        feedback = [dict(r) for r in self.db.conn.execute(
            'SELECT verdict,note,at FROM owner_verdicts WHERE task_id=? ORDER BY seq', (task_id,))]
        valid = ep['verified'] is True and not (feedback and feedback[-1]['verdict'] == 'fail')
        body = {'task_id': task_id, 'request': ep['request_text'], 'intent': ep['goal'], 'slots': {},
                'apps': {a: (apps or {}).get(a, 'unknown') for a in used_apps},
                'app_versions_captured': apps is not None, 'app_version_scope': 'identified apps and browser inventory', 'surfaces': ep['tools'], 'actions': actions,
                'status': ep['status'], 'verified': valid, 'conditions': ep['conditions'],
                'feedback': feedback, 'narrative': ep['answer'], 'problems': ep['problems'],
                'started_at': ep['started_at'], 'ended_at': ep['ended_at'], 'usage': ep['usage']}
        source_refs = self.resolve(source_refs)          # L28: lineage handles become their complete refs
        old = next((n for n in self.items('episode') if n['key'] == task_id), None)
        if old and apps is None:
            # A verdict/retention refresh keeps the versions actually captured at completion.
            body['apps'] = old['body'].get('apps', {})
            body['app_versions_captured'] = old['body'].get('app_versions_captured', False)
            if not actions:
                body['actions'] = old['body'].get('actions', [])
            # Retained cards keep lineage even when their raw rows have reached retention.
            source_refs.update(old['sources'])
        labels, damaged = self._card_labels(source_refs)
        return {'task_id': task_id, 'body': body, 'sources': sorted(source_refs - set(damaged)),
                'labels': sorted(labels), 'damaged': damaged}

    def _card_labels(self, refs: Iterable[str]) -> tuple[set[str], list[str]]:
        """The origins of a card's sources, and the refs skipped as damaged. A source that can't be found fails the card
        (it may have been forgotten on purpose), except a ref that holds the redaction marker: no FERB id contains '['
        or ']', so it is an id an earlier redaction altered (60 old rows in the owner's journal); it never resolves.
        A skipped ref adds the labels of a source whose content is unknown."""
        labels: set[str] = set()
        damaged: list[str] = []
        for ref in sorted(set(refs)):
            try:
                labels |= self._source_origins(ref)
            except LearningRefused:
                if REDACTED not in ref:
                    raise
                damaged.append(ref)
                # What it held is unknown: never assume it was less private (as for a retained fact's lost event).
                labels |= {origin.PRIVATE, origin.external('unlabeled')}
        return labels, damaged

    def _action_sources(self, inp: dict[str, Any], result: Any) -> set[str]:
        from ferb.memory.lineage import call_sources
        # Missing evidence must remain an edge: dropping it would let a forgotten fact's
        # descendants be reconstructed from old task traces without their deletion lineage.
        # put() validates these refs; retained cards already preserve expired-source origins.
        return call_sources(inp, result)

    def resolve(self, refs: Iterable[str]) -> set[str]:
        """Lineage handles (`entry:`, `task:`; memory/lineage.py) expanded into the refs they stand for, reading each
        conversation entry and each task's lineage once."""
        from ferb.memory.lineage import resolve
        tasks: dict[str, list[str]] = {}

        def entry(ident: str) -> dict[str, Any] | None:
            row = self.db.conn.execute('SELECT data_json FROM journal WHERE id=?', (ident,)).fetchone()
            return json.loads(row[0]) if row else None

        def task(ident: str) -> list[str]:
            if ident not in tasks:
                tasks[ident] = self.task_sources(ident)
            return tasks[ident]
        return resolve(refs, entry, task)

    def task_sources(self, task_id: str) -> list[str]:
        """Code-recorded lineage for a displayed task/history answer, never inferred from its prose. It may hold lineage
        handles; `resolve` expands them."""
        refs: set[str] = set()
        if self.db.conn.execute('SELECT 1 FROM episodes WHERE task_id=?', (task_id,)).fetchone():
            refs.add('episode:' + task_id)
        for row in self.db.conn.execute("SELECT id FROM learning_nodes WHERE kind='episode' AND key=?", (task_id,)):
            node = self.get(row[0])
            refs.add(node['id'])
            refs.update(node['sources'])
        forgotten = 'key-' + digest(['episode', task_id])
        if self.db.conn.execute('SELECT 1 FROM learning_forgotten WHERE id=?', (forgotten,)).fetchone():
            refs.add(forgotten)  # Keep this missing-source marker so a retained answer cannot recreate its card.
        from ferb.store.spans import tree_where
        where, params = tree_where(task_id)      # the task_id index range, never a LIKE scan of every span (L28)
        rows = self.db.conn.execute("SELECT kind,input_json,output_json FROM spans WHERE "
            f"kind IN ('tool.call','context.exposure') AND {where}", params)
        for row in rows:
            inp = json.loads(row['input_json'])
            if row['kind'] == 'context.exposure':
                refs.update(str(ref) for ref in inp.get('source_refs', []))
            else:
                refs.update(self._action_sources(inp, json.loads(row['output_json']) if row['output_json'] else {}))
        return sorted(refs)

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
        self.db.conn.execute('SAVEPOINT learning_forget')
        try:
            for ident in logical:
                self.db.conn.execute('INSERT OR IGNORE INTO learning_forgotten VALUES (?,?)', (ident, self.now()))
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
        self.db.truncate_wal('memory.forget')
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
               person: str | None = None, since: str | None = None, until: str | None = None,
               query_vector: list[int] | None = None, embed: bool = True) -> list[dict[str, Any]]:
        """`query_vector`: the query's embedding, already computed off the event loop (L28); else computed here.
        `embed` False: keyword ranking only (the caller's encode already failed and was reported)."""
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
        if self.embedder is not None and query and embed:
            from ferb.memory.embeddings import ranked
            try:
                rankings.append(ranked(self.db, self.embedder, query, vector=query_vector))
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
