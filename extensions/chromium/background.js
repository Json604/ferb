// FERB service worker: FERB's hands in the browser. Executes daemon commands and reports state; holds no policy
// and no task logic (ARCHITECTURE.md 9.4). Reliability (spike 13): commands carry the connection generation; ids
// are recorded as seen BEFORE executing (storage.session survives worker restarts); old-generation commands are
// answered "stale" and never run; duplicates run once; hello re-announces owned tabs and seen ids.
importScripts("protocol.js");

let port = null;
let gen = null;                      // connection generation, assigned by the daemon ("welcome" / "paired")
let retryMs = 1000;
let refused = null;                  // the daemon refused this install (e.g. protocol mismatch): stop retrying
const outbox = [];                   // queued while disconnected (bounded)
const OUTBOX_MAX = 200;
const DEBUGGEE = new Set();          // tabs this worker attached the debugger to

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
  return s || { seen: {}, order: [], owned: [] };
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
// Posted inside the lock, so messages leave in seq order.
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
    if (port) port.postMessage(msg);
    else { outbox.push(msg); if (outbox.length > OUTBOX_MAX) outbox.shift(); ensurePort(); }
  });
}

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
  retryMs = 1000;
  const st = await install(), s = await session();
  await send("hello", { version: chrome.runtime.getManifest().version, protocol: FERB_PROTOCOL,
                        browser: navigator.brave ? "brave" : "chrome", paired: !!st.key,
                        owned: await ownedTabs(), seen: s.order.slice(-200).map((id) => ({ id, status: s.seen[id] })) });
  while (outbox.length && port) port.postMessage(outbox.shift());
}

