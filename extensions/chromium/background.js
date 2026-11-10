// Owns authenticated browser transport and exact-target CDP mechanics; must not select a source/account or infer permission.
// Commands carry a connection generation and journaled ID. Stale/duplicate commands never execute.
// Task tabs open inactive (in the task's window, else the FERB minimized window), in one collapsed group per task
// (HAND-201, kb area hands). CDP focus emulation targets the tab only; the daemon sentinel measures OS effects
// rather than assuming every browser build stays background.
// A download in a tab FERB drives is held here and handed to the daemon in parts, never to the browser (D35).
// Daemon messages that act (welcome, cmd) are verified (HMAC, this connection's nonce, a counter) before anything runs.
importScripts("mask.js");
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
// A Web Store update Chrome downloaded but holds: Chrome installs an update only while the extension is idle, and the
// open line to FERB keeps it running, so without this it waited for a browser restart (HAND-430)
let storeUpdate = null;
let earlyCheckFor = null;              // the minimum version an early store check was asked for (once each)
let handshakeTimer = null;
const resultTransfers = new Map();
const commandTransfers = new Map();
// Results/events waiting for a connection that said welcome; signed only when posted, so they leave after the hello
// with fresh seqs (protocol 18: queued messages were flushed before the hello and dropped by the daemon). Bounded: a
// connection refused for a long time drops the oldest.
const outbox = [];
const OUTBOX_MAX = 5000;
let link = { nonce: null, seq: 0 };   // this connection's daemon-message signing state (its hello's nonce, last counter)
let pairingPending = null;           // when this profile's pairing page handed over a token (a `paired` is taken only then)
const PAIR_WAIT_MS = 300000;         // pairing.py TOKEN_TTL_S
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
  return s || { seen: {}, order: [], windowId: null };
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

// ---------- FERB's tabs in this browser session (HAND-201: tab groups) ----------
// In local storage, so a worker restart or an extension reload keeps them (a new browser session clears them:
// onStartup). tabs: tab id -> {task, root, cid (the opening command), borrowed?, group?}; groups: group id -> {root,
// windowId, title}. A tab FERB opened is in its task's collapsed group; a borrowed owner tab is recorded only so it is
// re-adopted after a reconnect, and is never grouped or closed.
let recordsLock = Promise.resolve();
async function records() {
  const { ferbTabs, ferbGroups } = await chrome.storage.local.get(["ferbTabs", "ferbGroups"]);
  return { tabs: ferbTabs && typeof ferbTabs === "object" ? ferbTabs : {},
           groups: ferbGroups && typeof ferbGroups === "object" ? ferbGroups : {} };
}
function mutateRecords(change) {                   // one read/modify/write at a time; `change` never nests one
  const next = recordsLock.then(async () => {
    const r = await records();
    const out = await change(r);
    await chrome.storage.local.set({ ferbTabs: r.tabs, ferbGroups: r.groups });
    return out;
  });
  recordsLock = next.catch(() => {});
  return next;
}

// The records the hello reports (`tabs`): each tab still open and, when FERB opened it into a group, still in that
// group with its title (else it is the owner's now: moved out while nobody watched, or ids of another browser session).
async function liveRecords() {
  const listed = [];
  await mutateRecords(async (r) => {
    for (const [key, rec] of Object.entries(r.tabs)) {
      const id = Number(key);
      let tab = null;
      try { tab = await chrome.tabs.get(id); } catch (_) { /* closed */ }
      let ok = !!tab && (rec.borrowed || rec.group == null);
      if (tab && !ok && tab.groupId === rec.group && r.groups[rec.group]) {
        try { ok = (await chrome.tabGroups.get(rec.group)).title === r.groups[rec.group].title; } catch (_) { ok = false; }
      }
      if (!ok) { delete r.tabs[key]; continue; }
      listed.push({ tab_id: id, task: rec.task ?? null, root: rec.root ?? null, borrowed: !!rec.borrowed,
                    cid: rec.cid ?? null, url: maskUrl(tab.url || tab.pendingUrl || "") });
    }
    for (const gid of Object.keys(r.groups))
      if (!Object.values(r.tabs).some((t) => String(t.group) === gid)) delete r.groups[gid];
  });
  return listed;
}

const grouping = new Set();          // tabs FERB is putting into a group right now: their move is FERB's, not the owner's
const groupJoins = new Map();        // task root -> the end of its chain of joins (one group per task, never two)
function joinGroup(tabId, windowId, g) {
  if (!g || typeof g.root !== "string" || !g.root) return Promise.resolve(null);
  const prev = groupJoins.get(g.root) || Promise.resolve();
  const next = prev.then(() => joinGroupNow(tabId, windowId, g));
  const done = next.catch(() => null);
  groupJoins.set(g.root, done);
  done.then(() => { if (groupJoins.get(g.root) === done) groupJoins.delete(g.root); });
  return next;
}
// The task's group in this window, or a new one: labelled, grey and collapsed when made; FERB never opens it, and a
// tab added to a collapsed group keeps it collapsed. A popup window holds no groups: its tab stays ungrouped (still
// FERB's, closed with its task).
async function joinGroupNow(tabId, windowId, g) {
  let w;
  try { w = await chrome.windows.get(windowId); } catch (_) { return null; }
  if (w.type !== "normal") return null;
  const r = await records();
  const known = Object.entries(r.groups).find(([, v]) => v.root === g.root && v.windowId === windowId);
  let gid = null, collapsed = true;
  if (known) {
    try { const live = await chrome.tabGroups.get(Number(known[0])); gid = live.id; collapsed = live.collapsed; }
    catch (_) { /* gone: a new one */ }
  }
  const title = typeof g.title === "string" && g.title.trim() ? g.title.trim().slice(0, 60) : "FERB";
  grouping.add(tabId);
  try {
    if (gid !== null) {
      await chrome.tabs.group({ groupId: gid, tabIds: [tabId] });
      if (collapsed) await chrome.tabGroups.update(gid, { collapsed: true });
    } else {
      gid = await chrome.tabs.group({ tabIds: [tabId], createProperties: { windowId } });
      await chrome.tabGroups.update(gid, { title, color: "grey", collapsed: true });
    }
    await mutateRecords((rr) => {
      if (!rr.groups[gid]) rr.groups[gid] = { root: g.root, windowId, title };
      if (rr.tabs[tabId]) rr.tabs[tabId].group = gid;
    });
  } finally {
    setTimeout(() => grouping.delete(tabId), 1000);   // its own onUpdated may come after the promise
  }
  return gid;
}

// No tab FERB opened is left in this group: the owner's tabs dragged into it leave it (they were never FERB's), and
// it is forgotten.
async function settleGroup(gid) {
  if (gid == null) return;
  const empty = await mutateRecords((r) => {
    if (Object.values(r.tabs).some((t) => t.group === gid)) return false;
    delete r.groups[gid];
    return true;
  });
  if (!empty) return;
  try {
    const left = await chrome.tabs.query({ groupId: gid });
    if (left.length) await chrome.tabs.ungroup(left.map((t) => t.id));
  } catch (_) { /* the group went with its last tab */ }
}

// The window the task's group is in, so its next tab joins it there (one group per task).
async function taskWindow(g) {
  if (!g || typeof g.root !== "string" || !g.root) return null;
  const r = await records();
  for (const [gid, v] of Object.entries(r.groups)) {
    if (v.root !== g.root) continue;
    try { return (await chrome.tabGroups.get(Number(gid))).windowId; } catch (_) { /* gone */ }
  }
  return null;
}

// Sound is part of background-first (HAND-207, protocol 19): a tab FERB opens for a task is muted (the browser
// mutes its output; the page itself is untouched), unless the task needs its sound (`mute_tab`). A tab that becomes the
// owner's (handed over, or moved out of its group) gets its sound back, only when FERB's extension muted it.
async function soundBack(tabId) {
  try {
    const t = await chrome.tabs.get(tabId);
    const m = t.mutedInfo || {};
    if (m.muted && m.reason === "extension" && m.extensionId === chrome.runtime.id) {
      await chrome.tabs.update(tabId, { muted: false });
    }
  } catch (_) { /* gone: nothing to give back */ }
}

// A tab FERB opened left its task's group (the owner dragged it out, into another group or window, or ungrouped the
// group): it is the owner's now. FERB lets go of it as of a borrowed tab (debugger off, a held download back to the
// browser), never closes it, and tells the daemon (`taken`), which stops using it.
async function tabMoved(tabId, groupId) {
  if (grouping.has(tabId)) return;
  const gone = await mutateRecords((r) => {
    const rec = r.tabs[tabId];
    if (!rec || rec.borrowed || rec.group == null) return null;
    const g = groupId >= 0 ? r.groups[groupId] : null;
    if (g && g.root === rec.root) { rec.group = groupId; return null; }
    delete r.tabs[tabId];
    return rec;
  });
  if (!gone) return;
  try { await dropDownloads(tabId); await detach(tabId); } catch (e) { console.warn("FERB taken tab:", String(e)); }
  await soundBack(tabId);
  send("event", { event: "taken", tab_id: tabId, reason: groupId >= 0 ? "moved_to_another_group" : "moved_out_of_group" });
  await settleGroup(gone.group);
}
chrome.tabs.onUpdated.addListener((id, info) => {
  if (info && Number.isInteger(info.groupId)) tabMoved(id, info.groupId).catch((e) => console.warn("FERB group:", String(e)));
});

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
// Signed and posted inside the lock, so messages leave in seq order. Until the daemon's welcome only the hello and a
// pairing go out; everything else waits in the outbox and leaves after the welcome (flushOutbox), signed then, so the
// daemon, which attributes a message to a profile only after its hello, never drops them as replays or strays.
function send(type, body) {
  return serially(async () => {
    if (!port || (gen === null && type !== "hello" && type !== "pair")) {
      outbox.push([type, body]);
      if (outbox.length > OUTBOX_MAX) outbox.shift();
      ensurePort();
      return;
    }
    await post(type, body);
  });
}
async function post(type, body) {                   // inside the state lock only
  const st = await install();
  const msg = { install_id: st.installId, type, body_json: JSON.stringify(body) };
  if (st.key) {
    st.seq += 1;
    await chrome.storage.local.set({ ferb: st });
    msg.seq = st.seq;
    msg.mac = await hmac(st.key, `${msg.install_id}|${msg.seq}|${msg.type}|${msg.body_json}`);
  }
  port.postMessage(msg);
}
function flushOutbox() {
  return serially(async () => {
    while (outbox.length && port && gen !== null) { const [type, body] = outbox.shift(); await post(type, body); }
  });
}

