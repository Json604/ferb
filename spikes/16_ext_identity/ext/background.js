// Spike 16: hands only. open (background tab) / read / close, for the FERB identity profile experiment.
const HOST = "com.ferb.spike16";

function waitComplete(tabId, timeoutMs = 30000) {
  return new Promise((resolve) => {
    const done = () => { chrome.tabs.onUpdated.removeListener(check); clearTimeout(timer); resolve(); };
    const check = (id, info) => { if (id === tabId && info.status === "complete") done(); };
    const timer = setTimeout(done, timeoutMs);
    chrome.tabs.onUpdated.addListener(check);
    chrome.tabs.get(tabId).then((t) => { if (t.status === "complete") done(); });
  });
}

const handlers = {
  async open({ url }) {
    const tab = await chrome.tabs.create({ url, active: false });
    await waitComplete(tab.id);
    return { tabId: tab.id };
  },
  async read({ tabId }) {
    const [res] = await chrome.scripting.executeScript({ target: { tabId }, func: () => ({
      url: location.href, title: document.title, webdriver: navigator.webdriver,
      source: document.documentElement ? document.documentElement.outerHTML.slice(0, 20000) : "",
    }) });
    if (!res || res.result == null) throw new Error("read failed");
    return res.result;
  },
  async close({ tabId }) { await chrome.tabs.remove(tabId); return {}; },
};

chrome.runtime.onStartup.addListener(() => {});

// Reconnect with backoff when the relay drops; report why the last connection ended in the next hello.
let lastDisconnect = null;
function connect() {
  const port = chrome.runtime.connectNative(HOST);
  port.onMessage.addListener(async (msg) => {
    try {
      const h = handlers[msg.type];
      if (!h) throw new Error(`unknown command: ${msg.type}`);
      port.postMessage({ id: msg.id, type: "result", result: await h(msg.args || {}) });
    } catch (e) {
      port.postMessage({ id: msg.id, type: "error", error: String((e && e.message) || e) });
    }
  });
  port.onDisconnect.addListener(() => {
    lastDisconnect = { at: new Date().toISOString(), error: chrome.runtime.lastError?.message || null };
    setTimeout(connect, 5000);
  });
  port.postMessage({ type: "hello", extensionId: chrome.runtime.id, version: chrome.runtime.getManifest().version,
                     lastDisconnect });
}
connect();
