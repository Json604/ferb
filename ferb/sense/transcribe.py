"""Turn a captured sound file into text on this Mac with Apple's on-device speech recognition, run by FERB.app. Must
not use a cloud recognizer, ask macOS for a permission from a task, or keep or log the raw transcript.

L26 (owner, 2026-10-14). Apple requires an app using the Speech framework to carry NSSpeechRecognitionUsageDescription
in its Info.plist, or it crashes (developer.apple.com/documentation/speech/asking-permission-to-use-speech-recognition);
FERB's daemon is a bare Python interpreter, so FERB.app (macapp/FERB, signed with FERB's local certificate) does it:
- launched per transcription with `/usr/bin/open -g -n -W --env FERB_HOME=… FERB.app --args transcribe <audio>
  <answer>`: through LaunchServices so macOS holds FERB.app (not the daemon) responsible for the permission, in the
  background (-g, an LSUIElement app), waited on (-W); its signature is checked against the one `ferb setup` recorded
  before it is given anything;
- FERB.app requires on-device recognition (`requiresOnDeviceRecognition`, after `supportsOnDeviceRecognition`) and
  otherwise answers on_device_unavailable: there is no cloud fallback;
- the answer file is also FERB.app's lease. This side creates it empty (0600, in FERB's private audio folder) before
  the launch; FERB.app accepts only such a file there, opens it without creating anything, and writes its answer into
  it. Deleting the file means stop: FERB.app watches it and exits without answering. Killing `open -W` alone wouldn't
  stop an app LaunchServices started, and an answer written after the deletion lands in an unlinked file, so no raw
  transcript is left behind. This side reads the answer once and deletes it. Nothing goes to stdout, stderr or a log.
`redacted` builds what a model may see: the transcript through the Redactor, and sentence segments cut from that
redacted text only, so a secret spoken across several words never survives in any one segment."""

import asyncio
import json
import os
import secrets
import subprocess
from dataclasses import dataclass, field
from difflib import SequenceMatcher
from pathlib import Path
from typing import Any, Awaitable, Callable

from ferb.core.errors import Refused

Launch = Callable[[list[str]], Awaitable[int]]
SENTENCE_END = (".", "?", "!", "…", "。", "?", "!", "\n")


class HearingUnavailable(Refused):
    """FERB can't turn the sound into text right now; the message says why and what fixes it."""


@dataclass(frozen=True)
class Word:
    text: str
    start: float              # seconds from the start of the audio file
    end: float


@dataclass
class Transcript:
    text: str
    words: list[Word] = field(default_factory=list)
    locale: str = ""


def launch_args(app: Path, mode: str, *args: str, home: Path | None = None) -> list[str]:
    """FERB_HOME tells FERB.app which audio folder its answer and input files must be in: FERB's home, which is the
    folder ~/.ferb/FERB.app sits in, but not the packaged FERB.app's nested voice helper's (L27)."""
    return ["/usr/bin/open", "-g", "-n", "-W", "--env", f"FERB_HOME={home or app.parent}", str(app), "--args", mode,
            *args]


async def launch(argv: list[str]) -> int:
    """Run `open … -W` until FERB.app exits; a cancelled wait kills the launch. Its output goes nowhere."""
    proc = await asyncio.create_subprocess_exec(*argv, stdin=asyncio.subprocess.DEVNULL,
                                                stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.DEVNULL)
    try:
        return await proc.wait()
    except BaseException:
        if proc.returncode is None:
            proc.kill()
        raise


def signed(app: Path, requirement: str) -> bool:
    """FERB.app's signature meets the requirement `ferb setup` recorded (bundle id + FERB's certificate)."""
    r = subprocess.run(["/usr/bin/codesign", "--verify", "--strict", f"-R={requirement}", str(app)], capture_output=True)
    return r.returncode == 0


