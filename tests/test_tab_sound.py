"""HAND-207, "sound is part of background-first" (protocol 19): a tab FERB opens for a task is muted through the
browser's tab mute, a tab the task needs to hear (audio.listen, a recording with sound) is unmuted for it, a tab handed
to the owner gets its sound back, and the owner's own tabs are never touched; sound starting or stopping in a tab that
isn't FERB's near one of FERB's calls is a disturbance fact for the checker. The real daemon and gateway with the fake
extension (which mutes as the extension does) and fakes at the audio-capture boundary."""
import json
import time
from pathlib import Path
from typing import Any

import ferb.daemon.disturbance as disturbance
from ferb.act.listen import AudioListen
from ferb.daemon.disturbance import _Observation
from ferb.orchestrator.evidence import how_facts
from tests.browserkit import OWNER, bd, call, pair, task  # noqa: F401  (the fixture and its helpers)
from tests.chromelab import SHOP
from tests.test_audio_listen import _hearing


def _setup(d: Any, launched: Any, tmp_path: Path) -> tuple[Any, Any]:
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    g = task(d, "x", OWNER | {"audio"}, sites=frozenset({"shop.example"}))
    hearing, _, _ = _hearing(tmp_path)
    d.daemon.registry.capabilities["audio.listen"] = AudioListen(hearing, lambda identity: d.daemon.router)
    d.daemon.router.browser_pid = lambda ref: None           # the fake apps know the browser by its bundle id
    return ext, g


def _chrome_id(d: Any, tab: str) -> int:
    return int(d.daemon.router._tabs[tab].chrome_id)


def _mutes(ext: Any) -> list[dict[str, Any]]:
    cmds = [json.loads(json.loads(m)["body"]) for m in ext.wire if '"type": "cmd"' in m]
    return [c["args"] for c in cmds if c["op"] == "mute_tab"]


def test_a_ferb_tab_opens_muted_a_listen_unmutes_it_and_the_owners_tab_is_left_alone(bd: Any, tmp_path: Path) -> None:
    d, launched = bd
    ext, g = _setup(d, launched, tmp_path)
    a = call(d, g, "browser.open", url=f"{SHOP}/a")["content"]["tab"]
    ca = _chrome_id(d, a)
    assert ca in ext.muted                                   # muted before anything played
    ext.tabs[41], ext.tab_windows[41] = "https://www.youtube.com/watch?v=x", 7
    ext.muted.discard(41)
    owners = call(d, g, "browser.use_tab", tab_ref="chrome:Profile 1:41")["content"]["tab"]
    assert 41 not in ext.muted
    heard = call(d, g, "audio.listen", tab=a, seconds=0.01)
    assert not heard["is_error"], heard
    assert ca not in ext.muted and heard["content"]["tab_muted"] is False   # heard for the rest of the task
    before = len(_mutes(ext))
    assert not call(d, g, "audio.listen", tab=owners, seconds=0.01)["is_error"]
    assert 41 not in ext.muted and len(_mutes(ext)) == before             # the owner's tab: never sent a mute
    b = call(d, g, "browser.open", url=f"{SHOP}/b")["content"]["tab"]
    cb = _chrome_id(d, b)
    assert cb in ext.muted
    assert not call(d, g, "browser.keep_open", tab=b)["is_error"]
    assert cb not in ext.muted                               # handed over: the owner's, with its sound
    assert ext.op_mute_tab({"tab_id": 41, "muted": True}, "x")["reason"] == "not_ferbs"   # refused there too


def _sound_facts(d: Any) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        return list(how_facts(d.daemon.db.conn, "x")["sound_changes_in_other_tabs_near_ferb_calls"]["newest"])
    return d.run(go())


def test_sound_stopping_in_the_owners_tab_near_a_ferb_call_is_a_disturbance_fact(bd: Any, tmp_path: Path,
                                                                                 monkeypatch: Any) -> None:
    d, launched = bd
    ext, g = _setup(d, launched, tmp_path)

    async def watch() -> None:
        d.daemon.disturbance.active.setdefault("x", _Observation())
    d.run(watch())
    ext.tabs[41], ext.tab_windows[41] = "https://open.spotify.com/", 7
    ext.audible.add(41)
    call(d, g, "browser.use_tab", tab_ref="chrome:Profile 1:41")
    ext.sound(41, False)                                     # the owner's music stopped right after FERB's call
    ext.wait_for(lambda: bool(_sound_facts(d)))
    [fact] = _sound_facts(d)
    assert fact["near_calls"] == ["browser.use_tab"]
    assert fact["changed"]["audible"] == {"before": True, "after": False} and fact["changed"]["tab"]["tab_id"] == 41
    a = call(d, g, "browser.open", url=f"{SHOP}/a")["content"]["tab"]
    ext.sound(_chrome_id(d, a), True)                        # FERB's own tab: not a disturbance
    monkeypatch.setattr(disturbance, "SOUND_NEAR_S", 0.0)
    ext.sound(41, True)                                      # the owner's own play, with no FERB call near
    time.sleep(0.5)
    assert len(_sound_facts(d)) == 1
