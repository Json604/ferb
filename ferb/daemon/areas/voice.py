"""Own hearing's FERB.app plumbing: where FERB Voice is, its recorded signature, its Speech Recognition probe, and
quitting it before an update. Must not listen: FERB Voice only transcribes sound a task captured from an app (L26).
Push-to-talk is the FERB app's own dictation (notes/voice.md decision 3)."""

import asyncio
from pathlib import Path
from typing import Any


class HearingGlue:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    @property
    def voice_bundle(self) -> Path:
        """~/.ferb/FERB.app from a checkout install; the voice helper nested in FERB.app when the daemon runs from it."""
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
        from AppKit import NSRunningApplication
        for app in NSRunningApplication.runningApplicationsWithBundleIdentifier_("com.ferb.app"):
            app.terminate()
