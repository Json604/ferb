"""P29 live terminal session: background streams never read from the owner's input line."""
import shutil
import textwrap
import threading
import time
from typing import Any, Callable

from ferb.cli.chat import Chat, ReplyDraft, question, render

SECRETS = {'owner_login_code', 'owner_admin_password', 'owner_secret'}


class LiveChat(Chat):
    def __init__(self, *args: Any, **kwargs: Any) -> None:
        super().__init__(*args, **kwargs)
        self.pending: dict[str, dict[str, Any]] = {}
        self.running: dict[str, str] = {}
        self._lock = threading.RLock()
        self._thinking = 0
        self._route_order = threading.Condition()
        self._next_ticket = 0
        self._routing_ticket = 0
        self._streaming_tasks: set[str] = set()
        self._drafts: dict[int, str] = {}
        self.invalidate: Callable[[], None] = lambda: None

    def answer_approval(self, data: dict[str, Any]) -> None:
        with self._lock:
            if data['approval_id'] in self.pending:
                return
            self.pending[data['approval_id']] = data
        if data.get('kind') in SECRETS:
            self.say(question(data).rstrip())
            self.say('   Press Enter on an empty line to enter this privately.')
            return
        # Typed text answers it: FERB matches a reply to its question (L3); its own checks take a plain Yes or No
        self.say(question(data, prompt=False).rstrip())
        self.say('   Reply Yes or No.' if data.get('capability') == 'ferb.confirm' else '   Reply here to answer.')

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
                        self.say('ferb> ' + str((None if self.verbose else data.get('display_text')) or data.get('answer', '')))
                elif topic == 'attention.notice':
                    self.notice(data)
                    self.call('attention.ack', {'ids': [data['id']]})
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
            drafts = list(self._drafts.items())
        tick = '⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏'[int(time.monotonic() * 8) % 10] if count or thinking else '·'
        status = f' {tick} {count} running · {questions} waiting for you · /tasks · Esc stops latest · /quit leaves tasks running '
        if not drafts:
            return status
        # PromptSession's toolbar is a dynamic pane, outside patched stdout's line buffer.
        # Wrap to the actual viewport, retaining every character and every draft, without a content cap.
        width = max(1, shutil.get_terminal_size().columns)
        rows = [status]
        for ticket, draft in drafts:
            text = f'ferb (draft {ticket + 1} · still responding)> ' + draft
            for line in text.split('\n'):
                rows.extend(textwrap.wrap(line, width, replace_whitespace=False, drop_whitespace=False,
                                          break_on_hyphens=False) or [''])
        return '\n'.join(rows)

    def toolbar_fragments(self) -> list[tuple[str, str]]:
        # This is a virtual cursor in the non-focused draft pane, so long text follows its
        # newest line without stealing the real cursor from the owner's input buffer.
        text = self.toolbar()
        return [('', text), ('[SetCursorPosition]', '')] if '\n' in text else [('', text)]

    def _show_draft(self, ticket: int, text: str) -> None:
        with self._lock:
            if text:
                self._drafts[ticket] = text
            else:
                self._drafts.pop(ticket, None)
        self.invalidate()

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
        # Typed text is a message, even with a question open: the planner routes an answer to its question by id
        # (L3, L19). The daemon acknowledges it on receipt (L7).
        with self._lock:
            self._thinking += 1
            ticket = self._next_ticket
            self._next_ticket += 1
        threading.Thread(target=self._stream_turn, args=(text, ticket), daemon=True).start()

    def _stream_turn(self, text: str, ticket: int) -> None:
        client = None
        registered = False
        routed = False                  # the daemon has this message in its arrival order: the next may be sent
        draft = ReplyDraft()
        confirmed_reply: tuple[str, str] | None = None
        with self._route_order:
            self._route_order.wait_for(lambda: ticket == self._routing_ticket)
        try:
            client = self.connect()
            for event in client.stream('chat.send', {'session_id': self.session_id, 'text': text}):
                kind = event.get('type')
                if not routed and kind in ('ack', 'plan', 'reply', 'clarify', 'error'):
                    with self._route_order:
                        self._routing_ticket += 1
                        routed = True
                        self._route_order.notify_all()
                if kind == 'ack':
                    self.say('ferb> ' + str(event.get('text') or ''))
                    continue
                if kind == 'reply_delta':
                    if draft.append(str(event.get('text') or '')):
                        self._show_draft(ticket, draft.text)
                    continue
                if kind == 'reply_abort':
                    if draft.accepting and not draft.final:
                        self.write('   Draft interrupted: ' + str(event.get('reason') or 'The response was not confirmed.') + '\n')
                        draft.discard()
                        self._show_draft(ticket, '')
                    continue
                if kind in ('reply', 'clarify'):
                    canonical = (kind, str((None if self.verbose else event.get('display_text')) or event.get('text') or ''))
                    if canonical == confirmed_reply:
                        continue
                    confirmed_reply = canonical if draft.text else None
                    draft.final = True
                elif kind in ('plan', 'outcome', 'error'):
                    confirmed_reply = None
                    if kind != 'plan':
                        draft.final = True
                if kind in ('plan', 'reply', 'clarify', 'outcome', 'error'):
                    draft.discard()
                    self._show_draft(ticket, '')
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
            self.write('ferb> I lost the connection to this conversation: ' + str(exc) + '\n')
        finally:
            if draft.text and not draft.final:
                self.write('   Draft interrupted: the stream ended before FERB confirmed the response.\n')
            self._show_draft(ticket, '')
            if not routed:
                with self._route_order:
                    self._routing_ticket += 1
                    self._route_order.notify_all()
            with self._lock:
                if not registered:
                    self._thinking -= 1
            if client is not None:
                client.close()
