"""Step 11 (VOICE-401-3): FERB speaks a voice turn's replies through the configured speech provider,
from the turn's structured events, through one shared path that redacts first; a new turn or a stop interrupts it.
Hearing's technical guards: a transcription time limit, FERB's hearing spared mid-transcription, and a recording's sound
refused on an excluded site. Nothing is spoken or recorded: fake providers and fake processes stand in."""

import asyncio
import signal
import subprocess
import sys
import threading
import time
import types
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from ferb.cli.client import Client
from ferb.core.types import Identity, TabRef, ToolContext
from ferb.voice import speak
from tests.conftest import RunningDaemon, done, install, sup, task
from tests.support.hearing import FakeApp, words

CARD = "Your card is 4111 1111 1111 1111."


class Cfg:
    def __init__(self, **voice: Any) -> None:
        self.voice = voice

    def get(self, section: str, key: str, default: Any = None) -> Any:
        assert section == "voice"
        return self.voice.get(key, default)


class Blocking:
    """A provider whose utterance lasts until released; records what it began and whether it was cut off."""

    def __init__(self) -> None:
        self.began: list[str] = []
        self.cut: list[str] = []
        self.release = asyncio.Event()

    async def say(self, text: str) -> None:
        self.began.append(text)
        try:
            await self.release.wait()
        except asyncio.CancelledError:
            self.cut.append(text)
            raise


# ---------- the speaker factory ----------

# decision: VOICE-401, VOICE-407
def test_the_factory_builds_the_configured_provider_with_its_voice_and_rate() -> None:
    built = speak.build(Cfg())
    assert isinstance(built, speak.MacVoice) and built.argv() == ["/usr/bin/say"]     # the system voice, its own rate
    built = speak.build(Cfg(tts="macos", voice="Daniel (Enhanced)", rate=190))
    assert isinstance(built, speak.MacVoice) and built.argv() == ["/usr/bin/say", "-v", "Daniel (Enhanced)", "-r", "190"]


# decision: VOICE-407, VOICE-408
@pytest.mark.parametrize("voice", [
    {"tts": "kokoro"}, {"tts": ""}, {"tts": 3},
    {"rate": 0}, {"rate": -10}, {"rate": "fast"}, {"rate": True}, {"rate": float("inf")},
    {"voice": ""}, {"voice": "-o /tmp/x.aiff"}, {"voice": "Alex\nZoe"}, {"voice": 7},
])
def test_the_factory_refuses_a_value_it_cannot_use(voice: dict[str, Any]) -> None:
    with pytest.raises(speak.VoiceConfigError):
        speak.build(Cfg(**voice))


# decision: VOICE-401, VOICE-407
def test_a_registered_provider_is_a_drop_in(monkeypatch: pytest.MonkeyPatch) -> None:
    made: list[speak.VoiceSettings] = []

    def hosted(s: speak.VoiceSettings) -> Any:
        made.append(s)
        return Blocking()
    monkeypatch.setitem(speak.PROVIDERS, "hosted", speak.Provider("hosted", hosted))
    assert isinstance(speak.build(Cfg(tts="hosted", voice="Ava", rate=1.5)), Blocking)
    assert made == [speak.VoiceSettings("hosted", "Ava", 1.5)]


# decision: VOICE-404, VOICE-407
def test_the_macs_voice_reads_the_text_from_stdin_and_is_killed_when_interrupted() -> None:
    runs: list[tuple[tuple[str, ...], bytes | None]] = []

    class Proc:
        def __init__(self, argv: tuple[str, ...], hold: bool) -> None:
            self.argv, self.hold, self.returncode, self.killed = argv, hold, None, False

        async def communicate(self, data: bytes) -> tuple[bytes, bytes]:
            runs.append((self.argv, data))
            if self.hold:
                await asyncio.sleep(3600)
            self.returncode = 0
            return b"", b""

        def kill(self) -> None:
            self.killed = True
    procs: list[Proc] = []

    async def run(*argv: str, **kwargs: Any) -> Proc:
        procs.append(Proc(argv, hold=len(procs) == 1))
        return procs[-1]

    async def go() -> None:
        voice = speak.MacVoice("Samantha", 200, run=run)
        await voice.say("Hello there.")
        job = asyncio.ensure_future(voice.say("A long answer"))
        await asyncio.sleep(0.01)
        job.cancel()
        with pytest.raises(asyncio.CancelledError):
            await job
    asyncio.run(go())
    assert runs[0] == (("/usr/bin/say", "-v", "Samantha", "-r", "200"), b"Hello there.")   # never on argv
    assert not procs[0].killed and procs[1].killed


