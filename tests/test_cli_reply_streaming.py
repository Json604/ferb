"""The owner's continuous chat shows real provisional text without speaking or replaying it."""
import io
import threading

from ferb.cli.chat import Chat
from ferb.cli.live_chat import LiveChat


class Speaker:
    def __init__(self): self.spoken = []
    def say(self, text): self.spoken.append(text)
    def stop(self): pass


class Stream:
    def __init__(self, events): self.events = events
    def stream(self, method, params): yield from self.events
    def close(self): pass


def test_plain_chunks_flush_before_final_and_final_is_not_printed_twice():
    out, speaker = io.StringIO(), Speaker()
    def events():
        yield {'type': 'reply_delta', 'text': 'Hello'}
        assert out.getvalue().endswith('Hello')
        assert not speaker.spoken
        yield {'type': 'reply_delta', 'text': ' there'}
        yield {'type': 'reply', 'text': 'Hello there!'}
        yield {'type': 'reply_delta', 'text': 'late'}
        yield {'type': 'reply', 'text': 'Hello there!'}
    Chat(lambda: Stream(events()), out, lambda _: '', speaker=speaker).turn('Hello')
    assert out.getvalue().count('Hello there!') == 1
    assert 'confirmed' in out.getvalue()
    assert 'late' not in out.getvalue()
    assert speaker.spoken == ['ferb> Hello there!']


def test_plain_revised_final_visibly_replaces_the_draft():
    out, speaker = io.StringIO(), Speaker()
    events = [{'type': 'reply_delta', 'text': 'Tentative answer'}, {'type': 'reply', 'text': 'Canonical answer'}]
    Chat(lambda: Stream(events), out, lambda _: '', speaker=speaker).turn('Question')
    assert 'replaces draft' in out.getvalue()
    assert out.getvalue().count('Canonical answer') == 1
    assert speaker.spoken == ['ferb> Canonical answer']


def test_plain_abort_and_eof_never_speak_or_confirm_partial():
    for suffix in ([], [{'type': 'reply_abort', 'reason': 'Connection lost'}]):
        out, speaker = io.StringIO(), Speaker()
        events = [{'type': 'reply_delta', 'text': 'Partial\x1b[2J\r'}] + suffix
        Chat(lambda: Stream(events), out, lambda _: '', speaker=speaker).turn('Question')
        assert 'interrupted' in out.getvalue().lower()
        assert '\x1b' not in out.getvalue() and '\r' not in out.getvalue()
        assert '[final response confirmed]' not in out.getvalue()
        assert not speaker.spoken


def test_live_draft_renders_without_stdout_newline_and_other_events_stay_separate():
    entered, release = threading.Event(), threading.Event()
    out, speaker = io.StringIO(), Speaker()
    def events():
        yield {'type': 'reply_delta', 'text': 'Visible now\x1b[2J'}
        entered.set()
        assert release.wait(2)
        yield {'type': 'reply', 'text': 'Visible now, finalized.'}
    chat = LiveChat(lambda: Stream(events()), out, lambda _: '', speaker=speaker)
    chat._thinking = 1
    invalidations = []
    chat.invalidate = lambda: invalidations.append(True)
    thread = threading.Thread(target=chat._stream_turn, args=('Question', 0))
    thread.start()
    assert entered.wait(2)
    try:
        draft = chat.toolbar()
        assert 'draft' in draft.lower() and 'Visible now' in draft
        assert '\x1b' not in draft
        assert 'Visible now' not in out.getvalue()
        assert not speaker.spoken
        chat.say('ferb> Another completed task.')
        assert out.getvalue() == 'ferb> Another completed task.\n'
        assert 'Visible now' in chat.toolbar()
        assert invalidations
    finally:
        release.set()
        thread.join(2)
    assert not thread.is_alive()
    assert 'Visible now' not in chat.toolbar()
    assert out.getvalue().count('Visible now, finalized.') == 1
    assert speaker.spoken == ['ferb> Another completed task.', 'ferb> Visible now, finalized.']


def test_live_abort_and_eof_clear_draft_and_are_not_spoken():
    for suffix in ([], [{'type': 'reply_abort', 'reason': 'Model interrupted'}]):
        out, speaker = io.StringIO(), Speaker()
        chat = LiveChat(lambda: Stream([{'type': 'reply_delta', 'text': 'Partial'}] + suffix), out,
                        lambda _: '', speaker=speaker)
        chat._thinking = 1
        chat._stream_turn('Question', 0)
        assert 'interrupted' in out.getvalue().lower()
        assert 'Partial' not in chat.toolbar()
        assert not speaker.spoken


