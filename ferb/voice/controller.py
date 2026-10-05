"""Coordinate push-to-talk transcription and explicit question routing. Must not select among pending questions by order.

Push-to-talk in the daemon: the talk key's decisions become commands for FERB.app; what the owner said
comes back as a transcript and runs as a chat turn in the "voice" session; FERB's replies are spoken.

FERB.app is the only part that opens a microphone; this side decides which one (never a Bluetooth headset's), when to
start, send or discard, and what to do with the words. Responses to questions are forwarded in full so the agent
can interpret choices and corrections. Login codes use chat's hidden input, never the voice event stream."""

import asyncio
import time
from typing import Any, AsyncIterator, Callable

from ferb.voice.mic import InputDevice, choose
from ferb.voice.push_to_talk import MIN_HOLD_S, TalkKey
from ferb.voice.speak import spoken

class Voice:
    def __init__(self, run_turn: Callable[[str], AsyncIterator[dict[str, Any]]], pending: Callable[[], list[Any]],
                 answer: Callable[[str, bool, int | None, str | None], Any], speaker: Any, journal: Any,
                 launch_app: Callable[[], None], mode: Callable[[], str] = lambda: "auto",
                 render: Callable[[dict[str, Any]], str | None] = lambda ev: None,
                 question: Callable[[dict[str, Any]], str] = lambda d: "",
                 publish: Callable[[str, dict[str, Any]], None] = lambda topic, data: None,
                 enabled: Callable[[], bool] = lambda: True, sounds: Callable[[], bool] = lambda: True) -> None:
        self.run_turn, self.pending, self.answer = run_turn, pending, answer
        self.speaker, self.journal = speaker, journal
        self.launch_app, self.mode, self.render, self.question = launch_app, mode, render, question
        self.publish = publish
        self.enabled, self.sounds = enabled, sounds          # push-to-talk on? `[voice] sounds`?
        self._chime: asyncio.TimerHandle | None = None
        self.key = TalkKey()
        self.devices: list[InputDevice] = []
        self.permissions: dict[str, Any] = {}
        self.app: asyncio.Queue[dict[str, Any]] | None = None     # commands to FERB.app while it's connected
        self.listening = False

    def _say(self, words: str, line: str | None = None) -> None:
        """Speak (only while push-to-talk is on), and show it in `ferb voice` (the chat-style line when there is one)."""
        if self.enabled():
            self.speaker.say(words)
        self.publish("voice.said", {"text": (line or words).strip()})

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
                self._say("Voice isn't ready yet. Try again in a moment.")
                return act
            device = choose(self.devices, self.mode())
            if device is None:
                self.publish("voice.device_unresolved", {"candidates": [
                    {"uid": d.uid, "name": d.name, "transport": d.transport, "default": d.default}
                    for d in self.devices]})
                self._say("I can't identify one microphone from the configured preference.")
                return act
            self.listening = True
            self.app.put_nowait({"op": "start", "device_uid": device.uid, "device": device.name})   # silent
            if self.sounds():                                    # the start sound once it's a real hold, not a tap
                app = self.app

                def chime() -> None:
                    if self.key.held:
                        app.put_nowait({"op": "chime"})
                try:
                    self._chime = asyncio.get_running_loop().call_later(MIN_HOLD_S, chime)
                except RuntimeError:                             # no event loop (only outside the daemon)
                    self._chime = None
        elif act in ("send", "ignore", "discard"):
            if self._chime is not None:
                self._chime.cancel()
                self._chime = None
            if self.listening and self.app is not None:
                self.listening = False
                self.app.put_nowait({"op": "stop", "sound": self.sounds()} if act == "send" else {"op": "discard"})
        return act

    # ---------- what the owner said ----------

    async def transcript(self, text: str) -> dict[str, Any]:
        pending = self.pending()
        if any(getattr(q, 'kind', '') == 'owner_login_code' for q in pending):
            self._say("Enter the login code in ferb chat's hidden prompt.")
            return {"handled": "secure_input_required"}
        if len(pending) > 1:
            questions = [{"id": q.id, "question": getattr(q, "question", ""),
                          "task_id": getattr(q, "task_id", None)} for q in pending]
            self._say("Several questions are waiting. Select the intended question in ferb chat before answering.")
            self.publish("voice.question_selection_required", {"questions": questions})
            return {"handled": "question_selection_required", "questions": questions}
        self.publish("voice.heard", {"text": text})
        if not text.strip():
            self._say("I didn't catch that.")
            return {"handled": "empty"}
        if pending:
            self.answer(pending[0].id, True, None, text)
            self._say("Got it.")
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
                if line and line.lstrip().startswith(("ferb>", "?")):
                    shown = line.strip().removeprefix("ferb>").strip()
                    shown = shown.replace("number or your answer (Enter to skip):", "say your answer")
                    shown = shown.replace("your answer (Enter to skip):", "say your answer")
                    words = spoken(shown)
                    await asyncio.to_thread(self.speaker.wait)       # don't cut off what it's still saying
                    self._say(words, shown)
        except Exception as e:                                    # the owner hears it, the journal keeps it
            self.journal.append("voice.failed", {"why": f"{type(e).__name__}: {e}"})
            self._say("Sorry — something went wrong on my side.")
