"""Own push-to-talk's and hearing's FERB.app plumbing. Must not listen without the owner's key press."""

import asyncio
import subprocess
from pathlib import Path
from typing import Any, AsyncIterator

from ferb.daemon.server import Session


class VoiceGlue:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    def _build_voice(self) -> None:
        from ferb.cli.chat import question, render
        from ferb.voice.controller import Voice
        from ferb.voice.speak import SaySpeaker
        self.voice = Voice(self._voice_turn, self.state.approvals.pending, self.voice_speaker or SaySpeaker(),
                           self._journal(), self._launch_voice_app,
                           mode=lambda: str(self.config.get("voice", "mic", "auto")), render=render, question=question,
                           publish=self._publish, hide=lambda text: self._hide_code_reply(text, "voice"),
                           redact=lambda text: self.redact(text),
                           enabled=lambda: self._ptt, sounds=lambda: bool(self.config.get("voice", "sounds", True)))
        loop = asyncio.get_running_loop()

        def on_key(code: int, down: bool) -> None:               # the tap's thread: hand off to the loop
            if self._ptt:
                loop.call_soon_threadsafe(self.voice.on_key, code, down)
        self.owner_input.on_key = on_key
        self.server.unary["voice.transcript"] = self._voice_transcript
        self.server.unary["voice.reload"] = self._voice_reload
        self.server.unary["voice.switch"] = self._voice_switch
        self.server.streams["voice.attach"] = self._voice_attach
        self.server.voice_app = self._voice_app_exe
        self.server.voice_signed = self._voice_signed
        if self._ptt:
            self._launch_voice_app()

    @property
    def voice_bundle(self) -> Path:
        """~/.ferb/FERB.app from a checkout install; the voice helper nested in FERB.app when the daemon runs from it."""
        from ferb.device import install
        return install.voice_app(self.home, install.app_bundle())

    def _hearing_requirement(self) -> str | None:
        """FERB.app's signature as `ferb setup` recorded it (hearing), or as `ferb setup --voice` did (same app)."""
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

    def _voice_app_exe(self) -> str | None:
        exe = self.voice_bundle / "Contents" / "MacOS" / "FERB"
        return str(exe) if self._ptt and exe.exists() else None

    def _voice_signed(self, exe: str) -> bool:
        """FERB.app's signature meets the requirement recorded by `ferb setup --voice` (bundle id + certificate)."""
        req = self.config.get("voice", "requirement")
        if not req:
            return False
        r = subprocess.run(["/usr/bin/codesign", "--verify", "--strict", f"-R={req}", str(self.voice_bundle)],
                           capture_output=True)
        return r.returncode == 0

    def _launch_voice_app(self) -> None:
        if self.voice_bundle.exists():
            subprocess.Popen(["/usr/bin/open", "-g", str(self.voice_bundle)], stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL)

    async def _voice_turn(self, text: str) -> AsyncIterator[dict[str, Any]]:
        async for ev in self._chat_send(None, {"session_id": "voice", "text": text}):     # type: ignore[arg-type]
            yield ev

    async def _voice_attach(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        """FERB.app connects and waits for commands: start (with the chosen mic), stop, discard."""
        q = self.voice.attach(list(params.get("devices", [])), dict(params.get("permissions", {})))
        try:
            while True:
                yield await q.get()
        finally:
            self.voice.detach(q)

    async def _voice_transcript(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if params.get("on_device") is False:                     # no on-device recognition: never the network
            self._journal().append("voice.failed", {"why": str(params.get("error") or "on-device recognition unavailable")})
            self.voice._say("On-device speech recognition isn't available on this Mac, so I didn't listen. Your voice "
                            "never leaves the Mac.")
            return {"handled": "on_device_unavailable"}
        if params.get("error"):                                  # FERB.app couldn't listen
            self._journal().append("voice.failed", {"why": str(params["error"])})
            self.voice._say("I couldn't use the microphone.")
            return {"handled": "error"}
        result: dict[str, Any] = await self.voice.transcript(str(params.get("text", "")))
        return result

    async def _voice_switch(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`ferb voice on|off`. Off: the setting, FERB stops speaking, FERB.app quits. On: needs `ferb setup --voice`
        to have recorded FERB.app's signature."""
        on = bool(params.get("on"))
        if on and not self.config.get("voice", "requirement"):
            return {"needs_setup": True, "push_to_talk": False}
        self.config.set("voice", "push_to_talk", on)
        out = await self._voice_reload(session, {})
        if not on:
            self._quit_voice_app()
        self._journal().append("voice.switched", {"on": on})
        return out

    def _quit_voice_app(self) -> None:
        from AppKit import NSRunningApplication
        for app in NSRunningApplication.runningApplicationsWithBundleIdentifier_("com.ferb.app"):
            app.terminate()

    async def _voice_reload(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`ferb setup --voice` finished: read the voice settings again and open FERB.app (no daemon restart)."""
        self._load_config()
        if self._ptt:
            self._launch_voice_app()
        elif self.voice is not None:
            self.voice.speaker.stop()                     # off: stop speaking replies to voice requests now
        return {"push_to_talk": self._ptt}
