// FERB spike 4 extension: hands, not brains. Executes daemon commands, reports results.
const HOST = "com.ferb.spike4";

// Exact CDP allowlist enforced here, not only in the daemon. Anything else is refused even if asked.
const CDP_ALLOW = new Set(["Input.dispatchMouseEvent", "Input.dispatchKeyEvent", "Input.insertText",
                           "Emulation.setFocusEmulationEnabled"]);

function waitComplete(tabId) {
  return new Promise((resolve) => {
    const check = (id, info) => {
      if (id === tabId && info.status === "complete") {
        chrome.tabs.onUpdated.removeListener(check);
        resolve();
      }
    };
    chrome.tabs.onUpdated.addListener(check);
    chrome.tabs.get(tabId).then((t) => { if (t.status === "complete") check(tabId, t); });
  });
}

// Runs in the page. Elements with ids, role, label, value, rect, visibility. Password values never sent.
function snapshotPage() {
  const els = [...document.querySelectorAll("a, button, input, textarea, select, [role]")].slice(0, 500);
  return {
    url: location.href,
    title: document.title,
    visibility: document.visibilityState,
    webdriver: navigator.webdriver,
    elements: els.map((el, i) => {
      const r = el.getBoundingClientRect();
      const isSecret = el.type === "password" || /one-time-code/.test(el.autocomplete || "");
      return {
        id: i,
        tag: el.tagName.toLowerCase(),
        role: el.getAttribute("role") || "",
        name: el.getAttribute("name") || el.id || "",
        label: String(el.getAttribute("aria-label") || el.innerText || el.placeholder || "").slice(0, 80),
        value: isSecret ? "[redacted]" : (typeof el.value === "string" ? el.value.slice(0, 200) : ""),
        rect: [r.x, r.y, r.width, r.height],
        visible: r.width > 0 && r.height > 0,
      };
    }),
  };
}

const handlers = {
  async open({ url }) {
    const tab = await chrome.tabs.create({ url, active: false });
    await waitComplete(tab.id);
    return { tabId: tab.id };
  },
  async snapshot({ tabId }) {
    const [res] = await chrome.scripting.executeScript({ target: { tabId }, func: snapshotPage });
    if (res.error) throw new Error(`snapshot failed: ${res.error.message || res.error}`);
    if (res.result == null) throw new Error("snapshot failed: no result");
    return res.result;
  },
  async focus({ tabId, selector }) {
    const [res] = await chrome.scripting.executeScript({
      target: { tabId }, args: [selector],
      func: (sel) => { const el = document.querySelector(sel); if (!el) return false; el.focus();
                       return document.activeElement === el; },
    });
    return { focused: res.result };
  },
  async tab_state({ tabId }) {
    const t = await chrome.tabs.get(tabId);
    const [res] = await chrome.scripting.executeScript({ target: { tabId }, func: () => document.visibilityState });
    return { active: t.active, visibility: res.result };
  },
  async focused({ tabId }) {
    const [res] = await chrome.scripting.executeScript({
      target: { tabId }, func: () => { const a = document.activeElement; return a ? (a.name || a.id || a.tagName) : ""; },
    });
    return { name: res.result };
  },
  // Input readiness probe: a listener in the extension's isolated world (invisible to the page) records whether
  // a debugger-dispatched mouse move arrived. The daemon sends the move, then asks.
  async input_probe_arm({ tabId }) {
    await chrome.scripting.executeScript({ target: { tabId }, func: () => {
      window.__ferbProbe = false;
      addEventListener("mousemove", (e) => { if (e.isTrusted) window.__ferbProbe = true; }, { once: true });
    } });
    return {};
  },
  async input_probe_seen({ tabId }) {
    const [res] = await chrome.scripting.executeScript({ target: { tabId }, func: () => window.__ferbProbe === true });
    return { seen: res.result };
  },
  async attach_debugger({ tabId }) {
    await chrome.debugger.attach({ tabId }, "1.3");
    return {};
  },
  async cdp({ tabId, method, params }) {
    if (!CDP_ALLOW.has(method)) throw new Error(`cdp method not allowed: ${method}`);
    return (await chrome.debugger.sendCommand({ tabId }, method, params || {})) || {};
  },
  async detach_debugger({ tabId }) {
    await chrome.debugger.detach({ tabId });
    return {};
  },
  async close({ tabId }) {
    await chrome.tabs.remove(tabId);
    return {};
  },
};

// MV3 service workers start only for events they listen to. Without onStartup, a browser restart with the
// extension already installed never runs this file, so nothing connects to the native host.
chrome.runtime.onStartup.addListener(() => {});

const port = chrome.runtime.connectNative(HOST);
port.onMessage.addListener(async (msg) => {
  try {
    const handler = handlers[msg.type];
    if (!handler) throw new Error(`unknown command: ${msg.type}`);
    port.postMessage({ id: msg.id, type: "result", result: await handler(msg.args || {}) });
  } catch (e) {
    port.postMessage({ id: msg.id, type: "error", error: String(e && e.message || e) });
  }
});
port.onDisconnect.addListener(() => console.log("native host disconnected:", chrome.runtime.lastError?.message));
port.postMessage({ type: "hello", ua: navigator.userAgent, extensionId: chrome.runtime.id,
                   version: chrome.runtime.getManifest().version });
