"""Render chat streaming and send explicit question answers. Must not decide task plans or answer questions for the owner.

`ferb chat`: talk to FERB.

- A real line editor (prompt_toolkit): Enter sends, Option-Enter (or Esc then Enter) adds a line, arrows and
  history work. A paste arrives whole — several lines, one message — and is still marked as data, not instructions:
  FERB wraps each pasted piece in the paste markers the daemon treats as untrusted.
- Plain by default: FERB's own words, its questions, and their results. `--verbose` shows
  the plan, each step, tools and checks.
- Ctrl-C while FERB works stops that task ("stopped by you"). Closing chat (Ctrl-D, /quit, closing the terminal)
  never stops anything: tasks keep running, their answers wait for next time (and arrive as a macOS notification).
- A loader while FERB thinks or works (owner request, 2026-10-01): an animated line ("FERB is thinking…", then
  "FERB is working… (12s)" once a task starts), on the terminal only, cleared before anything is printed and while
  FERB asks a question.
- On opening: answers that arrived while chat was closed, tasks still running, and any question waiting for the
  owner."""

import json
import re
import sys
import threading
import time
from typing import Any, Callable, TextIO

from ferb.cli.client import Client
from ferb.core import origin as origin_
from ferb.sanitize.terminal import printable
from ferb.sanitize.untrusted import PASTE_END, PASTE_START
from ferb.voice.speak import Silent

PASTE_ON, PASTE_OFF = "\x1b[?2004h", "\x1b[?2004l"
FERB_CHECKS = {"ferb.confirm", "ferb.retry", "conversation.approve", "memory.forget"}   # FERB's own Yes/No (L3)

def read_message(inp: TextIO) -> str | None:
    """Without a terminal (piped input): one message per line; a bracketed paste may span several lines."""
    line = inp.readline()
    if not line:
        return None
    text = line
    while text.count(PASTE_START) > text.count(PASTE_END):
        more = inp.readline()
        if not more:
            break
        text += more
    return text.rstrip("\n")


PASTE_QUESTION = "   ? You pasted this as your whole message. Is it your own request? [y/N] "


GREETING = re.compile(r"^(hey|hi|hello|yo|ok|okay|so|ferb|hey ferb|hi ferb|please|pls)[\s,.!:;-]*$", re.I)


def whole_paste(text: str, pastes: list[str]) -> bool:
    """Nothing typed around it but perhaps a greeting ("hey " + the paste, owner's report 2026-10-01): the message is
    the paste."""
    rest = text
    for p in pastes:
        rest = rest.replace(p, "", 1)
    return bool(pastes) and (not rest.strip() or bool(GREETING.match(rest.strip())))


def pasted_is_data() -> bool:
    """`[policy] pasted_text_is_data` (the owner's switch): off means pastes are the owner's own words — no question."""
    try:
        from ferb.paths import ferb_home
        from ferb.config import Config
        return bool(Config(ferb_home()).get("policy", "pasted_text_is_data", True))
    except Exception:
        return True


def prepare(text: str, pastes: list[str], ask: Callable[[str], str]) -> str:
    """What goes to the daemon. Pastes are data, never instructions — unless the whole message is a paste and
    the owner says, here in their own terminal, that it's their request (e.g. copied from their own notes)."""
    if whole_paste(text, pastes) and ask(PASTE_QUESTION).strip().lower() in ("y", "yes"):
        return text
    return wrap_pastes(text, pastes)


def wrap_pastes(text: str, pastes: list[str]) -> str:
    """Mark each pasted piece (in order) as untrusted: the daemon sees exactly what was pasted, and where."""
    out, rest = [], text
    for p in pastes:
        i = rest.find(p)
        if i < 0 or not p.strip():
            continue
        out.append(rest[:i] + PASTE_START + p + PASTE_END)
        rest = rest[i + len(p):]
    return "".join(out) + rest


