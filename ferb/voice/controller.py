"""Coordinate push-to-talk transcription. Must not answer a pending question by itself: what was said routes like typed text.

Push-to-talk in the daemon: the talk key's decisions become commands for FERB.app; what the owner said
comes back as a transcript and runs as a chat turn in the "voice" session; FERB's replies are spoken.

FERB.app is the only part that opens a microphone; this side decides which one (never a Bluetooth headset's), when to
start, send or discard, and what to do with the words. What the owner said is a chat turn, open question or not: the
planner routes an answer to its question by id, as for typed text (L3). Secrets use chat's hidden input, never the voice
event stream or a model."""

import asyncio
import time
from typing import Any, AsyncIterator, Callable

from ferb.orchestrator.worker import couldnt
from ferb.voice.mic import InputDevice, choose
from ferb.voice.push_to_talk import MIN_HOLD_S, TalkKey
from ferb.voice.speak import spoken

HIDDEN_QUESTIONS = frozenset({'owner_login_code', 'owner_secret', 'owner_admin_password'})

class Voice:
    def __init__(self, run_turn: Callable[[str], AsyncIterator[dict[str, Any]]], pending: Callable[[], list[Any]],
                 speaker: Any, journal: Any,
                 launch_app: Callable[[], None], mode: Callable[[], str] = lambda: "auto",
                 render: Callable[[dict[str, Any]], str | None] = lambda ev: None,
                 question: Callable[[dict[str, Any]], str] = lambda d: "",
                 publish: Callable[[str, dict[str, Any]], None] = lambda topic, data: None,
                 hide: Callable[[str], str] = lambda text: text,
                 enabled: Callable[[], bool] = lambda: True, sounds: Callable[[], bool] = lambda: True,
                 redact: Callable[[str], str] = lambda text: text) -> None:
        self.run_turn, self.pending = run_turn, pending
        self.speaker, self.journal = speaker, journal
        self.launch_app, self.mode, self.render, self.question = launch_app, mode, render, question
        self.publish, self.hide = publish, hide            # hide: a code said to a code question (rule 14)
        self.redact = redact                                # what a failure's reason says goes through it first
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
            if any(getattr(q, 'kind', '') in HIDDEN_QUESTIONS for q in self.pending()):
                self._say("Enter the secret in ferb chat's hidden prompt.")
                return act
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
        """What the owner said runs as a chat turn, like typed text: with a question open, the planner routes an answer
        to it by id (L3), so a correction or a new request never answers the only open question. While a hidden
        input is waiting, nothing said goes to a model: a spoken code would."""
        if any(getattr(q, 'kind', '') in HIDDEN_QUESTIONS for q in self.pending()):
            self._say("Enter the secret in ferb chat's hidden prompt.")
            return {"handled": "secure_input_required"}
        if not text.strip():
            self._say("I didn't catch that.")
            return {"handled": "empty"}
        text = self.hide(text)                     # never published or sent on as said (rule 14, L18)
        self.publish("voice.heard", {"text": text})
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
                    shown = shown.replace("number or your answer (Enter = No):", "say Yes or No")
                    shown = shown.replace("number or your answer (Enter = Drop):", "say Keep, Edit or Drop")
                    shown = shown.replace("number or your answer (Enter to skip):", "say your answer")
                    shown = shown.replace("your answer (Enter to skip):", "say your answer")
                    words = spoken(shown)
                    await asyncio.to_thread(self.speaker.wait)       # don't cut off what it's still saying
                    self._say(words, shown)
        except Exception as e:                # the owner hears why (owner 2026-10-24), the journal keeps it whole
            self.journal.append("voice.failed", {"why": f"{type(e).__name__}: {e}"})
            self._say(str(self.redact(couldnt("do that", e, self.redact))))
