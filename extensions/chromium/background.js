// Owns authenticated browser transport and exact-target CDP mechanics; must not select a source/account or infer permission.
// Commands carry a connection generation and journaled ID. Stale/duplicate commands never execute.
// Task tabs open inactive in the FERB minimized window. CDP focus emulation targets the tab only;
// the daemon sentinel measures OS effects rather than assuming every browser build stays background.
// A download in a tab FERB drives is held here and handed to the daemon in parts, never to the browser (D35).
importScripts("protocol.js");
importScripts("ax.js");
importScripts("pointer.js");
importScripts("downloads.js");

let port = null;
let gen = null;                      // connection generation, assigned by the daemon ("welcome" / "paired")
let retryMs = 1000;
let refused = null;
let requiredRelease = null;          // structured daemon compatibility facts, never inferred from error prose
let activeCommands = 0;
let updateCheck = null;
let handshakeTimer = null;
const resultTransfers = new Map();
const commandTransfers = new Map();
const outbox = [];                   // retain all queued results/events until reconnect; never discard evidence
const downloads = new Map();         // transfer id -> a download held for the daemon (D35)
let DOWNLOAD_IDLE_MS = 30000;        // a held download nobody takes goes back to the browser (FERB's profile: failed)
let profileIdentity = "owner";       // from the daemon's welcome: in FERB's own profile every download is FERB's
const TRANSPORT_OPS = new Set(["download_part"]);   // reads of held data: never in the 500-entry command journal
const armedTabs = () => [...TABS].filter(([, t]) => t.armed).map(([id]) => id);
const VIEWPORT = { width: 1280, height: 900 };   // Emulation.setDeviceMetricsOverride for FERB's own tabs

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
    else { outbox.push(msg); ensurePort(); }
  });
}
function flushOutbox() { while (outbox.length && port) port.postMessage(outbox.shift()); }

// ---------- connection ----------

async function connectionStatus(state, error = "") {
  await chrome.storage.session.set({ferbConnection: {state, error: String(error),
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
    if (activeCommands || downloads.size || [...TABS.values()].some(t => t.gesture || t.attached) || (await ownedTabs()).length) return;
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
                        owned: await ownedTabs(), seen: s.order.slice(-200).map((id) => ({ id, status: s.seen[id] })),
                        armed: armedTabs() });
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
    profileIdentity = m.identity === "ferb" ? "ferb" : "owner";
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
                          browser: navigator.brave ? "brave" : "chrome", paired: true, owned: [], seen: [],
                          armed: armedTabs() });
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
chrome.runtime.onMessage.addListener((m, sender, reply) => {
  if (m && m.pairToken) send("pair", { token: m.pairToken });
  else if (m && m.ferbArmed && sender && sender.tab) reply(!!(TABS.get(sender.tab.id) || {}).armed);   // content.js
  // content.js asks before it keeps what it caught; "not armed" makes it hand the click back to the browser at once.
  else if (m && m.ferbDownload && sender && sender.tab)
    reply(pageDownload(sender.tab.id, sender.frameId || 0, m.ferbDownload, sender.documentId));
  else if (m && m.ferbDropped && sender && sender.tab)
    pageDropped(sender.tab.id, sender.frameId || 0, sender.documentId, m.ferbDropped.name);
  else if (m && m.ferbOpen && sender && sender.tab) {           // answered once the daemon has (or after ~1 s)
    pageOpen(sender.tab.id, m.ferbOpen).then(reply, () => reply(false));
    return true;
  }
  else if (m && m.ferbPopup && sender && sender.tab) reply(pageHookPopup(sender.tab.id, sender.frameId || 0, m.ferbPopup));
});

// ---------- commands ----------

async function onCommand(m) {
  activeCommands++;
  try { await executeCommand(m); }
  finally { activeCommands--; }
}

