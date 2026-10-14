"""Capture the sound one app is playing into a WAV file in FERB's workspace. Must not capture FERB's own sound, the
microphone, a display's picture or a substitute app.

L26 (owner, 2026-10-14). ScreenCaptureKit app audio, under the Screen Recording permission FERB already holds for
screenshots (screenshot.py uses ScreenCaptureKit the same way, through PyObjC):
- One app: a display filter that includes only that app's exact process, found in ScreenCaptureKit's own list by
  bundle id and pid, never a near match. Audio is filtered per app (WWDC22 10156), so a browser is heard as a whole,
  every tab of it that makes sound. The app may be in the background: audio doesn't follow focus.
- `excludesCurrentProcessAudio` keeps FERB's own sound out; `captureMicrophone` stays off.
- Audio only in effect: the stream has to deliver video too (without a screen output it logs dropped frames), so the
  video is a 2x2 picture that is ignored.
- Mono float32 samples are written as they arrive (no recording is held in memory), so how long to listen stays the
  model's choice; the file's header is completed when capture stops.
- On its true timeline: each buffer is placed at its presentation time (host clock) relative to the moment capture
  started, and silence the app didn't send (a late first buffer, a pause) is written as zeros, so transcript times
  match the media's. A buffer is never placed later than it arrived (a physical bound, not a FERB limit).
Blocking PyObjC calls: callers run start() and stop() in a thread."""

import array
import struct
import threading
import time
from pathlib import Path
from types import SimpleNamespace
from typing import Any, Callable

from ferb.act.mac.screenshot import TIMEOUT_S, _wait
from ferb.core.errors import Refused

SAMPLE_RATE = 16000          # ScreenCaptureKit accepts 8000, 16000, 24000 or 48000 Hz; speech needs no more than 16 kHz
FLOAT, NON_INTERLEAVED = 1, 32    # kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved
ZEROS = b"\0" * 65536              # silence is written in pieces of this many bytes (memory only, not a limit)


class WavWriter:
    """Mono float32 WAV (format 3, IEEE float), written as samples arrive; close() fills in the sizes."""

    def __init__(self, path: Path) -> None:
        self.path = path
        path.parent.mkdir(parents=True, exist_ok=True)
        self.f = open(path, "xb")                 # a new file only: an existing one is never replaced
        path.chmod(0o600)
        self.f.write(b"\0" * 44)
        self.frames, self.peak, self.rate = 0, 0.0, 0
        self.lock = threading.Lock()

    def write(self, samples: bytes, rate: int, at: float | None = None) -> None:
        """Append mono float32 samples; `at` (seconds from the file's start) zero-fills any silence before them."""
        values = array.array("f")
        values.frombytes(samples[: len(samples) - len(samples) % 4])
        with self.lock:
            if self.f.closed:
                return
            self.rate = self.rate or rate
            gap = round(at * self.rate) - self.frames if at is not None else 0
            left = 4 * gap
            while left > 0:
                piece = min(left, len(ZEROS))
                self.f.write(memoryview(ZEROS)[:piece])
                left -= piece
            self.frames += max(0, gap)
            self.f.write(values.tobytes())
            self.frames += len(values)
            if values:
                self.peak = max(self.peak, max(abs(v) for v in values))

    def close(self) -> dict[str, Any]:
        with self.lock:
            if not self.f.closed:
                rate, data = self.rate or SAMPLE_RATE, self.frames * 4
                self.f.seek(0)
                self.f.write(b"RIFF" + struct.pack("<I", 36 + data) + b"WAVE"
                             + b"fmt " + struct.pack("<IHHIIHH", 16, 3, 1, rate, rate * 4, 4, 32)
                             + b"data" + struct.pack("<I", data))
                self.f.close()
        rate = self.rate or SAMPLE_RATE
        return {"frames": self.frames, "sample_rate": rate, "audio_s": self.frames / rate, "peak": round(self.peak, 4)}


def _frameworks() -> tuple[Any, Any]:
    import CoreMedia  # type: ignore[import-untyped]
    import ScreenCaptureKit  # type: ignore[import-untyped]
    return ScreenCaptureKit, CoreMedia


_SINK: Any = None


def _sink(on_buffer: Callable[[Any, int], None]) -> Any:
    """An SCStreamOutput object handing each sample buffer and its type to `on_buffer` (one class per process)."""
    global _SINK
    if _SINK is None:
        import objc  # type: ignore[import-untyped]
        from Foundation import NSObject  # type: ignore[import-untyped]

        class FerbAudioSink(NSObject, protocols=[objc.protocolNamed("SCStreamOutput")]):  # type: ignore[misc, call-arg]
            def stream_didOutputSampleBuffer_ofType_(self, stream: Any, buf: Any, kind: int) -> None:
                self.on_buffer(buf, int(kind))
        _SINK = FerbAudioSink
    sink = _SINK.alloc().init()
    sink.on_buffer = on_buffer
    return sink


def _done(start: Callable[[Callable[[Any], None]], None]) -> None:
    """A ScreenCaptureKit call that answers with an error or nothing (start/stop capture)."""
    finished, box = threading.Event(), {}

    def handler(error: Any) -> None:
        box["e"] = error
        finished.set()
    start(handler)
    if not finished.wait(TIMEOUT_S):
        raise TimeoutError("the screen capture didn't answer")
    if box.get("e") is not None:
        raise RuntimeError(str(box["e"].localizedDescription()))


# AudioStreamBasicDescription's fields in struct order. PyObjC returns the struct as a plain tuple (measured on macOS 26
# with PyObjC 12: CMAudioFormatDescriptionGetStreamBasicDescription -> (48000.0, 1819304813, 41, 4, 1, 4, 1, 32, 0)).
ASBD_FIELDS = ("mSampleRate", "mFormatID", "mFormatFlags", "mBytesPerPacket", "mFramesPerPacket", "mBytesPerFrame",
               "mChannelsPerFrame", "mBitsPerChannel", "mReserved")


