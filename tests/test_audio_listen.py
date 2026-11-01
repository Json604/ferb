"""L26: FERB hears one app's sound during a task (ScreenCaptureKit app audio, FERB's own sound excluded), turns it
into text on this Mac only (FERB.app's Apple on-device speech recognition, never a cloud fallback), and gives the
model only the redacted transcript labelled as outside content. Fakes stand in at the ScreenCaptureKit, Speech and
launch boundaries (tests/support/hearing.py): no real sound is captured and no permission is asked."""

import asyncio
import json
import os
import shutil
import socket
import subprocess
import sys
import threading
import types
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from ferb.act.mac.apps import RunningApp
from ferb.core.types import Identity, TabRef, ToolContext
from tests.conftest import RunningDaemon, gateway_request as request, gateway_setup as setup, gateway_tool_call as call
from tests.support.hearing import FakeApp, FakeCapture, sine, wait_started, words

ROOT = Path(__file__).resolve().parents[1]
SWIFT = ROOT / "ui" / "mac" / "FERBNotch" / "Sources" / "FERBNotch" / "Hearing.swift"
STARTUP = "Here is the startup he means. It is called Ferb Labs, and they build agents."
CARD = "Pay with 4111 1111 1111 1111 today. Thanks."


def _hearing(tmp_path: Path, capture: FakeCapture | None = None, text: str = STARTUP,
             error: dict[str, Any] | None = None, redact: Any = None) -> tuple[Any, FakeApp, FakeCapture]:
    from ferb.act.listen import Hearing
    from ferb.sanitize.redact import Redactor
    from ferb.sense.transcribe import Transcriber
    app = tmp_path / "FERB.app"
    app.mkdir(exist_ok=True)
    fake = FakeApp(app, text, words(text), error=error)
    capture = capture or FakeCapture()
    t = Transcriber(app, lambda: "identifier com.ferb.FERB", launch=fake, signed=lambda bundle, req: True)
    apps = {"Music": RunningApp("com.apple.Music", "Music", 77, False),
            "com.google.Chrome": RunningApp("com.google.Chrome", "Google Chrome", 88, False),
            "pid:91": RunningApp("com.google.Chrome", "Google Chrome", 91, False)}

    def find(name: str) -> RunningApp:
        from ferb.core.errors import Refused
        if name not in apps:
            raise Refused(f"{name} isn't running (open it first)")
        return apps[name]
    h = Hearing(tmp_path / "workspace" / "audio", capture, t, redact or Redactor().text, apps=find)
    return h, fake, capture


def _ctx(tmp_path: Path, identity: Identity = Identity.OWNER) -> ToolContext:
    return ToolContext("task", "w", "r", identity, frozenset({"audio"}), "device", None, str(tmp_path / "workspace"),
                       (), ())


# ---------- audio.listen through the gateway ----------

def test_listen_returns_a_redacted_transcript_with_segments_and_the_trace_records_app_and_seconds(
        daemon: RunningDaemon, tmp_path: Path) -> None:
    from ferb.act.listen import AudioListen
    hearing, fake, capture = _hearing(tmp_path)
    cap = AudioListen(hearing, lambda identity: None)
    grant, _ = setup(daemon, {"audio"}, request("find the startup he's talking about"), [cap])
    r = call(daemon, grant, "audio.listen", app="Music", seconds=0.05)
    assert not r["is_error"], r
    c = r["content"]
    assert c["transcript"] == STARTUP and c["on_device"] is True
    assert c["app"] == "com.apple.Music" and c["seconds"] == 0.05
    assert [s["text"] for s in c["segments"]] == ["Here is the startup he means.",
                                                 "It is called Ferb Labs, and they build agents."]
    assert c["segments"][0]["start"] == 0 and c["segments"][1]["start"] > c["segments"][0]["end"] - 0.01
    # Owner decision (2026-10-14): the sound is deleted as soon as it is transcribed; only the transcript is kept.
    assert "audio_path" not in c and not list((tmp_path / "workspace" / "audio").iterdir())
    assert capture.handles[0].bundle_id == "com.apple.Music" and capture.handles[0].pid == 77
    assert capture.handles[0].stopped
    # Trust follows origin: what the app played is outside content, data rather than instructions (MP2).
    assert "external:audio:com.apple.Music" in r["footer"] and "outside content" in r["footer"]
    span = next(n.span for n in daemon.run(_spans(daemon)) if n.span.input.get("tool") == "audio.listen")
    assert span.input["args"] == {"app": "Music", "seconds": 0.05}
    assert span.output["app"] == "com.apple.Music" and span.output["seconds"] == 0.05
    assert all(not p.exists() for p in fake.raw_paths)            # the raw transcript is read once and deleted


async def _spans(d: RunningDaemon) -> list[Any]:
    return d.daemon.spans.tree("task-1")[0].children


def test_a_secret_spoken_in_the_audio_is_redacted_everywhere(daemon: RunningDaemon, tmp_path: Path) -> None:
    from ferb.act.listen import AudioListen
    hearing, fake, _ = _hearing(tmp_path, text=CARD)
    grant, _ = setup(daemon, {"audio"}, request("what does the video say"), [AudioListen(hearing, lambda i: None)])
    r = call(daemon, grant, "audio.listen", app="Music", seconds=0.01)
    assert not r["is_error"], r
    shown = json.dumps(r["content"])
    assert "4111" not in shown and "1111" not in shown and "[redacted]" in r["content"]["transcript"]
    assert r["content"]["segments"][0]["text"] == "Pay with [redacted] today."
    trace = json.dumps([n.span.output for n in daemon.run(_spans(daemon))], default=str)
    assert "4111" not in trace and "1111" not in trace


def test_the_transcript_is_redacted_at_capture_not_only_by_the_gateway(tmp_path: Path) -> None:
    hearing, _, _ = _hearing(tmp_path, text=CARD)
    out = asyncio.run(hearing.listen("task", "com.apple.Music", 77, 0.01, None))
    assert "4111" not in json.dumps(out) and "1111" not in json.dumps(out)
    # A card split over word segments never leaves a digit group in any one segment.
    assert all("1111" not in s["text"] for s in out["segments"])


def test_on_device_unavailable_is_a_clear_error_and_nothing_reaches_the_network(tmp_path: Path, monkeypatch) -> None:
    from ferb.act.listen import AudioListen

    def no_network(*a: Any, **k: Any) -> None:
        raise AssertionError("hearing must never use the network")
    monkeypatch.setattr(socket, "create_connection", no_network)
    monkeypatch.setattr(socket.socket, "connect", no_network)
    hearing, fake, _ = _hearing(tmp_path, error={"error": "on-device recognition isn't available for en-IN",
                                                 "reason": "on_device_unavailable", "locale": "en-IN"})
    tool = AudioListen(hearing, lambda i: None)
    r = asyncio.run(tool.run(tool.Input(app="Music", seconds=0.01, language="en-IN"), _ctx(tmp_path)))
    assert r.is_error
    assert "On-device speech recognition isn't available for en-IN" in r.content["error"]
    assert "never" in r.content["error"] and "cloud" in r.content["error"]
    assert not list((tmp_path / "workspace" / "audio").iterdir())   # no sound is kept, transcribed or not
    argv = fake.argv[0]
    assert argv[8] == "transcribe" and argv[9].endswith(".wav") and argv[10:] == [str(fake.raw_paths[0]), "en-IN"]