# V4: a task's three outcomes read differently. "done" is set only when every condition was verified; done_unverified
# reads "not confirmed" (V5: "unverified" is said only beside a condition a check ran on).
OUTCOME = {"done": "done ✓", "done_unverified": "done — not confirmed", "failed": "failed ✗"}


def render(ev: dict[str, Any], verbose: bool = False) -> str | None:
    t = ev.get("type")
    if t in ("reply", "clarify"):
        return f"ferb> {ev['text'] if verbose else ev.get('display_text') or ev['text']}"
    if t == "memory":
        return f"   ✎ {ev['text']}"
    if t == "timer":                      # the reminder's id is its own field (L11): shown only when verbose
        return f"   {ev['text']}" + (f"  [{ev['timer_id']}]" if verbose and ev.get("timer_id") else "")
    if t == "plan":
        if not verbose:
            return f"ferb> {ev['text']}"
        return f"ferb: {ev['text']}  [task {ev['task_id']}, {ev.get('identity', 'owner')}]: {ev.get('goal', '')}"
    if t == "subtask":
        if not verbose:
            return None
        if ev["status"] == "started":
            return f"   ▸ {ev['id']} started"
        return f"   {'✓' if ev['status'] == 'done' else '↻' if ev['status'] == 'retrying' else '✗'} {ev['id']} " \
               f"{ev['status']}: {ev.get('summary', '')}"
    if t == "verification":
        if not verbose:
            return None
        return "\n".join(f"   {'✓' if r['met'] else '?' if r['met'] is None else '✗'} {r['condition']}  "
                         f"[{r['path']}: {r['evidence']}]" for r in ev["results"])
    if t == "outcome":
        answer = ev.get("answer") or ev.get("summary", "")
        if not verbose:
            return f"ferb> {ev.get('display_text') or answer}"
        arts = f"\n      files: {', '.join(ev['artifacts'])}" if ev.get("artifacts") else ""
        return f"ferb: {OUTCOME.get(ev['status'], ev['status'])} — {ev['summary']}{arts}\nferb> {answer}"
    if t == "error":
        return f"ferb> {ev['text']}"
    if t == "event":
        d, topic = ev["data"], ev["topic"]
        if not verbose:
            return None
        if topic == "tool.called":
            return f"   · {d.get('tool')}"
    return None


def question(d: dict[str, Any], prompt: bool = True) -> str:
    """An approval in plain words, without terminal control characters (the question and options are model text).
    `prompt=False`: without the answer prompt line, for live chat, where the owner answers by replying (L3)."""
    text = printable(_question(d))
    if prompt or d.get("kind") in {"owner_login_code", "owner_admin_password", "owner_secret"}:
        return text
    return text.rstrip().rsplit("\n", 1)[0] if "\n" in text.rstrip() else text


def _question(d: dict[str, Any]) -> str:
    if d.get("kind") in {"owner_login_code", "owner_admin_password", "owner_secret"}:
        return f"   ? {d.get('reason', '')} "
    if d.get("kind") == "owner_question" and d.get("capability") in FERB_CHECKS and d.get("options"):
        # FERB's own check (D33; P19 retry, F9 conversation scope): skipping it is a No
        opts = "\n".join(f"     {i}) {o}" for i, o in enumerate(d["options"], 1))
        return f"   ? {d.get('reason', '')}\n{opts}\n   number or your answer (Enter = No): "
    if d.get("kind") == "owner_question":                 # a worker's question to the owner (owner.ask)
        if d.get("options"):
            opts = "\n".join(f"     {i}) {o}" for i, o in enumerate(d["options"], 1))
            return f"   ? {d.get('reason', '')}\n{opts}\n   number or your answer (Enter to skip): "
        return f"   ? {d.get('reason', '')}\n   your answer (Enter to skip): "
    return f"   ? {d.get('reason', '')}\n   your answer (Enter to skip): "


