"""FERB never speaks (VOICE-409, owner 2026-11-04): a dictated message, even one an older FERB.app sends with
`speak: true`, gets its reply in text only; nothing starts macOS's voice and no spoken-words event is published.
Hearing's technical guards: a transcription time limit, FERB's hearing spared mid-transcription, and a recording's sound
refused on an excluded site. Nothing is recorded: fake providers and fake processes stand in."""

import asyncio
import signal
import subprocess
import threading
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from ferb.cli.client import Client
from ferb.core.types import Identity, TabRef, ToolContext
from tests.conftest import RunningDaemon, done, install, sup, task
from tests.support.hearing import FakeApp, words

# ---------- FERB has no voice ----------

def _send(daemon: RunningDaemon, text: str, speak: bool | None) -> list[dict[str, Any]]:
    c = Client(daemon.sock, timeout=30)
    try:
        params: dict[str, Any] = {"session_id": "s1", "text": text}
        if speak is not None:
            params["speak"] = speak
        return list(c.stream("chat.send", params))
    finally:
        c.close()


# decision: VOICE-409, VOICE-410
def test_a_dictated_message_is_answered_in_text_and_nothing_is_spoken(daemon: RunningDaemon,
                                                                      monkeypatch: pytest.MonkeyPatch) -> None:
    """Live 2026-11-04: FERB said in the chat that it replies in text while macOS's voice read that reply aloud. Now a
    turn flagged as dictated (as an older FERB.app sends it) runs like a typed one: its reply and its task's answer come
    back as text, no process runs macOS's `say`, and no `voice.*` event is published."""
    started: list[tuple[Any, ...]] = []
    real_exec = asyncio.create_subprocess_exec

    async def spy(*argv: Any, **kw: Any) -> Any:
        started.append(argv)
        return await real_exec(*argv, **kw)
    monkeypatch.setattr(asyncio, "create_subprocess_exec", spy)
    real_popen = subprocess.Popen

    def popen(argv: Any, *a: Any, **kw: Any) -> Any:
        started.append(tuple(argv) if isinstance(argv, (list, tuple)) else (argv,))
        return real_popen(argv, *a, **kw)
    monkeypatch.setattr(subprocess, "Popen", popen)
    published: list[str] = []
    publish = daemon.daemon._publish
    daemon.daemon._publish = lambda topic, data: (published.append(topic), publish(topic, data))[1]
    install(daemon, [sup("reply", "I reply in text."), sup("task", "Writing the note.", task("write the note"))],
            {"worker": done("Wrote the note.")})
    events = _send(daemon, "do you speak back?", True)
    assert any(e["type"] == "reply" and e.get("text") == "I reply in text." for e in events)
    events = _send(daemon, "write the note", True)
    assert any(e["type"] == "outcome" for e in events)
    assert not [a for a in started if a and str(a[0]).endswith("/say")]
    assert not [t for t in published if t.startswith("voice.")]


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


# decision: DMN-401
def test_update_quit_never_terminates_the_notch(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    """FERB.app's bundle id is the running notch's too: one process scan ends only hearing instances (FERB.app's
    executable run with a hearing mode on a file in the audio folder), by SIGTERM, and never while transcribing."""
    from ferb.daemon.areas import voice
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