async function executeCommand(m) {
  const claimed = TRANSPORT_OPS.has(m.op) ? (m.gen !== gen ? 'stale' : 'claimed') : await mutateSession(s => {
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
  let body, nothingSent = false;
  try {
    if (!handler) throw new Error(`unknown op ${m.op}`);
    const value = await withTimeout(handler({ ...(m.args || {}), _timeout_ms: m.timeout_ms || 30000 }, m.id),
                                    m.timeout_ms || 30000);
    body = { id: m.id, gen: m.gen, status: "ok", value };
  } catch (e) {
    if (m.op === "pointer") cancelPointer(m.args.tab_id, m.id);
    body = { id: m.id, gen: m.gen, status: "error", error: String((e && e.message) || e) };
    nothingSent = !!(e && e.nothingSent);
  }
  // A handler error may follow partial input; a timeout does not cancel the renderer operation. Only an error raised
  // before anything reached the page (refusedCommand) is a known failure.
  const outcome = body.status === "ok" ? (body.value && body.value.ok === false ? "failed" : "done")
    : nothingSent ? "failed" : "unknown";
  body.outcome = outcome;
  await mutateSession(s => {if (Object.hasOwn(s.seen, m.id)) s.seen[m.id] = outcome;});
  if (body.status === "ok" && m.op !== "result_part") {
    // ASCII JSON avoids splitting a Unicode scalar across native-message chunks.
    const serialized = JSON.stringify(body.value ?? null).replace(/[\u0080-\uffff]/g,c=>"\\u"+c.charCodeAt(0).toString(16).padStart(4,"0"));
    if (serialized.length > 100000) {
      resultTransfers.set(m.id, serialized);
      body.value = {result_parts:{id:m.id,total_chars:serialized.length}};
    }
  }
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

// Load a URL in a tab and wait for it: the old page is still "complete", so wait for the new load to start first.
async function load(tabId, url) {
  const started = new Promise((resolve) => {
    const on = (id, info) => { if (id === tabId && info.status === "loading") { chrome.tabs.onUpdated.removeListener(on); resolve(); } };
    chrome.tabs.onUpdated.addListener(on);
    setTimeout(() => { chrome.tabs.onUpdated.removeListener(on); resolve(); }, 3000);
  });
  await chrome.tabs.update(tabId, { url });
  await started;
  await waitComplete(tabId, 20000);
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

// ---------- Reuse FERB's window; cold creation requires explicit foreground permission ----------

let windowCreation = null;
async function ferbWindow(allowForeground = false) {
  if (!windowCreation) windowCreation = getFerbWindow(allowForeground).finally(() => {windowCreation = null;});
  return windowCreation;
}

async function getFerbWindow(allowForeground = false) {
  const s = await session();
  if (s.windowId != null) {
    try {
      const w = await chrome.windows.get(s.windowId);
      // An owner may restore it to watch a handed-off class. Never minimize it again behind their back.
      return w.id;
    } catch (_) { /* gone: return choices unless creation is explicitly allowed */ }
  }
  // Window creation may switch macOS Spaces before its requested minimized state takes effect.
  // Return all choices instead of guessing an owner window or creating one without foreground consent.
  if (!allowForeground) return null;
  // Requested minimized (probe E5, S10); focused:false on top (probe E1). A blank placeholder tab keeps the window
  // alive when FERB's last task tab closes, so a window is created once per browser session, not per task.
  const w = await chrome.windows.create({ url: "about:blank", state: "minimized", focused: false });
  await mutateSession(s => {s.windowId = w.id;});
  const got = await chrome.windows.get(w.id);                // create() may report "normal" (probe E5): verify
  if (got.state !== "minimized") await chrome.windows.update(w.id, { state: "minimized" });
  return w.id;
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

// A capture's mask fields (protocol 15): both looks when the page was read; when it wasn't, `unread` (why), the look
// before if only the look after failed (`mask.after` null), and the tab's window (`tabWindow`).
async function unreadCapture(tabId, first, last) {
  if (!first.withheld && !last.withheld) return {mask: {before: first, after: last}};
  return {unread: (first.withheld || last.withheld), mask: first.withheld ? null : {before: first, after: null},
          window: await tabWindow(tabId)};
}

// Where a tab's window is, for the daemon's own look at it (protocol 16): screen bounds, whether the tab is the one it
// shows, whether it is minimized, the tab's title (to find that very window), and whether FERB lays the tab out at its
// own size in it (then Accessibility's places don't match a tab capture). null: the tab or window went.
async function tabWindow(tabId) {
  try {
    const tab = await chrome.tabs.get(tabId);
    const w = await chrome.windows.get(tab.windowId);
    return {bounds: [w.left, w.top, w.width, w.height], active: !!tab.active, minimized: w.state === "minimized",
            title: String(tab.title || ""), ...(tabState(tabId).emulated ? {emulated: true} : {})};
  } catch (_) { return null; }
}

// A capture's attach, after `arm` (rounds 4–5, rulings). A tab already attached fully needs no more (an iframe's
// input-setup error, which refuses input commands, doesn't touch a capture). A page scripts can't reach stays armed,
// the debugger alone: no full attach, so no detach and re-attach per capture (overlapping captures and a recording's
// frames never collide, nothing held is handed back, D35). A scriptable page gets the input setup and readiness probe
// (focus emulation and FERB's viewport: the emulation background captures were measured with, 2.7), but their
// failure (a full-page PDF or plugin document the probe's move can't reach) refuses no capture: kept attached, what
// applied kept, and the returned note says so ("": nothing to say).
async function readyForCapture(tabId) {
  const t = tabState(tabId);
  if (t.attached && t.ready) return "";
  if (!(await scriptable(tabId))) return "";
  try {
    await inputSetup(tabId);
    await ready(tabId);
    t.ready = true;
    return "";
  } catch (e) {
    return `input readiness failed (${String(e && e.message || e)}); the capture went on without it`;
  }
}

async function scriptable(tabId, frameId = 0) {
  try { await chrome.scripting.executeScript({ target: { tabId, frameIds: [frameId] }, func: () => true }); return true; }
  catch (_) { return false; }
}

// A tab the browser won't let its debugger attach to (the Web Store, chrome:// and other extensions' pages) can't be
// captured as a tab at all (protocol 16): said honestly, with its window, for the daemon's native window capture.
async function undebuggable(tabId, e) {
  return {withheld: `the browser won't let its debugger attach to this page (${String(e && e.message || e)}), so ` +
                    "the tab itself can't be captured", undebuggable: true, window: await tabWindow(tabId)};
}

// One look at a time per tab: a recording's frames (two looks each) and a worker's screenshot or a native capture
// of the same tab can overlap, and their rounds must not interleave in the page. `deadline` is the command's own
// (Date.now() + its _timeout_ms): a look holds its tab until it ends or its deadline passes, waits for its turn only
// until its deadline, and is skipped (withheld, the page never touched) when that passes first. A skipped look still
// keeps its place, so the look after it waits for the one before it: the chain stays strictly serial.
const maskLooks = new Map();                       // tabId -> the end of that tab's chain of looks
function maskState(tabId, deadline) {
  let timer;
  const expired = new Promise(resolve => { timer = setTimeout(resolve, Math.max(0, deadline - Date.now())); });
  const prev = maskLooks.get(tabId) || Promise.resolve();
  const turn = Promise.race([prev.then(() => true), expired.then(() => false)]);
  const look = turn.then(mine => mine && Date.now() < deadline ? readMaskState(tabId)
    : {withheld: "this look at the page ran out of time waiting for its turn, so its secret fields can't be found",
       unread: true});
  const done = Promise.all([prev, Promise.race([look.catch(() => {}), expired]).then(() => clearTimeout(timer))]);
  maskLooks.set(tabId, done);
  done.then(() => { if (maskLooks.get(tabId) === done) maskLooks.delete(tabId); });
  return look;
}

// What a capture of this tab must paint over (L13), in top-viewport CSS px. Three rounds over every frame: all listen,
// each child announces a fresh token to its parent, each reports its secret fields, its frame hosts with the tokens
// they heard, and its own token (content.js). Only the top document's own secret fields (its shadow roots' too) are
// masked one by one: their getBoundingClientRect is already where they are drawn. A child frame's content is never
// placed (CSS on the host's ancestors can move it where no offset says). A host is linked to a frame when it heard
// exactly one token, nothing else heard that token, and exactly one frame owns it; a top-document host is left
// unpainted only when it is linked to a frame that is clean: no secret field, and every host in it with any area is
// linked to a clean frame. Any other host (holding a secret at any depth, nothing loaded, out of reach, a token
// missing, repeated or shared) is painted whole: its box, clipped to the viewport, already includes every transform
// the browser applies, and a forged message only adds black. Withheld when a frame with secret fields is accounted
// for by no host (it can't be covered: an <embed>, a host made mid-capture), when a top-document secret field or a
// host painted whole is drawn again elsewhere (redrawn: -webkit-box-reflect or an SVG filter puts the copy outside
// the mask), or the page can't be read.
// Items carry keys, so the daemon can tell what moved between the looks before and after a capture. Geometry only,
// never page content.
async function readMaskState(tabId) {
  const round = crypto.randomUUID();
  let results;
  try {
    await inject(tabId);
    const target = {tabId, allFrames: true}, args = [round];
    await chrome.scripting.executeScript({target, args, func: r => window.__ferb.listenForFrames(r)});
    await chrome.scripting.executeScript({target, args, func: r => window.__ferb.announceFrame(r)});
    results = await chrome.scripting.executeScript({target, args, func: r => window.__ferb.captureState(r)});
  } catch (_) { return {withheld: "the page can't be read, so its secret fields can't be found", unread: true}; }
  const frames = results.filter(r => r && r.result).map(r => ({frameId: r.frameId, ...r.result}));
  const top = frames.find(f => f.frameId === 0);
  if (!top) return {withheld: "the page's top frame can't be read, so its secret fields can't be found", unread: true};
  const times = new Map(), owners = new Map();     // token -> how often any host heard it; -> the frame owning it
  for (const f of frames) for (const c of f.frames) for (const t of c.heard) times.set(t, (times.get(t) || 0) + 1);
  for (const f of frames) if (f !== top && f.own) owners.set(f.own, owners.has(f.own) ? null : f);
  const linked = c => c.heard.length === 1 && times.get(c.heard[0]) === 1 && owners.get(c.heard[0]) || null;
  const accounted = new Set([top]);                // heard by a host in an accounted frame, linked or not
  for (const queue = [top]; queue.length;) {
    for (const c of queue.shift().frames) for (const t of c.heard) {
      const f = owners.get(t);
      if (f && !accounted.has(f)) { accounted.add(f); queue.push(f); }
    }
  }
  if (frames.some(f => !accounted.has(f) && f.rects.length))
    return {withheld: "a frame's secret fields can't be covered: no frame element on the page is known to hold it"};
  const cut = (r, c) => {
    const x = Math.max(r[0], c[0]), y = Math.max(r[1], c[1]);
    const right = Math.min(r[0] + r[2], c[0] + c[2]), bottom = Math.min(r[1] + r[3], c[1] + c[3]);
    return right > x && bottom > y ? [x, y, right - x, bottom - y] : null;
  };
  const clean = new Map();                          // frame -> nothing in it or in any frame below it to paint
  const isClean = f => {
    if (!clean.has(f)) {
      clean.set(f, false);                            // met again inside itself: not clean
      clean.set(f, !f.rects.length && f.frames.every(c => {
        if (!(c.rect[2] > 0 && c.rect[3] > 0)) return true;          // a host with no area shows nothing of its frame
        const k = linked(c);
        return !!k && isClean(k);
      }));
    }
    return clean.get(f);
  };
  const shown = c => { const k = linked(c); return !!k && isClean(k); };
  if (top.rects.some(r => r.redrawn) || top.frames.some(c => c.redrawn && !shown(c)))
    return {withheld: "a secret field is drawn again elsewhere on the page (a reflection or an SVG filter), outside "
                      + "where it can be painted"};
  const items = [];
  for (const r of top.rects) {
    const m = cut(r.rect, top.viewport);
    if (m) items.push({key: `field|${top.documentId}|${r.id}`, rect: m, paint: true});
  }
  for (const c of top.frames) {
    const box = cut(c.rect, top.viewport);
    if (!box) continue;                                   // off the page: nothing of it shows
    const key = `frame|${top.documentId}|${c.id}`, child = linked(c);
    items.push(child ? {key, rect: box, paint: !isClean(child), child: child.documentId} : {key, rect: box, paint: true});
  }
  return {top: top.documentId, viewport: top.viewport, items};
}

// ---------- the debugger session: attached on a task tab's first action, kept for its life ----------

const TABS = new Map();          // tabId -> { attached, armed, ready, detached, dialogs, expect, leaving, frames, ax, shot }
function tabState(tabId) {
  let t = TABS.get(tabId);
  if (!t) { t = { attached: false, armed: false, ready: false, detached: null, dialogs: [], fileChoosers: [], inputErrors: [], expect: null, leaving: false, frames: {}, sessions: new Map(), axErrors: [], shot: null,
                  redirects: new Map(), native: new Map(), held: new Map(), popup: false };
            TABS.set(tabId, t); }
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
  if (t.attached && t.ready) {
    await Promise.all(t.sessions.values());
    if (t.inputErrors.length) throw Error(`background_input_unavailable: ${t.inputErrors.join("; ")}`);
    return t;
  }
  await arm(tabId);
  try {
    await inputSetup(tabId);
    await ready(tabId);                                        // the owner's own tab keeps its real viewport
    t.ready = true;
    return t;
  } catch (e) {
    await dropDownloads(tabId);
    t.attached = false; t.armed = false; t.emulated = false;
    try { await chrome.debugger.detach({tabId}); } catch (_) {}
    throw e;
  }
}

// What trusted input needs besides the debugger, before the readiness probe: file-chooser interception, focus
// emulation and, in FERB's own window, FERB's viewport.
async function inputSetup(tabId) {
  const t = tabState(tabId);
  // Intercept the browser mechanism, including indirect showPicker()/label/button routes.
  // The event supplies evidence; it never chooses a local/Drive source or opens a native picker.
  await cdp(tabId, "Page.setInterceptFileChooserDialog", {enabled: true});
  await cdp(tabId, "Emulation.setFocusEmulationEnabled", { enabled: true });
  const s = await session(), tab = await chrome.tabs.get(tabId);
  if (s.windowId != null && tab.windowId === s.windowId) {     // FERB's own window: layout independent of its size.
    await cdp(tabId, "Emulation.setDeviceMetricsOverride", { ...VIEWPORT, deviceScaleFactor: 1, mobile: false });
    t.emulated = true;                                         // its captures aren't the window's layout (tabWindow)
  }
}

// D35: downloads are caught only in a tab FERB drives (opened or borrowed for a task), and only while it does: the
// tab's own debugger session pauses its document responses at the response stage (Fetch), child frames included, and
// content.js holds its blob/data download links. Nothing applies to the profile, so the owner's downloads elsewhere
// are never touched (rule 17). Browser/Page.setDownloadBehavior would be profile-wide, and an extension may not call
// them (measured). Armed before a URL loads, so a URL that is a download is caught too.
async function arm(tabId) {
  const t = tabState(tabId);
  if (!t.attached) {
    try { await chrome.debugger.attach({ tabId }, "1.3"); }
    catch (e) { throw debuggerRefused(e); }
    t.attached = true; t.detached = null; t.ready = false; t.armed = false; t.emulated = false;   // a new session
  }
  if (t.armed) return t;
  try {
    await cdp(tabId, "Page.enable", {});                       // dialogs, and the browser's own download events
    await cdp(tabId, "Fetch.enable", { patterns: DOWNLOAD_PATTERNS });
    t.armed = true;
    await cdp(tabId, "Target.setAutoAttach", { autoAttach: true, waitForDebuggerOnStart: false, flatten: true });
  } catch (e) {
    t.attached = false; t.armed = false;
    try { await chrome.debugger.detach({tabId}); } catch (_) {}
    throw e;
  }
  t.hookId = await installHook(tabId);
  await armPage(tabId, true);
  return t;
}

// The browser refused the debugger (chrome.debugger.attach threw): nothing was attached and nothing sent. Chrome
// checks every frame in the tab and refuses the whole tab for one it won't let an extension debug, so one frame another
// extension put into a page of the owner's sites ("Cannot access a chrome-extension:// URL of different extension")
// keeps FERB's debugger off that tab while it is there; Chrome also detaches a session from such a tab when its page
// navigates. The other refusals (chrome://, the Web Store, other extensions' own pages) are flagged the same way.
// Scripting may still reach the page's own frames: Chrome skips the other extension's frame there.
function debuggerRefused(e) {
  const refused = new Error(String(e && e.message || e));
  refused.debuggerRefused = true;
  return refused;
}
const refusedWhy = e => `the browser refused FERB's debugger for this tab (${e.message})`;
// A command that needs the debugger, refused before anything reached the page: a known failure (executeCommand), its
// error led by "debugger_refused:" as a lost session's is by "detached:", so a daemon of any version reads it as one.
function refusedCommand(detail) {
  const e = new Error(`debugger_refused: ${detail}`);
  e.nothingSent = true;
  return e;
}
// Page events where the browser refused the debugger: its Fetch interception, which holds a tab's downloads for FERB
// (D35), isn't there, and FERB has no downloads permission to see one; a click or press that starts a download leaves it
// to the browser (owner, 2026-10-24: allowed, with guards).
const DOWNLOAD_KINDS = new Set(["click", "dblclick", "press"]);
const UNMANAGED_DOWNLOADS = "a download this starts is saved by the browser to its Downloads folder, not FERB's " +
  "workspace: FERB can't see it or apply its download limits";

// The page hook (protocol.js PAGE_HOOK) in this session's documents, the current one included; best effort: without it
// only what it catches (unseen save links, window.open) is left to the browser, reported when seen.
async function installHook(tabId, sessionId) {
  try {
    const r = await cdp(tabId, "Page.addScriptToEvaluateOnNewDocument", { source: PAGE_HOOK, runImmediately: true }, sessionId);
    return (r && r.identifier) || null;
  } catch (e) { console.warn("FERB page hook:", String(e.message || e)); return null; }
}

async function armPage(tabId, on) {                          // content.js in documents already there
  // Disarming hands what the page holds back to the browser; in FERB's own profile it is dropped (every download there
  // is FERB's, and a handed-back one could open a dialog).
  const drop = profileIdentity === "ferb";
  try {
    await Promise.resolve(chrome.scripting.executeScript({ target: { tabId, allFrames: true },
      func: (v, d) => { if (window.__ferb && window.__ferb.armDownloads) window.__ferb.armDownloads(v, d); }, args: [on, drop] }));
  } catch (_) { /* a page scripts can't reach holds no listener of ours */ }
}

async function detach(tabId) {
  const t = TABS.get(tabId);
  if (!t || !t.attached) { TABS.delete(tabId); return; }
  // Keep the state until the browser acknowledges release. A failed detach is actionable, not success.
  t.leaving = true;
  if (t.armed) {                                             // let go of what this tab still holds, then of the page
    await dropDownloads(tabId);
    await armPage(tabId, false);
    if (t.hookId) {                                          // documents loaded from now on are native again
      try { await cdp(tabId, "Page.removeScriptToEvaluateOnNewDocument", { identifier: t.hookId }); } catch (_) { /* gone */ }
      t.hookId = null;
    }
  }
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
  const wasArmed = t.armed;
  t.attached = false; t.armed = false; t.ready = false; t.emulated = false; t.detached = reason || "unknown";
  if (wasArmed) { forgetDownloads(src.tabId); armPage(src.tabId, false); }   // the session (and what it held) is gone
  // Not FERB's own close, and a tab the daemon knows: a popup still being checked was never announced to it.
  if (!t.leaving && !t.claiming) send("event", { event: "detached", tab_id: src.tabId, reason: t.detached });
});

// Dialog decisions belong to the agent. An unexpected dialog remains pending, including beforeunload.
chrome.debugger.onEvent.addListener((src, method, params) => {
  if (method === "Fetch.requestPaused") { onPaused(src, params).catch(e => console.warn("FERB download:", String(e))); return; }
  if (method === "Page.downloadWillBegin" || method === "Page.downloadProgress") { browserDownload(src.tabId, method, params); return; }
  if (method === "Target.attachedToTarget") {
    const t = tabState(src.tabId), sid = params.sessionId;
    if (params.targetInfo && params.targetInfo.type === "iframe") {
      t.sessions.set(sid, (async () => {
        if (t.armed) {                                         // an OOPIF's own requests and its page hook
          await cdp(src.tabId, "Fetch.enable", { patterns: DOWNLOAD_PATTERNS }, sid);
          await installHook(src.tabId, sid);
        }
        await cdp(src.tabId, "Page.enable", {}, sid);
        await cdp(src.tabId, "Page.setInterceptFileChooserDialog", {enabled: true}, sid);
        await cdp(src.tabId, "Target.setAutoAttach", {autoAttach: true, waitForDebuggerOnStart: false, flatten: true}, sid);
      })().catch(e => {
        const error = String(e.message || e); t.axErrors.push(error); t.inputErrors.push(error);
      }));
    }
    return;
  }
  if (method === "Target.detachedFromTarget") { tabState(src.tabId).sessions.delete(params.sessionId); return; }
  if (method === "Page.fileChooserOpened") {
    const t = tabState(src.tabId);
    const fact = {reason:"native_file_chooser_intercepted", frame_id:params.frameId,
      session_id:src.sessionId || null, backend_node_id:params.backendNodeId, mode:params.mode,
      native_picker_shown:false, transferred:false, direct_upload_available:true,
      detail:"Native chooser suppressed for background operation. Inspect the file input and choose an explicit local source with browser upload; no source was selected."};
    t.fileChoosers.push(fact);
    t.dialogs.push({type:"file_chooser", message:JSON.stringify(fact), handled:"intercepted"});
    send("event", {event:"file_chooser", tab_id:src.tabId, ...fact});
    return;
  }
  if (method !== "Page.javascriptDialogOpening") return;
  const t = TABS.get(src.tabId);
  if (!t) return;
  const d = { type: params.type, message: String(params.message || ""), default: String(params.defaultPrompt || ""), handled:"pending" };
  t.pendingDialog = d;
  t.dialogs.push(d);
  send("event", { event: "dialog", tab_id: src.tabId, ...d });
  if (t.expect) {
    const expected = t.expect; t.expect = null;
    commands.dialog({tab_id:src.tabId,decision:expected.accept ? "accept" : "dismiss",text:expected.text})
      .catch(e => send("event",{event:"dialog",tab_id:src.tabId,...d,reason:String(e.message || e)}));
  }
});

chrome.tabs.onRemoved.addListener(tabId => {
  forgetDownloads(tabId);
  TABS.delete(tabId);
  mutateSession(s => {s.owned = s.owned.filter(o => o.tabId !== tabId);}).catch(e => console.warn('FERB tab cleanup:', String(e)));
  serially(async () => {
    const ids = await keptTabs();
    if (ids.includes(tabId)) await chrome.storage.local.set({ferbKeptTabs: ids.filter(id => id !== tabId)});
  }).catch(e => console.warn('FERB kept-tab cleanup:', String(e)));
});

// ---------- downloads (D35): held for the daemon, never handed to the browser ----------
// A response held at the response stage never reaches the browser's download flow, so no Save dialog and no file in the
// owner's Downloads. The daemon pulls it in parts (download_part); the last part, a cancel, an idle timeout or letting
// go of the tab fails the held request (net::ERR_ABORTED: the page stays where it was).

async function onPaused(src, params) {
  const tabId = src.tabId, t = TABS.get(tabId), sid = src.sessionId;
  let kind = null;
  try { kind = t && t.armed ? classify(params, t.redirects) : null; } catch (_) { kind = null; }
  if (t && (kind || (params.responseStatusCode >= 300 && params.responseStatusCode < 400 &&
                     t.redirects.size && !hasHeader(params.request.headers, "upgrade-insecure-requests"))))
    t.held.set(params.request.url, Date.now() + 120000);    // the browser's own "canceled" entry for it is not news
  if (kind && !port) {                                       // no daemon to take it: never held for nobody
    if (profileIdentity === "ferb") kind = "fail"; else kind = null;
  }
  if (!kind) {
    try { await cdp(tabId, "Fetch.continueRequest", { requestId: params.requestId }, sid); } catch (_) { /* gone */ }
    return;
  }
  if (kind === "fail") {
    try { await cdp(tabId, "Fetch.failRequest", { requestId: params.requestId, errorReason: "Aborted" }, sid); } catch (_) { /* gone */ }
    return;
  }
  const h = headerMap(params.responseHeaders), size = Number(h["content-length"]);
  const tr = { id: "d-" + crypto.randomUUID().replace(/-/g, "").slice(0, 16), tabId, sessionId: sid || null,
               requestId: params.requestId, stream: null, page: null, offset: 0, timer: null };
  downloads.set(tr.id, tr);
  idle(tr);
  await send("event", { event: "download", tab_id: tabId, download_id: tr.id,
    name: downloadName(params.responseHeaders, params.request.url), mime: mimeOf(h) || null,
    size: h["content-length"] !== undefined && Number.isSafeInteger(size) && size >= 0 ? size : null,
    source_url: downloadSource(params.request.url, maskUrl), source: kind });
}

// content.js caught a blob/data download link; it keeps the bytes only if this tab is armed (true), else hands the click
// back to the browser itself (false: a stale listener in a tab FERB no longer drives never eats the owner's click).
// Bounded: a page's flood of saves past 20 held in its tab (100 in all) is dropped ("drop"), never the browser's; the
// daemon hears of it as a refused download (pageDropped).
const PAGE_HOLDS_PER_TAB = 20, PAGE_HOLDS_MAX = 100;
function pageDownload(tabId, frameId, d, documentId) {
  const t = TABS.get(tabId);
  if (!d || typeof d.id !== "string" || !t || !t.armed || !port) return profileIdentity === "ferb" ? "drop" : false;
  const held = [...downloads.values()].filter((tr) => tr.page);
  if (held.length >= PAGE_HOLDS_MAX || held.filter((tr) => tr.tabId === tabId).length >= PAGE_HOLDS_PER_TAB) {
    pageDropped(tabId, frameId, documentId, d.name);
    return "drop";
  }
  const tr = { id: "d-" + crypto.randomUUID().replace(/-/g, "").slice(0, 16), tabId, sessionId: null, requestId: null,
               stream: null, page: { frameId, ref: d.id }, offset: 0, timer: null };
  downloads.set(tr.id, tr);
  idle(tr);
  send("event", { event: "download", tab_id: tabId, download_id: tr.id, name: String(d.name || "").slice(0, 1024),
    mime: String(d.mime || "") || null, size: Number.isSafeInteger(d.size) && d.size >= 0 ? d.size : null,
    source_url: downloadSource(d.source_url, maskUrl), source: "page" });
  return true;
}

// Saves a page made past the hold caps (here or content.js's own) are dropped: neither taken nor the browser's. The
// worker hears of it as a refused download: one fact per document per minute, not one per dropped save.
const DROP_FACT_MS = 60000, DROP_DOCS_MAX = 200;
const dropFacts = new Map();         // document (tab/documentId, else tab/frame) -> when its last fact was sent
function pageDropped(tabId, frameId, documentId, name) {
  const t = TABS.get(tabId);
  if (!t || !t.armed || !port) return;
  const key = `${tabId}/${documentId || "frame-" + frameId}`, now = Date.now(), last = dropFacts.get(key);
  if (last !== undefined && now - last < DROP_FACT_MS) return;
  dropFacts.delete(key);
  dropFacts.set(key, now);
  if (dropFacts.size > DROP_DOCS_MAX) dropFacts.delete(dropFacts.keys().next().value);
  send("event", { event: "download", tab_id: tabId, uncaptured: true, detail: "dropped", source: "page",
                  name: String(name || "").slice(0, 1024) });
}

// A task tab's link to a new tab (target=_blank, a modifier or middle click): FERB's daemon opens it itself, blank and
// armed before it loads, inactive, as the same task's tab. Resolves true once the daemon has answered (open_seen); with
// no answer within OPEN_WAIT_MS (the daemon down or restarting, a tab only stale-armed) it resolves false and content.js
// lets the browser open the link, so the owner's click is never lost; a later answer then gets "expired" and FERB
// doesn't open it too. Not armed: false at once.
const OPEN_WAIT_MS = 1000;
const pendingOpens = new Map();      // open_id -> settle(true): waiting for the daemon's answer
function pageOpen(tabId, o) {
  const t = TABS.get(tabId);
  if (!o || typeof o.url !== "string" || !/^https?:/i.test(o.url) || !t || !t.armed || !port) return Promise.resolve(false);
  const id = "o-" + crypto.randomUUID().replace(/-/g, "").slice(0, 16);
  return new Promise((resolve) => {
    const timer = setTimeout(() => { if (pendingOpens.delete(id)) resolve(false); }, OPEN_WAIT_MS);
    pendingOpens.set(id, () => { clearTimeout(timer); resolve(true); });
    chrome.tabs.get(tabId).then((tab) => send("event", { event: "open", tab_id: tabId, open_id: id, url: o.url.slice(0, 8192),
                                                        window_id: tab.windowId, background: o.background === true }))
      .catch(() => { if (pendingOpens.delete(id)) { clearTimeout(timer); resolve(false); } });
  });
}

// Popups (D35): FERB claims only a NEW tab it can prove the page hook of a live task tab opened: the tab appeared in
// chrome.tabs.onCreated and its first URL report (there, or its first onUpdated url) is exactly about:blank#ferb-<token>
// for a token that task tab relayed (either may come first; each waits 2 s); then, attached, the browser itself names
// that task tab as the opener (Target.getTargetInfo: the popup's openerId is the opener tab's own targetId, measured
// for a popup tab, a popup window with features and a popup opened from a cross-site iframe). So a tab that existed
// before can't be claimed by navigating itself to that URL, and a new popup of some other tab (the owner's) at that URL
// is let go of at once. Chrome's openerTabId names the window's active tab (measured), so it is never used. FERB never
// navigates the popup: once it is armed and the daemon has made it the task's (popup_ready), the opener's page is told
// and the hook loads the URL itself, once; without that, the hook loads it after 1.5 s. Bounded: 20 relays per opener
// tab and 100 in all (oldest dropped), one relay per token; the tokens already relayed are bounded the same way.
const POPUP_URL = /^about:blank#ferb-([0-9a-f]{32})$/;
const POPUP_WINDOW_MS = 2000, RELAYS_PER_OPENER = 20, RELAYS_MAX = 100, NEW_TABS_MAX = 200;
const popupRelays = new Map();       // token -> {opener, frameId, url, at}, oldest first
const popupTabs = new Map();         // token -> {tabId, at}: the new tab appeared before the relay
const newTabs = new Map();           // tabId -> {at}: created just now, its first URL not reported yet
const relayedTokens = new Map();     // token -> {opener, at}: relayed once already, oldest first
function prunePopups(now) {
  for (const m of [popupRelays, popupTabs, newTabs, relayedTokens])
    for (const [k, v] of m) if (v.at < now - POPUP_WINDOW_MS) m.delete(k);
}
function pageHookPopup(tabId, frameId, p) {
  const t = TABS.get(tabId);
  if (!p || typeof p.token !== "string" || !/^[0-9a-f]{32}$/.test(p.token) || typeof p.url !== "string" ||
      !/^https?:/i.test(p.url) || !t || !t.armed || !port) return false;
  const now = Date.now();
  prunePopups(now);
  if (relayedTokens.has(p.token)) return false;                      // the first relay for a token wins
  const seen = [...relayedTokens].filter(([, r]) => r.opener === tabId);
  if (seen.length >= RELAYS_PER_OPENER) relayedTokens.delete(seen[0][0]);
  if (relayedTokens.size >= RELAYS_MAX) relayedTokens.delete(relayedTokens.keys().next().value);
  relayedTokens.set(p.token, { opener: tabId, at: now });
  const mine = [...popupRelays].filter(([, r]) => r.opener === tabId);
  if (mine.length >= RELAYS_PER_OPENER) popupRelays.delete(mine[0][0]);
  if (popupRelays.size >= RELAYS_MAX) popupRelays.delete(popupRelays.keys().next().value);
  popupRelays.set(p.token, { opener: tabId, frameId, url: p.url.slice(0, 8192), at: now });
  const waiting = popupTabs.get(p.token);
  if (waiting) { popupTabs.delete(p.token); claimPopup(p.token, waiting.tabId); }
  return true;
}
function popupCandidate(tabId, url) {              // a new tab's first URL
  const m = POPUP_URL.exec(String(url || ""));
  if (!m) return;
  const now = Date.now();
  prunePopups(now);
  if (popupRelays.has(m[1])) claimPopup(m[1], tabId);
  else if (!popupTabs.has(m[1])) popupTabs.set(m[1], { tabId, at: now });
}
function tabCreated(tab) {
  const url = tab.pendingUrl || tab.url || "";
  if (url) { popupCandidate(tab.id, url); return; }  // its first URL is known now
  const now = Date.now();
  prunePopups(now);
  if (newTabs.size >= NEW_TABS_MAX) newTabs.delete(newTabs.keys().next().value);
  newTabs.set(tab.id, { at: now });
}
function tabUpdated(tabId, url) {
  const fresh = newTabs.get(tabId);
  newTabs.delete(tabId);                             // only a new tab's first URL report; a later navigation never
  if (fresh && fresh.at >= Date.now() - POPUP_WINDOW_MS) popupCandidate(tabId, url);
}
chrome.tabs.onCreated.addListener(tabCreated);
chrome.tabs.onUpdated.addListener((id, info) => { if (info && info.url) tabUpdated(id, info.url); });
async function claimPopup(token, tabId) {
  const relay = popupRelays.get(token);
  popupRelays.delete(token);
  const opener = relay ? TABS.get(relay.opener) : null;
  if (!relay || !opener || !opener.armed || !port || (TABS.get(tabId) || {}).armed) return;
  const t = tabState(tabId);
  t.claiming = true;                                 // the daemon hasn't heard of this tab: no "detached" for it yet
  try {
    if (!t.attached) {
      await chrome.debugger.attach({ tabId }, "1.3");
      t.attached = true; t.detached = null; t.ready = false; t.armed = false;
    }
    // Not confirmed as that task tab's popup: let it go. Nothing was enabled on it, so the debugger is simply detached
    // (never through detach(), whose focus-emulation reset could fail and keep it attached). Only the hook's own
    // popup, whose opener the browser couldn't confirm, is then loaded by the hook (natively, after 1.5 s); any other
    // tab at that URL stays with what opened it.
    if (!(await openedBy(tabId, relay.opener))) {
      TABS.delete(tabId);
      try { await chrome.debugger.detach({ tabId }); } catch (_) { /* already gone */ }
      return;
    }
    await arm(tabId);
    t.popup = true;
    t.popupFor = { opener: relay.opener, frameId: relay.frameId, token };
    t.claiming = false;
    await send("event", { event: "popup", tab_id: tabId, opener_tab_id: relay.opener, url: maskUrl(relay.url) });
  } catch (e) { console.warn("FERB popup:", String(e.message || e)); }
  finally { t.claiming = false; }
}
// The browser's own word on who opened a tab: the popup's openerId must be the relaying task tab's own target id. Each
// session answers for its own target only. Unanswerable (an older browser, a detached session): not confirmed.
async function openedBy(tabId, openerTabId) {
  try {
    const own = await cdp(tabId, "Target.getTargetInfo", {});
    const opener = await cdp(openerTabId, "Target.getTargetInfo", {});
    const by = own.targetInfo && own.targetInfo.openerId, id = opener.targetInfo && opener.targetInfo.targetId;
    return typeof by === "string" && by.length > 0 && by === id;
  } catch (_) { return false; }
}

function idle(tr) {
  clearTimeout(tr.timer);
  tr.timer = setTimeout(() => { if (downloads.get(tr.id) === tr) handBack(tr); }, DOWNLOAD_IDLE_MS);
}

// Not FERB's to take (no live task tab, nobody pulled it, FERB let go of the tab): the response continues to the
// browser untouched, or content.js re-clicks its link. Once FERB took the body it can only be failed.
async function handBack(tr) {
  if (tr.stream || profileIdentity === "ferb") return releaseTransfer(tr);   // FERB's own profile: never the browser's
  downloads.delete(tr.id);
  clearTimeout(tr.timer);
  if (tr.page) {
    try { await inFrame(tr.tabId, tr.page.frameId, (ref) => window.__ferb.releaseDownload(ref), [tr.page.ref]); }
    catch (_) { /* the document is gone, and its link with it */ }
    return;
  }
  try { await cdp(tr.tabId, "Fetch.continueRequest", { requestId: tr.requestId }, tr.sessionId || undefined); }
  catch (_) { /* the request already ended */ }
}

// Done, cancelled or abandoned: the same end. Nothing of it is left to the browser.
async function releaseTransfer(tr) {
  downloads.delete(tr.id);
  clearTimeout(tr.timer);
  if (tr.page) {
    try { await inFrame(tr.tabId, tr.page.frameId, (ref) => { window.__ferb.dropDownload(ref); return true; }, [tr.page.ref]); }
    catch (_) { /* the document is gone, and the bytes with it */ }
    return;
  }
  const sid = tr.sessionId || undefined;
  if (tr.stream) { try { await cdp(tr.tabId, "IO.close", { handle: tr.stream }, sid); } catch (_) { /* closed */ } }
  try { await cdp(tr.tabId, "Fetch.failRequest", { requestId: tr.requestId, errorReason: "Aborted" }, sid); }
  catch (_) { /* the request already ended */ }
}

// Letting go of a tab the owner keeps (release, disarm, a failed setup): what it holds goes back to the browser.
// Closing FERB's own tab (`fail`): what it holds is failed, so nothing outlives the tab as a browser download.
async function dropDownloads(tabId, fail = false) {
  for (const tr of [...downloads.values()]) if (tr.tabId === tabId) await (fail ? releaseTransfer(tr) : handBack(tr));
}

function forgetDownloads(tabId) {                    // the tab or its session is gone, and its held requests with it
  for (const tr of [...downloads.values()]) if (tr.tabId === tabId) { downloads.delete(tr.id); clearTimeout(tr.timer); }
}

// A download the browser started itself in a tab FERB drives (a script-made blob/data link without a connected anchor,
// a type the classification let render): reported once as a fact. Its "canceled" entry for a response FERB held
// is not one.
function browserDownload(tabId, method, params) {
  const t = TABS.get(tabId);
  if (!t || !t.armed) return;
  const now = Date.now();
  for (const [g, n] of t.native) if (n.at < now - 600000) t.native.delete(g);       // pruned: ten minutes
  for (const [u, until] of t.held) if (until < now) t.held.delete(u);
  if (method === "Page.downloadWillBegin") {
    if (t.native.size < 50) t.native.set(params.guid, { url: params.url || "", name: params.suggestedFilename || "", at: now });
    return;
  }
  const known = t.native.get(params.guid);
  if (known && !known.reported) {
    if (params.state === "canceled" && known.url && t.held.has(known.url)) { t.native.delete(params.guid); return; }
  }
  const n = known || { url: "", name: "", at: now, unseen: true };   // began before FERB could attach (a new tab)
  const say = (detail) => send("event", { event: "download", tab_id: tabId, uncaptured: true,
    name: String(n.name).slice(0, 1024), source_url: downloadSource(n.url, maskUrl), detail });
  if (!n.reported) {
    n.reported = params.state === "canceled" ? "canceled" : "started";
    if (t.native.size < 50 || known) t.native.set(params.guid, n);
    say(n.reported === "canceled" ? "canceled" : (n.unseen && t.popup ? "new_tab" : "started"));
  } else if (n.reported === "started" && params.state === "canceled") {
    n.reported = "canceled";                  // e.g. the owner dismissed the browser's Save dialog for it
    say("canceled");
  }
  if (params.state === "completed" || params.state === "canceled") t.native.delete(params.guid);
}

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

// ---------- trusted tab-local input ----------

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
      if (!(await pressKey(tabId, a.text))) return { ok: false, detail: "unsupported_key" };
      return { ok: true, detail: `pressed ${a.text} (trusted)` };
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
  async download_part({ download_id, offset, abort, release }) {
    const tr = downloads.get(download_id);
    if (!tr) throw Error("download transfer unavailable: it was already taken, cancelled or let go");
    if (release === true) {                          // not FERB's: the browser's (FERB's own profile: failed)
      const back = !tr.stream && profileIdentity !== "ferb";
      await handBack(tr);
      return { released: true, handed_back: back };
    }
    if (abort === true) { await releaseTransfer(tr); return { aborted: true }; }      // refused in a task tab: no dialog
    if (!Number.isSafeInteger(offset) || offset !== tr.offset) throw Error("download parts are missing or out of order");
    idle(tr);
    let data, eof;
    try {
      if (tr.page) {
        const r = await inFrame(tr.tabId, tr.page.frameId, (ref, at, n) => window.__ferb.downloadPart(ref, at, n),
                                [tr.page.ref, offset, DOWNLOAD_PART]);
        if (r.error) throw Error(`the page let go of the download (${r.error})`);
        data = r.data; eof = !!r.eof;
      } else {
        const sid = tr.sessionId || undefined;
        if (!tr.stream) tr.stream = (await cdp(tr.tabId, "Fetch.takeResponseBodyAsStream", { requestId: tr.requestId }, sid)).stream;
        const r = await cdp(tr.tabId, "IO.read", { handle: tr.stream, size: DOWNLOAD_PART }, sid);
        // Text arrives as text (measured: whole characters, at most DOWNLOAD_PART bytes); its UTF-8 is the bytes.
        data = r.base64Encoded ? String(r.data || "") : _b64e(new TextEncoder().encode(String(r.data || "")));
        eof = !!r.eof;
      }
    } catch (e) {
      await releaseTransfer(tr);
      throw e;
    }
    tr.offset += b64Bytes(data);
    if (eof) await releaseTransfer(tr);
    return { data, offset, next_offset: eof ? null : tr.offset, eof };
  },
  // A link the daemon won't open as FERB's (the task's tab cap, or no live task: a stale listener caught the owner's own
  // click): the browser opens it as an ordinary tab, never FERB's (no opener link, so it isn't adopted).
  async open_plain({ tab_id, url, active = false }) {
    if (!/^https?:/i.test(String(url || ""))) throw Error("only http(s) links are opened");
    const opener = await chrome.tabs.get(tab_id);
    const tab = await chrome.tabs.create({ windowId: opener.windowId, index: opener.index + 1, url, active: active === true });
    return { opened: true, tab_id: tab.id, window_id: tab.windowId };
  },
  // The daemon's answer to an "open" event: it will open the link (seen), unless the wait already ran out and the
  // browser opened it (expired: FERB must not open it a second time).
  async open_seen({ open_id }) {
    const settle = pendingOpens.get(open_id);
    if (!settle) return { expired: true };
    pendingOpens.delete(open_id);
    settle();
    return { seen: true };
  },
  // The daemon made a claimed popup the task's: tell the opener's page, whose hook loads the URL (FERB never does).
  // `told`: content.js in that frame answered, so the page was reached (else the hook's 1.5 s fallback loads it).
  async popup_ready({ tab_id }) {
    const t = TABS.get(tab_id), f = t && t.popupFor;
    if (!f) return { ready: false };
    t.popupFor = null;
    const told = await chrome.tabs.sendMessage(f.opener, { ferbPopupReady: f.token }, { frameId: f.frameId });
    return { ready: true, told: told === true };
  },
  // The daemon holds no live task tab here (a failed release, a restart): stop holding its downloads, never close it.
  async disarm({ tab_id }) {
    const t = TABS.get(tab_id);
    await dropDownloads(tab_id);
    if (!t || !t.attached) { if (t) TABS.delete(tab_id); await armPage(tab_id, false); return { disarmed: true, tab_id }; }
    await detach(tab_id);
    return { disarmed: true, tab_id };
  },
  async command_part({id,offset,text,total_chars}) {
    if (typeof id !== "string" || typeof text !== "string" || !Number.isSafeInteger(offset) || offset<0 ||
        !Number.isSafeInteger(total_chars) || total_chars<1 || offset+text.length>total_chars) throw Error("invalid command part");
    if (offset===0 && !commandTransfers.has(id)) commandTransfers.set(id,{parts:[],length:0,total:total_chars});
    const transfer=commandTransfers.get(id);
    if (!transfer || transfer.length!==offset || transfer.total!==total_chars) throw Error("command parts are missing or out of order");
    transfer.parts.push(text);transfer.length+=text.length;
    if (transfer.length===total_chars) {
      commandTransfers.delete(id);
      const command=JSON.parse(transfer.parts.join(""));
      if (command.id!==id || command.op==="command_part") throw Error("invalid assembled command");
      await onCommand(command);
    }
    return {received:offset+text.length,total_chars};
  },
  async dialog({tab_id,decision,text}) {
    const state=tabState(tab_id), pending=state.pendingDialog;
    if (!decision) return {pending:pending || null, file_choosers:state.fileChoosers};
    if (!["accept","dismiss"].includes(decision)) throw Error("invalid dialog decision");
    if (!pending) return {ok:false,reason:"no_pending_dialog"};
    await cdp(tab_id,"Page.handleJavaScriptDialog",{accept:decision === "accept",...(text !== undefined ? {promptText:text} : {})});
    pending.handled=decision === "accept" ? "accepted" : "dismissed";
    state.pendingDialog=null;
    return {ok:true,...pending};
  },
  async result_part({id,offset=0}) {
    const text=resultTransfers.get(id);
    if (text===undefined) throw Error("result transfer unavailable; original operation must not be replayed without inspection");
    if (!Number.isInteger(offset) || offset<0 || offset>text.length) throw Error("invalid result offset");
    const end=Math.min(offset+100000,text.length);
    const result={text:text.slice(offset,end),offset,next_offset:end<text.length?end:null,total_chars:text.length};
    if (end===text.length) resultTransfers.delete(id);
    return result;
  },
  async open({ url, window_id = null, allow_foreground = false }, id) {
    const windowId = window_id !== null ? (await chrome.windows.get(window_id)).id
                                        : await ferbWindow(allow_foreground);
    if (windowId === null) {
      const windows = await chrome.windows.getAll({populate:false});
      return {opened:false, reason:'window_selection_required', needs_foreground:false,
        new_window_needs_foreground:true,
        detail:'No FERB window is available. Choose an exact existing window_id for an inactive tab; creating a new window can switch Spaces and requires foreground permission.',
        windows:windows.map(w=>({id:w.id,state:w.state,focused:w.focused,type:w.type,incognito:w.incognito}))};
    }
    // Blank first, never active, never focused (E3); the URL loads once the tab can hold a download (D35).
    const tab = await chrome.tabs.create({ windowId, url: "about:blank", active: false });
    await mutateSession(s => {s.owned.push({id, tabId:tab.id});});
    try { await chrome.tabs.update(tab.id, { autoDiscardable: false }); } catch (_) { /* Memory Saver opt-out is best effort */ }
    try { await arm(tab.id); }
    catch (e) {
      try { await chrome.tabs.remove(tab.id); } catch (_) { /* already gone */ }
      await mutateSession(s => {s.owned = s.owned.filter(o => o.tabId !== tab.id);});
      return { opened: false, reason: "download_capture_unavailable",
               detail: `FERB couldn't prepare a new tab to hold downloads, so it opened nothing (${e.message || e})` };
    }
    if (url !== "about:blank") await load(tab.id, url);   // FERB's daemon opens blank and navigates once the tab is its
    const t = await chrome.tabs.get(tab.id);
    return { tab_id: t.id, url: maskUrl(t.url || t.pendingUrl || ""), title: t.title || "", window_id: t.windowId };
  },
  async navigate({ tab_id, url }) {
    const st = tabState(tab_id);
    // A URL that is a download is held, never the browser's (D35). A page the debugger can't attach to (a new-tab page,
    // chrome:// or brave://, the Web Store, another extension) is left for a blank page first; only if that fails too
    // does the URL not load, as a fact the agent can act on (e.g. open a FERB tab instead).
    try { await arm(tab_id); }
    catch (first) {
      try {
        st.leaving = true;
        await load(tab_id, "about:blank");
        await arm(tab_id);
      } catch (e) {
        const now = await chrome.tabs.get(tab_id).catch(() => ({}));
        return { navigated: false, reason: "download_capture_unavailable", tab_id, url: maskUrl(now.url || ""),
                 detail: `FERB couldn't prepare this tab to hold downloads (${first.message || first}; after a blank ` +
                         `page: ${e.message || e}), so it didn't load the URL. A tab FERB opens itself can.` };
      } finally { st.leaving = false; }
    }
    st.leaving = true; st.frames = {};
    try { await load(tab_id, url); }
    finally { st.leaving = false; }
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
    await dropDownloads(tab_id, true);
    try { await chrome.tabs.remove(tab_id); }
    catch (e) { if (!/No tab with id/i.test(String(e && e.message))) throw e; }
    TABS.delete(tab_id); // Closing the tab destroys its debugger target; onRemoved may arrive later.
    await mutateSession(s => {s.owned = s.owned.filter(o => o.tabId !== tab_id);});
    return { closed: true };
  },
  async release_tab({tab_id, keep_open = false}) {
    let tab;
    try { tab = await chrome.tabs.get(tab_id); }  // missing tab is not a successful keep-open handoff
    catch (e) {
      if (keep_open || !/No tab with id/i.test(String(e && e.message))) throw e;
      TABS.delete(tab_id);                        // e.g. a popup that closed itself: nothing left to let go of
      await mutateSession(s => {s.owned = s.owned.filter(o => o.tabId !== tab_id);});
      return {tab_id, released:true, kept_open:false, gone:true, url:'', window_id:null};
    }
    if (keep_open) await rememberKeptTab(tab_id);  // persist before detach/session cleanup, safe after lost ACK
    await detach(tab_id);
    await mutateSession(s => {s.owned = s.owned.filter(o => o.tabId !== tab_id);});
    return {tab_id, released:true, kept_open:keep_open || (await keptTabs()).includes(tab_id),
            url:maskUrl(tab.url || tab.pendingUrl || ''), window_id:tab.windowId};
  },
  // Trusted wheel at the scroller's centre: the window, or a container from the latest snapshot.
  async scroll({ tab_id, frame_id = 0, direction, container, dy }) {
    const cid = container || null;
    try { await attach(tab_id); }
    catch (e) {
      if (!e.debuggerRefused) throw e;
      throw refusedCommand(`${refusedWhy(e)}, so its wheel can't scroll it; nothing was scrolled.` +
        (await scriptable(tab_id, frame_id) ? " browser.act with action=scroll on an element of the latest snapshot " +
          "scrolls that element's area with page events" : ""));
    }
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
  // L26: one media element of the latest snapshot, in its exact frame, through the content script's own controls.
  async media({ tab_id, frame_id = 0, snap, index, action, to = null }) {
    const r = await inFrame(tab_id, frame_id, (g, i, a, t) => window.__ferb.media(g, i, a, t), [snap, index, action, to]);
    if (r.error === "stale") throw Error("that media element is from an older snapshot; take a new snapshot");
    return r;
  },
  async find({tab_id, text}) {
    const results = await chrome.scripting.executeScript({target:{tabId:tab_id,allFrames:true},
      func:q => {
        const needle=(q || "").toLowerCase();
        const lines=(document.body ? document.body.innerText : "").split(/\n+/).map(line=>line.trim()).filter(Boolean);
        return {lines:lines.length,hits:needle ? lines.filter(line=>line.toLowerCase().includes(needle)) : []};
      },args:[text]});
    const hits=results.flatMap(r=>(r.result?.hits || []).map(text=>({frame_id:r.frameId,text})));
    return {count:hits.length,lines:results.reduce((n,r)=>n+(r.result?.lines || 0),0),hits};
  },
  // L31: `audible` and `muted` are the browser's own facts about a tab's sound, for a loop's probe without a snapshot.
  async list_tabs() {
    const tabs = await chrome.tabs.query({}), s = await session(), kept = new Set(await keptTabs());
    return { tabs: tabs.map((t) => ({ tab_id: t.id, host: (() => { try { return new URL(t.url).hostname; } catch (_) { return ""; } })(),
                                      url:t.url || "",title:t.title || "", active: t.active, window_id: t.windowId, kept_open:kept.has(t.id),
                                      audible: !!t.audible, muted: !!(t.mutedInfo && t.mutedInfo.muted),
                                      ferb: !kept.has(t.id) && s.windowId != null && t.windowId === s.windowId })) };
  },
  async snapshot({ tab_id, snap }) {
    tabState(tab_id).visual = null;
    if (tabState(tab_id).pendingDialog) return {frames:[{frame_id:0,url:(await chrome.tabs.get(tab_id)).url || "",title:"",text:"[Page blocked by pending dialog; use browser.dialog]",elements:[],truncated:true}],dialogs:tabState(tab_id).dialogs.splice(0)};
    await inject(tab_id);
    const st = tabState(tab_id);
    st.axErrors = [];
    let attached = false;
    try { await attach(tab_id); attached = true; }
    catch (e) { st.axErrors.push(String(e.message || e)); }
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
    // L33 (0.15.8, optional): where the tab is shown, so FERB's cursor can point at an element on screen
    return { frames, dialogs, window: await tabWindow(tab_id) };
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
    if (["type","select","press"].includes(kind) && typeof text !== "string")
      return {ok:false,reason:"explicit_input_required",detail:"No default key or replacement text is supplied"};
    const page = page_mode === true || kind === "select" || kind === "scroll";
    const pageAct = () => inFrame(tab_id, frame_id, (g, i, k, t) => window.__ferb.act(g, i, k, t), [snap, el, kind, text ?? null]);
    let state;
    try { state = await attach(tab_id); }
    catch (e) {
      if (!e.debuggerRefused) throw e;
      // The browser refused the debugger (debuggerRefused: e.g. another extension's frame in the tab): nothing was
      // sent. Real input needs the debugger, so a trusted act is a known failure that says so, never page events by
      // itself, and advises page events only where scripting reaches the frame. Page events, asked for, go through
      // scripting to the snapshot's frame (Chrome never lists or scripts another extension's frame for FERB). Nothing
      // watches this tab's dialogs, file choosers or downloads (owner, 2026-10-24: allowed, with guards): an act that
      // expects a dialog fails before anything is sent, and a click or press says where a download it starts goes.
      const why = refusedWhy(e);
      if (expect_dialog) return {ok: false, reason: "debugger_refused", detail: `${why}, so this tab's dialogs can't ` +
        "be watched or answered; nothing was clicked or typed"};
      if (!page) return {ok: false, reason: "debugger_refused", detail: `${why}, so real input can't reach it; ` +
        "nothing was done. " + (await scriptable(tab_id, frame_id) ? "page_mode=true sends page events to this frame"
          : "scripting can't reach this frame either, so FERB can't act on it in this tab")};
      const result = await pageAct();
      return {...result, debugger_refused: `${why}: page events only; this tab's dialogs, file choosers and downloads ` +
        "aren't watched", ...(result.ok && DOWNLOAD_KINDS.has(kind) ? {download_note: UNMANAGED_DOWNLOADS} : {})};
    }
    const before = state.fileChoosers.length;
    const result = page ? await pageAct()
      : await trusted(tab_id, { frame_id, snap, el, kind, text, expect_dialog, dialog_text, append, per_key });
    const choosers = state.fileChoosers.slice(before);
    return choosers.length ? {...result, file_choosers:choosers} : result;
  },
  // Resolve a random stamp in the explicitly selected frame; no picker or source substitution.
  async upload({ tab_id, frame_id, snap, el, paths }) {
    const token = Array.from(crypto.getRandomValues(new Uint8Array(8)), b => b.toString(16).padStart(2, "0")).join("");
    const stamped = await inFrame(tab_id, frame_id, (g, i, t) => window.__ferb.stamp(g, i, t), [snap, el, token]);
    if (!stamped.ok) return {...stamped, reason:"upload_target_unavailable", frame_id, transferred:false};
    const searches = [], targets = [];
    try {
      let state;
      try { state = await attach(tab_id); }
      catch (e) {
        if (!e.debuggerRefused) throw e;
        return {ok:false, reason:"debugger_refused", detail:`${refusedWhy(e)}, so a file can't be set on the field; ` +
          "nothing was uploaded", frame_id, transferred:false};
      }
      await Promise.all(state.sessions.values());
      for (const sessionId of [undefined, ...state.sessions.keys()]) {
        await cdp(tab_id,"DOM.getDocument",{depth:0},sessionId); // enables this exact target's DOM agent
        const result = await cdp(tab_id, "DOM.performSearch", {query:`[data-ferb-node="${token}"]`}, sessionId);
        searches.push({sessionId, searchId:result.searchId});
        if (result.resultCount > 1) return {ok:false, reason:"upload_target_ambiguous", detail:"multiple nodes have the requested binding", transferred:false};
        if (result.resultCount === 1) {
          const {nodeIds} = await cdp(tab_id, "DOM.getSearchResults", {searchId:result.searchId,fromIndex:0,toIndex:1}, sessionId);
          if (nodeIds?.length === 1 && nodeIds[0]) targets.push({sessionId,nodeId:nodeIds[0]});
        }
      }
      if (targets.length !== 1) return {ok:false, reason:targets.length ? "upload_target_ambiguous" : "upload_route_unavailable",
        detail:"the selected file input has no unique debugger binding", frame_id, transferred:false};
      const target = targets[0];
      await cdp(tab_id, "DOM.setFileInputFiles", {nodeId:target.nodeId,files:paths}, target.sessionId);
      const selected_files = await inFrame(tab_id, frame_id, (g,i) => window.__ferb.uploadFiles(g,i), [snap,el]);
      return {ok:true, detail:`${paths.length} file(s) set on the selected local upload input; inspect destination attachment`,
        frame_id, selected_files, transferred:true};
    } finally {
      for (const search of searches) {
        try { await cdp(tab_id,"DOM.discardSearchResults",{searchId:search.searchId},search.sessionId); } catch (_) { /* temporary query cleanup only */ }
      }
      try { await inFrame(tab_id, frame_id, t => window.__ferb.unstamp(t), [token]); } catch (_) { /* target may navigate after upload */ }
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
        try {
          const found=await commands.find({tab_id,text:value || ""});
          if (found.count) return {met:true,frames:[...new Set(found.hits.map(hit=>hit.frame_id))]};
        }
        catch (_) { /* page between documents */ }
      }
      await sleep(250);
    }
    return { met: false, detail: "timeout" };
  },
  // jpeg, and split into parts when one message would come near the 1 MB native-messaging line. Never refused for
  // what the page shows (L13): the reply's `mask` is the page's mask state before and after the capture (maskState)
  // and the daemon paints the secret fields over before anything else sees the picture. A page that can't be read
  // is still captured (protocol 15, owner: no screenshot refused): `unread` says why, `mask` keeps the look before
  // when only the look after failed, and `window` says where the tab's window is, for the daemon's Accessibility and
  // text-recognition masking. `withheld` (no picture) for a secret field it found but can't cover, and (with
  // `undebuggable` and `window`) for a page the debugger can't attach to, which the daemon may capture natively.
  // The tab is attached fully when its page can be scripted, with the debugger alone when not (`readyForCapture`);
  // `input_note` says when the input setup or readiness probe failed and the capture went on without it.
  async screenshot({ tab_id, format, quality, snap, _timeout_ms }) {
    const deadline = Date.now() + _timeout_ms;
    try { await arm(tab_id); }           // the debugger can't attach: no tab capture at all (round 3, I1)
    catch (e) { return await undebuggable(tab_id, e); }
    const inputNote = await readyForCapture(tab_id);
    const st = tabState(tab_id);
    st.shot = null; st.visual = null;
    const first = await maskState(tab_id, deadline);
    if (first.withheld && !first.unread) return {withheld: first.withheld};
    const before = snap == null || first.withheld ? null : JSON.stringify(await visualStates(tab_id));
    const { data } = await cdp(tab_id, "Page.captureScreenshot", { format: format || "jpeg", quality: quality || 70 });
    const last = first.withheld ? first : await maskState(tab_id, deadline);
    if (last.withheld && !last.unread) return {withheld: last.withheld};
    const extra = {...await unreadCapture(tab_id, first, last), ...(inputNote ? {input_note: inputNote} : {})};
    if (before !== null && !extra.unread) {
      if (before !== JSON.stringify(await visualStates(tab_id))) throw new Error("page_changed_during_screenshot");
      st.visual = {at: Date.now(), snap, states: before, points: new Map()};
    }
    if (data.length <= SHOT_PART) { st.shot = null; return { data, format: format || "jpeg", ...extra }; }
    st.shot = data;
    return { parts: Math.ceil(data.length / SHOT_PART), size: data.length, format: format || "jpeg", ...extra };
  },
  async record_frame({tab_id, part, capture_id, _timeout_ms}) {
    const deadline = Date.now() + _timeout_ms;
    const st = tabState(tab_id);
    if (part !== undefined) {
      const saved = st.recordFrame;
      if (!saved || saved.id !== capture_id || !Number.isInteger(part) || part < 0 || part*SHOT_PART >= saved.data.length)
        throw new Error('recording frame expired or invalid part');
      const data = saved.data.slice(part*SHOT_PART, (part+1)*SHOT_PART);
      if ((part+1)*SHOT_PART >= saved.data.length) st.recordFrame = null;
      return {data};
    }
    try { await arm(tab_id); }           // as a screenshot
    catch (e) { return await undebuggable(tab_id, e); }
    const inputNote = await readyForCapture(tab_id);
    const first = await maskState(tab_id, deadline);   // as a screenshot (L13): the page before and after, or unread
    if (first.withheld && !first.unread) return {withheld: first.withheld};
    const {data} = await cdp(tab_id, 'Page.captureScreenshot',
      {format:'jpeg', quality:70, fromSurface:true, captureBeyondViewport:false});
    const last = first.withheld ? first : await maskState(tab_id, deadline);
    if (last.withheld && !last.unread) return {withheld: last.withheld};
    const extra = {...await unreadCapture(tab_id, first, last), ...(inputNote ? {input_note: inputNote} : {})};
    if (data.length <= SHOT_PART) return {data, ...extra};
    const id = crypto.randomUUID(); st.recordFrame = {id, data};
    return {parts:Math.ceil(data.length/SHOT_PART), capture_id:id, ...extra};
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
  // Sealed owner/FERB input. The value is opened here, for this command id
  // and origin only, handed to that frame's isolated world, and never posted back or logged.
  async fill({ tab_id, frame_id, snap, el, sealed, origin, code_only, split_els }, id) {
    const st = await install();
    if (!st.key) throw new Error("not paired");
    let value;
    try { value = await unseal(st.key, sealed, id, origin); }
    catch (_) { return { ok: false, detail: "seal_refused" }; }            // another command or origin: nothing filled
    const r = await inFrame(tab_id, frame_id, (g, i, o, v, c, targets) => {
      // An already-open page can still hold the previous isolated helper after an extension update.
      // Never let that helper ignore the group argument and write the full code into the first field.
      if (targets !== null && window.__ferb.supportsSplitSecretFill !== true)
        return {ok:false, detail:'split_fill_helper_update_required'};
      return window.__ferb.fill(g, i, o, v, c, targets);
    },
      [snap, el, origin, value, code_only === true, split_els ?? null]);
    value = null;
    // Renderer input events can submit/reset/rebuild the form. Once input began, an unconfirmed final value
    // cannot become a known failure: onCommand persists unknown and the daemon's P19 ledger blocks blind retry.
    if (r.unknown === true) throw new Error("secret fill outcome unknown after input; take a fresh observation");
    return { ok: !!r.ok, detail: String(r.detail || "") };
  },
  async read_source({ tab_id }, id) {
    const t = await chrome.tabs.get(tab_id);
    const u = new URL(t.url || "about:blank");
    if (`${u.origin}${u.pathname}` !== FEED_URL) throw new Error("read_source is only for FERB's inbox feed");
    const [r] = await chrome.scripting.executeScript({ target: { tabId: tab_id },
      func: () => (document.documentElement ? document.documentElement.outerHTML : "") });
    const st = await install();
    if (!st.key) throw new Error("not paired");
    return { sealed: await seal(st.key, (r && r.result) || "", id, FEED_URL) };   // email content never in clear
  },
  // A native capture of this browser's window (L13): the mask state (maskState) of the active tab of this profile's
  // window at exactly these screen bounds, for the daemon to paint over, and that tab's URL (secret parameters
  // masked) and title (protocol 15: the daemon checks the title against the captured window's, and the front window
  // takes its tab from here). With several such windows (maximized ones share bounds), the one whose active tab's
  // title is the captured window's `title`: the window server names a Chromium window after its active tab. A
  // minimized window is never the one on screen. Geometry, URL and title only, never page content. complete:false:
  // the page can't be read.
  async window_secrets({ bounds, title, _timeout_ms }) {
    const deadline = Date.now() + _timeout_ms;
    const [x, y, w, h] = bounds;
    const wins = await chrome.windows.getAll({ windowTypes: ["normal", "popup"] });
    let matches = wins.filter((v) => v.state !== "minimized" && Math.abs(v.left - x) <= 2 &&
                               Math.abs(v.top - y) <= 2 && Math.abs(v.width - w) <= 2 && Math.abs(v.height - h) <= 2);
    const shown = (s) => String(s || "").replace(/\s+/g, " ").trim();
    if (matches.length > 1 && shown(title)) {
      const named = [];
      for (const v of matches) {
        const [t] = await chrome.tabs.query({ active: true, windowId: v.id });
        if (t && shown(t.title) === shown(title)) named.push(v);
      }
      matches = named.length === 1 ? named : matches;
    }
    if (matches.length !== 1) return {matched: false, reason: matches.length ? "ambiguous_window" : "window_not_found"};
    const [tab] = await chrome.tabs.query({ active: true, windowId: matches[0].id });
    if (!tab) return {matched: true, complete: false, reason: "no_active_tab"};
    const page = {url: maskUrl(tab.url || tab.pendingUrl || ""), title: String(tab.title || "")};
    const state = await maskState(tab.id, deadline);
    if (state.withheld) return {matched: true, complete: false, reason: state.withheld, tab: page};
    return {matched: true, complete: true, state, tab: page};
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
