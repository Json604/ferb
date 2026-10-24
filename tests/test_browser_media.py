"""L26 media control: the page's video/audio state in every snapshot, and `browser.media` playing, pausing, muting,
unmuting or seeking one element through the content script (no script evaluation, no click). The content script runs
under node with a fake DOM; the daemon side runs against a fake bridge. No browser is touched."""

import asyncio
import dataclasses
import json
import re
import subprocess
from types import SimpleNamespace
from typing import Any

import pytest

from ferb.core.types import Identity, TabRef, ToolContext
from tests.chromelab import EXT, _node, worker

# A page with one paused, muted <video>: the fake DOM content.js needs for a snapshot, then media calls on it.
CONTENT_HARNESS = r'''
const vm = require('vm'), fs = require('fs');
const listeners = {};
const video = {tagName: 'VIDEO', isConnected: true, paused: true, ended: false, currentTime: 3, duration: 30,
  muted: true, volume: 1, shadowRoot: null,
  play() { if (process.argv[2] === 'refuse') return Promise.reject(Object.assign(new Error('no gesture'), {name: 'NotAllowedError'}));
           this.paused = false; return Promise.resolve(); },     // per the HTML spec, paused clears at once
  pause() { this.paused = true; },
  addEventListener(type, fn) { (listeners[type] = listeners[type] || []).push(fn); },
  matches() { return false; }, getAttribute() { return null; }, setAttribute() {}, removeAttribute() {},
  closest() { return null; }, getBoundingClientRect() { return {x: 0, y: 0, width: 640, height: 360, right: 640, bottom: 360, left: 0, top: 0}; }};
const root = {setAttribute() {}, getAttribute() { return null; }, removeAttribute() {}, scrollTop: 0, scrollHeight: 900};
const doc = {documentElement: root, scrollingElement: root, body: {}, title: 'Reel',
  querySelectorAll: sel => sel === '*' ? [video] : [],
  createTreeWalker: () => ({nextNode: () => null}), createRange: () => ({})};
const ctx = vm.createContext({document: doc, window: {}, top: null, location: {href: 'https://www.instagram.com/reel/x'},
  crypto: globalThis.crypto, WeakRef, URL, setTimeout, innerWidth: 1280, innerHeight: 900, addEventListener() {},
  MutationObserver: class { observe() {} }, ShadowRoot: class {}, NodeFilter: {SHOW_TEXT: 4, FILTER_REJECT: 2, FILTER_ACCEPT: 1}});
ctx.top = ctx.window;
vm.runInContext(fs.readFileSync(process.argv[1], 'utf8'), ctx);
const f = ctx.window.__ferb;
(async () => {
  const fire = type => (listeners[type] || []).forEach(fn => fn());
  const before = f.snapshot(1).media;
  const played = await f.media(1, 0, 'play', null);
  if (process.argv[2] !== 'defer') { video.currentTime = 3.25; fire('timeupdate'); }   // the media really plays
  const unmuted = await f.media(1, 0, 'unmute', null);
  const sought = await f.media(1, 0, 'seek', 12.5);
  if (process.argv[2] !== 'defer') { video.currentTime = 12.75; fire('timeupdate'); }
  const stale = await f.media(0, 0, 'pause', null);
  const after = f.snapshot(2).media;
  console.log(JSON.stringify({before, played, unmuted, sought, stale, after}));
})().catch(e => { console.error(e); process.exit(1); });
'''


