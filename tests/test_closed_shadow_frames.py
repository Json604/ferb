"""Live 2026-10-26 (task 490777c8): Cloudflare's "Verify you are human" checkbox failed with
pointer_did_not_reach_target_frame, reported as an unknown outcome. Its frame sits inside a closed shadow root:
`point` descended only open roots, so the screenshot point named the wrapper, and the snapshot's frame list (which links
a child frame to its element) never saw the frame, so FERB aimed at the wrapper's frame while the move landed in the
checkbox's. content.js now reaches closed roots (`chrome.dom.openOrClosedShadowRoot`, as its secret checks already did),
and a press that never started is a known failure. The extension's own code, run in node over a fake DOM."""

import json
import subprocess
from pathlib import Path
from typing import Any

from tests.chromelab import _node

EXT = Path(__file__).parents[1] / "extensions" / "chromium"

FAKE_DOM = r"""
const mk = (tag, kids = [], attrs = {}) => ({tagName: tag, kids, attrs, shadowRoot: null, closedRoot: null,
  getAttribute(k) { return k in this.attrs ? this.attrs[k] : null; }, matches: () => tag === "INPUT"});
const all = (n) => { const out = []; const walk = (m) => { for (const k of m.kids) { out.push(k); walk(k); } }; walk(n); return out; };
const rootOf = (kids, at = null) => ({kids, elementFromPoint: () => at, querySelectorAll: function () { return all(this); },
  querySelector: function (sel) { const t = /data-ferb-ax="([^"]+)"/.exec(sel)[1]; return all(this).find((e) => e.attrs["data-ferb-ax"] === t) || null; }});
const chrome = {dom: {openOrClosedShadowRoot: (el) => el.closedRoot || el.shadowRoot}};
// Cloudflare's widget: a wrapper whose closed shadow root holds the checkbox's iframe
const frame = mk("IFRAME", [], {"data-ferb-ax": "cf-frame"});
const wrapper = mk("DIV");
wrapper.closedRoot = rootOf([frame], frame);
const document = rootOf([mk("H1"), wrapper], wrapper);
"""


def _content(*names: str) -> str:
    """The named top-level functions of content.js, and its shadowOf and eachElement helpers."""
    src = (EXT / "content.js").read_text()
    shadow = src[src.index("  const shadowOf = "):src.index("  function eachElement(")]
    each = src[src.index("  function eachElement("):src.index("  const windowOf = ")]
    parts = [shadow, each]
    for name in names:
        start = src.index(f"  function {name}(")
        end = src.index("\n  function ", start + 1)
        parts.append(src[start:end])
    return "\n".join(parts)


def _run(script: str) -> Any:
    ran = subprocess.run([_node(), "-e", script], capture_output=True, text=True, timeout=20)
    assert ran.returncode == 0, ran.stderr
    return json.loads(ran.stdout)


def test_a_screenshot_point_over_a_closed_shadow_root_reaches_the_frame_inside() -> None:
    script = FAKE_DOM + r"""
let snapGen = 1; const pinCache = new Map(), boxCache = new Map(), frameCache = new Map();
const innerWidth = 1000, innerHeight = 800, visible = () => true, els = new Map(), SEL = "*", admits = () => false;
const describe = (el, id) => ({id, tag: el.tagName});
""" + _content("point") + r"""
console.log(JSON.stringify(point(1, 300, 300)));
"""
    assert _run(script) == {"frame": "cf-frame"}         # before: {"element": {"tag": "DIV"}}, the wrapper


def test_the_snapshot_lists_a_frame_inside_a_closed_shadow_root() -> None:
    script = FAKE_DOM + _content("frameElements") + r"""
console.log(JSON.stringify(frameElements().map((f) => f.attrs["data-ferb-ax"])));
"""
    assert _run(script) == ["cf-frame"]


def test_an_accessibility_target_inside_a_closed_shadow_root_is_adopted() -> None:
    """CDP stamps the node it sees through every shadow root; the content script must find it there too (the snapshot
    said "AX target has no accessible DOM binding" four times on Cloudflare's page)."""
    script = FAKE_DOM + r"""
const box = mk("INPUT", [], {"data-ferb-ax": "cf-box"});
frame.kids.push(mk("DIV")); const inner = mk("DIV"); inner.closedRoot = rootOf([box]); wrapper.closedRoot.kids.push(inner);
""" + _content("findBound") + r"""
console.log(JSON.stringify([findBound(document, "cf-box") === box, findBound(document, "missing")]));
"""
    assert _run(script) == [True, None]


def test_snapshot_candidates_include_controls_inside_closed_shadow_roots() -> None:
    script = FAKE_DOM + r"""
const SEL = "*", admits = () => true; const box = mk("INPUT", [], {id: "box"});
const inner = mk("DIV"); inner.closedRoot = rootOf([box]); wrapper.closedRoot.kids.push(inner);
""" + _content("collect") + r"""
const out = []; collect(document, out); console.log(JSON.stringify(out.filter((e) => e.tagName === "INPUT").length));
"""
    assert _run(script) == 1


def test_a_press_that_never_started_is_a_known_failure_with_nothing_pressed() -> None:
    """The move never reached the target's frame: only moves were sent, so the click is a known failure (no P19
    unknown that stops the task), and no press or release went out."""
    script = (EXT / "pointer.js").read_text() + r"""
const sent = [];
const point = {x: 5, y: 5, frame_id: 3, frame_x: 5, frame_y: 5, element: {}};
const st = {visual: {points: new Map([["p1", point]]), at: Date.now()}, gesture: null};
async function attach() { return st; }
async function checkVisual() {}
async function checkPoint() { return point; }
async function inFrame(tab, frame, fn) { return fn.toString().includes("pointerSeen") ? [] : true; }
async function mouse(tab, type) { sent.push(type); }
async function sleep() {}
exactPointer({tab_id: 1, point: "p1", kind: "click"}, "c1").then((r) => console.log(JSON.stringify({r, sent})));
"""
    out = _run(script)
    assert out["r"]["ok"] is False and out["r"]["reason"] == "pointer_did_not_reach_target_frame"
    assert "nothing was pressed" in out["r"]["detail"]
    assert set(out["sent"]) == {"mouseMoved"}
