"""Durable delegated conversations. Observations can wake work, never create its authority."""
import asyncio
import json
from typing import Any, Callable, Literal

from pydantic import BaseModel, Field, model_validator

from ferb.policy.confirm import SEND_CLASS


class ConversationScope(BaseModel):
    identity: Literal['owner', 'ferb']
    account: str = Field(min_length=1)
    site: str = Field(min_length=1)
    app: str = Field(min_length=1)
    thread: str = Field(min_length=1)
    recipients: list[str] = Field(min_length=1)
    topic: str = Field(min_length=1)
    handover_when: str = Field(min_length=1)
    window_id: str | None = None

    @model_validator(mode='after')
    def concrete(self) -> 'ConversationScope':
        if any(not value.strip() for value in [self.account, self.site, self.app, self.thread,
                                                self.topic, self.handover_when, *self.recipients]):
            raise ValueError('Conversation scope needs concrete nonempty identifiers')
        return self

    def approval_text(self) -> str:
        return (f'Continue this conversation as {self.identity}, account {self.account}, on {self.site} '
                f'({self.app}), thread {self.thread}, with {", ".join(self.recipients)}?\n'
                f'Topic: {self.topic}\nHand over when: {self.handover_when}\n'
                'This includes accepting this platform\'s automation terms and account risks for this task. '
                'You can pause, correct the scope or take over at any time. '
                'Message sends still follow the separately displayed standing permission and security checks.')


