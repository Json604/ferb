"""Own hearing's FERB.app plumbing (where the app is, its recorded signature, its Speech Recognition probe, ending its
hearing before an update). Must not listen: hearing only transcribes sound a task captured from an app (L26).
Push-to-talk is the FERB app's own dictation. Must not speak: FERB replies only in text (VOICE-409)."""

import contextlib
import os
import signal
from pathlib import Path
from typing import Any


class VoiceGlue:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

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
        and on-device support, asked of FERB.app itself in the background. Never a prompt. The signature is verified
        once per call, by the status ask itself (Transcriber._checked)."""
        out: dict[str, Any] = {"installed": self.voice_bundle.exists()}
        if not out["installed"] or not probe:
            return out
        from ferb.sense.transcribe import NotSigned
        try:
            out.update(await self.hearing.transcriber.status(self.hearing.folder))
            out["signed"] = True
        except NotSigned:
            out["signed"] = False
        except Exception as e:
            out["signed"] = True                    # the check passed; FERB.app's answer didn't come
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
