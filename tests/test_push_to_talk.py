"""Push-to-talk (section 20.1): the talk key's rules, never a Bluetooth headset's microphone, only the installed and
signed FERB.app may speak for the owner, and what's said runs as the owner's own chat turn with FERB's reply spoken."""

import asyncio
import json
import os
import sys
import time
from pathlib import Path
from typing import Any

import pytest

from ferb.core.errors import AuthError
from ferb.daemon.auth import Role, allowed, check_voice_peer
from ferb.daemon.server import exe_of
from ferb.voice.controller import Voice
from ferb.voice.mic import InputDevice, choose
from ferb.voice.push_to_talk import RIGHT_OPTION, TalkKey
from tests.conftest import RunningDaemon, install, raw, send, sup

OTHER_KEY = 0                                  # "a"


class FakeSpeaker:
    def __init__(self) -> None:
        self.said: list[str] = []
        self.stops = 0

    def say(self, text: str) -> None:
        self.said.append(text)

    def stop(self) -> None:
        self.stops += 1

    def wait(self) -> None:
        return None


# ---------- the talk key ----------

def test_hold_to_talk_tap_ignored_and_shortcut_discarded() -> None:
    k = TalkKey()
    assert k.key(RIGHT_OPTION, True, 0.0) == "start"
    assert k.key(RIGHT_OPTION, False, 1.2) == "send"                 # held long enough: send
    assert k.key(RIGHT_OPTION, True, 2.0) == "start"
    assert k.key(RIGHT_OPTION, False, 2.2) == "ignore"               # a tap under 0.3 s
    assert k.key(RIGHT_OPTION, True, 3.0) == "start"
    assert k.key(OTHER_KEY, True, 3.5) is None                       # Right Option + a key: a shortcut
    assert k.key(RIGHT_OPTION, False, 5.0) == "discard"
    assert k.key(OTHER_KEY, True, 6.0) is None and not k.held        # other keys alone mean nothing
    assert k.key(RIGHT_OPTION, True, 7.0) == "start"                 # the shortcut doesn't linger
    assert k.key(RIGHT_OPTION, False, 8.0) == "send"


# ---------- which microphone ----------

BUILT_IN = InputDevice("builtin", "MacBook Air Microphone", "bltn")
HEADSET = InputDevice("bt", "AirPods", "blue", default=True)
USB = InputDevice("usb", "USB Mic", "usb ", default=True)


def test_a_bluetooth_headset_mic_is_never_chosen() -> None:
    assert choose([BUILT_IN, HEADSET]) == BUILT_IN                     # default input is Bluetooth: built-in instead
    assert choose([BUILT_IN, InputDevice("le", "Buds", "blea", default=True)]) == BUILT_IN
    assert choose([HEADSET]) is None                                   # nothing allowed: nothing opened
    assert choose([HEADSET], "default") is None                        # not even when asked for the default
    assert choose([BUILT_IN, USB]) == USB                              # a wired default is used
    assert choose([BUILT_IN, USB], "builtin") == BUILT_IN


# ---------- who may speak for the owner ----------

def test_only_the_installed_signed_app_may_connect_as_voice(tmp_path: Path) -> None:
    app = tmp_path / "FERB.app" / "Contents" / "MacOS" / "FERB"
    app.parent.mkdir(parents=True)
    app.write_text("")
    me = os.getuid()
    check_voice_peer(me, me, str(app), str(app), lambda p: True)
    with pytest.raises(AuthError):
        check_voice_peer(me + 1, me, str(app), str(app), lambda p: True)         # another user
    with pytest.raises(AuthError):
        check_voice_peer(me, me, sys.executable, str(app), lambda p: True)       # any other program
    with pytest.raises(AuthError):
        check_voice_peer(me, me, None, str(app), lambda p: True)
    with pytest.raises(AuthError):
        check_voice_peer(me, me, str(app), str(app), lambda p: False)            # signature doesn't match setup
    assert not allowed(Role.VOICE, "chat.send") and not allowed(Role.VOICE, "chat.approve")
    assert not allowed(Role.VOICE, "tools.call") and allowed(Role.VOICE, "voice.transcript")