class Transcriber:
    def __init__(self, app: Path, requirement: Callable[[], str | None], launch: Launch = launch,
                 signed: Callable[[Path, str], bool] = signed, *, home: Path | None = None) -> None:
        """`home`: FERB's home, whose workspace/audio folder FERB.app reads and answers in (None: the folder `app`
        is in, ~/.ferb for a checkout install's ~/.ferb/FERB.app)."""
        self.app, self.requirement, self.launch, self.signed = app, requirement, launch, signed
        self.home = home

    async def _checked(self) -> None:
        if not self.app.exists():
            raise HearingUnavailable("FERB.app isn't installed, so FERB can't turn sound into text: run `ferb setup`")
        req = self.requirement()
        if not req or not await asyncio.to_thread(self.signed, self.app, req):
            raise HearingUnavailable("FERB.app's signature isn't the one recorded at setup, so FERB didn't give it "
                                     "the sound: run `ferb setup`")

    async def _ask(self, mode: str, folder: Path, *args: str) -> dict[str, Any]:
        await self._checked()
        folder.mkdir(parents=True, exist_ok=True)
        folder.chmod(0o700)
        answer = folder / f".{mode}-{secrets.token_hex(8)}.json"
        os.close(os.open(answer, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600))   # the lease
        try:
            code = await self.launch(launch_args(self.app, mode, *args[:1], str(answer), *args[1:], home=self.home))
            try:
                raw = answer.read_text()
            except OSError:
                raw = ""
            if not raw:
                raise HearingUnavailable(f"FERB.app gave no answer (exit {code}); run `ferb doctor`")
            data = json.loads(raw)
        except ValueError:
            raise HearingUnavailable("FERB.app's answer couldn't be read; run `ferb doctor`") from None
        finally:                                  # read once and never kept; a stop deletes it first (FERB.app exits)
            answer.unlink(missing_ok=True)
        if not isinstance(data, dict):
            raise HearingUnavailable("FERB.app's answer couldn't be read; run `ferb doctor`")
        return data

    async def transcribe(self, audio: Path, language: str | None) -> Transcript:
        data = await self._ask("transcribe", audio.parent, str(audio), *([language] if language else []))
        if data.get("error"):
            raise HearingUnavailable(_why(data, audio))
        words = []
        for s in data.get("segments") or []:
            start = float(s.get("start") or 0)
            words.append(Word(str(s.get("text") or ""), start, start + float(s.get("duration") or 0)))
        return Transcript(str(data.get("text") or ""), words, str(data.get("locale") or language or ""))

    async def status(self, folder: Path) -> dict[str, Any]:
        """FERB.app's Speech Recognition state, for `ferb doctor` and setup: speech, on_device, locale. No prompt."""
        data = await self._ask("status", folder)
        return {k: data[k] for k in ("speech", "on_device", "locale") if k in data}


def _why(data: dict[str, Any], audio: Path) -> str:
    reason, locale = str(data.get("reason") or ""), str(data.get("locale") or "this language")
    if reason == "on_device_unavailable":
        return (f"On-device speech recognition isn't available for {locale} on this Mac, so FERB didn't transcribe it; "
                "it never sends sound to a cloud recognizer.")
    if reason == "not_authorized":
        return (f"FERB.app isn't allowed to use Speech Recognition ({data.get('speech') or 'not allowed'}): run "
                "`ferb setup` and allow it.")
    if reason == "unsupported_language":
        return f"Apple speech recognition has no recognizer for {locale}."
    return f"FERB.app couldn't transcribe the sound: {data.get('error')}."


def redacted(t: Transcript, redact: Callable[[str], str], offset: float = 0.0) -> dict[str, Any]:
    """The transcript through the Redactor, and its sentences with times (seconds, plus `offset`) as segments. Each
    segment's text is a slice of the already-redacted transcript, so no secret can come back through a segment."""
    raw = t.text
    red = redact(raw)
    ops = SequenceMatcher(None, raw, red, autojunk=False).get_opcodes()

    def at(i: int, end: bool) -> int:
        """Where raw position i falls in the redacted text; inside a replaced stretch, its whole replacement."""
        for tag, i1, i2, j1, j2 in ops:
            if (i1 < i <= i2) if end else (i1 <= i < i2):
                return j1 + (i - i1) if tag == "equal" else (j2 if end else j1)
        return len(red) if i >= len(raw) else 0

    placed, cursor = [], 0                       # each word's place in the transcript, in order
    for w in t.words:
        found = raw.find(w.text, cursor) if w.text else -1
        if found < 0:
            continue
        placed.append((w, found, found + len(w.text)))
        cursor = found + len(w.text)
    segments, group = [], []
    for n, item in enumerate(placed):
        group.append(item)
        nxt = placed[n + 1][1] if n + 1 < len(placed) else len(raw)
        if n + 1 == len(placed) or any(ch in raw[item[2] - 1:nxt] for ch in SENTENCE_END):
            first, last = group[0], group[-1]
            gap = raw[last[2]:nxt]
            stop = last[2] + next((k + 1 for k, ch in enumerate(gap) if ch in SENTENCE_END), 0)
            text = redact(red[at(first[1], False):at(stop, True)].strip())
            if text:
                segments.append({"start": round(first[0].start + offset, 2), "end": round(last[0].end + offset, 2),
                                 "text": text})
            group = []
    return {"transcript": red, "segments": segments}
