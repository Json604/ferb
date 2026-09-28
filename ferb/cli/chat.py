"""`ferb chat`: talk to the supervisor (section 15). Approvals are answered inline; a held send can be cancelled
with `ferb undo` from another terminal.

Bracketed paste is switched on so pasted text arrives wrapped in paste markers — the daemon treats it as
untrusted data (section 10.2).
"""

import secrets
import sys
from typing import Any, Callable, TextIO

from ferb.cli.client import Client
from ferb.sanitize.untrusted import PASTE_END, PASTE_START

PASTE_ON, PASTE_OFF = "\x1b[?2004h", "\x1b[?2004l"


def read_message(inp: TextIO) -> str | None:
    """One message; a bracketed paste may span several lines."""
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


def render(ev: dict[str, Any]) -> str | None:
    t = ev.get("type")
    if t == "reply":
        return f"ferb> {ev['text']}"
    if t == "clarify":
        return f"ferb? {ev['text']}"
    if t == "plan":
        conds = "".join(f"\n        - {c}" for c in ev.get("success_conditions", []))
        return f"ferb: {ev['text']}  [task {ev['task_id']}]{conds}"
    if t == "outcome":
        arts = f"\n      files: {', '.join(ev['artifacts'])}" if ev.get("artifacts") else ""
        return (f"ferb: {ev['status']} — {ev['summary']}{arts}\n"
                f"      (the worker's own report; independent verification arrives with build step 8)")
    if t == "error":
        return f"ferb! {ev['text']}"
    if t == "event":
        d, topic = ev["data"], ev["topic"]
        if topic == "tool.called":
            return f"   · {d.get('tool')}"
        if topic == "policy.blocked":
            return f"   ✗ blocked ({d.get('step')}): {d.get('reason')}"
        if topic == "worker.exposed":
            return f"   ! worker exposed to untrusted content: {d.get('reason')}"
        if topic == "action.held":
            return (f"   ⏳ {d.get('kind')} held {d.get('seconds', 10):.0f}s — `ferb undo {d.get('hold_id')}` in another "
                    "terminal cancels it")
        if topic == "action.released":
            return "   → released"
        if topic == "action.undone":
            return "   ↩ cancelled"
    return None


def run(connect: Callable[[], Client], inp: TextIO, out: TextIO, ask: Callable[[str], str]) -> int:
    session_id = secrets.token_hex(6)
    tty = hasattr(out, "isatty") and out.isatty()
    if tty:
        out.write(PASTE_ON)
    print("FERB chat — /quit to leave. Pasted text is treated as data, not instructions.", file=out)
    try:
        while True:
            print("you> ", end="", file=out, flush=True)
            msg = read_message(inp)
            if msg is None or msg.strip() in ("/quit", "/exit"):
                return 0
            if not msg.strip():
                continue
            client = connect()
            try:
                for ev in client.stream("chat.send", {"session_id": session_id, "text": msg}):
                    if ev.get("type") == "event" and ev["topic"] == "approval.requested":
                        d = ev["data"]
                        answer = ask(f"   ? approve {d.get('kind')} via {d.get('capability')} — {d.get('reason')} [y/N] ")
                        c2 = connect()
                        try:
                            r = c2.call("chat.approve", {"approval_id": d["approval_id"],
                                                         "granted": answer.strip().lower() == "y"})
                            print(f"   {r['state']}", file=out)
                        finally:
                            c2.close()
                        continue
                    line = render(ev)
                    if line:
                        print(line, file=out, flush=True)
            finally:
                client.close()
    finally:
        if tty:
            out.write(PASTE_OFF)
            out.flush()


def main_chat(connect: Callable[[], Client]) -> int:
    return run(connect, sys.stdin, sys.stdout, input)
