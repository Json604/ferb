// FERB spike 13: survive service-worker death without replaying commands. Hands, not brains.
const HOST = "com.ferb.spike13";
const BOOT_ID = crypto.randomUUID();       // new every time this worker starts
let port = null;
let gen = null;                            // connection generation, assigned by the daemon per connect

// Wake-ups: browser start, install, a periodic alarm, any tab load.
chrome.runtime.onStartup.addListener(() => ensurePort());
chrome.runtime.onInstalled.addListener(() => ensurePort());
chrome.alarms.create("ferb-reconnect", { periodInMinutes: 0.5 });
chrome.alarms.onAlarm.addListener(() => ensurePort());
chrome.tabs.onUpdated.addListener((_id, info) => { if (info.status === "complete") ensurePort(); });

// Survives worker restarts within the browser session (not browser restarts).
async function session() {
  const { s } = await chrome.storage.session.get("s");
  return s || { seen: [], owned: [] };
}
async function saveSession(s) { await chrome.storage.session.set({ s }); }

function post(msg) { if (port) port.postMessage(msg); }

async function ownedTabs() {
  const s = await session();
  const alive = [];
  for (const o of s.owned) {
    try { const t = await chrome.tabs.get(o.tabId); alive.push({ id: o.id, tabId: t.id, url: t.url || t.pendingUrl }); }
    catch (_) { /* closed */ }
  }
  return alive;
}

const commands = {
  async open({ url }, id) {
    const t = await chrome.tabs.create({ url, active: false });
    const s = await session(); s.owned.push({ id, tabId: t.id }); await saveSession(s);
    return { tabId: t.id };
  },
  async slow_open({ url, delay_ms }, id) {        // effect first, reply later: a window to die in
    const r = await commands.open({ url }, id);
    await new Promise((res) => setTimeout(res, delay_ms));
    return r;
  },
  async delayed_open({ url, delay_ms }, id) {     // wait first, then act: dying here means no effect
    await new Promise((res) => setTimeout(res, delay_ms));
    return commands.open({ url }, id);
  },
  async list_tabs() { return { tabs: await ownedTabs() }; },
  async close({ tabId }) { await chrome.tabs.remove(tabId); return {}; },
};

async function onCommand(m) {
  if (m.gen !== gen) { post({ type: "stale", id: m.id, gen: m.gen, current: gen }); return; }  // never replay old-gen
  const s = await session();
  if (s.seen.includes(m.id)) { post({ type: "duplicate", id: m.id, gen }); return; }
  s.seen.push(m.id); s.seen = s.seen.slice(-500); await saveSession(s);   // record BEFORE executing
  try {
    post({ type: "result", id: m.id, gen, result: await commands[m.name](m.args || {}, m.id) });
  } catch (e) {
    post({ type: "error", id: m.id, gen, error: String((e && e.message) || e) });
  }
}

async function ensurePort() {
  if (port) return;
  port = chrome.runtime.connectNative(HOST);
  gen = null;
  port.onMessage.addListener((m) => {
    if (m.type === "welcome") { gen = m.gen; return; }
    if (m.type === "cmd") onCommand(m);
  });
  port.onDisconnect.addListener(() => { void chrome.runtime.lastError; port = null; gen = null; });
  // Re-announce state on every (re)connect.
  post({ type: "hello", boot_id: BOOT_ID, version: chrome.runtime.getManifest().version, owned: await ownedTabs() });
}

ensurePort();
