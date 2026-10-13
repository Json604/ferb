"""Render supplied status text as speech with interruption support. Must not invent task completion or decide task outcomes.

FERB's voice (owner request, 2026-10-01): the Mac's built-in `say` for now, behind a small interface so a
text-to-speech API can replace it later. FERB speaks its own words only — answers and questions —
never internals; anything new to say, or the owner typing, cuts off what it was saying."""

import re
import subprocess
from typing import Protocol


class Speaker(Protocol):
    def say(self, text: str) -> None: ...
    def stop(self) -> None: ...
    def wait(self) -> None: ...


class Silent:
    def say(self, text: str) -> None:
        return None

    def stop(self) -> None:
        return None

    def wait(self) -> None:
        return None


class SaySpeaker:
    """macOS `say` with the system voice; one utterance at a time."""

    def __init__(self, run: type[subprocess.Popen] = subprocess.Popen) -> None:   # type: ignore[type-arg]
        self._run = run
        self._proc: subprocess.Popen | None = None                                   # type: ignore[type-arg]

    def say(self, text: str) -> None:
        words = spoken(text)
        if not words:
            return
        self.stop()
        self._proc = self._run(["say", words], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL)

    def stop(self) -> None:
        if self._proc is not None and self._proc.poll() is None:
            self._proc.terminate()
        self._proc = None

    def wait(self, timeout: float = 60) -> None:
        """Until the current utterance is finished (so the next one doesn't cut it off)."""
        p = self._proc
        if p is not None:
            try:
                p.wait(timeout)
            except Exception:
                pass


def spoken(line: str) -> str:
    """A chat line as words: no "ferb>" or "ferb (noticed)>" prompt, no emoji or markdown, units read naturally."""
    text = re.sub(r"^\s*(ferb( \(noticed\))?>|\?)\s*", "", line.strip())
    text = re.sub(r"\[y/N\]|\(Ctrl-C to cancel\)|`", "", text)
    text = re.sub(r"[^\w\s.,;:!?'’\"()/%+@&-]", "", text)                 # emoji, arrows, bullets
    text = re.sub(r"\b(\d+) s\b", r"\1 seconds", text)
    text = re.sub(r"\*\*|__|#+ ", "", text)
    return " ".join(text.split())