// A daemon message that acts (DAEMON_SIGNED), verified: its HMAC with the pairing key over this connection's hello
// nonce, its counter and its body, the counter above every one seen on this connection (an assembled command's parts
// were each verified so: `assembled` skips the counter). One at a time. null: not FERB's daemon, a replay, or no
// pairing, and nothing runs.
let verifyLock = Promise.resolve();
function daemonBody(m, assembled = false) {
  const p = verifyLock.then(() => verifyDaemon(m, assembled));
  verifyLock = p.catch(() => {});
  return p;
}
async function verifyDaemon(m, assembled) {
  if (!m || !DAEMON_SIGNED.has(m.type) || typeof m.body !== "string" || typeof m.mac !== "string" ||
      !Number.isSafeInteger(m.seq) || m.seq < 1) return null;
  const st = await install();
  if (!st.key || !link.nonce || (!assembled && m.seq <= link.seq)) return null;
  const want = await hmac(st.key, daemonMacText(link.nonce, m.seq, m.type, m.body));
  let diff = want.length ^ m.mac.length;
  for (let i = 0; i < want.length; i++) diff |= want.charCodeAt(i) ^ (m.mac.charCodeAt(i) || 0);
  if (diff !== 0) return null;
  if (!assembled) link.seq = m.seq;
  try { const body = JSON.parse(m.body); return body && typeof body === "object" ? body : null; }
  catch (_) { return null; }
}

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
    // Task tabs survive a reload (their records are in local storage and the next hello re-adopts them), so only work
    // in flight waits: a command, a held download, a gesture, a debugger session of a connection that can drive it.
    if (busy()) return;
    const {ferbUpdateAttempt} = await chrome.storage.local.get('ferbUpdateAttempt');
    const attempt = `${current}/p${FERB_PROTOCOL}->${disk.version}/p${requiredRelease?.protocol ?? FERB_PROTOCOL}`;
    if (ferbUpdateAttempt === attempt) return;  // a failed reload must not loop forever
    await chrome.storage.local.set({ferbUpdateAttempt: attempt});  // never overwrite pairing state
    await connectionStatus('updating', 'Loading local extension ' + disk.version);
    if (busy()) {
      await chrome.storage.local.set({ferbUpdateAttempt: null});
      return;
    }
    chrome.runtime.reload();
  })().catch(e => console.warn('FERB update check:', String(e.message || e))).finally(() => {updateCheck = null;});
  return updateCheck;
}

// HAND-430: install a Web Store update Chrome holds as soon as no FERB work is in flight, through the same safe reload
// as a local bundle (task tabs survive it; one attempt per version, never a loop). Busy now: try again in a minute.
async function applyStoreUpdate() {
  if (!storeUpdate) return false;
  if (busy()) { setTimeout(() => applyStoreUpdate().catch(() => {}), 60000); return false; }
  const attempt = `${chrome.runtime.getManifest().version}->store ${storeUpdate}`;
  const {ferbUpdateAttempt} = await chrome.storage.local.get('ferbUpdateAttempt');
  if (ferbUpdateAttempt === attempt) return false;     // that reload already happened and didn't take: never loop
  await chrome.storage.local.set({ferbUpdateAttempt: attempt});
  await connectionStatus('updating', 'Installing FERB extension ' + storeUpdate);
  if (busy()) { await chrome.storage.local.set({ferbUpdateAttempt: null}); return applyStoreUpdate(); }
  chrome.runtime.reload();
  return true;
}

chrome.runtime.onUpdateAvailable.addListener((details) => {
  storeUpdate = String((details && details.version) || 'new');
  applyStoreUpdate().catch(e => console.warn('FERB store update:', String(e.message || e)));
});

// HAND-430: FERB refused this extension as older than its minimum: the one case FERB knows an update exists, so ask the
// store now instead of waiting for Chrome's next check (Google: only "when you know an update is available")
async function askStoreForUpdate() {
  if (!requiredRelease || earlyCheckFor === requiredRelease.version ||
      !newerVersion(requiredRelease.version, chrome.runtime.getManifest().version)) return null;
  earlyCheckFor = requiredRelease.version;
  try { return (await chrome.runtime.requestUpdateCheck()).status || null; }
  catch (e) { console.warn('FERB store check:', String(e.message || e)); return null; }
}

// Busy for a reload: a command running, a download held, a gesture, or (only while a connection could drive it) a
// debugger session. Refused or disconnected, an attached tab drives nothing, and its records survive the reload.
function busy() {
  return !!(activeCommands || downloads.size ||
            [...TABS.values()].some(t => t.gesture || (t.attached && gen !== null)));
}

// Each hello carries a fresh nonce: the daemon signs this connection's messages over it (a recorded message can't
// be replayed on another connection), and the tabs FERB holds in this browser session (`tabs`, liveRecords).
async function announceHello() {
  const st = await install(), s = await session();
  link = { nonce: _hex(16), seq: 0 };
  await send("hello", { version: chrome.runtime.getManifest().version, protocol: FERB_PROTOCOL,
                        browser: navigator.brave ? "brave" : "chrome", paired: !!st.key,
                        owned: await ownedTabs(), seen: s.order.slice(-200).map((id) => ({ id, status: s.seen[id] })),
                        armed: armedTabs(), nonce: link.nonce, tabs: await liveRecords() });
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
    const w = await daemonBody(m);
    if (!w || !Number.isSafeInteger(w.gen)) { console.warn("FERB: an unsigned welcome was ignored"); return; }
    clearTimeout(handshakeTimer); refused = null; requiredRelease = null; gen = w.gen; retryMs = 1000;
    profileIdentity = w.identity === "ferb" ? "ferb" : "owner";
    await connectionStatus('connected');
    await flushOutbox();                                   // what waited for this connection, now in seq order
    return;
  }
  if (m.type === "pairing_required") {
    clearTimeout(handshakeTimer); gen = null; requiredRelease = null;
    await connectionStatus('pairing_required', 'Not paired with FERB yet: open FERB, then Settings → Browsers → Pair all');
    return;
  }
  if (m.type === "paired" && m.key) {
    // Only while this profile's own pairing page is pending (it handed over a token): a `paired` nobody started could
    // otherwise swap in a key someone else holds.
    if (pairingPending === null || Date.now() - pairingPending > PAIR_WAIT_MS) {
      console.warn("FERB: a pairing nobody started here was ignored"); return;
    }
    pairingPending = null;
    clearTimeout(handshakeTimer); refused = null; requiredRelease = null;
    await serially(async () => {
      const st = await install();
      st.key = m.key; st.seq = 0;
      await chrome.storage.local.set({ ferb: st });
      outbox.length = 0;                                   // signed under the old key / seq: never valid again
    });
    link = { nonce: _hex(16), seq: 0 };
    await send("hello", { version: chrome.runtime.getManifest().version, protocol: FERB_PROTOCOL,
                          browser: navigator.brave ? "brave" : "chrome", paired: true, owned: [], seen: [],
                          armed: armedTabs(), nonce: link.nonce, tabs: await liveRecords() });
    return;
  }
  if (m.type === "refused") {
    clearTimeout(handshakeTimer); gen = null; refused = m.reason || "refused";
    requiredRelease = Number.isInteger(m.required_protocol) && m.required_protocol > 0 &&
      typeof m.minimum_version === 'string' && /^\d+(?:\.\d+){0,3}$/.test(m.minimum_version)
      ? {protocol: m.required_protocol, version: m.minimum_version} : null;
    await connectionStatus('refused', refused);
    await askStoreForUpdate();
    await checkExtensionUpdate(); return;
  }
  if (m.type === "cmd") {
    const c = await daemonBody(m);
    if (!c) { console.warn("FERB: an unsigned or replayed command was dropped; nothing ran"); return; }
    await onCommand(c);
  }
}

// A new browser session: tab and group ids from the last one mean nothing now (they are reused), so FERB's records of
// them go; a group the browser restored is the owner's from now on (a documented residual of tab groups).
chrome.runtime.onStartup.addListener(() => {
  mutateRecords(r => { r.tabs = {}; r.groups = {}; })
    .then(() => chrome.storage.local.set({ ferbKeptTabs: [] }))
    .catch(e => console.warn("FERB records:", String(e)))
    .finally(() => ensurePort());
});
chrome.runtime.onInstalled.addListener(() => ensurePort());
chrome.alarms.create("ferb-reconnect", { periodInMinutes: 0.5 });
chrome.alarms.onAlarm.addListener(async () => {
  await checkExtensionUpdate();
  if (port && refused) await announceHello();  // retry after daemon updates too; refusal is not permanent
  else await ensurePort();
});
chrome.runtime.onMessage.addListener((m, sender, reply) => {
  if (m && m.pairToken) { pairingPending = Date.now(); send("pair", { token: m.pairToken }); }
  else if (m && m.ferbArmed && sender && sender.tab)                 // content.js: armed, and the hook's event names
    reply({ armed: !!(TABS.get(sender.tab.id) || {}).armed, token: FERB_TOKEN });
  // content.js asks before it keeps what it caught; "not armed" makes it hand the click back to the browser at once.
  else if (m && m.ferbDownload && sender && sender.tab)
    reply(pageDownload(sender.tab.id, sender.frameId || 0, m.ferbDownload, sender.documentId));
  else if (m && m.ferbDropped && sender && sender.tab)
    pageDropped(sender.tab.id, sender.frameId || 0, sender.documentId, m.ferbDropped.name, m.ferbDropped.count);
  else if (m && m.ferbOpen && sender && sender.tab) {           // answered once the daemon has (or after ~1 s)
    pageOpen(sender.tab.id, m.ferbOpen).then(reply, () => reply(false));
    return true;
  }
  else if (m && m.ferbPopup && sender && sender.tab) reply(pageHookPopup(sender.tab.id, sender.frameId || 0, m.ferbPopup));
});