class ConversationStore:
    def __init__(self, db: Any, clock: Any, redact: Callable[[Any], Any],
                 publish: Callable[[str, dict[str, Any]], None]) -> None:
        self.db, self.clock, self.redact, self.publish = db, clock, redact, publish
        self._wake: dict[str, asyncio.Event] = {}

    def get(self, ident: str) -> dict[str, Any]:
        row = self.db.conn.execute('SELECT * FROM delegated_conversations WHERE id=?', (ident,)).fetchone()
        if row is None:
            raise ValueError('No such delegated conversation')
        result = dict(row)
        result['scope'] = json.loads(result.pop('scope_json'))
        result['handover'] = json.loads(result.pop('handover_json') or 'null')
        return result

    def list(self, tree_id: str | None = None) -> list[dict[str, Any]]:
        sql = 'SELECT id FROM delegated_conversations'
        args = () if tree_id is None else (tree_id,)
        if tree_id is not None:
            sql += ' WHERE tree_id=?'
        return [self.get(row['id']) for row in self.db.conn.execute(sql + ' ORDER BY created_at', args)]

    def open(self, ident: str, tree_id: str, scope: ConversationScope) -> dict[str, Any]:
        encoded = json.dumps(self.redact(scope.model_dump()), sort_keys=True)
        existing = self.db.conn.execute('SELECT tree_id,scope_json FROM delegated_conversations WHERE id=?', (ident,)).fetchone()
        if existing:
            if existing['tree_id'] != tree_id or existing['scope_json'] != encoded:
                raise ValueError('An approved conversation scope cannot be silently replaced; start a newly approved scope')
            return self.get(ident)
        prior = self.db.conn.execute('SELECT id,status FROM delegated_conversations WHERE tree_id=? AND scope_json=?',
                                     (tree_id, encoded)).fetchone()
        if prior is not None:
            raise ValueError('This task already has that conversation scope (' + prior['id'] + '); a new ID cannot repeat its approval or sends')
        at = self.clock.now().isoformat()
        self.db.conn.execute('INSERT INTO delegated_conversations(id,tree_id,scope_json,status,created_at,updated_at) '
                             'VALUES(?,?,?,\'proposed\',?,?)', (ident, tree_id, encoded, at, at))
        return self.get(ident)

    def approve(self, ident: str, approval_id: str, answer: str) -> None:
        if self.get(ident)['status'] != 'proposed' or not approval_id:
            raise ValueError('Only a pending conversation can receive its one owner approval')
        self.db.conn.execute('UPDATE delegated_conversations SET status=?,approval_id=?,updated_at=? WHERE id=?',
            ('active' if answer == 'Yes' else 'declined', approval_id, self.clock.now().isoformat(), ident))
        self._signal(ident)

    def bind(self, ident: str, task_id: str) -> None:
        row = self.get(ident)
        if task_id.split('.', 1)[0] != row['tree_id']:
            raise ValueError('Conversation worker belongs to another task tree')
        self.db.conn.execute('INSERT OR REPLACE INTO conversation_workers VALUES(?,?)', (task_id, ident))

    def guard(self, grant: Any, name: str, cap: Any, spec: Any) -> str | None:
        if getattr(cap, 'observation', False):
            return None
        bound = self.db.conn.execute('SELECT conversation_id FROM conversation_workers WHERE task_id=?',
                                     (grant.task_id,)).fetchone()
        if bound is None:
            # A final synthesizer must not resume a handed-over conversation through an unbound root worker.
            root = grant.task_id.split('.', 1)[0]
            for row in self.list(root):
                if grant.task_id == root + '.worker' and row['status'] in ('handed_over', 'closed'):
                    return 'The final conversation summary is read-only after hand-over; the owner controls further actions.'
                if row['status'] in ('paused', 'handed_over', 'closed', 'declined') and spec.target.site in (
                        row['scope']['site'], row['scope']['app']):
                    return 'This conversation belongs to the owner now; further input is paused.'
            return None
        row = self.get(bound['conversation_id'])
        if row['status'] != 'active':
            return 'This delegated conversation is not active; only the owner can resume it.'
        scope = row['scope']
        if grant.identity.value != scope['identity']:
            return 'This conversation was approved under a different identity.'
        if (spec.target.site or '').casefold() not in (scope['site'].casefold(), scope['app'].casefold()):
            return 'This conversation worker may act only on its approved app/site; use a separate research step.'
        if spec.target.account is not None and spec.target.account != scope['account']:
            return 'The action names an account outside this conversation permission.'
        if spec.target.recipients and sorted(spec.target.recipients) != sorted(scope['recipients']):
            # Typed text occasionally appears in the recipients field to make provenance stricter; only actual
            # send/contact declarations are recipient comparisons. A mere text field cannot authorize a send.
            if spec.kind in SEND_CLASS or spec.declared in SEND_CLASS:
                return 'The action names a recipient outside this conversation permission.'
        if spec.target.contact is not None and spec.target.contact not in scope['recipients']:
            return 'The action names a contact outside this conversation permission.'
        return None

    def events(self, ident: str, after: int = 0) -> list[dict[str, Any]]:
        self.get(ident)
        return [{'seq': row['seq'], 'source_id': row['source_id'], 'origin': row['origin'],
                 'data': json.loads(row['data_json']), 'at': row['at']} for row in self.db.conn.execute(
            'SELECT * FROM conversation_events WHERE conversation_id=? AND seq>? ORDER BY seq', (ident, after))]

    def observe(self, event: dict[str, Any]) -> None:
        if not event.get('id'):
            return
        if self.db.conn.execute('SELECT 1 FROM conversation_forgotten_sources WHERE source_id=?', ('trail:' + event['id'],)).fetchone():
            return
        data = event.get('data', {})
        for row in self.list():
            if row['status'] not in ('active', 'waiting', 'paused'):
                continue
            scope = row['scope']
            if str(data.get('bundle_id', '')).casefold() != scope['app'].casefold():
                continue
            if event.get('source') == 'typed_text' and event.get('actor') == 'owner':
                window = scope.get('window_id')
                if window is None or str(data.get('window_id')) == window:
                    self._status(row['id'], 'paused')
                    self.publish('conversation.paused', {'id': row['id'], 'tree_id': row['tree_id'],
                        'reason': 'owner typed in the conversation app/window', 'source_ref': 'trail:' + event['id']})
                continue
            if event.get('source') not in ('notification', 'conversation_message'):
                continue
            if data.get('conversation') != scope['thread']:
                continue
            if data.get('account') is not None and data['account'] != scope['account']:
                continue
            clean = self.redact(data)
            self.db.conn.execute('INSERT OR IGNORE INTO conversation_events(conversation_id,source_id,origin,data_json,at) '
                'VALUES(?,?,\'external:conversation\',?,?)', (row['id'], 'trail:' + event['id'], json.dumps(clean), self.clock.now().isoformat()))
            self._signal(row['id'])

    def pending(self, ident: str) -> bool:
        row = self.get(ident)
        return bool(self.db.conn.execute('SELECT 1 FROM conversation_events WHERE conversation_id=? AND seq>?',
                                        (ident, row['cursor'])).fetchone())

    def begin_turn(self, ident: str) -> int:
        row = self.get(ident)
        if row['status'] not in ('active', 'waiting') or not row['approval_id']:
            raise ValueError('The owner has not authorized an active conversation')
        cursor = self.db.conn.execute('SELECT COALESCE(MAX(seq),0) FROM conversation_events WHERE conversation_id=?',
                                      (ident,)).fetchone()[0]
        number = row['turn'] + 1
        self.db.conn.execute("BEGIN IMMEDIATE")
        try:
            self.db.conn.execute('UPDATE delegated_conversations SET turn=?,turn_cursor=?,status=\'active\',updated_at=? WHERE id=?',
                                 (number, cursor, self.clock.now().isoformat(), ident))
            self.db.conn.execute('INSERT INTO conversation_turns VALUES(?,?,NULL,\'running\')', (ident, number))
            self.db.conn.execute('COMMIT')
        except BaseException:
            self.db.conn.execute('ROLLBACK')
            raise
        return number

    def finish_turn(self, ident: str, number: int, output: dict[str, Any]) -> None:
        row = self.get(ident)
        previous = self.db.conn.execute('SELECT status FROM conversation_turns WHERE conversation_id=? AND number=?',
                                        (ident, number)).fetchone()
        if previous is not None and previous['status'] == 'evidence_forgotten':
            output = {'status': 'blocked', 'summary': 'Source evidence was forgotten; inspect afresh after owner resume.',
                      'conversation_action': 'blocked', 'evidence': []}
        action = output.get('conversation_action')
        if action in ('handover', 'finished') and not output.get('evidence'):
            raise ValueError('A hand-over or finished conversation needs observed evidence')
        output = self.redact(output)
        # An owner pause arriving during the worker cannot be undone by its later model report.
        status = row['status'] if row['status'] in ('paused', 'handed_over', 'closed') else {
            'wait': 'waiting', 'handover': 'handed_over', 'finished': 'closed'}.get(action, 'blocked')
        if output.get('status') != 'done' and status not in ('paused', 'handed_over', 'closed'):
            status = 'blocked'
        handover = json.dumps(output) if action == 'handover' else None
        self.db.conn.execute('BEGIN IMMEDIATE')
        try:
            self.db.conn.execute('UPDATE conversation_turns SET output_json=?,status=? WHERE conversation_id=? AND number=?',
                                 (json.dumps(output), output.get('status', 'blocked'), ident, number))
            self.db.conn.execute('UPDATE delegated_conversations SET status=?,cursor=turn_cursor,handover_json=COALESCE(?,handover_json),updated_at=? WHERE id=?',
                                 (status, handover, self.clock.now().isoformat(), ident))
            self.db.conn.execute('COMMIT')
        except BaseException:
            self.db.conn.execute('ROLLBACK')
            raise
        self._signal(ident)

    def owner_change(self, ident: str, action: str, owner_words: str) -> None:
        row = self.get(ident)
        if action not in ('pause', 'resume', 'takeover', 'close') or not owner_words.strip():
            raise ValueError('Conversation control needs an owner instruction and supported action')
        if action == 'resume' and (not row['approval_id'] or row['status'] == 'declined'):
            raise ValueError('A declined or unapproved scope needs a new conversation approval')
        self.db.conn.execute('INSERT INTO conversation_owner_changes(conversation_id,action,owner_words,at) VALUES(?,?,?,?)',
            (ident, action, self.redact(owner_words), self.clock.now().isoformat()))
        self._status(ident, {'pause': 'paused', 'resume': 'active', 'takeover': 'handed_over', 'close': 'closed'}[action])

    async def wait(self, ident: str, stopped: Callable[[], bool]) -> bool:
        wake = self._wake.setdefault(ident, asyncio.Event())
        while not stopped():
            wake.clear()
            row = self.get(ident)
            if row['status'] in ('closed', 'handed_over', 'declined', 'blocked'):
                return False
            if row['status'] == 'active' or (row['status'] == 'waiting' and self.pending(ident)):
                return True
            await wake.wait()
        return False

    def wake_all(self) -> None:
        for wake in self._wake.values():
            wake.set()

    def _status(self, ident: str, status: str) -> None:
        self.db.conn.execute('UPDATE delegated_conversations SET status=?,updated_at=? WHERE id=?',
                             (status, self.clock.now().isoformat(), ident))
        self._signal(ident)

    def _signal(self, ident: str) -> None:
        self._wake.setdefault(ident, asyncio.Event()).set()

    def forget_sources(self, refs: set[str]) -> dict[str, int]:
        """Explicit owner forget only. Drop copied evidence and pause delegations before another input can run."""
        affected: set[str] = set()
        events = notices = 0
        self.db.conn.execute('BEGIN IMMEDIATE')
        try:
            for ref in refs:
                self.db.conn.execute('INSERT OR IGNORE INTO conversation_forgotten_sources VALUES(?)', (ref,))
                affected.update(row[0] for row in self.db.conn.execute(
                    'SELECT conversation_id FROM conversation_events WHERE source_id=?', (ref,)))
                events += self.db.conn.execute('DELETE FROM conversation_events WHERE source_id=?', (ref,)).rowcount
            for row in self.db.conn.execute('SELECT id,source_refs_json FROM attention_notices').fetchall():
                if refs.intersection(json.loads(row['source_refs_json'])):
                    self.db.conn.execute('UPDATE attention_notices SET text=?,delivered_at=COALESCE(delivered_at,?) WHERE id=?',
                        ('Source evidence was forgotten.', self.clock.now().isoformat(), row['id']))
                    notices += 1
            for ident in affected:
                self.db.conn.execute("UPDATE conversation_turns SET output_json=NULL,status='evidence_forgotten' WHERE conversation_id=?", (ident,))
                self.db.conn.execute("UPDATE delegated_conversations SET handover_json=NULL,status='paused',updated_at=? WHERE id=?",
                                     (self.clock.now().isoformat(), ident))
            self.db.conn.execute('COMMIT')
        except BaseException:
            self.db.conn.execute('ROLLBACK')
            raise
        for ident in affected:
            self._signal(ident)
        return {'conversation_events': events, 'conversation_evidence': len(affected), 'attention_notices': notices}

    def digest(self, ident: str) -> dict[str, Any]:
        row = self.get(ident)
        turns = [dict(r) for r in self.db.conn.execute('SELECT number,status,output_json FROM conversation_turns '
                                                     'WHERE conversation_id=? ORDER BY number', (ident,))]
        for turn in turns:
            turn['output'] = json.loads(turn.pop('output_json') or 'null')
        sends = [dict(r) for r in self.db.conn.execute('SELECT l.id,l.task_id,l.capability,l.target_json,l.state,l.evidence_json '
            'FROM action_ledger l JOIN conversation_workers w ON w.task_id=l.task_id WHERE w.conversation_id=? ORDER BY l.rowid', (ident,))]
        return {'conversation': row, 'turns': turns, 'actions': sends,
                'warning': 'Ledger done means tool returned; use turn evidence for delivery. Unknown actions must not be repeated automatically.'}


