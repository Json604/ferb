// Owns exact tab-local pointer gestures; must not choose a target, move the OS cursor or leave buttons down.
const CANCELLED_POINTERS = new Set();

// What a click at a screenshot point depends on in each frame it goes through (HAND-480): the same document, no newer
// snapshot, the same viewport size, scroll and zoom. Not whether anything else on the page changed: busy pages (ads,
// carousels) never sit still. The element under the point is checked on its own (checkPoint).
const SURFACE = ["documentId", "generation", "viewport", "scroll", "scale"];
const surfaceOf = state => Object.fromEntries(SURFACE.map(k => [k, state[k]]));

// [frame, its surface, or null when that frame can't be read now] for the given frames (default: every snapshot frame)
async function visualStates(tabId, frames) {
  const refs = tabState(tabId).frameRefs;
  if (!refs) throw new Error("take_a_snapshot_first");
  return Promise.all((frames || [...refs.keys()]).map(async frame => {
    try { return [frame, surfaceOf(await inFrame(tabId, frame, () => window.__ferb.visualState(), []))]; }
    catch (_) { return [frame, null]; }
  }));
}

// The first part of a frame's surface that differs, in words ("" when none).
function surfaceChange(before, now) {
  if (!before) return "(it wasn't readable when the screenshot was taken)";
  if (!now) return "(it can't be read now: gone or navigated)";
  for (const k of SURFACE) if (JSON.stringify(before[k]) !== JSON.stringify(now[k]))
    return k === "documentId" ? "document (it navigated or reloaded)" : k === "generation" ? "snapshot (a newer one was taken)" : k;
  return "";
}

// The screenshot still holds for these frames: under 30 s old, none of them changed while it was taken, and each one's
// surface now is the one it had then. Every refusal names the frame and the part that differed.
async function checkVisual(tabId, visual, frames) {
  if (!visual) throw new Error("visual_stale: no screenshot to point on; take another screenshot");
  if (Date.now() - visual.at > 30000) throw new Error("visual_stale: the screenshot is over 30 s old; take another screenshot");
  for (const f of frames) if (visual.unsteady[f])
    throw new Error(`visual_stale: frame ${f} ${visual.unsteady[f]} changed while the screenshot was taken; take another screenshot`);
  for (const [f, now] of await visualStates(tabId, frames)) {
    const part = surfaceChange(visual.states[f], now);
    if (part) throw new Error(`visual_stale: frame ${f} ${part}; take another screenshot`);
  }
}

async function resolveVisualPoint(tabId, snap, x, y) {
  const refs = tabState(tabId).frameRefs;
  let frame = 0, px = x, py = y;
  const visited = new Set();
  while (!visited.has(frame)) {
    visited.add(frame);
    const hit = await inFrame(tabId, frame, (g, a, b) => window.__ferb.point(g, a, b), [snap, px, py]);
    if (hit.error) throw new Error(hit.error);
    // `frames`: the frames the click goes through, top first (the ones checkVisual compares)
    if (hit.element) return {x, y, frame_id: frame, frame_x: px, frame_y: py, element: hit.element, frames: [...visited]};
    const children = refs ? [...refs.values()].filter(f => f.parent_frame_id === frame && f.owner_binding === hit.frame) : [];
    if (children.length !== 1) throw new Error(children.length ? "ambiguous_frame_binding" : "frame_offset_unknown");
    const child=children[0];
    const r = await inFrame(tabId, frame, token => window.__ferb.frameRect(token), [hit.frame]);
    if (r.error || r.occluded) throw new Error(r.error || "frame_occluded");
    px -= r.x; py -= r.y; frame = child.frame_id;
  }
  throw new Error("frame_binding_cycle");
}

// What differs between the point as reported to the worker and as resolved now ("" when it is the same target): the
// frames it goes through, where it lands in its frame (a frame moved), the element under it (content.js keeps one id
// per element for the snapshot's life), and that element's role, label, link and rect.
function pointChange(was, now) {
  if (JSON.stringify(was.frames) !== JSON.stringify(now.frames))
    return `frames (${(was.frames || []).join(">")} then, ${now.frames.join(">")} now)`;
  const f = now.frame_id;
  if (Math.abs(was.frame_x - now.frame_x) > 0.5 || Math.abs(was.frame_y - now.frame_y) > 0.5) return `frame ${f} position`;
  const a = was.element, b = now.element;
  if (a.id !== b.id) return `frame ${f} element (another element is under the point now)`;
  for (const k of ["role", "label", "href", "rect"])
    if (JSON.stringify(a[k]) !== JSON.stringify(b[k])) return `frame ${f} element ${k}`;
  return "";
}

// Re-resolves the point now: the same element at the same place as when the worker was given it, every frame on its
// path unchanged, and a target that takes the click (not secret, not disabled). Run before and right before the press.
async function checkPoint(tabId, point, visual) {
  const current = await resolveVisualPoint(tabId, visual.snap, point.x, point.y);
  const part = pointChange(point, current);
  if (part) throw new Error(`visual_target_changed: ${part}; take another screenshot`);
  // content.js describe() reports a disabled control as `disabled: true` (protocol 17 read a `state` list it never sent)
  if (current.element.secret || current.element.disabled === true) throw new Error("pointer_target_unavailable");
  await checkVisual(tabId, visual, current.frames);
  return current;
}