// ---------- commands ----------

// L31: file-chooser interception for a held tab and its frame sessions (off only while it rests between a loop's rounds).
async function interceptPicker(tabId, enabled) {
  const t = TABS.get(tabId);
  await cdp(tabId, "Page.setInterceptFileChooserDialog", {enabled});
  for (const sid of (t ? t.sessions.keys() : [])) {
    try { await cdp(tabId, "Page.setInterceptFileChooserDialog", {enabled}, sid); } catch (_) { /* that frame is gone */ }
  }
}
// An acting command means a loop's round is running: a resting tab takes its file picker back first, so a rest(false)
// that was missed or failed still fails closed (L31). Reads (a probe's snapshot or find) leave it resting.
const ACTING = new Set(["act", "fill", "upload", "navigate", "media", "scroll", "pointer"]);
async function awake(tabId) {
  const t = TABS.get(tabId);
  if (!t || !t.resting) return;
  if (t.attached) {                        // detached: the next attach intercepts
    try { await interceptPicker(tabId, true); }
    catch (e) {                            // still resting: the next acting command tries again
      const failed = new Error(`picker_not_taken_back: FERB couldn't take this tab's file picker back for the round ` +
                               `(${e.message || e}), so it did nothing; try the action again`);
      failed.nothingSent = true;
      throw failed;
    }
  }
  t.resting = false;                       // only once the picker is FERB's again
}

async function onCommand(m) {
  activeCommands++;
  try { await executeCommand(m); }
  finally {
    activeCommands--;
    if (storeUpdate && !activeCommands) applyStoreUpdate().catch(() => {});   // HAND-430: the work it waited for ended
  }
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
  let body, nothingSent = false, watch = null;
  const run = newRun(), tabId = m.args && Number.isInteger(m.args.tab_id) ? m.args.tab_id : null;
  const pageOp = tabId !== null && DIALOG_BLOCKED.has(m.op) && m.args.part === undefined;
  try {
    if (!handler) throw beforeInput(new Error(`unknown op ${m.op}`));
    // 0.19.7 (HAND-442): a page showing a dialog does nothing else until it is answered, so a command for it is
    // refused at once with the dialog's type (its text only through `dialog`); a snapshot says so in its frame
    const pending = pageOp ? (TABS.get(tabId) || {}).pendingDialog : null;
    if (pending && m.op !== "snapshot" && m.op !== "close") throw dialogPending(pending);   // a tab may still close
    if (ACTING.has(m.op) && tabId !== null) await awake(tabId);
    if (INPUT_OPS.has(m.op) && tabId !== null) noteInput(tabId);
    if (pageOp) watch = watchDialog(tabId);
    const work = (async () => {
      // 0.19.6: a tab FERB reads or acts in is kept awake first (focus emulation), so a frozen page answers (wake);
      // never under a dialog (0.19.7): the debugger's call would wait for the dialog like the page does
      if (WAKES.has(m.op) && tabId !== null && m.args.part === undefined && !pending) await step(run, "wake", () => wake(tabId));
      return handler({ ...(m.args || {}), _timeout_ms: m.timeout_ms || 30000, _run: run }, m.id);
    })();
    // a dialog the page opens during the command (none expected) ends it at once, as a fact (dialogOpened)
    const value = await withTimeout(watch ? Promise.race([work, watch.opened.then((d) => dialogOpened(m.op, d, run))]) : work,
                                    m.timeout_ms || 30000, run);
    body = { id: m.id, gen: m.gen, status: "ok", value };
  } catch (e) {
    if (m.op === "pointer") cancelPointer(m.args.tab_id, m.id);
    body = { id: m.id, gen: m.gen, status: "error", error: String((e && e.message) || e) };
    nothingSent = !!(e && e.nothingSent);
    // 0.19.6, optional fields (an older daemon ignores them): the step the command was in when it failed or timed
    // out, and how long each step took, so the daemon can say where a page stopped answering
    if (run.step) body.step = run.step;
    if (Object.keys(run.steps).length) body.steps = { ...run.steps };
    if (e && e.dialog) body.dialog = e.dialog;           // 0.19.7, optional: the pending dialog's type
  } finally {
    if (watch) watch.done();
    if (INPUT_OPS.has(m.op) && tabId !== null) noteInput(tabId);   // a tab its input opens may follow the command
  }
  // A handler error may follow partial input; a timeout does not cancel the renderer operation. Only an error raised
  // before anything reached the page (nothingSent: refusedCommand, awake, the preparation before input, HAND-422) is a
  // known failure, and so is any read's (READ_OPS: it changed nothing, so retrying it is no repeat).
  const read = READ_OPS.has(m.op) || (m.op === "dialog" && !(m.args && m.args.decision));
  const outcome = body.status === "ok" ? (body.value && body.value.ok === false ? "failed" : "done")
    : nothingSent || read ? "failed" : "unknown";
  body.outcome = outcome;
  await mutateSession(s => {if (Object.hasOwn(s.seen, m.id)) s.seen[m.id] = outcome;});
  // download_part's data (DOWNLOAD_PART bytes, base64) goes whole: well under the host's 1 MiB message (protocol 20)
  if (body.status === "ok" && m.op !== "result_part" && m.op !== "download_part") {
    // ASCII JSON avoids splitting a Unicode scalar across native-message chunks.
    const serialized = JSON.stringify(body.value ?? null).replace(/[\u0080-\uffff]/g,c=>"\\u"+c.charCodeAt(0).toString(16).padStart(4,"0"));
    if (serialized.length > 100000) {
      resultTransfers.set(m.id, serialized);
      body.value = {result_parts:{id:m.id,total_chars:serialized.length}};
    }
  }
  await send("result", body);
}

