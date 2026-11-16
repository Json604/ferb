// The extension's own code under node, with fake Chrome APIs and a fake clock (no real browser, no real waiting).
// Usage: node harness.js <extension> <worker|hook> <scenario source>
//   worker: background.js (and what it imports) runs in a context with `fake` (tabs, windows, debugger calls, the
//           window updates and focus events), `advance(ms)` (runs the timers due, letting promises settle) and
//           `settle()`; the scenario is an async function body whose return value is printed as JSON.
//   hook:   protocol.js's page hook (ferbPageHook) runs in a fake page: `page` (calls the native functions saw, the
//           events the hook dispatched) and `armed(on)` (whether content.js says FERB drives the tab).
"use strict";
const vm = require("vm"), fs = require("fs"), path = require("path");
const [dir, mode, scenario] = process.argv.slice(2);

function clock() {
  let now = 1_800_000_000_000, seq = 0;
  const timers = new Map();
  class FakeDate extends Date {
    constructor(...a) { super(...(a.length ? a : [now])); }
    static now() { return now; }
  }
  const setTimeout_ = (fn, ms = 0, ...args) => { const id = ++seq; timers.set(id, { at: now + Math.max(0, ms), fn, args }); return id; };
  const clearTimeout_ = (id) => { timers.delete(id); };
  const settle = async () => { for (let i = 0; i < 20; i++) await new Promise((r) => setImmediate(r)); };
  async function advance(ms) {
    const end = now + ms;
    await settle();
    for (;;) {
      const due = [...timers].filter(([, t]) => t.at <= end).sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
      if (!due) break;
      timers.delete(due[0]);
      now = Math.max(now, due[1].at);
      due[1].fn(...due[1].args);
      await settle();
    }
    now = end;
    await settle();
  }
  return { FakeDate, setTimeout_, clearTimeout_, settle, advance, setInterval: () => 0, clearInterval: () => {} };
}

function event() {
  const listeners = [];
  return { listeners, addListener(fn) { listeners.push(fn); }, removeListener(fn) { const i = listeners.indexOf(fn); if (i >= 0) listeners.splice(i, 1); },
           hasListener(fn) { return listeners.includes(fn); }, fire(...args) { return listeners.map((fn) => fn(...args)); } };
}

// Any namespace or member the code reaches that a scenario didn't fake: a function answering nothing (never a
// thenable, so an await of it can't hang), with event methods.
const stub = new Proxy(function () { return Promise.resolve(undefined); }, {
  get: (o, k) => (k === "then" ? undefined : k === "addListener" || k === "removeListener" ? () => {} : stub),
});

function worker() {
  const c = clock();
  const fake = { tabs: new Map(), windows: new Map(), updates: [], cdp: [], attached: new Set(), targets: {}, created: [], ports: [] };
  const local = { ferb: { key: null } };
  const session = { s: { seen: {}, order: [], windowId: null } };
  const tabsOnCreated = event(), tabsOnUpdated = event(), tabsOnRemoved = event(), tabsOnActivated = event();
  const winFocus = event(), winRemoved = event(), dbgEvent = event(), dbgDetach = event(), onMessage = event();
  const missing = (what, id) => Promise.reject(new Error(`No ${what} with id: ${id}.`));
  const chrome = {
    runtime: { id: "ferbtest", getManifest: () => ({ version: "test" }), onMessage, connectNative: () => {
                 // each link the worker opens: what it posted, and its events (a scenario fires the daemon's lines)
                 const p = { posted: [], closed: false, postMessage(m) { if (p.closed) throw new Error("Attempting to use a disconnected port object"); p.posted.push(m); },
                             disconnect() { p.closed = true; }, onMessage: event(), onDisconnect: event() };
                 fake.ports.push(p);
                 return p;
               },
               onStartup: event(), onInstalled: event(), onUpdateAvailable: event(), lastError: undefined, requestUpdateCheck: stub, reload: stub },
    storage: { local: { get: async () => structuredClone(local), set: async (x) => { Object.assign(local, structuredClone(x)); } },
               session: { get: async () => structuredClone(session), set: async (x) => { Object.assign(session, structuredClone(x)); } } },
    tabs: {
      onCreated: tabsOnCreated, onUpdated: tabsOnUpdated, onRemoved: tabsOnRemoved, onActivated: tabsOnActivated,
      get: async (id) => (fake.tabs.has(id) ? { ...fake.tabs.get(id) } : missing("tab", id)),
      query: async () => [...fake.tabs.values()].map((t) => ({ ...t })),
      update: async (id, p) => { const t = fake.tabs.get(id); if (!t) return missing("tab", id); Object.assign(t, p); return { ...t }; },
      create: async (p) => { const id = 1000 + fake.created.length; fake.created.push(p); const t = { id, windowId: p.windowId, url: p.url, active: !!p.active }; fake.tabs.set(id, t); return { ...t }; },
      remove: async (id) => { fake.tabs.delete(id); }, group: stub, ungroup: stub,
    },
    windows: {
      WINDOW_ID_NONE: -1, onFocusChanged: winFocus, onRemoved: winRemoved, onCreated: event(), onBoundsChanged: event(),
      get: async (id) => (fake.windows.has(id) ? { id, ...fake.windows.get(id) } : missing("window", id)),
      getAll: async () => [...fake.windows].map(([id, w]) => ({ id, ...w })),
      update: async (id, p) => { const w = fake.windows.get(id); if (!w) return missing("window", id); fake.updates.push([id, p]); Object.assign(w, p, p.state === "minimized" ? { focused: false } : {}); return { id, ...w }; },
      create: stub,
    },
    debugger: {
      onEvent: dbgEvent, onDetach: dbgDetach,
      attach: async ({ tabId }) => { fake.attached.add(tabId); },
      detach: async ({ tabId }) => { fake.attached.delete(tabId); },
      sendCommand: async (target, method, params) => {
        fake.cdp.push([target.tabId, method]);
        if (method === "Target.getTargetInfo") return { targetInfo: fake.targets[target.tabId] || { targetId: "T" + target.tabId } };
        if (method === "Page.addScriptToEvaluateOnNewDocument") return { identifier: "hook-" + target.tabId };
        if (method === "Page.getFrameTree") return { frameTree: null };
        return {};
      },
    },
    scripting: { executeScript: async () => [{ frameId: 0, result: true }] },
    tabGroups: stub, alarms: stub, action: stub, permissions: stub, management: stub,
  };
  const ctx = vm.createContext({ chrome, crypto: globalThis.crypto, btoa, atob, TextEncoder, TextDecoder, URL, structuredClone,
    console, navigator: {}, Date: c.FakeDate, setTimeout: c.setTimeout_, clearTimeout: c.clearTimeout_,
    setInterval: c.setInterval, clearInterval: c.clearInterval, fake, advance: c.advance, settle: c.settle });
  ctx.importScripts = (f) => vm.runInContext(fs.readFileSync(path.join(dir, f), "utf8"), ctx, { filename: f });
  vm.runInContext(fs.readFileSync(path.join(dir, "background.js"), "utf8").replace(/ensurePort\(\);\s*$/, ""), ctx, { filename: "background.js" });
  return ctx;
}

