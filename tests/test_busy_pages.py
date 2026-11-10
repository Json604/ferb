"""Past-bug guards from live 2026-11-10 (amazon.in product pages, task 64a149b8): FERB couldn't press Add to cart or
Buy Now. A screenshot point failed 10 of 10 as "visual_stale" and screenshots as "page_changed_during_screenshot",
because any DOM change anywhere counted (HAND-480). The extension's own code, run under node."""

from tests.chromelab import worker

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
  mouse = async (tab, type, x, y) => { events.push(type); if (type === "mouseMoved") lastMove = y >= 500 ? [x, y - 500] : [x, y]; };
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