def _asbd(raw: Any) -> Any:
    """The stream description by field name, whether PyObjC gives a tuple (it does) or a struct object."""
    return SimpleNamespace(**dict(zip(ASBD_FIELDS, raw))) if isinstance(raw, tuple) else raw


class AudioCapture:
    """One running app-audio stream into one file."""

    def __init__(self, stream: Any, sink: Any, writer: WavWriter, cm: Any,
                 now: Callable[[], float] = time.monotonic) -> None:
        self.stream, self.sink, self.writer, self.cm = stream, sink, writer, cm   # held: a freed sink drops samples
        self.now = now
        self.error: str | None = None
        self.reference = 0.0                            # host-clock seconds at the file's time 0
        self.started = now()                            # the same moment on FERB's monotonic clock

    def on_buffer(self, buf: Any, kind: int) -> None:
        if kind != 1:                                   # SCStreamOutputTypeAudio; the 2x2 video is ignored
            return
        try:
            cm = self.cm
            asbd = _asbd(cm.CMAudioFormatDescriptionGetStreamBasicDescription(cm.CMSampleBufferGetFormatDescription(buf)))
            block = cm.CMSampleBufferGetDataBuffer(buf)
            got = cm.CMBlockBufferCopyDataBytes(block, 0, cm.CMBlockBufferGetDataLength(block), None)
            data = bytes(got[1] if isinstance(got, tuple) else got)
            if not (int(asbd.mFormatFlags) & FLOAT) or int(asbd.mBitsPerChannel) != 32:
                raise ValueError(f"unexpected audio format (flags {asbd.mFormatFlags}, {asbd.mBitsPerChannel} bits)")
            channels = max(1, int(asbd.mChannelsPerFrame))
            if channels > 1:                            # asked for mono; keep the first channel if more arrive
                values = array.array("f")
                values.frombytes(data[: len(data) - len(data) % 4])
                first = values[: len(values) // channels] if int(asbd.mFormatFlags) & NON_INTERLEAVED \
                    else values[::channels]
                data = first.tobytes()
            pts = float(cm.CMTimeGetSeconds(cm.CMSampleBufferGetPresentationTimeStamp(buf)))
            at = min(max(0.0, pts - self.reference), max(0.0, self.now() - self.started))
            self.writer.write(data, int(asbd.mSampleRate), at)
        except Exception as e:                          # recorded, never raised into ScreenCaptureKit's queue
            self.error = self.error or f"{type(e).__name__}: {e}"

    def stop(self) -> dict[str, Any]:
        stopped: str | None = None
        try:
            _done(lambda h: self.stream.stopCaptureWithCompletionHandler_(h))
        except Exception as e:
            stopped = f"{type(e).__name__}: {e}"
        facts = self.writer.close()
        error = self.error or stopped
        return facts | ({"error": error} if error else {})


class AppAudio:
    """Starts app-audio capture: start(bundle_id, pid, path) -> AudioCapture (blocking; run it in a thread)."""

    def __init__(self, now: Callable[[], float] = time.monotonic) -> None:
        self.now = now

    def start(self, bundle_id: str, pid: int, path: Path) -> AudioCapture:
        sck, cm = _frameworks()
        listed = sck.SCShareableContent.getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler_
        content = _wait(lambda h: listed(True, False, h))
        app = next((a for a in content.applications()
                    if int(a.processID()) == pid and str(a.bundleIdentifier() or "") == bundle_id), None)
        if app is None:
            raise Refused(f"{bundle_id} (pid {pid}) isn't one screen capture can hear right now")
        displays = list(content.displays())
        if not displays:
            raise Refused("screen capture lists no display to attach the app's sound to")
        try:
            import Quartz  # type: ignore[import-untyped]
            main = int(Quartz.CGMainDisplayID())
        except Exception:
            main = None
        display = next((d for d in displays if int(d.displayID()) == main), displays[0])
        flt = sck.SCContentFilter.alloc().initWithDisplay_includingApplications_exceptingWindows_(display, [app], [])
        cfg = sck.SCStreamConfiguration.alloc().init()
        cfg.setCapturesAudio_(True)
        cfg.setExcludesCurrentProcessAudio_(True)
        cfg.setCaptureMicrophone_(False)
        cfg.setSampleRate_(SAMPLE_RATE)
        cfg.setChannelCount_(1)
        cfg.setWidth_(2)
        cfg.setHeight_(2)
        cfg.setMinimumFrameInterval_(cm.CMTimeMake(1, 1))
        cfg.setShowsCursor_(False)
        writer = WavWriter(path)
        stream = sck.SCStream.alloc().initWithFilter_configuration_delegate_(flt, cfg, None)
        capture = AudioCapture(stream, None, writer, cm, self.now)
        capture.sink = _sink(capture.on_buffer)
        try:
            for kind in (sck.SCStreamOutputTypeAudio, sck.SCStreamOutputTypeScreen):
                ok = stream.addStreamOutput_type_sampleHandlerQueue_error_(capture.sink, kind, None, None)
                ok, err = ok if isinstance(ok, tuple) else (ok, None)
                if not ok:
                    raise Refused(f"screen capture refused the audio output: {err}")
            # time 0 of the file: no buffer can be presented before capture starts
            capture.reference = float(cm.CMTimeGetSeconds(cm.CMClockGetTime(cm.CMClockGetHostTimeClock())))
            capture.started = self.now()
            _done(lambda h: stream.startCaptureWithCompletionHandler_(h))
        except BaseException:
            writer.close()
            path.unlink(missing_ok=True)
            raise
        return capture
