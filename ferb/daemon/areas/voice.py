"""Own the daemon's voice: speaking a voice turn's replies (notes/voice.md decisions 2, 3) and hearing's FERB.app
plumbing (where the app is, its recorded signature, its Speech Recognition probe, ending its hearing before an update).
Must not listen: hearing only transcribes sound a task captured from an app (L26). Push-to-talk is the FERB app's own
dictation; a turn it sends with `speak: true` is answered aloud from the turn's structured events, never from rendered
text."""

import asyncio
import contextlib
import os
import signal
from pathlib import Path
from typing import Any


def answer_words(event: dict[str, Any]) -> str:
    """What a voice turn says for one of its events: a reply's text, a task's plan line as it starts, or its answer as
    it ends, without code's own lines after them (sites contacted, schedules, apps left open: the app shows them; they
    aren't read out). A resumed task's plan isn't said again; other events say nothing."""
    kind = event.get("type")
    if kind == "reply":
        return str(event.get("text") or "")
    if kind == "plan" and not event.get("resumed"):
        answer = str(event.get("text") or "")
    elif kind == "outcome":
        answer = str(event.get("answer") or event.get("summary") or "")
    else:
        return ""
    lines = event.get("lines") or (event.get("card") or {}).get("lines") or []
    tail = "\n".join(str(line) for line in lines)
    if tail and answer.endswith(tail):
        answer = answer[: -len(tail)]
    return answer.strip()


class VoiceGlue:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    def _speak_event(self, event: dict[str, Any]) -> None:
        """A voice turn's event: its words go to the shared speech path (redacted there, spoken in order)."""
        words = answer_words(event)
        if words:
            self.speech.say(words)

    def _speech_failed(self, why: str) -> None:
        self._journal().append("voice.failed", {"why": self._redact_text(why)})

    @property
    def voice_bundle(self) -> Path:
        """~/.ferb/FERB.app from a checkout install; FERB.app itself when the daemon runs from it."""
        from ferb.device import install
        return install.voice_app(self.home, install.app_bundle())

    def _hearing_requirement(self) -> str | None:
        """FERB.app's signature as setup recorded it (hearing; `[voice] requirement` from older setups: same app)."""
        req = self.config.get("hearing", "requirement") or self.config.get("voice", "requirement")
        return str(req) if req else None

    async def _hearing_status(self, probe: bool) -> dict[str, Any]:
        """L26 for doctor and setup: FERB.app installed and signed as recorded, and (probe) its Speech Recognition state
        and on-device support, asked of FERB.app itself in the background. Never a prompt."""
        out: dict[str, Any] = {"installed": self.voice_bundle.exists()}
        if not out["installed"] or not probe:
            return out
        from ferb.sense.transcribe import signed
        req = self._hearing_requirement()
        out["signed"] = bool(req) and await asyncio.to_thread(signed, self.voice_bundle, str(req))
        if out["signed"]:
            try:
                out.update(await self.hearing.transcriber.status(self.hearing.folder))
            except Exception as e:
                out["error"] = self._redact_text(f"{type(e).__name__}: {e}")
        return out

    def _quit_voice_app(self) -> None:
        """Hearing runs in the app an update replaces. One transcribing is never ended: it stops through its lease when
        its ask ends (the daemon's shutdown cancels it, deleting the lease), and the process then exits itself. Otherwise
        one process scan finds the hearing instances (FERB.app's executable run with `transcribe` or `status` on a file in
        FERB's audio folder) and ends them with SIGTERM: they have no run loop for a quit event, and the bundle id is also
        the running notch's, which the update quits itself and which never matches."""
        transcriber = self.hearing.transcriber
        if transcriber.busy:
            return
        for pid in transcriber.pids(transcriber.app, self.hearing.folder):
            with contextlib.suppress(OSError):
                os.kill(pid, signal.SIGTERM)
