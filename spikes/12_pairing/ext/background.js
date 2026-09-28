// FERB spike 12: pairing + authenticated messages. Hands, not brains: no policy here.
const HOST = "com.ferb.spike12";

// MV3: register startup/installed listeners from the first version so the worker runs when a profile starts.
chrome.runtime.onStartup.addListener(() => ensurePort());
chrome.runtime.onInstalled.addListener(() => ensurePort());

let port = null;
let retryMs = 1000;

const b64e = (buf) => btoa(String.fromCharCode(...new Uint8Array(buf)));
const b64d = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));

async function getState() {
  let { ferb } = await chrome.storage.local.get("ferb");
  if (!ferb) {
    ferb = { installId: crypto.randomUUID(), key: null, seq: 0 };  // generated once per profile
    await chrome.storage.local.set({ ferb });
  }
  return ferb;
}

async function hmac(keyB64, text) {
  const k = await crypto.subtle.importKey("raw", b64d(keyB64), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return b64e(await crypto.subtle.sign("HMAC", k, new TextEncoder().encode(text)));
}

// Every message: install_id + type + body_json. Once paired: seq (strictly increasing) + mac over all of it.
async function send(type, body) {
  const st = await getState();
  const msg = { install_id: st.installId, type, body_json: JSON.stringify(body) };
  if (st.key) {
    st.seq += 1;
    await chrome.storage.local.set({ ferb: st });
    msg.seq = st.seq;
    msg.mac = await hmac(st.key, `${msg.install_id}|${msg.seq}|${msg.type}|${msg.body_json}`);
  }
  ensurePort().postMessage(msg);
}

function ensurePort() {
  if (port) return port;
  port = chrome.runtime.connectNative(HOST);
  port.onMessage.addListener(async (m) => {
    if (m.type === "paired" && m.key) {
      const st = await getState();
      st.key = m.key; st.seq = 0;
      await chrome.storage.local.set({ ferb: st });
      await send("paired_ack", {});
    } else if (m.type === "open") {            // an action; the daemon only sends these to paired installs
      const t = await chrome.tabs.create({ url: m.url, active: false });
      await send("result", { id: m.id, tabId: t.id });
    } else if (m.type === "forget_pairing") {  // test helper: drop key (simulates a fresh install)
      await chrome.storage.local.remove("ferb");
    }
  });
  port.onDisconnect.addListener(() => {
    port = null;
    setTimeout(ensurePort, retryMs);
    retryMs = Math.min(retryMs * 2, 60000);    // host missing -> back off, don't spin
  });
  retryMs = 1000;
  getState().then((st) => send("hello", { version: chrome.runtime.getManifest().version, paired: !!st.key }));
  return port;
}

chrome.runtime.onMessage.addListener((m) => {
  if (m && m.pairToken) send("pair", { token: m.pairToken });
});

chrome.tabs.onUpdated.addListener((tabId, info, tab) => {
  if (info.status === "complete") send("tab_event", { tabId, url: tab.url });   // trail event (needs pairing)
});

ensurePort();
