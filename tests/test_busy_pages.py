"""Past-bug guards from live 2026-11-10 (amazon.in product pages, task 64a149b8): FERB couldn't press Add to cart or
Buy Now. A screenshot point failed 10 of 10 as "visual_stale" and screenshots as "page_changed_during_screenshot",
because any DOM change anywhere counted (HAND-480, HAND-481); an element parked off the page's left edge was called
covered (HAND-482); the in-view buy box was cut from the snapshot preview for off-screen items (HAND-483);
browser.find could only return page text, never the button to press (HAND-484). The extension's own code, run under
node; the daemon's preview, and find through the real daemon and gateway over the fake extension."""

import json
import subprocess
import time
from typing import Any

from ferb.act.browser.snapshot import for_model
from ferb.core.types import FormSubmit, PageElement, PageSnapshot, TabRef
from tests.browserkit import OWNER, bd, call, pair, task  # noqa: F401 (fixture)
from tests.chromelab import EXT, _node, worker

# A tab whose top frame (0) holds the buy button above y=500 and an ad's frame (7) below it. Every read of a frame's
# visual state gives a new `epoch`, as an older content.js would on a page that never sits still. `during` runs while
# the screenshot is captured.
_PAGE = r'''
  const refs = new Map([[0, {frame_id: 0}], [7, {frame_id: 7, parent_frame_id: 0, owner_binding: "b7"}]]);
  const st = tabState(1); st.frameRefs = refs;
  let epoch = 0, lastMove = null, during = () => {};
  const states = {0: {documentId: "top", generation: 1, viewport: [1280, 800], scroll: [0, 300], scale: 2},
                  7: {documentId: "ad-1", generation: 1, viewport: [300, 250], scroll: [0, 0], scale: 2}};
  let buy = {id: 3, role: "button", label: "Add to cart", href: null, rect: [10, 10, 200, 40], value: null};
  const ad = {id: 1, role: "link", label: "Sale", href: "https://ads.example/x", rect: [0, 0, 300, 250]};
  const events = [];
  inFrame = async (tab, frame, func, args) => {
    const src = String(func);
    if (src.includes("visualState")) return {...states[frame], epoch: ++epoch};
    if (src.includes("__ferb.point(")) {
      const [, x, y] = args;
      if (frame === 0) return y >= 500 ? {frame: "b7"} : {element: {...buy, epoch: ++epoch}};
      return {element: {...ad}};
    }
    if (src.includes("frameRect")) return {x: 0, y: 500, occluded: false};
    if (src.includes("viewport()")) return [1280, 800];
    if (src.includes("pointerArm")) return true;
    if (src.includes("pointerSeen")) return lastMove;
    throw new Error("unexpected page script " + src);
  };
  let hover = () => {};                                  // what the pointer's own move does to the page
  mouse = async (tab, type, x, y) => {
    events.push(type);
    if (type === "mouseMoved") { lastMove = y >= 500 ? [x, y - 500] : [x, y]; hover(); }
  };
  attach = async (id) => tabState(id);
  arm = async () => {}; readyForCapture = async () => "";
  maskState = async () => ({top: "d", viewport: [0, 0, 1280, 800], items: []});
  cdp = async (tab, method) => { if (method === "Page.captureScreenshot") { during(); return {data: "jpeg"}; } return {}; };
  const shoot = () => commands.screenshot({tab_id: 1, snap: 1, _timeout_ms: 5000});
  const point = (x, y) => commands.point({tab_id: 1, snap: 1, x, y, image_width: 1280, image_height: 800});
  const press = (p, id) => commands.pointer({tab_id: 1, point: p.pointer, kind: "click"}, id);
  const err = async (f) => { try { await f(); return null; } catch (e) { return String(e.message || e); } };
'''


# decision: HAND-480, HAND-481
def test_a_point_on_a_busy_page_is_pressed_and_a_frame_that_changed_is_named() -> None:
    """Every read of the page differs (epoch) and the ad frame reloads during the capture: the screenshot is still
    taken, the buy button is pressed, and a point through the ad frame is refused naming it."""
    out = worker('(async () => {' + _PAGE + r'''
      during = () => { states[7] = {...states[7], documentId: "ad-2"}; };
      const shot = await shoot();
      const p = await point(50, 30);
      const throughAd = await err(() => point(50, 600));
      const r = await press(p, "c1");
      return {shot: shot.data, frames: p.frames, label: p.element.label, throughAd, ok: r.ok, events};
    })()''')
    assert out["shot"] == "jpeg" and out["frames"] == [0] and out["label"] == "Add to cart"
    assert out["throughAd"].startswith("visual_stale: frame 7 document"), out
    assert out["ok"] is True and out["events"].count("mousePressed") == 1 and out["events"].count("mouseReleased") == 1


