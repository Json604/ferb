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
SWIFT = ROOT / "macapp" / "FERB" / "main.swift"
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
    t = Transcriber(app, lambda: "identifier com.ferb.app", launch=fake, signed=lambda bundle, req: True)
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
    assert Path(c["audio_path"]).is_file() and Path(c["audio_path"]).parent == tmp_path / "workspace" / "audio"
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
    assert Path(r.content["audio_path"]).is_file()               # the sound stays in FERB's workspace
    assert fake.argv[0][6:] == ["transcribe", r.content["audio_path"], str(fake.raw_paths[0]), "en-IN"]


def test_ferb_app_transcribes_on_device_only_and_never_asks_for_permission_from_a_task() -> None:
    src = SWIFT.read_text()
    body = src[src.index("func transcribe("):src.index("// ---------- the service")]
    assert "requiresOnDeviceRecognition = true" in body
    assert body.index("supportsOnDeviceRecognition") < body.index("recognitionTask(")   # checked before any request
    assert "on_device_unavailable" in body and "requestAuthorization" not in body
    # No request anywhere may fall back to the network (rule 15): push-to-talk included.
    assert "requiresOnDeviceRecognition = rec.supportsOnDeviceRecognition" not in src
    assert src.count("requiresOnDeviceRecognition = true") >= 3          # warm-up, push-to-talk, hearing


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
    ref = TabRef("device", "chrome", "Profile 22", 1, "t3")
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
    assert fake.argv[0][:7] == ["open", "-g", "-n", "-W", str(app), "--args", "transcribe"]   # -g: never takes focus
    assert got.text == "hello there" and [w.text for w in got.words] == ["hello", "there"]
    assert got.words[1].start == 0.4 and abs(got.words[1].end - 0.7) < 1e-9
    assert not fake.raw_paths[0].exists()


def test_transcriber_refuses_a_missing_or_unsigned_ferb_app(tmp_path: Path) -> None:
    from ferb.sense.transcribe import HearingUnavailable, Transcriber
    audio = tmp_path / "a.wav"
    audio.write_bytes(b"RIFF")
    app = tmp_path / "FERB.app"
    fake = FakeApp(app)
    with pytest.raises(HearingUnavailable, match="ferb setup"):
        asyncio.run(Transcriber(app, lambda: "req", launch=fake, signed=lambda a, r: True).transcribe(audio, None))
    app.mkdir()
    with pytest.raises(HearingUnavailable, match="signature"):
        asyncio.run(Transcriber(app, lambda: None, launch=fake, signed=lambda a, r: True).transcribe(audio, None))
    with pytest.raises(HearingUnavailable, match="signature"):
        asyncio.run(Transcriber(app, lambda: "req", launch=fake, signed=lambda a, r: False).transcribe(audio, None))
    assert fake.argv == []


def test_transcriber_cancelled_kills_the_launch_and_leaves_no_raw_answer(tmp_path: Path) -> None:
    from ferb.sense.transcribe import Transcriber
    app = tmp_path / "FERB.app"
    app.mkdir()
    audio = tmp_path / "a.wav"
    audio.write_bytes(b"RIFF")
    seen: list[list[str]] = []

    async def hang(argv: list[str]) -> int:
        seen.append(argv)
        Path(argv[8]).write_text('{"text": "raw"}')
        await asyncio.sleep(3600)
        return 0

    async def go() -> None:
        job = asyncio.create_task(Transcriber(app, lambda: "r", launch=hang, signed=lambda a, r: True).transcribe(audio, None))
        while not seen:
            await asyncio.sleep(0.01)
        job.cancel()
        with pytest.raises(asyncio.CancelledError):
            await job
    asyncio.run(go())
    assert not Path(seen[0][8]).exists()


