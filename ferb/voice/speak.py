"""Speak FERB's own replies aloud through the speech provider the owner configured. Must not choose what is said or
when, or let any provider receive text that hasn't been through the Redactor.

Owner decisions (notes/voice.md 2, 3): macOS's own voice now; other providers plug in later the way model runtimes do
(ferb/runtimes/catalog.py). A reply is spoken when the app's turn asked for it (`chat.send` with `speak: true`, sent
from the app's dictation); its words come from the turn's structured reply and answer events (ferb/daemon/areas/voice.py).
- `[voice]` in config.toml: `tts` names a provider in PROVIDERS (default "macos"), `voice` the provider's voice
  (default: the system voice), `rate` words per minute (default: the voice's own). Values are checked here, at use.
- `Speech` is the one shared path: every text goes through the Redactor before a provider is even built, so a hosted
  provider would only ever receive redacted text. One utterance at a time, in order; `interrupt` (a new turn, a stop)
  ends the current one at once and drops the rest. It runs on the event loop and never blocks it.
- A provider is async: `say` returns once the words are spoken, and cancelling it stops the speech at once."""

import asyncio
import contextlib
import math
import re
from collections import deque
from dataclasses import dataclass
from typing import Any, Callable, Protocol

from ferb.core.errors import FerbError


class VoiceConfigError(FerbError):
    """A `[voice]` value FERB can't use; the message names the key and what it accepts."""


class Speaker(Protocol):
    async def say(self, text: str) -> None: ...


@dataclass(frozen=True)
class VoiceSettings:
    tts: str
    voice: str | None = None
    rate: float | None = None


@dataclass(frozen=True)
class Provider:
    name: str
    build: Callable[[VoiceSettings], Speaker]


class MacVoice:
    """macOS's own voice (`say`), the text on its standard input: never on its command line or in the process list."""

    def __init__(self, voice: str | None = None, rate: float | None = None,
                 run: Callable[..., Any] = asyncio.create_subprocess_exec) -> None:
        self.voice, self.rate, self._run = voice, rate, run

    def argv(self) -> list[str]:
        return ["/usr/bin/say", *(["-v", self.voice] if self.voice else []),
                *(["-r", f"{self.rate:g}"] if self.rate else [])]

    async def say(self, text: str) -> None:
        proc = await self._run(*self.argv(), stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.DEVNULL,
                               stderr=asyncio.subprocess.PIPE)
        try:
            _, err = await proc.communicate(text.encode())
        except BaseException:                   # interrupted: the voice stops now
            if proc.returncode is None:
                with contextlib.suppress(ProcessLookupError):
                    proc.kill()
            raise
        if proc.returncode:
            raise FerbError(f"say exited {proc.returncode}: {(err or b'').decode(errors='replace').strip()[:200]}")


PROVIDERS: dict[str, Provider] = {
    "macos": Provider("macos", lambda s: MacVoice(s.voice, s.rate)),
}


def settings(config: Any) -> VoiceSettings:
    """The owner's `[voice]` speech settings, checked; VoiceConfigError names a value FERB can't use."""
    tts = config.get("voice", "tts", "macos")
    if not isinstance(tts, str) or tts not in PROVIDERS:
        raise VoiceConfigError(f"[voice] tts is {tts!r}; FERB speaks with: {', '.join(sorted(PROVIDERS))}")
    voice = config.get("voice", "voice")
    if voice is not None and (not isinstance(voice, str) or not voice.strip() or voice.startswith("-")
                              or any(ord(ch) < 32 for ch in voice)):
        raise VoiceConfigError(f"[voice] voice is {voice!r}; it must be a voice's name, as macOS lists them")
    rate = config.get("voice", "rate")
    if rate is not None and (isinstance(rate, bool) or not isinstance(rate, (int, float)) or not math.isfinite(rate)
                             or rate <= 0):
        raise VoiceConfigError(f"[voice] rate is {rate!r}; it must be a positive number of words per minute")
    return VoiceSettings(tts, voice, float(rate) if rate is not None else None)


def build(config: Any) -> Speaker:
    """The configured provider, built with the owner's settings."""
    s = settings(config)
    return PROVIDERS[s.tts].build(s)


class Speech:
    """The daemon's voice: what to say, in order, through a provider built for each utterance (a settings change
    applies to the next one). `said`: told each utterance as it starts; `failed`: why one couldn't be spoken."""

    def __init__(self, speaker: Callable[[], Speaker], redact: Callable[[str], str], *,
                 said: Callable[[str], None] = lambda words: None,
                 failed: Callable[[str], None] = lambda why: None) -> None:
        self.speaker, self.redact, self.said, self.failed = speaker, redact, said, failed
        self._pending: deque[str] = deque()
        self._job: asyncio.Task[None] | None = None

    def say(self, text: str) -> None:
        """Queue `text` (on the event loop); it is redacted here, before anything else sees it: the raw text first (a
        saved value holding `**` or `$` no longer matches once `spoken` strips those), then the words as reshaped."""
        words = self.redact(spoken(self.redact(text)))
        if not words.strip():
            return
        self._pending.append(words)
        if self._job is None or self._job.done():
            self._job = asyncio.ensure_future(self._run())

    def interrupt(self) -> None:
        """Stop speaking now and forget what was queued."""
        self._pending.clear()
        if self._job is not None and not self._job.done():
            self._job.cancel()
        self._job = None

    @property
    def speaking(self) -> bool:
        return self._job is not None and not self._job.done()

    async def close(self) -> None:
        job = self._job
        self.interrupt()
        if job is not None:
            with contextlib.suppress(asyncio.CancelledError):
                await job

    async def _run(self) -> None:
        while self._pending:
            words = self._pending.popleft()
            try:
                speaker = self.speaker()
                self.said(words)
                await speaker.say(words)
            except asyncio.CancelledError:
                raise
            except Exception as e:              # a bad setting or a failed provider: the rest isn't tried either
                self._pending.clear()
                self.failed(f"{type(e).__name__}: {e}")


def spoken(text: str) -> str:
    """Words to say: markdown links as their text, no emoji, arrows or markdown marks, units read naturally."""
    text = re.sub(r"\[([^\]]+)\]\([^)\s]+\)", r"\1", text)
    text = re.sub(r"\*\*|__|`|^#+ ", "", text, flags=re.M)
    text = re.sub(r"[^\w\s.,;:!?'’\"()/%+@&-]", "", text)           # emoji, arrows, bullets
    text = re.sub(r"\b(\d+) s\b", r"\1 seconds", text)
    return " ".join(text.split())