# decision: HAND-480
def test_a_point_is_refused_when_what_the_click_depends_on_changed_and_says_what() -> None:
    """The click still lands only on what the worker saw: a scroll of a frame on its path, another element under the
    point, or the same element relabelled or moved refuses it before anything is pressed, naming frame and part."""
    out = worker('(async () => {' + _PAGE + r'''
      const tries = {};
      const attempt = async (name, change) => {
        await shoot(); const p = await point(50, 30); change();
        const r = await press(p, "c-" + name);
        tries[name] = r.ok ? "pressed" : r.detail;
      };
      await attempt("scroll", () => { states[0] = {...states[0], scroll: [0, 340]}; });
      await attempt("other", () => { buy = {...buy, id: 9}; });
      await attempt("label", () => { buy = {...buy, label: "Remove"}; });
      await attempt("rect", () => { buy = {...buy, rect: [10, 60, 200, 40]}; });
      await attempt("ad", () => { states[7] = {...states[7], documentId: "ad-3"}; });   // off the click's path
      await shoot(); const p = await point(50, 30); const r = await press(p, "c-done");
      const again = await press(p, "c-again");                                          // a gesture uses its screenshot
      return {tries, events, again: again.detail};
    })()''')
    t = out["tries"]
    assert t["scroll"].startswith("visual_stale: frame 0 scroll"), t
    assert t["other"].startswith("visual_target_changed: frame 0 element (another element"), t
    assert t["label"].startswith("visual_target_changed: frame 0 element label"), t
    assert t["rect"].startswith("visual_target_changed: frame 0 element rect"), t
    assert t["ad"] == "pressed"
    assert out["events"].count("mousePressed") == 2 and out["again"].startswith("visual_stale: no screenshot")


# decision: HAND-480
def test_the_pointers_own_hover_may_restyle_the_target_but_a_tooltip_over_the_point_refuses() -> None:
    out = worker('(async () => {' + _PAGE + r'''
      await shoot(); let p = await point(50, 30);
      hover = () => { buy = {...buy, rect: [8, 8, 204, 44]}; };            // a hover style grows the button
      const grown = await press(p, "c-grow");
      buy = {...buy, rect: [10, 10, 200, 40]};
      await shoot(); p = await point(50, 30);
      hover = () => { buy = {id: 12, role: "tooltip", label: "Ships tomorrow", href: null, rect: [0, 0, 120, 60]}; };
      const covered = await press(p, "c-tip");
      return {grown: grown.ok, covered, presses: events.filter(e => e === "mousePressed").length};
    })()''')
    assert out["grown"] is True and out["presses"] == 1
    assert out["covered"]["ok"] is False and out["covered"]["detail"].startswith(
        "visual_target_changed: frame 0 element (another element"), out


# decision: HAND-482
def test_an_element_still_off_the_page_after_scrolling_is_said_so_and_a_covered_one_stays_covered() -> None:
    src = (EXT / "content.js").read_text()
    fns = []
    for name in ("hitTest", "rect"):
        start = src.index(f"  function {name}(")
        fns.append(src[start:src.index("\n  }\n", start) + 4])
    script = r'''
class ShadowRoot {}
const innerWidth = 1280, innerHeight = 800;
let cover = null;
const document = {elementFromPoint: () => cover};
const isSecret = () => false;
const box = (x, y, w, h) => ({x, y, width: w, height: h, left: x, top: y, right: x + w, bottom: y + h});
const el = (r) => ({tagName: "A", getRootNode: () => document, scrollIntoView() {}, getBoundingClientRect: () => r,
                    getAttribute: () => null, contains: (o) => false, isContentEditable: false});
let target = null;
const element = () => ({el: target});
''' + "\n".join(fns) + r'''
const out = {};
target = el(box(-10000, 300, 120, 30)); cover = {}; out.left = rect(1, 0).error;
target = el(box(400, 900, 120, 30)); out.below = rect(1, 0).error;
target = el(box(400, 300, 120, 30)); cover = {}; out.covered = rect(1, 0);
cover = target; out.clear = rect(1, 0);
console.log(JSON.stringify(out));
'''
    ran = subprocess.run([_node(), "-e", script], capture_output=True, text=True, timeout=20)
    out = json.loads(ran.stdout or "null")
    assert out, ran.stderr
    assert out["left"].startswith("outside_page:") and "even after scrolling" in out["left"] and "left" in out["left"]
    assert "bottom" in out["below"]
    assert out["covered"]["occluded"] is True and "error" not in out["covered"]
    assert out["clear"]["occluded"] is False


