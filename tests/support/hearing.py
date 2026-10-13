"""Fakes at L26's two Mac boundaries: ScreenCaptureKit app-audio capture (ferb/act/mac/audio.py's AppAudio) and the
FERB.app launch that runs Apple's on-device speech recognition (ferb/sense/transcribe.py). Nothing here captures real
sound, launches an app or asks macOS for a permission."""

import json
import math
import os
import struct
import threading
import time
from pathlib import Path
from typing import Any

APP = "/Applications/FERB.app"           # replaced by each test's own folder


def sine(seconds: float = 0.5, rate: int = 16000, level: float = 0.5) -> bytes:
    """Mono float32 samples, as ScreenCaptureKit delivers them with channelCount 1."""
    n = int(seconds * rate)
    return struct.pack(f"<{n}f", *(level * math.sin(2 * math.pi * 440 * i / rate) for i in range(n)))


class FakeHandle:
    def __init__(self, capture: "FakeCapture", bundle_id: str, pid: int, path: Path) -> None:
        self.capture, self.bundle_id, self.pid, self.path = capture, bundle_id, pid, path
        self.stopped = False

    def stop(self) -> dict[str, Any]:
        from ferb.act.mac.audio import WavWriter
        self.stopped = True
        w = WavWriter(self.path)
        if self.capture.samples:
            w.write(self.capture.samples, self.capture.rate)
        return w.close()


class FakeCapture:
    """AppAudio's interface: start(bundle_id, pid, path) -> a handle whose stop() finishes the WAV file."""

    def __init__(self, samples: bytes | None = None, rate: int = 16000, refuse: str | None = None) -> None:
        self.samples = sine() if samples is None else samples
        self.rate, self.refuse = rate, refuse
        self.handles: list[FakeHandle] = []
        self.started = threading.Event()

    def start(self, bundle_id: str, pid: int, path: Path) -> FakeHandle:
        if self.refuse:
            from ferb.core.errors import Refused
            raise Refused(self.refuse)
        h = FakeHandle(self, bundle_id, pid, path)
        self.handles.append(h)
        self.started.set()
        return h


class FakeApp:
    """FERB.app as `open -g -n -W FERB.app --args <mode> ...` runs it: writes its answer (0600) where it was told."""

    def __init__(self, app: Path, text: str = "", words: list[tuple[str, float, float]] | None = None,
                 error: dict[str, Any] | None = None, status: dict[str, Any] | None = None) -> None:
        self.app, self.text, self.words, self.error = app, text, words or [], error
        self.status_answer = status or {"speech": "granted", "on_device": True, "locale": "en-US"}
        self.argv: list[list[str]] = []
        self.raw_paths: list[Path] = []

    async def __call__(self, argv: list[str]) -> int:
        self.argv.append(list(argv))
        assert argv[:6] == ["open", "-g", "-n", "-W", str(self.app), "--args"], argv
        mode = argv[6]
        if mode == "transcribe":
            audio, out = Path(argv[7]), Path(argv[8])
            assert audio.is_file(), "FERB.app is given the captured file"
            answer = self.error or {"on_device": True, "text": self.text, "locale": argv[9] if len(argv) > 9 else "en-US",
                                    "segments": [{"text": t, "start": s, "duration": d} for t, s, d in self.words]}
        elif mode == "status":
            out, answer = Path(argv[7]), self.status_answer
        else:
            raise AssertionError(mode)
        fd = os.open(out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as f:
            json.dump(answer, f)
        self.raw_paths.append(out)
        return 0


def words(text: str, step: float = 0.4) -> list[tuple[str, float, float]]:
    """Word segments as Apple's recognizer gives them: each word, its start and duration (seconds)."""
    out, t = [], 0.0
    for w in text.replace(".", " ").replace(",", " ").split():
        out.append((w, round(t, 2), step * 0.8))
        t += step
    return out


def wait_started(capture: FakeCapture, timeout: float = 5) -> None:
    end = time.monotonic() + timeout
    while not capture.started.is_set():
        assert time.monotonic() < end, "the capture never started"
        time.sleep(0.01)