class Spinner:
    """The loader: one animated line on a terminal, redrawn in place; never in piped output or logs."""
    FRAMES = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"

    def __init__(self, out: TextIO | None, interval: float = 0.1) -> None:
        self.out, self.interval = out, interval
        self.enabled = out is not None and hasattr(out, "isatty") and out.isatty()
        self.label, self.since = "", time.monotonic()
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None

    def begin(self) -> None:
        """A new request: the elapsed time counts from here."""
        self.since = time.monotonic()

    def start(self, label: str) -> None:
        self.label = label
        if not self.enabled or self._thread is not None:
            return
        self._stop.clear()
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _run(self) -> None:
        assert self.out is not None
        i = 0
        while not self._stop.is_set():
            secs = int(time.monotonic() - self.since)
            elapsed = f" ({secs}s)" if secs >= 3 else ""
            self.out.write(f"\r\x1b[2K{self.FRAMES[i % len(self.FRAMES)]} FERB is {self.label}…{elapsed}")
            self.out.flush()
            i += 1
            self._stop.wait(self.interval)

    def stop(self) -> None:
        if self._thread is None:
            return
        self._stop.set()
        self._thread.join()
        self._thread = None
        assert self.out is not None
        self.out.write("\r\x1b[2K")                       # clear the loader line before anything is printed
        self.out.flush()


FINAL = ("outcome", "reply", "clarify", "error")


class ReplyDraft:
    """Per-turn provisional response state; final text always remains authoritative."""
    def __init__(self) -> None:
        self.text = ""
        self.accepting = True
        self.final = False

    def append(self, text: str) -> str:
        if not self.accepting or self.final:
            return ""
        safe = printable(text)
        self.text += safe
        return safe

    def discard(self) -> None:
        self.text = ""
        self.accepting = False


class PlainReply:
    """Piped output cannot erase a draft; explicitly confirm it or show the canonical correction."""
    def __init__(self, write: Callable[[str], None], speak: Callable[[str], None], verbose: bool = False) -> None:
        self.draft = ReplyDraft()
        self.write, self.speak = write, speak
        self.verbose = verbose
        self.line_open = False
        self.started = False
        self.confirmed_reply: tuple[str, str] | None = None

    def consume(self, event: dict[str, Any]) -> bool:
        kind = event.get("type")
        if kind == "reply_delta":
            chunk = self.draft.append(str(event.get("text") or ""))
            if chunk:
                if not self.line_open:
                    self.write("ferb (draft)> " if not self.started else "ferb (draft, continued)> ")
                self.write(chunk)
                self.line_open = self.started = True
            return True
        if kind == "reply_abort":
            if not self.draft.final and self.draft.accepting:
                self.interrupt(str(event.get("reason") or "The response was not confirmed."))
            return True
        if kind in FINAL:
            canonical = (str(kind), printable(str((None if self.verbose else event.get("display_text")) or event.get("text") or "")))
            if kind in ("reply", "clarify") and canonical == self.confirmed_reply:
                return True
            self.confirmed_reply = None
            self.draft.final = True
            self.draft.accepting = False
            if kind in ("reply", "clarify") and self.draft.text:
                text = canonical[1]
                self.confirmed_reply = canonical
                if text.startswith(self.draft.text):
                    self.write(text[len(self.draft.text):] + "\n   [final response confirmed]\n")
                else:
                    self.break_line()
                    self.write("ferb> Final response (replaces draft): " + text + "\n")
                self.speak("ferb> " + text)
                self.line_open = False
                self.draft.discard()
                return True
            self.break_line()
            self.draft.discard()
        elif kind == "plan":
            self.confirmed_reply = None
            if self.draft.text:
                self.break_line()
                self.write("   Provisional reply discarded; starting the task.\n")
            self.draft.discard()
        return False

    def break_line(self) -> None:
        if self.line_open:
            self.write("\n")
            self.line_open = False

    def interrupt(self, reason: str) -> None:
        self.break_line()
        self.write("   Draft interrupted: " + printable(reason) + "\n")
        self.draft.discard()

    def close(self) -> None:
        if self.draft.text and not self.draft.final:
            self.interrupt("The stream ended before FERB confirmed the response.")


