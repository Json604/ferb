"""P29 chat keeps accepting input during work; secret prompts wait for explicit empty-line Enter."""
import io
import threading

from ferb.cli.live_chat import LiveChat


def test_two_messages_are_routed_in_order_while_first_task_runs():
    started = threading.Event()
    release = threading.Event()
    second = threading.Event()
    sent = []
    class Client:
        def stream(self, method, params):
            sent.append(params['text'])
            if params['text'] == 'Read my session files':
                yield {'type': 'plan', 'task_id': 'sessions', 'goal': 'Read files', 'text': 'Reading.'}
                started.set()
                release.wait(2)
                yield {'type': 'outcome', 'task_id': 'sessions', 'status': 'done', 'answer': 'Read.'}
            else:
                second.set()
                yield {'type': 'reply', 'text': 'The session task is still running.'}
        def close(self): pass
    chat = LiveChat(Client, io.StringIO(), lambda _: '')
    chat.submit('Read my session files')
    assert started.wait(2)
    chat.submit('What are you doing?')
    assert second.wait(2)
    assert sent == ['Read my session files', 'What are you doing?']
    release.set()


def test_secret_approval_queues_without_reading_and_preserves_password_spaces(monkeypatch):
    calls = []
    class Client:
        def call(self, method, params):
            calls.append((method, params))
            return {'state': 'granted'}
        def close(self): pass
    def forbidden(_): raise AssertionError('ordinary input was stolen')
    chat = LiveChat(Client, io.StringIO(), forbidden)
    monkeypatch.setattr('getpass.getpass', forbidden)
    chat.answer_approval({'approval_id': 'secret', 'kind': 'owner_admin_password', 'reason': 'Your Mac password'})
    assert not calls
    monkeypatch.setattr('getpass.getpass', lambda _: '  retained spaces  ')
    assert chat.enter_secret()
    assert calls == [('chat.answer', {'approval_id': 'secret', 'text': '  retained spaces  '})]
