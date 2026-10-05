// FERB service worker: FERB's hands in the browser. Executes daemon commands and reports state; holds no policy
// and no task logic (ARCHITECTURE.md 9.4). Reliability (spike 13): commands carry the connection generation; ids
// are recorded as seen BEFORE executing (storage.session survives worker restarts); old-generation commands are
// answered "stale" and never run; duplicates run once; hello re-announces owned tabs and seen ids.
//
// Background by construction (PLATFORM_BROWSER.md 13): one FERB window per profile, born minimized and kept;
// every task tab is created inactive inside it; the debugger stays attached to a task tab for its life with focus
// emulation on, so trusted input, scrolling and screenshots work while the owner never sees a thing.
importScripts("protocol.js");
importScripts("ax.js");
importScripts("pointer.js");

let port = null;
let gen = null;                      // connection generation, assigned by the daemon ("welcome" / "paired")
let retryMs = 1000;
let refused = null;
let requiredRelease = null;          // structured daemon compatibility facts, never inferred from error prose
let activeCommands = 0;
let updateCheck = null;
let handshakeTimer = null;
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

// Only the small read/modify/write transaction is serialized, never an entire browser command.
let sessionLock = Promise.resolve();
function mutateSession(change) {
  const next = sessionLock.then(async () => {
    const s = await session();
    const result = change(s);
    await saveSession(s);
    return result;
  });
  sessionLock = next.catch(() => {});
  return next;
}

async function keptTabs() {
  const { ferbKeptTabs } = await chrome.storage.local.get('ferbKeptTabs');
  return Array.isArray(ferbKeptTabs) ? ferbKeptTabs : [];
}

function rememberKeptTab(tabId) {
  return serially(async () => {
    const ids = new Set(await keptTabs());
    ids.add(tabId);
    await chrome.storage.local.set({ferbKeptTabs: [...ids]});
  });
}

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

async function connectionStatus(state, error = "") {
  await chrome.storage.session.set({ferbConnection: {state, error: String(error).slice(0, 300),
    version: chrome.runtime.getManifest().version, protocol: FERB_PROTOCOL, at: Date.now()}});
}

function newerVersion(a, b) {
  const aa = a.split('.').map(Number), bb = b.split('.').map(Number);
  for (let i = 0; i < Math.max(aa.length, bb.length); i++) {
    if ((aa[i] || 0) !== (bb[i] || 0)) return (aa[i] || 0) > (bb[i] || 0);
  }
  return false;
}

async function checkExtensionUpdate() {
  if (updateCheck) return updateCheck;
  updateCheck = (async () => {
    // Only our local bundle. No remote code or browser/profile switching.
    const response = await fetch(chrome.runtime.getURL('manifest.json'), {cache: 'no-store'});
    if (!response.ok) return;
    const disk = await response.json(), current = chrome.runtime.getManifest().version;
    if (typeof disk.version !== 'string' || !/^\d+(?:\.\d+){0,3}$/.test(disk.version) ||
        newerVersion(current, disk.version)) return;
    // A cached worker can advertise the new manifest version while still running an older imported protocol.
    // The daemon's required release permits one reload even when the visible version number is unchanged.
    const repair = requiredRelease && requiredRelease.protocol > FERB_PROTOCOL &&
      !newerVersion(requiredRelease.version, disk.version);
    if (!newerVersion(disk.version, current) && !repair) return;
    if (activeCommands || [...TABS.values()].some(t => t.gesture || t.attached) || (await ownedTabs()).length) return;
    const {ferbUpdateAttempt} = await chrome.storage.local.get('ferbUpdateAttempt');
    const attempt = `${current}/p${FERB_PROTOCOL}->${disk.version}/p${requiredRelease?.protocol ?? FERB_PROTOCOL}`;
    if (ferbUpdateAttempt === attempt) return;  // a failed reload must not loop forever
    await chrome.storage.local.set({ferbUpdateAttempt: attempt});  // never overwrite pairing state
    await connectionStatus('updating', 'Loading local extension ' + disk.version);
    const live = await ownedTabs();
    if (activeCommands || [...TABS.values()].some(t => t.gesture || t.attached) || live.length) {
      await chrome.storage.local.set({ferbUpdateAttempt: null});
      return;
    }
    chrome.runtime.reload();
  })().catch(e => console.warn('FERB update check:', String(e.message || e))).finally(() => {updateCheck = null;});
  return updateCheck;
}