NOTICED = "ferb (noticed)>"                  # a perception notice: FERB's words about what it noticed (L10)
# Where a quote came from, in plain words for the owner (presentation only; an origin label never gates anything).
OUTSIDE = {"observation": "what FERB saw on screen", "web": "a web page", "email": "an email", "message": "a message",
           "conversation": "a conversation", "paste": "pasted text", "download": "a download",
           "memory": "FERB's memory", "procedure": "a saved procedure"}
OWN = {"owner": "your own words", "private": "your own data", "inferred": "FERB's inference", "ferb": "FERB"}


def origin_phrase(label: str) -> str:
    """An origin label (MP2) as the owner reads it: `external:web:claude.ai` → "a web page (claude.ai)"; an unknown
    outside label is "an outside source"."""
    least = origin_.least_trusted(origin_.split(label))
    if not origin_.is_external(least):
        return OWN.get(least, "an outside source")
    kind, _, detail = least.removeprefix(origin_.EXTERNAL).lstrip(":").partition(":")
    phrase = OUTSIDE.get(kind, "an outside source")
    return phrase + (f" ({detail})" if detail and phrase != "an outside source" else "")


class Chat:
    def __init__(self, connect: Callable[[], Client], out: TextIO, ask: Callable[[str], str],
                 verbose: bool = False, spinner_out: TextIO | None = None, speaker: Any = None,
                 new: bool = False, resume: str | None = None) -> None:
        self.connect, self.out, self.ask, self.verbose = connect, out, ask, verbose
        self.spinner = Spinner(spinner_out)
        self.speaker = speaker if speaker is not None else Silent()        # `--speak`: FERB's own words, out loud
        self.session_id = resume or "owner"
        self.new, self.resume = new, resume
        self._output_lock = threading.RLock()

    def write(self, text: str) -> None:
        with self._output_lock:
            self.out.write(printable(text))
            self.out.flush()

    def say(self, line: str) -> None:
        line = printable(line)                            # model text never drives the owner's terminal
        self.write(line + "\n")
        if line.lstrip().startswith(("ferb>", NOTICED, "📞", "✉️")):       # FERB's words, never internals
            self.speaker.say(line)

    def notice(self, notice: dict[str, Any]) -> None:
        """A perception notice (L9/L10): FERB's words with a "noticed" tag, then each outside quote as quoted text with
        its origin, never as FERB's own words (MP2). A quote stays one quoted string: its own line breaks can't start
        a line that looks like FERB's."""
        self.say(NOTICED + " " + str(notice.get("text", "")))
        for quote in notice.get("quotes") or []:
            text = json.dumps(str(quote.get("text", "")), ensure_ascii=False)
            self.say(f'   quoted from {origin_phrase(str(quote.get("origin", "")))}: {text}')

    def call(self, method: str, params: dict[str, Any] | None = None) -> Any:
        c = self.connect()
        try:
            return c.call(method, params or {})
        finally:
            c.close()

    def answer_approval(self, d: dict[str, Any]) -> None:
        self.speaker.say(question(d))
        if d.get("kind") in {"owner_login_code", "owner_admin_password", "owner_secret"}:
            import getpass
            # Never let getpass fall back to echoing a code through a pipe or chat history.
            import warnings
            with warnings.catch_warnings():
                warnings.simplefilter("error", getpass.GetPassWarning)
                try:
                    reply = getpass.getpass(question(d)) if sys.stdin.isatty() else ""
                    if d.get("kind") not in {"owner_admin_password", "owner_secret"}:
                        reply = reply.strip()
                except (getpass.GetPassWarning, EOFError):
                    reply = ""
        else:
            reply = self.ask(question(d)).strip()
        self.speaker.stop()
        r = self.call("chat.answer", {"approval_id": d["approval_id"], "text": reply})
        if self.verbose:
            self.say(f"   {r['state']}")

    def opening(self) -> None:
        """What happened while chat was closed, what's still running, and questions waiting."""
        try:
            attached = self.call("chat.attach", {"new": self.new, "resume": self.resume})
            self.session_id = attached.get("session_id", self.session_id)
            if self.new or self.resume:
                print(f"Conversation: {self.session_id}", file=self.out, flush=True)
            st = self.call("chat.status")
        except Exception:
            if self.new or self.resume:
                raise
            return
        for r in st.get("results", []):
            self.say(f"ferb> (while you were away) {r['answer'] if self.verbose else r.get('display_text') or r['answer']}")
        for r in st.get("running", []):
            self.say(f"ferb> Still working on: {r['goal']}")
        digest = st.get('memory_digest')
        if digest:
            self.say('ferb> Here are things I learned that need your confirmation:')
            for proposal in digest.get('proposals', []):
                self.say(f"   {proposal['text']} — /memory confirm {proposal['id']}")
        for notice in st.get('notices', []):
            self.notice(notice)
        if st.get('notices'):
            self.call('attention.ack', {'ids': [notice['id'] for notice in st['notices']]})
        for d in st.get("approvals", []):
            self.answer_approval(d)
        for q in st.get("questions", []):                    # asked while you were away (a scheduled run)
            reply = self.ask(printable(f"   ? {q['question']} [y/N] "))
            self.call("questions.answer", {"id": q["id"], "yes": reply.strip().lower() in ("y", "yes")})
        reminders(self.connect, self.out)

    def turn(self, msg: str) -> None:
        client = self.connect()
        tasks: list[str] = []
        self.speaker.stop()                                   # a new message cuts off what FERB was saying
        spin = self.spinner
        reply = PlainReply(self.write, self.speaker.say, self.verbose)
        spin.begin()
        spin.start("thinking")
        try:
            for ev in client.stream("chat.send", {"session_id": self.session_id, "text": msg}):
                spin.stop()                                       # never draw over a line or a question
                if reply.consume(ev):
                    continue
                reply.break_line()
                if ev.get("type") == "plan" and ev.get("task_id"):
                    tasks.append(ev["task_id"])
                if ev.get("type") == "event" and ev["topic"] == "approval.requested":
                    self.answer_approval(ev["data"])
                else:
                    line = render(ev, self.verbose)
                    if line:
                        self.say(line)
                if ev.get("type") not in FINAL and not reply.draft.text:
                    spin.start("working" if tasks else "thinking")
        except KeyboardInterrupt:                             # Ctrl-C: stop what FERB is doing for this message
            spin.stop()
            if tasks:
                self.call("task.stop", {"task_ids": tasks})
                self.say("ferb> Stopped.")
        finally:
            spin.stop()
            reply.close()
            client.close()