def test_ferb_app_transcribes_on_device_only_and_never_asks_for_permission_from_a_task() -> None:
    src = SWIFT.read_text()
    body = src[src.index("static func transcribe("):]
    assert "requiresOnDeviceRecognition = true" in body
    assert body.index("supportsOnDeviceRecognition") < body.index("recognitionTask(")   # checked before any request
    assert "on_device_unavailable" in body and "requestAuthorization" not in body
    # No request anywhere may fall back to the network (rule 15).
    assert "requiresOnDeviceRecognition = rec.supportsOnDeviceRecognition" not in src
    # hearing opens no microphone (push-to-talk is the app's own dictation, VOICE-403)
    assert "AVCaptureDevice" not in src and "AudioDeviceCreateIOProcID" not in src


def test_task_stop_ends_a_listen_in_progress(daemon: RunningDaemon, tmp_path: Path) -> None:
    from ferb.act.listen import AudioListen
    hearing, _, capture = _hearing(tmp_path)
    grant, _ = setup(daemon, {"audio"}, request("listen to the reel"), [AudioListen(hearing, lambda i: None)])
    fut = asyncio.run_coroutine_threadsafe(
        daemon.daemon.gateway.call(grant, "audio.listen", {"app": "Music", "seconds": 3600}), daemon.loop)
    wait_started(capture)
    daemon.call_soon(daemon.daemon.gateway.cancel_tasks, {"task-1"})
    with pytest.raises(BaseException):
        fut.result(timeout=10)
    assert capture.handles[0].stopped                                # the stream is stopped, not left running
    assert not hearing.listening


def test_task_end_ends_a_listen_in_progress(tmp_path: Path) -> None:
    hearing, fake, capture = _hearing(tmp_path)

    async def go() -> tuple[Any, Any]:
        job = asyncio.create_task(hearing.listen("task", "com.apple.Music", 77, 3600, None))
        while not capture.started.is_set():
            await asyncio.sleep(0.01)
        ended = await hearing.end_task("task")
        return ended, await job
    ended, result = asyncio.run(asyncio.wait_for(go(), 10))
    assert ended == [{"resource": "listen", "app": "com.apple.Music", "released": True}]
    assert capture.handles[0].stopped and result["error"].startswith("listening stopped: the task ended")
    assert fake.argv == []                                            # nobody is left to read a transcript


def test_a_tab_listen_hears_the_browser_app_showing_that_task_tab(tmp_path: Path) -> None:
    from ferb.act.listen import AudioListen
    hearing, _, capture = _hearing(tmp_path)
    ref = TabRef("device", "chrome", "Profile 7", 1, "t3")
    driver = SimpleNamespace(owned_by=lambda tab, owner: ref, tab_url=lambda tab: "https://www.instagram.com/reel/x")
    tool = AudioListen(hearing, lambda identity: driver if identity is Identity.FERB else None)
    assert "every tab of that browser" in tool.description and "browser_media" in tool.description
    r = asyncio.run(tool.run(tool.Input(tab="t3", seconds=0.01), _ctx(tmp_path, Identity.FERB)))
    assert not r.is_error, r.content
    assert capture.handles[0].bundle_id == "com.google.Chrome" and r.content["tab"] == "t3"
    assert r.read_sources == (("web", "https://www.instagram.com/reel/x"), ("audio", "com.google.Chrome"))
    with pytest.raises(ValueError):
        tool.Input(app="Music", tab="t3", seconds=1)
    with pytest.raises(ValueError):
        tool.Input(seconds=1)
    with pytest.raises(ValueError):
        tool.Input(app="Music", seconds=0)


def test_no_sound_is_reported_as_a_fact_without_launching_ferb_app(tmp_path: Path) -> None:
    hearing, fake, _ = _hearing(tmp_path, capture=FakeCapture(samples=b""))
    out = asyncio.run(hearing.listen("task", "com.apple.Music", 77, 0.01, None))
    assert out["transcript"] == "" and out["segments"] == [] and out["audio_s"] == 0
    assert "no sound" in out["note"] and fake.argv == []


# decision: SENSE-400
def test_listen_has_no_ferb_cap_on_seconds() -> None:
    from ferb.act.listen import AudioListen
    schema = AudioListen.Input.model_json_schema()["properties"]["seconds"]
    assert "maximum" not in schema and "exclusiveMaximum" not in schema   # the model chooses how long


# ---------- the FERB.app transcription boundary ----------

def test_transcriber_launches_ferb_app_in_the_background_and_deletes_the_raw_answer(tmp_path: Path) -> None:
    from ferb.sense.transcribe import Transcriber
    app = tmp_path / "FERB.app"
    app.mkdir()
    audio = tmp_path / "a.wav"
    audio.write_bytes(b"RIFF")
    fake = FakeApp(app, "hello there", [("hello", 0.0, 0.3), ("there", 0.4, 0.3)])
    t = Transcriber(app, lambda: "req", launch=fake, signed=lambda a, r: True)
    got = asyncio.run(t.transcribe(audio, None))
    assert fake.argv[0][:9] == ["/usr/bin/open", "-g", "-n", "-W", "--env", f"FERB_HOME={tmp_path}", str(app),
                                "--args", "transcribe"]                     # -g: never takes focus
    assert got.text == "hello there" and [w.text for w in got.words] == ["hello", "there"]
    assert got.words[1].start == 0.4 and abs(got.words[1].end - 0.7) < 1e-9
    assert not fake.raw_paths[0].exists()


def test_the_packaged_app_is_told_ferbs_home_not_its_own_folder(tmp_path: Path) -> None:
    """L26 x L27 integration: FERB.app accepts audio and answer files only in $FERB_HOME/workspace/audio, so it must be
    told the daemon's FERB home, never the folder it happens to sit in."""
    from ferb.sense.transcribe import Transcriber
    home = tmp_path / ".ferb"
    app = tmp_path / "Applications" / "FERB.app"
    app.mkdir(parents=True)
    audio = home / "workspace" / "audio" / "a.wav"
    audio.parent.mkdir(parents=True)
    audio.write_bytes(b"RIFF")
    fake = FakeApp(app, "hello", [("hello", 0.0, 0.3)], home=home)
    t = Transcriber(app, lambda: "req", launch=fake, signed=lambda a, r: True, home=home)
    asyncio.run(t.transcribe(audio, None))
    assert fake.argv[0][4:7] == ["--env", f"FERB_HOME={home}", str(app)]


