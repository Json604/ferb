"""P29 live terminal session: background streams never read from the owner's input line."""
import random
import threading
import time
from collections import deque
from typing import Any

from ferb.cli.chat import Chat, question, render

# P25 neutral variants from the owner's plan. Avoid the last eight acknowledgements.
ACKNOWLEDGEMENTS = ('Got it.', 'Okay.', 'Understood.', 'Noted.', 'Received.', 'Alright.', 'Sure.',
                    'Okay, noted.', 'Copy that.', 'Let me see.')
SECRETS = {'owner_login_code', 'owner_admin_password'}


class LiveChat(Chat):
    def __init__(self, *args: Any, **kwargs: Any) -> None:
        super().__init__(*args, **kwargs)
        self.pending: dict[str, dict[str, Any]] = {}
        self.running: dict[str, str] = {}
        self.received: deque[str] = deque(maxlen=8)
        self._lock = threading.RLock()
        self._thinking = 0
        self._route_order = threading.Condition()
        self._next_ticket = 0
        self._routing_ticket = 0
        self._streaming_tasks: set[str] = set()

    def answer_approval(self, data: dict[str, Any]) -> None:
        with self._lock:
            if data['approval_id'] in self.pending:
                return
            self.pending[data['approval_id']] = data
        self.say(question(data).rstrip())
        self.say('   Press Enter on an empty line to enter this privately.' if data.get('kind') in SECRETS else
                 f"   /answer {data['approval_id']} <your answer> (or reply normally)")

    def opening(self) -> None:
        super().opening()                         # dynamic answer_approval queues questions, never steals input
        self.refresh()
        threading.Thread(target=self._events, daemon=True).start()

    def _events(self) -> None:
        client = self.connect()
        try:
            for event in client.stream('logs.subscribe', {}):
                topic, data = event.get('topic'), event.get('data', {})
                if topic == 'approval.requested':
                    self.answer_approval(data)
                elif topic == 'approval.resolved':
                    with self._lock:
                        self.pending.pop(data.get('approval_id', ''), None)
                elif topic == 'task.created':
                    with self._lock:
                        self.running[data['task_id']] = data.get('goal', '')
                elif topic == 'task.result':
                    with self._lock:
                        self.running.pop(data['task_id'], None)
                        own_stream = data['task_id'] in self._streaming_tasks
                    if not own_stream:
                        self.say('ferb> ' + str(data.get('answer', '')))
                elif topic == 'attention.notice':
                    self.say('ferb> ' + str(data.get('text', '')))
                    self.call('attention.ack', {'id': data['id']})
                elif topic == 'task.resumed_event' and data.get('type') == 'reply':
                    self.say('ferb> ' + str(data.get('text', '')))
        except Exception:
            return
        finally:
            client.close()

    def refresh(self) -> None:
        status = self.call('chat.status', {'peek': True})
        with self._lock:
            self.running = {r['task_id']: r['goal'] for r in status.get('running', [])}
            active = {q['approval_id'] for q in status.get('approvals', [])}
            self.pending = {k: v for k, v in self.pending.items() if k in active}
        for item in status.get('approvals', []):
            self.answer_approval(item)

    def toolbar(self) -> str:
        with self._lock:
            count, questions, thinking = len(self.running), len(self.pending), self._thinking
        tick = '⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏'[int(time.monotonic() * 8) % 10] if count or thinking else '·'
        return f' {tick} {count} running · {questions} waiting for you · /tasks · Esc stops latest · /quit leaves tasks running '

    def stop_latest(self) -> None:
        with self._lock:
            latest = next(reversed(self.running), None)
        if latest:
            self.call('task.stop', {'task_ids': [latest]})
            self.say('ferb> Stopped, as you asked.')

    def enter_secret(self) -> bool:
        with self._lock:
            data = next((q for q in self.pending.values() if q.get('kind') in SECRETS), None)
        if data is None:
            return False
        import getpass
        import warnings
        with warnings.catch_warnings():
            warnings.simplefilter('error', getpass.GetPassWarning)
            try:
                answer = getpass.getpass(question(data))  # spaces may be part of the password
            except (getpass.GetPassWarning, EOFError, KeyboardInterrupt):
                return True
        self.call('chat.answer', {'approval_id': data['approval_id'], 'text': answer})
        with self._lock:
            self.pending.pop(data['approval_id'], None)
        return True

    def submit(self, text: str) -> None:
        self.speaker.stop()
        if text.startswith('/memory confirm '):
            ident = text.removeprefix('/memory confirm ').strip()
            result = self.call('memory.learning', {'op': 'confirm', 'id': ident})
            self.say('ferb> ' + str(result.get('message', 'Your confirmation is recorded.')))
            return
        if text == '/tasks':
            self.refresh()
            with self._lock:
                rows = list(self.running.items())
            self.say('\n'.join(f'{key}: {goal}' for key, goal in rows) or 'No tasks are running.')
            return
        if text == '/stop' or text.startswith('/stop '):
            ids = text.split()[1:]
            self.call('task.stop', {'task_ids': ids})
            self.say('ferb> Stop requested.')
            return
        if text.startswith('/answer '):
            parts = text.split(' ', 2)
            if len(parts) != 3:
                self.say('Use /answer <question number> <your answer>.')
                return
            with self._lock:
                pending = self.pending.get(parts[1])
            if pending and pending.get('kind') in SECRETS:
                self.say('Press Enter on an empty line to enter that privately.')
                return
            self.call('chat.answer', {'approval_id': parts[1], 'text': parts[2]})
            with self._lock:
                self.pending.pop(parts[1], None)
            return
        with self._lock:
            candidates = [p for p in self.pending.values() if p.get('kind') not in SECRETS]
        if len(candidates) == 1 and text.strip().casefold() in {str(o).casefold() for o in candidates[0].get('options', [])}:
            item = candidates[0]
            self.call('chat.answer', {'approval_id': item['approval_id'], 'text': text})
            with self._lock:
                self.pending.pop(item['approval_id'], None)
            return
        with self._lock:
            ack = random.choice([a for a in ACKNOWLEDGEMENTS if a not in self.received])
            self.received.append(ack)
            self._thinking += 1
            ticket = self._next_ticket
            self._next_ticket += 1
        self.say('ferb> ' + ack)
        threading.Thread(target=self._stream_turn, args=(text, ticket), daemon=True).start()

    def _stream_turn(self, text: str, ticket: int) -> None:
        client = None
        registered = False
        routed = False
        with self._route_order:
            self._route_order.wait_for(lambda: ticket == self._routing_ticket)
        try:
            client = self.connect()
            for event in client.stream('chat.send', {'session_id': self.session_id, 'text': text}):
                if not routed and event.get('type') in ('plan', 'reply', 'clarify', 'error'):
                    with self._route_order:
                        self._routing_ticket += 1
                        routed = True
                        self._route_order.notify_all()
                with self._lock:
                    if event.get('type') == 'plan':
                        self.running[event['task_id']] = event.get('goal', '')
                        self._streaming_tasks.add(event['task_id'])
                        if not registered:
                            self._thinking -= 1
                            registered = True
                    if event.get('type') == 'outcome':
                        self.running.pop(event['task_id'], None)
                if event.get('type') == 'event' and event.get('topic') == 'approval.requested':
                    self.answer_approval(event['data'])
                else:
                    line = render(event, self.verbose)
                    if line:
                        self.say(line)
        except Exception as exc:
            self.say('ferb> I lost the connection to this conversation: ' + str(exc))
        finally:
            if not routed:
                with self._route_order:
                    self._routing_ticket += 1
                    self._route_order.notify_all()
            with self._lock:
                if not registered:
                    self._thinking -= 1
            if client is not None:
                client.close()