def run(connect: Callable[[], Client], inp: TextIO, out: TextIO, ask: Callable[[str], str],
        verbose: bool = False, speaker: Any = None, new: bool = False, resume: str | None = None) -> int:
    """Piped / non-terminal input (and tests): line by line."""
    chat = Chat(connect, out, ask, verbose, speaker=speaker, new=new, resume=resume)
    chat.say("FERB — /quit to leave. Pasted text is treated as data, not instructions.")
    chat.opening()
    while True:
        print("you> ", end="", file=out, flush=True)
        msg = read_message(inp)
        if msg is None or msg.strip() in ("/quit", "/exit"):
            return 0
        if msg.strip():
            chat.turn(msg)


def run_tty(connect: Callable[[], Client], verbose: bool = False, speaker: Any = None, new: bool = False, resume: str | None = None) -> int:
    """A terminal: the line editor, with pastes kept whole and marked untrusted."""
    from prompt_toolkit import PromptSession
    from prompt_toolkit.history import InMemoryHistory
    from prompt_toolkit.key_binding import KeyBindings
    from prompt_toolkit.keys import Keys
    from prompt_toolkit.patch_stdout import patch_stdout

    pastes: list[str] = []
    interrupts = [0]
    kb = KeyBindings()

    @kb.add(Keys.BracketedPaste)
    def _(event: Any) -> None:
        data = event.data.replace("\r\n", "\n").replace("\r", "\n")
        pastes.append(data)
        event.current_buffer.insert_text(data)

    @kb.add("enter")
    def _(event: Any) -> None:
        event.current_buffer.validate_and_handle()

    @kb.add("escape", "enter")
    def _(event: Any) -> None:
        event.current_buffer.insert_text("\n")

    @kb.add("c-c")
    def _(event: Any) -> None:
        if event.current_buffer.text:
            event.current_buffer.reset()
            pastes.clear()
            interrupts[0] = 0
        elif interrupts[0] == 0:
            interrupts[0] = 1
            chat.say('Press Ctrl-C again to close chat. Tasks keep running.')
        else:
            event.app.exit(exception=EOFError)

    @kb.add('escape')
    def _(event: Any) -> None:
        if not event.current_buffer.text:
            threading.Thread(target=chat.stop_latest, daemon=True).start()

    session: PromptSession[str] = PromptSession(history=InMemoryHistory(), key_bindings=kb, multiline=True)

    def ask(q: str) -> str:
        return session.prompt(q, multiline=False, key_bindings=None)

    from ferb.cli.live_chat import LiveChat
    with patch_stdout():
        # Bind after patching: background final lines must go above the live editor.
        chat = LiveChat(connect, sys.stdout, ask, verbose, speaker=speaker, new=new, resume=resume)
        chat.invalidate = session.app.invalidate
        chat.say("FERB — type any time. /tasks lists work; Esc stops the latest task. /quit leaves tasks running.")
        chat.opening()
        threading.Thread(target=_live_reminders, args=(connect, sys.stdout), daemon=True).start()
        while True:
            pastes.clear()
            try:
                text = session.prompt("you> ", bottom_toolbar=chat.toolbar_fragments, refresh_interval=0.1)
            except (EOFError, KeyboardInterrupt):
                return 0
            if text.strip() in ("/quit", "/exit"):
                return 0
            interrupts[0] = 0
            if text.strip():
                chat.submit(prepare(text, list(pastes), ask) if pasted_is_data() else text)
            else:
                chat.enter_secret()