def _hello(daemon: RunningDaemon) -> dict[str, Any]:
    s, f = raw(daemon.sock)
    send(s, {"jsonrpc": "2.0", "id": 1, "method": "auth.hello", "params": {"role": "voice"}})
    out: dict[str, Any] = json.loads(f.readline())
    if "result" in out:
        send(s, {"jsonrpc": "2.0", "id": 2, "method": "chat.send", "params": {"session_id": "x", "text": "hi"}})
        out["then"] = json.loads(f.readline())
    s.close()
    return out


def test_the_daemon_checks_the_voice_peer(daemon: RunningDaemon) -> None:
    srv = daemon.daemon.server
    assert "isn't set up" in _hello(daemon)["error"]["message"]                    # push-to-talk off
    srv.voice_app = lambda: "/Users/nobody/.ferb/FERB.app/Contents/MacOS/FERB"
    srv.voice_signed = lambda p: True
    assert _hello(daemon)["error"]["message"] == "voice role is only for the installed FERB.app"   # this test isn't
    me = exe_of(os.getpid())
    srv.voice_app = lambda: me                                                    # pretend the test is FERB.app
    srv.voice_signed = lambda p: False
    assert "signature" in _hello(daemon)["error"]["message"]
    srv.voice_signed = lambda p: True
    ok = _hello(daemon)
    assert ok["result"]["role"] == "voice"
    assert "not allowed for voice" in ok["then"]["error"]["message"]               # it can't chat or act


# ---------- the daemon side ----------

def _voice(pending: list[Any] | None = None, answers: list[Any] | None = None,
           turns: list[str] | None = None) -> tuple[Voice, FakeSpeaker, list[str]]:
    speaker, launched = FakeSpeaker(), []

    async def run_turn(text: str) -> Any:
        (turns if turns is not None else []).append(text)
        yield {"type": "reply", "text": "It's 31°C in Delhi."}

    class J:
        def append(self, *a: Any) -> None:
            return None
    v = Voice(run_turn, lambda: pending or [], lambda *a: (answers if answers is not None else []).append(a),
              speaker, J(), lambda: launched.append("open"))
    return v, speaker, launched


def test_key_drives_the_app_with_an_allowed_mic() -> None:
    v, speaker, launched = _voice()
    v.on_key(RIGHT_OPTION, True, 0.0)
    assert launched == ["open"] and speaker.said == ["Voice isn't ready yet. Try again in a moment."]
    q = v.attach([{"uid": "bt", "name": "AirPods", "transport": "blue", "default": True},
                  {"uid": "builtin", "name": "MacBook Air Microphone", "transport": "bltn"}], {})
    v.on_key(RIGHT_OPTION, False, 1.0)
    v.on_key(RIGHT_OPTION, True, 2.0)
    assert speaker.stops == 2                                          # the owner talks: FERB stops speaking
    assert q.get_nowait() == {"op": "start", "device_uid": "builtin", "device": "MacBook Air Microphone"}
    v.on_key(RIGHT_OPTION, False, 3.0)
    assert q.get_nowait() == {"op": "stop", "sound": True}
    v.on_key(RIGHT_OPTION, True, 4.0)
    v.on_key(RIGHT_OPTION, False, 4.1)                                 # a tap: what was heard is dropped
    q.get_nowait()
    assert q.get_nowait() == {"op": "discard"}
    v.detach(q)
    assert v.app is None


def test_only_bluetooth_means_no_listening() -> None:
    v, speaker, _ = _voice()
    q = v.attach([{"uid": "bt", "name": "AirPods", "transport": "blue", "default": True}], {})
    v.on_key(RIGHT_OPTION, True, 0.0)
    assert q.empty() and "can't find a microphone" in speaker.said[0]