function hook() {
  const calls = [], events = [];
  let on = false;
  class Element_ extends EventTarget {
    requestFullscreen() { calls.push("requestFullscreen"); return Promise.resolve(); }
    webkitRequestFullscreen() { calls.push("webkitRequestFullscreen"); }
  }
  class HTMLElement_ extends Element_ { click() { calls.push("click"); } }
  class HTMLAnchorElement_ extends HTMLElement_ {}
  class HTMLAreaElement_ extends HTMLElement_ {}
  class HTMLVideoElement_ extends HTMLElement_ { requestPictureInPicture() { calls.push("requestPictureInPicture"); return Promise.resolve({}); } }
  const document = new EventTarget();
  document.baseURI = "https://site.example/page";
  document.documentElement = new HTMLElement_();
  const dispatch = EventTarget.prototype.dispatchEvent;
  const sandbox = {
    EventTarget, CustomEvent, Event, URL, crypto: globalThis.crypto, console, Symbol, Object, Promise, Error, TypeError,
    Element: Element_, HTMLElement: HTMLElement_, HTMLAnchorElement: HTMLAnchorElement_, HTMLAreaElement: HTMLAreaElement_,
    HTMLVideoElement: HTMLVideoElement_, document, setTimeout: () => 0,
    focus() { calls.push("focus"); },
    open(url, target, features) { calls.push(["open", url, target, features]); return { closed: false, location: { href: String(url) } }; },
    print() { calls.push("print"); }, close() { calls.push("close"); },
    addEventListener() {},
  };
  sandbox.window = sandbox;
  sandbox.self = sandbox;
  const ctx = vm.createContext(sandbox);
  const token = "a1b2c3d4e5f6";
  // content.js's side: it answers "armed" while FERB drives the tab, and records what the hook tells it
  document.addEventListener(token + ":armed", (e) => { if (on) e.preventDefault(); });
  for (const name of ["surface", "open", "popup", "print", "closing"])
    document.addEventListener(token + ":" + name, (e) => events.push([name, e.detail ?? null]));
  const src = fs.readFileSync(path.join(dir, "protocol.js"), "utf8");
  vm.runInContext(src + `\nferbPageHook(${JSON.stringify(token)});`, ctx, { filename: "protocol.js" });
  ctx.page = { calls, events, Element: Element_, Video: HTMLVideoElement_ };
  ctx.armed = (v) => { on = v; };
  return ctx;
}

const ctx = mode === "hook" ? hook() : worker();
vm.runInContext(`(async () => {${scenario}\n})()`, ctx, { filename: "scenario.js" })
  .then((x) => process.stdout.write(JSON.stringify(x === undefined ? null : x) + "\n", () => process.exit(0)))
  .catch((e) => { console.error(e && e.stack || e); process.exit(1); });