async function announceHello() {
  const st = await install(), s = await session();
  await send("hello", { version: chrome.runtime.getManifest().version, protocol: FERB_PROTOCOL,
                        browser: navigator.brave ? "brave" : "chrome", paired: !!st.key,
                        owned: await ownedTabs(), seen: s.order.slice(-200).map((id) => ({ id, status: s.seen[id] })) });
}

async function ensurePort() {
  if (port) return;
  try { port = chrome.runtime.connectNative(FERB_HOST); }
  catch (e) { port = null; await connectionStatus('disconnected', e.message || e); return; }
  const opened = port;
  let closeReason = '';
  gen = null;
  port.onMessage.addListener(onDaemon);
  port.onDisconnect.addListener(() => {
    const reason = chrome.runtime.lastError?.message || closeReason || 'Native connection closed';
    if (port !== opened) return;    // a late disconnect must not clear a replacement connection
    clearTimeout(handshakeTimer);
    port = null; gen = null;
    connectionStatus('disconnected', reason);
    console.warn('FERB disconnected:', reason);
    for (const [tabId, tab] of TABS) if (tab.gesture) cancelPointer(tabId, tab.gesture.id);
    setTimeout(ensurePort, retryMs);
    retryMs = Math.min(retryMs * 2, 60000);
  });
  handshakeTimer = setTimeout(() => {
    if (port === opened && gen === null) {
      closeReason = 'FERB did not answer the connection handshake within 15 seconds';
      opened.disconnect();
    }
  }, 15000);
  try { await announceHello(); }
  catch (e) { await connectionStatus('disconnected', e.message || e); opened.disconnect(); }
}