# decision: HAND-482
def test_an_off_page_copy_names_the_same_control_on_screen(bd: Any) -> None:  # noqa: F811
    """Amazon parks a screen-reader copy of "Add to cart" off the page; the refusal names the one on screen."""
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    g = task(d, "o", OWNER, {"research"})
    tab = call(d, g, "browser.open", url="https://shop.example/product")["content"]["tab"]
    els = call(d, g, "browser.snapshot", tab=tab)["content"]["elements"]
    parked = next(e["id"] for e in els if e.get("offscreen") == "left")
    shown = next(e["id"] for e in els if e["label"] == "Add to cart" and "in_view" not in e)
    r = call(d, g, "browser.act", tab=tab, element=parked, action="click")
    assert r["is_error"] and r["content"]["on_screen"] == shown and shown in r["content"]["detail"], r


def _el(n: int, label: str, in_view: bool) -> PageElement:
    return PageElement(id=f"s1e{n}", frame_id="0", role="link" if not in_view else "button", label=label, value=None,
                       rect=(0, 0, 10, 10), visible=True, in_view=in_view, offscreen=None if in_view else "below")


# decision: HAND-483
def test_the_preview_keeps_what_is_on_screen_and_says_how_many_on_screen_were_left_out() -> None:
    """Accessibility-matched elements come first, page-walk ones after: off-screen menu links filled the budget and the
    buy box in view (later in the list) was cut."""
    tab = TabRef("d", "chrome", "Default", 1, "t1")
    menu = [_el(n, f"Department {n} " + "x" * 200, False) for n in range(1, 400)]
    box = [_el(400, "Buy Now", True), _el(401, "Add to Cart", True)]
    out = for_model(PageSnapshot(tab, 1, "https://shop.example/p", "P", menu + box))
    labels = [e["label"] for e in out["elements"]]
    assert "Buy Now" in labels and "Add to Cart" in labels and labels[-2:] == ["Buy Now", "Add to Cart"]
    assert out["omitted_elements"] > 0 and "omitted_in_view_elements" not in out
    crowded = for_model(PageSnapshot(tab, 1, "https://shop.example/p", "P",
                                     [_el(n, "On screen " + "y" * 300, True) for n in range(1, 200)]))
    assert crowded["omitted_in_view_elements"] == crowded["omitted_elements"] > 0
    assert str(crowded["omitted_in_view_elements"]) in crowded["omitted_note"]
    full = for_model(PageSnapshot(tab, 1, "https://shop.example/p", "P", menu + box), full=True)
    assert len(full["elements"]) == 401                                        # the saved snapshot keeps everything
    index = out["omitted_index"]
    assert index["by_role"] == {"link": out["omitted_elements"]} and index["buttons_and_links"][0].startswith("Department")


# decision: HAND-483
def test_the_preview_keeps_a_dialog_a_covered_control_and_an_on_screen_forms_submit_first() -> None:
    """A control under a sticky bar is `in_view: false` but on screen; a dialog's elements and the submit button of a
    form whose field is on screen come before everything else, wherever they sit in the list."""
    tab = TabRef("d", "chrome", "Default", 1, "t1")
    filler = [_el(n, "On screen " + "y" * 300, True) for n in range(1, 200)]
    form = "html:1>body:1>form:2"
    field = PageElement(id="s1e500", frame_id="0", role="textbox", label="Pincode", value=None, rect=(0, 0, 9, 9),
                        visible=True, path=form + ">input:1")
    submit = PageElement(id="s1e501", frame_id="0", role="button", label="Check", value=None, rect=(0, 0, 9, 9),
                         visible=True, in_view=False, offscreen="below", path=form + ">button:1",
                         submit=FormSubmit(True, False, "shop.example", "GET"))
    covered = PageElement(id="s1e502", frame_id="0", role="button", label="Buy Now", value=None, rect=(0, 0, 9, 9),
                          visible=True, in_view=False, occluded=True)
    close = PageElement(id="s1e503", frame_id="0", role="button", label="Close", value=None, rect=(0, 0, 9, 9),
                        visible=True, in_view=False, offscreen="below", in_dialog=True)
    out = for_model(PageSnapshot(tab, 1, "https://shop.example/p", "P", filler + [field, submit, covered, close]))
    kept = {e["id"] for e in out["elements"]}
    assert {"s1e501", "s1e503"} <= kept and out["omitted_in_view_elements"] > 0
    assert [e for e in out["elements"] if e["id"] == "s1e503"][0]["dialog"] is True
    crowded = for_model(PageSnapshot(tab, 1, "https://shop.example/p", "P",
                                     [covered] + [_el(n, "Off " + "z" * 300, False) for n in range(1, 200)]))
    assert crowded["elements"][0]["id"] == "s1e502"                             # covered, still on screen: kept