def reminders(connect: Callable[[], Client], out: TextIO) -> None:
    """Delivered reminders not shown in chat yet (the daemon marks them shown)."""
    try:
        c = connect()
        try:
            for r in c.call("reminders.unseen")["reminders"]:
                late = f" (due {round(r['overdue_s'] / 60)} min ago)" if (r.get("overdue_s") or 0) >= 60 else ""
                out.write(printable(f"\n   ⏰ reminder: {r['message']}{late}\n"))
                out.flush()
        finally:
            c.close()
    except Exception:
        pass                                                  # never break the chat over a reminder


def _live_reminders(connect: Callable[[], Client], out: TextIO) -> None:
    try:
        c = connect()
        for ev in c.stream("logs.subscribe", {"filters": ["timers"]}):
            if ev.get("topic") == "timer.fired":
                reminders(connect, out)
    except Exception:
        return


def main_chat(connect: Callable[[], Client], verbose: bool = False, speak: bool = False,
              new: bool = False, resume: str | None = None) -> int:
    from ferb.voice.speak import SaySpeaker
    speaker = SaySpeaker() if speak else None
    if sys.stdin.isatty() and sys.stdout.isatty():
        return run_tty(connect, verbose, speaker, new, resume)
    return run(connect, sys.stdin, sys.stdout, input, verbose, speaker, new, resume)