class ConversationRunner:
    """Run one checked turn at a time and sleep without occupying a model session."""
    def __init__(self, store: ConversationStore, approve: Any, turn: Any, emit: Any, stopped: Callable[[], bool]) -> None:
        self.store, self.approve, self.turn, self.emit, self.stopped = store, approve, turn, emit, stopped

    async def run(self, ident: str, tree_id: str, scope: ConversationScope) -> dict[str, Any]:
        row = self.store.open(ident, tree_id, scope)
        if row['status'] == 'proposed':
            answer = await self.approve(scope.approval_text(), ['Yes', 'No'])
            if not answer.get('approval_id'):
                return {'status': 'blocked', 'summary': 'Conversation scope is waiting for your answer.', 'conversation_id': ident}
            self.store.approve(ident, answer['approval_id'], answer.get('answer', ''))
        while not self.stopped():
            row = self.store.get(ident)
            if row['status'] in ('closed', 'handed_over'):
                return {'status': 'done', 'summary': (row['handover'] or {}).get('summary', 'Conversation handed back to you.'),
                        'evidence': (row['handover'] or {}).get('evidence', []), 'conversation_id': ident,
                        'conversation_status': row['status'], 'digest': self.store.digest(ident)}
            if row['status'] in ('declined', 'blocked'):
                return {'status': 'blocked', 'summary': 'The conversation needs your attention.', 'conversation_id': ident}
            if not await self.store.wait(ident, self.stopped):
                continue
            number = self.store.begin_turn(ident)
            result = await self.turn(ident, number, self.store.events(ident, row['cursor']), self.store.digest(ident))
            self.store.finish_turn(ident, number, result)
            current = self.store.get(ident)
            if current['status'] == 'handed_over':
                await self.emit({'type': 'reply', 'task_id': tree_id,
                    'text': 'Conversation handed over: ' + result['summary'], 'evidence': result.get('evidence', [])})
            elif current['status'] == 'waiting':
                await self.emit({'type': 'progress', 'task_id': tree_id, 'text': 'Waiting for a reply in ' + scope.thread,
                                 'conversation_id': ident})
        return {'status': 'cancelled', 'summary': 'Stopped by you.', 'conversation_id': ident}
