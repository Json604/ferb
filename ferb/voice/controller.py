"""Push-to-talk in the daemon (section 20.1): the talk key's decisions become commands for FERB.app; what the owner said
comes back as a transcript and runs as a chat turn in the "voice" session; FERB's replies are spoken.

FERB.app is the only part that opens a microphone; this side decides which one (never a Bluetooth headset's), when to
start, send or discard, and what to do with the words. A spoken "yes" / "no" / number answers the oldest waiting
question; "stop" / "cancel" during a send or call countdown cancels it (what Ctrl-C does in chat)."""

import asyncio
import re
import time
from typing import Any, AsyncIterator, Callable

from ferb.voice.mic import InputDevice, choose
from ferb.voice.push_to_talk import TalkKey
from ferb.voice.speak import spoken

YES = re.compile(r"^\s*(yes|yeah|yep|yup|sure|ok|okay|go ahead|do it|approve[d]?|allow|confirm)\b", re.I)
NO = re.compile(r"^\s*(no|nope|don'?t|cancel|stop|deny|decline)\b", re.I)
CANCEL = re.compile(r"^\s*(stop|cancel|don'?t|no|wait|hold on)\b", re.I)
NUMBERS = {"one": 1, "two": 2, "three": 3, "four": 4, "five": 5, "six": 6, "seven": 7, "eight": 8, "nine": 9,
           "first": 1, "second": 2, "third": 3}


def spoken_choice(text: str) -> int | None:
    """The first number said: "2", "two", "the second one" (not "one")."""
    words = "|".join(NUMBERS)
    m = re.search(rf"\b(\d|{words})\b", text, re.I)
    if m is None:
        return None
    w = m.group(1).lower()
    return int(w) if w.isdigit() else NUMBERS[w]


class Voice:
    def __init__(self, run_turn: Callable[[str], AsyncIterator[dict[str, Any]]], pending: Callable[[], list[Any]],
                 answer: Callable[[str, bool, int | None], Any], speaker: Any, journal: Any,
                 launch_app: Callable[[], None], mode: Callable[[], str] = lambda: "auto",
                 render: Callable[[dict[str, Any]], str | None] = lambda ev: None,
                 question: Callable[[dict[str, Any]], str] = lambda d: "",
                 cancel_countdown: Callable[[], bool] = lambda: False) -> None:
        self.run_turn, self.pending, self.answer = run_turn, pending, answer
        self.speaker, self.journal = speaker, journal
        self.launch_app, self.mode, self.render, self.question = launch_app, mode, render, question
        self.cancel_countdown = cancel_countdown
        self.key = TalkKey()
        self.devices: list[InputDevice] = []
        self.permissions: dict[str, Any] = {}
        self.app: asyncio.Queue[dict[str, Any]] | None = None     # commands to FERB.app while it's connected
        self.listening = False

    # ---------- FERB.app ----------

    def attach(self, devices: list[dict[str, Any]], permissions: dict[str, Any]) -> asyncio.Queue[dict[str, Any]]:
        self.devices = [InputDevice(str(d.get("uid", "")), str(d.get("name", "")), str(d.get("transport", "")),
                                    bool(d.get("default"))) for d in devices]
        self.permissions = dict(permissions)
        self.app = asyncio.Queue()
        self.journal.append("voice.attached", {"devices": len(self.devices), "permissions": self.permissions})
        return self.app

    def detach(self, q: asyncio.Queue[dict[str, Any]]) -> None:
        if self.app is q:
            self.app = None
            self.listening = False

    # ---------- the talk key ----------

    def on_key(self, keycode: int, down: bool, t: float | None = None) -> str | None:
        act = self.key.key(keycode, down, time.monotonic() if t is None else t)
        if act == "start":
            self.speaker.stop()                                  # the owner talks: FERB stops speaking
            if self.app is None:
                self.launch_app()
                self.journal.append("voice.unavailable", {"why": "FERB.app isn't connected"})
                self.speaker.say("Voice isn't ready yet. Try again in a moment.")
                return act
            device = choose(self.devices, self.mode())
            if device is None:
                self.speaker.say("I can't find a microphone I'm allowed to use.")
                return act
            self.listening = True
            self.app.put_nowait({"op": "start", "device_uid": device.uid, "device": device.name})
        elif act in ("send", "ignore", "discard") and self.listening and self.app is not None:
            self.listening = False
            self.app.put_nowait({"op": "stop" if act == "send" else "discard"})
        return act

    # ---------- what the owner said ----------

    async def transcript(self, text: str) -> dict[str, Any]:
        text = " ".join(text.split())
        if not text:
            self.speaker.say("I didn't catch that.")
            return {"handled": "empty"}
        if CANCEL.search(text) and self.cancel_countdown():    # a send / call still counting down
            self.speaker.say("Cancelled.")
            return {"handled": "cancelled"}
        pending = self.pending()
        if pending:                                              # a spoken answer to the oldest waiting question
            ap = pending[0]
            if ap.options:
                n = spoken_choice(text)
                if n is not None and 1 <= n <= len(ap.options):
                    self.answer(ap.id, True, n)
                    self.speaker.say(f"OK, {ap.options[n - 1]}.")
                    return {"handled": "answer"}
            elif YES.search(text) or NO.search(text):
                granted = bool(YES.search(text)) and not NO.search(text)
                self.answer(ap.id, granted, None)
                self.speaker.say("OK." if granted else "OK, I won't.")
                return {"handled": "answer"}
        asyncio.get_running_loop().create_task(self._turn(text))
        return {"handled": "request"}

    async def _turn(self, text: str) -> None:
        """One voice request: a chat turn with the owner's own words; FERB's words spoken, in order."""
        try:
            async for ev in self.run_turn(text):
                line: str | None = None
                if ev.get("type") == "event" and ev.get("topic") == "approval.requested":
                    line = self.question(ev.get("data", {}))
                else:
                    line = self.render(ev)
                if line and (line.lstrip().startswith(("ferb>", "📞", "✉️", "?"))):
                    words = spoken(line.replace("(Ctrl-C to cancel)", ". Say stop to cancel.")
                                   .replace("number (Enter to cancel):", ". Say the number."))
                    await asyncio.to_thread(self.speaker.wait)       # don't cut off what it's still saying
                    self.speaker.say(words)
        except Exception as e:                                    # the owner hears it, the journal keeps it
            self.journal.append("voice.failed", {"why": f"{type(e).__name__}: {e}"[:300]})
            self.speaker.say("Sorry — something went wrong on my side.")