# decision: HAND-484
def test_find_returns_the_snapshots_matching_elements_by_id_before_the_page_text(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    g = task(d, "f", OWNER, {"research"})
    tab = call(d, g, "browser.open", url="https://shop.example/form")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    submit = next(e["id"] for e in snap["elements"] if e["label"] == "Submit")
    found = call(d, g, "browser.find", tab=tab, text="submit")["content"]
    hits = json.loads(found["text"])
    assert found["element_matches"] == 1 and hits[0] == {"element": submit, "role": "button", "label": "Submit"}
    assert found["matches"] == 0 and found["examples"] == []          # "submit" isn't in the page's words (case)
    buttons = call(d, g, "browser.find", tab=tab, text="button")["content"]   # by role too
    assert [h["element"] for h in json.loads(buttons["text"]) if "element" in h] == [submit]
    link = next(e["id"] for e in snap["elements"] if e["label"] == "Next")
    by_href = json.loads(call(d, g, "browser.find", tab=tab, text="example/next")["content"]["text"])
    assert by_href[0]["element"] == link and by_href[0]["href"].endswith("/next")       # a link by its address
    lines = json.loads(call(d, g, "browser.find", tab=tab, text="Submit")["content"]["text"])
    assert lines[0]["element"] == submit and [h.get("element") for h in lines[1:]] == [submit]   # the line is its label
    assert not call(d, g, "browser.act", tab=tab, element=link, action="click")["is_error"]   # navigates
    time.sleep(0.2)
    stale = call(d, g, "browser.find", tab=tab, text="submit")["content"]
    assert stale["element_matches"] == 0 and "take one for element ids" in stale["note"]     # no ids from before


# decision: HAND-484
def test_find_skips_a_frame_the_page_doesnt_draw() -> None:
    """Live: find returned a hidden ad frame's script source ("ReactDOM.hydrateRoot…"): an undrawn body's innerText is
    its raw text."""
    out = worker(r'''(async () => {
      const frames = [{id: 0, w: 1280, rects: 1, text: "Buy Now\nAdd to Cart"},
                      {id: 9, w: 0, rects: 0, text: "ReactDOM.hydrateRoot(Buy Now)"}];
      chrome.scripting = {executeScript: async ({func, args}) => frames.map(f => {
        globalThis.innerWidth = f.w; globalThis.innerHeight = f.w ? 800 : 0;
        globalThis.document = {body: {innerText: f.text, getClientRects: () => new Array(f.rects)}};
        return {frameId: f.id, result: func(...args)};
      })};
      return await commands.find({tab_id: 1, text: "buy now"});
    })()''')
    assert out["count"] == 1 and out["hits"] == [{"frame_id": 0, "text": "Buy Now"}]


# decision: HAND-487
def test_an_act_says_what_changed_around_it_and_nothing_is_not_failure() -> None:
    """Live: every act came back `after: {"read": "none", "changed": null}` and "clicked (trusted)" whether or not
    anything happened."""
    out = worker(r'''(async () => {
      let doc = "d1", near = {gone: false, state: [], mutations: 0, scope: "form"}, url = "https://s.example/a";
      inFrame = async (tab, frame, func) => {
        const src = String(func);
        if (src.includes("documentKey")) return doc;
        if (src.includes("watchNear")) return true;
        if (src.includes("nearChange")) return near;
        throw new Error("unexpected " + src);
      };
      chrome.tabs.get = async () => ({id: 1, url});
      let effect = () => {};
      commands.actOnce = async () => { effect(); return {ok: true, detail: "clicked (trusted)"}; };
      const act = () => commands.act({tab_id: 1, frame_id: 0, snap: 1, el: 3, kind: "click"});
      const out = {};
      effect = () => { doc = "d2"; url = "https://s.example/b"; }; out.nav = await act();
      effect = () => { near = {...near, state: ["expanded"]}; }; out.menu = await act();
      effect = () => { near = {gone: false, state: [], mutations: 0, scope: "form"}; }; out.none = await act();
      commands.actOnce = async () => ({ok: false, detail: "target_disabled"}); out.refused = await act();
      return out;
    })()''')
    assert out["nav"]["observed_change"] is True and "new document" in out["nav"]["observed"]
    assert out["menu"]["observed_change"] is True and out["menu"]["observed"] == "the element's expanded changed"
    assert out["none"]["observed_change"] is False and out["none"]["observed"] == "nothing changed within 0.5 s"
    assert "observed_change" not in out["refused"]


# decision: HAND-487
def test_after_reads_the_pages_own_observation() -> None:
    from ferb.act.aftercheck import after_of
    seen = after_of({"ok": True, "observed_change": True, "observed": "the address changed"})
    quiet = after_of({"ok": True, "observed_change": False, "observed": "nothing changed within 0.5 s"})
    assert seen["read"] == "page" and seen["changed"] is True and seen["note"] == "the address changed"
    assert quiet["changed"] is None and quiet["note"] == "nothing changed within 0.5 s"      # not confirmed, not failed
