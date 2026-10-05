// Owns exact tab-local pointer gestures; must not choose a target, move the OS cursor or leave buttons down.
const CANCELLED_POINTERS = new Set();

async function visualStates(tabId) {
  const refs = tabState(tabId).frameRefs;
  if (!refs) throw new Error("take_a_snapshot_first");
  return Promise.all([...refs.keys()].map(async frame => [frame,
    await inFrame(tabId, frame, () => window.__ferb.visualState(), [])]));
}

async function checkVisual(tabId, visual = tabState(tabId).visual) {
  if (!visual || Date.now() - visual.at > 30000 ||
      JSON.stringify(await visualStates(tabId)) !== visual.states) throw new Error("visual_stale; take another screenshot");
}

async function resolveVisualPoint(tabId, snap, x, y) {
  const refs = tabState(tabId).frameRefs;
  let frame = 0, px = x, py = y;
  const visited = new Set();
  while (!visited.has(frame)) {
    visited.add(frame);
    const hit = await inFrame(tabId, frame, (g, a, b) => window.__ferb.point(g, a, b), [snap, px, py]);
    if (hit.error) throw new Error(hit.error);
    if (hit.element) return {x, y, frame_id: frame, frame_x: px, frame_y: py, element: hit.element};
    const children = refs ? [...refs.values()].filter(f => f.parent_frame_id === frame && f.owner_binding === hit.frame) : [];
    if (children.length !== 1) throw new Error(children.length ? "ambiguous_frame_binding" : "frame_offset_unknown");
    const child=children[0];
    const r = await inFrame(tabId, frame, token => window.__ferb.frameRect(token), [hit.frame]);
    if (r.error || r.occluded) throw new Error(r.error || "frame_occluded");
    px -= r.x; py -= r.y; frame = child.frame_id;
  }
  throw new Error("frame_binding_cycle");
}

async function checkPoint(tabId, point, visual) {
  const current = await resolveVisualPoint(tabId, visual.snap, point.x, point.y);
  if (JSON.stringify(current) !== JSON.stringify(point)) throw new Error("visual_target_changed; take another screenshot");
  if (current.element.secret || (current.element.state || []).includes("disabled")) throw new Error("pointer_target_unavailable");
  return current;
}

async function checkSurface(tabId, visual) {
  // Drag changes content by design, but it must never continue into a new document or shifted viewport.
  const withoutEpoch = states => states.map(([frame, state]) => {
    const {epoch, ...surface} = state;
    return [frame, surface];
  });
  if (JSON.stringify(withoutEpoch(await visualStates(tabId))) !== JSON.stringify(withoutEpoch(JSON.parse(visual.states))))
    throw new Error("pointer_surface_changed");
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
  const st = await attach(tab_id), visual = st.visual;
  let start, finish;
  try {
    await checkVisual(tab_id, visual);
    if (st.gesture) throw new Error("pointer_busy");
    start = visual.points.get(point); finish = kind === "drag" ? visual.points.get(end) : start;
    if (!start || !finish) throw new Error("unknown_visual_point");
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
    await readyPoint(tab_id, start, alive);
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
            if (i % 5 === 0) await checkSurface(tab_id, visual);
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