async function onDaemon(m) {
  if (m.type === "welcome") {
    clearTimeout(handshakeTimer); refused = null; requiredRelease = null; gen = m.gen; retryMs = 1000;
    await connectionStatus('connected'); return;
  }
  if (m.type === "pairing_required") {
    clearTimeout(handshakeTimer); gen = null; requiredRelease = null;
    await connectionStatus('pairing_required', 'This extension has no pairing; run ferb browsers pair for its profile');
    return;
  }
  if (m.type === "paired" && m.key) {
    clearTimeout(handshakeTimer); refused = null; requiredRelease = null;
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
  if (m.type === "refused") {
    clearTimeout(handshakeTimer); gen = null; refused = m.reason || "refused";
    requiredRelease = Number.isInteger(m.required_protocol) && m.required_protocol > 0 &&
      typeof m.minimum_version === 'string' && /^\d+(?:\.\d+){0,3}$/.test(m.minimum_version)
      ? {protocol: m.required_protocol, version: m.minimum_version} : null;
    await connectionStatus('refused', refused);
    await checkExtensionUpdate(); return;
  }
  if (m.type === "cmd") await onCommand(m);
}

chrome.runtime.onStartup.addListener(() => ensurePort());
chrome.runtime.onInstalled.addListener(() => ensurePort());
chrome.alarms.create("ferb-reconnect", { periodInMinutes: 0.5 });
chrome.alarms.onAlarm.addListener(async () => {
  await checkExtensionUpdate();
  if (port && refused) await announceHello();  // retry after daemon updates too; refusal is not permanent
  else await ensurePort();
});
chrome.runtime.onMessage.addListener((m) => { if (m && m.pairToken) send("pair", { token: m.pairToken }); });

// ---------- commands ----------

async function onCommand(m) {
  activeCommands++;
  try { await executeCommand(m); }
  finally { activeCommands--; }
}

async function executeCommand(m) {
  const claimed = await mutateSession(s => {
    if (m.gen !== gen) return 'stale';
    if (Object.hasOwn(s.seen, m.id)) return 'duplicate';
    s.seen[m.id] = 'started'; s.order.push(m.id);
    while (s.order.length > 500) delete s.seen[s.order.shift()];
    return 'claimed';
  });
  if (claimed !== 'claimed') {
    await send('result', {id:m.id, gen:m.gen, status:claimed}); return;
  }
  const handler = OPS.includes(m.op) ? commands[m.op] : null;
  let body;
  try {
    if (!handler) throw new Error(`unknown op ${m.op}`);
    const value = await withTimeout(handler({ ...(m.args || {}), _timeout_ms: m.timeout_ms || 30000 }, m.id),
                                    m.timeout_ms || 30000);
    body = { id: m.id, gen: m.gen, status: "ok", value };
  } catch (e) {
    if (m.op === "pointer") cancelPointer(m.args.tab_id, m.id);
    body = { id: m.id, gen: m.gen, status: "error", error: String((e && e.message) || e).slice(0, 300) };
  }
  // A handler error may follow partial input; a timeout does not cancel the renderer operation.
  const outcome = body.status === "ok" ? (body.value && body.value.ok === false ? "failed" : "done") : "unknown";
  body.outcome = outcome;
  await mutateSession(s => {if (Object.hasOwn(s.seen, m.id)) s.seen[m.id] = outcome;});
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

let windowCreation = null;
async function ferbWindow() {
  if (!windowCreation) windowCreation = getFerbWindow().finally(() => {windowCreation = null;});
  return windowCreation;
}

async function getFerbWindow() {
  const s = await session();
  if (s.windowId != null) {
    try {
      const w = await chrome.windows.get(s.windowId);
      // An owner may restore it to watch a handed-off class. Never minimize it again behind their back.
      return w.id;
    } catch (_) { /* gone: recreate */ }
  }
  // Born minimized (probe E5, S10); focused:false on top (probe E1). A blank placeholder tab keeps the window
  // alive when FERB's last task tab closes, so a window is created once per browser session, not per task.
  const w = await chrome.windows.create({ url: "about:blank", state: "minimized", focused: false });
  await mutateSession(s => {s.windowId = w.id;});
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

async function recordingState(tabId) {
  await inject(tabId);
  const results = await chrome.scripting.executeScript({target: {tabId, allFrames: true},
    func: () => window.__ferb.captureState()});
  const frames = results.filter(r => r && r.result).map(r => ({frameId:r.frameId, ...r.result}));
  // Missing/inaccessible child frames leave the capture inventory incomplete. AX binding gaps do not.
  if (!frames.some(r => r.frameId === 0) || frames.length !== 1 + frames.reduce((n,r) => n+r.frames,0))
    throw new Error('recording field inventory incomplete');
  return frames.sort((a,b) => a.frameId-b.frameId);
}

// ---------- the debugger session (13.2): attached on a task tab's first action, kept for its life ----------

const TABS = new Map();          // tabId -> { attached, detached, dialogs, expect, leaving, frames, ax, shot }
function tabState(tabId) {
  let t = TABS.get(tabId);
  if (!t) { t = { attached: false, detached: null, dialogs: [], expect: null, leaving: false, frames: {}, sessions: new Map(), axErrors: [], shot: null }; TABS.set(tabId, t); }
  return t;
}

async function cdp(tabId, method, params, sessionId) {
  if (!cdpAllowed(method, params)) throw new Error(`cdp method not allowed: ${method}`);
  const t = TABS.get(tabId);
  try {
    return cdpFilter(method, await chrome.debugger.sendCommand({ tabId, ...(sessionId ? {sessionId} : {}) }, method, params || {}));
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
  try {
    await cdp(tabId, "Page.enable", {});
    await cdp(tabId, "Emulation.setFocusEmulationEnabled", { enabled: true });
    await cdp(tabId, "Target.setAutoAttach", { autoAttach: true, waitForDebuggerOnStart: false, flatten: true });
    const s = await session(), tab = await chrome.tabs.get(tabId);
    if (s.windowId != null && tab.windowId === s.windowId)     // FERB's own window: layout independent of its size.
      await cdp(tabId, "Emulation.setDeviceMetricsOverride", { ...VIEWPORT, deviceScaleFactor: 1, mobile: false });
    await ready(tabId);                                        // the owner's own tab keeps its real viewport
    return t;
  } catch (e) {
    t.attached = false;
    try { await chrome.debugger.detach({tabId}); } catch (_) {}
    throw e;
  }
}

async function detach(tabId) {
  const t = TABS.get(tabId);
  if (!t || !t.attached) { TABS.delete(tabId); return; }
  // Keep the state until the browser acknowledges release. A failed detach is actionable, not success.
  t.leaving = true;
  try {
    await chrome.debugger.sendCommand({tabId}, 'Emulation.setFocusEmulationEnabled', {enabled:false});
    await chrome.debugger.detach({tabId});
  } catch (e) {
    t.leaving = false;
    throw e;
  }
  TABS.delete(tabId);
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
  if (method === "Target.attachedToTarget") {
    const t = tabState(src.tabId), sid = params.sessionId;
    if (params.targetInfo && params.targetInfo.type === "iframe") {
      t.sessions.set(sid, cdp(src.tabId, "Target.setAutoAttach",
        {autoAttach: true, waitForDebuggerOnStart: false, flatten: true}, sid).catch(e => {
          t.axErrors.push(String(e.message || e));
        }));
    }
    return;
  }
  if (method === "Target.detachedFromTarget") { tabState(src.tabId).sessions.delete(params.sessionId); return; }
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

chrome.tabs.onRemoved.addListener(tabId => {
  TABS.delete(tabId);
  mutateSession(s => {s.owned = s.owned.filter(o => o.tabId !== tabId);}).catch(e => console.warn('FERB tab cleanup:', String(e)));
  serially(async () => {
    const ids = await keptTabs();
    if (ids.includes(tabId)) await chrome.storage.local.set({ferbKeptTabs: ids.filter(id => id !== tabId)});
  }).catch(e => console.warn('FERB kept-tab cleanup:', String(e)));
});

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
  return toTopPoint(tabId, frameId, r);
}

async function toTopPoint(tabId, frameId, r) {
  if (frameId !== 0) {
    const refs = tabState(tabId).frameRefs;
    let fid = frameId;
    const visited = new Set();
    while (fid !== 0) {
      if (visited.has(fid)) return {error: "frame_cycle"};
      visited.add(fid);
      const f = refs && refs.get(fid);
      if (!f || f.parent_frame_id == null || !f.owner_binding) return {error: "frame_offset_unknown"};
      const owner = await inFrame(tabId, f.parent_frame_id, token => window.__ferb.frameRect(token), [f.owner_binding]);
      if (owner.error) return owner;
      r.x += owner.x; r.y += owner.y; r.occluded = r.occluded || owner.occluded;
      fid = f.parent_frame_id;
    }
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
    if (p.secret && a.kind === "type") return {ok: false, detail: "secret_field"};
    if (p.disabled) return {ok: false, detail: "target_disabled"};
    if (p.occluded) return { ok: false, detail: "target_occluded; take a fresh snapshot" };
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
      let active = await inFrame(tabId, a.frame_id, (g, i) => window.__ferb.isActive(g, i), [a.snap, a.el]);
      if (!active) active = await inFrame(tabId, a.frame_id, (g, i) => window.__ferb.focus(g, i), [a.snap, a.el]);
      if (!active) return { ok: false, detail: "focus_failed" };
      if (!(await pressKey(tabId, a.text || "Enter"))) return { ok: false, detail: "unsupported_key" };
      return { ok: true, detail: `pressed ${a.text || "Enter"} (trusted)` };
    }
    return { ok: false, detail: "trusted input supports click, dblclick, rightclick, hover, type and press" };
  } finally {
    setTimeout(() => { if (TABS.get(tabId) === t) t.expect = null; }, 1000);   // a dialog may follow the act briefly
  }
}

// Child-frame rects use the CDP frame owner bound to its isolated DOM reference, plus the parent offset.
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
        if (p.frame_id !== f.parent_frame_id) continue;
        const hit = (p.iframes || []).find((i) => i.binding && i.binding === f.owner_binding);
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
async function settled(tabId, frameId, cid) {
  let last = null;
  for (let i = 0; i < 8; i++) {
    await sleep(60);
    const s = await inFrame(tabId, frameId, (c) => window.__ferb.scrolled(c), [cid || null]);
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
    await mutateSession(s => {s.owned.push({id, tabId:tab.id});});
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
    if ((await keptTabs()).includes(tab_id)) {
      const released = await commands.release_tab({tab_id, keep_open:true});
      return {...released, closed:false};
    }
    const st = TABS.get(tab_id);
    if (st) st.leaving = true;
    try { await chrome.tabs.remove(tab_id); }
    catch (e) { if (!/No tab with id/i.test(String(e && e.message))) throw e; }
    TABS.delete(tab_id); // Closing the tab destroys its debugger target; onRemoved may arrive later.
    await mutateSession(s => {s.owned = s.owned.filter(o => o.tabId !== tab_id);});
    return { closed: true };
  },
  async release_tab({tab_id, keep_open = false}) {
    const tab = await chrome.tabs.get(tab_id);  // missing tab is not a successful keep-open handoff
    if (keep_open) await rememberKeptTab(tab_id);  // persist before detach/session cleanup, safe after lost ACK
    await detach(tab_id);
    await mutateSession(s => {s.owned = s.owned.filter(o => o.tabId !== tab_id);});
    return {tab_id, released:true, kept_open:keep_open || (await keptTabs()).includes(tab_id),
            url:maskUrl(tab.url || tab.pendingUrl || ''), window_id:tab.windowId};
  },
  // Trusted wheel at the scroller's centre (13.4): the window, or a container from the latest snapshot.
  async scroll({ tab_id, frame_id = 0, direction, container, dy }) {
    const cid = container || null;
    await attach(tab_id);
    const target = await inFrame(tab_id, frame_id, c => window.__ferb.scroller(c), [cid]);
    if (target.error) throw Error(target.error === 'stale' ? 'that container is from an older snapshot' : target.error);
    const point = await toTopPoint(tab_id, frame_id, {x:target.px, y:target.py, occluded:target.occluded});
    if (point.error || point.occluded) throw Error(point.error || 'scroll container is occluded');
    const step = Number.isFinite(dy) ? dy : Math.round(target.viewport_h * 0.85) * (direction === 'up' ? -1 : 1);
    await cdp(tab_id, 'Input.dispatchMouseEvent', {type:'mouseMoved', x:point.x, y:point.y});
    await cdp(tab_id, 'Input.dispatchMouseEvent', {type:'mouseWheel', x:point.x, y:point.y, deltaX:0, deltaY:step});
    const s = await settled(tab_id, frame_id, cid);
    if (s.error) throw Error(s.error);
    return {container:cid, frame_id, y:s.y, height:s.height, viewport_h:s.viewport_h,
            at_end:direction === 'up' ? s.y <= 0 : s.at_end};
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
    const tabs = await chrome.tabs.query({}), s = await session(), kept = new Set(await keptTabs());
    return { tabs: tabs.map((t) => ({ tab_id: t.id, host: (() => { try { return new URL(t.url).hostname; } catch (_) { return ""; } })(),
                                      active: t.active, window_id: t.windowId, kept_open:kept.has(t.id),
                                      ferb: !kept.has(t.id) && s.windowId != null && t.windowId === s.windowId })) };
  },
  async snapshot({ tab_id, snap }) {
    tabState(tab_id).visual = null;
    await inject(tab_id);
    const st = tabState(tab_id);
    st.axErrors = [];
    let attached = false;
    try { await attach(tab_id); attached = true; }
    catch (e) { st.axErrors.push(String(e.message || e).slice(0, 160)); }
    const results = await chrome.scripting.executeScript({ target: { tabId: tab_id, allFrames: true },
                                                           func: (g) => window.__ferb.snapshot(g), args: [snap] });
    const frames = results.filter((r) => r && r.result).map((r) => ({ frame_id: r.frameId, ...r.result }));
    const top = frames.find((f) => f.frame_id === 0);
    if (!top) throw new Error("snapshot failed in the page");
    if (attached) await accessibilitySnapshot(tab_id, snap, frames);
    st.frames = composeFrames(frames);
    st.frameRefs = new Map(frames.map(f => [f.frame_id, f]));
    if (st.axErrors.length) top.truncated = true;
    if (st.axErrors.length) top.text = `[Accessibility view incomplete: ${st.axErrors.join("; ")}]\n${top.text}`;
    const dialogs = st.dialogs.splice(0);
    return { frames, dialogs };
  },
  async point({tab_id, snap, x, y, image_width, image_height}) {
    const visual = tabState(tab_id).visual;
    await checkVisual(tab_id, visual);
    if (snap !== visual.snap || ![x,y,image_width,image_height].every(Number.isFinite) ||
        x < 0 || y < 0 || image_width <= 0 || image_height <= 0 || x >= image_width || y >= image_height)
      throw new Error("invalid_visual_point");
    const [width,height] = await inFrame(tab_id, 0, () => window.__ferb.viewport(), []);
    const point = await resolveVisualPoint(tab_id, snap, x * width / image_width, y * height / image_height);
    const token = crypto.randomUUID();
    if (visual.points.size >= 32) throw new Error("too_many_visual_points; take another screenshot");
    visual.points.set(token, point);
    return {...point, pointer: token};
  },
  pointer: exactPointer,
  async pointer_cancel({tab_id, command_id}) { return cancelPointer(tab_id, command_id); },
  async act({ tab_id, frame_id, snap, el, kind, text, page_mode, expect_dialog, dialog_text, append, per_key }) {
    const page = page_mode === true || kind === "select" || kind === "scroll";
    if (!page) {
      return trusted(tab_id, { frame_id, snap, el, kind, text, expect_dialog, dialog_text, append, per_key });
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
  async screenshot({ tab_id, format, quality, snap }) {
    await attach(tab_id);
    const before = snap == null ? null : JSON.stringify(await visualStates(tab_id));
    const { data } = await cdp(tab_id, "Page.captureScreenshot", { format: format || "jpeg", quality: quality || 70 });
    const st = tabState(tab_id);
    st.visual = null;
    if (before !== null) {
      if (before !== JSON.stringify(await visualStates(tab_id))) throw new Error("page_changed_during_screenshot");
      st.visual = {at: Date.now(), snap, states: before, points: new Map()};
    }
    if (data.length <= SHOT_PART) { st.shot = null; return { data, format: format || "jpeg" }; }
    st.shot = data;
    return { parts: Math.ceil(data.length / SHOT_PART), size: data.length, format: format || "jpeg" };
  },
  async record_frame({tab_id, part, capture_id}) {
    const st = tabState(tab_id);
    if (part !== undefined) {
      const saved = st.recordFrame;
      if (!saved || saved.id !== capture_id || !Number.isInteger(part) || part < 0 || part*SHOT_PART >= saved.data.length)
        throw new Error('recording frame expired or invalid part');
      const data = saved.data.slice(part*SHOT_PART, (part+1)*SHOT_PART);
      if ((part+1)*SHOT_PART >= saved.data.length) st.recordFrame = null;
      return {data};
    }
    await attach(tab_id);
    const check = async () => {
      const frames = await recordingState(tab_id);
      if (frames.some(f => f.secret || !f.complete)) throw new Error('recording sensitive field or incomplete inventory');
      return JSON.stringify(frames.map(f => [f.frameId,f.documentId]));
    };
    const before = await check();
    const {data} = await cdp(tab_id, 'Page.captureScreenshot',
      {format:'jpeg', quality:70, fromSurface:true, captureBeyondViewport:false});
    if (before !== await check()) throw new Error('recording document changed during capture');
    if (data.length <= SHOT_PART) return {data};
    const id = crypto.randomUUID(); st.recordFrame = {id, data};
    return {parts:Math.ceil(data.length/SHOT_PART), capture_id:id};
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
  async fill({ tab_id, frame_id, snap, el, sealed, origin, code_only }, id) {
    const st = await install();
    if (!st.key) throw new Error("not paired");
    let value;
    try { value = await unseal(st.key, sealed, id, origin); }
    catch (_) { return { ok: false, detail: "seal_refused" }; }            // another command or origin: nothing filled
    const r = await inFrame(tab_id, frame_id, (g, i, o, v, c) => window.__ferb.fill(g, i, o, v, c), [snap, el, origin, value, code_only === true]);
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