def test_spoken_answers_go_to_the_waiting_question() -> None:
    class Ap:
        def __init__(self, options: tuple[str, ...] = ()) -> None:
            self.id, self.options = "ap1", options
    answers: list[Any] = []
    v, speaker, _ = _voice([Ap()], answers)
    assert asyncio.run(v.transcript("yes go ahead"))["handled"] == "answer" and answers == [("ap1", True, None, "yes go ahead")]
    asyncio.run(v.transcript("no"))
    assert answers[-1] == ("ap1", True, None, "no")
    v2, _, _ = _voice([Ap(("Rahul Sharma", "Rahul (work)"))], answers)
    assert asyncio.run(v2.transcript("the second one"))["handled"] == "answer"
    assert answers[-1] == ("ap1", True, None, "the second one")


@pytest.mark.parametrize("words", ["Use the saved address", "not the first one, use the second", "stop that task"])
def test_voice_preserves_the_complete_owner_answer(words):
    from types import SimpleNamespace
    answers, turns = [], []
    ap = SimpleNamespace(id="ap1", kind="owner_question", options=("first", "second"))
    voice, _, _ = _voice([ap], answers, turns)
    assert asyncio.run(voice.transcript(words)) == {"handled": "answer"}
    assert answers == [("ap1", True, None, words)] and turns == []


def test_voice_does_not_publish_or_route_a_spoken_login_code():
    from types import SimpleNamespace
    answers, published = [], []
    ap = SimpleNamespace(id="otp", kind="owner_login_code", options=())
    voice, speaker, _ = _voice([ap], answers)
    voice.publish = lambda *args: published.append(args)
    assert asyncio.run(voice.transcript("123456")) == {"handled": "secure_input_required"}
    assert not answers and "123456" not in repr(published) and "123456" not in repr(speaker.said)


def test_a_voice_request_is_a_chat_turn_and_the_reply_is_spoken(daemon: RunningDaemon) -> None:
    install(daemon, [sup("reply", "It's 31 degrees in Delhi.")])
    speaker = FakeSpeaker()
    daemon.daemon.voice.speaker = speaker
    daemon.daemon._ptt = True                                          # push-to-talk on
    assert daemon.run(daemon.daemon.voice.transcript("  what's the weather   in Delhi "))["handled"] == "request"
    deadline = time.time() + 10
    while not speaker.said and time.time() < deadline:
        time.sleep(0.05)
    assert speaker.said == ["It's 31 degrees in Delhi."]                  # the owner's words in, FERB's reply out loud


def test_the_talk_key_does_nothing_while_push_to_talk_is_off(daemon: RunningDaemon) -> None:
    calls: list[Any] = []
    daemon.daemon.voice.on_key = lambda *a: calls.append(a)          # type: ignore[method-assign]
    hook = daemon.daemon.owner_input.on_key
    assert hook is not None
    hook(RIGHT_OPTION, True)
    time.sleep(0.1)
    assert calls == []
    daemon.daemon._ptt = True
    hook(RIGHT_OPTION, True)
    time.sleep(0.1)
    assert calls == [(RIGHT_OPTION, True)]


def test_ferb_voice_shows_what_was_heard_and_said(daemon: RunningDaemon) -> None:
    import io
    import threading

    from ferb.cli.client import Client
    from ferb.cli.logs import voice as voice_view
    install(daemon, [sup("reply", "It's 4:05 pm.")])
    daemon.daemon.voice.speaker = FakeSpeaker()
    out = io.StringIO()
    c = Client(daemon.sock, timeout=15)
    viewer = threading.Thread(target=voice_view, args=(c, out, 2))
    viewer.start()
    time.sleep(0.3)                                                    # subscribed
    daemon.run(daemon.daemon.voice.transcript("what's the time"))
    viewer.join(15)
    c.close()
    assert "you (voice)> what's the time\nferb> It's 4:05 pm." in out.getvalue()


