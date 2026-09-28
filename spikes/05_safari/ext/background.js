// FERB spike 5 (Safari): hands, not brains. Spike-only control channel: long-poll a local harness.
// (Spike 7 replaces this with SafariWebExtensionHandler -> daemon socket.)
const HARNESS = "http://127.0.0.1:47811";

const ELEMENTS = "a, button, input, textarea, select, [role]";
const SNAPSHOT = `(() => {
  const els = [...document.querySelectorAll(${JSON.stringify(ELEMENTS)})].slice(0, 500);
  return {
    url: location.href, title: document.title, visibility: document.visibilityState,
    webdriver: navigator.webdriver === true,
    elements: els.map((el, i) => {
      const r = el.getBoundingClientRect();
      const secret = el.type === "password" || /one-time-code/.test(el.autocomplete || "");
      return { id: i, tag: el.tagName.toLowerCase(), role: el.getAttribute("role") || "",
               name: el.getAttribute("name") || el.id || "",
               label: String(el.getAttribute("aria-label") || el.innerText || el.placeholder || "").slice(0, 80),
               value: secret ? "[redacted]" : (typeof el.value === "string" ? el.value.slice(0, 200) : ""),
               rect: [r.x, r.y, r.width, r.height], visible: r.width > 0 && r.height > 0 };
    }),
  };
})()`;

function waitComplete(tabId) {
  return new Promise((resolve) => {
    const check = (id, info) => {
      if (id === tabId && info.status === "complete") { browser.tabs.onUpdated.removeListener(check); resolve(); }
    };
    browser.tabs.onUpdated.addListener(check);
    browser.tabs.get(tabId).then((t) => { if (t.status === "complete") check(tabId, t); });
  });
}

async function run(tabId, code) {
  const [result] = await browser.tabs.executeScript(tabId, { code });
  if (result === undefined || result === null) throw new Error("script returned no result");
  return result;
}

const handlers = {
  async open({ url, active }) {
    const tab = await browser.tabs.create({ url, active: !!active });
    await waitComplete(tab.id);
    return { tabId: tab.id };
  },
  snapshot: ({ tabId }) => run(tabId, SNAPSHOT),
  // Act on an element by its snapshot id (same selector list and order as SNAPSHOT).
  script_click: ({ tabId, id }) => run(tabId,
    `(() => { const el = [...document.querySelectorAll(${JSON.stringify(ELEMENTS)})][${Number(id)}];
              if (!el) return {ok: false}; el.click(); return {ok: true}; })()`),
  script_set_value: ({ tabId, id, value }) => run(tabId,
    `(() => { const el = [...document.querySelectorAll(${JSON.stringify(ELEMENTS)})][${Number(id)}];
              if (!el) return {ok: false}; el.focus(); el.value = ${JSON.stringify(value)};
              el.dispatchEvent(new Event("input", {bubbles: true})); return {ok: true}; })()`),
  async activate({ tabId }) {
    const t = await browser.tabs.update(tabId, { active: true });
    await browser.windows.update(t.windowId, { focused: true });
    return {};
  },
  async close({ tabId }) { await browser.tabs.remove(tabId); return {}; },
};

async function loop() {
  const hello = { type: "hello", ua: navigator.userAgent, version: browser.runtime.getManifest().version };
  let outbox = hello;
  for (;;) {
    try {
      const r = await fetch(`${HARNESS}/poll`, { method: "POST", body: JSON.stringify(outbox) });
      outbox = { type: "idle" };
      const cmd = await r.json();
      if (!cmd || cmd.type === "none") continue;
      try {
        const handler = handlers[cmd.type];
        if (!handler) throw new Error(`unknown command: ${cmd.type}`);
        outbox = { id: cmd.id, type: "result", result: await handler(cmd.args || {}) };
      } catch (e) {
        outbox = { id: cmd.id, type: "error", error: String((e && e.message) || e) };
      }
    } catch (e) {
      outbox = hello;               // harness not up: retry, re-announce when it is
      await new Promise((r) => setTimeout(r, 1000));
    }
  }
}
loop();