# ---------- the shared speech path ----------

# decision: VOICE-406
def test_every_provider_gets_redacted_text_only(monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.sanitize.redact import Redactor
    got = Blocking()
    got.release.set()
    monkeypatch.setitem(speak.PROVIDERS, "hosted", speak.Provider("hosted", lambda s: got))
    said: list[str] = []

    async def go() -> None:
        speech = speak.Speech(lambda: speak.build(Cfg(tts="hosted")), Redactor().text, said=said.append)
        speech.say(CARD)
        await asyncio.wait_for(_idle(speech), 5)
    asyncio.run(go())
    assert got.began and all("4111" not in t and "1111" not in t for t in got.began + said)


# decision: VOICE-406
def test_a_saved_value_with_markdown_marks_is_redacted_before_speech_reshapes_it(
        monkeypatch: pytest.MonkeyPatch) -> None:
    """Past bug (12e review): speech stripped `**`, `#`, `$`, `{}`, `~` first and redacted after, so a saved value
    holding them no longer matched and its letters were spoken and published in `voice.said`."""
    from ferb.sanitize.redact import Redactor
    got = Blocking()
    got.release.set()
    monkeypatch.setitem(speak.PROVIDERS, "hosted", speak.Provider("hosted", lambda s: got))
    redactor, said = Redactor(), []
    redactor.add_secret_value("pa**ss#w0rd${k}~zq")

    async def go() -> None:
        speech = speak.Speech(lambda: speak.build(Cfg(tts="hosted")), redactor.text, said=said.append)
        speech.say("Your code is pa**ss#w0rd${k}~zq, ready.")
        await asyncio.wait_for(_idle(speech), 5)
    asyncio.run(go())
    assert got.began and all("w0rd" not in t and "zq" not in t for t in got.began + said), got.began


async def _idle(speech: speak.Speech) -> None:
    while speech.speaking:
        await asyncio.sleep(0.01)


# decision: VOICE-404
def test_utterances_play_in_order_and_an_interrupt_cuts_the_current_one_and_drops_the_rest() -> None:
    provider = Blocking()

    async def go() -> None:
        speech = speak.Speech(lambda: provider, lambda t: t)
        speech.say("First.")
        speech.say("Second.")
        await asyncio.sleep(0.01)
        assert provider.began == ["First."]                     # one at a time, in order
        speech.interrupt()
        await asyncio.sleep(0.01)
        assert provider.cut == ["First."] and not speech.speaking
        provider.release.set()
        speech.say("Third.")
        await asyncio.wait_for(_idle(speech), 5)
    asyncio.run(go())
    assert provider.began == ["First.", "Third."]                # "Second." was dropped


# decision: VOICE-407
def test_a_bad_setting_is_reported_and_nothing_is_spoken() -> None:
    failed: list[str] = []

    async def go() -> None:
        speech = speak.Speech(lambda: speak.build(Cfg(tts="nope")), lambda t: t, failed=failed.append)
        speech.say("Hello.")
        speech.say("Again.")
        await asyncio.wait_for(_idle(speech), 5)
    asyncio.run(go())
    assert len(failed) == 1 and "VoiceConfigError" in failed[0]


# ---------- voice turns through the real daemon ----------

def _send(daemon: RunningDaemon, text: str, speak_: bool | None, session: str = "s1") -> list[dict[str, Any]]:
    c = Client(daemon.sock, timeout=30)
    try:
        params: dict[str, Any] = {"session_id": session, "text": text}
        if speak_ is not None:
            params["speak"] = speak_
        return list(c.stream("chat.send", params))
    finally:
        c.close()


def _until(check: Any, timeout: float = 10) -> None:
    end = time.monotonic() + timeout
    while not check():
        assert time.monotonic() < end, "timed out"
        time.sleep(0.02)


# decision: VOICE-405, VOICE-406
def test_a_voice_turns_reply_is_spoken_from_its_reply_event(daemon: RunningDaemon) -> None:
    install(daemon, [sup("reply", "**It's** 5 s past noon 🙂."), sup("reply", "Typed answer.")])
    published: list[tuple[str, dict[str, Any]]] = []
    publish = daemon.daemon._publish
    daemon.daemon._publish = lambda topic, data: (published.append((topic, data)), publish(topic, data))[1]
    events = _send(daemon, "what time is it", True)
    assert any(e["type"] == "reply" for e in events)
    _until(lambda: daemon.speaker.said)
    assert daemon.speaker.said == ["It's 5 seconds past noon ."]
    assert [d["text"] for t, d in published if t == "voice.said"] == daemon.speaker.said
    _send(daemon, "and now", None)                               # a typed turn isn't spoken
    time.sleep(0.2)
    assert daemon.speaker.said == ["It's 5 seconds past noon ."]


# decision: VOICE-405
def test_a_voice_turns_task_answer_is_spoken_without_codes_lines(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "Writing the note.", task("write the note"))], {"worker": done("Wrote the note.")})
    events = _send(daemon, "write the note", True)
    outcome = next(e for e in events if e["type"] == "outcome")
    _until(lambda: len(daemon.speaker.said) >= 2)
    assert daemon.speaker.said[0] == "Writing the note." and "Wrote the note." in daemon.speaker.said[-1]
    for line in outcome.get("lines") or []:
        assert line not in daemon.speaker.said[-1]