def test_real_launch_waits_for_the_app_and_kills_it_when_cancelled(tmp_path: Path, monkeypatch) -> None:
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
        CMTimeMake=lambda v, s: (v, s))
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
    seen["sink"].on_buffer(SimpleNamespace(asbd=asbd, data=samples), 1)
    seen["sink"].on_buffer(SimpleNamespace(asbd=asbd, data=b"\x00" * 64), 0)    # a video frame: ignored
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
    ref = TabRef("device", "chrome", "Profile 22", 1, "t1")

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
    assert Path(result["audio_path"]).is_file()
    receipt = (tmp_path / "reel.mp4.json").read_text()
    assert "4111" not in receipt and "[redacted]" in receipt        # the receipt on disk is redacted too
    streams = json.loads(subprocess.check_output([shutil.which("ffprobe") or "ffprobe", "-v", "error", "-show_entries",
                                                  "stream=codec_type", "-of", "json", str(tmp_path / "reel.mp4")]))
    assert {s["codec_type"] for s in streams["streams"]} == {"video", "audio"}   # the video carries the sound


def test_recording_with_audio_needs_the_audio_scope(tmp_path: Path) -> None:
    from ferb.act.browser.tools import FERB_BROWSER, BrowserRecord
    ref = TabRef("device", "chrome", "Profile 22", 1, "t1")
    driver = SimpleNamespace(owned_by=lambda tab, owner: ref, tab_url=lambda tab: "https://x.example/")
    tool = BrowserRecord(driver, FERB_BROWSER, SimpleNamespace(hearing=None))  # type: ignore[arg-type]
    ctx = ToolContext("task", "w", "r", Identity.FERB, frozenset({"identity:browse", "fs:write"}), "device", None,
                      str(tmp_path), (), ())
    r = asyncio.run(tool.run(tool.Input(tab="t1", path=str(tmp_path / "x.mp4"), with_audio=True), ctx))
    assert r.is_error and "audio" in r.content["error"]


# ---------- the worker prompt, the planner, push-to-talk ----------

def test_the_worker_prompt_says_how_to_hear_and_to_confirm_playback_first() -> None:
    words_ = " ".join((ROOT / "ferb" / "orchestrator" / "prompts" / "worker.md").read_text().split())
    assert ("Before recording or listening, confirm the media is actually playing and not muted: check the snapshot's "
            "`media` (playing, muted, volume), use browser_media to play or unmute it, and check again") in words_
    assert "audio_listen" in words_ and "the whole browser" in words_
    assert "They are silent sampled videos" not in words_


def test_the_planner_sees_hearing_when_its_tools_are_installed() -> None:
    from ferb.orchestrator.availability import build
    text = build({"audio"}, {}).text
    assert "(audio)" in text and "on this Mac" in text
    assert "hearing" in build({"audio"}, {"hearing": "no Screen Recording"}).unavailable_line()


def test_push_to_talk_without_on_device_recognition_says_so_and_never_uses_the_network() -> None:
    from ferb.daemon.main import Daemon
    said: list[str] = []
    journal: list[tuple[str, Any]] = []
    host = SimpleNamespace(voice=SimpleNamespace(_say=said.append),
                           _journal=lambda: SimpleNamespace(append=lambda kind, data: journal.append((kind, data))))
    out = asyncio.run(Daemon._voice_transcript(host, None, {"text": "", "error": "on-device speech recognition "  # type: ignore[arg-type]
                                                            "isn't available on this Mac", "on_device": False}))
    assert out == {"handled": "on_device_unavailable"}
    assert "On-device speech recognition isn't available" in said[0] and "never leaves" in said[0]
    src = SWIFT.read_text()
    listener = src[src.index("final class Listener"):src.index("// ---------- permissions")]
    assert "guard rec.supportsOnDeviceRecognition else" in listener
    assert listener.count("requiresOnDeviceRecognition = true") == 2


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
    ctx = ToolContext("task", "w", "r", Identity.OWNER, frozenset({"screen", "fs:write", "audio"}), "device", None,
                      str(tmp_path), (), ())
    r = asyncio.run(tool.run(tool.Input(app="Music", shot="s1", path=str(tmp_path / "m.mp4"), with_audio=True,
                                        language="hi-IN"), ctx))
    assert not r.is_error, r.content
    assert got == {"audio": ("com.apple.Music", 77), "language": "hi-IN"}
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