async function onDaemon(m) {
  if (m.type === "welcome") { gen = m.gen; return; }
  if (m.type === "paired" && m.key) {
    await serially(async () => {
      const st = await install();
      st.key = m.key; st.seq = 0;
      await chrome.storage.local.set({ ferb: st });
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
    const value = await withTimeout(handler(m.args || {}, m.id), m.timeout_ms || 30000);
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

async function debuggerOn(tabId) {
  if (!DEBUGGEE.has(tabId)) { await chrome.debugger.attach({ tabId }, "1.3"); DEBUGGEE.add(tabId); }
  await chrome.debugger.sendCommand({ tabId }, "Emulation.setFocusEmulationEnabled", { enabled: true });
}
async function debuggerOff(tabId) {
  if (!DEBUGGEE.has(tabId)) return;
  DEBUGGEE.delete(tabId);
  try { await chrome.debugger.sendCommand({ tabId }, "Emulation.setFocusEmulationEnabled", { enabled: false }); } catch (_) {}
  try { await chrome.debugger.detach({ tabId }); } catch (_) {}
}
chrome.debugger.onDetach.addListener((src) => DEBUGGEE.delete(src.tabId));

async function cdp(tabId, method, params) {
  if (!cdpAllowed(method, params)) throw new Error(`cdp method not allowed: ${method}`);
  return cdpFilter(method, await chrome.debugger.sendCommand({ tabId }, method, params || {}));
}

// Trusted input needs focus emulation and a readiness probe: the tab drops input until a probe move lands.
async function ready(tabId) {
  await inFrame(tabId, 0, () => { window.__ferb.probeArm(); return true; }, []);
  for (let i = 0; i < 20; i++) {
    await cdp(tabId, "Input.dispatchMouseEvent", { type: "mouseMoved", x: 2 + (i % 3), y: 2 });
    if (await inFrame(tabId, 0, () => window.__ferb.probeSeen(), [])) return;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error("tab never became ready for input");
}

const KEYS = { Enter: [13, "\r"], Tab: [9, null], Escape: [27, null], Backspace: [8, null], Space: [32, " "],
               ArrowDown: [40, null], ArrowUp: [38, null], ArrowLeft: [37, null], ArrowRight: [39, null] };

async function trusted(tabId, a) {
  await debuggerOn(tabId);
  try {
    await ready(tabId);
    const r = await inFrame(tabId, 0, (g, i) => window.__ferb.rect(g, i), [a.snap, a.el]);
    if (r.error) return { ok: false, detail: r.error };
    if (a.kind === "click") {
      for (const type of ["mouseMoved", "mousePressed", "mouseReleased"])
        await cdp(tabId, "Input.dispatchMouseEvent", { type, x: r.x, y: r.y, button: "left", clickCount: 1 });
      return { ok: true, detail: "clicked (trusted)" };
    }
    if (a.kind === "type") { await cdp(tabId, "Input.insertText", { text: a.text || "" }); return { ok: true, detail: "typed (trusted)" }; }
    if (a.kind === "press") {
      const key = a.text || "Enter", [code, text] = KEYS[key] || [];
      if (code === undefined) return { ok: false, detail: "unsupported_key" };
      await cdp(tabId, "Input.dispatchKeyEvent", { type: "keyDown", key, windowsVirtualKeyCode: code, ...(text ? { text } : {}) });
      await cdp(tabId, "Input.dispatchKeyEvent", { type: "keyUp", key, windowsVirtualKeyCode: code });
      return { ok: true, detail: `pressed ${key} (trusted)` };
    }
    return { ok: false, detail: "trusted input supports click, type and press" };
  } finally {
    await debuggerOff(tabId);
  }
}

const commands = {
  async open({ url }, id) {
    let win;
    try { win = await chrome.windows.getLastFocused({ windowTypes: ["normal"] }); } catch (_) { win = null; }
    let tab;
    if (win) tab = await chrome.tabs.create({ url, active: false, windowId: win.id });
    else { const w = await chrome.windows.create({ url, focused: false }); tab = w.tabs[0]; }
    const s = await session(); s.owned.push({ id, tabId: tab.id }); await saveSession(s);   // tagged with its command
    try { await ferbGroup(tab.id, tab.windowId); } catch (_) { /* grouping is cosmetic */ }
    await waitComplete(tab.id, 20000);
    const t = await chrome.tabs.get(tab.id);
    return { tab_id: t.id, url: maskUrl(t.url || t.pendingUrl || ""), title: t.title || "" };
  },
  async navigate({ tab_id, url }) {
    const started = new Promise((resolve) => {        // the old page is still "complete": wait for the new load
      const on = (id, info) => { if (id === tab_id && info.status === "loading") { chrome.tabs.onUpdated.removeListener(on); resolve(); } };
      chrome.tabs.onUpdated.addListener(on);
      setTimeout(() => { chrome.tabs.onUpdated.removeListener(on); resolve(); }, 3000);
    });
    await chrome.tabs.update(tab_id, { url });
    await started;
    await waitComplete(tab_id, 20000);
    const t = await chrome.tabs.get(tab_id);
    return { tab_id: t.id, url: maskUrl(t.url || t.pendingUrl || ""), title: t.title || "" };
  },
  async close({ tab_id }) {
    await debuggerOff(tab_id);
    await chrome.tabs.remove(tab_id);
    const s = await session(); s.owned = s.owned.filter((o) => o.tabId !== tab_id); await saveSession(s);
    return { closed: true };
  },
  async scroll({ tab_id, direction }) {           // the whole page (a page may have no elements to act on)
    const [r] = await chrome.scripting.executeScript({ target: { tabId: tab_id },
      func: (d) => {
        const el = document.scrollingElement || document.documentElement;
        window.scrollBy(0, (d === "up" ? -1 : 1) * Math.round(innerHeight * 0.85));
        return { y: Math.round(el.scrollTop), height: el.scrollHeight,
                 at_end: d === "up" ? el.scrollTop <= 0 : el.scrollTop + innerHeight >= el.scrollHeight - 2 };
      }, args: [direction] });
    return r && r.result ? r.result : { y: 0, height: 0, at_end: false };
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
    const tabs = await chrome.tabs.query({});
    return { tabs: tabs.map((t) => ({ tab_id: t.id, host: (() => { try { return new URL(t.url).hostname; } catch (_) { return ""; } })(),
                                      active: t.active })) };
  },
  async snapshot({ tab_id, snap }) {
    await inject(tab_id);
    const results = await chrome.scripting.executeScript({ target: { tabId: tab_id, allFrames: true },
                                                           func: (g) => window.__ferb.snapshot(g), args: [snap] });
    const frames = results.filter((r) => r && r.result).map((r) => ({ frame_id: r.frameId, ...r.result }));
    if (!frames.some((f) => f.frame_id === 0)) throw new Error("snapshot failed in the page");
    return { frames };
  },
  async act({ tab_id, frame_id, snap, el, kind, text, trusted: wantTrusted }) {
    if (wantTrusted) {
      if (frame_id !== 0) return { ok: false, detail: "trusted input works in the top frame only" };
      return trusted(tab_id, { snap, el, kind, text });
    }
    return inFrame(tab_id, frame_id, (g, i, k, t) => window.__ferb.act(g, i, k, t), [snap, el, kind, text ?? null]);
  },
  async wait({ tab_id, until, value }) {
    for (;;) {
      const t = await chrome.tabs.get(tab_id);
      if (until === "load" && t.status === "complete") return { met: true };
      if (until === "url" && (t.url || "").includes(value || "")) return { met: true };
      if (until === "text" && t.status === "complete") {
        try { if ((await inFrame(tab_id, 0, () => window.__ferb.text(), [])).includes(value || "")) return { met: true }; }
        catch (_) { /* page between documents */ }
      }
      await new Promise((r) => setTimeout(r, 250));
    }
  },
  async account({ tab_id }) { return inFrame(tab_id, 0, () => window.__ferb.account(), []); },
  async screenshot({ tab_id }) {
    await debuggerOn(tab_id);
    try { return await cdp(tab_id, "Page.captureScreenshot", { format: "png" }); } finally { await debuggerOff(tab_id); }
  },
  async attach_debugger({ tab_id }) { await debuggerOn(tab_id); return {}; },
  async detach_debugger({ tab_id }) { await debuggerOff(tab_id); return {}; },
  async cdp({ tab_id, method, params }) {
    if (!DEBUGGEE.has(tab_id)) throw new Error("debugger not attached");
    return cdp(tab_id, method, params);
  },
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