def test_speak_must_be_a_boolean(daemon: RunningDaemon) -> None:
    from ferb.cli.client import RpcError
    install(daemon, [sup("reply", "Hi.")])
    with pytest.raises(RpcError):
        _send(daemon, "hi", "yes")  # type: ignore[arg-type]


# decision: VOICE-404
def test_a_stop_or_a_new_turn_interrupts_the_speech(daemon: RunningDaemon) -> None:
    provider = Blocking()
    daemon.daemon.build_speaker = lambda: provider
    install(daemon, [sup("reply", "A long spoken answer."), sup("reply", "Second."), sup("reply", "Third.")])
    _send(daemon, "tell me", True)
    _until(lambda: provider.began == ["A long spoken answer."])
    c = Client(daemon.sock)
    try:
        c.call("task.stop", {"task_ids": ["nothing-running"]})
    finally:
        c.close()
    _until(lambda: provider.cut == ["A long spoken answer."])
    _send(daemon, "again", True)
    _until(lambda: provider.began[-1:] == ["Second."])
    _send(daemon, "typed now", None)                              # any new message cuts FERB off
    _until(lambda: provider.cut == ["A long spoken answer.", "Second."])
    assert provider.began == ["A long spoken answer.", "Second."]


# decision: VOICE-404
def test_the_global_stop_interrupts_the_speech(daemon: RunningDaemon) -> None:
    provider = Blocking()
    daemon.daemon.build_speaker = lambda: provider
    install(daemon, [sup("reply", "Talking.")])
    _send(daemon, "talk", True)
    _until(lambda: provider.began == ["Talking."])
    c = Client(daemon.sock)
    try:
        c.call("stop.engage", {})
        _until(lambda: provider.cut == ["Talking."])
        c.call("stop.lift", {})
    finally:
        c.close()


# ---------- hearing ----------

def _app(tmp_path: Path) -> tuple[Path, Path]:
    app = tmp_path / "FERB.app"
    app.mkdir()
    folder = tmp_path / "audio"
    folder.mkdir()
    audio = folder / "a.wav"
    audio.write_bytes(b"RIFF")
    return app, audio


