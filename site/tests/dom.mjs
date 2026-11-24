// A stand-in for the few browser pieces analytics.js touches: elements, a document, localStorage and the page's
// address. Selectors: `tag`, `[attr]`, `tag[attr]`, `tag[attr^="start"]`.

class Node {
  constructor(tag) {
    this.tagName = tag.toUpperCase();
    this.attrs = new Map();
    this.children = [];
    this.parent = null;
    this.listeners = {};
    this.hidden = false;
    this.textContent = "";
  }
  getAttribute(name) {
    return this.attrs.has(name) ? this.attrs.get(name) : null;
  }
  setAttribute(name, value) {
    this.attrs.set(name, String(value));
    if (name === "hidden") this.hidden = true;
  }
  appendChild(child) {
    child.parent = this;
    this.children.push(child);
    return child;
  }
  remove() {
    if (!this.parent) return;
    this.parent.children = this.parent.children.filter((c) => c !== this);
    this.parent = null;
  }
  addEventListener(type, fn) {
    (this.listeners[type] ||= []).push(fn);
  }
  dispatch(type, event = {}) {
    let node = this;
    const e = { target: this, ...event };
    while (node) {
      for (const fn of node.listeners[type] || []) fn(e);
      node = node.parent;
    }
  }
  click() {
    this.dispatch("click");
  }
  matches(selector) {
    const m = /^([a-z]*)(?:\[([a-z-]+)(?:\^="([^"]*)")?\])?$/.exec(selector);
    if (!m) throw new Error(`the fake DOM doesn't know the selector ${selector}`);
    return (
      (!m[1] || this.tagName === m[1].toUpperCase()) &&
      (!m[2] || this.attrs.has(m[2])) &&
      (m[3] === undefined || (this.getAttribute(m[2]) || "").startsWith(m[3]))
    );
  }
  closest(selector) {
    for (let n = this; n; n = n.parent) if (n.matches?.(selector)) return n;
    return null;
  }
  *walk() {
    for (const c of this.children) {
      yield c;
      if (c.walk) yield* c.walk();
    }
  }
  querySelectorAll(selector) {
    return [...this.walk()].filter((n) => n.matches?.(selector));
  }
  querySelector(selector) {
    return this.querySelectorAll(selector)[0] || null;
  }
  get text() {
    return this.textContent + this.children.map((c) => c.text ?? c.textContent).join("");
  }
}

export function fakePage({ tag = true, path = "/", stored = null, dnt = undefined, gpc = undefined, storageThrows = false } = {}) {
  const doc = new Node("#document");
  doc.head = doc.appendChild(new Node("head"));
  doc.body = doc.appendChild(new Node("body"));
  doc.createElement = (t) => new Node(t);
  doc.createTextNode = (text) => ({ textContent: text, text });
  if (tag) {
    const script = doc.head.appendChild(new Node("script"));
    script.setAttribute("data-posthog-key", "phc_TESTKEY0123456789abcdefghij");
    script.setAttribute("data-posthog-host", "https://eu.i.posthog.com");
    script.setAttribute("data-posthog-lib", "/vendor/posthog-js-1.0.0.js");
    script.setAttribute("data-posthog-integrity", "sha384-test");
  }
  const store = new Map(stored ? [["ferb-analytics", stored]] : []);
  const localStorage = {
    getItem: (k) => {
      if (storageThrows) throw new Error("SecurityError");
      return store.has(k) ? store.get(k) : null;
    },
    setItem: (k, v) => {
      if (storageThrows) throw new Error("SecurityError");
      store.set(k, String(v));
    },
  };
  const win = {
    location: { pathname: path },
    navigator: { doNotTrack: dnt, globalPrivacyControl: gpc },
    localStorage,
  };
  return { doc, win, store };
}

// The scripts analytics.js added to load PostHog (the page's own tag excluded).
export const posthogScripts = (doc) => doc.head.children.filter((n) => n.tagName === "SCRIPT" && n.src);

export const banner = (doc) => doc.body.querySelectorAll("div").find((n) => n.getAttribute("class") === "consent") || null;

export const buttons = (node) => node.querySelectorAll("button");

// PostHog as analytics.js sees it once its script has loaded.
export function fakePostHog() {
  const calls = [];
  const ph = {
    __loaded: false,
    optedOut: false,
    init(key, config) {
      calls.push(["init", key, config]);
      ph.__loaded = true;
      config.loaded?.(ph);
      return ph;
    },
    has_opted_out_capturing: () => ph.optedOut,
    opt_in_capturing: () => {
      calls.push(["opt_in"]);
      ph.optedOut = false;
    },
    opt_out_capturing: () => {
      calls.push(["opt_out"]);
      ph.optedOut = true;
    },
    stopSessionRecording: () => calls.push(["stop_recording"]),
    set_config: (config) => calls.push(["set_config", config]),
    capture: (...args) => calls.push(["capture", ...args]),
  };
  return { ph, calls };
}
