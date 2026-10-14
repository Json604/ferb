"""I7, M3: a ScreenCaptureKit failure keeps its error domain and code (and the useful parts of its userInfo) so a live
failure like "Failed to start stream due to audio/video capture failure" can be diagnosed; a minimized window is named
as such before ScreenCaptureKit is asked (and FERB leaves it minimized: background first); an audio stream whose start
timed out is stopped, best effort. No real capture runs."""
import types
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from ferb.core.errors import Refused


class FakeError:
    """An NSError as PyObjC hands it over."""

    def __init__(self, domain: str, code: int, text: str, info: dict[str, Any] | None = None) -> None:
        self._domain, self._code, self._text, self._info = domain, code, text, info or {}

    def domain(self) -> str:
        return self._domain

    def code(self) -> int:
        return self._code

    def localizedDescription(self) -> str:
        return self._text

    def userInfo(self) -> dict[str, Any]:
        return self._info


def test_a_screen_capture_error_keeps_its_domain_code_and_reason() -> None:
    from ferb.act.mac import screenshot
    underlying = FakeError("CoreGraphicsErrorDomain", 1003, "denied")
    error = FakeError("com.apple.ScreenCaptureKit.SCStreamErrorDomain", -3805,
                      "Failed to start stream due to audio/video capture failure",
                      {"NSLocalizedFailureReason": "The window isn't available", "NSUnderlyingError": underlying,
                       "unrelated": object()})
    with pytest.raises(screenshot.CaptureError) as raised:
        screenshot._wait(lambda handler: handler(None, error))
    e = raised.value
    assert e.domain == "com.apple.ScreenCaptureKit.SCStreamErrorDomain" and e.code == -3805
    text = str(e)
    for part in ("Failed to start stream", "SCStreamErrorDomain", "-3805", "The window isn't available",
                 "CoreGraphicsErrorDomain 1003"):
        assert part in text, text


def test_the_journal_names_a_capture_error_by_domain_and_code() -> None:
    from ferb.act.mac import screenshot
    from ferb.daemon.gateway import error_name
    e = screenshot.CaptureError("Failed to start stream", "com.apple.ScreenCaptureKit.SCStreamErrorDomain", -3805)
    assert error_name(e) == "CaptureError (com.apple.ScreenCaptureKit.SCStreamErrorDomain -3805)"


def test_an_audio_start_error_keeps_its_domain_and_code() -> None:
    from ferb.act.mac import audio, screenshot
    with pytest.raises(screenshot.CaptureError) as raised:
        audio._done(lambda handler: handler(FakeError("SCStreamErrorDomain", -3801, "The user declined TCCs")))
    assert raised.value.code == -3801 and "-3801" in str(raised.value)


def test_a_minimized_window_is_named_and_never_unminimized(monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.act.mac import accessibility, screenshot
    asked: list[Any] = []
    monkeypatch.setattr(accessibility, "window_minimized", lambda pid, bounds: True)
    monkeypatch.setattr(screenshot, "_wait", lambda start: asked.append(start))
    with pytest.raises(Refused, match="minimized"):
        screenshot.capture(screenshot.Window(7, 40, (0.0, 0.0, 800.0, 600.0), "Docs"))
    assert asked == []                                              # ScreenCaptureKit was never asked


def test_an_audio_start_that_times_out_stops_the_stream(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """M3: a start that answered after the 10 s timeout left a stream running with nobody to stop it."""
    from ferb.act.mac import audio
    from tests.test_audio_listen import _Obj, _fake_frameworks
    sck, cm, seen = _fake_frameworks([_Obj(processID=lambda: 77, bundleIdentifier=lambda: "com.apple.Music")])
    stream_class = sck.SCStream

    class SlowStart(stream_class):                                  # type: ignore[misc, valid-type]
        @staticmethod
        def alloc() -> Any:
            return SlowStart()

        def startCaptureWithCompletionHandler_(self, h: Any) -> None:
            pass                                                    # never answers in time

    sck.SCStream = SlowStart
    monkeypatch.setattr(audio, "_frameworks", lambda: (sck, cm))
    monkeypatch.setattr(audio, "_sink", lambda on_buffer: SimpleNamespace(on_buffer=on_buffer))
    monkeypatch.setattr(audio, "TIMEOUT_S", 0.05)
    with pytest.raises(TimeoutError):
        audio.AppAudio().start("com.apple.Music", 77, tmp_path / "x.wav")
    assert seen["stopped"] and not (tmp_path / "x.wav").exists()
    assert types                                                    # (module used by the fakes)