# decision: SENSE-400
def test_a_transcription_past_its_time_limit_stops_hearing_and_says_so(tmp_path: Path,
                                                                          monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.sense import transcribe
    from ferb.sense.transcribe import HearingUnavailable, Transcriber
    monkeypatch.setattr(transcribe, "LIMIT_S", 0.1)
    monkeypatch.setattr(transcribe, "PER_AUDIO_S", 0.01)
    app, audio = _app(tmp_path)
    fake = FakeApp(app, "hello there", words("hello there"))

    async def slow(argv: list[str]) -> int:                         # recognition takes 0.3 s
        await asyncio.sleep(0.3)
        return await fake(argv)
    t = Transcriber(app, lambda: "req", launch=slow, signed=lambda a, r: True)
    with pytest.raises(HearingUnavailable, match="0 s|didn't finish"):
        asyncio.run(t.transcribe(audio, None, audio_s=0))
    assert not t.busy and [p.name for p in audio.parent.iterdir()] == ["a.wav"]   # the lease went: hearing stops
    got = asyncio.run(t.transcribe(audio, None, audio_s=100))        # a long sound earns a longer limit
    assert got.text == "hello there" and not t.busy


# decision: SENSE-400
def test_the_transcript_alignment_runs_off_the_event_loop(tmp_path: Path) -> None:
    from tests.test_audio_listen import _hearing
    hearing, _, _ = _hearing(tmp_path)
    loop_thread: list[int] = []
    threads: list[int] = []
    redact = hearing.redact

    def tracking(text: str) -> str:
        threads.append(threading.get_ident())
        return redact(text)
    hearing.redact = tracking

    async def go() -> dict[str, Any]:
        loop_thread.append(threading.get_ident())
        return await hearing.listen("task", "com.apple.Music", 77, 0.01, None)
    out = asyncio.run(go())
    assert out["transcript"] and threads and loop_thread[0] not in threads


def test_update_quit_never_terminates_the_notch(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    """FERB.app's bundle id is the running notch's too: one process scan ends only hearing instances (FERB.app's
    executable run with a hearing mode on a file in the audio folder), by SIGTERM, and never while transcribing."""
    import ferb.daemon.areas.voice as voice
    from ferb.sense.transcribe import hearing_pids
    killed: list[tuple[int, int]] = []
    monkeypatch.setattr(voice.os, "kill", lambda pid, sig: killed.append((pid, sig)))
    d = daemon.daemon
    app, folder = d.hearing.transcriber.app, d.hearing.folder
    exe = f"{app}/Contents/MacOS/FERBNotch"
    table = "\n".join([f"  100 {exe}", f"  101 {exe} --service",
                       f"  200 {exe} transcribe {folder}/a.wav {folder}/.transcribe-1.json",
                       f"  201 {exe} status {folder}/.status-2.json",
                       f"  202 {exe} request-speech",
                       f"  203 {exe} transcribe /elsewhere/a.wav /elsewhere/.t.json",
                       f"  204 /usr/bin/other transcribe {folder}/a.wav"])
    from ferb.sense import transcribe

    def table_answer(code: int) -> Any:                                   # only hearing_pids sees the fake table
        return SimpleNamespace(run=lambda *a, **k: SimpleNamespace(returncode=code, stdout=table),
                               DEVNULL=subprocess.DEVNULL, SubprocessError=subprocess.SubprocessError)
    monkeypatch.setattr(transcribe, "subprocess", table_answer(0))
    assert hearing_pids(app, folder) == [200, 201]
    t = d.hearing.transcriber
    d._quit_voice_app()
    assert killed == [(200, signal.SIGTERM), (201, signal.SIGTERM)]       # not the notch (100, 101), not request-speech
    killed.clear()
    t.in_flight += 1
    try:
        d._quit_voice_app()
        assert killed == []                                               # transcribing: nothing is ended
        assert [r["goal"] for r in d._update_blockers()] == ["turning captured sound into text"]
    finally:
        t.in_flight -= 1
    assert d._update_blockers() == []
    monkeypatch.setattr(transcribe, "subprocess", table_answer(1))
    assert hearing_pids(app, folder) == []                                # an unreadable table finds nothing


def test_a_recording_with_sound_refuses_an_excluded_site(tmp_path: Path) -> None:
    from ferb.act.browser.tools import FERB_BROWSER, BrowserRecord
    started: list[Any] = []
    hearing = SimpleNamespace(excluded_host=lambda host: host == "bank.example",
                              resolve=lambda name: ("com.google.Chrome", 88))
    recordings = SimpleNamespace(hearing=hearing, start=lambda *a, **k: started.append(a))
    ref = TabRef("device", "chrome", "Profile 7", 1, "t1")
    driver = SimpleNamespace(owned_by=lambda tab, owner: ref, tab_url=lambda tab: "https://bank.example/statement",
                             recording_source=lambda tab, owner: ({"kind": "tab"}, None))
    tool = BrowserRecord(driver, FERB_BROWSER, recordings)  # type: ignore[arg-type]
    ctx = ToolContext("task", "w", "r", Identity.FERB, frozenset({"identity:browse", "fs:write", "audio"}), "device",
                      None, str(tmp_path), (), ())
    r = asyncio.run(tool.run(tool.Input(tab="t1", path=str(tmp_path / "x.mp4"), with_audio=True), ctx))
    assert r.is_error and "exclusion" in r.content["error"] and started == []


# decision: VOICE-405
def test_codes_own_lines_after_an_answer_are_not_read_out() -> None:
    from ferb.daemon.areas.voice import answer_words
    lines = ["Sites contacted: example.com", "Left open: Notes"]
    assert answer_words({"type": "outcome", "answer": "Booked it.\n\n" + "\n".join(lines), "lines": lines}) == "Booked it."
    assert answer_words({"type": "outcome", "answer": "A: done\n\n1 of 1 done\n" + lines[0],
                         "card": {"lines": lines[:1]}}) == "A: done\n\n1 of 1 done"
    assert answer_words({"type": "plan", "text": "On it.\n" + lines[1], "lines": lines[1:]}) == "On it."
    assert answer_words({"type": "plan", "text": "On it.", "resumed": True}) == ""
    assert answer_words({"type": "subtask", "summary": "internal"}) == ""
