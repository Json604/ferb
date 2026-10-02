// FERB service worker: FERB's hands in the browser. Executes daemon commands and reports state; holds no policy
// and no task logic (ARCHITECTURE.md 9.4). Reliability (spike 13): commands carry the connection generation; ids
// are recorded as seen BEFORE executing (storage.session survives worker restarts); old-generation commands are
// answered "stale" and never run; duplicates run once; hello re-announces owned tabs and seen ids.
//
// Background by construction (PLATFORM_BROWSER.md 13): one FERB window per profile, born minimized and kept;
// every task tab is created inactive inside it; the debugger stays attached to a task tab for its life with focus
// emulation on, so trusted input, scrolling and screenshots work while the owner never sees a thing.
importScripts("protocol.js");

let port = null;
let gen = null;                      // connection generation, assigned by the daemon ("welcome" / "paired")
let retryMs = 1000;
let refused = null;                  // the daemon refused this install (e.g. protocol mismatch): stop retrying
const outbox = [];                   // queued while disconnected (bounded)
const OUTBOX_MAX = 200;
const VIEWPORT = { width: 1280, height: 900 };   // Emulation.setDeviceMetricsOverride for FERB's own tabs (2.8)

// ---------- state ----------

const b64e = (buf) => btoa(String.fromCharCode(...new Uint8Array(buf)));
const b64d = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));

async function install() {           // install_id is generated once per profile; key arrives at pairing
  let { ferb } = await chrome.storage.local.get("ferb");
  if (!ferb) {
    ferb = { installId: crypto.randomUUID(), key: null, seq: 0 };
    await chrome.storage.local.set({ ferb });
  }
  return ferb;
}

async function session() {
  const { s } = await chrome.storage.session.get("s");
  return s || { seen: {}, order: [], owned: [], windowId: null };
}
async function saveSession(s) { await chrome.storage.session.set({ s }); }