// The command's deadline. A timeout can't cancel what a page or the browser is already doing, but from then on the
// command is given up (`run.aborted`): its handler stops at its next step instead of working on for nobody (0.19.6;
// before, a stuck read kept its tab's work going for minutes after FERB had moved on).
function withTimeout(p, ms, run) {
  let timer;
  const expired = new Promise((_, rej) => { timer = setTimeout(() => { if (run) run.aborted = true; rej(new Error("timeout")); }, ms); });
  return Promise.race([p, expired]).finally(() => clearTimeout(timer));
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// One command's run (0.19.6): the step in flight, the time each step took (`<step>_ms`; reads return them as `steps`,
// a failure as `step`/`steps`), and whether the command was given up. A helper called without one gets its own.
function newRun() { return { aborted: false, step: null, steps: {} }; }
class Aborted extends Error {}
async function step(run, name, fn) {
  if (run.aborted) throw new Aborted(`given up before ${name}`);
  run.step = name;
  const t0 = Date.now();
  try { return await fn(); }
  finally { run.steps[`${name}_ms`] = (run.steps[`${name}_ms`] || 0) + (Date.now() - t0); }
}

// Every script FERB runs in a page goes in at once (`injectImmediately`, 0.19.6), never waiting for each frame to
// reach document_idle: one frame that never finished loading (a stuck embed or ad) held every read of its page until
// the command timed out (lab 2026-11-08: 96.7 s; at once, the same page read whole in 6.3 s). A read can so see a page
// still loading: the snapshot says so (`loading`), and a frame not parsed yet gives less, never an error.
const script = (injection) => chrome.scripting.executeScript({ ...injection, injectImmediately: true });

// Chrome freezes the page of a tab that is in a group at the moment the group collapses, 2 s after its load ends: the
// task's first tab, which makes the task's collapsed group (HAND-201). A frozen page runs no script, so executeScript
// (every read's first step) never answers (lab 2026-11-08: 20 of 20 first tabs; joiners, added to a group already
// collapsed, 0 of 40). Focus emulation, which every tab FERB reads has had since its first read (inputSetup), keeps a
// page from freezing and wakes one that froze (0 of 4 froze with it set at open; 4 of 4 frozen tabs woke for a read).
// It is the debugger's own call, answered while the page is frozen; the owner sees nothing (the group stays collapsed).
// Sent when a tab is armed, before every command that reads or acts in it, and whenever Chrome reports one frozen.
const WAKES = new Set(["snapshot", "find", "wait", "screenshot", "record_frame", "point", "frame_origin", "pdf",
  "act", "fill", "upload", "media", "scroll", "pointer"]);
async function wake(tabId) {
  const t = TABS.get(tabId);
  if (!t || !t.attached) return false;               // not FERB's debugger: nothing to send it through
  try { await cdp(tabId, "Emulation.setFocusEmulationEnabled", { enabled: true }); return true; }
  catch (_) { return false; }                        // the read itself says what's wrong
}
function wakeFrozen(id, info) { if (info && info.frozen === true) wake(id).catch(() => {}); }

// ---------- JavaScript dialogs as facts (0.19.7, HAND-442) ----------
// A page showing alert, confirm, prompt or "Leave site?" (beforeunload) runs nothing else, so every command that reads
// or acts in it would wait out its timeout (live 2026-11-09: a navigate 20 s, a wait 10 s, a snapshot 30 s). Instead a
// command for a tab with a dialog pending is refused at once, and one that is running when the page opens a dialog
// nobody declared ends at once: a read or a refused command with the error `dialog_pending` and `dialog: {type}`; an
// input command with what it delivered and `dialog`; navigate and close with `reason: "dialog_pending"`. Code never
// answers it: the worker reads it and decides with `dialog`, or said beforehand (act's expect_dialog, navigate's and
// close's leave_page). Its text goes only through `dialog`, which labels it as the page's.
const DIALOG_BLOCKED = new Set([...WAKES, "navigate", "close", "read_source"]);
const dialogFact = (d) => ({ type: d.type });
function dialogPending(d) {
  const e = beforeInput(new Error(`dialog_pending: the page shows a ${d.type} dialog and does nothing else until it is ` +
                                  "answered; read it and answer it with the dialog tool (accept or dismiss)"));
  e.dialog = dialogFact(d);
  return e;
}
const dialogWatchers = new Map();      // tabId -> Set of watchers of the commands running in it
function watchDialog(tabId) {
  const w = {};
  w.opened = new Promise((resolve) => { w.fire = resolve; });
  if (!dialogWatchers.has(tabId)) dialogWatchers.set(tabId, new Set());
  dialogWatchers.get(tabId).add(w);
  w.done = () => { const set = dialogWatchers.get(tabId); if (set) { set.delete(w); if (!set.size) dialogWatchers.delete(tabId); } };
  return w;
}
function dialogOpened(op, d, run) {
  run.aborted = true;                                  // the command's handler goes no further
  const fact = dialogFact(d);
  if (op === "navigate") return { ok: false, navigated: false, reason: "dialog_pending", dialog: fact,
    detail: `the page opened a ${d.type} dialog instead of leaving; nothing else happens until it is answered: read ` +
            "it and answer with the dialog tool (for a beforeunload dialog, accept leaves the page and dismiss stays), " +
            "or say leave_page when navigating" };
  if (op === "close") return { ok: false, closed: false, reason: "dialog_pending", dialog: fact,
    detail: `the page opened a ${d.type} dialog instead of closing; answer it with the dialog tool (accept closes the ` +
            "tab, dismiss keeps it), or say leave_page when closing" };
  if (op === "act" || op === "pointer") return { ok: true, dialog: fact,
    detail: `the input reached the page, and the page then opened a ${d.type} dialog, which waits for an answer; read ` +
            "it and answer it with the dialog tool" };
  if (READ_OPS.has(op)) throw dialogPending(d);
  // a sealed fill, an upload, media or a scroll the dialog cut short: how much of it the page took isn't known
  const e = new Error(`dialog_opened: the page opened a ${d.type} dialog during this; answer it with the dialog tool, ` +
                      "then look at the page");
  e.dialog = fact;
  throw e;
}

// A tab the task's own input opened (0.19.7, HAND-443): within INPUT_OPENS_MS of an input command in a task tab, a new
// tab whose first address is http(s) is checked: the browser's own openerId (Target.getTargetInfo, as for a hook's
// popup) must be that task tab's target. Then it is claimed like a popup (`popup` event; the daemon makes it the task's,
// so the opener's next result names it), even when the page opened it with no opener link (noopener: Chrome still
// reports the opener, measured). Any other new tab is let go of at once. No address or page text decides it.
const INPUT_OPS = new Set(["act", "pointer", "fill"]);
const INPUT_OPENS_MS = 3000;
function noteInput(tabId) { const t = TABS.get(tabId); if (t && t.armed) t.lastInput = Date.now(); }
chrome.tabs.onUpdated.addListener(wakeFrozen);

const maskUrl = (u) => ferbMaskUrl(u);               // mask.js: one rule with content.js

// Tabs FERB opened, by the command that opened them (the hello's `owned`: P19 reconciliation of an open).
async function ownedTabs() {
  const r = await records(), alive = [];
  for (const [key, rec] of Object.entries(r.tabs)) {
    if (rec.borrowed || !rec.cid) continue;
    try { const t = await chrome.tabs.get(Number(key)); alive.push({ id: rec.cid, tab_id: t.id, url: maskUrl(t.url || t.pendingUrl || "") }); }
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

// The worker's choice for a page that asks before it is left (navigate, close: `leave_page`, 0.19.7, HAND-442). Code
// never makes it (rule 4). "leave": the page hook skips the page's own beforeunload listeners for this leave (all its
// frames are told, at most 1 s), so no dialog is raised; one raised anyway (a page FERB's hook reached only after it
// loaded, or a listener the hook can't precede) is accepted by the debugger within milliseconds. "stay": a "Leave site?"
// dialog is dismissed at once and the command says it stayed (`stayed` resolves true). Chrome brings the tab forward
// in its window for that dialog (measured), so "leave" is the choice that keeps it in the background. No choice: the
// dialog is the command's answer (dialogOpened).
async function leaveChoice(tabId, choice) {
  const t = tabState(tabId);
  const none = { stayed: new Promise(() => {}), done() {} };
  if (choice !== "leave" && choice !== "stay") return none;
  let stay;
  const stayed = new Promise((resolve) => { stay = () => resolve(true); });
  t.onStay = stay;
  t.expect = { accept: choice === "leave", text: "", leave: true };
  if (choice === "leave") {
    try {
      await Promise.race([script({ target: { tabId, allFrames: true },
        func: (name) => { document.dispatchEvent(new CustomEvent(name)); return true; }, args: [FERB_TOKEN + ":leave"] }),
        sleep(1000)]);
    } catch (_) { /* a page scripts can't reach: the debugger still answers its dialog */ }
  }
  return { stayed, done() { if (t.onStay === stay) t.onStay = null; if (t.expect && t.expect.leave) t.expect = null; } };
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
  await script({ target, files: ["mask.js", "content.js"] });
}

async function inFrame(tabId, frameId, func, args) {
  await inject(tabId, [frameId]);
  const [r] = await script({ target: { tabId, frameIds: [frameId] }, func, args });
  if (!r || r.result == null) throw new Error("script failed in the page");   // null = threw (spike 4)
  return r.result;
}

// A capture's mask fields (protocol 15): both looks when the page was read; when it wasn't, `unread` (why), the look
// before if only the look after failed (`mask.after` null), and the tab's window (`tabWindow`).
// A capture asked for without the page's looks (`looks: false`, 0.19.6): unread, as a page scripts can't reach.
const NOT_ANSWERING = {withheld: "the page didn't answer FERB's last read in time, so its secret fields weren't looked " +
                                 "for", unread: true};
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
  try { await script({ target: { tabId, frameIds: [frameId] }, func: () => true }); return true; }
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
function maskState(tabId, deadline, run) {
  let timer;
  const expired = new Promise(resolve => { timer = setTimeout(resolve, Math.max(0, deadline - Date.now())); });
  const prev = maskLooks.get(tabId) || Promise.resolve();
  const turn = Promise.race([prev.then(() => true), expired.then(() => false)]);
  const look = turn.then(mine => mine && Date.now() < deadline && !(run && run.aborted) ? readMaskState(tabId, run)
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
async function readMaskState(tabId, run) {
  const round = crypto.randomUUID();
  let results;
  // a command given up (0.19.6) stops between rounds: the page is touched no further for nobody
  const live = () => { if (run && run.aborted) throw new Aborted("given up"); };
  try {
    await inject(tabId);
    const target = {tabId, allFrames: true}, args = [round];
    live();
    await script({target, args, func: r => window.__ferb.listenForFrames(r)});
    live();
    await script({target, args, func: r => window.__ferb.announceFrame(r)});
    live();
    results = await script({target, args, func: r => window.__ferb.captureState(r)});
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
  // whole: the whole document (not only what the viewport shows) holds no secret field and no frame that might: what a
  // PDF of the tab needs, since nothing paints over a PDF (extension 0.19.3, `pdf`)
  return {top: top.documentId, viewport: top.viewport, items, whole: isClean(top)};
}

// ---------- the debugger session: attached on a task tab's first action, kept for its life ----------

const TABS = new Map();          // tabId -> { attached, armed, ready, detached, dialogs, expect, leaving, frames, ax, shot }
function tabState(tabId) {
  let t = TABS.get(tabId);
  if (!t) { t = { attached: false, armed: false, ready: false, detached: null, dialogs: [], fileChoosers: [], inputErrors: new Map(), expect: null, leaving: false, frames: {}, sessions: new Map(), axErrors: [], shot: null,
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

// HAND-422: preparing a tab for input (the debugger, its frame sessions, focus emulation, FERB's viewport, the readiness
// probe) and reading where the target is send the page nothing the command asked for, so a failure there is a known
// one: marked nothingSent, the outcome is "failed". From the command's first input event on (a press, a move to the
// target, typed text, a page-script action, a file set on a field) an error stays "unknown" (executeCommand).
function beforeInput(e) {
  const failed = e instanceof Error ? e : new Error(String(e));
  failed.nothingSent = true;
  return failed;
}
async function prepared(step) {
  try { return await step(); }
  catch (e) { throw beforeInput(e); }
}

function attach(tabId) {
  return prepared(() => attachForInput(tabId));
}

async function attachForInput(tabId) {
  const t = tabState(tabId);
  if (!(t.attached && t.ready)) { await attachFresh(tabId); return t; }
  await Promise.all(t.sessions.values());
  if (!t.inputErrors.size) return t;
  // HAND-423: a frame session of this debugger session failed its setup and is still there. The debugger is let go and
  // attached again once (every frame session set up afresh), unless the tab holds a download for FERB, which letting
  // go would hand to the browser. Still failing: a known failure, nothing was sent.
  const why = () => [...t.inputErrors.values()].join("; ");
  if ([...downloads.values()].some(tr => tr.tabId === tabId))
    throw Error(`background_input_unavailable: ${why()}`);
  await letGoOfDebugger(tabId);
  await attachFresh(tabId);
  await Promise.all(t.sessions.values());
  if (t.inputErrors.size) throw Error(`background_input_unavailable: ${why()} (after attaching again)`);
  return t;
}

async function attachFresh(tabId) {
  const t = tabState(tabId);
  await arm(tabId);
  try {
    await inputSetup(tabId);
    await ready(tabId);                                        // the owner's own tab keeps its real viewport
    t.ready = true;
  } catch (e) {
    await letGoOfDebugger(tabId);
    throw e;
  }
}

async function letGoOfDebugger(tabId) {
  const t = tabState(tabId);
  await dropDownloads(tabId);
  t.attached = false; t.armed = false; t.ready = false; t.emulated = false;
  try { await chrome.debugger.detach({tabId}); } catch (_) {}
}

// What trusted input needs besides the debugger, before the readiness probe: file-chooser interception, focus
// emulation and, in FERB's own window, FERB's viewport.
async function inputSetup(tabId) {
  const t = tabState(tabId);
  // Intercept the browser mechanism, including indirect showPicker()/label/button routes.
  // The event supplies evidence; it never chooses a local/Drive source or opens a native picker.
  await cdp(tabId, "Page.setInterceptFileChooserDialog", {enabled: !t.resting});   // a resting tab's picker: the owner's
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
    t.sessions.clear(); t.inputErrors.clear();   // the old session's frame sessions, and their errors, went with it
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
  await wake(tabId);                                         // kept from freezing from the start (0.19.6, `wake`)
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
    await Promise.resolve(script({ target: { tabId, allFrames: true },
      func: (v, d, k) => { if (window.__ferb && window.__ferb.armDownloads) window.__ferb.armDownloads(v, d, k); },
      args: [on, drop, FERB_TOKEN] }));
  } catch (_) { /* a page scripts can't reach holds no listener of ours */ }
}

// HAND-418 (live 2026-11-04: an owner's tab whose page didn't answer kept `release_tab` waiting past its timeout,
// twice): letting go never waits on the page. What the browser itself holds for FERB (a held response) goes back first;
// what only the page can answer (content.js's holds, its disarm, the hook's removal, focus emulation off) is asked
// without waiting, since detaching the debugger ends the session's hook and emulation anyway, and a page that answers
// later still lets go.
const unawaited = (p) => { Promise.resolve(p).catch(() => {}); };
async function detach(tabId) {
  const t = TABS.get(tabId);
  if (!t || !t.attached) { TABS.delete(tabId); return; }
  // Keep the state until the browser acknowledges release. A failed detach is actionable, not success.
  t.leaving = true;
  if (t.armed) {                                             // let go of what this tab still holds, then of the page
    await dropDownloads(tabId);
    unawaited(armPage(tabId, false));
    if (t.hookId) {                                          // documents loaded from now on are native again
      unawaited(cdp(tabId, "Page.removeScriptToEvaluateOnNewDocument", { identifier: t.hookId }));
      t.hookId = null;
    }
  }
  unawaited(chrome.debugger.sendCommand({tabId}, 'Emulation.setFocusEmulationEnabled', {enabled:false}));
  try {
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
  t.resting = false;                       // fail closed (L31): the next session intercepts the file picker again
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
      const setup = (async () => {
        if (t.armed) {                                         // an OOPIF's own requests and its page hook
          await cdp(src.tabId, "Fetch.enable", { patterns: DOWNLOAD_PATTERNS }, sid);
          await installHook(src.tabId, sid);
        }
        await cdp(src.tabId, "Page.enable", {}, sid);
        await cdp(src.tabId, "Page.setInterceptFileChooserDialog", {enabled: !t.resting}, sid);
        await cdp(src.tabId, "Target.setAutoAttach", {autoAttach: true, waitForDebuggerOnStart: false, flatten: true}, sid);
      })().catch(e => {
        const error = String(e.message || e); t.axErrors.push(error);
        // A frame's error refuses input only while that frame session is there (HAND-423): a frame that went before
        // its setup ended (an ad frame that came and went) leaves nothing behind.
        if (t.sessions.get(sid) === setup) t.inputErrors.set(sid, error);
      });
      t.sessions.set(sid, setup);
    }
    return;
  }
  if (method === "Target.detachedFromTarget") {
    const t = tabState(src.tabId);
    t.sessions.delete(params.sessionId); t.inputErrors.delete(params.sessionId);
    return;
  }
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
  if (method === "Page.javascriptDialogClosed") {             // answered in the browser (the owner) or by FERB
    const t = TABS.get(src.tabId);
    if (t && t.pendingDialog) { t.pendingDialog.handled = params.result ? "accepted" : "dismissed"; t.pendingDialog = null; }
    return;
  }
  if (method !== "Page.javascriptDialogOpening") return;
  const t = TABS.get(src.tabId);
  if (!t) return;
  const d = { type: params.type, message: String(params.message || ""), default: String(params.defaultPrompt || ""), handled:"pending" };
  t.pendingDialog = d;
  t.dialogs.push(d);
  send("event", { event: "dialog", tab_id: src.tabId, ...d });
  // the worker's choice made beforehand: act's expect_dialog for any dialog, leave_page (`leave`) only for "Leave site?"
  if (t.expect && (!t.expect.leave || d.type === "beforeunload")) {
    const expected = t.expect; t.expect = null;
    commands.dialog({tab_id:src.tabId,decision:expected.accept ? "accept" : "dismiss",text:expected.text})
      .then(() => { if (expected.leave && !expected.accept && t.onStay) t.onStay(); })
      .catch(e => send("event",{event:"dialog",tab_id:src.tabId,...d,reason:String(e.message || e)}));
    return;
  }
  for (const w of dialogWatchers.get(src.tabId) || []) w.fire(d);   // nobody said: the running command ends with it
});

chrome.tabs.onRemoved.addListener(tabId => {
  forgetDownloads(tabId);
  TABS.delete(tabId);
  mutateRecords(r => { const rec = r.tabs[tabId]; delete r.tabs[tabId]; return rec ? rec.group : null; })
    .then(settleGroup).catch(e => console.warn('FERB tab cleanup:', String(e)));
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

// Saves a page made past the hold caps (here or content.js's own) are dropped: neither taken nor the browser's. Every
// one is reported, never silently (protocol 20): the first of a document's at once, then those that follow within the
// minute together in one event at its end (`count`), so a flood is a few facts, not one per save. content.js may
// report several at once (`count`). A tab no task drives any more (its document was let go of) has no one to tell.
const DROP_FACT_MS = 60000, DROP_DOCS_MAX = 200;
const dropFacts = new Map();         // document (tab/documentId, else tab/frame) -> {at, pending, name, timer}
function dropEvent(tabId, name, count) {
  send("event", { event: "download", tab_id: tabId, uncaptured: true, detail: "dropped", source: "page",
                  name: String(name || "").slice(0, 1024), count });
}
function pageDropped(tabId, frameId, documentId, name, count = 1) {
  const t = TABS.get(tabId);
  const n = Number.isSafeInteger(count) && count > 0 ? count : 1;
  if (!t || !t.armed || !port) return;
  const key = `${tabId}/${documentId || "frame-" + frameId}`, now = Date.now(), last = dropFacts.get(key);
  if (last && now - last.at < DROP_FACT_MS) {          // within the minute: told together at its end
    last.pending += n;
    last.name = last.name || name;
    if (!last.timer) last.timer = setTimeout(() => {
      const entry = dropFacts.get(key);
      if (!entry) return;
      entry.timer = null;
      if (entry.pending > 0) { dropEvent(tabId, entry.name, entry.pending); entry.pending = 0; entry.at = Date.now(); }
    }, DROP_FACT_MS - (now - last.at));
    return;
  }
  if (last && last.timer) clearTimeout(last.timer);
  if (last && last.pending > 0) dropEvent(tabId, last.name, last.pending);
  dropFacts.delete(key);
  dropFacts.set(key, { at: now, pending: 0, name: null, timer: null });
  if (dropFacts.size > DROP_DOCS_MAX) {                 // the oldest document's waiting count is told now, not lost
    const [oldKey, old] = dropFacts.entries().next().value;
    if (old.timer) clearTimeout(old.timer);
    if (old.pending > 0) dropEvent(Number(oldKey.split("/")[0]), old.name, old.pending);
    dropFacts.delete(oldKey);
  }
  dropEvent(tabId, name, n);
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
  if (!m) { openedByInput(tabId, url); return; }
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
// A new tab right after a task tab's input (0.19.7, HAND-443; INPUT_OPENS_MS): looked at only when it is in that task
// tab's window or Chrome names a task tab as its openerTabId (a popup window's), so the owner's own new tab in their
// window is never touched; the task's when the browser's openerId names that task tab, else let go of at once.
async function openedByInput(tabId, url) {
  if (!/^https?:/i.test(String(url || "")) || !port) return;
  const now = Date.now();
  let openers = [...TABS].filter(([id, t]) => id !== tabId && t.armed && t.lastInput && now - t.lastInput <= INPUT_OPENS_MS)
    .sort((a, b) => b[1].lastInput - a[1].lastInput).map(([id]) => id);
  if (!openers.length || (TABS.get(tabId) || {}).attached) return;
  let tab;
  try { tab = await chrome.tabs.get(tabId); } catch (_) { return; }
  const near = [];
  for (const id of openers) {
    try { const o = await chrome.tabs.get(id); if (o.windowId === tab.windowId || tab.openerTabId === id) near.push(id); }
    catch (_) { /* closed */ }
  }
  if (!near.length && !(Number.isInteger(tab.openerTabId) && (TABS.get(tab.openerTabId) || {}).armed)) return;
  openers = near.length ? near : openers;
  if ((TABS.get(tabId) || {}).attached) return;
  const t = tabState(tabId);
  t.claiming = true;
  try {
    await chrome.debugger.attach({ tabId }, "1.3");
    t.attached = true; t.detached = null; t.ready = false; t.armed = false;
    let opener = null;
    for (const id of openers) if (await openedBy(tabId, id)) { opener = id; break; }
    if (opener === null) {
      TABS.delete(tabId);
      try { await chrome.debugger.detach({ tabId }); } catch (_) { /* already gone */ }
      return;
    }
    await arm(tabId);
    t.popup = true;
    t.popupFor = { opener, frameId: null, token: null };   // loaded by the browser already: nothing to tell the page
    t.claiming = false;
    await send("event", { event: "popup", tab_id: tabId, opener_tab_id: opener, url: maskUrl(url) });
  } catch (e) { console.warn("FERB new tab:", String(e.message || e)); }
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
  for (const tr of [...downloads.values()]) {
    if (tr.tabId !== tabId) continue;
    const done = fail ? releaseTransfer(tr) : handBack(tr);
    if (tr.page) unawaited(done); else await done;           // a page's own hold: asked, never waited on (HAND-418)
  }
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

// Form submissions the page's own submit events showed (content.js, a capture-phase listener in every frame), taken
// (and so cleared) from every frame: the form's action host and method. A submission a page makes with fetch() fires
// no submit event, so only network observation could see it (not here). Best effort: a frame that went (the submit
// navigated it) says nothing.
const SUBMITTING = new Set(["click", "dblclick", "press"]);
async function formSubmits(tabId) {
  try {
    const results = await script({ target: { tabId, allFrames: true },
      func: () => (window.__ferb && window.__ferb.takeSubmits ? window.__ferb.takeSubmits() : []) });
    return results.flatMap((r) => Array.isArray(r && r.result) ? r.result.map((s) => ({ ...s, frame_id: r.frameId })) : []).slice(0, 10);
  } catch (_) { return []; }
}

async function trusted(tabId, a) {
  const t = await attach(tabId);
  t.expect = a.expect_dialog ? { accept: a.expect_dialog === "accept", text: a.dialog_text || "" } : null;
  try {
    const p = await prepared(() => topPoint(tabId, a.frame_id, a.snap, a.el));   // a read: nothing sent yet
    if (p.error) return { ok: false, detail: p.error };
    // A secret field (password, login code, card: content.js `fieldKind`) takes no typed text and no key that puts
    // text in (entersText); Return, Tab and the like still go (kb area hands).
    if (p.secret && (a.kind === "type" || (a.kind === "press" && entersText(a.text)))) return {ok: false, detail: "secret_field"};
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
      // focus may sit in a secret field inside the element (a widget around a password box): checked where it landed
      if (entersText(a.text) && await inFrame(tabId, a.frame_id, () => window.__ferb.focusedSecret(), []))
        return { ok: false, detail: "secret_field" };
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
    if (!f.token) return { ready: true, told: false };   // opened by the browser itself (openedByInput): already loading
    const told = await chrome.tabs.sendMessage(f.opener, { ferbPopupReady: f.token }, { frameId: f.frameId });
    return { ready: true, told: told === true };
  },
  // Past the task's tab cap (protocol 20): the daemon won't make this claimed popup the task's, so it is closed while
  // still blank (about:blank#ferb-<token>, nothing loaded), never left to load as an ordinary tab whose downloads would
  // skip every limit. Only a popup this worker claimed for a task tab (popupFor) is ever closed here.
  async popup_refused({ tab_id }) {
    const t = TABS.get(tab_id);
    if (!t || !t.popupFor) return { closed: false, reason: "not_a_claimed_popup" };
    t.popupFor = null;
    await dropDownloads(tab_id);
    TABS.delete(tab_id);
    try { await chrome.debugger.detach({ tabId: tab_id }); } catch (_) { /* already gone */ }
    try { await chrome.tabs.remove(tab_id); } catch (_) { return { closed: false, reason: "gone" }; }
    return { closed: true, tab_id };
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
      // the assembled message is the daemon's signed envelope: verified like any other (its parts were, one by one)
      const command = await daemonBody(JSON.parse(transfer.parts.join("")), true);
      if (!command || command.id!==id || command.op==="command_part") throw Error("invalid assembled command");
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
  // `group` ({root, task, title}): the task this tab is for; it joins the task's collapsed group, in the window the
  // group is in when no window is named (one group per task). No task (FERB's inbox read): no group.
  async open({ url, window_id = null, allow_foreground = false, group = null, muted = false }, id) {
    const windowId = window_id !== null ? (await chrome.windows.get(window_id)).id
                                        : (await taskWindow(group)) ?? await ferbWindow(allow_foreground);
    if (windowId === null) {
      const windows = await chrome.windows.getAll({populate:false});
      return {opened:false, reason:'window_selection_required', needs_foreground:false,
        new_window_needs_foreground:true,
        detail:'No FERB window is available. Choose an exact existing window_id for an inactive tab; creating a new window can switch Spaces and requires foreground permission.',
        windows:windows.map(w=>({id:w.id,state:w.state,focused:w.focused,type:w.type,incognito:w.incognito}))};
    }
    // Blank first, never active, never focused (E3); the URL loads once the tab can hold a download (D35).
    const tab = await chrome.tabs.create({ windowId, url: "about:blank", active: false });
    const g = group && typeof group === "object" ? group : null;
    await mutateRecords(r => { r.tabs[tab.id] = { task: g && typeof g.task === "string" ? g.task : null,
                                                  root: g && typeof g.root === "string" ? g.root : null, cid: id }; });
    try { await chrome.tabs.update(tab.id, { autoDiscardable: false }); } catch (_) { /* Memory Saver opt-out is best effort */ }
    let silent = false;                               // protocol 19: muted before anything loads (HAND-207)
    if (muted === true) {
      try { silent = !!((await chrome.tabs.update(tab.id, { muted: true })).mutedInfo || {}).muted; } catch (_) { /* said */ }
    }
    try { await arm(tab.id); }
    catch (e) {
      try { await chrome.tabs.remove(tab.id); } catch (_) { /* already gone */ }
      await mutateRecords(r => { delete r.tabs[tab.id]; });
      return { opened: false, reason: "download_capture_unavailable",
               detail: `FERB couldn't prepare a new tab to hold downloads, so it opened nothing (${e.message || e})` };
    }
    let groupId = null;
    try { groupId = await joinGroup(tab.id, windowId, g); }
    catch (e) { console.warn("FERB group:", String(e.message || e)); }   // ungrouped, still FERB's (closed with its task)
    if (url !== "about:blank") await load(tab.id, url);   // FERB's daemon opens blank and navigates once the tab is its
    const t = await chrome.tabs.get(tab.id);
    return { tab_id: t.id, url: maskUrl(t.url || t.pendingUrl || ""), title: t.title || "", window_id: t.windowId,
             group_id: groupId, muted: silent };
  },
  // Protocol 19 (HAND-207): mute or unmute one tab FERB opened (its records hold it, never borrowed), through the
  // browser's own tab mute; the owner's tabs are never touched. Returns the tab's sound state afterwards.
  async mute_tab({ tab_id, muted }) {
    if (typeof muted !== "boolean") throw Error("muted must be true or false");
    const rec = (await records()).tabs[tab_id];
    const t = await chrome.tabs.get(tab_id);
    if (!rec || rec.borrowed || (await keptTabs()).includes(tab_id)) {
      return { tab_id, changed: false, reason: "not_ferbs", muted: !!(t.mutedInfo && t.mutedInfo.muted),
               audible: !!t.audible };
    }
    const after = await chrome.tabs.update(tab_id, { muted });
    return { tab_id, changed: true, muted: !!(after.mutedInfo && after.mutedInfo.muted), audible: !!after.audible };
  },
  async navigate({ tab_id, url, leave_page }) {
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
    const choice = await leaveChoice(tab_id, leave_page);
    try {
      if (await Promise.race([load(tab_id, url).then(() => false), choice.stayed])) {
        const t = await chrome.tabs.get(tab_id);
        return { ok: false, navigated: false, reason: "stayed", tab_id, url: maskUrl(t.url || ""),
                 detail: "the page asked before leaving (it may hold unsaved changes) and FERB stayed, as leave_page said" };
      }
    } finally { st.leaving = false; choice.done(); }
    const t = await chrome.tabs.get(tab_id);
    return { tab_id: t.id, url: maskUrl(t.url || t.pendingUrl || ""), title: t.title || "" };
  },
  async close({ tab_id, leave_page }) {
    if ((await keptTabs()).includes(tab_id)) {
      const released = await commands.release_tab({tab_id, keep_open:true});
      return {...released, closed:false};
    }
    // FERB never closes a tab it didn't open (HAND-201): only one its records hold as opened by FERB.
    const rec = (await records()).tabs[tab_id];
    if (!rec || rec.borrowed) {
      let open = true;
      try { await chrome.tabs.get(tab_id); } catch (_) { open = false; }
      if (open) return { closed: false, released: false, reason: "not_ferbs",
                         detail: "FERB closes only tabs it opened; this one isn't recorded as FERB's, so it stays open" };
      return { closed: true, gone: true };                  // nothing left to close
    }
    const st = TABS.get(tab_id);
    if (st) st.leaving = true;
    const choice = st ? await leaveChoice(tab_id, leave_page) : { stayed: new Promise(() => {}), done() {} };
    await dropDownloads(tab_id, true);
    // the browser runs the page's beforeunload for this too (a "Leave site?" dialog, without activating the tab:
    // measured); a dialog nobody declared ends the command at once (dialogOpened), and the removal goes on if answered
    const removed = chrome.tabs.remove(tab_id).then(() => false,
      (e) => { if (!/No tab with id/i.test(String(e && e.message))) throw e; return false; });
    try {
      if (await Promise.race([removed, choice.stayed])) {
        st.leaving = false;
        return { ok: false, closed: false, reason: "stayed", detail: "the page asked before closing (it may hold unsaved changes) " +
                 "and FERB kept it open, as leave_page said" };
      }
    } finally { choice.done(); }
    TABS.delete(tab_id); // Closing the tab destroys its debugger target; onRemoved may arrive later.
    await mutateRecords(r => { delete r.tabs[tab_id]; });
    await settleGroup(rec.group);
    return { closed: true };
  },
  // L31 (0.15.10; owner, 2026-10-25: "give them back between rounds"): a held tab resting between a loop's rounds
  // keeps only what keeps its page running (the debugger, focus emulation); its file chooser is the owner's again, in
  // its frames too. A round turns the interception back on. A tab FERB doesn't hold is left alone.
  async rest({tab_id, resting}) {
    const t = TABS.get(tab_id);
    if (t) t.resting = resting === true;     // recorded even without a session: the next attach follows it
    if (!t || !t.attached) return {tab_id, resting: false, held: false};
    await interceptPicker(tab_id, !t.resting);
    return {tab_id, resting: t.resting, file_chooser_intercepted: !t.resting};
  },
  // 0.15.13: the owner clicked FERB's human-check notification, so this tab comes to the front in its own window
  // (restored if minimized, focused). Only on that click: FERB never brings a tab forward by itself.
  async show_tab({tab_id}) {
    const tab = await chrome.tabs.get(tab_id);
    const w = await chrome.windows.get(tab.windowId);
    await chrome.windows.update(tab.windowId, w.state === "minimized" ? {state: "normal", focused: true} : {focused: true});
    await chrome.tabs.update(tab_id, {active: true});
    return {tab_id, window_id: tab.windowId, shown: true};
  },
  // Letting go of a tab: a borrowed one is left where the work finished; one FERB opened (handed over: it holds the
  // result, or the owner asked to keep the task's tabs) leaves FERB's group and stays open. Its record goes first, so
  // FERB's own ungrouping isn't taken for the owner's move.
  async release_tab({tab_id, keep_open = false}) {
    let tab;
    try { tab = await chrome.tabs.get(tab_id); }  // missing tab is not a successful keep-open handoff
    catch (e) {
      if (keep_open || !/No tab with id/i.test(String(e && e.message))) throw e;
      TABS.delete(tab_id);                        // e.g. a popup that closed itself: nothing left to let go of
      await mutateRecords(r => { delete r.tabs[tab_id]; });
      return {tab_id, released:true, kept_open:false, gone:true, url:'', window_id:null};
    }
    if (keep_open) await rememberKeptTab(tab_id);  // persist before detach/session cleanup, safe after lost ACK
    const rec = await mutateRecords(r => { const x = r.tabs[tab_id]; delete r.tabs[tab_id]; return x || null; });
    if (rec && !rec.borrowed && rec.group != null) {
      try { await chrome.tabs.ungroup(tab_id); } catch (_) { /* already out of it */ }
      await settleGroup(rec.group);
    }
    if (keep_open && rec && !rec.borrowed) await soundBack(tab_id);   // the owner's now: its sound with it
    await detach(tab_id);
    return {tab_id, released:true, kept_open:keep_open || (await keptTabs()).includes(tab_id),
            url:maskUrl(tab.url || tab.pendingUrl || ''), window_id:tab.windowId};
  },
  // A tab that became a task's without `open` (protocol 18): a popup a task tab's page opened (it joins the task's
  // group when its window can hold one) or the owner's tab taken over (`borrowed`: recorded only, so a reconnect can
  // re-adopt it; never grouped, never closed).
  async adopt_tab({ tab_id, task = null, root = null, borrowed = false, title = null, muted = false }) {
    const tab = await chrome.tabs.get(tab_id);
    await mutateRecords(r => { r.tabs[tab_id] = { task: typeof task === "string" ? task : null,
                                                  root: typeof root === "string" ? root : null,
                                                  ...(borrowed === true ? { borrowed: true } : {}) }; });
    let groupId = null;
    if (borrowed !== true && typeof root === "string" && root) {
      try { groupId = await joinGroup(tab_id, tab.windowId, { root, task, title }); }
      catch (e) { console.warn("FERB group:", String(e.message || e)); }
    }
    let silent = !!(tab.mutedInfo && tab.mutedInfo.muted);
    if (borrowed !== true && muted === true) {        // a popup of a FERB tab is FERB's: muted as well (protocol 19)
      try { silent = !!((await chrome.tabs.update(tab_id, { muted: true })).mutedInfo || {}).muted; } catch (_) { /* said */ }
    }
    return { adopted: true, tab_id, group_id: groupId, muted: silent };
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
    const {target, point} = await prepared(async () => {          // reads where to scroll: nothing sent yet (HAND-422)
      const target = await inFrame(tab_id, frame_id, c => window.__ferb.scroller(c), [cid]);
      if (target.error) throw Error(target.error === 'stale' ? 'that container is from an older snapshot' : target.error);
      const point = await toTopPoint(tab_id, frame_id, {x:target.px, y:target.py, occluded:target.occluded});
      if (point.error || point.occluded) throw Error(point.error || 'scroll container is occluded');
      return {target, point};
    });
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
    const results = await script({target:{tabId:tab_id,allFrames:true},
      func:q => {
        const needle=(q || "").toLowerCase();
        const lines=(document.body ? document.body.innerText : "").split(/\n+/).map(line=>line.trim()).filter(Boolean);
        return {lines:lines.length,hits:needle ? lines.filter(line=>line.toLowerCase().includes(needle)) : []};
      },args:[text]});
    const hits=results.flatMap(r=>(r.result?.hits || []).map(text=>({frame_id:r.frameId,text})));
    return {count:hits.length,lines:results.reduce((n,r)=>n+(r.result?.lines || 0),0),hits};
  },
  // L31: `audible` and `muted` are the browser's own facts about a tab's sound, for a loop's probe without a snapshot.
  // URLs masked like every other path (protocol 18). `ferb`: a tab FERB opened (its records) or one in FERB's own
  // window, never the owner's.
  async list_tabs() {
    const tabs = await chrome.tabs.query({}), s = await session(), kept = new Set(await keptTabs());
    const mine = (await records()).tabs;
    return { tabs: tabs.map((t) => ({ tab_id: t.id, host: (() => { try { return new URL(t.url).hostname; } catch (_) { return ""; } })(),
                                      url: maskUrl(t.url || ""), title: t.title || "", active: t.active, window_id: t.windowId,
                                      kept_open: kept.has(t.id), audible: !!t.audible, muted: !!(t.mutedInfo && t.mutedInfo.muted),
                                      ferb: !kept.has(t.id) && ((mine[t.id] && !mine[t.id].borrowed) ||
                                                                (s.windowId != null && t.windowId === s.windowId)) })) };
  },
  // 0.19.6 (optional reply fields, no protocol change): `steps`, how long each step took (wake, attach, inject,
  // script, ax, unbind, in ms) with the accessibility pass's counts (ax_nodes, adopted) and `frames`; `loading` when
  // the tab was still loading as it was read (scripts go in at once, `script`). The debugger is attached (and the tab
  // woken) before anything is injected, so a frozen page is awake by the time a script goes in.
  async snapshot({ tab_id, snap, _run }) {
    const run = _run || newRun();
    tabState(tab_id).visual = null;
    if (tabState(tab_id).pendingDialog) return {frames:[{frame_id:0,url:maskUrl((await chrome.tabs.get(tab_id)).url || ""),title:"",text:"[Page blocked by pending dialog; use browser.dialog]",elements:[],truncated:true}],dialogs:tabState(tab_id).dialogs.splice(0)};
    const st = tabState(tab_id);
    st.axErrors = [];
    let attached = false;
    try { await step(run, "attach", () => attach(tab_id)); attached = true; }
    catch (e) { if (e instanceof Aborted) throw e; st.axErrors.push(String(e.message || e)); }
    await step(run, "inject", () => inject(tab_id));
    const results = await step(run, "script", () => script({ target: { tabId: tab_id, allFrames: true },
                                                           func: (g, a) => window.__ferb.snapshot(g, a), args: [snap, BIND_ATTR] }));
    const frames = results.filter((r) => r && r.result).map((r) => ({ frame_id: r.frameId, ...r.result }));
    const top = frames.find((f) => f.frame_id === 0);
    if (!top) throw new Error("snapshot failed in the page");
    let counts = {};
    const unbind = () => script({ target: { tabId: tab_id, allFrames: true },
                                  func: () => window.__ferb && window.__ferb.unbind && window.__ferb.unbind() });
    try { if (attached) counts = await step(run, "ax", () => accessibilitySnapshot(tab_id, snap, frames, run)) || {}; }
    finally {                                        // the linking attributes leave the page right after (protocol 18)
      if (run.aborted) unawaited(unbind());          // even for a command given up, never waited for
      else {
        try { await step(run, "unbind", unbind); }
        catch (_) { /* a frame went meanwhile: its document took them along */ }
      }
    }
    st.frames = composeFrames(frames);
    st.frameRefs = new Map(frames.map(f => [f.frame_id, f]));
    if (st.axErrors.length) top.truncated = true;
    if (st.axErrors.length) top.text = `[Accessibility view incomplete: ${st.axErrors.join("; ")}]\n${top.text}`;
    const dialogs = st.dialogs.splice(0);
    let loading = false;
    try { loading = (await chrome.tabs.get(tab_id)).status === "loading"; } catch (_) { /* gone: the read stands */ }
    // L33 (0.15.9, optional): where the tab is shown, so FERB's cursor can point at an element on screen
    return { frames, dialogs, window: await tabWindow(tab_id), ...(loading ? { loading: true } : {}),
             steps: { ...run.steps, ...counts, frames: frames.length } };
  },
  async point({tab_id, snap, x, y, image_width, image_height}) {
    const visual = tabState(tab_id).visual;
    await checkVisual(tab_id, visual, [0]);
    if (snap !== visual.snap || ![x,y,image_width,image_height].every(Number.isFinite) ||
        x < 0 || y < 0 || image_width <= 0 || image_height <= 0 || x >= image_width || y >= image_height)
      throw new Error("invalid_visual_point");
    const [width,height] = await inFrame(tab_id, 0, () => window.__ferb.viewport(), []);
    const point = await resolveVisualPoint(tab_id, snap, x * width / image_width, y * height / image_height);
    await checkVisual(tab_id, visual, point.frames);   // every frame the point goes through still as in the picture
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
    // a page-mode press uses the one key table (protocol.js keyEvent), with whether it puts text in (entersText)
    const key = kind === "press" ? (() => { const k = keyEvent(text);
      return k ? { key: k.key, code: k.code, keyCode: k.keyCode, modifiers: k.modifiers, enters: entersText(text) } : null; })() : null;
    const pageAct = () => inFrame(tab_id, frame_id, (g, i, k, t, y) => window.__ferb.act(g, i, k, t, y),
                                  [snap, el, kind, text ?? null, key]);
    const submits = SUBMITTING.has(kind);
    if (submits) await formSubmits(tab_id);          // what the page submitted before this act isn't this act's
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
      const submitted = submits ? await formSubmits(tab_id) : [];
      return {...result, debugger_refused: `${why}: page events only; this tab's dialogs, file choosers and downloads ` +
        "aren't watched", ...(result.ok && DOWNLOAD_KINDS.has(kind) ? {download_note: UNMANAGED_DOWNLOADS} : {}),
        ...(submitted.length ? {submitted} : {})};
    }
    const before = state.fileChoosers.length;
    const result = page ? await pageAct()
      : await trusted(tab_id, { frame_id, snap, el, kind, text, expect_dialog, dialog_text, append, per_key });
    const choosers = state.fileChoosers.slice(before);
    const submitted = submits ? await formSubmits(tab_id) : [];
    return {...result, ...(choosers.length ? {file_choosers: choosers} : {}), ...(submitted.length ? {submitted} : {})};
  },
  // Resolve a random stamp in the explicitly selected frame; no picker or source substitution.
  async upload({ tab_id, frame_id, snap, el, paths }) {
    const token = Array.from(crypto.getRandomValues(new Uint8Array(8)), b => b.toString(16).padStart(2, "0")).join("");
    const stamped = await inFrame(tab_id, frame_id, (g, i, t, a) => window.__ferb.stamp(g, i, t, a), [snap, el, token, STAMP_ATTR]);
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
        const result = await cdp(tab_id, "DOM.performSearch", {query:`[${STAMP_ATTR}="${token}"]`}, sessionId);
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
      try { await inFrame(tab_id, frame_id, (t, a) => window.__ferb.unstamp(t, a), [token, STAMP_ATTR]); } catch (_) { /* target may navigate after upload */ }
    }
  },
  async wait({ tab_id, until, value, _timeout_ms }) {
    const deadline = Date.now() + Math.max(250, (_timeout_ms || 30000) - 500);   // answers before the daemon gives up
    while (Date.now() < deadline) {
      let t;
      try { t = await chrome.tabs.get(tab_id); } catch (_) { return { met: false, detail: "tab closed" }; }
      if (until === "load" && t.status === "complete") return { met: true };
      if (until === "url" && urlHas(t.url, value)) return { met: true, url: maskUrl(t.url || "") };
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
  // 0.19.6 (optional, no protocol change): `looks: false` (the daemon's read of this page just timed out) captures
  // without the page's own looks, which would wait the same way: `unread`, so the daemon masks it as any page the
  // extension can't read (Accessibility and text recognition, B6). The reply carries `steps` (arm, ready, look,
  // capture, in ms). An older daemon never sends `looks`.
  async screenshot({ tab_id, format, quality, snap, looks, _timeout_ms, _run }) {
    const run = _run || newRun(), deadline = Date.now() + _timeout_ms;
    try { await step(run, "arm", () => arm(tab_id)); }   // the debugger can't attach: no tab capture (round 3, I1)
    catch (e) { if (e instanceof Aborted) throw e; return await undebuggable(tab_id, e); }
    const skip = looks === false;
    const inputNote = skip ? "" : await step(run, "ready", () => readyForCapture(tab_id));
    const st = tabState(tab_id);
    st.shot = null; st.visual = null;
    const first = skip ? NOT_ANSWERING : await step(run, "look", () => maskState(tab_id, deadline, run));
    if (first.withheld && !first.unread) return {withheld: first.withheld};
    const before = snap == null || first.withheld ? null : await visualStates(tab_id);
    const { data } = await step(run, "capture", () => cdp(tab_id, "Page.captureScreenshot",
                                                          { format: format || "jpeg", quality: quality || 70 }));
    const last = first.withheld ? first : await step(run, "look", () => maskState(tab_id, deadline, run));
    if (last.withheld && !last.unread) return {withheld: last.withheld};
    const extra = {...await unreadCapture(tab_id, first, last), ...(inputNote ? {input_note: inputNote} : {}),
                   steps: { ...run.steps }};
    // Points on the picture (HAND-480): each frame's surface (pointer.js SURFACE) before the capture, and the frames whose
    // surface changed during it, which no point may go through. A busy page is still captured: the secret fields are
    // painted from the two looks above (mask.page_masks), which this check never stood in for.
    if (before !== null && !extra.unread) {
      const after = new Map(await visualStates(tab_id)), unsteady = {};
      for (const [f, was] of before) {
        const part = was && after.get(f) ? surfaceChange(was, after.get(f)) : "can't be read";
        if (part) unsteady[f] = part;
      }
      st.visual = {at: Date.now(), snap, states: Object.fromEntries(before), unsteady, points: new Map()};
    }
    if (data.length <= SHOT_PART) { st.shot = null; return { data, format: format || "jpeg", ...extra }; }
    st.shot = data;
    return { parts: Math.ceil(data.length / SHOT_PART), size: data.length, format: format || "jpeg", ...extra };
  },
  // As a screenshot: `looks: false`, `steps` (0.19.6).
  async record_frame({tab_id, part, capture_id, looks, _timeout_ms, _run}) {
    const run = _run || newRun(), deadline = Date.now() + _timeout_ms;
    const st = tabState(tab_id);
    if (part !== undefined) {
      const saved = st.recordFrame;
      if (!saved || saved.id !== capture_id || !Number.isInteger(part) || part < 0 || part*SHOT_PART >= saved.data.length)
        throw new Error('recording frame expired or invalid part');
      const data = saved.data.slice(part*SHOT_PART, (part+1)*SHOT_PART);
      if ((part+1)*SHOT_PART >= saved.data.length) st.recordFrame = null;
      return {data};
    }
    try { await step(run, "arm", () => arm(tab_id)); }   // as a screenshot
    catch (e) { if (e instanceof Aborted) throw e; return await undebuggable(tab_id, e); }
    const skip = looks === false;
    const inputNote = skip ? "" : await step(run, "ready", () => readyForCapture(tab_id));
    // as a screenshot (L13): the page before and after, or unread
    const first = skip ? NOT_ANSWERING : await step(run, "look", () => maskState(tab_id, deadline, run));
    if (first.withheld && !first.unread) return {withheld: first.withheld};
    const {data} = await step(run, "capture", () => cdp(tab_id, 'Page.captureScreenshot',
      {format:'jpeg', quality:70, fromSurface:true, captureBeyondViewport:false}));
    const last = first.withheld ? first : await step(run, "look", () => maskState(tab_id, deadline, run));
    if (last.withheld && !last.unread) return {withheld: last.withheld};
    const extra = {...await unreadCapture(tab_id, first, last), ...(inputNote ? {input_note: inputNote} : {}),
                   steps: { ...run.steps }};
    if (data.length <= SHOT_PART) return {data, ...extra};
    const id = crypto.randomUUID(); st.recordFrame = {id, data};
    return {parts:Math.ceil(data.length/SHOT_PART), capture_id:id, ...extra};
  },
  // Extension 0.19.3 (protocol 21, added only: an older FERB never asks for it): the tab as a PDF, as Chrome prints it
  // (Page.printToPDF: backgrounds on, the page's own page size when it sets one), for a document FERB makes from a page
  // it opened. A read. Nothing paints over a PDF, so a page with a password, code or card field anywhere in it, or one
  // whose secret fields can't be found, is withheld (L13). Split into parts like a screenshot.
  async pdf({ tab_id, landscape, _timeout_ms, _run }) {
    const deadline = Date.now() + _timeout_ms;
    await arm(tab_id);
    const state = await maskState(tab_id, deadline, _run);
    if (state.withheld) return {withheld: state.withheld};
    if (!state.whole) return {withheld: "the page has a password, code or card field, and nothing can paint over it in a PDF"};
    const { data } = await cdp(tab_id, "Page.printToPDF", { printBackground: true, preferCSSPageSize: true,
                                                            landscape: landscape === true });
    const st = tabState(tab_id);
    if (data.length <= SHOT_PART) { st.pdf = null; return { data }; }
    st.pdf = data;
    return { parts: Math.ceil(data.length / SHOT_PART), size: data.length };
  },
  async pdf_part({ tab_id, part }) {
    const st = TABS.get(tab_id);
    if (!st || st.pdf == null || !Number.isInteger(part) || part < 0 || part * SHOT_PART >= st.pdf.length)
      throw new Error("no pdf to continue");
    const data = st.pdf.slice(part * SHOT_PART, (part + 1) * SHOT_PART);
    if ((part + 1) * SHOT_PART >= st.pdf.length) st.pdf = null;
    return { data };
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
  async window_secrets({ bounds, title, _timeout_ms, _run }) {
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
    await wake(tab.id);                              // a tab FERB holds is kept awake for its look (0.19.6)
    const state = await maskState(tab.id, deadline, _run);
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
// Protocol 19 (HAND-207): a tab's sound started or stopped, or it was muted or unmuted: the browser's own facts,
// with whether the tab is FERB's (as list_tabs says it) and, protocol 21, who last changed its mute (`muted_reason`).
// The daemon records a change in a tab that isn't FERB's as a
// disturbance fact when a FERB call was near; nothing about the page is sent.
async function soundEvent(tabId, info, tab) {
  const st = await install();
  if (!st.key) return;
  const s = await session(), kept = new Set(await keptTabs()), mine = (await records()).tabs;
  const changed = ["audible", "mutedInfo"].filter((k) => k in info).map((k) => (k === "mutedInfo" ? "muted" : k));
  await send("tab_event", { event: "sound", tab_id: tabId, audible: !!tab.audible,
                            muted: !!(tab.mutedInfo && tab.mutedInfo.muted), changed,
                            muted_reason: (tab.mutedInfo && tab.mutedInfo.reason) || null,
                            ferb: !kept.has(tabId) && ((mine[tabId] && !mine[tabId].borrowed) ||
                                                      (s.windowId != null && tab.windowId === s.windowId)) });
}
chrome.tabs.onUpdated.addListener((id, info, t) => {
  if (info && ("audible" in info || "mutedInfo" in info)) soundEvent(id, info, t).catch(() => {});
});

ensurePort();