# decision: VOICE-403
def test_hearing_launches_the_app_itself(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """After the merge there is no voice helper: the daemon opens FERB.app with the hearing mode as its arguments."""
    import ferb.device.install as install
    from ferb.daemon.main import Daemon
    from ferb.sense.transcribe import Transcriber
    from tests.support.app_bundle import make_ferb_app
    app = make_ferb_app(tmp_path / "Applications")
    assert install.voice_app(tmp_path / ".ferb", app) == app
    monkeypatch.setattr(install, "app_bundle", lambda prefix=None: app)
    d = Daemon(tmp_path / ".ferb", resources=False)
    t = d.hearing.transcriber
    assert t.app == app and t.home == tmp_path / ".ferb"
    audio = tmp_path / ".ferb" / "workspace" / "audio"
    audio.mkdir(parents=True)
    (audio / "a.wav").write_bytes(b"RIFF")
    fake = FakeApp(app, "hello", [("hello", 0.0, 0.3)], home=tmp_path / ".ferb")
    asyncio.run(Transcriber(t.app, lambda: "req", launch=fake, signed=lambda a, r: True, home=t.home)
                .transcribe(audio / "a.wav", None))
    assert fake.argv[0][:3] == ["/usr/bin/open", "-g", "-n"] and str(app) in fake.argv[0]
    assert fake.argv[0][fake.argv[0].index("--args") + 1] == "transcribe"


def test_transcriber_refuses_a_missing_or_unsigned_ferb_app(tmp_path: Path) -> None:
    from ferb.sense.transcribe import HearingUnavailable, Transcriber
    audio = tmp_path / "a.wav"
    audio.write_bytes(b"RIFF")
    app = tmp_path / "FERB.app"
    fake = FakeApp(app)
    with pytest.raises(HearingUnavailable):
        asyncio.run(Transcriber(app, lambda: "req", launch=fake, signed=lambda a, r: True).transcribe(audio, None))
    app.mkdir()
    with pytest.raises(HearingUnavailable, match="signature"):
        asyncio.run(Transcriber(app, lambda: None, launch=fake, signed=lambda a, r: True).transcribe(audio, None))
    with pytest.raises(HearingUnavailable, match="signature"):
        asyncio.run(Transcriber(app, lambda: "req", launch=fake, signed=lambda a, r: False).transcribe(audio, None))
    assert fake.argv == []


def test_the_hearing_status_checks_the_signature_once_per_call(tmp_path: Path) -> None:
    """Doctor's and setup's hearing probe verifies FERB.app's signature once per call (the status ask's own check), and
    an unsigned app is reported as such and never launched."""
    from ferb.daemon.areas.voice import VoiceGlue
    from ferb.sense.transcribe import Transcriber
    app = tmp_path / "FERB.app"
    app.mkdir()
    for ok in (True, False):
        checks: list[str] = []

        def signed(bundle: Path, req: str) -> bool:
            checks.append(req)
            return ok
        fake = FakeApp(app)
        glue = SimpleNamespace(voice_bundle=app, _hearing_requirement=lambda: "req", _redact_text=str,
                               hearing=SimpleNamespace(transcriber=Transcriber(app, lambda: "req", launch=fake,
                                                                               signed=signed),
                                                       folder=tmp_path / "workspace" / "audio"))
        out = asyncio.run(VoiceGlue._hearing_status(glue, True))   # type: ignore[arg-type]
        assert checks == ["req"]
        assert out["signed"] is ok and ("speech" in out) is ok and "error" not in out
        assert len(fake.argv) == (1 if ok else 0)


def test_a_cancelled_transcription_tells_ferb_app_to_stop_and_leaves_nothing_on_disk(tmp_path: Path) -> None:
    """Killing `open -W` doesn't stop a LaunchServices-launched FERB.app. The answer file is its lease: the daemon
    deletes it to say stop (FERB.app watches it and exits), and an answer written after that goes to an unlinked file."""
    from ferb.sense.transcribe import Transcriber
    app = tmp_path / "FERB.app"
    app.mkdir()
    folder = tmp_path / "audio"
    folder.mkdir()
    audio = folder / "a.wav"
    audio.write_bytes(b"RIFF")
    fake = FakeApp(app, CARD, words(CARD), late=True)

    async def go() -> None:
        job = asyncio.create_task(Transcriber(app, lambda: "r", launch=fake, signed=lambda a, r: True).transcribe(audio, None))
        while not fake.raw_paths and not job.done():
            await asyncio.sleep(0.01)
        assert not job.done(), job.exception()
        await asyncio.sleep(0.05)                                    # FERB.app holds its answer file open
        job.cancel()
        with pytest.raises(asyncio.CancelledError):
            await job
        assert not fake.raw_paths[0].exists()                        # the stop signal FERB.app watches for
    asyncio.run(go())
    fake.release.set()                                               # it answers anyway, after the stop
    assert fake.wrote.wait(5)
    left = [p.name for p in folder.iterdir() if p.name != "a.wav"]
    assert left == [] and not any("4111" in p.read_bytes().decode(errors="ignore") for p in folder.iterdir())


def test_real_launch_kills_the_open_wait_when_cancelled(tmp_path: Path, monkeypatch) -> None:
    """The `open -W` process is killed on cancel; FERB.app itself is told to stop through its answer file (above)."""
    from ferb.sense import transcribe
    ran: list[tuple[str, ...]] = []

    class Proc:
        returncode = None

        async def wait(self) -> int:
            await asyncio.sleep(3600)
            return 0

        def kill(self) -> None:
            ran.append(("killed",))

    async def spawn(*argv: str, **kw: Any) -> Proc:
        ran.append(argv)
        assert kw["stdout"] == kw["stderr"] == asyncio.subprocess.DEVNULL   # no transcript in any log
        return Proc()
    monkeypatch.setattr(asyncio, "create_subprocess_exec", spawn)

    async def go() -> None:
        job = asyncio.create_task(transcribe.launch(["open", "-g", "x"]))
        await asyncio.sleep(0.01)
        job.cancel()
        with pytest.raises(asyncio.CancelledError):
            await job
    asyncio.run(go())
    assert ran == [("open", "-g", "x"), ("killed",)]


# ---------- the ScreenCaptureKit boundary ----------

class _Obj:
    """A stand-in for one ScreenCaptureKit object: records every setter."""

    def __init__(self, **kw: Any) -> None:
        self.__dict__.update(kw)
        self.set: dict[str, Any] = {}

    def __getattr__(self, name: str) -> Any:
        if name.startswith("set") and name.endswith("_"):
            return lambda v: self.set.__setitem__(name, v)
        raise AttributeError(name)


def _fake_frameworks(apps: list[Any]) -> tuple[Any, Any, dict[str, Any]]:
    seen: dict[str, Any] = {"outputs": [], "stopped": False}
    display = _Obj(displayID=lambda: 1)
    content = _Obj(applications=lambda: apps, displays=lambda: [display])

    class Filter:
        @staticmethod
        def alloc() -> Any:
            return Filter()

        def initWithDisplay_includingApplications_exceptingWindows_(self, d: Any, a: list[Any], w: list[Any]) -> Any:
            seen["filter"] = (d, list(a), list(w))
            return self

    class Config(_Obj):
        @staticmethod
        def alloc() -> Any:
            c = Config()
            seen["config"] = c
            return c

        def init(self) -> Any:
            return self

    class Stream:
        @staticmethod
        def alloc() -> Any:
            return Stream()

        def initWithFilter_configuration_delegate_(self, f: Any, c: Any, d: Any) -> Any:
            return self

        def addStreamOutput_type_sampleHandlerQueue_error_(self, out: Any, kind: int, q: Any, e: Any) -> Any:
            seen["outputs"].append(kind)
            seen["sink"] = out
            return True, None

        def startCaptureWithCompletionHandler_(self, h: Any) -> None:
            h(None)

        def stopCaptureWithCompletionHandler_(self, h: Any) -> None:
            seen["stopped"] = True
            h(None)

    sck = types.SimpleNamespace(
        SCShareableContent=types.SimpleNamespace(
            getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler_=lambda a, b, h: h(content, None)),
        SCContentFilter=Filter, SCStreamConfiguration=Config, SCStream=Stream,
        SCStreamOutputTypeAudio=1, SCStreamOutputTypeScreen=0)
    cm = types.SimpleNamespace(
        CMSampleBufferGetFormatDescription=lambda b: b,
        CMAudioFormatDescriptionGetStreamBasicDescription=lambda f: f.asbd,
        CMSampleBufferGetDataBuffer=lambda b: b,
        CMBlockBufferGetDataLength=lambda bb: len(bb.data),
        CMBlockBufferCopyDataBytes=lambda bb, off, n, out: (0, bb.data[off:off + n]),
        CMTimeMake=lambda v, s: (v, s),
        CMSampleBufferGetPresentationTimeStamp=lambda b: b.pts,
        CMTimeGetSeconds=lambda t: t,
        CMClockGetHostTimeClock=lambda: "host clock",
        CMClockGetTime=lambda clock: 100.0)                         # the host clock when capture starts
    return sck, cm, seen


def test_capture_takes_one_apps_sound_without_ferbs_own_or_the_microphone(tmp_path: Path, monkeypatch) -> None:
    from ferb.act.mac import audio
    target = _Obj(processID=lambda: 77, bundleIdentifier=lambda: "com.apple.Music")
    other = _Obj(processID=lambda: 78, bundleIdentifier=lambda: "com.spotify.client")
    sck, cm, seen = _fake_frameworks([other, target])
    monkeypatch.setattr(audio, "_frameworks", lambda: (sck, cm))
    monkeypatch.setattr(audio, "_sink", lambda on_buffer: SimpleNamespace(on_buffer=on_buffer))
    handle = audio.AppAudio().start("com.apple.Music", 77, tmp_path / "x.wav")
    cfg = seen["config"].set
    assert cfg["setCapturesAudio_"] is True and cfg["setExcludesCurrentProcessAudio_"] is True
    assert cfg["setCaptureMicrophone_"] is False and cfg["setChannelCount_"] == 1
    assert seen["filter"][1] == [target]                      # only the target app, found by its exact process
    assert 1 in seen["outputs"]
    samples = sine(0.25)
    asbd = SimpleNamespace(mSampleRate=16000.0, mFormatFlags=1 | 8 | 32, mBitsPerChannel=32, mChannelsPerFrame=1,
                           mBytesPerFrame=4)
    seen["sink"].on_buffer(SimpleNamespace(asbd=asbd, data=samples, pts=100.0), 1)
    seen["sink"].on_buffer(SimpleNamespace(asbd=asbd, data=b"\x00" * 64, pts=100.0), 0)    # a video frame: ignored
    facts = handle.stop()
    assert seen["stopped"] and facts["frames"] == 4000 and facts["sample_rate"] == 16000
    assert abs(facts["audio_s"] - 0.25) < 1e-6 and 0.45 < facts["peak"] <= 0.5
    ffprobe = shutil.which("ffprobe")
    if ffprobe:                                                  # a real decoder reads the file FERB.app is given
        meta = json.loads(subprocess.check_output([ffprobe, "-v", "error", "-show_entries", "stream=sample_rate,channels",
                                                   "-of", "json", str(tmp_path / "x.wav")]))
        assert meta["streams"][0] == {"sample_rate": "16000", "channels": 1}


def test_capture_refuses_an_app_screencapturekit_doesnt_list(tmp_path: Path, monkeypatch) -> None:
    from ferb.act.mac import audio
    from ferb.core.errors import Refused
    sck, cm, _ = _fake_frameworks([_Obj(processID=lambda: 78, bundleIdentifier=lambda: "com.apple.Music")])
    monkeypatch.setattr(audio, "_frameworks", lambda: (sck, cm))
    with pytest.raises(Refused):
        audio.AppAudio().start("com.apple.Music", 77, tmp_path / "x.wav")   # same app, another process: not chosen
    assert not (tmp_path / "x.wav").exists()


# ---------- recordings with sound ----------

@pytest.fixture
def frames() -> list[bytes]:
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        pytest.skip("recording requires ffmpeg")
    return [subprocess.check_output([ffmpeg, "-v", "error", "-f", "lavfi", "-i", f"color={c}:s=320x180", "-frames:v",
                                     "1", "-f", "image2pipe", "-c:v", "mjpeg", "pipe:1"]) for c in ("red", "blue")]


def test_browser_record_with_audio_attaches_a_time_aligned_transcript(tmp_path: Path, frames: list[bytes]) -> None:
    from ferb.act.browser.tools import FERB_BROWSER, BrowserRecord
    from ferb.act.recording import RecordingManager
    hearing, fake, capture = _hearing(tmp_path, text=CARD)
    clock = [0.0]
    m = RecordingManager(now=lambda: clock[0], sanitize=lambda image: image, hearing=hearing)
    ref = TabRef("device", "chrome", "Profile 7", 1, "t1")

    async def shot() -> bytes:
        return frames[0]
    starting = capture.start

    def slow_start(*args: Any) -> Any:                              # the sound starts half a second into the video
        clock[0] += 0.5
        return starting(*args)
    capture.start = slow_start  # type: ignore[method-assign]
    driver = SimpleNamespace(owned_by=lambda tab, owner: ref, tab_url=lambda tab: "https://www.instagram.com/reel/x",
                             recording_source=lambda tab, owner: ({"kind": "tab", "tab": "t1"}, shot))
    tool = BrowserRecord(driver, FERB_BROWSER, m)  # type: ignore[arg-type]
    ctx = ToolContext("task", "w", "r", Identity.FERB, frozenset({"identity:browse", "fs:write", "audio"}), "device",
                      None, str(tmp_path), (), ())

    async def go() -> dict[str, Any]:
        started = await tool.run(tool.Input(tab="t1", path=str(tmp_path / "reel.mp4"), with_audio=True), ctx)
        assert not started.is_error, started.content
        clock[0] = 3.0
        return await m.stop(started.content["id"], "task")
    result = asyncio.run(go())
    assert capture.handles[0].bundle_id == "com.google.Chrome" and capture.handles[0].stopped
    assert result["capture_complete"], result
    assert result["transcript"] == "Pay with [redacted] today. Thanks."
    assert result["segments"][0]["start"] == 0.5                   # shifted onto the video's own timeline
    assert "audio_path" not in result and not list((tmp_path / "workspace" / "audio").glob("*.wav"))
    receipt = (tmp_path / "reel.mp4.json").read_text()
    assert "4111" not in receipt and "[redacted]" in receipt        # the receipt on disk is redacted too
    streams = json.loads(subprocess.check_output([shutil.which("ffprobe") or "ffprobe", "-v", "error", "-show_entries",
                                                  "stream=codec_type", "-of", "json", str(tmp_path / "reel.mp4")]))
    # Owner decision: the video keeps its frames only; the sound file goes once transcribed (no audio track).
    assert {s["codec_type"] for s in streams["streams"]} == {"video"} and result["audio_track"] is False


def test_recording_with_audio_needs_the_audio_scope(tmp_path: Path) -> None:
    from ferb.act.browser.tools import FERB_BROWSER, BrowserRecord
    ref = TabRef("device", "chrome", "Profile 7", 1, "t1")
    driver = SimpleNamespace(owned_by=lambda tab, owner: ref, tab_url=lambda tab: "https://x.example/")
    tool = BrowserRecord(driver, FERB_BROWSER, SimpleNamespace(hearing=None))  # type: ignore[arg-type]
    ctx = ToolContext("task", "w", "r", Identity.FERB, frozenset({"identity:browse", "fs:write"}), "device", None,
                      str(tmp_path), (), ())
    r = asyncio.run(tool.run(tool.Input(tab="t1", path=str(tmp_path / "x.mp4"), with_audio=True), ctx))
    assert r.is_error and "audio" in r.content["error"]


# ---------- the worker prompt, the planner, push-to-talk ----------

def test_the_planner_sees_hearing_when_its_tools_are_installed() -> None:
    from ferb.orchestrator.availability import build
    text = build({"audio"}, {}).text
    assert "(audio)" in text and "on this Mac" in text
    assert "hearing" in build({"audio"}, {"hearing": "no Screen Recording"}).unavailable_line()


def test_daemon_wires_hearing_into_task_end_and_the_registry() -> None:
    from ferb.daemon.main import Daemon
    order: list[str] = []

    async def hear(task: str) -> list[dict[str, Any]]:
        order.append("listen")
        return [{"resource": "listen", "released": True}]

    async def rec(task: str) -> list[dict[str, Any]]:
        order.append("recording")
        return []
    host = SimpleNamespace(hearing=SimpleNamespace(end_task=hear), recordings=SimpleNamespace(end_task=rec),
                           identity_browser=None, router=None, shells=None)
    assert asyncio.run(Daemon._task_end(host, "task")) == [{"resource": "listen", "released": True}]
    assert order == ["listen", "recording"]
    from ferb.registry import Registry, add_hearing
    r = Registry()
    add_hearing(r, SimpleNamespace(), lambda identity: None)
    cap = r.capabilities["audio.listen"]
    assert cap.scopes == {"audio"} and cap.identities == {Identity.OWNER, Identity.FERB} and cap.observation is True


def test_mac_record_with_audio_hears_the_pinned_windows_own_process(tmp_path: Path) -> None:
    from ferb.act.mac.tools import MacRecord
    got: dict[str, Any] = {}

    async def start(owner: str, target: dict[str, Any], path: Path, capture: Any, fps: float = 2, **kw: Any) -> dict:
        got.update(kw)
        return {"id": "rec-1"}
    recordings = SimpleNamespace(start=start, hearing=SimpleNamespace())
    driver = SimpleNamespace(recording_source=lambda app, shot: ({"kind": "window", "app": "com.apple.Music", "pid": 77,
                                                                  "window_id": 5}, None),
                             apps=SimpleNamespace(find=lambda name: None))
    tool = MacRecord(driver, recordings)  # type: ignore[arg-type]
    checked: list[tuple[Path, str]] = []
    ctx = ToolContext("task", "w", "r", Identity.OWNER, frozenset({"screen", "fs:write", "audio"}), "device", None,
                      str(tmp_path), (), (), path_closed=lambda path, what: checked.append((path, what)))
    r = asyncio.run(tool.run(tool.Input(app="Music", shot="s1", path=str(tmp_path / "m.mp4"), with_audio=True,
                                        language="hi-IN"), ctx))
    assert not r.is_error, r.content
    assert callable(got["closed"])                 # integrated with L27: the write check goes with the recording
    got["closed"](tmp_path / "m.mp4")
    assert checked == [(tmp_path / "m.mp4", "write")]                  # the protection lists' check, for a write
    assert {k: v for k, v in got.items() if k != "closed"} == {"audio": ("com.apple.Music", 77), "language": "hi-IN",
                                                               "sources": ()}
    no_scope = ToolContext("task", "w", "r", Identity.OWNER, frozenset({"screen", "fs:write"}), "device", None,
                           str(tmp_path), (), ())
    r = asyncio.run(tool.run(tool.Input(app="Music", shot="s1", path=str(tmp_path / "n.mp4"), with_audio=True), no_scope))
    assert r.is_error and "audio" in r.content["error"]


def test_a_tab_is_heard_through_its_own_browser_process_when_several_share_the_bundle(tmp_path: Path) -> None:
    from ferb.act.listen import AudioListen
    hearing, _, capture = _hearing(tmp_path)
    ref = TabRef("device", "chrome", "Default", 1, "t1")
    driver = SimpleNamespace(owned_by=lambda tab, owner: ref, tab_url=lambda tab: "https://x.example/",
                             browser_pid=lambda tab: 91)              # the native host's parent: that Chrome
    tool = AudioListen(hearing, lambda identity: driver)
    r = asyncio.run(tool.run(tool.Input(tab="t1", seconds=0.01), _ctx(tmp_path)))
    assert not r.is_error, r.content
    assert (capture.handles[0].bundle_id, capture.handles[0].pid) == ("com.google.Chrome", 91)


def test_the_browser_behind_a_native_host_connection_is_its_parent_process() -> None:
    from ferb.act.browser.ext_bridge import _browser_pid
    a, b = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    try:                                       # the peer of a socketpair is this process: its parent is the "browser"
        assert _browser_pid(SimpleNamespace(get_extra_info=lambda key: a if key == "socket" else None)) == os.getppid()
        assert _browser_pid(SimpleNamespace(get_extra_info=lambda key: None)) is None
    finally:
        a.close(); b.close()


def test_a_capture_that_cannot_start_is_a_clear_refusal_not_an_unknown_outcome(tmp_path: Path) -> None:
    from ferb.act.listen import AudioListen

    class Declined(FakeCapture):
        def start(self, bundle_id: str, pid: int, path: Path) -> Any:
            raise RuntimeError("The user declined TCCs for application, window, display capture")
    hearing, fake, _ = _hearing(tmp_path, capture=Declined())
    tool = AudioListen(hearing, lambda i: None)
    r = asyncio.run(tool.run(tool.Input(app="Music", seconds=0.01), _ctx(tmp_path)))
    assert r.is_error and "Screen Recording" in r.content["error"] and "declined" in r.content["error"]
    assert not hearing.listening and fake.argv == []


# ---------- fix round 1: the owner's decision and the reviewer's gaps ----------

def test_after_a_listen_no_audio_file_remains_and_the_transcript_is_in_the_result(tmp_path: Path) -> None:
    """Owner decision (2026-10-14): captured sound is deleted as soon as transcription finishes."""
    hearing, _, capture = _hearing(tmp_path)
    out = asyncio.run(hearing.listen("task", "com.apple.Music", 77, 0.01, None))
    assert out["transcript"] == STARTUP and out["segments"]
    assert not capture.handles[0].path.exists() and not list((tmp_path / "workspace" / "audio").iterdir())


def test_recording_stop_with_audio_labels_the_transcript_outside_content(daemon: RunningDaemon, tmp_path: Path,
                                                                         frames: list[bytes]) -> None:
    from ferb.act.recording import RecordingManager, RecordingStop
    hearing, _, _ = _hearing(tmp_path)
    m = RecordingManager(sanitize=lambda image: image, hearing=hearing)
    grant, _ = setup(daemon, {"fs:write"}, request("record the reel with its sound"), [RecordingStop(m)])

    async def shot() -> bytes:
        return frames[0]
    started = daemon.run(m.start("task-1", {"kind": "tab", "tab": "t1"}, tmp_path / "r.mp4", shot, 1,
                                 audio=("com.google.Chrome", 88), sources=(("web", "https://www.instagram.com/reel/x"),)))
    r = call(daemon, grant, "recording.stop", id=started["id"])
    assert not r["is_error"], r
    assert r["content"]["transcript"] == STARTUP
    assert "external:audio:com.google.Chrome" in r["footer"] and "external:web:www.instagram.com" in r["footer"]
    assert "outside content" in r["footer"]


def test_mac_record_with_audio_marks_its_sound_as_outside_content(tmp_path: Path) -> None:
    from ferb.act.mac.tools import MacRecord
    got: dict[str, Any] = {}

    async def start(owner: str, target: dict[str, Any], path: Path, capture: Any, fps: float = 2, **kw: Any) -> dict:
        got.update(kw)
        return {"id": "rec-1"}
    driver = SimpleNamespace(recording_source=lambda app, shot: ({"kind": "window", "app": "com.apple.Music", "pid": 77,
                                                                  "window_id": 5}, None),
                             apps=SimpleNamespace(find=lambda name: None))
    tool = MacRecord(driver, SimpleNamespace(start=start, hearing=SimpleNamespace()))  # type: ignore[arg-type]
    ctx = ToolContext("task", "w", "r", Identity.OWNER, frozenset({"screen", "fs:write", "audio"}), "device", None,
                      str(tmp_path), (), ())
    asyncio.run(tool.run(tool.Input(app="Music", shot="s1", path=str(tmp_path / "m.mp4"), with_audio=True), ctx))
    assert got["sources"] == () and got["audio"] == ("com.apple.Music", 77)   # the manager adds ('audio', bundle)


class _SlowStart(FakeCapture):
    def start(self, bundle_id: str, pid: int, path: Path) -> Any:
        import time as _t
        _t.sleep(0.3)                                    # ScreenCaptureKit's shareable content + startCapture
        return super().start(bundle_id, pid, path)


def test_a_stop_while_the_capture_is_starting_still_stops_the_stream(tmp_path: Path) -> None:
    hearing, _, capture = _hearing(tmp_path, capture=_SlowStart())

    async def go() -> list[dict[str, Any]]:
        job = asyncio.create_task(hearing.listen("task", "com.apple.Music", 77, 3600, None))
        await asyncio.sleep(0.05)
        job.cancel()                                     # task stop while the stream is still starting
        with pytest.raises(asyncio.CancelledError):
            await job
        return await hearing.end_task("task")            # the stream that was starting is stopped, not orphaned
    ended = asyncio.run(asyncio.wait_for(go(), 10))
    assert ended == [{"resource": "listen", "app": "com.apple.Music", "released": True}]
    assert len(capture.handles) == 1 and capture.handles[0].stopped and not hearing.listening
    assert not list((tmp_path / "workspace" / "audio").iterdir())


def test_a_task_ending_while_the_capture_is_starting_stops_it(tmp_path: Path) -> None:
    hearing, fake, capture = _hearing(tmp_path, capture=_SlowStart())

    async def go() -> tuple[Any, Any]:
        job = asyncio.create_task(hearing.listen("task", "com.apple.Music", 77, 3600, None))
        await asyncio.sleep(0.05)
        ended = await hearing.end_task("task")
        return ended, await job
    ended, result = asyncio.run(asyncio.wait_for(go(), 10))
    assert ended and capture.handles[0].stopped and result["error"].startswith("listening stopped")
    assert fake.argv == [] and not hearing.listening


def test_a_recording_cancelled_while_its_sound_starts_leaves_no_stream(tmp_path: Path, frames: list[bytes]) -> None:
    from ferb.act.recording import RecordingManager
    hearing, _, capture = _hearing(tmp_path, capture=_SlowStart())
    m = RecordingManager(sanitize=lambda image: image, hearing=hearing)

    async def shot() -> bytes:
        return frames[0]

    async def go() -> None:
        job = asyncio.create_task(m.start("task", {"kind": "tab", "tab": "t1"}, tmp_path / "x.mp4", shot, 1,
                                          audio=("com.google.Chrome", 88)))
        await asyncio.sleep(0.05)
        job.cancel()
        with pytest.raises(asyncio.CancelledError):
            await job
        await hearing.shutdown()                         # waits for the abandoned start, then stops it
    asyncio.run(asyncio.wait_for(go(), 10))
    assert capture.handles[0].stopped and not hearing.listening
    assert not list((tmp_path / "workspace" / "audio").iterdir())


def test_recordings_the_task_ends_are_not_transcribed_and_keep_no_sound(tmp_path: Path, frames: list[bytes]) -> None:
    from ferb.act.recording import RecordingManager
    hearing, fake, _ = _hearing(tmp_path)
    m = RecordingManager(sanitize=lambda image: image, hearing=hearing)

    async def shot() -> bytes:
        return frames[0]

    async def go() -> list[dict[str, Any]]:
        await m.start("task", {"kind": "tab", "tab": "t1"}, tmp_path / "y.mp4", shot, 1, audio=("com.google.Chrome", 88))
        return await m.end_task("task")
    result = asyncio.run(go())[0]
    assert fake.argv == [] and "transcript" not in result and "not transcribed" in result["audio_note"]
    assert not list((tmp_path / "workspace" / "audio").iterdir())


def test_capture_places_each_buffer_at_its_timestamp(tmp_path: Path, monkeypatch) -> None:
    """A late first buffer and a silent gap keep their time: the transcript's segment times stay true."""
    import array
    from ferb.act.mac import audio
    target = _Obj(processID=lambda: 77, bundleIdentifier=lambda: "com.apple.Music")
    sck, cm, seen = _fake_frameworks([target])
    monkeypatch.setattr(audio, "_frameworks", lambda: (sck, cm))
    monkeypatch.setattr(audio, "_sink", lambda on_buffer: SimpleNamespace(on_buffer=on_buffer))
    clock = [0.0]                                                       # when each buffer arrives
    handle = audio.AppAudio(now=lambda: clock[0]).start("com.apple.Music", 77, tmp_path / "t.wav")
    asbd = SimpleNamespace(mSampleRate=16000.0, mFormatFlags=1 | 8 | 32, mBitsPerChannel=32, mChannelsPerFrame=1,
                           mBytesPerFrame=4)
    clock[0] = 0.8
    seen["sink"].on_buffer(SimpleNamespace(asbd=asbd, data=sine(0.25), pts=100.5), 1)   # 0.5 s after the start
    clock[0] = 1.8
    seen["sink"].on_buffer(SimpleNamespace(asbd=asbd, data=sine(0.25), pts=101.5), 1)   # after 0.75 s of silence
    facts = handle.stop()
    assert facts["frames"] == 28000 and abs(facts["audio_s"] - 1.75) < 1e-6
    samples = array.array("f")
    samples.frombytes((tmp_path / "t.wav").read_bytes()[44:])
    assert not any(samples[:8000]) and any(samples[8000:12000])
    assert not any(samples[12000:24000]) and any(samples[24000:])


def test_hearing_sweeps_sound_and_answers_a_restart_left_behind(tmp_path: Path) -> None:
    hearing, _, _ = _hearing(tmp_path)
    folder = tmp_path / "workspace" / "audio"
    folder.mkdir(parents=True)
    for name in ("listen-1.wav", ".transcribe-ab.json", ".status-cd.json"):
        (folder / name).write_text("left")
    hearing.sweep()
    assert not list(folder.iterdir())


# ---------- hearing runs only with the owner's own FERB home (the rule's checks: T/FerbHomeRuleTests.swift, T/HearingTests.swift) ----------


def test_hearing_takes_ferb_home_only_through_the_one_rule() -> None:
    src = SWIFT.read_text()
    assert 'environment["FERB_HOME"]' not in src                            # the rule is the only way in
    run = src[src.index("static func run("):src.index("// ---------- permissions")]
    assert run.index("resolveFerbHome") < run.index('case "request-speech"')   # checked first, for every mode
    assert "home:" in src[src.index("static func inAudioFolder("):src.index("static func writeAnswer(")]


def test_the_daemon_tells_ferb_app_the_accounts_ferb_home(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """The daemon keeps passing the owner's home, which is what FERB.app's hearing accepts."""
    from ferb.daemon.main import Daemon
    from ferb.sense.transcribe import launch_args
    monkeypatch.delenv("FERB_HOME", raising=False)
    monkeypatch.setenv("HOME", str(tmp_path))
    d = Daemon(resources=False)
    t = d.hearing.transcriber
    assert t.home == tmp_path / ".ferb"
    assert launch_args(t.app, "status", "out", home=t.home)[5] == f"FERB_HOME={tmp_path / '.ferb'}"


def test_ferb_app_writes_only_into_the_daemons_answer_file_and_stops_when_it_goes() -> None:
    src = SWIFT.read_text()
    body = src[src.index("// ---------- hearing (L26)"):]
    opened = body[body.index("static func answerFile("):body.index("private static func recognizer(")]
    assert "O_CREAT" not in opened and "O_NOFOLLOW" in opened and "st_size" in opened   # never creates or replaces
    assert "workspace/audio" in opened and "realpath" in opened                       # only FERB's audio folder
    assert "makeFileSystemObjectSource" in body and ".delete" in body                 # deleting it means stop
    assert "rename(" not in body                                                      # no file appears afterwards
    assert "st_nlink == 1" in opened                                                  # no second name for it
    write = body[body.index("static func writeAnswer("):]
    write = write[:write.index("\n    }\n")]
    assert "leaseWatch" in write and ".cancel()" in write                             # unwatched before closing
    assert "setCancelHandler" in opened                                               # the watch closes its fd


def test_a_buffer_is_never_placed_after_it_arrived(tmp_path: Path, monkeypatch) -> None:
    """A presentation time on another clock can't make FERB write a long run of zeros: arrival time bounds it."""
    from ferb.act.mac import audio
    sck, cm, seen = _fake_frameworks([_Obj(processID=lambda: 77, bundleIdentifier=lambda: "com.apple.Music")])
    monkeypatch.setattr(audio, "_frameworks", lambda: (sck, cm))
    monkeypatch.setattr(audio, "_sink", lambda on_buffer: SimpleNamespace(on_buffer=on_buffer))
    clock = [0.0]
    handle = audio.AppAudio(now=lambda: clock[0]).start("com.apple.Music", 77, tmp_path / "u.wav")
    asbd = SimpleNamespace(mSampleRate=16000.0, mFormatFlags=1 | 8 | 32, mBitsPerChannel=32, mChannelsPerFrame=1,
                           mBytesPerFrame=4)
    clock[0] = 0.25
    seen["sink"].on_buffer(SimpleNamespace(asbd=asbd, data=sine(0.25), pts=900000.0), 1)
    assert handle.stop()["frames"] == 4000 + 4000                     # placed at its arrival, 0.25 s in


# ---------- fix round 2 ----------

class _SlowStop(FakeCapture):
    def start(self, bundle_id: str, pid: int, path: Path) -> Any:
        handle = super().start(bundle_id, pid, path)
        stop = handle.stop

        def slow() -> dict[str, Any]:
            import time as _t
            _t.sleep(0.3)                                # ScreenCaptureKit's stopCapture completion
            return stop()
        handle.stop = slow  # type: ignore[method-assign]
        return handle


def test_a_task_ending_while_a_finished_listen_stops_never_launches_ferb_app(tmp_path: Path) -> None:
    hearing, fake, capture = _hearing(tmp_path, capture=_SlowStop())

    async def go() -> tuple[Any, Any]:
        job = asyncio.create_task(hearing.listen("task", "com.apple.Music", 77, 0.01, None))
        await asyncio.sleep(0.1)                         # the listen's time is up; its stream is stopping
        ended = await hearing.end_task("task")
        return ended, await job
    ended, result = asyncio.run(asyncio.wait_for(go(), 10))
    assert ended and result["error"].startswith("listening stopped: the task ended")
    assert fake.argv == [] and not list((tmp_path / "workspace" / "audio").iterdir())


def test_cancelling_recording_stop_while_it_encodes_deletes_the_sound(tmp_path: Path, frames: list[bytes],
                                                                      monkeypatch) -> None:
    from ferb.act import recording
    from ferb.act.recording import RecordingManager
    hearing, fake, capture = _hearing(tmp_path)
    m = RecordingManager(sanitize=lambda image: image, hearing=hearing)
    encoding = asyncio.Event()

    async def shot() -> bytes:
        return frames[0]

    async def slow_encoder(*args: str) -> bytes:
        encoding.set()
        await asyncio.sleep(3600)
        return b""

    async def go() -> None:
        started = await m.start("task", {"kind": "tab", "tab": "t1"}, tmp_path / "z.mp4", shot, 1,
                                audio=("com.google.Chrome", 88))
        monkeypatch.setattr(recording, "command", slow_encoder)
        job = asyncio.create_task(m.stop(started["id"], "task"))
        await encoding.wait()
        assert capture.handles[0].path.exists()          # the sound, finished, waiting for the frames to encode
        job.cancel()
        with pytest.raises(asyncio.CancelledError):
            await job
    asyncio.run(asyncio.wait_for(go(), 20))
    assert not list((tmp_path / "workspace" / "audio").iterdir()) and fake.argv == []


def _open_fds() -> set[int]:
    return {int(f) for f in os.listdir("/dev/fd")}


def test_a_stop_cancelled_while_transcribing_keeps_the_published_video_and_finishes_its_receipt(
        tmp_path: Path, frames: list[bytes], monkeypatch) -> None:
    """M-1: the video was published and staging removed when the stop was cancelled during transcription; the task's
    end must not encode again from staging that is gone, and the receipt and the held folder are finished."""
    from ferb.act import recording
    from ferb.act.recording import RecordingManager
    hearing, fake, _ = _hearing(tmp_path)
    fake.late = True                                       # recognizing until released
    m = RecordingManager(sanitize=lambda image: image, hearing=hearing)
    before = _open_fds()

    async def shot() -> bytes:
        return frames[0]

    async def never(*args: str) -> bytes:
        raise AssertionError("encoded again after the stop was cancelled")

    async def go() -> dict[str, Any]:
        started = await m.start("task", {"kind": "tab", "tab": "t1"}, tmp_path / "t.mp4", shot, 1,
                                audio=("com.google.Chrome", 88))
        job = asyncio.create_task(m.stop(started["id"], "task"))
        while not fake.argv:                               # transcribing: the video is already published
            await asyncio.sleep(0.01)
        job.cancel()
        with pytest.raises(asyncio.CancelledError):
            await job
        final = m.records[started["id"]].result
        assert final is not None                           # ended once, by the cancelled stop
        monkeypatch.setattr(recording, "command", never)
        assert await m.end_task("task") == []              # nothing left to stop: never encoded again
        return final
    try:
        result = asyncio.run(asyncio.wait_for(go(), 20))
    finally:
        fake.release.set()
    assert result["capture_complete"] and result["bytes"] == (tmp_path / "t.mp4").stat().st_size, result
    assert "transcript" not in result and "cancelled" in result["audio_note"]
    receipt = json.loads((tmp_path / "t.mp4.json").read_text())
    assert receipt["released"] and receipt["capture_complete"] and "cancelled" in receipt["audio_note"]
    assert _open_fds() <= before                           # the held output folder is closed
    assert not list((tmp_path / "workspace" / "audio").glob("*.wav"))


def test_a_stop_cancelled_while_the_sound_finishes_ends_the_recording_once(tmp_path: Path, frames: list[bytes],
                                                                           monkeypatch) -> None:
    """M-1: cancelled before encoding, the recording ends there: no video is made later from what is left, its
    receipt says why, its staging and held folder are gone, and its sound is deleted."""
    from ferb.act import recording
    from ferb.act.recording import RecordingManager
    hearing, fake, capture = _hearing(tmp_path)
    m = RecordingManager(sanitize=lambda image: image, hearing=hearing)
    before = _open_fds()

    async def shot() -> bytes:
        return frames[0]

    async def never(*args: str) -> bytes:
        raise AssertionError("a video was made after the stop was cancelled")

    async def go() -> dict[str, Any]:
        finishing, release = asyncio.Event(), asyncio.Event()
        real_stop = hearing._stop

        async def slow_stop(listen: Any) -> dict[str, Any]:     # the capture takes its time to stop
            finishing.set()
            await release.wait()
            return await real_stop(listen)
        monkeypatch.setattr(hearing, "_stop", slow_stop)
        started = await m.start("task", {"kind": "tab", "tab": "t1"}, tmp_path / "f.mp4", shot, 1,
                                audio=("com.google.Chrome", 88))
        job = asyncio.create_task(m.stop(started["id"], "task"))
        await finishing.wait()
        job.cancel()
        with pytest.raises(asyncio.CancelledError):
            await job
        final = m.records[started["id"]].result
        assert final is not None
        monkeypatch.setattr(recording, "command", never)
        release.set()
        assert await m.end_task("task") == []
        await hearing.shutdown()                            # the sound's own stop finishes in the background
        return final
    result = asyncio.run(asyncio.wait_for(go(), 20))
    assert not result["capture_complete"] and "cancelled" in result["error"], result
    assert not (tmp_path / "f.mp4").exists()
    receipt = json.loads((tmp_path / "f.mp4.json").read_text())
    assert receipt["released"] and not receipt["capture_complete"] and "cancelled" in receipt["error"]
    assert not list(m._staging().iterdir()) and _open_fds() <= before
    assert not list((tmp_path / "workspace" / "audio").glob("*.wav")) and fake.argv == []


def test_long_silence_is_written_in_pieces(tmp_path: Path, monkeypatch) -> None:
    import array
    from ferb.act.mac import audio
    monkeypatch.setattr(audio, "ZEROS", b"\0" * 1024)                 # a small piece, to cross many boundaries
    w = audio.WavWriter(tmp_path / "s.wav")
    w.write(sine(0.01), 16000, at=0.0)
    w.write(sine(0.01), 16000, at=1.0)                                  # 0.99 s of silence before it
    facts = w.close()
    assert facts["frames"] == 16000 + 160
    samples = array.array("f")
    samples.frombytes((tmp_path / "s.wav").read_bytes()[44:])
    assert len(samples) == 16160 and not any(samples[160:16000]) and any(samples[16000:])


def test_capture_reads_the_format_pyobjc_really_returns(tmp_path: Path, monkeypatch) -> None:
    """Live (owner, 2026-10-14): PyObjC returns AudioStreamBasicDescription as a plain tuple in struct order, not an
    object with fields, so every buffer failed ('tuple' object has no attribute 'mFormatFlags') and nothing was heard."""
    from ferb.act.mac import audio
    target = _Obj(processID=lambda: 77, bundleIdentifier=lambda: "com.brave.Browser")
    sck, cm, seen = _fake_frameworks([target])
    monkeypatch.setattr(audio, "_frameworks", lambda: (sck, cm))
    monkeypatch.setattr(audio, "_sink", lambda on_buffer: SimpleNamespace(on_buffer=on_buffer))
    handle = audio.AppAudio().start("com.brave.Browser", 77, tmp_path / "x.wav")
    # mSampleRate, mFormatID ('lpcm'), mFormatFlags (float|packed|non-interleaved), mBytesPerPacket, mFramesPerPacket,
    # mBytesPerFrame, mChannelsPerFrame, mBitsPerChannel, mReserved — exactly as CoreMedia gave it on this Mac
    asbd = (16000.0, 1819304813, 41, 4, 1, 4, 1, 32, 0)
    seen["sink"].on_buffer(SimpleNamespace(asbd=asbd, data=sine(0.25), pts=100.0), 1)
    facts = handle.stop()
    assert "error" not in facts, facts
    assert facts["sample_rate"] == 16000 and abs(facts["audio_s"] - 0.25) < 1e-6 and facts["peak"] > 0.4


def test_a_capture_error_is_never_told_as_paused_or_muted_media(tmp_path: Path) -> None:
    """A capture that failed on FERB's side says so; it never sends the model off to unmute or replay the media."""
    import asyncio
    from ferb.act import listen as L
    hearing = L.Hearing.__new__(L.Hearing)
    out = asyncio.run(L.Hearing.transcript(hearing, tmp_path / "none.wav",
                                           {"audio_s": 0.0, "peak": 0.0, "error": "AttributeError: boom"}, "en-US"))
    assert out["transcript"] == "" and "muted" not in out["note"] and "paused" not in out["note"]
    assert "FERB" in out["note"]


def test_the_tool_never_offers_a_language_as_on_device_without_this_mac_saying_so() -> None:
    """M2: on the owner's Mac hi-IN has no on-device recognizer (supportsOnDeviceRecognition false); the tool text
    offered it as an example. It now says only this Mac's on-device languages work and that the result says when not."""
    from ferb.act.listen import AudioListen
    text = AudioListen.description
    assert "hi-IN" not in text
    assert "on-device" in text and "says so" in text
