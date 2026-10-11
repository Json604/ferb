// Owns isolated-world snapshots, exact element references and page events. Must not choose targets or submit implicitly.
// All source labels/values and visible text are retained; the daemon pages model previews. Secret values remain masked.
// Hidden file inputs remain addressable for direct upload; no native file picker is required. Open shadow roots and
// child frame geometry are inventoried. UI visibility, covering elements and scroll containers are observed facts.
(() => {
  if (window.__ferb) return;
  const SEL = 'a[href],button,input,select,textarea,summary,[role],[contenteditable],h1,h2,h3,h4,h5,h6,[tabindex]';
  const ROLES = new Set(["button", "link", "checkbox", "radio", "tab", "menuitem", "option", "combobox", "textbox",
    "searchbox", "switch", "slider", "spinbutton", "listbox", "menuitemcheckbox", "menuitemradio", "treeitem",
    "row", "listitem", "cell", "gridcell", "columnheader", "rowheader", "article", "heading"]);
  const NATIVE = new Set(["A", "BUTTON", "INPUT", "SELECT", "TEXTAREA", "SUMMARY"]);
  const HEADING = new Set(["H1", "H2", "H3", "H4", "H5", "H6"]);
  const SKIP_TEXT = new Set(["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "SVG", "HEAD", "TITLE"]);
  const SECRET_NAME = /otp|one.?time|2fa|totp|mfa|passcode|verification.?code|security.?code|\bpin\b/i;
  const SECRET_PARAM = /token|key|code|secret|session|auth|sig|password|pwd|otp|ticket|nonce|state/i;
  const sealedFields = new WeakSet(); // Exact caller-selected fields remain masked after a sealed fill.
  // Unclassified split widgets may replace every node while retaining individual secret characters.
  // Without stable bindings, mask captured editable values for this document until navigation.
  // This is capture protection only: it must not prohibit typing into unrelated search/form inputs.
  let sealedSplitDocument = false;
  let els = new Map();                 // local id -> WeakRef(element), for the latest snapshot only
  let containers = new Map();          // container id -> WeakRef(scroller element), for the latest snapshot only
  let snapGen = 0;
  let bindings = new Map();
  const randomId = () => {                         // getRandomValues: also on plain-http pages (randomUUID isn't)
    const b = new Uint8Array(16);
    crypto.getRandomValues(b);
    return Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
  };
  const documentId = randomId();
  let visualEpoch = 0;
  let pointerSeen = null;
  addEventListener("mousemove", e => {
    if (e.isTrusted) pointerSeen = [e.clientX, e.clientY];
  }, true);
  const visualChanged = () => { visualEpoch++; };
  new MutationObserver(records => {
    if (records.some(r => r.type !== "attributes" || !["data-ferb-ax", "data-ferb-node"].includes(r.attributeName))) visualChanged();
  }).observe(document, {subtree: true, childList: true, characterData: true, attributes: true});
  addEventListener("scroll", visualChanged, true);
  addEventListener("resize", visualChanged);
  function visualState() {
    return {documentId, generation: snapGen, epoch: visualEpoch, viewport: [innerWidth, innerHeight],
            scroll: [scrollX, scrollY], scale: devicePixelRatio};
  }
  function bind(el) {
    const token = Array.from(crypto.getRandomValues(new Uint8Array(8)), b => b.toString(16).padStart(2, "0")).join("");
    el.setAttribute("data-ferb-ax", token); bindings.set(token, new WeakRef(el));
    return token;
  }
  function adopt(gen, token) {
    if (gen !== snapGen) return null;
    const find = root => {
      const hit = root.querySelector(`[data-ferb-ax="${token}"]`);
      if (hit) return hit;
      for (const host of root.querySelectorAll("*")) if (host.shadowRoot) { const found = find(host.shadowRoot); if (found) return found; }
    };
    const el = find(document);
    if (!el || !visible(el)) return null;
    const id = els.size;
    els.set(id, new WeakRef(el)); bindings.set(token, new WeakRef(el));
    return {...describe(el, id), binding: token};
  }
  function point(gen, x, y) {
    if (gen !== snapGen) return {error: "stale"};
    if (x < 0 || y < 0 || x >= innerWidth || y >= innerHeight) return {error: "outside_viewport"};
    let el = document.elementFromPoint(x, y);
    while (el && el.shadowRoot) {
      const hit = el.shadowRoot.elementFromPoint(x, y);
      if (!hit || hit === el) break;
      el = hit;
    }
    if (!el || !visible(el)) return {error: "no_target"};
    if (["IFRAME", "FRAME"].includes(el.tagName)) return {frame: el.getAttribute("data-ferb-ax")};
    const control = el.closest(SEL);
    if (control && admits(control)) el = control;
    for (const [id, ref] of els) if (ref.deref() === el) return {element: describe(el, id)};
    const id = els.size;
    els.set(id, new WeakRef(el));
    return {element: describe(el, id)};
  }
  function frameRect(token) {
    const ref = bindings.get(token), el = ref && ref.deref();
    if (!el || !el.isConnected) return {error: "frame_changed"};
    const r = el.getBoundingClientRect();
    // A scaled frame needs a full coordinate transform; refuse it instead of using a guessed offset.
    if (Math.abs(r.width - el.offsetWidth) > 1 || Math.abs(r.height - el.offsetHeight) > 1)
      return {error: "frame_transform_unsupported"};
    return {x: r.x + el.clientLeft, y: r.y + el.clientTop, occluded: hitTest(el, r)};
  }

  const labelText = s => (s || "").replace(/\s+/g, " ").trim();
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

  function maskedEditable(el) {
    return sealedSplitDocument && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable);
  }

  function captureText(el) {
    if (!el) return "";
    if (!sealedSplitDocument) return el.innerText || el.textContent || "";
    if (maskedEditable(el)) return "";
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT, {
      acceptNode: n => {
        const p = n.parentElement;
        return !p || maskedEditable(p) || SKIP_TEXT.has(p.tagName) ||
          p.checkVisibility && !p.checkVisibility({visibilityProperty:true}) ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT;
      }
    });
    const text = []; let n;
    while ((n = walker.nextNode())) text.push(n.data);
    return text.join(" ");
  }

  function label(el) {
    let s = el.getAttribute("aria-label");
    if (!s && el.getAttribute("aria-labelledby"))
      s = el.getAttribute("aria-labelledby").split(/\s+/).map((id) => {
        const n = (el.getRootNode().getElementById ? el.getRootNode() : document).getElementById(id);
        return n ? (sealedSplitDocument ? captureText(n) : n.textContent) : ""; }).join(" ");
    if (!s && el.labels && el.labels.length) s = Array.from(el.labels).map((l) => sealedSplitDocument ? captureText(l) : l.textContent).join(" ");
    if (!s) s = el.getAttribute("placeholder") || el.getAttribute("title");
    if (!s && el.tagName === "INPUT" && ["button", "submit", "reset"].includes(el.type)) s = el.value;
    if (!s) { const img = el.querySelector && el.querySelector("img[alt]"); if (img) s = img.alt; }
    if (!s) s = captureText(el);
    return labelText(String(s || ""));
  }

  function codeGroup(el) {
    // Split-code controls often have no names/autocomplete (the owner's Zomato login). Bind a contiguous group
    // by the nearest containing element whose visible text inputs are all single-character numeric controls.
    const digit = n => n.tagName === "INPUT" && n.maxLength === 1 && visible(n) && !n.disabled && !n.readOnly &&
      (["tel", "number"].includes(n.type) || ["numeric", "decimal"].includes(n.inputMode) ||
       /[0-9]|\\d/.test(n.pattern || ""));
    if (!digit(el)) return [];
    for (let group = el.parentElement; group; group = group.parentElement) {
      const inputs = Array.from(group.querySelectorAll("input")).filter(n => visible(n) && n.type !== "hidden");
      if (inputs.length > 1) return inputs.every(digit) ? inputs : [];
      if (group === el.form || group.tagName === "BODY") break;
    }
    return [];
  }
  const isSecret = (el) => sealedFields.has(el) || el.tagName === "INPUT" && ((el.type || "").toLowerCase() === "password"
    || /one-time-code/.test(el.autocomplete || "") || SECRET_NAME.test((el.name || "") + " " + (el.id || "") + " " + label(el))
    || codeGroup(el).length > 1);


  function formKind(el) {
    const f = el.form || (el.closest && el.closest("form"));
    return f ? (f.getAttribute("role") || "form") : null;
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
  function collect(root, out) {
    for (const el of root.querySelectorAll("*")) {
      if (el.matches(SEL) && admits(el)) out.push(el);
      if (el.shadowRoot) collect(el.shadowRoot, out);
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
    return !(hit === el || el.contains(hit));
  }

  function describe(el, id) {
    const r = el.getBoundingClientRect(), secret = isSecret(el);
    const role = el.getAttribute("role") || implicitRole(el);
    let value = null;
    if (el.tagName === "SELECT") value = Array.from(el.selectedOptions).map(o=>o.text).join("\n");
    else if (el.tagName === "INPUT" && (el.type === "checkbox" || el.type === "radio")) value = el.checked ? "checked" : "";
    else if (el.tagName === "INPUT" && el.type === "file") value = el.files && el.files.length ? `${el.files.length} file(s)` : "";
    else if (el.tagName === "INPUT" || el.tagName === "TEXTAREA")
      value = secret || maskedEditable(el) ? (el.value ? "filled" : "") : el.value;   // preserve editor newlines and whitespace
    else if (el.isContentEditable) value = maskedEditable(el) ? (el.innerText ? "filled" : "") : el.innerText;
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
                href: el.tagName === "A" ? maskUrl(el.href) : null, form: formKind(el), secret,
                capture_masked: sealedSplitDocument };
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
        if (!p || maskedEditable(p) || SKIP_TEXT.has(p.tagName) || !/\S/.test(n.data)) return NodeFilter.FILTER_REJECT;
        return NodeFilter.FILTER_ACCEPT;
      } });
    const chunks = [];
    const range = document.createRange();
    let n;
    while ((n = walker.nextNode())) {
      const p = n.parentElement;
      if (p.checkVisibility && !p.checkVisibility({ visibilityProperty: true })) continue;
      range.selectNodeContents(n);
      const r = range.getBoundingClientRect();
      if (r.width === 0 && r.height === 0) continue;
      const t = n.data;
      if (!t) continue;
      chunks.push({t,top:r.top});
    }
    let out = "", lastTop = null;
    for (const c of chunks) {
      if (out) out += lastTop !== null && Math.abs(c.top - lastTop) > 2 ? "\n" : " ";
      out += c.t; lastTop = c.top;
    }
    return out;
  }

  function captureState() {
    // A separate field inventory: recording must not replace snapshot/point bindings while the worker acts.
    let secret = sealedSplitDocument, frames = 0;
    const walk = root => {
      for (const el of root.querySelectorAll('*')) {
        if (isSecret(el) && visible(el)) secret = true;
        if (el.tagName === 'IFRAME' || el.tagName === 'FRAME') frames++;
        if (el.shadowRoot) walk(el.shadowRoot);
      }
    };
    walk(document);
    return {documentId, secret, frames, complete: true};
  }

  function snapshot(gen) {
    for (const [token, ref] of bindings) {
      const el = ref.deref();
      if (el && el.getAttribute("data-ferb-ax") === token) el.removeAttribute("data-ferb-ax");
    }
    bindings = new Map();
    snapGen = gen;
    els = new Map(); containers = new Map(); scrollerCache.clear();
    const found = [];
    collect(document, found);
    const shown = found.filter(el => visible(el) || (el.tagName === "INPUT" && el.type === "file"));
    const out = shown.map((el,id) => {
      const e=describe(el,id);e.binding=bind(el);els.set(id,new WeakRef(el));return e;
    });
    const usedContainers = new Set(out.map((e) => e.container).filter(Boolean));
    const scrollers = [];
    for (const [cid, ref] of containers) {
      const sc = ref.deref();
      if (!sc || !usedContainers.has(cid)) { containers.delete(cid); continue; }
      const r = sc.getBoundingClientRect();
      scrollers.push({ id: cid, label: labelText(sc.getAttribute("aria-label") || sc.getAttribute("role") || sc.id || sc.tagName.toLowerCase()),
                       rect: [round(r.x), round(r.y), round(r.width), round(r.height)], ...scrollState(sc) });
    }
    const iframes = [];
    for (const f of document.querySelectorAll("iframe,frame")) {
      const r = f.getBoundingClientRect();
      if (r.width <= 0 || r.height <= 0) continue;
      const meets = r.right > 0 && r.bottom > 0 && r.left < innerWidth && r.top < innerHeight;
      iframes.push({ binding: bind(f), name: f.name || "", src: maskUrl(f.src || ""), rect: [round(r.x), round(r.y), round(r.width), round(r.height)],
                     in_view: meets && !hitTest(f, r) });
    }
    const text = pageText();
    return { gen, document_binding: bind(document.documentElement), url: maskUrl(location.href), title: document.title, name: window.name || "", top: window === top,
             viewport: [innerWidth, innerHeight], scroll: scrollState(null), scrollers, iframes, elements: out, text,
             truncated: false };
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
    const parts = String(spec || "").split("+"), key = parts[parts.length - 1];
    const mods = new Set(parts.slice(0, -1).map((m) => m.toLowerCase()));
    const [keyCode, code] = K[key] || (key.length === 1 ? [key.toUpperCase().charCodeAt(0),
      /[a-z]/i.test(key) ? "Key" + key.toUpperCase() : /[0-9]/.test(key) ? "Digit" + key : ""] : [0, ""]);
    return { key: key === "Space" ? " " : key, code, keyCode, which: keyCode, bubbles: true, cancelable: true,
             composed: true, shiftKey: mods.has("shift"), ctrlKey: mods.has("ctrl") || mods.has("control"),
             altKey: mods.has("alt") || mods.has("option"), metaKey: mods.has("cmd") || mods.has("meta") };
  }

  // Page-mode actions (untrusted events). Trusted input goes through the debugger (background.js).
  function act(gen, id, kind, text) {
    if (["type","select","press"].includes(kind) && typeof text !== "string")
      return {ok:false,reason:"explicit_input_required"};
    const { el, error } = element(gen, id);
    if (error) return { ok: false, detail: error };
    if (kind === "type") {
      if (isSecret(el)) return { ok: false, detail: "secret_field" };
      el.focus();
      if (el.isContentEditable) {
        const sel = getSelection(), range = document.createRange();
        range.selectNodeContents(el); sel.removeAllRanges(); sel.addRange(range);
        if (!document.execCommand("insertText", false, text || ""))
          return {ok:false,reason:"editor_insert_refused",detail:"The editor refused insertText; choose another method after inspecting the field"};
      } else setValue(el, text || "");
      return { ok: true, detail: "typed (page events)" };
    }
    if (kind === "select") {
      if (!(el instanceof HTMLSelectElement)) return { ok: false, detail: "not_a_select" };
      const matches = Array.from(el.options).filter(o => o.value === text || o.text.trim() === text);
      if (matches.length !== 1) return {ok:false,reason:matches.length ? "ambiguous_option" : "no_such_option",
        detail:"Choose an unambiguous option from its evidence",candidates:matches.map(o=>({value:o.value,label:o.text}))};
      el.selectedIndex = Array.from(el.options).indexOf(matches[0]);
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
      const init = keyInit(text);
      for (const type of ["keydown", "keypress", "keyup"]) el.dispatchEvent(new KeyboardEvent(type, init));
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
    return { x: r.x + r.width / 2, y: r.y + r.height / 2, w: r.width, h: r.height, occluded, secret: isSecret(el),
             disabled: !!el.disabled || el.getAttribute("aria-disabled") === "true",
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
  async function uploadFiles(gen, id) {
    const {el,error}=element(gen,id);
    if (error) throw Error(error);
    const out=[];
    for (const file of el.files || []) {
      const digest=await crypto.subtle.digest("SHA-256",await file.arrayBuffer());
      out.push({name:file.name,size:file.size,type:file.type,modified_ms:file.lastModified,
        sha256:Array.from(new Uint8Array(digest),b=>b.toString(16).padStart(2,"0")).join("")});
    }
    return out;
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
  function scroller(cid) {           // where a trusted wheel event must land (px, py) to scroll that container (or the window)
    const ref = cid && containers.get(cid), sc = ref ? ref.deref() : null;
    if (cid && (!sc || !sc.isConnected)) return {error:"stale"};
    const r = sc ? sc.getBoundingClientRect() : {x:0, y:0, width:innerWidth, height:innerHeight};
    const left = Math.max(1, r.x), top = Math.max(1, r.y);
    const right = Math.min(innerWidth - 1, r.x + r.width), bottom = Math.min(innerHeight - 1, r.y + r.height);
    if (right <= left || bottom <= top) return {error:"scroll target is outside the viewport"};
    scrollerCache.clear();
    for (const fx of [0.5, 0.1, 0.9]) for (const fy of [0.5, 0.1, 0.9]) {
      const px = left + (right-left)*fx, py = top + (bottom-top)*fy;
      let hit = document.elementFromPoint(px, py);
      while (hit && hit.shadowRoot) {
        const inner = hit.shadowRoot.elementFromPoint(px, py);
        if (!inner || inner === hit) break;
        hit = inner;
      }
      if (!hit || hit.tagName === 'IFRAME') continue;
      const actual = hit === document.body || hit === document.documentElement ? null :
        (isScroller(hit) ? hit : scrollerOf(hit));
      if (actual === sc) return {px, py, ...scrollState(sc)};
    }
    return {error:"no uncovered wheel target for this scroll surface; take a fresh snapshot"};
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

  // Downloads and new tabs (D35), while FERB drives this tab. A blob: or data: link with a download attribute (no
  // network request, so the tab's debugger can't hold it) is taken here, in the capture phase so a page's
  // stopPropagation can't hide it: its default action is cancelled before the browser starts a download (no Save
  // dialog) and the bytes wait in this frame for the service worker. The page hook (protocol.js) adds links the page never
  // put in the document for their click. A trusted click on a link to a new tab (target=_blank, also through
  // <base target>, or a modifier or middle click) is opened by FERB itself instead: blank and armed before it loads,
  // inactive, the same task's tab; decided in the bubble phase, after the page's own handlers, so a click the page
  // cancelled is left alone, and untrusted (script) clicks keep the browser's own flow and popup blocker. If the service
  // worker says this tab isn't armed (a stale listener), the click goes back to the browser at once; if the daemon
  // doesn't answer within about a second (down or restarting), it goes back to the browser then: never lost.
  let downloadsArmed = false;
  const heldDownloads = new Map();                 // id -> {blob, name, href}
  const HELD_MAX = 20;                             // caught links held or being read; a flood past it is dropped
  let saving = 0;                                  // caught links whose bytes are still being read
  let droppedAt = -Infinity;                       // when this document last told the worker it dropped a save
  try { chrome.runtime.sendMessage({ ferbArmed: true }, (on) => { downloadsArmed = on === true; }); } catch (_) { /* no worker */ }
  const linkOf = (e) => (e.composedPath ? e.composedPath() : [])
    .find((n) => n instanceof HTMLAnchorElement || (typeof HTMLAreaElement !== "undefined" && n instanceof HTMLAreaElement));
  function opensNewTab(a, e) {
    if (e.type === "auxclick") return e.button === 1;
    if (e.metaKey || e.ctrlKey || e.shiftKey) return true;
    let target = a.getAttribute("target");
    if (target === null) {
      const base = document.querySelector ? document.querySelector("base[target]") : null;
      target = base ? base.getAttribute("target") : "";
    }
    return String(target || "").trim().toLowerCase() === "_blank";   // other names: the browser's (frames, windows)
  }
  // After an extension reload (`ferb setup --update`) this document's old script can't reach the service worker any
  // more: chrome.runtime.id is gone and sending throws. Then it stops taking clicks, and a click it already took goes
  // back to the browser, so the owner's click is never lost.
  function live() {
    try { if (chrome.runtime && chrome.runtime.id) return true; } catch (_) { /* invalidated */ }
    downloadsArmed = false;
    return false;
  }
  function passOn(href, target, name) {            // a detached link the browser follows itself; no listener sees it
    const a = document.createElement("a");
    a.href = href;
    if (target) a.target = target;
    if (name !== undefined) a.download = name;
    a.click();
  }
  function onSave(e) {                             // capture phase: blob/data download links
    if (!downloadsArmed) return;
    const a = linkOf(e);
    const href = a ? String(a.href || "") : "";
    if (!a || !a.hasAttribute("download") || !/^(blob|data):/i.test(href) || !live()) return;
    e.preventDefault();
    if (heldDownloads.size + saving >= HELD_MAX) {   // a page's flood of saves: dropped, never the browser's; the
      const now = Date.now();                        // worker hears of it (at most once a minute from this document)
      if (now - droppedAt >= 60000) {
        droppedAt = now;
        try { chrome.runtime.sendMessage({ ferbDropped: { name: a.getAttribute("download") || "" } }); } catch (_) { /* no worker */ }
      }
      return;
    }
    const id = randomId(), name = a.getAttribute("download") || "";
    saving++;
    fetch(href).then((r) => r.blob()).then((blob) => {   // started now: a page may revoke the blob right after the click
      saving--;
      heldDownloads.set(id, { blob, name, href });
      try {
        chrome.runtime.sendMessage({ ferbDownload: { id, name, mime: blob.type || null, size: blob.size,
          source_url: /^data:/i.test(href) ? "data:" + (href.slice(5).split(/[;,]/)[0] || "") : href.slice(0, 4096) } },
          (kept) => { if (kept === "drop") heldDownloads.delete(id); else if (kept !== true) releaseDownload(id); });
      } catch (_) {                                  // orphaned by a reload: the browser saves it as it would have
        downloadsArmed = false;
        releaseDownload(id);
      }
    }).catch(() => { saving--; /* nothing to take; the browser never started a download either */ });
  }
  function onNewTab(e) {                           // bubble phase: trusted clicks on links to a new tab
    if (!downloadsArmed || e.defaultPrevented || e.isTrusted !== true) return;
    const a = linkOf(e);
    const href = a ? String(a.href || "") : "";
    if (!a || !href || a.hasAttribute("download") || !/^https?:/i.test(href) || !opensNewTab(a, e) || !live()) return;
    e.preventDefault();
    const target = a.getAttribute("target") || "_blank";
    // the owner's intent, if it isn't FERB's: cmd/ctrl or middle click opens behind, with shift in front (as the browser)
    const background = (e.type === "auxclick" || e.metaKey === true || e.ctrlKey === true) && e.shiftKey !== true;
    try {
      chrome.runtime.sendMessage({ ferbOpen: { url: href, background } }, (opened) => { if (opened !== true) passOn(href, target); });
    } catch (_) {                                    // orphaned by a reload: the browser opens it after all
      downloadsArmed = false;
      passOn(href, target);
    }
  }
  addEventListener("click", onSave, true);
  addEventListener("click", onNewTab, false);
  addEventListener("auxclick", onNewTab, false);
  // The page hook asks whether FERB drives this tab (a cancelable event: the answer is preventDefault), and relays a
  // window.open it opened blank for a URL.
  if (typeof document.addEventListener === "function") {
    document.addEventListener("ferb-armed", (e) => { if (downloadsArmed && live()) e.preventDefault(); });
    document.addEventListener("ferb-popup", (e) => {     // page content: cut to shape before it leaves the page
      if (!downloadsArmed || typeof e.detail !== "string" || e.detail.length > 16384) return;
      let p = null;
      try { p = JSON.parse(e.detail); } catch (_) { return; }
      if (p && typeof p.token === "string" && /^[0-9a-f]{32}$/.test(p.token) && typeof p.url === "string") {
        try { chrome.runtime.sendMessage({ ferbPopup: { token: p.token, url: p.url.slice(0, 8192) } }); }
        catch (_) { downloadsArmed = false; }       // orphaned: the hook loads its popup itself after 1.5 s
      }
    });
  }
  // The service worker claimed the hook's popup and the daemon made it the task's: the hook loads its URL now. The
  // answer tells the worker the page was reached (the lab checks the ready path was taken, not the 1.5 s fallback).
  try {
    chrome.runtime.onMessage.addListener((m, sender, reply) => {
      if (m && typeof m.ferbPopupReady === "string") {
        document.dispatchEvent(new CustomEvent("ferb-popup-ready", { detail: m.ferbPopupReady }));
        if (typeof reply === "function") reply(true);
      }
    });
  } catch (_) { /* no worker */ }
  async function downloadPart(id, offset, n) {
    const held = heldDownloads.get(id);
    if (!held) return { error: "gone" };
    const bytes = new Uint8Array(await held.blob.slice(offset, offset + n).arrayBuffer());
    const eof = offset + bytes.length >= held.blob.size;
    if (eof) heldDownloads.delete(id);
    let text = "";
    for (let i = 0; i < bytes.length; i += 32768) text += String.fromCharCode(...bytes.subarray(i, i + 32768));
    return { data: btoa(text), eof };
  }
  function releaseDownload(id) {                   // not FERB's after all: the browser downloads it as it would have
    const held = heldDownloads.get(id);
    if (!held) return false;
    heldDownloads.delete(id);
    const url = /^data:/i.test(held.href) ? held.href : URL.createObjectURL(held.blob);
    passOn(url, "", held.name);
    if (url !== held.href) setTimeout(() => URL.revokeObjectURL(url), 60000);
    return true;
  }
  function armDownloads(on, drop = false) {
    downloadsArmed = on === true;
    if (!downloadsArmed) for (const id of [...heldDownloads.keys()]) { if (drop) heldDownloads.delete(id); else releaseDownload(id); }
  }

  // Sealed fill: the value arrives already opened by the service worker, for this element only. Re-check this
  // frame's origin in the same call that sets it; set it on that exact element (never typed into whatever has focus).
  function fill(gen, id, expectedOrigin, value, codeOnly = false, splitIds = null) {
    if (location.origin !== expectedOrigin) return { ok: false, detail: "origin_changed" };
    const { el, error } = element(gen, id);
    if (error) return { ok: false, detail: error };
    if (!(el instanceof HTMLInputElement) && !(el instanceof HTMLTextAreaElement)) return { ok: false, detail: "not_a_text_field" };
    if (codeOnly && (!isSecret(el) || el.type === "password")) return {ok: false, detail: "code_field_changed"};
    let group = codeGroup(el);
    if (splitIds !== null) {
      if (!Array.isArray(splitIds) || splitIds.length < 2 || splitIds[0] !== id ||
          new Set(splitIds).size !== splitIds.length) return {ok:false, detail:"invalid_split_targets"};
      group = splitIds.map(i => element(gen, i).el);
      if (!group.every(n => n instanceof HTMLInputElement && ["text", "tel", "number"].includes(n.type) &&
          visible(n) && !n.disabled && !n.readOnly)) return {ok:false, detail:"split_target_changed"};
    }
    if (group.length) {
      const chars = Array.from(value);
      if ((splitIds === null && !/^\d+$/.test(value)) || chars.length !== group.length)
        return {ok: false, detail: "code_length_mismatch"};
      // Revalidate all references before any mutation. Do not choose another group after a rerender.
      if (!group.every(n => n.isConnected && n.ownerDocument === el.ownerDocument)) return {ok:false, detail:"code_group_changed"};
      if (splitIds !== null) sealedSplitDocument = true;
      group.forEach(n => sealedFields.add(n));
      for (let i = 0; i < group.length; i++) {
        if (!group[i].isConnected) return {ok:false, unknown:true, detail:"code_group_changed_after_partial_fill"};
        setValue(group[i], chars[i]);
      }
      const ok = group.every((n, i) => n.value === chars[i]);
      return {ok, unknown: !ok, detail: ok ? "filled_split_code" : "code_state_changed_after_input"};
    }
    sealedFields.add(el);
    setValue(el, value);
    const ok = el.isConnected && el.value === value;
    return { ok, unknown: !ok, detail: ok ? "filled" : "field_state_changed_after_input" };
  }

  window.__ferb = { snapshot, uploadFiles, adopt, point, frameRect, visualState, captureState,
    supportsSplitSecretFill: true,
    pointerArm: () => { pointerSeen = null; }, pointerSeen: () => pointerSeen || [],
    viewport: () => [innerWidth, innerHeight], act, fill, origin: () => location.origin, rect, focus, isActive, stamp, unstamp, scroller,
                    scrolled, probeArm, probeSeen: () => probeSeen,
                    text: () => captureText(document.body), url: () => location.href,
                    ready: () => document.readyState, downloadPart, armDownloads, releaseDownload,
                    dropDownload: (id) => { heldDownloads.delete(id); } };
})();