def test_live_plan_discards_provisional_text_and_late_chunks():
    out = io.StringIO()
    events = [{'type': 'reply_delta', 'text': 'Tentative'},
              {'type': 'plan', 'task_id': 'task', 'text': 'Working.'},
              {'type': 'reply_delta', 'text': 'STALE-DRAFT'},
              {'type': 'outcome', 'task_id': 'task', 'status': 'done', 'answer': 'Done.'}]
    chat = LiveChat(lambda: Stream(events), out, lambda _: '')
    chat._thinking = 1
    chat._stream_turn('Question', 0)
    assert 'Tentative' not in chat.toolbar() and 'STALE-DRAFT' not in chat.toolbar()
    assert out.getvalue().count('Done.') == 1


def test_authoritative_intermediate_replies_and_later_task_outcome_are_preserved():
    events = [{'type': 'reply', 'text': 'Saved your notification preference.'},
              {'type': 'reply', 'text': 'Updated the conversation.'},
              {'type': 'plan', 'task_id': 'task', 'text': 'Checking your files now.'},
              {'type': 'outcome', 'task_id': 'task', 'status': 'done', 'answer': 'Found the report.'}]
    for live in (False, True):
        out = io.StringIO()
        chat = (LiveChat if live else Chat)(lambda: Stream(events), out, lambda _: '')
        if live:
            chat._thinking = 1
            chat._stream_turn('Question', 0)
        else:
            chat.turn('Question')
        assert out.getvalue().splitlines() == ['ferb> Saved your notification preference.',
            'ferb> Updated the conversation.', 'ferb> Checking your files now.', 'ferb> Found the report.']


def test_live_draft_pane_scrolls_to_latest_chunk_without_moving_input_focus():
    from prompt_toolkit.layout.controls import FormattedTextControl
    chat = LiveChat(lambda: None, io.StringIO(), lambda _: '')
    idle = FormattedTextControl(chat.toolbar_fragments).create_content(width=80, height=1)
    assert idle.cursor_position.x == 0  # The idle status must keep its running/question counts visible.
    chat._show_draft(0, 'first line\n' * 50 + 'latest chunk')
    control = FormattedTextControl(chat.toolbar_fragments)
    content = control.create_content(width=80, height=5)
    assert content.cursor_position.y == content.line_count - 1
    assert not control.is_focusable()
    assert 'latest chunk' in ''.join(piece[1] for piece in content.get_line(content.line_count - 1))


def test_small_terminal_renders_owner_input_and_newest_long_draft_together(monkeypatch):
    import asyncio

    async def render_fixture():
        from prompt_toolkit import PromptSession
        from prompt_toolkit.application.current import set_app
        from prompt_toolkit.data_structures import Size
        from prompt_toolkit.input.defaults import create_pipe_input
        from prompt_toolkit.output import DummyOutput
        from prompt_toolkit.document import Document
    
        class SmallTerminal(DummyOutput):
            def get_size(self): return Size(rows=6, columns=36)
    
        monkeypatch.setattr('ferb.cli.live_chat.shutil.get_terminal_size', lambda: SimpleSize(36, 6))
        chat = LiveChat(lambda: None, io.StringIO(), lambda _: '')
        chat._show_draft(0, 'Earlier provisional line\n' * 50 + 'NEWEST-DRAFT')
        with create_pipe_input() as inp:
            session = PromptSession('you> ', input=inp, output=SmallTerminal(), multiline=True,
                                    bottom_toolbar=chat.toolbar_fragments)
            with set_app(session.app):
                session.default_buffer.document = Document('STILL-TYPING', cursor_position=len('STILL-TYPING'))
                session.app.renderer.request_absolute_cursor_position()
                session.app.renderer.render(session.app, session.app.layout)
                screen = session.app.renderer._last_screen
                rendered = '\n'.join(''.join(screen.data_buffer[row][col].char for col in range(36)) for row in range(6))
                assert 'you> STILL-TYPING' in rendered, rendered
                assert 'NEWEST-DRAFT' in rendered, rendered
                assert session.app.layout.current_control.buffer is session.default_buffer
                assert session.app.layout.current_window.render_info.window_height >= 1
        assert len(chat._drafts[0]) > 1000  # The viewport scrolls; retained content is not truncated.

    asyncio.run(render_fixture())


class SimpleSize:
    def __init__(self, columns, lines): self.columns, self.lines = columns, lines
