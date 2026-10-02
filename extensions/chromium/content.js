// FERB content script: runs in the extension's isolated world (the page can't see or change it). Snapshots,
// element lookup and page-mode actions. No decisions. Sanitizes before anything leaves the page (9.4):
// password/OTP values are reported only as filled/empty, hidden inputs are skipped, URL tokens are masked.
//
// Snapshot semantics (PLATFORM_BROWSER.md 6, 13.3): every element carries `in_view` (its rect meets this frame's
// viewport and elementFromPoint at its centre hits it or a descendant), `occluded` (on screen but covered),
// `container` (the id of its nearest scrollable ancestor, whose scroll position is in `scrollers`), and state
// (disabled / expanded / selected / checked). Open shadow roots are walked; iframes are listed with their rects so
// the service worker can turn child-frame rects into top-viewport ones. The 400-element cap keeps in-view elements
// first. `text` is the visible text nearest the viewport, in document order, not the top of the page.
(() => {
  if (window.__ferb) return;
  const MAX_ELEMENTS = 400, MAX_TEXT = 8000, MAX_LABEL = 120, MAX_VALUE = 200, MAX_TEXT_NODES = 40000;
  const SEL = 'a[href],button,input,select,textarea,summary,[role],[contenteditable],h1,h2,h3,h4,h5,h6,[tabindex]';
  const ROLES = new Set(["button", "link", "checkbox", "radio", "tab", "menuitem", "option", "combobox", "textbox",
    "searchbox", "switch", "slider", "spinbutton", "listbox", "menuitemcheckbox", "menuitemradio", "treeitem",
    "row", "listitem", "cell", "gridcell", "columnheader", "rowheader", "article", "heading"]);
  const NATIVE = new Set(["A", "BUTTON", "INPUT", "SELECT", "TEXTAREA", "SUMMARY"]);
  const HEADING = new Set(["H1", "H2", "H3", "H4", "H5", "H6"]);
  const SKIP_TEXT = new Set(["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "SVG", "HEAD", "TITLE"]);
  const SECRET_NAME = /otp|one.?time|2fa|totp|mfa|passcode|verification.?code|security.?code|\bpin\b/i;
  const SECRET_PARAM = /token|key|code|secret|session|auth|sig|password|pwd|otp|ticket|nonce|state/i;
  let els = new Map();                 // local id -> WeakRef(element), for the latest snapshot only
  let containers = new Map();          // container id -> WeakRef(scroller element), for the latest snapshot only
  let snapGen = 0;

  const clip = (s, n) => (s || "").replace(/\s+/g, " ").trim().slice(0, n);
  const round = (v) => Math.round(v * 10) / 10;

  function maskUrl(u) {
    try {
      const url = new URL(u, location.href);
      for (const k of [...url.searchParams.keys()]) if (SECRET_PARAM.test(k)) url.searchParams.set(k, "[redacted]");
      if (url.hash.length > 1 && /[=&]/.test(url.hash)) url.hash = "#[redacted]";
      if (url.username || url.password) { url.username = ""; url.password = ""; }
      return url.href;
    } catch (_) { return ""; }
  }

  function implicitRole(el) {
    const t = el.tagName;
    if (t === "A") return "link";
    if (t === "BUTTON" || t === "SUMMARY") return "button";
    if (t === "SELECT") return "combobox";
    if (t === "TEXTAREA" || el.isContentEditable) return "textbox";
    if (HEADING.has(t)) return "heading";
    if (t === "INPUT") {
      const ty = (el.type || "text").toLowerCase();
      if (["button", "submit", "reset", "image"].includes(ty)) return "button";
      if (ty === "checkbox" || ty === "radio") return ty;
      if (ty === "search") return "searchbox";
      if (ty === "file") return "button";
      return "textbox";
    }
    return "generic";
  }

  function label(el) {
    let s = el.getAttribute("aria-label");
    if (!s && el.getAttribute("aria-labelledby"))
      s = el.getAttribute("aria-labelledby").split(/\s+/).map((id) => {
        const n = (el.getRootNode().getElementById ? el.getRootNode() : document).getElementById(id);
        return n ? n.textContent : ""; }).join(" ");
    if (!s && el.labels && el.labels.length) s = Array.from(el.labels).map((l) => l.textContent).join(" ");
    if (!s) s = el.getAttribute("placeholder") || el.getAttribute("title");
    if (!s && el.tagName === "INPUT" && ["button", "submit", "reset"].includes(el.type)) s = el.value;
    if (!s) { const img = el.querySelector && el.querySelector("img[alt]"); if (img) s = img.alt; }
    if (!s) s = el.innerText;
    return clip(String(s || ""), MAX_LABEL);
  }

  const isSecret = (el) => el.tagName === "INPUT" && ((el.type || "").toLowerCase() === "password"
    || /one-time-code/.test(el.autocomplete || "") || SECRET_NAME.test((el.name || "") + " " + (el.id || "")));

  function formKind(el) {
    const f = el.form || (el.closest && el.closest("form"));
    if (!f) return (el.type === "search" || el.getAttribute("role") === "searchbox") ? "search" : null;
    if (f.getAttribute("role") === "search" || f.querySelector("input[type=search]")) return "search";
    if (f.querySelector("input[type=password]")) return "login";
    const fields = f.querySelectorAll("input:not([type=hidden]):not([type=submit]):not([type=button]),textarea,select");
    if (fields.length === 1 && /(^|[^a-z])(q|query|search|keywords?)([^a-z]|$)/i.test(
      (fields[0].name || "") + " " + (fields[0].id || ""))) return "search";
    return "other";
  }

  function visible(el) {
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) return false;
    if (el.checkVisibility && !el.checkVisibility({ visibilityProperty: true })) return false;
    return !el.closest('[aria-hidden="true"],[inert]');
  }

  function admits(el) {
    const t = el.tagName;
    if (t === "INPUT" && el.type === "hidden") return false;
    if (NATIVE.has(t) || HEADING.has(t) || el.isContentEditable) return true;
    const role = el.getAttribute("role");
    if (role && ROLES.has(role)) return true;
    const ti = el.getAttribute("tabindex");
    return ti !== null && Number(ti) >= 0;
  }

  // Every candidate in document order, descending into open shadow roots right after their host.
  function collect(root, out, budget) {
    for (const el of root.querySelectorAll("*")) {
      if (out.length >= budget) return;
      if (el.matches(SEL) && admits(el)) out.push(el);
      if (el.shadowRoot) collect(el.shadowRoot, out, budget);
    }
  }

  const parentOf = (el) => el.parentElement || (el.getRootNode() instanceof ShadowRoot ? el.getRootNode().host : null);
  const scrollerCache = new Map();
  function isScroller(p) {
    let v = scrollerCache.get(p);
    if (v === undefined) {
      const o = getComputedStyle(p).overflowY;
      v = (o === "auto" || o === "scroll" || o === "overlay") && p.scrollHeight > p.clientHeight + 1;
      scrollerCache.set(p, v);
    }
    return v;
  }
  function scrollerOf(el) {
    for (let p = parentOf(el); p && p !== document.documentElement && p !== document.body; p = parentOf(p))
      if (isScroller(p)) return p;
    return null;
  }
  function scrollState(sc) {       // sc: a container element or null for the window
    if (!sc) {
      const d = document.scrollingElement || document.documentElement;
      return { y: Math.round(d.scrollTop), height: d.scrollHeight, viewport_h: innerHeight,
               at_end: d.scrollTop + innerHeight >= d.scrollHeight - 2 };
    }
    return { y: Math.round(sc.scrollTop), height: sc.scrollHeight, viewport_h: sc.clientHeight,
             at_end: sc.scrollTop + sc.clientHeight >= sc.scrollHeight - 2 };
  }

  function hitTest(el, r) {
    const cx = Math.min(Math.max(r.x + r.width / 2, 0), innerWidth - 1);
    const cy = Math.min(Math.max(r.y + r.height / 2, 0), innerHeight - 1);
    const root = el.getRootNode();
    const hit = (root instanceof ShadowRoot ? root : document).elementFromPoint(cx, cy);
    if (!hit) return true;                                        // nothing painted there: treat as covered
    return !(hit === el || el.contains(hit) || hit.contains(el));
  }

  function describe(el, id) {
    const r = el.getBoundingClientRect(), secret = isSecret(el);
    const role = el.getAttribute("role") || implicitRole(el);
    let value = null;
    if (el.tagName === "SELECT") value = el.selectedOptions.length ? clip(el.selectedOptions[0].text, MAX_VALUE) : "";
    else if (el.tagName === "INPUT" && (el.type === "checkbox" || el.type === "radio")) value = el.checked ? "checked" : "";
    else if (el.tagName === "INPUT" && el.type === "file") value = el.files && el.files.length ? `${el.files.length} file(s)` : "";
    else if (el.tagName === "INPUT" || el.tagName === "TEXTAREA")
      value = secret ? (el.value ? "filled" : "") : clip(el.value, MAX_VALUE);   // never a secret value
    else if (el.isContentEditable) value = clip(el.innerText, MAX_VALUE);
    const meets = r.right > 0 && r.bottom > 0 && r.left < innerWidth && r.top < innerHeight;
    const occluded = meets ? hitTest(el, r) : false;
    const sc = scrollerOf(el);
    let container = null;
    if (sc) {
      container = sc.__ferbCid;
      if (!container || !containers.has(container) || containers.get(container).deref() !== sc) {
        container = `c${containers.size + 1}`;
        sc.__ferbCid = container;
        containers.set(container, new WeakRef(sc));
      }
    }
    const e = { id, role, label: label(el), value, rect: [round(r.x), round(r.y), round(r.width), round(r.height)],
                in_view: meets && !occluded, input_type: el.tagName === "INPUT" ? (el.type || "text") : null,
                href: el.tagName === "A" ? maskUrl(el.href) : null, form: formKind(el), secret };
    if (!meets) e.offscreen = r.bottom <= 0 ? "above" : r.top >= innerHeight ? "below" : r.right <= 0 ? "left" : "right";
    if (occluded) e.occluded = true;
    if (container) e.container = container;
    if (el.disabled || el.getAttribute("aria-disabled") === "true") e.disabled = true;
    const exp = el.getAttribute("aria-expanded"), sel = el.getAttribute("aria-selected"), chk = el.getAttribute("aria-checked");
    if (exp !== null) e.expanded = exp === "true";
    if (sel !== null) e.selected = sel === "true";
    if (chk !== null && value === null) e.value = chk === "true" ? "checked" : "";
    if (el.tagName === "DETAILS" || (el.tagName === "SUMMARY" && el.parentElement && el.parentElement.tagName === "DETAILS"))
      e.expanded = !!(el.parentElement && el.parentElement.open);
    if (role === "heading") e.level = HEADING.has(el.tagName) ? Number(el.tagName[1]) : Number(el.getAttribute("aria-level") || 2);
    return e;
  }

  // Visible text nearest the viewport, in document order. Each text node is a chunk with its rect; in-view chunks
  // come first, then the nearest above/below until the budget; "[…]" marks what was left out between chunks.
  function pageText() {
    if (!document.body) return "";
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, {
      acceptNode: (n) => {
        const p = n.parentElement;
        if (!p || SKIP_TEXT.has(p.tagName) || !/\S/.test(n.data)) return NodeFilter.FILTER_REJECT;
        return NodeFilter.FILTER_ACCEPT;
      } });
    const chunks = [];
    const range = document.createRange();
    let n, i = 0;
    while ((n = walker.nextNode()) && i < MAX_TEXT_NODES) {
      i++;
      const p = n.parentElement;
      if (p.checkVisibility && !p.checkVisibility({ visibilityProperty: true })) continue;
      range.selectNodeContents(n);
      const r = range.getBoundingClientRect();
      if (r.width === 0 && r.height === 0) continue;
      const t = clip(n.data, 400);
      if (!t) continue;
      const dist = r.bottom < 0 ? -r.bottom : r.top > innerHeight ? r.top - innerHeight : 0;
      chunks.push({ i: chunks.length, t, top: r.top, dist });
    }
    const keep = new Set();
    let used = 0;
    for (const c of chunks.slice().sort((a, b) => a.dist - b.dist || a.i - b.i)) {
      if (used + c.t.length + 1 > MAX_TEXT) { if (c.dist === 0) continue; break; }
      keep.add(c.i); used += c.t.length + 1;
    }
    let out = "", lastTop = null, gap = false;
    for (const c of chunks) {
      if (!keep.has(c.i)) { gap = true; continue; }
      if (out) out += gap ? "\n[…]\n" : (lastTop !== null && Math.abs(c.top - lastTop) > 2 ? "\n" : " ");
      out += c.t; lastTop = c.top; gap = false;
    }
    return out;
  }

  function snapshot(gen) {
    snapGen = gen;
    els = new Map(); containers = new Map(); scrollerCache.clear();
    const found = [];
    collect(document, found, 5000);
    const shown = found.filter(visible);
    const described = shown.map((el, idx) => [el, describe(el, 0), idx]);   // idx: document order
    const inView = described.filter(([, e]) => e.in_view), rest = described.filter(([, e]) => !e.in_view);
    const picked = inView.concat(rest).slice(0, MAX_ELEMENTS);              // the cap keeps what's on screen
    picked.sort((a, b) => a[2] - b[2]);
    const out = picked.map(([el, e], id) => { e.id = id; els.set(id, new WeakRef(el)); return e; });
    const usedContainers = new Set(out.map((e) => e.container).filter(Boolean));
    const scrollers = [];
    for (const [cid, ref] of containers) {
      const sc = ref.deref();
      if (!sc || !usedContainers.has(cid)) { containers.delete(cid); continue; }
      const r = sc.getBoundingClientRect();
      scrollers.push({ id: cid, label: clip(sc.getAttribute("aria-label") || sc.getAttribute("role") || sc.id || sc.tagName.toLowerCase(), 60),
                       rect: [round(r.x), round(r.y), round(r.width), round(r.height)], ...scrollState(sc) });
    }
    const iframes = [];
    for (const f of document.querySelectorAll("iframe,frame")) {
      const r = f.getBoundingClientRect();
      if (r.width <= 0 || r.height <= 0) continue;
      const meets = r.right > 0 && r.bottom > 0 && r.left < innerWidth && r.top < innerHeight;
      iframes.push({ name: f.name || "", src: maskUrl(f.src || ""), rect: [round(r.x), round(r.y), round(r.width), round(r.height)],
                     in_view: meets && !hitTest(f, r) });
    }
    const text = pageText();
    return { gen, url: maskUrl(location.href), title: document.title, name: window.name || "", top: window === top,
             viewport: [innerWidth, innerHeight], scroll: scrollState(null), scrollers, iframes, elements: out, text,
             truncated: described.length > MAX_ELEMENTS || text.length >= MAX_TEXT };
  }

  function element(gen, id) {
    if (gen !== snapGen) return { error: "stale" };
    const ref = els.get(id), el = ref && ref.deref();
    if (!el || !el.isConnected) return { error: "stale" };
    return { el };
  }

  function setValue(el, value) {
    const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value").set.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
  }

  function keyInit(spec) {         // page-mode key events: key / code / keyCode / which like a real press
    const K = { Enter: [13, "Enter"], Tab: [9, "Tab"], Escape: [27, "Escape"], Backspace: [8, "Backspace"],
                Delete: [46, "Delete"], Space: [32, "Space"], ArrowDown: [40, "ArrowDown"], ArrowUp: [38, "ArrowUp"],
                ArrowLeft: [37, "ArrowLeft"], ArrowRight: [39, "ArrowRight"], Home: [36, "Home"], End: [35, "End"],
                PageUp: [33, "PageUp"], PageDown: [34, "PageDown"] };
    const parts = String(spec || "Enter").split("+"), key = parts[parts.length - 1];
    const mods = new Set(parts.slice(0, -1).map((m) => m.toLowerCase()));
    const [keyCode, code] = K[key] || (key.length === 1 ? [key.toUpperCase().charCodeAt(0),
      /[a-z]/i.test(key) ? "Key" + key.toUpperCase() : /[0-9]/.test(key) ? "Digit" + key : ""] : [0, ""]);
    return { key: key === "Space" ? " " : key, code, keyCode, which: keyCode, bubbles: true, cancelable: true,
             composed: true, shiftKey: mods.has("shift"), ctrlKey: mods.has("ctrl") || mods.has("control"),
             altKey: mods.has("alt") || mods.has("option"), metaKey: mods.has("cmd") || mods.has("meta") };
  }

  // Page-mode actions (untrusted events). Trusted input goes through the debugger (background.js).
  function act(gen, id, kind, text) {
    const { el, error } = element(gen, id);
    if (error) return { ok: false, detail: error };
    if (kind === "type") {
      if (isSecret(el)) return { ok: false, detail: "secret_field" };
      el.focus();
      if (el.isContentEditable) {
        const sel = getSelection(), range = document.createRange();
        range.selectNodeContents(el); sel.removeAllRanges(); sel.addRange(range);
        if (!document.execCommand("insertText", false, text || "")) {        // editors that refuse execCommand
          el.textContent = text || "";
          el.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertText", data: text || "" }));
        }
      } else setValue(el, text || "");
      return { ok: true, detail: "typed (page events)" };
    }
    if (kind === "select") {
      if (!(el instanceof HTMLSelectElement)) return { ok: false, detail: "not_a_select" };
      const opt = Array.from(el.options).find((o) => o.value === text || o.text.trim() === text);
      if (!opt) return { ok: false, detail: "no_such_option" };
      el.value = opt.value;
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
      return { ok: true, detail: "selected" };
    }
    if (kind === "click" || kind === "dblclick" || kind === "rightclick" || kind === "hover") {
      el.scrollIntoView({ block: "center", inline: "center" });
      const r = el.getBoundingClientRect(), at = { bubbles: true, cancelable: true, composed: true,
                                                   clientX: r.x + r.width / 2, clientY: r.y + r.height / 2 };
      if (kind === "hover") {
        for (const t of ["pointerover", "pointerenter", "mouseover", "mouseenter", "pointermove", "mousemove"])
          el.dispatchEvent(t.startsWith("pointer") ? new PointerEvent(t, at) : new MouseEvent(t, at));
        return { ok: true, detail: "hovered (page events)" };
      }
      if (kind === "rightclick") { el.dispatchEvent(new MouseEvent("contextmenu", { ...at, button: 2 })); return { ok: true, detail: "right-clicked (page events)" }; }
      el.click();
      if (kind === "dblclick") { el.click(); el.dispatchEvent(new MouseEvent("dblclick", { ...at, detail: 2 })); }
      return { ok: true, detail: `${kind === "dblclick" ? "double-clicked" : "clicked"} (page events)` };
    }
    if (kind === "press") {
      el.focus();
      const init = keyInit(text || "Enter");
      for (const type of ["keydown", "keypress", "keyup"]) el.dispatchEvent(new KeyboardEvent(type, init));
      if (init.key === "Enter" && el.form && !init.shiftKey) el.form.requestSubmit ? el.form.requestSubmit() : el.form.submit();
      return { ok: true, detail: `pressed ${init.key} (page events)` };
    }
    if (kind === "scroll") {                       // element-relative: its own box if it scrolls, else its container
      const sc = isScroller(el) ? el : scrollerOf(el);
      const step = Math.round((sc ? sc.clientHeight : innerHeight) * 0.85) * (text === "up" ? -1 : 1);
      if (sc) sc.scrollBy(0, step); else window.scrollBy(0, step);
      return { ok: true, detail: "scrolled", ...scrollState(sc) };
    }
    return { ok: false, detail: "unknown_action" };
  }

  function rect(gen, id) {           // for trusted input: element centre in this frame's viewport, after scrolling it in
    const { el, error } = element(gen, id);
    if (error) return { error };
    el.scrollIntoView({ block: "center", inline: "center" });
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) return { error: "not_visible" };
    const occluded = hitTest(el, r);
    return { x: r.x + r.width / 2, y: r.y + r.height / 2, w: r.width, h: r.height, occluded,
             editable: el.isContentEditable || el.tagName === "TEXTAREA"
               || (el.tagName === "INPUT" && !["button", "submit", "reset", "checkbox", "radio", "file", "image"].includes(el.type)),
             file: el.tagName === "INPUT" && el.type === "file" };
  }

  function deepActive() {
    let a = document.activeElement;
    while (a && a.shadowRoot && a.shadowRoot.activeElement) a = a.shadowRoot.activeElement;
    return a;
  }
  function isActive(gen, id) {       // the element (or something inside it) has focus: typing goes there
    const { el, error } = element(gen, id);
    if (error) return false;
    const a = deepActive();
    return !!a && (a === el || el.contains(a));
  }
  function focus(gen, id) {
    const { el, error } = element(gen, id);
    if (error) return false;
    el.focus();
    return isActive(gen, id);
  }
  function stamp(gen, id, token) {   // mark the element for a DOM.querySelector follow-up (uploads)
    const { el, error } = element(gen, id);
    if (error) return { ok: false, detail: error };
    if (!(el instanceof HTMLInputElement) || el.type !== "file") return { ok: false, detail: "not_a_file_input" };
    el.setAttribute("data-ferb-node", token);
    return { ok: true };
  }
  function unstamp(token) {
    for (const el of document.querySelectorAll(`[data-ferb-node="${token}"]`)) el.removeAttribute("data-ferb-node");
    return true;
  }
  function scroller(cid) {           // where a trusted wheel event must land to scroll that container (or the window)
    if (!cid) return { x: innerWidth / 2, y: innerHeight / 2, ...scrollState(null) };
    const ref = containers.get(cid), sc = ref && ref.deref();
    if (!sc || !sc.isConnected) return { error: "stale" };
    const r = sc.getBoundingClientRect();
    return { x: Math.min(Math.max(r.x + r.width / 2, 1), innerWidth - 1), y: Math.min(Math.max(r.y + r.height / 2, 1), innerHeight - 1),
             ...scrollState(sc) };
  }
  function scrolled(cid) {
    if (!cid) return scrollState(null);
    const ref = containers.get(cid), sc = ref && ref.deref();
    return sc ? scrollState(sc) : { error: "stale" };
  }

  // Readiness probe (spike 4): a trusted, debugger-dispatched mouse move must land before real input starts.
  let probeSeen = false;
  function probeArm() {
    probeSeen = false;
    addEventListener("mousemove", (e) => { if (e.isTrusted) probeSeen = true; }, { once: true, capture: true });
  }

  // Per-site account extractors (9.3): which account is signed in on this page, or null. Gmail and YouTube first.
  const EMAIL = /[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/;
  const ACCOUNT = {
    "mail.google.com": () => {
      const t = document.title.match(/ - ([^\s]+@[^\s]+) - Gmail$/);
      if (t) return t[1].toLowerCase();
      const a = document.querySelector('a[aria-label*="@"][href*="accounts.google.com"]');
      const m = a && a.getAttribute("aria-label").match(EMAIL);
      return m ? m[0].toLowerCase() : null;
    },
    // YouTube shows the address only on its account page ("Signed in as ..."); elsewhere just the avatar.
    "www.youtube.com": () => {
      if (location.pathname.startsWith("/account")) {
        const m = (document.body ? document.body.innerText : "").match(/Signed in as\s+([^\s]+@[^\s]+)/);
        if (m && EMAIL.test(m[1])) return m[1].toLowerCase();
      }
      return document.querySelector("#avatar-btn, button[aria-label='Account menu']") ? "signed_in_unknown" : null;
    },
  };
  function account() {
    const f = ACCOUNT[location.hostname];
    if (!f) return { supported: false };
    try { return { supported: true, account: f() }; } catch (_) { return { supported: true, account: null }; }
  }

  // Sealed fill (10.1): the value arrives already opened by the service worker, for this element only. Re-check this
  // frame's origin in the same call that sets it; set it on that exact element (never typed into whatever has focus).
  function fill(gen, id, expectedOrigin, value) {
    if (location.origin !== expectedOrigin) return { ok: false, detail: "origin_changed" };
    const { el, error } = element(gen, id);
    if (error) return { ok: false, detail: error };
    if (!(el instanceof HTMLInputElement) && !(el instanceof HTMLTextAreaElement)) return { ok: false, detail: "not_a_text_field" };
    setValue(el, value);
    return { ok: true, detail: "filled" };
  }

  window.__ferb = { snapshot, act, fill, origin: () => location.origin, rect, focus, isActive, stamp, unstamp, scroller,
                    scrolled, probeArm, probeSeen: () => probeSeen, account,
                    text: () => (document.body ? document.body.innerText : ""), url: () => location.href,
                    ready: () => document.readyState };
})();