async function hmac(keyB64, text) {
  const k = await crypto.subtle.importKey("raw", b64d(keyB64), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return b64e(await crypto.subtle.sign("HMAC", k, new TextEncoder().encode(text)));
}

// The stored {key, seq} is read-modify-written by every send and by pairing: one at a time, or a send racing the
// "paired" reply writes the old key back (every later message fails its MAC) and two sends can share a seq.
let stateLock = Promise.resolve();
function serially(fn) {
  const p = stateLock.then(fn);
  stateLock = p.catch(() => {});
  return p;
}

// Every message: install_id + type + body_json; once paired, a strictly increasing seq and an HMAC over all of it.
// Posted inside the lock, and always after whatever the outbox still holds, so messages leave in seq order (a
// hello sent on reconnect never overtakes events queued while disconnected: the daemon would drop those as replays).
function send(type, body) {
  return serially(async () => {
    const st = await install();
    const msg = { install_id: st.installId, type, body_json: JSON.stringify(body) };
    if (st.key) {
      st.seq += 1;
      await chrome.storage.local.set({ ferb: st });
      msg.seq = st.seq;
      msg.mac = await hmac(st.key, `${msg.install_id}|${msg.seq}|${msg.type}|${msg.body_json}`);
    }
    if (port) { flushOutbox(); port.postMessage(msg); }
    else { outbox.push(msg); if (outbox.length > OUTBOX_MAX) outbox.shift(); ensurePort(); }
  });
}
function flushOutbox() { while (outbox.length && port) port.postMessage(outbox.shift()); }

// ---------- connection ----------

async function ensurePort() {
  if (port || refused) return;
  try { port = chrome.runtime.connectNative(FERB_HOST); } catch (_) { port = null; return; }
  gen = null;
  port.onMessage.addListener(onDaemon);
  port.onDisconnect.addListener(() => {
    void chrome.runtime.lastError;   // read it, or Chrome logs "Unchecked runtime.lastError" (spike 12)
    port = null; gen = null;
    setTimeout(ensurePort, retryMs);
    retryMs = Math.min(retryMs * 2, 60000);
  });
  const st = await install(), s = await session();
  await send("hello", { version: chrome.runtime.getManifest().version, protocol: FERB_PROTOCOL,
                        browser: navigator.brave ? "brave" : "chrome", paired: !!st.key,
                        owned: await ownedTabs(), seen: s.order.slice(-200).map((id) => ({ id, status: s.seen[id] })) });
}

async function onDaemon(m) {
  if (m.type === "welcome") { gen = m.gen; retryMs = 1000; return; }   // backoff resets only once the daemon answered
  if (m.type === "paired" && m.key) {
    await serially(async () => {
      const st = await install();
      st.key = m.key; st.seq = 0;
      await chrome.storage.local.set({ ferb: st });
      outbox.length = 0;                                   // signed under the old key / seq: never valid again
    });
    gen = m.gen;
    await send("hello", { version: chrome.runtime.getManifest().version, protocol: FERB_PROTOCOL,
                          browser: navigator.brave ? "brave" : "chrome", paired: true, owned: [], seen: [] });
    return;
  }
  if (m.type === "refused") { refused = m.reason || "refused"; return; }
  if (m.type === "cmd") await onCommand(m);
}

chrome.runtime.onStartup.addListener(() => ensurePort());
chrome.runtime.onInstalled.addListener(() => ensurePort());
chrome.alarms.create("ferb-reconnect", { periodInMinutes: 0.5 });
chrome.alarms.onAlarm.addListener(() => ensurePort());
chrome.runtime.onMessage.addListener((m) => { if (m && m.pairToken) send("pair", { token: m.pairToken }); });

// ---------- commands ----------

async function onCommand(m) {
  if (m.gen !== gen) { await send("result", { id: m.id, gen: m.gen, status: "stale" }); return; }   // never replay
  const s = await session();
  if (s.seen[m.id]) { await send("result", { id: m.id, gen, status: "duplicate" }); return; }
  s.seen[m.id] = "started"; s.order.push(m.id);
  while (s.order.length > 500) delete s.seen[s.order.shift()];
  await saveSession(s);                                        // recorded BEFORE executing
  const handler = OPS.includes(m.op) ? commands[m.op] : null;
  let body;
  try {
    if (!handler) throw new Error(`unknown op ${m.op}`);
    const value = await withTimeout(handler({ ...(m.args || {}), _timeout_ms: m.timeout_ms || 30000 }, m.id),
                                    m.timeout_ms || 30000);
    body = { id: m.id, gen, status: "ok", value };
  } catch (e) {
    body = { id: m.id, gen, status: "error", error: String((e && e.message) || e).slice(0, 300) };
  }
  const s2 = await session(); s2.seen[m.id] = "done"; await saveSession(s2);
  await send("result", body);
}

function withTimeout(p, ms) {
  return Promise.race([p, new Promise((_, rej) => setTimeout(() => rej(new Error("timeout")), ms))]);
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function maskUrl(u) {
  try {
    const url = new URL(u);
    for (const k of [...url.searchParams.keys()])
      if (/token|key|code|secret|session|auth|sig|password|pwd|otp|ticket|nonce|state/i.test(k))
        url.searchParams.set(k, "[redacted]");
    if (url.hash.length > 1 && /[=&]/.test(url.hash)) url.hash = "#[redacted]";
    url.username = ""; url.password = "";
    return url.href;
  } catch (_) { return ""; }
}

async function ownedTabs() {
  const s = await session(), alive = [];
  for (const o of s.owned) {
    try { const t = await chrome.tabs.get(o.tabId); alive.push({ id: o.id, tab_id: t.id, url: maskUrl(t.url || t.pendingUrl || "") }); }
    catch (_) { /* closed */ }
  }
  return alive;
}

function waitComplete(tabId, ms) {
  return new Promise((resolve) => {
    const done = () => { chrome.tabs.onUpdated.removeListener(check); clearTimeout(timer); resolve(); };
    const check = (id, info) => { if (id === tabId && info.status === "complete") done(); };
    const timer = setTimeout(done, ms);
    chrome.tabs.onUpdated.addListener(check);
    chrome.tabs.get(tabId).then((t) => { if (t.status === "complete") done(); }).catch(done);
  });
}

// ---------- FERB's window (13.1): one per profile, born minimized, never shown or focused by FERB ----------

async function ferbWindow() {
  const s = await session();
  if (s.windowId != null) {
    try {
      const w = await chrome.windows.get(s.windowId);
      if (w.state !== "minimized") await chrome.windows.update(w.id, { state: "minimized" });   // the owner restored it
      return w.id;
    } catch (_) { /* gone: recreate */ }
  }
  // Born minimized (probe E5, S10); focused:false on top (probe E1). A blank placeholder tab keeps the window
  // alive when FERB's last task tab closes, so a window is created once per browser session, not per task.
  const w = await chrome.windows.create({ url: "about:blank", state: "minimized", focused: false });
  const s2 = await session(); s2.windowId = w.id; await saveSession(s2);
  const got = await chrome.windows.get(w.id);                // create() may report "normal" (probe E5): verify
  if (got.state !== "minimized") await chrome.windows.update(w.id, { state: "minimized" });
  return w.id;
}

async function ferbGroup(tabId, windowId) {
  const groups = await chrome.tabGroups.query({ windowId, title: "FERB" });
  if (groups.length) { await chrome.tabs.group({ tabIds: [tabId], groupId: groups[0].id }); return; }
  const groupId = await chrome.tabs.group({ tabIds: [tabId], createProperties: { windowId } });
  await chrome.tabGroups.update(groupId, { title: "FERB", color: "grey" });
}

async function inject(tabId, frameIds) {
  const target = frameIds ? { tabId, frameIds } : { tabId, allFrames: true };
  await chrome.scripting.executeScript({ target, files: ["content.js"] });
}

async function inFrame(tabId, frameId, func, args) {
  await inject(tabId, [frameId]);
  const [r] = await chrome.scripting.executeScript({ target: { tabId, frameIds: [frameId] }, func, args });
  if (!r || r.result == null) throw new Error("script failed in the page");   // null = threw (spike 4)
  return r.result;
}

// ---------- the debugger session (13.2): attached on a task tab's first action, kept for its life ----------

const TABS = new Map();          // tabId -> { attached, detached, dialogs, expect, leaving, frames, ax, shot }
function tabState(tabId) {
  let t = TABS.get(tabId);
  if (!t) { t = { attached: false, detached: null, dialogs: [], expect: null, leaving: false, frames: {}, ax: null, shot: null }; TABS.set(tabId, t); }
  return t;
}

async function cdp(tabId, method, params) {
  if (!cdpAllowed(method, params)) throw new Error(`cdp method not allowed: ${method}`);
  const t = TABS.get(tabId);
  try {
    return cdpFilter(method, await chrome.debugger.sendCommand({ tabId }, method, params || {}));
  } catch (e) {
    if (t && t.detached) throw new Error(`detached:${t.detached}`);   // the owner pressed Cancel mid-action
    throw e;
  }
}

async function attach(tabId) {
  const t = tabState(tabId);
  if (t.attached) return t;
  await chrome.debugger.attach({ tabId }, "1.3");
  t.attached = true; t.detached = null;
  await cdp(tabId, "Page.enable", {});
  await cdp(tabId, "Emulation.setFocusEmulationEnabled", { enabled: true });
  await cdp(tabId, "Target.setAutoAttach", { autoAttach: true, waitForDebuggerOnStart: false, flatten: true });
  const s = await session(), tab = await chrome.tabs.get(tabId);
  if (s.windowId != null && tab.windowId === s.windowId)     // FERB's own window: layout independent of its size.
    await cdp(tabId, "Emulation.setDeviceMetricsOverride", { ...VIEWPORT, deviceScaleFactor: 1, mobile: false });
  await ready(tabId);                                        // the owner's own tab keeps its real viewport
  return t;
}

async function detach(tabId) {
  const t = TABS.get(tabId);
  TABS.delete(tabId);
  if (!t || !t.attached) return;
  try { await chrome.debugger.sendCommand({ tabId }, "Emulation.setFocusEmulationEnabled", { enabled: false }); } catch (_) {}
  try { await chrome.debugger.detach({ tabId }); } catch (_) {}
}

chrome.debugger.onDetach.addListener((src, reason) => {
  const t = TABS.get(src.tabId);
  if (!t || !t.attached) return;
  t.attached = false; t.detached = reason || "unknown";
  if (!t.leaving) send("event", { event: "detached", tab_id: src.tabId, reason: t.detached });   // not FERB's own close
});

// Dialogs (13.5): a background tab's dialog is invisible to the owner, so the extension answers it: dismissed,
// unless the act in flight declared it expects one; beforeunload is accepted only while FERB itself leaves the page.
chrome.debugger.onEvent.addListener((src, method, params) => {
  if (method !== "Page.javascriptDialogOpening") return;
  const t = TABS.get(src.tabId);
  if (!t) return;
  const d = { type: params.type, message: String(params.message || "").slice(0, 300), default: String(params.defaultPrompt || "") };
  let accept = false, promptText;
  if (params.type === "beforeunload") accept = t.leaving;
  else if (t.expect) { accept = t.expect.accept; promptText = t.expect.text || undefined; }
  d.handled = accept ? "accepted" : "dismissed";
  t.dialogs.push(d); if (t.dialogs.length > 10) t.dialogs.shift();
  send("event", { event: "dialog", tab_id: src.tabId, ...d });
  const p = { accept, ...(promptText !== undefined ? { promptText } : {}) };
  cdp(src.tabId, "Page.handleJavaScriptDialog", p).catch(() => {});
});

chrome.tabs.onRemoved.addListener((tabId) => TABS.delete(tabId));

// Trusted input needs a readiness probe right after attaching: the tab drops presses until a probe move lands.
async function ready(tabId) {
  await inFrame(tabId, 0, () => { window.__ferb.probeArm(); return true; }, []);
  for (let i = 0; i < 20; i++) {
    await cdp(tabId, "Input.dispatchMouseEvent", { type: "mouseMoved", x: 2 + (i % 3), y: 2 });
    if (await inFrame(tabId, 0, () => window.__ferb.probeSeen(), [])) return;
    await sleep(100);
  }
  throw new Error("tab never became ready for input");
}

// ---------- trusted input (13.4, 13.6) ----------

async function mouse(tabId, type, x, y, extra) {
  await cdp(tabId, "Input.dispatchMouseEvent", { type, x, y, ...(extra || {}) });
}
async function pointerClick(tabId, x, y, { button = "left", count = 1, modifiers = 0 } = {}) {
  await mouse(tabId, "mouseMoved", x, y, { modifiers });
  for (let n = 1; n <= count; n++) {
    await mouse(tabId, "mousePressed", x, y, { button, clickCount: n, modifiers });
    await mouse(tabId, "mouseReleased", x, y, { button, clickCount: n, modifiers });
  }
}
async function pressKey(tabId, spec) {
  const k = keyEvent(spec);
  if (!k) return false;
  const base = { key: k.key, code: k.code, windowsVirtualKeyCode: k.keyCode, nativeVirtualKeyCode: k.keyCode, modifiers: k.modifiers };
  const down = { ...base, type: k.text !== undefined ? "keyDown" : "rawKeyDown" };
  if (k.text !== undefined) { down.text = k.text; down.unmodifiedText = k.text; }
  if (k.commands) down.commands = k.commands;
  await cdp(tabId, "Input.dispatchKeyEvent", down);
  await cdp(tabId, "Input.dispatchKeyEvent", { ...base, type: "keyUp" });
  return true;
}

// Element centre in top-viewport coordinates: the frame's own rect plus the frame offset from the last snapshot.
async function topPoint(tabId, frameId, snap, el) {
  const r = await inFrame(tabId, frameId, (g, i) => window.__ferb.rect(g, i), [snap, el]);
  if (r.error) return { error: r.error };
  if (frameId !== 0) {
    const f = (TABS.get(tabId) || {}).frames || {};
    if (!f[frameId]) return { error: "frame_offset_unknown" };
    r.x += f[frameId][0]; r.y += f[frameId][1];
  }
  return r;
}

const CLICKS = { click: { button: "left", count: 1 }, dblclick: { button: "left", count: 2 }, rightclick: { button: "right", count: 1 } };

async function trusted(tabId, a) {
  const t = await attach(tabId);
  t.expect = a.expect_dialog ? { accept: a.expect_dialog === "accept", text: a.dialog_text || "" } : null;
  try {
    const p = await topPoint(tabId, a.frame_id, a.snap, a.el);
    if (p.error) return { ok: false, detail: p.error };
    if (a.kind === "hover") { await mouse(tabId, "mouseMoved", p.x, p.y); return { ok: true, detail: "hovered (trusted)" }; }
    if (CLICKS[a.kind]) {
      await pointerClick(tabId, p.x, p.y, CLICKS[a.kind]);
      const name = a.kind === "dblclick" ? "double-clicked" : a.kind === "rightclick" ? "right-clicked" : "clicked";
      return { ok: true, detail: `${name} (trusted)${p.occluded ? "; the element looked covered" : ""}` };
    }
    if (a.kind === "type") {
      if (p.editable) await pointerClick(tabId, p.x, p.y);          // focus by a real click, like a person
      let active = await inFrame(tabId, a.frame_id, (g, i) => window.__ferb.isActive(g, i), [a.snap, a.el]);
      if (!active) active = await inFrame(tabId, a.frame_id, (g, i) => window.__ferb.focus(g, i), [a.snap, a.el]);
      if (!active) return { ok: false, detail: "focus_failed" };     // never type into whatever else has focus
      const text = a.text || "";
      if (!a.append) await pressKey(tabId, "cmd+a");                  // replace: select the field's text first
      if (!text) { await pressKey(tabId, "Backspace"); return { ok: true, detail: "cleared (trusted)" }; }
      if (a.per_key) {
        for (const ch of text) if (!(await pressKey(tabId, ch))) await cdp(tabId, "Input.insertText", { text: ch });
        return { ok: true, detail: "typed key by key (trusted)" };
      }
      await cdp(tabId, "Input.insertText", { text });
      return { ok: true, detail: "typed (trusted)" };
    }
    if (a.kind === "press") {
      const active = await inFrame(tabId, a.frame_id, (g, i) => window.__ferb.isActive(g, i), [a.snap, a.el]);
      if (!active) await inFrame(tabId, a.frame_id, (g, i) => window.__ferb.focus(g, i), [a.snap, a.el]);
      if (!(await pressKey(tabId, a.text || "Enter"))) return { ok: false, detail: "unsupported_key" };
      return { ok: true, detail: `pressed ${a.text || "Enter"} (trusted)` };
    }
    return { ok: false, detail: "trusted input supports click, dblclick, rightclick, hover, type and press" };
  } finally {
    setTimeout(() => { if (TABS.get(tabId) === t) t.expect = null; }, 1000);   // a dialog may follow the act briefly
  }
}

// AX fallback (13.3): canvas / ARIA pages where the DOM walk finds almost nothing. Nodes with a name and an
// actionable role, rects from DOM.getBoxModel; ids from AX_BASE up are acted on by coordinates, never by element.
const AX_BASE = 100000, AX_MAX = 120;
const AX_ROLES = new Set(["button", "link", "textbox", "searchbox", "combobox", "checkbox", "radio", "menuitem", "tab",
  "option", "switch", "slider", "treeitem", "listitem", "row", "cell", "gridcell", "heading", "menuitemcheckbox", "menuitemradio"]);
async function axElements(tabId, gen) {
  const { nodes } = await cdp(tabId, "Accessibility.getFullAXTree", { depth: 60 });
  const out = [], map = {};
  for (const n of nodes || []) {
    if (out.length >= AX_MAX) break;
    const role = n.role && n.role.value, name = n.name && n.name.value;
    if (!role || !AX_ROLES.has(role) || !name || !n.backendDOMNodeId || n.ignored) continue;
    if ((n.properties || []).some((p) => p.name === "hidden" && p.value && p.value.value === true)) continue;
    let model;
    try { ({ model } = await cdp(tabId, "DOM.getBoxModel", { backendNodeId: n.backendDOMNodeId })); } catch (_) { continue; }
    if (!model || !model.content) continue;
    const q = model.content, xs = [q[0], q[2], q[4], q[6]], ys = [q[1], q[3], q[5], q[7]];
    const x = Math.min(...xs), y = Math.min(...ys), w = Math.max(...xs) - x, h = Math.max(...ys) - y;
    if (w <= 0 || h <= 0) continue;
    const id = AX_BASE + out.length;
    map[id] = n.backendDOMNodeId;
    const meets = x + w > 0 && y + h > 0 && x < VIEWPORT.width && y < VIEWPORT.height;
    out.push({ id, role, label: String(name).slice(0, 120), value: n.value && n.value.value != null ? String(n.value.value).slice(0, 200) : null,
               rect: [x, y, w, h], in_view: meets, input_type: null, href: null, form: null, secret: false, ax: true,
               ...(meets ? {} : { offscreen: y >= VIEWPORT.height ? "below" : "above" }) });
  }
  tabState(tabId).ax = { gen, map };
  return out;
}
async function axAct(tabId, a) {
  const t = await attach(tabId);
  const ax = t.ax;
  if (!ax || ax.gen !== a.snap || !ax.map[a.el]) return { ok: false, detail: "stale" };
  const { model } = await cdp(tabId, "DOM.getBoxModel", { backendNodeId: ax.map[a.el] });
  const q = model.content, x = (q[0] + q[4]) / 2, y = (q[1] + q[5]) / 2;
  if (CLICKS[a.kind]) { await pointerClick(tabId, x, y, CLICKS[a.kind]); return { ok: true, detail: `${a.kind} (trusted, by position)` }; }
  if (a.kind === "hover") { await mouse(tabId, "mouseMoved", x, y); return { ok: true, detail: "hovered (trusted, by position)" }; }
  if (a.kind === "type") {
    await pointerClick(tabId, x, y);
    if (!a.append) await pressKey(tabId, "cmd+a");
    await cdp(tabId, "Input.insertText", { text: a.text || "" });
    return { ok: true, detail: "typed (trusted, by position)" };
  }
  if (a.kind === "press") { await pointerClick(tabId, x, y); await pressKey(tabId, a.text || "Enter"); return { ok: true, detail: `pressed ${a.text || "Enter"} (trusted)` }; }
  return { ok: false, detail: "that action needs a DOM element" };
}

// Child-frame rects become top-viewport rects: a frame's offset is its <iframe> element's rect in the parent
// (matched by URL, then by name) plus the parent's own offset (2.5).
function composeFrames(frames) {
  const byId = new Map(frames.map((f) => [f.frame_id, f]));
  const offsets = {}, visible = {};
  if (byId.has(0)) { offsets[0] = [0, 0]; visible[0] = true; }
  for (let pass = 0; pass < 8; pass++) {
    let progress = false;
    for (const f of frames) {
      if (f.frame_id in offsets) continue;
      for (const p of frames) {
        if (!(p.frame_id in offsets)) continue;
        const hit = (p.iframes || []).find((i) => i.src && i.src === f.url) || (p.iframes || []).find((i) => f.name && i.name === f.name);
        if (!hit) continue;
        offsets[f.frame_id] = [offsets[p.frame_id][0] + hit.rect[0], offsets[p.frame_id][1] + hit.rect[1]];
        visible[f.frame_id] = visible[p.frame_id] && hit.in_view;
        progress = true;
        break;
      }
    }
    if (!progress) break;
  }
  for (const f of frames) {
    const o = offsets[f.frame_id];
    f.offset = o || null;
    if (!o) { f.offset_unknown = true; continue; }
    for (const e of f.elements) {
      e.rect = [e.rect[0] + o[0], e.rect[1] + o[1], e.rect[2], e.rect[3]];
      if (f.frame_id !== 0) e.in_view = !!(e.in_view && visible[f.frame_id]);
    }
    for (const s of f.scrollers || []) s.rect = [s.rect[0] + o[0], s.rect[1] + o[1], s.rect[2], s.rect[3]];
  }
  return offsets;
}

// Wheel scrolls settle asynchronously: read the position until two reads agree.
async function settled(tabId, cid) {
  let last = null;
  for (let i = 0; i < 8; i++) {
    await sleep(60);
    const s = await inFrame(tabId, 0, (c) => window.__ferb.scrolled(c), [cid || null]);
    if (s.error) return s;
    if (last && last.y === s.y) return s;
    last = s;
  }
  return last;
}

const SHOT_PART = 600000;        // base64 chars per message: well under the 1 MB native-messaging line

const commands = {
  async open({ url }, id) {
    const windowId = await ferbWindow();
    const tab = await chrome.tabs.create({ windowId, url, active: false });      // never active, never focused (E3)
    const s = await session(); s.owned.push({ id, tabId: tab.id }); await saveSession(s);   // tagged with its command
    try { await chrome.tabs.update(tab.id, { autoDiscardable: false }); } catch (_) { /* Memory Saver opt-out is best effort */ }
    try { await ferbGroup(tab.id, tab.windowId); } catch (_) { /* grouping is cosmetic */ }
    await waitComplete(tab.id, 20000);
    const t = await chrome.tabs.get(tab.id);
    return { tab_id: t.id, url: maskUrl(t.url || t.pendingUrl || ""), title: t.title || "", window_id: t.windowId };
  },
  async navigate({ tab_id, url }) {
    const st = tabState(tab_id);
    st.leaving = true; st.frames = {};
    const started = new Promise((resolve) => {        // the old page is still "complete": wait for the new load
      const on = (id, info) => { if (id === tab_id && info.status === "loading") { chrome.tabs.onUpdated.removeListener(on); resolve(); } };
      chrome.tabs.onUpdated.addListener(on);
      setTimeout(() => { chrome.tabs.onUpdated.removeListener(on); resolve(); }, 3000);
    });
    try {
      await chrome.tabs.update(tab_id, { url });
      await started;
      await waitComplete(tab_id, 20000);
    } finally { st.leaving = false; }
    const t = await chrome.tabs.get(tab_id);
    return { tab_id: t.id, url: maskUrl(t.url || t.pendingUrl || ""), title: t.title || "" };
  },
  async close({ tab_id }) {
    const st = TABS.get(tab_id);
    if (st) st.leaving = true;
    try { await chrome.tabs.remove(tab_id); }
    catch (e) { if (!/No tab with id/i.test(String(e && e.message))) throw e; }
    await detach(tab_id);
    const s = await session(); s.owned = s.owned.filter((o) => o.tabId !== tab_id); await saveSession(s);
    return { closed: true };
  },
  // Trusted wheel at the scroller's centre (13.4): the window, or a container from the latest snapshot.
  async scroll({ tab_id, direction, container, dy }) {
    const cid = container || null;
    let target;
    try { await attach(tab_id); target = await inFrame(tab_id, 0, (c) => window.__ferb.scroller(c), [cid]); }
    catch (e) {
      if (String(e && e.message).startsWith("detached:")) throw e;
      target = null;                                   // no debugger on this page (e.g. a browser page): page mode
    }
    if (target && target.error) throw new Error(target.error === "stale" ? "that container is from an older snapshot" : target.error);
    if (target) {
      const step = Number.isFinite(dy) ? dy : Math.round(target.viewport_h * 0.85) * (direction === "up" ? -1 : 1);
      // the move first: under the device-metrics override a bare wheel isn't targeted at the scroller under it
      // (lab probe 2026-10-02: box 0 without the move, 170 with it; the window scrolled instead)
      await cdp(tab_id, "Input.dispatchMouseEvent", { type: "mouseMoved", x: target.px, y: target.py });
      await cdp(tab_id, "Input.dispatchMouseEvent", { type: "mouseWheel", x: target.px, y: target.py, deltaX: 0, deltaY: step });
      const s = await settled(tab_id, cid);
      if (!s.error) return { container: cid, y: s.y, height: s.height, viewport_h: s.viewport_h, at_end: direction === "up" ? s.y <= 0 : s.at_end };
    }
    const [r] = await chrome.scripting.executeScript({ target: { tabId: tab_id },
      func: (d) => {
        const el = document.scrollingElement || document.documentElement;
        window.scrollBy(0, (d === "up" ? -1 : 1) * Math.round(innerHeight * 0.85));
        return { y: Math.round(el.scrollTop), height: el.scrollHeight, viewport_h: innerHeight,
                 at_end: d === "up" ? el.scrollTop <= 0 : el.scrollTop + innerHeight >= el.scrollHeight - 2 };
      }, args: [direction] });
    return { container: null, ...(r && r.result ? r.result : { y: 0, height: 0, viewport_h: 0, at_end: false }) };
  },
  async find({ tab_id, text }) {                   // the whole page's text, not just the snapshot's first screens
    const [r] = await chrome.scripting.executeScript({ target: { tabId: tab_id },
      func: (q) => {
        const needle = (q || "").toLowerCase();
        const lines = (document.body ? document.body.innerText : "").split(/\n+/).map((l) => l.trim()).filter(Boolean);
        const hits = needle ? lines.filter((l) => l.toLowerCase().includes(needle)) : [];
        return { count: hits.length, lines: lines.length, examples: hits.slice(0, 20).map((l) => l.slice(0, 160)) };
      }, args: [text] });
    return r && r.result ? r.result : { count: 0, lines: 0, examples: [] };
  },
  async list_tabs() {
    const tabs = await chrome.tabs.query({}), s = await session();
    return { tabs: tabs.map((t) => ({ tab_id: t.id, host: (() => { try { return new URL(t.url).hostname; } catch (_) { return ""; } })(),
                                      active: t.active, window_id: t.windowId, ferb: s.windowId != null && t.windowId === s.windowId })) };
  },
  async snapshot({ tab_id, snap }) {
    await inject(tab_id);
    const results = await chrome.scripting.executeScript({ target: { tabId: tab_id, allFrames: true },
                                                           func: (g) => window.__ferb.snapshot(g), args: [snap] });
    const frames = results.filter((r) => r && r.result).map((r) => ({ frame_id: r.frameId, ...r.result }));
    const top = frames.find((f) => f.frame_id === 0);
    if (!top) throw new Error("snapshot failed in the page");
    const st = tabState(tab_id);
    st.frames = composeFrames(frames);
    st.ax = null;
    if (top.elements.filter((e) => e.role !== "heading").length < 3) {       // canvas / ARIA app: AX fallback
      const s = await session(), tab = await chrome.tabs.get(tab_id);          // attaching FERB's own tab is invisible;
      if (st.attached || (s.windowId != null && tab.windowId === s.windowId)) {   // an owner tab only once attached
        try { await attach(tab_id); top.elements = top.elements.concat(await axElements(tab_id, snap)); top.ax = true; }
        catch (_) { /* the DOM walk stands */ }
      }
    }
    const dialogs = st.dialogs.splice(0);
    return { frames, dialogs };
  },
  async act({ tab_id, frame_id, snap, el, kind, text, page_mode, expect_dialog, dialog_text, append, per_key }) {
    if (el >= AX_BASE) return axAct(tab_id, { snap, el, kind, text, append });
    const page = page_mode === true || kind === "select" || kind === "scroll";
    if (!page) {
      let r = { ok: false, detail: "no_debugger" };
      let attached = false;
      try { await attach(tab_id); attached = true; }
      catch (e) { if (String(e && e.message).startsWith("detached:")) throw e; }   // else: a page it can't attach to
      if (attached) r = await trusted(tab_id, { frame_id, snap, el, kind, text, expect_dialog, dialog_text, append, per_key });
      if (r.ok || (r.detail !== "frame_offset_unknown" && r.detail !== "no_debugger")) return r;
      // real input can't reach it (a frame FERB can't place, or a page the debugger may not attach to): page events
    }
    const r = await inFrame(tab_id, frame_id, (g, i, k, t) => window.__ferb.act(g, i, k, t), [snap, el, kind, text ?? null]);
    return r;
  },
  // DOM.setFileInputFiles on the stamped element (13.7): no dialog, top frame only (DOM.querySelector doesn't pierce).
  async upload({ tab_id, frame_id, snap, el, paths }) {
    if (frame_id !== 0) return { ok: false, detail: "uploads work in the top frame only" };
    const token = Array.from(crypto.getRandomValues(new Uint8Array(8)), (b) => b.toString(16).padStart(2, "0")).join("");
    const s = await inFrame(tab_id, 0, (g, i, t) => window.__ferb.stamp(g, i, t), [snap, el, token]);
    if (!s.ok) return s;
    try {
      await attach(tab_id);
      const { root } = await cdp(tab_id, "DOM.getDocument", { depth: 0 });
      const { nodeId } = await cdp(tab_id, "DOM.querySelector", { nodeId: root.nodeId, selector: `[data-ferb-node="${token}"]` });
      if (!nodeId) return { ok: false, detail: "stale" };
      await cdp(tab_id, "DOM.setFileInputFiles", { nodeId, files: paths });
      return { ok: true, detail: `${paths.length} file(s) attached` };
    } finally {
      try { await inFrame(tab_id, 0, (t) => window.__ferb.unstamp(t), [token]); } catch (_) {}
    }
  },
  async wait({ tab_id, until, value, _timeout_ms }) {
    const deadline = Date.now() + Math.max(250, (_timeout_ms || 30000) - 500);   // answers before the daemon gives up
    while (Date.now() < deadline) {
      let t;
      try { t = await chrome.tabs.get(tab_id); } catch (_) { return { met: false, detail: "tab closed" }; }
      if (until === "load" && t.status === "complete") return { met: true };
      if (until === "url" && (t.url || "").includes(value || "")) return { met: true };
      if (until === "text" && t.status === "complete") {
        try { if ((await inFrame(tab_id, 0, () => window.__ferb.text(), [])).includes(value || "")) return { met: true }; }
        catch (_) { /* page between documents */ }
      }
      await sleep(250);
    }
    return { met: false, detail: "timeout" };
  },
  async account({ tab_id }) { return inFrame(tab_id, 0, () => window.__ferb.account(), []); },
  // jpeg, and split into parts when one message would come near the 1 MB native-messaging line (1.4).
  async screenshot({ tab_id, format, quality }) {
    await attach(tab_id);
    const { data } = await cdp(tab_id, "Page.captureScreenshot", { format: format || "jpeg", quality: quality || 70 });
    const st = tabState(tab_id);
    if (data.length <= SHOT_PART) { st.shot = null; return { data, format: format || "jpeg" }; }
    st.shot = data;
    return { parts: Math.ceil(data.length / SHOT_PART), size: data.length, format: format || "jpeg" };
  },
  async screenshot_part({ tab_id, part }) {
    const st = TABS.get(tab_id);
    if (!st || st.shot == null) throw new Error("no screenshot to continue");
    const data = st.shot.slice(part * SHOT_PART, (part + 1) * SHOT_PART);
    if ((part + 1) * SHOT_PART >= st.shot.length) st.shot = null;
    return { data };
  },
  async ping() { await chrome.storage.session.get("s"); return { ok: true }; },   // any API call resets the idle timer
  async frame_origin({ tab_id, frame_id }) {
    return { origin: await inFrame(tab_id, frame_id, () => window.__ferb.origin(), []) };
  },
  // FERB identity only (the daemon never sends it to owner profiles). The value is opened here, for this command id
  // and origin only, handed to that frame's isolated world, and never posted back or logged.
  async fill({ tab_id, frame_id, snap, el, sealed, origin }, id) {
    const st = await install();
    if (!st.key) throw new Error("not paired");
    let value;
    try { value = await unseal(st.key, sealed, id, origin); }
    catch (_) { return { ok: false, detail: "seal_refused" }; }            // another command or origin: nothing filled
    const r = await inFrame(tab_id, frame_id, (g, i, o, v) => window.__ferb.fill(g, i, o, v), [snap, el, origin, value]);
    value = null;
    return { ok: !!r.ok, detail: String(r.detail || "") };
  },
  async read_source({ tab_id }, id) {
    const t = await chrome.tabs.get(tab_id);
    const u = new URL(t.url || "about:blank");
    if (`${u.origin}${u.pathname}` !== FEED_URL) throw new Error("read_source is only for FERB's inbox feed");
    const [r] = await chrome.scripting.executeScript({ target: { tabId: tab_id },
      func: () => (document.documentElement ? document.documentElement.outerHTML.slice(0, 500000) : "") });
    const st = await install();
    if (!st.key) throw new Error("not paired");
    return { sealed: await seal(st.key, (r && r.result) || "", id, FEED_URL) };   // email content never in clear
  },
  // Screenshot check (build step 9): does the active tab of this profile's window at exactly these screen bounds
  // hold a password field, in any frame? Only yes / no / can't tell comes back, never page content.
  async window_password({ bounds }) {
    const [x, y, w, h] = bounds;
    const wins = await chrome.windows.getAll({ windowTypes: ["normal", "popup"] });
    const win = wins.find((v) => Math.abs(v.left - x) <= 2 && Math.abs(v.top - y) <= 2 &&
                                 Math.abs(v.width - w) <= 2 && Math.abs(v.height - h) <= 2);
    if (!win) return { matched: false, password: null };
    const [tab] = await chrome.tabs.query({ active: true, windowId: win.id });
    if (!tab) return { matched: true, password: null };
    try {
      const rs = await chrome.scripting.executeScript({ target: { tabId: tab.id, allFrames: true },
        func: () => !!document.querySelector('input[type="password" i]') });
      const all = rs.map((r) => r.result);
      if (all.some((v) => v === true)) return { matched: true, password: true };
      return { matched: true, password: all.length && all.every((v) => v === false) ? false : null };
    } catch (_) {
      return { matched: true, password: null };                 // e.g. a browser page scripts can't read
    }
  },
  async wake_close({ url }) {            // close exactly the unique wake page (spike 13)
    const tabs = (await chrome.tabs.query({})).filter((t) => (t.url || t.pendingUrl) === url);
    for (const t of tabs) await chrome.tabs.remove(t.id);
    return { closed: tabs.length };
  },
};

// ---------- tab events (paired installs only; the daemon refuses unpaired ones anyway) ----------

async function tabEvent(event, tab) {
  const st = await install();
  if (!st.key) return;
  await send("tab_event", { event, tab_id: tab.id, url: tab.url ? maskUrl(tab.url) : null, title: tab.title || null,
                            active: tab.active ?? null, incognito: tab.incognito ?? null });
}
chrome.tabs.onCreated.addListener((t) => tabEvent("created", t));
chrome.tabs.onUpdated.addListener((id, info, t) => { if (info.status === "complete") { ensurePort(); tabEvent("updated", t); } });
chrome.tabs.onActivated.addListener(async ({ tabId }) => { try { tabEvent("activated", await chrome.tabs.get(tabId)); } catch (_) {} });
chrome.tabs.onRemoved.addListener((tabId) => tabEvent("removed", { id: tabId }));

ensurePort();
