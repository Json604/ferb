"""`ferb chat`: talk to FERB (section 15, build step 14a).

- A real line editor (prompt_toolkit): Enter sends, Option-Enter (or Esc then Enter) adds a line, arrows and
  history work. A paste arrives whole — several lines, one message — and is still marked as data, not instructions:
  FERB wraps each pasted piece in the paste markers the daemon treats as untrusted (section 10.2).
- Plain by default: FERB's own words, its questions, and a line when something is held for undo. `--verbose` shows
  the plan, each step, tools and checks.
- Ctrl-C while FERB works stops that task ("stopped by you"). Closing chat (Ctrl-D, /quit, closing the terminal)
  never stops anything: tasks keep running, their answers wait for next time (and arrive as a macOS notification).
- On opening: answers that arrived while chat was closed, tasks still running, and any question waiting for the
  owner.
"""

import secrets
import sys
import threading
from typing import Any, Callable, TextIO

from ferb.cli.client import Client
from ferb.sanitize.untrusted import PASTE_END, PASTE_START

PASTE_ON, PASTE_OFF = "\x1b[?2004h", "\x1b[?2004l"

KIND_TEXT = {"call": "start a call", "send": "send a message", "post": "post", "reply": "reply", "mailbox_send": "send an email from FERB",
             "pay": "make a payment", "delete_permanent": "permanently delete something",
             "account_security": "change account or security settings", "run_shell": "run a command",
             "applescript": "control an app with AppleScript", "mac_input": "use the mouse and keyboard in an app",
             "fill_form": "fill in a form", "submit": "submit something", "write_file": "change a file",
             "fill_secret": "fill in a password", "mailbox_read": "read FERB's inbox"}


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


def render(ev: dict[str, Any], verbose: bool = False) -> str | None:
    t = ev.get("type")
    if t in ("reply", "clarify"):
        return f"ferb> {ev['text']}"
    if t == "memory":
        return f"   ✎ {ev['text']}"
    if t == "timer":
        return f"   {ev['text']}"
    if t == "plan":
        if not verbose:
            return f"ferb> {ev['text']}"
        subs = "".join(f"\n        {s['id']} ({s['role']}{', after ' + ', '.join(s['depends_on']) if s['depends_on'] else ''})"
                       f": {s['goal']}" for s in ev.get("subtasks", []))
        return f"ferb: {ev['text']}  [task {ev['task_id']}]{subs}"
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
            return f"ferb> {answer}"
        arts = f"\n      files: {', '.join(ev['artifacts'])}" if ev.get("artifacts") else ""
        check = "verified" if ev.get("verified") else "NOT verified"
        return f"ferb: {ev['status']} ({check}) — {ev['summary']}{arts}\nferb> {answer}"
    if t == "error":
        return f"ferb> {ev['text']}"
    if t == "event":
        d, topic = ev["data"], ev["topic"]
        if topic == "action.held":
            secs = f"{d.get('seconds', 10):.0f} s"
            to = d.get("to")
            if d.get("kind") == "call":
                return f"   📞 Calling {to} in {secs}… (Ctrl-C to cancel)" if to else \
                    f"   📞 calling in {secs}… (Ctrl-C to cancel)"
            if d.get("kind") in ("send", "mailbox_send", "reply", "post"):
                return f"   ✉️ Sending to {to} in {secs}… (Ctrl-C to cancel)" if to else \
                    f"   ✉️ sending in {secs}… (Ctrl-C to cancel)"
            return f"   ⏳ holding this for {secs} so you can stop it (Ctrl-C)"
        if topic == "action.undone":
            return "   ↩ cancelled"
        if not verbose:
            return None
        if topic == "tool.called":
            return f"   · {d.get('tool')}"
        if topic == "policy.blocked":
            return f"   ✗ blocked ({d.get('step')}): {d.get('reason')}"
        if topic == "worker.exposed":
            return f"   ! worker exposed to untrusted content: {d.get('reason')}"
        if topic == "action.released":
            return "   → released"
    return None


def question(d: dict[str, Any]) -> str:
    """An approval in plain words."""
    if d.get("options"):                                  # "Which Rahul did you mean?" (16a)
        opts = "\n".join(f"     {i}) {o}" for i, o in enumerate(d["options"], 1))
        return f"   ? {d.get('reason', '')}\n{opts}\n   number (Enter to cancel): "
    if d.get("kind") == "route_change":                   # FERB found a better route while exploring (10.3)
        return f"   ? {d.get('reason', '')} [y/N] "
    if d.get("kind") == "folder_access":
        return f"   ? {d.get('reason', '')} [y/N] "
    what = KIND_TEXT.get(str(d.get("kind")), str(d.get("kind") or "do this"))
    return f"   ? OK for FERB to {what}? {d.get('reason', '')} [y/N] "