async function checkSurface(tabId, visual, frames) {
  // Drag changes content by design, but it must never continue into a new document or shifted viewport.
  for (const [f, now] of await visualStates(tabId, frames)) {
    const part = surfaceChange(visual.states[f], now);
    if (part) throw new Error(`pointer_surface_changed: frame ${f} ${part}`);
  }
}

async function readyPoint(tabId, point, alive) {
  // OOPIF compositor transforms can lag DOM geometry after metrics setup. Never press until a trusted move
  // is observed at the intended frame-local coordinate; a successful CDP response alone isn't sufficient.
  for (let attempt = 0; attempt < 20; attempt++) {
    alive();
    await inFrame(tabId, point.frame_id, () => { window.__ferb.pointerArm(); return true; }, []);
    await mouse(tabId, "mouseMoved", point.x, point.y, {buttons: 0});
    const seen = await inFrame(tabId, point.frame_id, () => window.__ferb.pointerSeen(), []);
    if (seen.length === 2 && Math.abs(seen[0] - point.frame_x) <= 1 && Math.abs(seen[1] - point.frame_y) <= 1) return;
    await sleep(25);
  }
  throw new Error("pointer_did_not_reach_target_frame");
}

async function exactPointer({tab_id, point, end, kind}, commandId) {
  if (!["click", "dblclick", "rightclick", "hover", "drag"].includes(kind)) throw new Error("unsupported_pointer_action");
  let st;
  try { st = await attach(tab_id); }
  catch (e) {                                   // the browser refused the debugger: nothing sent, a known refusal
    if (!e.debuggerRefused) throw e;
    return {ok: false, reason: "debugger_refused", detail: `${refusedWhy(e)}, so a screenshot point can't be ` +
      "clicked; nothing was sent. Try an element of a snapshot with browser.act and page_mode=true"};
  }
  const visual = st.visual;
  let start, finish;
  try {
    if (st.gesture) throw new Error("pointer_busy");
    start = visual && visual.points.get(point); finish = kind === "drag" ? visual && visual.points.get(end) : start;
    if (visual && (!start || !finish)) throw new Error("unknown_visual_point");
    await checkVisual(tab_id, visual, start ? start.frames : [0]);
    if (kind === "drag" && start.frame_id !== finish.frame_id) throw new Error("drag_across_frames_unsupported");
    await checkPoint(tab_id, start, visual);
    if (finish !== start) await checkPoint(tab_id, finish, visual);
  } catch (e) {
    return {ok: false, detail: String(e.message || e)};   // no input attempted: a known refusal, not unknown
  }
  const gesture = {id: commandId, cancelled: false};
  st.gesture = gesture;
  st.visual = null;                             // every gesture consumes the screenshot, even on partial failure
  const button = kind === "rightclick" ? "right" : "left";
  let held = false, x = start.x, y = start.y;
  const alive = () => {
    if (gesture.cancelled || CANCELLED_POINTERS.has(commandId) || st.leaving || st.detached)
      throw new Error("pointer_cancelled");
  };
  try {
    alive();
    try { await readyPoint(tab_id, start, alive); }
    catch (e) {                                 // only moves were sent, never a press: a known failure (live 2026-10-26)
      if (String(e.message || e) !== "pointer_did_not_reach_target_frame") throw e;
      return {ok: false, reason: "pointer_did_not_reach_target_frame", detail: "the pointer's move never reached the " +
        "target's frame, so nothing was pressed; take a fresh screenshot and point again"};
    }
    alive();
    // Hover can expose a different control. Recheck the actual hit before pressing.
    await checkPoint(tab_id, start, visual);
    if (kind !== "hover") {
      for (let n = 1; n <= (kind === "dblclick" ? 2 : 1); n++) {
        alive();
        held = true;                              // a rejected CDP response may follow an already delivered press
        await mouse(tab_id, "mousePressed", x, y, {button, buttons: button === "right" ? 2 : 1, clickCount: n});
        if (kind === "drag") {
          for (let i = 1; i <= 20; i++) {
            alive();
            if (i % 5 === 0) await checkSurface(tab_id, visual, start.frames);
            x = start.x + (finish.x - start.x) * i / 20;
            y = start.y + (finish.y - start.y) * i / 20;
            await mouse(tab_id, "mouseMoved", x, y, {button, buttons: 1});
            await sleep(20);
          }
        }
        alive();
        await mouse(tab_id, "mouseReleased", x, y, {button, buttons: 0, clickCount: n});
        held = false;
      }
    }
    st.pointer = {x, y, action: kind};
    return {ok: true, detail: `${kind} input delivered at (${Math.round(x)}, ${Math.round(y)}); verify the result`};
  } finally {
    try {
      if (held) await mouse(tab_id, "mouseReleased", x, y, {button, buttons: 0, clickCount: 1});
    } finally {
      if (st.gesture === gesture) st.gesture = null;
      CANCELLED_POINTERS.delete(commandId);
    }
  }
}

function cancelPointer(tabId, commandId) {
  CANCELLED_POINTERS.add(commandId);
  if (CANCELLED_POINTERS.size > 500) CANCELLED_POINTERS.delete(CANCELLED_POINTERS.values().next().value);
  const st = TABS.get(tabId);
  if (st && st.gesture && st.gesture.id === commandId) st.gesture.cancelled = true;
  return {ok: true};
}