def _content(mode: str = "allow") -> dict[str, Any]:
    result = subprocess.run([_node(), "-e", CONTENT_HARNESS, str(EXT / "content.js"), mode],
                            capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


def test_play_on_a_paused_video_shows_playing_once_its_time_moves() -> None:
    out = _content()
    page = out["before"][0].pop("id")
    assert re.fullmatch(r"m1-[0-9a-f]{6}", page)              # the element's number, then this page's own token
    assert out["before"] == [{"index": 0, "playing": False, "paused": True, "current_time": 3,
                              "duration": 30, "muted": True, "volume": 1, "in_view": True}]
    assert out["played"]["id"] == page and out["after"][0]["id"] == page
    # play() clears `paused` at once; `playing` waits for the media's time to move (I5)
    assert out["played"]["playing"] is False and out["played"]["paused"] is False and "error" not in out["played"]
    assert out["unmuted"]["playing"] is True                      # its time moved: really playing
    assert out["after"][0]["playing"] is True                    # the next snapshot reports it playing


def test_a_play_whose_media_never_starts_never_reads_as_playing() -> None:
    """A play() the browser accepted but never started (its time stays put) is not playing, however long FERB looks:
    `!paused` alone said it was, and a listen then heard silence (codebase review I5)."""
    out = _content("defer")
    assert out["played"]["paused"] is False and out["played"]["playing"] is False
    assert out["unmuted"]["playing"] is False and out["after"][0]["playing"] is False


def test_a_seek_is_not_mistaken_for_playback() -> None:
    out = _content("defer")
    assert out["sought"]["current_time"] == 12.5 and out["sought"]["playing"] is False


def test_unmute_sets_muted_false_and_seek_moves_the_time() -> None:
    out = _content()
    assert out["unmuted"]["muted"] is False
    assert out["after"][0]["muted"] is False
    assert out["sought"]["current_time"] == 12.5


def test_media_from_an_older_snapshot_is_stale_and_a_refused_play_is_reported() -> None:
    out = _content()
    assert out["stale"] == {"error": "stale"}                    # never a different element after the page changed
    refused = _content("refuse")
    assert refused["played"]["playing"] is False                 # the page refused play() without a gesture
    assert refused["played"]["error"] == "NotAllowedError"


# A feed whose videos come and go (Instagram): the list the page shows changes between snapshots.
FEED_HARNESS = r'''
const vm = require('vm'), fs = require('fs');
const video = name => ({tagName: 'VIDEO', name, isConnected: true, paused: false, ended: false, currentTime: 1,
  duration: 30, muted: false, volume: 1, shadowRoot: null, play() { return Promise.resolve(); }, pause() {},
  addEventListener() {}, matches() { return false; }, getAttribute() { return null; }, setAttribute() {},
  removeAttribute() {}, closest() { return null; },
  getBoundingClientRect() { return {x: 0, y: 0, width: 640, height: 360, right: 640, bottom: 360, left: 0, top: 0}; }});
const [a, b, c] = [video('a'), video('b'), video('c')];
let page = [a];
const root = {setAttribute() {}, getAttribute() { return null; }, removeAttribute() {}, scrollTop: 0, scrollHeight: 900};
const doc = {documentElement: root, scrollingElement: root, body: {}, title: 'Feed',
  querySelectorAll: sel => sel === '*' ? page : [],
  createTreeWalker: () => ({nextNode: () => null}), createRange: () => ({})};
const ctx = vm.createContext({document: doc, window: {}, top: null, location: {href: 'https://www.instagram.com/'},
  crypto: globalThis.crypto, WeakRef, WeakMap, URL, setTimeout, innerWidth: 1280, innerHeight: 900,
  addEventListener() {}, MutationObserver: class { observe() {} }, ShadowRoot: class {},
  NodeFilter: {SHOW_TEXT: 4, FILTER_REJECT: 2, FILTER_ACCEPT: 1}});
ctx.top = ctx.window;
vm.runInContext(fs.readFileSync(process.argv[1], 'utf8'), ctx);
const f = ctx.window.__ferb, ids = m => m.map(x => [x.index, x.id]);
const first = ids(f.snapshot(1).media);
page = [b, a];                       // a new video loads above the first one
const second = ids(f.snapshot(2).media);
page = [c, a];                       // b scrolled away, c loaded
const third = ids(f.snapshot(3).media);
page = [b, c, a];                    // b back in the page
const fourth = ids(f.snapshot(4).media);
// the tab loads a new page: a fresh content script, whose first element is m1 again, with its own page token
page = [a];
const next = vm.createContext({...ctx, window: {}});
next.top = next.window;
vm.runInContext(fs.readFileSync(process.argv[1], 'utf8'), next);
const reloaded = ids(next.window.__ferb.snapshot(1).media);
console.log(JSON.stringify({first, second, third, fourth, reloaded}));
'''


def test_each_media_element_keeps_one_id_for_the_page_while_list_positions_shift() -> None:
    """Live 2026-10-24: a loop's probe on media.44 watched another video once the feed loaded more. An element's id
    is its own for the page's lifetime, assigned on first sight and never given to another element."""
    result = subprocess.run([_node(), "-e", FEED_HARNESS, str(EXT / "content.js")], capture_output=True, text=True,
                            timeout=10)
    assert result.returncode == 0, result.stderr
    out = json.loads(result.stdout)
    page = out["first"][0][1].split("-")[1]
    numbers = {k: [[i, mid.split("-")[0]] for i, mid in out[k]] for k in ("first", "second", "third", "fourth")}
    assert all(mid.endswith("-" + page) for k in numbers for _, mid in out[k])
    assert numbers["first"] == [[0, "m1"]]
    assert numbers["second"] == [[0, "m2"], [1, "m1"]]          # a moved from index 0 to 1, still m1
    assert numbers["third"] == [[0, "m3"], [1, "m1"]]           # c is new: m3, never b's m2
    assert numbers["fourth"] == [[0, "m2"], [1, "m3"], [2, "m1"]]
    # review I2: a new page of the tab numbers from m1 again; its token keeps an old id from matching anything there
    (_, again), = out["reloaded"]
    assert again.startswith("m1-") and again != out["first"][0][1]


def test_worker_media_command_runs_the_content_script_in_the_exact_frame() -> None:
    result = worker('''(async () => {
      const calls = [];
      inFrame = async (tab, frame, func, args) => { calls.push({tab, frame, args, func: String(func)}); return {index: 2, playing: true}; };
      const ok = await commands.media({tab_id: 4, frame_id: 7, snap: 3, index: 2, action: "play"});
      inFrame = async () => ({error: "stale"});
      let stale; try { await commands.media({tab_id: 4, frame_id: 0, snap: 3, index: 0, action: "pause"}); } catch (e) { stale = e.message; }
      return {ok, calls, stale, listed: OPS.includes("media")};
    })()''')
    assert result["ok"] == {"index": 2, "playing": True}
    call = result["calls"][0]
    assert call["tab"] == 4 and call["frame"] == 7 and call["args"] == [3, 2, "play", None]
    assert "__ferb.media" in call["func"]
    assert "older snapshot" in result["stale"]
    assert result["listed"] is True


def test_the_tabs_listing_says_whether_each_tab_plays_sound_and_is_muted() -> None:
    """L31: whether a tab makes sound is the browser's own fact (chrome.tabs `audible`, `mutedInfo.muted`): a probe on
    it needs no snapshot of the page."""
    result = worker('''(async () => {
      chrome.tabs.query = async () => [
        {id: 4, windowId: 2, url: "https://www.instagram.com/reels/x/", title: "Reel", active: false, audible: true,
         mutedInfo: {muted: false}},
        {id: 5, windowId: 2, url: "https://example.com/", title: "Quiet", active: true, mutedInfo: {muted: true}}];
      return await commands.list_tabs();
    })()''')
    tabs = {t["tab_id"]: t for t in result["tabs"]}
    assert (tabs[4]["audible"], tabs[4]["muted"]) == (True, False)
    assert (tabs[5]["audible"], tabs[5]["muted"]) == (False, True)


# ---------- the daemon side ----------

class FakeBridge:
    def __init__(self, frames: list[dict[str, Any]]) -> None:
        self.frames, self.calls = frames, []
        self.navs: dict[int, int] = {}
        self.gen, self.conn = 1, object()

    async def call(self, op: str, args: dict[str, Any], timeout: float = 30) -> Any:
        self.calls.append((op, args))
        if op == "snapshot":
            return {"frames": self.frames, "dialogs": []}
        if op == "media":
            return {"index": args["index"], "playing": args["action"] == "play", "current_time": 4.0,
                    "duration": 30.0, "muted": args["action"] != "unmute", "volume": 1.0}
        raise AssertionError(op)


def _driver(frames: list[dict[str, Any]]) -> tuple[Any, Any, TabRef]:
    from ferb.act.browser.ext_bridge import _Tab, _TabDriver
    ref = TabRef("device", "chrome", "Profile 22", 1, "t1")
    bridge = FakeBridge(frames)
    d = _TabDriver(None, lambda: [])  # type: ignore[arg-type]
    d._tabs["t1"] = _Tab(ref, bridge, 19, "task", "https://www.instagram.com/reel/x")  # type: ignore[arg-type]
    return d, bridge, ref


FRAMES = [{"frame_id": 0, "url": "https://www.instagram.com/reel/x", "title": "Reel", "text": "", "elements": [],
           "media": [{"index": 0, "id": "m7-a1b2c3", "playing": False, "current_time": 3, "duration": 30, "muted": True,
                      "volume": 1, "in_view": True}]},
          {"frame_id": 5, "url": "https://player.example/embed", "text": "", "elements": [],
           "media": [{"index": 0, "id": "m7-d4e5f6", "playing": True, "current_time": 1, "duration": None, "muted": False,
                      "volume": 1, "in_view": False}]}]


def test_snapshot_reports_every_frames_media_with_one_index_and_the_model_sees_it() -> None:
    from ferb.act.browser.snapshot import for_model
    d, _, ref = _driver(FRAMES)
    snap = asyncio.run(d.snapshot(ref))
    shown = for_model(snap)["media"]
    assert [m["index"] for m in shown] == [0, 1]
    assert shown[0]["playing"] is False and shown[0]["muted"] is True and "frame" not in shown[0]
    assert shown[1]["frame"] == 5 and shown[1]["duration"] is None


def test_the_snapshot_keys_each_frames_media_by_its_own_id_and_a_probe_reads_one_element() -> None:
    """Each frame numbers its own media: a child frame's id carries its frame ("f5:m7-d4e5f6"), as a scroller's does. A
    probe path reaches one element by id, whatever its list position."""
    from ferb.act.browser.snapshot import for_model
    from ferb.orchestrator.probes import MISSING, field_value
    d, bridge, ref = _driver(FRAMES)
    shown = for_model(asyncio.run(d.snapshot(ref)), probe=True)
    assert [m["id"] for m in shown["media"]] == ["m7-a1b2c3", "f5:m7-d4e5f6"]
    assert shown["media_by_id"] == {"m7-a1b2c3": shown["media"][0], "f5:m7-d4e5f6": shown["media"][1]}
    assert field_value(shown, "media_by_id.m7-a1b2c3.playing") is False
    assert field_value(shown, "media_by_id.f5:m7-d4e5f6.playing") is True
    assert field_value(shown, "media_by_id.m9-a1b2c3.playing") is MISSING      # gone from the page: the probe trips
    # review I2: the tab loads a new page, whose content script numbers from m1 again with its own token
    bridge.frames = [dict(FRAMES[0], media=[dict(FRAMES[0]["media"][0], id="m7-0f0f0f", playing=True)])]
    again = for_model(asyncio.run(d.snapshot(ref)), probe=True)
    assert field_value(again, "media_by_id.m7-a1b2c3.playing") is MISSING     # the old page's id matches nothing
    assert field_value(again, "media_by_id.m7-0f0f0f.playing") is True


def test_the_model_sees_one_media_list_with_ids_and_only_a_probe_read_gets_the_map() -> None:
    """Review M5: the keyed map doubled the media list in every snapshot a model read (about 8.6 KB more on an
    Instagram feed of 52 videos). The model reads each entry's id from the list; a probe's read resolves the map."""
    from ferb.act.browser.snapshot import for_model
    from ferb.act.browser.tools import OWNER_BROWSER, BrowserSnapshot
    from ferb.orchestrator.probes import field_value
    d, _, ref = _driver(FRAMES)
    snap = asyncio.run(d.snapshot(ref))
    assert "media_by_id" not in for_model(snap) and "media_by_id" not in json.loads(d._tabs["t1"].snapshot_json)
    tool = BrowserSnapshot(d, OWNER_BROWSER)
    seen = asyncio.run(tool.run(tool.Input(tab="t1"), _ctx())).content
    probed = asyncio.run(tool.run(tool.Input(tab="t1"), dataclasses.replace(_ctx(), probe=True))).content
    assert "media_by_id" not in seen and [m["id"] for m in seen["media"]] == ["m7-a1b2c3", "f5:m7-d4e5f6"]
    assert field_value(probed, "media_by_id.f5:m7-d4e5f6.playing") is True


def test_an_older_extensions_snapshot_has_no_media_ids() -> None:
    """Extension 0.15.7 and older send no id: the list is as before and media_by_id is absent, so a probe on it sees
    its field missing (refused at arming; a vanished field trips a round)."""
    from ferb.act.browser.snapshot import for_model
    old = [dict(f, media=[{k: v for k, v in m.items() if k != "id"} for m in f["media"]]) for f in FRAMES]
    d, _, ref = _driver(old)
    shown = for_model(asyncio.run(d.snapshot(ref)), probe=True)
    assert "media_by_id" not in shown and all("id" not in m for m in shown["media"])


def _ctx() -> ToolContext:
    return ToolContext("task", "w", "r", Identity.OWNER, frozenset({"browser:act", "browser:read"}), "device", None,
                       "/tmp", (), ())


def test_browser_media_tool_targets_the_frame_and_local_index_from_the_snapshot() -> None:
    from ferb.act.browser.tools import OWNER_BROWSER, BrowserMedia
    d, bridge, ref = _driver(FRAMES)
    asyncio.run(d.snapshot(ref))
    tool = BrowserMedia(d, OWNER_BROWSER)
    assert tool.name == "browser.media" and tool.scopes == {"browser:act"} and tool.on_screen is True
    # Page playback only, reversible like scroll: no ledger row, so a timed-out play stays retryable (P19 is for
    # consequential actions); on_screen marks it in the disturbance record.
    assert tool.describe(tool.Input(tab="t1", action="play")).kind == "read"
    assert not getattr(tool, "observation", False)               # the final checker never plays media
    r = asyncio.run(tool.run(tool.Input(tab="t1", action="unmute", index=1), _ctx()))
    assert not r.is_error, r.content
    op, args = bridge.calls[-1]
    assert op == "media" and args["frame_id"] == 5 and args["index"] == 0 and args["snap"] == 1
    assert args["action"] == "unmute" and r.content["state"]["muted"] is False and r.content["state"]["index"] == 1
    assert r.read_sources == (("web", "https://www.instagram.com/reel/x"),)


def test_browser_media_takes_a_media_id_from_the_latest_snapshot() -> None:
    from ferb.act.browser.tools import OWNER_BROWSER, BrowserMedia
    d, bridge, ref = _driver(FRAMES)
    asyncio.run(d.snapshot(ref))
    tool = BrowserMedia(d, OWNER_BROWSER)
    r = asyncio.run(tool.run(tool.Input(tab="t1", action="pause", media_id="f5:m7-d4e5f6"), _ctx()))
    assert not r.is_error, r.content
    op, args = bridge.calls[-1]
    assert op == "media" and args["frame_id"] == 5 and args["index"] == 0 and args["snap"] == 1
    assert r.content["state"]["index"] == 1 and r.content["state"]["id"] == "f5:m7-d4e5f6"
    r = asyncio.run(tool.run(tool.Input(tab="t1", action="pause", media_id="m9-a1b2c3"), _ctx()))
    assert r.is_error and "m9-a1b2c3" in r.content["error"]
    with pytest.raises(ValueError):
        tool.Input(tab="t1", action="pause", index=0, media_id="m7-a1b2c3")     # one way to name it


def test_browser_media_needs_an_index_when_the_page_has_several_and_seek_needs_a_time() -> None:
    from ferb.act.browser.tools import OWNER_BROWSER, BrowserMedia
    d, bridge, ref = _driver(FRAMES)
    tool = BrowserMedia(d, OWNER_BROWSER)
    r = asyncio.run(tool.run(tool.Input(tab="t1", action="play"), _ctx()))
    assert r.is_error and "snapshot" in r.content["error"]          # no snapshot yet: no index to act on
    asyncio.run(d.snapshot(ref))
    r = asyncio.run(tool.run(tool.Input(tab="t1", action="play"), _ctx()))
    assert r.is_error and "2 media elements" in r.content["error"]
    assert not any(op == "media" for op, _ in bridge.calls)
    with pytest.raises(ValueError):
        tool.Input(tab="t1", action="seek")
    one, bridge1, ref1 = _driver(FRAMES[:1])
    asyncio.run(one.snapshot(ref1))
    r = asyncio.run(BrowserMedia(one, OWNER_BROWSER).run(tool.Input(tab="t1", action="play"), _ctx()))
    assert not r.is_error and r.content["state"]["playing"] is True and bridge1.calls[-1][1]["index"] == 0


def test_media_is_registered_for_both_browser_families() -> None:
    from ferb.registry import Registry, add_identity_browser, add_owner_browsers
    r = Registry()
    drv = SimpleNamespace(secrets=SimpleNamespace(specs=lambda: []))
    add_owner_browsers(r, drv)  # type: ignore[arg-type]
    add_identity_browser(r, drv)  # type: ignore[arg-type]
    assert {"browser.media", "ferb_browser.media"} <= set(r.capabilities)
    assert r.capabilities["ferb_browser.media"].scopes == {"identity:browse"}


def test_protocol_14_adds_the_media_op_in_python_and_javascript_together() -> None:
    from ferb.act.browser import ext_protocol
    js = subprocess.run([_node(), "-e", f"const p=require({json.dumps(str(EXT / 'protocol.js'))});"
                         "console.log(JSON.stringify({v:p.FERB_PROTOCOL, media:p.OPS.includes('media')}))"],
                        capture_output=True, text=True, timeout=10)
    assert json.loads(js.stdout) == {"v": 17, "media": True}
    assert ext_protocol.PROTOCOL_VERSION == 17 and "media" in ext_protocol.Op.__args__
    assert ext_protocol.MIN_EXTENSION_VERSION == "0.15.6"            # protocol 17 (fix wave 4) ships with 0.15.6