class Chat:
    def __init__(self, connect: Callable[[], Client], out: TextIO, ask: Callable[[str], str],
                 verbose: bool = False) -> None:
        self.connect, self.out, self.ask, self.verbose = connect, out, ask, verbose
        self.session_id = secrets.token_hex(6)

    def say(self, line: str) -> None:
        print(line, file=self.out, flush=True)

    def call(self, method: str, params: dict[str, Any] | None = None) -> Any:
        c = self.connect()
        try:
            return c.call(method, params or {})
        finally:
            c.close()

    def answer_approval(self, d: dict[str, Any]) -> None:
        reply = self.ask(question(d)).strip()
        if d.get("options"):
            n = int(reply) if reply.isdigit() and 1 <= int(reply) <= len(d["options"]) else None
            params: dict[str, Any] = {"approval_id": d["approval_id"], "granted": n is not None, "choice": n}
        else:
            params = {"approval_id": d["approval_id"], "granted": reply.lower() in ("y", "yes")}
        r = self.call("chat.approve", params)
        if self.verbose:
            self.say(f"   {r['state']}")

    def opening(self) -> None:
        """What happened while chat was closed, what's still running, and questions waiting."""
        try:
            self.call("chat.attach")
            st = self.call("chat.status")
        except Exception:
            return
        for r in st.get("results", []):
            self.say(f"ferb> (while you were away) {r['answer']}")
        for r in st.get("running", []):
            self.say(f"ferb> Still working on: {r['goal']}")
        for d in st.get("approvals", []):
            self.answer_approval(d)
        for q in st.get("questions", []):                    # asked while you were away (a scheduled run)
            reply = self.ask(f"   ? {q['question']} [y/N] ")
            self.call("questions.answer", {"id": q["id"], "yes": reply.strip().lower() in ("y", "yes")})
        reminders(self.connect, self.out)

    def turn(self, msg: str) -> None:
        client = self.connect()
        tasks: list[str] = []
        try:
            for ev in client.stream("chat.send", {"session_id": self.session_id, "text": msg}):
                if ev.get("type") == "plan" and ev.get("task_id"):
                    tasks.append(ev["task_id"])
                if ev.get("type") == "event" and ev["topic"] == "approval.requested":
                    self.answer_approval(ev["data"])
                    continue
                line = render(ev, self.verbose)
                if line:
                    self.say(line)
        except KeyboardInterrupt:                             # Ctrl-C: stop what FERB is doing for this message
            if tasks:
                self.call("task.stop", {"task_ids": tasks})
                self.say("ferb> Stopped.")
        finally:
            client.close()


def run(connect: Callable[[], Client], inp: TextIO, out: TextIO, ask: Callable[[str], str],
        verbose: bool = False) -> int:
    """Piped / non-terminal input (and tests): line by line."""
    chat = Chat(connect, out, ask, verbose)
    chat.say("FERB — /quit to leave. Pasted text is treated as data, not instructions.")
    chat.opening()
    while True:
        print("you> ", end="", file=out, flush=True)
        msg = read_message(inp)
        if msg is None or msg.strip() in ("/quit", "/exit"):
            return 0
        if msg.strip():
            chat.turn(msg)


def run_tty(connect: Callable[[], Client], verbose: bool = False) -> int:
    """A terminal: the line editor, with pastes kept whole and marked untrusted."""
    from prompt_toolkit import PromptSession
    from prompt_toolkit.history import InMemoryHistory
    from prompt_toolkit.key_binding import KeyBindings
    from prompt_toolkit.keys import Keys
    from prompt_toolkit.patch_stdout import patch_stdout

    pastes: list[str] = []
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
        else:
            event.app.exit(exception=EOFError)

    session: PromptSession[str] = PromptSession(history=InMemoryHistory(), key_bindings=kb, multiline=True)

    def ask(q: str) -> str:
        return session.prompt(q, multiline=False, key_bindings=None)

    chat = Chat(connect, sys.stdout, ask, verbose)
    with patch_stdout():
        chat.say("FERB — Ctrl-D or /quit to leave (tasks keep running). Option-Enter for a new line.")
        chat.opening()
        threading.Thread(target=_live_reminders, args=(connect, sys.stdout), daemon=True).start()
        while True:
            pastes.clear()
            try:
                text = session.prompt("you> ")
            except (EOFError, KeyboardInterrupt):
                return 0
            if text.strip() in ("/quit", "/exit"):
                return 0
            if text.strip():
                chat.turn(wrap_pastes(text, list(pastes)))


def reminders(connect: Callable[[], Client], out: TextIO) -> None:
    """Delivered reminders not shown in chat yet (the daemon marks them shown)."""
    try:
        c = connect()
        try:
            for r in c.call("reminders.unseen")["reminders"]:
                late = f" (due {round(r['overdue_s'] / 60)} min ago)" if (r.get("overdue_s") or 0) >= 60 else ""
                print(f"\n   ⏰ reminder: {r['message']}{late}", file=out, flush=True)
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


def main_chat(connect: Callable[[], Client], verbose: bool = False) -> int:
    if sys.stdin.isatty() and sys.stdout.isatty():
        return run_tty(connect, verbose)
    return run(connect, sys.stdin, sys.stdout, input, verbose)