def test_questions_are_shown_and_spoken_in_voice_words() -> None:
    from ferb.cli.chat import question, render
    published: list[Any] = []
    speaker = FakeSpeaker()

    async def run_turn(text: str) -> Any:
        yield {"type": "event", "topic": "approval.requested",
               "data": {"kind": "owner_question", "reason": "Which address should I use?", "options": []}}

    class J:
        def append(self, *a: Any) -> None:
            return None
    v = Voice(run_turn, lambda: [], lambda *a: None, speaker, J(), lambda: None, render=render, question=question,
              publish=lambda t, d: published.append((t, d["text"])))

    async def go() -> None:
        await v.transcript("tell Rahul I'm running late")
        await asyncio.sleep(0.1)
    asyncio.run(go())
    shown = [t for topic, t in published if topic == "voice.said"]
    assert shown == ["? Which address should I use?\n   say your answer"]
    assert speaker.said == ["Which address should I use? say your answer"]
    assert published[0] == ("voice.heard", "tell Rahul I'm running late")


def test_the_start_sound_only_for_a_real_hold_and_never_with_sounds_off() -> None:
    async def run(sounds: bool, hold_s: float) -> list[dict[str, Any]]:
        v, _, _ = _voice()
        v.sounds = lambda: sounds
        q = v.attach([{"uid": "builtin", "name": "Mic", "transport": "bltn", "default": True}], {})
        v.on_key(RIGHT_OPTION, True, 0.0)
        await asyncio.sleep(hold_s)
        v.on_key(RIGHT_OPTION, False, hold_s)
        await asyncio.sleep(0.4)
        return [q.get_nowait() for _ in range(q.qsize())]
    tap = asyncio.run(run(True, 0.1))
    assert [c["op"] for c in tap] == ["start", "discard"]               # a tap: silent
    hold = asyncio.run(run(True, 0.5))
    assert [c["op"] for c in hold] == ["start", "chime", "stop"] and hold[-1]["sound"] is True
    quiet = asyncio.run(run(False, 0.5))
    assert [c["op"] for c in quiet] == ["start", "stop"] and quiet[-1]["sound"] is False


def test_turning_push_to_talk_off_silences_replies_still_running() -> None:
    from ferb.cli.chat import render
    v, speaker, _ = _voice()
    published: list[Any] = []
    v.publish = lambda t, d: published.append(d["text"])
    v.render = render
    v.enabled = lambda: False

    async def go() -> None:
        await v.transcript("what's the time")
        await asyncio.sleep(0.05)
    asyncio.run(go())
    assert speaker.said == [] and "It's 31°C in Delhi." in published     # shown in `ferb voice`, not spoken


def test_ferb_voice_on_and_off(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    """`ferb voice off` stops push-to-talk (setting, speech, FERB.app); `on` needs `ferb setup --voice` first."""
    import io

    from ferb.cli.client import Client
    from ferb.cli.logs import voice_switch
    quit_calls: list[int] = []
    monkeypatch.setattr(daemon.daemon, "_quit_voice_app", lambda: quit_calls.append(1))
    monkeypatch.setattr(daemon.daemon, "_launch_voice_app", lambda: None)
    speaker = FakeSpeaker()
    daemon.daemon.voice.speaker = speaker
    c, out = Client(daemon.sock), io.StringIO()
    try:
        assert voice_switch(c, True, out) == 1 and "ferb setup --voice" in out.getvalue()   # not set up yet
        daemon.daemon.config.set("voice", "requirement", 'identifier "com.ferb.app"')
        assert voice_switch(c, True, out) == 0 and daemon.daemon._ptt is True
        assert voice_switch(c, False, out) == 0 and daemon.daemon._ptt is False
        assert quit_calls == [1] and speaker.stops >= 1 and "FERB.app has quit" in out.getvalue()
    finally:
        c.close()
