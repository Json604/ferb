// Owns isolated-world snapshots, exact element references and page events. Must not choose targets or submit implicitly.
// All source labels/values and visible text are retained; the daemon pages model previews. Secret values remain masked.
// Hidden file inputs remain addressable for direct upload; no native file picker is required. Shadow roots (closed
// ones too: content scripts may open them, and Cloudflare's checkbox frame sits in one) and child frame geometry are
// inventoried. UI visibility, covering elements and scroll containers are observed facts. Secret fields (password,
// login code, card: `fieldKind`) never give their value. Structural facts for the gateway (protocol 18): a control's
// form submission (action host, method, the form's secret field kinds), each element's DOM path, the page's
// structured amount and merchant, and the submissions the page's own submit events showed.
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
  const sealedFields = new WeakSet(); // Exact caller-selected fields remain masked after a sealed fill.
  // Unclassified split widgets may replace every node while retaining individual secret characters.
  // Without stable bindings, mask captured editable values for this document until navigation.
  // This is capture protection only: it must not prohibit typing into unrelated search/form inputs.
  let sealedSplitDocument = false;
  let els = new Map();                 // local id -> WeakRef(element), for the latest snapshot only
  let containers = new Map();          // container id -> WeakRef(scroller element), for the latest snapshot only
  let mediaEls = new Map();            // media index -> WeakRef(video/audio element), for the latest snapshot only
  let snapGen = 0;
  let bindings = new Map();
  // The attribute that links elements to the debugger's nodes for one snapshot (protocol.js BIND_ATTR, random per
  // service-worker start), removed right after it (`unbind`); the upload stamp's is STAMP_ATTR.
  let ATTR = null, STAMP = null;
  const randomId = () => {                         // getRandomValues: also on plain-http pages (randomUUID isn't)
    const b = new Uint8Array(16);
    crypto.getRandomValues(b);
    return Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
  };
  const documentId = randomId();
  let pointerSeen = null;
  addEventListener("mousemove", e => {
    if (e.isTrusted) pointerSeen = [e.clientX, e.clientY];
  }, true);
  // What a screenshot point depends on in this frame (HAND-480): no count of every DOM change (a busy page never sat
  // still for it); the element under the point is resolved again before the press (pointer.js checkPoint).
  function visualState() {
    return {documentId, generation: snapGen, viewport: [innerWidth, innerHeight], scroll: [scrollX, scrollY],
            scale: devicePixelRatio};
  }
  function bind(el) {
    const token = Array.from(crypto.getRandomValues(new Uint8Array(8)), b => b.toString(16).padStart(2, "0")).join("");
    if (ATTR) el.setAttribute(ATTR, token);
    bindings.set(token, new WeakRef(el));
    return token;
  }
  function tokenOf(el) {                           // an element's binding in the latest snapshot (the map, not the DOM)
    for (const [token, ref] of bindings) if (ref.deref() === el) return token;
    return null;
  }
  function unbind() {                              // the linking attributes leave the page; the bindings stay
    if (!ATTR) return true;
    for (const [token, ref] of bindings) {
      const el = ref.deref();
      if (el && el.getAttribute(ATTR) === token) el.removeAttribute(ATTR);
    }
    return true;
  }
  // HAND-490: a checkbox or radio the page hides (zero size, visually hidden) behind a visible <label> of it: the label
  // stands for it (a click on a label clicks its control). null for anything else.
  function standIn(el) {
    if (el.tagName !== "INPUT" || !["checkbox", "radio"].includes(el.type) || !el.labels || visible(el)) return null;
    return Array.from(el.labels).find(visible) || null;
  }
  // The element as the snapshot lists it, and what an act on it reaches: itself, or a hidden checkbox's label, described
  // as the box (its role, type and checked state).
  function listed(el, id) {
    const lab = standIn(el);
    if (!lab) return { target: el, e: describe(el, id) };
    return { target: lab, e: { ...describe(lab, id), role: implicitRole(el), input_type: el.type,
                               value: el.checked ? "checked" : "" } };
  }
  function adoptOne(el, token) {
    if (!el || !(visible(el) || standIn(el))) return null;
    const id = els.size, { target, e } = listed(el, id);
    els.set(id, new WeakRef(target)); bindings.set(token, new WeakRef(el));
    return {...e, binding: token};
  }
  function adopt(gen, token, attr) {
    if (gen !== snapGen) return null;
    if (attr) ATTR = attr;
    return adoptOne(findBound(document, token), token);
  }
  // Extension 0.19.6: many adoptions in one script, in the given order (each id as one adopt after another gives it):
  // the stamped elements are found in one pass (light DOM and every shadow root, closed ones too), not one per token.
  function adoptAll(gen, tokens, attr) {
    if (gen !== snapGen || !Array.isArray(tokens)) return null;
    if (attr) ATTR = attr;
    if (!ATTR) return tokens.map(() => null);
    const stamped = new Map();
    const gather = (root) => {
      for (const el of root.querySelectorAll(`[${ATTR}]`)) {
        const t = el.getAttribute(ATTR);
        if (!stamped.has(t)) stamped.set(t, el);
      }
      for (const host of root.querySelectorAll("*")) { const sr = shadowOf(host); if (sr) gather(sr); }
    };
    gather(document);
    return tokens.map((token) => adoptOne(stamped.get(token), token));
  }
  function findBound(root, token) {                 // the element CDP stamped, inside closed shadow roots too
    if (!ATTR) return null;
    const hit = root.querySelector(`[${ATTR}="${token}"]`);
    if (hit) return hit;
    for (const host of root.querySelectorAll("*")) {
      const sr = shadowOf(host);
      if (sr) { const found = findBound(sr, token); if (found) return found; }
    }
    return null;
  }
  function point(gen, x, y) {
    if (gen !== snapGen) return {error: "stale"};
    pinCache.clear(); boxCache.clear(); frameCache.clear();
    if (x < 0 || y < 0 || x >= innerWidth || y >= innerHeight) return {error: "outside_viewport"};
    let el = document.elementFromPoint(x, y);
    while (el) {                                   // into shadow roots, closed ones too (live 2026-10-26: Cloudflare)
      const sr = shadowOf(el), hit = sr && sr.elementFromPoint(x, y);
      if (!hit || hit === el) break;
      el = hit;
    }
    if (!el || !visible(el)) return {error: "no_target"};
    if (["IFRAME", "FRAME"].includes(el.tagName)) return {frame: tokenOf(el)};
    const control = el.closest(SEL);
    if (control && admits(control)) el = control;
    else for (let a = el; a; a = parentOf(a)) if (clickables.has(a)) { el = a; break; }   // HAND-489
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

  // mask.js, injected right before this file: one rule with background.js; without it, no URL at all (fails closed)
  const maskUrl = (u) => (typeof globalThis.ferbMaskUrl === "function" ? globalThis.ferbMaskUrl(u, location.href) : "");

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
    // Split-code controls often have no names/autocomplete (a food-delivery site's login). Bind a contiguous group
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
  // A real password: type=password, or a password autocomplete on an input of any type (a tel/number PIN). A
  // password field is also one a site's "Show password" made type=text (Google keeps autocomplete current-password
  // and name Passwd), named so by its name, id or label. Checkboxes, buttons and email fields are no such field.
  const realPassword = (el) => (el.type || "").toLowerCase() === "password" ||
    /\b(?:current|new)-password\b/.test(el.autocomplete || "");
  const passwordField = (el) => el.tagName === "INPUT" && (realPassword(el) ||
    ["text", ""].includes((el.type || "").toLowerCase()) &&
    /passw|pwd/i.test((el.name || "") + " " + (el.id || "") + " " + label(el)));
  // A field that names itself a code field ("One-time password" holds "passw" too) stays one for a login code.
  const codeField = (el) => /one-time-code/.test(el.autocomplete || "") ||
    SECRET_NAME.test((el.name || "") + " " + (el.id || "") + " " + label(el)) || codeGroup(el).length > 1;
  // A card field, by the page's own autocomplete token (cc-number, cc-csc, cc-exp, cc-exp-month, cc-exp-year, with
  // any section-/billing/shipping prefix): an input, or a select for the expiry (GATE-206).
  const CARD = /(?:^|\s)cc-(number|csc|exp|exp-month|exp-year)(?:\s|$)/;
  const cardKind = (el) => {
    if (el.tagName !== "INPUT" && el.tagName !== "SELECT") return null;
    const m = CARD.exec(String((el.getAttribute && el.getAttribute("autocomplete")) || el.autocomplete || "").toLowerCase());
    return m ? (m[1] === "number" ? "card_number" : m[1] === "csc" ? "card_csc" : "card_expiry") : null;
  };
  // The one secret-field rule (protocol 18): a password (type=password, current/new-password, a shown-password text
  // field), a login code (one-time-code, a code name, a split-code box) or a card field. Its kind is reported
  // (ActionFacts.field_kinds); its value never is; its box is painted in captures; typing and text-entering keys into it
  // are refused; it is filled only through the sealed path.
  function fieldKind(el) {
    const card = cardKind(el);
    if (card) return card;
    if (el.tagName !== "INPUT") return null;
    if ((el.type || "").toLowerCase() === "password" || /\bcurrent-password\b/.test(el.autocomplete || "")) return "password";
    if (codeField(el)) return "one_time_code";     // a code marker wins over new-password (codeFill's own rule)
    if (passwordField(el)) return /\bnew-password\b/.test(el.autocomplete || "") ? "new_password" : "password";
    return null;
  }
  const isSecret = (el) => sealedFields.has(el) || fieldKind(el) !== null;
  // Whether a login code may be sealed into this field; describe() reports it so the daemon refuses before asking the
  // owner, by this same rule. type=password and autocomplete current-password always refuse. A code marker overrides
  // autocomplete new-password, which pages set on OTP boxes to stop autofill (controller ruling).
  const codeFill = (el, secret = isSecret(el)) => secret && el.tagName === "INPUT" && !cardKind(el) &&
    (el.type || "").toLowerCase() !== "password" &&
    !/\bcurrent-password\b/.test(el.autocomplete || "") && (codeField(el) || !passwordField(el));

  // What submitting from this control would do (protocol 18, GATE-202), from the form's own markup: a click
  // on a submit control (`click`), or Enter in a field whose form submits implicitly (`enter`), with the action's host
  // (formaction first), the method (formmethod first) and the secret field kinds the form holds. Read through the
  // prototypes' getters, so a field named "action" or "method" can't stand in. null: no form, or neither.
  const FORM = typeof HTMLFormElement !== "undefined" ? HTMLFormElement.prototype : null;
  const formGet = (f, name) => { try { return Object.getOwnPropertyDescriptor(FORM, name).get.call(f); } catch (_) { return null; } };
  const TEXTLIKE = new Set(["text", "search", "email", "url", "tel", "password", "number", "date", "datetime-local",
                            "month", "week", "time", ""]);
  function submitInfo(el) {
    const f = el.form;
    if (!f || !FORM) return null;
    const type = String(el.type || "").toLowerCase();
    const click = (el.tagName === "BUTTON" && (el.getAttribute("type") || "submit").toLowerCase() === "submit") ||
      (el.tagName === "INPUT" && (type === "submit" || type === "image"));
    const fields = Array.from(formGet(f, "elements") || []);
    const defaultButton = fields.find((n) => (n.tagName === "BUTTON" && (n.getAttribute("type") || "submit").toLowerCase() === "submit") ||
      (n.tagName === "INPUT" && ["submit", "image"].includes(String(n.type).toLowerCase())));
    const single = fields.filter((n) => n.tagName === "INPUT" && TEXTLIKE.has(String(n.type || "").toLowerCase()));
    const enter = el.tagName === "INPUT" && TEXTLIKE.has(type) && (!!defaultButton || single.length === 1);
    if (!click && !enter) return null;
    const via = click ? el : defaultButton || null;
    let action = "";
    try { action = (via && via.hasAttribute("formaction") ? via.formAction : formGet(f, "action")) || ""; } catch (_) { action = ""; }
    let host = null;
    try { host = new URL(action, location.href).hostname || null; } catch (_) { host = null; }
    const method = String((via && via.getAttribute("formmethod")) || formGet(f, "method") || "get").toUpperCase();
    const kinds = [...new Set(fields.map((n) => fieldKind(n)).filter(Boolean))];
    return { click, enter, action_host: host, method, kinds, ...(via ? { label: label(via).slice(0, 200) } : {}) };
  }

  // Where the element is in the document, as a path of tags and same-tag positions from the root, through shadow roots
  // ("#shadow" marks a host's root): P19's identity for an act on it when the debugger gave no node id.
  function domPath(el) {
    const parts = [];
    for (let n = el; n && n.nodeType === 1 && parts.length < 64;) {
      const p = n.parentElement;
      let i = 1;
      for (let s = n.previousElementSibling; s; s = s.previousElementSibling) if (s.tagName === n.tagName) i++;
      parts.push(`${n.tagName.toLowerCase()}:${i}`);
      if (p) { n = p; continue; }
      const root = n.getRootNode ? n.getRootNode() : null;
      if (root && root.host) { parts.push("#shadow"); n = root.host; } else break;
    }
    return parts.reverse().join(">");
  }


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

  // HAND-489: a control the page made of a plain element (a date tab, a slot button: a div with a click handler) has
  // no role; the outermost element of each cluster the page draws with a pointer cursor counts as one, role
  // `clickable`, labelled by its text (a structural fact of its style). Never one inside a real control.
  const clickables = new WeakSet();
  const INTERACTIVE = 'a[href],button,input,select,textarea,summary,label,[role="button"],[role="link"],[role="tab"],' +
    '[role="menuitem"],[role="option"],[role="checkbox"],[role="radio"],[role="switch"]';
  const cursorOf = (el) => { try { return getComputedStyle(el).cursor; } catch (_) { return ""; } };
  function pointerCluster(el) {
    if (el.tagName === "BODY" || el.tagName === "HTML" || SKIP_TEXT.has(el.tagName) || cursorOf(el) !== "pointer") return false;
    const p = parentOf(el);
    if (p && cursorOf(p) === "pointer") return false;            // inside a larger pointer cluster: that one counts
    if (el.closest(INTERACTIVE)) return false;
    return !!((el.textContent || "").trim() || el.getAttribute("aria-label") || el.querySelector("img[alt]"));
  }
  // Every candidate in document order, descending into shadow roots (closed ones too) right after their host.
  function collect(root, out) {
    for (const el of root.querySelectorAll("*")) {
      if (el.matches(SEL) && admits(el)) out.push(el);
      else if (pointerCluster(el)) { clickables.add(el); out.push(el); }
      const sr = shadowOf(el);
      if (sr) collect(sr, out);
    }
  }
  // Every frame element in document order, inside shadow roots too, so a frame there (Cloudflare's checkbox) is
  // bound and linked to its child frame like any other.
  function frameElements() {
    const out = [];
    eachElement(document, (el) => { if (el.tagName === "IFRAME" || el.tagName === "FRAME") out.push(el); });
    return out;
  }

  const parentOf = (el) => el.parentElement || (el.getRootNode() instanceof ShadowRoot ? el.getRootNode().host : null);
  // A scroll pane, on each axis (HAND-510): overflow that scrolls (auto, scroll, overlay) with more to show than its box.
  const scrollerCache = new Map();
  const SCROLLING = new Set(["auto", "scroll", "overlay"]);
  function scrollAxes(p) {
    let v = scrollerCache.get(p);
    if (v === undefined) {
      const s = getComputedStyle(p);
      v = { y: SCROLLING.has(s.overflowY) && p.scrollHeight > p.clientHeight + 1,
            x: SCROLLING.has(s.overflowX) && p.scrollWidth > p.clientWidth + 1 };
      scrollerCache.set(p, v);
    }
    return v;
  }
  function isScroller(p, axis = "any") {
    const a = scrollAxes(p);
    return axis === "y" ? a.y : axis === "x" ? a.x : a.x || a.y;
  }
  function scrollerOf(el, axis = "any") {
    for (let p = parentOf(el); p && p !== document.documentElement && p !== document.body; p = parentOf(p))
      if (isScroller(p, axis)) return p;
    return null;
  }
  function scrollState(sc) {       // sc: a container element or null for the window; both axes (HAND-510)
    const d = sc || document.scrollingElement || document.documentElement;
    const vh = sc ? sc.clientHeight : innerHeight, vw = sc ? sc.clientWidth : innerWidth;
    return { y: Math.round(d.scrollTop), height: d.scrollHeight, viewport_h: vh,
             at_end: d.scrollTop + vh >= d.scrollHeight - 2,
             x: Math.round(d.scrollLeft), width: d.scrollWidth, viewport_w: vw,
             at_end_x: d.scrollLeft + vw >= d.scrollWidth - 2 };
  }
  // A scroll pane's id for the latest snapshot (`c1`, ...), the same one each time it is asked for.
  function containerId(sc) {
    let id = sc.__ferbCid;
    if (!id || !containers.has(id) || containers.get(id).deref() !== sc) {
      id = `c${containers.size + 1}`;
      sc.__ferbCid = id;
      containers.set(id, new WeakRef(sc));
    }
    return id;
  }
  // Every scroll pane the page draws, including ones that hold only text (a feed, a log) and sideways ones (a
  // carousel), in document order; at most SCROLLERS_MAX (HAND-510).
  const SCROLLERS_MAX = 60;
  function allScrollers() {
    const out = [];
    eachElement(document, (el) => {
      if (out.length >= SCROLLERS_MAX || el === document.body || el === document.documentElement) return;
      if (el.scrollHeight <= el.clientHeight + 1 && el.scrollWidth <= el.clientWidth + 1) return;   // cheap first
      if (!isScroller(el) || !visible(el)) return;
      const r = el.getBoundingClientRect();
      if (r.width >= 20 && r.height >= 20) out.push(el);
    });
    return out;
  }
  // HAND-510: a pane (or the page) to its start or end on one axis, at once (a virtual list then draws what is there)
  function scrollEdge(cid, to, axis) {
    const ref = cid && containers.get(cid), sc = ref ? ref.deref() : null;
    if (cid && (!sc || !sc.isConnected)) return { error: "stale" };
    const d = sc || document.scrollingElement || document.documentElement;
    const end = to === "end";
    if (axis === "x") d.scrollTo({ left: end ? d.scrollWidth : 0, behavior: "instant" });
    else d.scrollTo({ top: end ? d.scrollHeight : 0, behavior: "instant" });
    return scrollState(sc);
  }

  // L33: what a scroll into view moves besides the page, for FERB's cursor (geometry facts only), up the element's
  // ancestors as the page is drawn (`drawnParent`: a slotted element through its shadow tree's wrappers to the host).
  // `pinnedOf`: the element or an ancestor is position fixed or sticky (sticky wins), or "unknown" for a fixed one
  // inside a containing block for fixed descendants (`framesFixed`: a transform, filter, contain, will-change...), which
  // then scrolls with the page after all. `boxedBy`: that node or an ancestor below the body is a box a scroll into view
  // can scroll, on either axis (overflow other than visible or clip, with more to show: a carousel, hidden overflow).
  // `bodyScrolls`: the body scrolls by itself instead of the page (html's overflow isn't visible, so the body's stays
  // its own). Cached per snapshot, like scrollerCache.
  const pinCache = new Map(), boxCache = new Map(), frameCache = new Map();
  const SCROLLS = new Set(["auto", "scroll", "overlay", "hidden"]);
  const FRAMING = ["transform", "translate", "rotate", "scale", "perspective", "filter", "backdropFilter"];
  function framesFixed(s) {
    return FRAMING.some((k) => s[k] && s[k] !== "none") || /\b(paint|layout|strict|content)\b/.test(s.contain || "") ||
      /\b(transform|translate|rotate|scale|perspective|filter|backdrop-filter)\b/.test(s.willChange || "") ||
      (!!s.containerType && s.containerType !== "normal") || s.contentVisibility === "auto";
  }
  function framed(p) {                             // p or an ancestor below the root frames fixed descendants
    if (!p || p === document.documentElement) return false;
    let v = frameCache.get(p);
    if (v === undefined) {
      v = framesFixed(getComputedStyle(p)) || framed(drawnParent(p));
      frameCache.set(p, v);
    }
    return v;
  }
  function pinnedOf(el) {
    if (!el || el === document.documentElement) return null;
    let v = pinCache.get(el);
    if (v === undefined) {
      const own = getComputedStyle(el).position, up = pinnedOf(drawnParent(el));
      if (up === "unknown" || up === "sticky") v = up;
      else if (own === "sticky") v = "sticky";
      else if (own === "fixed") v = framed(drawnParent(el)) ? "unknown" : "fixed";
      else v = up;
      pinCache.set(el, v);
    }
    return v;
  }
  function overflows(p) {
    const s = getComputedStyle(p);
    return (SCROLLS.has(s.overflowX) && p.scrollWidth > p.clientWidth + 1) ||
           (SCROLLS.has(s.overflowY) && p.scrollHeight > p.clientHeight + 1);
  }
  function boxedBy(p) {
    if (!p || p === document.body || p === document.documentElement) return false;
    let v = boxCache.get(p);
    if (v === undefined) {
      v = overflows(p) || boxedBy(drawnParent(p));
      boxCache.set(p, v);
    }
    return v;
  }
  function bodyScrolls() {
    const b = document.body, root = document.documentElement;
    if (!b || !root || b === document.scrollingElement ||
        !(b.scrollHeight > b.clientHeight + 1 || b.scrollWidth > b.clientWidth + 1)) return false;
    const r = getComputedStyle(root);
    return !(r.overflowX === "visible" && r.overflowY === "visible") && overflows(b);
  }

  // HAND-490: a hit reaches the element when it is the element, inside it, or inside a <label> of it (clicking a label
  // clicks its control).
  const reaches = (el, hit) => {
    if (!hit) return false;
    if (hit === el || el.contains(hit)) return true;
    const lab = hit.closest ? hit.closest("label") : null;
    return !!lab && (lab.control === el || (el.labels ? Array.from(el.labels).includes(lab) : false));
  };
  // The first point inside the element's box (clamped to the viewport) whose hit reaches it: the centre, then a 3x3
  // grid at 20/50/80%. {x, y}, or {cover}: what is drawn on the centre when none reaches it (null: nothing is).
  function hitPoint(el, r) {
    const root = el.getRootNode();
    const at = (x, y) => (root instanceof ShadowRoot ? root : document).elementFromPoint(x, y);
    const clamp = (x, y) => [Math.min(Math.max(x, 0), innerWidth - 1), Math.min(Math.max(y, 0), innerHeight - 1)];
    const [cx, cy] = clamp(r.x + r.width / 2, r.y + r.height / 2);
    const centre = at(cx, cy);
    if (reaches(el, centre)) return { x: cx, y: cy };
    for (const fy of [0.2, 0.5, 0.8]) for (const fx of [0.2, 0.5, 0.8]) {
      const [x, y] = clamp(r.x + r.width * fx, r.y + r.height * fy);
      if (reaches(el, at(x, y))) return { x, y };
    }
    return { cover: centre || null };
  }
  function hitTest(el, r) { return !("x" in hitPoint(el, r)); }   // nothing painted there counts as covered
  // What covers an element, in words for the worker: its role and label, and whether it is a fixed or sticky bar.
  function coverOf(c) {
    if (!c) return "nothing the page draws";
    let bar = false;
    for (let a = c; a && a.nodeType === 1; a = a.parentElement) {
      const p = getComputedStyle(a).position;
      if (p === "fixed" || p === "sticky") { bar = true; break; }
    }
    const lab = label(c).slice(0, 60);
    return `${c.getAttribute("role") || implicitRole(c)}${lab ? ` "${lab}"` : ""}${bar ? " (a fixed or sticky bar)" : ""}`;
  }

  function describe(el, id) {
    const r = el.getBoundingClientRect(), secret = isSecret(el);
    const own = el.getAttribute("role") || implicitRole(el);
    const role = own === "generic" && clickables.has(el) ? "clickable" : own;   // HAND-489
    let value = null;
    if (el.tagName === "SELECT") value = secret ? (el.value ? "filled" : "") : Array.from(el.selectedOptions).map(o=>o.text).join("\n");
    else if (el.tagName === "INPUT" && (el.type === "checkbox" || el.type === "radio")) value = el.checked ? "checked" : "";
    else if (el.tagName === "INPUT" && el.type === "file") value = el.files && el.files.length ? `${el.files.length} file(s)` : "";
    else if (el.tagName === "INPUT" || el.tagName === "TEXTAREA")
      value = secret || maskedEditable(el) ? (el.value ? "filled" : "") : el.value;   // preserve editor newlines and whitespace
    else if (el.isContentEditable) value = maskedEditable(el) ? (el.innerText ? "filled" : "") : el.innerText;
    const meets = r.right > 0 && r.bottom > 0 && r.left < innerWidth && r.top < innerHeight;
    const occluded = meets ? hitTest(el, r) : false;
    const sc = scrollerOf(el);
    const container = sc ? containerId(sc) : null;
    const e = { id, role, label: label(el), value, rect: [round(r.x), round(r.y), round(r.width), round(r.height)],
                in_view: meets && !occluded, input_type: el.tagName === "INPUT" ? (el.type || "text") : null,
                href: el.tagName === "A" ? maskUrl(el.href) : null, form: formKind(el), secret,
                code_fill: codeFill(el, secret),
                capture_masked: sealedSplitDocument };
    if (!meets) e.offscreen = r.bottom <= 0 ? "above" : r.top >= innerHeight ? "below" : r.right <= 0 ? "left" : "right";
    if (occluded) e.occluded = true;
    if (container) e.container = container;
    // HAND-483: inside an open dialog (the preview keeps it first); a structural fact of the markup
    if (el.closest && el.closest('dialog[open],[role="dialog"],[role="alertdialog"],[aria-modal="true"]')) e.dialog = true;
    if (terminalOf(el)) e.terminal = true;
    const kind = fieldKind(el);
    if (kind) e.field_kind = kind;
    const submit = submitInfo(el);
    if (submit) e.submit = submit;
    e.path = domPath(el);
    const pinned = pinnedOf(el);                     // L33: what a scroll into view moves besides the page
    if (pinned) e.pinned = pinned;
    if (boxedBy(drawnParent(el))) e.scroll_box = true;
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

  // D31: an element inside a terminal library's own nodes, so the daemon asks the owner before typing into it.
  // xterm.js marks its root `xterm` and its input `xterm-helper-textarea`: classes the library sets, never page words.
  function terminalOf(el) {
    return !!(el.classList && (el.classList.contains("xterm-helper-textarea") || el.closest(".xterm")));
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

  // Frame links for capture masks (L13). Chromium can't name the frame inside a frame element (no
  // chrome.runtime.getFrameId), so each child frame announces itself: a fresh token for this round, posted to its
  // parent (announceFrame). The parent counts it only when the message's source, which the browser sets, is the window
  // of one of its frame hosts (any element with a contentWindow: listenForFrames, then captureState); only code
  // running in that frame can post from that window. Synthetic events (not isTrusted) are dropped as well. The background links a host to a frame only when it heard exactly one token, nothing else heard
  // that token, and exactly one frame owns it; it leaves a host unpainted only when that frame, and every frame in
  // it, is linked so and shows no secret field. Anything else paints the host whole. Only code running inside a child
  // frame can speak as that frame, so what a page forges (dropped, added or repeated messages) only adds black.
  const ROUND = "frameLinkRound", TOKEN = "frameLinkToken";   // neutral message keys
  const FRAME_TAGS = new Set(["IFRAME", "FRAME", "OBJECT", "EMBED", "FENCEDFRAME"]);
  const hostIds = new WeakMap();                   // frame host -> id, stable while this document lives
  const fieldIds = new WeakMap();                  // secret field -> id, stable while this document lives
  // Per round, so two looks at one tab that overlap never reset each other: this frame's hosts, what each heard,
  // and this frame's own token. Dropped once the round is reported.
  const rounds = new Map();                        // round -> {hosts, tokens: Map(host -> [token]), own}
  const roundOf = (round) => {
    let r = rounds.get(round);
    if (!r) { r = {hosts: [], tokens: new Map(), own: null}; rounds.set(round, r); }
    return r;
  };
  const stableId = (map, el) => { let id = map.get(el); if (!id) { id = randomId(); map.set(el, id); } return id; };
  const shadowOf = (el) => {                       // closed shadow roots too (content scripts may open them)
    try {
      return chrome.dom && chrome.dom.openOrClosedShadowRoot ? chrome.dom.openOrClosedShadowRoot(el) : el.shadowRoot;
    } catch (_) { return el.shadowRoot; }
  };
  function eachElement(root, visit) {
    for (const el of root.querySelectorAll("*")) {
      visit(el);
      const sr = shadowOf(el);
      if (sr) eachElement(sr, visit);
    }
  }
  const windowOf = (el) => { try { return el.contentWindow || null; } catch (_) { return null; } };
  const isHost = (el) => FRAME_TAGS.has(el.tagName) || windowOf(el) !== null;
  addEventListener("message", (e) => {
    const d = e.data;
    if (!e.isTrusted || !e.source || !d || typeof d !== "object" || typeof d[ROUND] !== "string"
        || typeof d[TOKEN] !== "string" || !rounds.has(d[ROUND])) return;
    const r = rounds.get(d[ROUND]);
    const host = r.hosts.find((el) => windowOf(el) === e.source);
    if (!host) return;                             // not from the window of one of this frame's hosts
    r.tokens.set(host, (r.tokens.get(host) || []).concat(d[TOKEN]));
  }, true);
  function listenForFrames(round) {                // first round: every frame listens before any announces
    const r = roundOf(round);
    eachElement(document, (el) => { if (isHost(el)) r.hosts.push(el); });
    return true;
  }
  function announceFrame(round) {
    const r = roundOf(round);
    r.own = randomId();
    if (window !== top) parent.postMessage({[ROUND]: round, [TOKEN]: r.own}, "*");
    return true;
  }
  // An element's parent as the page is drawn (the flat tree): the <slot> it is drawn through when its parent holds a
  // shadow root (closed too: content scripts can open it, though a closed root hides assignedSlot), the shadow
  // root's host at the top of a shadow tree, else its parent element.
  function drawnParent(el) {
    const p = el.parentElement;
    if (p) {
      const sr = shadowOf(p);
      if (!sr) return p;
      if (el.assignedSlot) return el.assignedSlot;
      for (const slot of sr.querySelectorAll("slot")) if (slot.assignedElements().includes(el)) return slot;
      return p;                                    // no slot found (a browser's own shadow tree): its parent
    }
    const root = el.getRootNode ? el.getRootNode() : null;
    return (root && root.host) || null;
  }
  // Whether the element is drawn again outside its box, where no mask goes: -webkit-box-reflect (a mirrored copy) or
  // an SVG filter (url(...): feOffset/feTile with a user-space region redraw content elsewhere at full fidelity) on
  // it or anything it is drawn inside, including a <details>' content box (::details-content) around everything but
  // its summary, open or not (CSS can show closed content). Plain filters (blur, drop-shadow) only smear or
  // silhouette. Fails closed: a value that isn't a plain answer counts as a copy.
  const styleValue = (cs, name) => { try { return cs.getPropertyValue(name); } catch (_) { return null; } };
  const copies = (cs) => {
    const reflect = styleValue(cs, "-webkit-box-reflect"), filter = styleValue(cs, "filter");
    return reflect !== "none" || typeof filter !== "string" || filter.includes("url(");
  };
  function redrawn(el) {
    for (let a = el; a; a = drawnParent(a)) {
      if (copies(getComputedStyle(a))) return true;
      const p = a.parentElement;
      if (p && p.tagName === "DETAILS" && a !== Array.from(p.children).find((c) => c.tagName === "SUMMARY")
          && copies(getComputedStyle(p, "::details-content"))) return true;
    }
    return false;
  }

  // What a screenshot or recording frame must paint over in this frame (L13), in its viewport CSS px: every secret
  // field isSecret marks (password and code fields, sealed fills), keyed so a field is known again after the capture,
  // and, after a split sealed fill, every editable field of this document; each frame host with the tokens it heard
  // this round and its box (getBoundingClientRect: every transform, zoom and motion path applied); whether a field or
  // host is drawn again elsewhere (redrawn); this frame's own token; and the visual viewport (what a capture shows, pinch zoom
  // included). Geometry only, never a value. A separate inventory: a capture never replaces the snapshot/point
  // bindings the worker acts with.
  function captureState(round) {
    const rects = [], frames = [];
    const state = rounds.get(round) || {tokens: new Map(), own: null};
    rounds.delete(round);
    eachElement(document, (el) => {
      if (isHost(el)) {
        const b = el.getBoundingClientRect();
        frames.push({id: stableId(hostIds, el), heard: state.tokens.get(el) || [], rect: [b.x, b.y, b.width, b.height],
                     redrawn: redrawn(el)});
      } else if (isSecret(el) || maskedEditable(el)) {
        const b = el.getBoundingClientRect();     // painted, even when aria-hidden: masking errs on the safe side
        if (b.width > 0 && b.height > 0 && (!el.checkVisibility || el.checkVisibility({visibilityProperty: true})))
          rects.push({id: stableId(fieldIds, el), rect: [b.x, b.y, b.width, b.height], redrawn: redrawn(el)});
      }
    });
    const vv = window.visualViewport;
    return {documentId, own: state.own, rects, frames,
            viewport: vv ? [vv.offsetLeft, vv.offsetTop, vv.width, vv.height] : [0, 0, innerWidth, innerHeight]};
  }

  async function snapshot(gen, attr) {
    unbind();
    if (typeof attr === "string" && /^data-[0-9a-f]{12}$/.test(attr)) ATTR = attr;
    bindings = new Map();
    snapGen = gen;
    els = new Map(); containers = new Map(); scrollerCache.clear(); pinCache.clear(); boxCache.clear(); frameCache.clear();
    const found = [];
    collect(document, found);
    const shown = found.filter(el => visible(el) || (el.tagName === "INPUT" && el.type === "file") || standIn(el));
    const out = shown.map((el,id) => {
      const {target, e} = listed(el, id); e.binding = bind(el); els.set(id, new WeakRef(target)); return e;
    });
    // HAND-510: every scroll pane gets an id, also one that holds only text or scrolls sideways
    for (const sc of allScrollers()) containerId(sc);
    const scrollers = [];
    for (const [cid, ref] of containers) {
      const sc = ref.deref();
      if (!sc || !sc.isConnected) { containers.delete(cid); continue; }
      const r = sc.getBoundingClientRect(), axes = scrollAxes(sc);
      scrollers.push({ id: cid, label: labelText(sc.getAttribute("aria-label") || sc.getAttribute("role") || sc.id || sc.tagName.toLowerCase()),
                       axis: axes.x && axes.y ? "both" : axes.x ? "x" : "y",
                       rect: [round(r.x), round(r.y), round(r.width), round(r.height)], ...scrollState(sc) });
    }
    const iframes = [];
    for (const f of frameElements()) {
      const r = f.getBoundingClientRect();
      if (r.width <= 0 || r.height <= 0) continue;
      const meets = r.right > 0 && r.bottom > 0 && r.left < innerWidth && r.top < innerHeight;
      iframes.push({ binding: bind(f), name: f.name || "", src: maskUrl(f.src || ""), rect: [round(r.x), round(r.y), round(r.width), round(r.height)],
                     in_view: meets && !hitTest(f, r) });
    }
    const text = pageText();
    mediaEls = new Map();
    const players = mediaElements(document, []);
    players.forEach((el, index) => mediaEls.set(index, new WeakRef(el)));
    await firstLook(players.filter(el => !progress.has(el) && !el.paused && !el.ended));
    const media = players.map((el, index) => mediaState(el, index));
    // L33, for the cursor: what the screen shows of the page (its pinch zoom `scale`), its scroll sideways, and
    // whether the body scrolls by itself
    const vv = window.visualViewport, d = document.scrollingElement || document.documentElement;
    let commerce = null;
    try { commerce = pageCommerce(); } catch (_) { commerce = null; }   // best effort: never fails a snapshot
    return { gen, document_binding: bind(document.documentElement), url: maskUrl(location.href), title: document.title, name: window.name || "", top: window === top,
             ...(commerce ? { commerce } : {}),
             viewport: [innerWidth, innerHeight], scroll: {...scrollState(null), x: Math.round(d.scrollLeft), width: d.scrollWidth},
             scrollers, iframes, elements: out, text,
             visual: vv ? [vv.offsetLeft, vv.offsetTop, vv.width, vv.height, vv.scale] : [0, 0, innerWidth, innerHeight, 1],
             ...(bodyScrolls() ? {body_scrolls: true} : {}), media, truncated: false };
  }

  // An amount and a merchant the page states in structured form (protocol 18, best effort, the owner accepted it): its
  // price metadata (product:/og:price:amount with its currency), schema.org JSON-LD totalPaymentDue / price with
  // priceCurrency, and who sells (JSON-LD seller / merchant / provider name, else og:site_name). Amounts only when the
  // page states exactly one; numbers as written. No text is read for meaning. null: nothing structured.
  function pageCommerce() {
    const amounts = new Set();
    let merchant = null;
    const meta = (p) => {
      const m = document.querySelector(`meta[property="${p}"],meta[name="${p}"]`);
      return m ? String(m.getAttribute("content") || "").trim() : "";
    };
    const add = (value, currency) => {
      const v = String(value ?? "").trim(), c = String(currency ?? "").trim();
      if (/^\d[\d,]*(?:\.\d+)?$/.test(v) && c.length <= 8) amounts.add(c ? `${c} ${v}` : v);
    };
    add(meta("product:price:amount") || meta("og:price:amount"), meta("product:price:currency") || meta("og:price:currency"));
    const named = (v) => (v && typeof v === "object" && typeof v.name === "string" ? v.name.trim() : null);
    let seen = 0;
    const walk = (node, depth) => {
      if (!node || typeof node !== "object" || depth > 6 || ++seen > 500) return;
      if (Array.isArray(node)) { node.forEach((n) => walk(n, depth + 1)); return; }
      const due = node.totalPaymentDue;
      if (due && typeof due === "object") add(due.price ?? due.value, due.priceCurrency ?? due.currency);
      if (node.price !== undefined && (typeof node.price === "string" || typeof node.price === "number")) add(node.price, node.priceCurrency);
      merchant = merchant || named(node.seller) || named(node.merchant) || named(node.provider);
      for (const k of Object.keys(node)) if (node[k] && typeof node[k] === "object") walk(node[k], depth + 1);
    };
    for (const s of Array.from(document.querySelectorAll('script[type="application/ld+json"]')).slice(0, 20)) {
      try { walk(JSON.parse(s.textContent || ""), 0); } catch (_) { /* not JSON */ }
    }
    merchant = merchant || meta("og:site_name") || null;
    const amount = amounts.size === 1 ? [...amounts][0] : null;
    if (!amount && !merchant) return null;
    return { amount, merchant: merchant ? merchant.slice(0, 200) : null };
  }

  // Video and audio elements (L26), in document order with open shadow roots, visible or not (an <audio> never is).
  function mediaElements(root, out) {
    for (const el of root.querySelectorAll("*")) {
      if (el.tagName === "VIDEO" || el.tagName === "AUDIO") out.push(el);
      if (el.shadowRoot) mediaElements(el.shadowRoot, out);
    }
    return out;
  }
  // Whether an element's media really plays (I5): its time moved while it wasn't paused, seen in the page's own
  // timeupdate events or between two of FERB's looks. play() clears `paused` at once, even before (or without) the
  // browser starting the media, so `!paused` is no proof. A new start, a pause, a seek or a stall needs fresh movement.
  const progress = new WeakMap();
  function moving(el) {
    let p = progress.get(el);
    if (!p) {
      p = { at: el.currentTime, moved: false };
      progress.set(el, p);
      if (typeof el.addEventListener === "function") {
        el.addEventListener("timeupdate", () => { if (!el.paused && el.currentTime !== p.at) p.moved = true; p.at = el.currentTime; });
        for (const type of ["play", "pause", "seeking", "waiting", "emptied", "ended"])
          el.addEventListener(type, () => { p.moved = false; p.at = el.currentTime; });
      }
    } else if (!el.paused && el.currentTime !== p.at) { p.moved = true; p.at = el.currentTime; }
    return p.moved;
  }
  function restart(el) { const p = progress.get(el); if (p) { p.moved = false; p.at = el.currentTime; } }
  // Media FERB hasn't looked at before has nothing to compare its time with, so `playing` would read false even while
  // it plays (live 2026-10-25: every loop round took a second snapshot to learn the reel played). Unpaused, it is
  // watched for up to FIRST_LOOK_MS: its timeupdate ends the wait once its time moved, and moving() compares the time
  // again at the end, so the first snapshot already says whether it plays. Paused or ended media isn't waited for.
  const FIRST_LOOK_MS = 300;
  function firstLook(fresh) {
    if (!fresh.length) return Promise.resolve();
    fresh.forEach(moving);                                     // tracking starts: its listener, its starting time
    return new Promise((resolve) => {
      const waiting = new Set(fresh), heard = [];
      const done = () => {
        clearTimeout(timer);
        for (const [el, fn] of heard) if (typeof el.removeEventListener === "function") el.removeEventListener("timeupdate", fn);
        resolve();
      };
      const timer = setTimeout(done, FIRST_LOOK_MS);
      for (const el of fresh) {
        if (typeof el.addEventListener !== "function") continue;
        const fn = () => { if (progress.get(el).moved && waiting.delete(el) && !waiting.size) done(); };
        el.addEventListener("timeupdate", fn);                  // after moving()'s own listener: `moved` is current
        heard.push([el, fn]);
      }
    });
  }
  // Each media element's own id for this page's lifetime (L31): given on first sight, never given to another element.
  // List positions shift as a feed loads more videos; a loop's probe watches one element by its id. The page's own
  // token ends it, so an id from an earlier page of the tab matches nothing in this one (review I2).
  const mediaIds = new WeakMap();
  const mediaPage = randomId().slice(0, 6);
  let mediaSeen = 0;
  function mediaId(el) {
    if (!mediaIds.has(el)) mediaIds.set(el, `m${++mediaSeen}-${mediaPage}`);
    return mediaIds.get(el);
  }
  function mediaState(el, index) {
    const r = el.getBoundingClientRect();
    const finite = v => Number.isFinite(v) ? round(v) : null;    // an unknown or live duration is null
    return { index, id: mediaId(el), playing: !el.paused && !el.ended && moving(el), paused: !!el.paused, current_time: finite(el.currentTime),
             duration: finite(el.duration), muted: !!el.muted, volume: finite(el.volume),
             in_view: r.width > 0 && r.height > 0 && r.right > 0 && r.bottom > 0 && r.left < innerWidth && r.top < innerHeight };
  }
  // The page's own media controls on one element of the latest snapshot: never a click, never page script. play()
  // is given one turn of the event loop so a refusal (no user gesture) is reported, not waited on.
  async function media(gen, index, action, to) {
    if (gen !== snapGen) return { error: "stale" };
    const ref = mediaEls.get(index), el = ref && ref.deref();
    if (!el || !el.isConnected) return { error: "stale" };
    let error = null;
    try {
      if (action === "play") {
        if (el.paused) restart(el);
        const refused = await Promise.race([Promise.resolve(el.play()).then(() => null, e => e),
                                            new Promise(r => setTimeout(() => r(null), 0))]);
        if (refused) error = String(refused.name || refused);
      } else if (action === "pause") el.pause();
      else if (action === "mute") el.muted = true;
      else if (action === "unmute") el.muted = false;
      else if (action === "seek" && Number.isFinite(to) && to >= 0) { el.currentTime = to; restart(el); }
      else return { error: "unsupported media action" };
    } catch (e) { error = String((e && e.name) || e); }
    return error ? { ...mediaState(el, index), error } : mediaState(el, index);
  }

  function element(gen, id) {
    if (gen !== snapGen) return { error: "stale" };
    const ref = els.get(id), el = ref && ref.deref();
    if (!el || !el.isConnected) return { error: "stale" };
    return { el };
  }

  // HAND-487: a watch on an act's target, read again after it (background.js observeAct). Its state (the value too) is
  // kept here and only compared: what leaves is which parts changed, never a value.
  let nearWatch = null;
  const NEAR_ATTRS = ["aria-expanded", "aria-checked", "aria-selected", "aria-pressed", "aria-disabled", "open", "disabled"];
  const DIALOG_SCOPE = 'dialog,[role="dialog"],[role="alertdialog"]';
  function nearState(el) {
    const s = {value: ["INPUT", "SELECT", "TEXTAREA"].includes(el.tagName) ? String(el.value)
                      : el.isContentEditable ? el.innerText : null, checked: el.checked === true};
    for (const k of NEAR_ATTRS) s[k] = el.getAttribute(k);
    return s;
  }
  function watchNear(gen, id) {
    const { el, error } = element(gen, id);
    if (error) return false;
    if (nearWatch) nearWatch.observer.disconnect();
    const scope = el.closest(`form,${DIALOG_SCOPE}`) || el.parentElement || el;
    const w = { el, before: nearState(el), mutations: 0,
                scope: scope.tagName === "FORM" ? "form" : scope.matches(DIALOG_SCOPE) ? "dialog" : "surroundings" };
    w.observer = new MutationObserver((records) => {
      w.mutations += records.filter((r) => r.type !== "attributes" || (r.attributeName !== ATTR && r.attributeName !== STAMP)).length;
    });
    w.observer.observe(scope, { subtree: true, childList: true, characterData: true, attributes: true });
    setTimeout(() => w.observer.disconnect(), 5000);
    nearWatch = w;
    return true;
  }
  function nearChange() {
    const w = nearWatch;
    if (!w) return null;
    const gone = !w.el.isConnected, now = gone ? null : nearState(w.el);
    const state = now ? Object.keys(now).filter((k) => now[k] !== w.before[k]).map((k) => k.replace(/^aria-/, "")) : [];
    return { gone, state, mutations: w.mutations, scope: w.scope };
  }

  // HAND-496: this frame's real markup for browser.html, read only: the document, one element of the latest snapshot
  // (`id`) or every match of a CSS selector (light DOM and every shadow root, closed ones too). Tags, attributes and
  // text as the page has them; shadow roots as <#shadow-root>; comments left out; scripts' and styles' contents left
  // out (inline scripts only when `scripts`); an <svg>'s inside left out. What the snapshot never gives stays out
  // here too: a secret field's value attribute (password, login code, card: isSecret), and every editable field's after a
  // split sealed fill (maskedEditable) is "[masked]", a secret <textarea>'s or masked editable's text is left out, and
  // every URL attribute goes through mask.js (maskUrl). Never a live typed value (properties aren't read). An element of
  // the latest snapshot carries `ferb-el-<nonce>="<its id here>"`, a frame element `ferb-frame-<nonce>="<binding>"`,
  // which the extension and the daemon turn into ids; the per-call nonce keeps the page from forging either.
  const URL_ATTRS = new Set(["href", "src", "action", "formaction", "poster", "cite", "data", "background", "ping"]);
  const SKIP_CONTENT = new Set(["STYLE", "NOSCRIPT", "TEMPLATE"]);
  const MARKUP_MAX = 8000000;                     // characters per frame, at most (HAND-496)
  const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  function markup(gen, id, selector, scripts, nonce, drawnOnly) {
    if (drawnOnly && (!document.body || innerWidth <= 0 || innerHeight <= 0 || !document.body.getClientRects().length))
      return { undrawn: true, url: maskUrl(location.href) };
    let roots;
    if (id !== null && id !== undefined) {
      const { el, error } = element(gen, id);
      if (error) return { error };
      roots = [el];
    } else if (selector) {
      roots = [];
      try {
        const deep = (root) => {
          roots.push(...root.querySelectorAll(selector));
          for (const host of root.querySelectorAll("*")) { const sr = shadowOf(host); if (sr) deep(sr); }
        };
        deep(document);
      } catch (e) { return { error: `selector: ${String(e.message || e).slice(0, 200)}` }; }
    } else roots = [document.documentElement];
    const ids = new Map();
    if (gen === snapGen) for (const [lid, ref] of els) { const e = ref.deref(); if (e) ids.set(e, lid); }
    const out = [];
    let size = 0, cut = false;
    const put = (s) => { if (size + s.length > MARKUP_MAX) { cut = true; return; } out.push(s); size += s.length; };
    const walk = (node, depth) => {
      if (cut) return;
      if (node.nodeType === 3) {
        const p = node.parentElement;
        if (p && (SKIP_CONTENT.has(p.tagName) || maskedEditable(p) || (p.tagName === "TEXTAREA" && isSecret(p)) ||
                  (p.tagName === "SCRIPT" && !(scripts && !p.hasAttribute("src"))))) return;
        const t = node.data.replace(/\s+/g, " ");
        if (t.trim()) put(esc(t));
        return;
      }
      if (node.nodeType === 11) { for (const c of node.childNodes) walk(c, depth + 1); return; }
      if (node.nodeType !== 1) return;
      const el = node, tag = el.tagName.toLowerCase();
      let a = "";
      for (const at of el.attributes) {
        const n = at.name;
        if (n === ATTR || n === STAMP) continue;
        let v = at.value;
        if (n === "value" && ["INPUT", "TEXTAREA", "SELECT"].includes(el.tagName) && (isSecret(el) || maskedEditable(el)))
          v = v ? "[masked]" : "";
        else if (URL_ATTRS.has(n)) v = maskUrl(v);
        // the page's own "ferb-…" attributes are renamed, so FERB's markers (ferb-element, ferb-frame) are only FERB's
        a += ` ${n.startsWith("ferb-") ? `page-${n}` : n}="${esc(v).replace(/"/g, "&quot;")}"`;
      }
      if (ids.has(el)) a += ` ferb-el-${nonce}="${ids.get(el)}"`;
      if (FRAME_TAGS.has(el.tagName)) { const tok = tokenOf(el); if (tok) a += ` ferb-frame-${nonce}="${tok}"`; }
      put(`<${tag}${a}>`);
      const sr = shadowOf(el);
      if (sr) { put("<#shadow-root>"); for (const c of sr.childNodes) walk(c, depth + 1); put("</#shadow-root>"); }
      // an <svg>'s drawing, and a secret <select>'s options (a card expiry's default choice), are left out
      if (tag !== "svg" && !(el.tagName === "SELECT" && isSecret(el))) for (const c of el.childNodes) walk(c, depth + 1);
      put(`</${tag}>`);
    };
    for (const r of roots) { walk(r, 0); put("\n"); }
    return { url: maskUrl(location.href), html: out.join(""), matches: roots.length, ...(cut ? { truncated: true } : {}) };
  }

  function setValue(el, value) {
    const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value").set.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
  }

  // Page-mode actions (untrusted events). Trusted input goes through the debugger (background.js). A press comes with
  // its key from the one key table (protocol.js keyEvent, via background.js): key, code, keyCode, modifier bits
  // (Alt 1, Ctrl 2, Meta 4, Shift 8) and whether it puts text in (`enters`).
  function act(gen, id, kind, text, key = null, extra = null) {
    if (["type","select","press","paste"].includes(kind) && typeof text !== "string")
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
      el.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
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
      if (!key || typeof key.key !== "string") return { ok: false, detail: "unsupported_key" };
      if (key.enters && isSecret(el)) return { ok: false, detail: "secret_field" };
      el.focus();
      if (key.enters && focusedSecret()) return { ok: false, detail: "secret_field" };
      const m = Number(key.modifiers) || 0;
      const init = { key: key.key, code: key.code || "", keyCode: key.keyCode || 0, which: key.keyCode || 0, bubbles: true,
                     cancelable: true, composed: true, altKey: !!(m & 1), ctrlKey: !!(m & 2), metaKey: !!(m & 4),
                     shiftKey: !!(m & 8) };
      for (const type of ["keydown", "keypress", "keyup"]) el.dispatchEvent(new KeyboardEvent(type, init));
      return { ok: true, detail: `pressed ${init.key} (page events)` };
    }
    // HAND-506: a paste as the page's own paste event, carrying text/plain (and text/html when given), into the field
    // the element is or holds; never the system clipboard. The browser inserts nothing for a scripted paste, so text
    // lands only where the page's own handler takes it (`paste_taken`: it cancelled the event, as editors do).
    if (kind === "paste") {
      if (isSecret(el)) return { ok: false, detail: "secret_field" };
      const inside = () => { const a = deepActive(); return a && (a === el || el.contains(a)) ? a : null; };
      if (!inside()) el.focus();
      const into = inside() || el;
      if (isSecret(into)) return { ok: false, detail: "secret_field" };
      const data = new DataTransfer();
      data.setData("text/plain", text || "");
      if (extra && typeof extra.html === "string" && extra.html) data.setData("text/html", extra.html);
      const ev = new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true, composed: true });
      into.dispatchEvent(ev);
      return { ok: true, detail: "pasted (the page's paste event; the system clipboard untouched)", paste_taken: ev.defaultPrevented };
    }
    if (kind === "scroll") {                       // element-relative: its own box if it scrolls, else its container
      // HAND-510: left and right scroll sideways (the nearest pane that scrolls that way), up and down vertically
      const axis = text === "left" || text === "right" ? "x" : "y";
      const sc = isScroller(el, axis) ? el : scrollerOf(el, axis);
      const size = axis === "x" ? (sc ? sc.clientWidth : innerWidth) : (sc ? sc.clientHeight : innerHeight);
      const step = Math.round(size * 0.85) * (text === "up" || text === "left" ? -1 : 1);
      const by = axis === "x" ? {left: step, behavior: "instant"} : {top: step, behavior: "instant"};
      // instant: a smooth-scrolling page would still report where it was (L33 review)
      if (sc) sc.scrollBy(by); else window.scrollBy(by);
      return { ok: true, detail: `scrolled ${text === "up" || text === "left" || text === "right" ? text : "down"}`, ...scrollState(sc) };
    }
    return { ok: false, detail: "unknown_action" };
  }

  // A field that takes typed text: a text-like input, a textarea, or anything the page made editable.
  const NOT_TYPED = ["button", "submit", "reset", "checkbox", "radio", "file", "image", "hidden"];
  const editableField = (el) => !!el && (el.isContentEditable || el.tagName === "TEXTAREA" ||
    (el.tagName === "INPUT" && !NOT_TYPED.includes(String(el.type || "text").toLowerCase())));
  // What kind of field it is (HAND-505): "input" or "textarea" hold one value; "rich" is a document editor the page
  // made editable (contenteditable), where typing goes at the caret by default. null: no field.
  const fieldOf = (el) => !editableField(el) ? null : el.isContentEditable ? "rich" : el.tagName === "TEXTAREA" ? "textarea" : "input";
  // HAND-509: inputs whose value the browser's own picker sets: typed keys go to its segments and usually set nothing,
  // so a type sets the value through its setter, with the events a person's choice fires. The form each one takes.
  const PICKED = {date: "YYYY-MM-DD", time: "HH:MM (24-hour)", "datetime-local": "YYYY-MM-DDTHH:MM", month: "YYYY-MM",
                  week: "YYYY-Www", color: "#rrggbb", range: "a number within its min and max"};
  const pickedType = (el) => el && el.tagName === "INPUT" && Object.hasOwn(PICKED, String(el.type).toLowerCase())
    ? String(el.type).toLowerCase() : null;
  function setPicked(gen, id, text) {
    const { el, error } = element(gen, id);
    if (error) return { ok: false, detail: error };
    const type = pickedType(el);
    if (!type) return { ok: false, detail: "not_a_picker_field" };
    if (isSecret(el) || el.disabled || el.readOnly) return { ok: false, detail: isSecret(el) ? "secret_field" : "target_disabled" };
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set, before = el.value;
    setter.call(el, text);
    const same = type === "range" ? Number(el.value) === Number(text) && String(text).trim() !== ""
      : type === "color" ? el.value.toLowerCase() === String(text).trim().toLowerCase() : el.value === text;
    if (!same) {
      setter.call(el, before);                     // the browser rejected or changed it: put it back, no events
      return { ok: false, reason: "value_not_taken", detail: `the ${type} field didn't take "${String(text).slice(0, 60)}": ` +
               `it takes ${PICKED[type]}; its value is unchanged` };
    }
    el.focus();
    el.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
    el.dispatchEvent(new Event("change", { bubbles: true, composed: true }));
    return { ok: true, detail: `set the ${type} field to ${el.value} (its value and input/change events: a ${type} ` +
             "field takes no typed text)", field_value: el.value };
  }
  // What a field shows after typing, when it isn't secret (a mask reformats what was typed); false otherwise (a page
  // script's null reads as a failure, background.js inFrame).
  function fieldValue(gen, id) {
    const { el, error } = element(gen, id);
    if (error) return false;
    const a = deepActive(), f = a && (a === el || el.contains(a)) ? a : el;
    if (!["INPUT", "TEXTAREA"].includes(f.tagName) || isSecret(f) || maskedEditable(f)) return false;
    return String(f.value);
  }
  function focusedField() {                        // where typed text goes now, as fieldOf says (into shadow roots); "" none
    const a = deepActive();
    return (a && a !== document.body ? fieldOf(a) : (document.body && document.body.isContentEditable ? "rich" : null)) || "";
  }
  // For trusted input: the element centre in this frame's viewport, after scrolling it in. `keys` (type or press, which
  // need no pointer, HAND-504): an element that is covered or stays outside the page is still described, with
  // `keyboard: true`, so the keys can go to it once it is focused by script; a click's refusal stays as it was.
  function rect(gen, id, keys = false) {
    const { el, error } = element(gen, id);
    if (error) return { error };
    el.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
    const r = el.getBoundingClientRect();
    const facts = { secret: isSecret(el), disabled: !!el.disabled || el.getAttribute("aria-disabled") === "true",
                    editable: editableField(el), field: fieldOf(el), picked: pickedType(el),
                    file: el.tagName === "INPUT" && el.type === "file" };
    if ((r.width <= 0 || r.height <= 0) && !keys) return { error: "not_visible" };
    // HAND-482: a centre still outside this frame's viewport after scrolling is a fact of its own (a control parked off
    // the page, e.g. a hidden keyboard-shortcut entry), never "covered": hitTest would clamp it onto whatever is there.
    const cx = r.x + r.width / 2, cy = r.y + r.height / 2;
    const side = cy < 0 ? "top" : cy >= innerHeight ? "bottom" : cx < 0 ? "left" : cx >= innerWidth ? "right" : null;
    const outside = side ? `outside_page: the element is outside the page even after scrolling, off its ${side} ` +
                           "edge, so it can't be clicked where it is; nothing was sent. Use a control the page shows" : null;
    if (r.width <= 0 || r.height <= 0) return { keyboard: true, outside: "it has no size on the page", ...facts };
    if (outside) return keys ? { keyboard: true, outside: `outside the page (off its ${side} edge)`, outside_error: outside,
                                 ...facts } : { error: outside };
    const hit = hitPoint(el, r), occluded = !("x" in hit);       // HAND-490: the first point that reaches it
    return { x: occluded ? cx : hit.x, y: occluded ? cy : hit.y, w: r.width, h: r.height, occluded,
             ...(occluded ? { cover: coverOf(hit.cover) } : {}), ...facts };
  }

  function deepActive() {                          // into shadow roots, closed ones too (shadowOf)
    let a = document.activeElement;
    for (let sr = a && shadowOf(a); sr && sr.activeElement && sr.activeElement !== a; sr = shadowOf(a)) a = sr.activeElement;
    return a;
  }
  function focusedSecret() {                       // where keys go now is a secret field
    const a = deepActive();
    return !!a && a !== document.body && isSecret(a);
  }
  function isActive(gen, id) {       // the element (or something inside it) has focus: typing goes there
    const { el, error } = element(gen, id);
    if (error) return false;
    const a = deepActive();
    // a frame's document without the focus still names its body active (an editor whose body is the field, TinyMCE's):
    // keys go there only once its frame has the focus (HAND-505)
    if (a === document.body && typeof document.hasFocus === "function" && !document.hasFocus()) return false;
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
    return describeFiles(el.files || []);
  }
  // What a field (or a drop) now holds: name, size, type, time and the bytes' SHA-256 (null where this frame has no
  // crypto.subtle), which the daemon compares with the file it read (`bytes_match`).
  async function describeFiles(files) {
    const out=[];
    for (const file of files) {
      const digest=crypto.subtle ? await crypto.subtle.digest("SHA-256",await file.arrayBuffer()) : null;
      out.push({name:file.name,size:file.size,type:file.type,modified_ms:file.lastModified,
        sha256:digest ? Array.from(new Uint8Array(digest),b=>b.toString(16).padStart(2,"0")).join("") : null});
    }
    return out;
  }

  // HAND-501: an upload without file access. The daemon sends each file's bytes in order (`upload_part`, background.js);
  // they are kept here, in this frame's isolated world (the page can't reach them), as Blobs (the browser keeps their
  // bytes, not this script's heap), one transfer at a time: a new one drops any other, and one left idle goes after
  // UPLOAD_IDLE_MS. `uploadSet` then builds the files and gives them to the element.
  const uploadTransfers = new Map();               // transfer id -> {files: [{parts: [Blob], received}], timer}
  const UPLOAD_IDLE_MS = 60000;
  function uploadPart(transfer, index, offset, data) {
    if (typeof transfer !== "string" || !/^[0-9a-f]{32}$/.test(transfer) || !Number.isSafeInteger(index) ||
        !Number.isSafeInteger(offset) || typeof data !== "string") return { ok: false, detail: "bad_part" };
    let t = uploadTransfers.get(transfer);
    if (!t) {
      if (index !== 0 || offset !== 0) return { ok: false, detail: "transfer_missing" };
      for (const other of uploadTransfers.values()) clearTimeout(other.timer);
      uploadTransfers.clear();
      t = { files: [], timer: null };
      uploadTransfers.set(transfer, t);
    }
    clearTimeout(t.timer);
    t.timer = setTimeout(() => uploadTransfers.delete(transfer), UPLOAD_IDLE_MS);
    if (index === t.files.length && offset === 0) t.files.push({ parts: [], received: 0 });
    const f = t.files[index];
    if (!f || index !== t.files.length - 1 || f.received !== offset) {
      uploadTransfers.delete(transfer); clearTimeout(t.timer);
      return { ok: false, detail: "part_out_of_order" };
    }
    let bytes;
    try { const bin = atob(data); bytes = new Uint8Array(bin.length); for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i); }
    catch (_) { uploadTransfers.delete(transfer); clearTimeout(t.timer); return { ok: false, detail: "bad_part" }; }
    if (bytes.length) f.parts.push(new Blob([bytes]));
    f.received += bytes.length;
    return { ok: true, received: f.received };
  }
  // The files of a transfer, on the element: a file field (`<input type=file>`, hidden ones too) gets them as its files,
  // then `input` and `change` (bubbling, composed), as a person's choice in the picker does; anything else is a drop
  // zone and gets dragenter, dragover and drop carrying them. Untrusted events: a page may ignore them, which the reply
  // says as far as it can be seen (the field's files; whether the page took the drop). Nothing is set when the bytes
  // don't add up.
  async function uploadSet(gen, id, transfer, meta) {
    const t = uploadTransfers.get(transfer);
    uploadTransfers.delete(transfer);
    if (t) clearTimeout(t.timer);
    const { el, error } = element(gen, id);
    if (error) return { ok: false, detail: error };
    if (!t || !Array.isArray(meta) || t.files.length !== meta.length)
      return { ok: false, reason: "upload_bytes_missing", detail: "the file's bytes didn't all reach the page; nothing was set" };
    const made = [];
    for (let i = 0; i < meta.length; i++) {
      const m = meta[i] || {}, f = t.files[i];
      if (f.received !== m.size) return { ok: false, reason: "upload_bytes_missing", detail: "a file arrived incomplete; nothing was set" };
      made.push(new File(f.parts, String(m.name || "file"), { type: String(m.type || ""),
                                                              lastModified: Number(m.last_modified) || Date.now() }));
    }
    const dt = new DataTransfer();
    for (const file of made) dt.items.add(file);
    if (el instanceof HTMLInputElement && el.type === "file") {
      if (made.length > 1 && !el.multiple)
        return { ok: false, reason: "single_file_field", detail: "this file field takes one file; nothing was set" };
      if (el.disabled) return { ok: false, reason: "field_disabled", detail: "the file field is disabled; nothing was set" };
      el.files = dt.files;
      const set = el.files ? el.files.length : 0;    // read before the page's handlers run: many empty the field
      if (set !== made.length) return { ok: false, reason: "files_not_set", route: "field", files_set: set,
        detail: `the field took ${set} of ${made.length} file(s); no event was sent to the page` };
      const selected = await describeFiles(el.files);
      el.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
      el.dispatchEvent(new Event("change", { bubbles: true, composed: true }));
      const after = el.files ? el.files.length : 0;
      return { ok: true, route: "field", files_set: set, selected_files: selected,
               ...(after !== set ? { files_after_change: after, note: "the page's own change handler then emptied " +
                 "the field, as sites do once they have taken the files" } : {}) };
    }
    const r = el.getBoundingClientRect();
    const at = { bubbles: true, cancelable: true, composed: true, dataTransfer: dt,
                 clientX: r.x + r.width / 2, clientY: r.y + r.height / 2 };
    el.dispatchEvent(new DragEvent("dragenter", at));
    const over = !el.dispatchEvent(new DragEvent("dragover", at));      // false: a listener took it (preventDefault)
    const taken = !el.dispatchEvent(new DragEvent("drop", at));
    return { ok: taken, route: "drop", dragover_taken: over, drop_taken: taken,
             ...(taken ? { selected_files: await describeFiles(made) }
                       : { reason: "drop_not_taken", detail: "no listener on the element took the drop (none cancelled " +
                           "it), so the page most likely ignored it; nothing is known to be uploaded" }) };
  }
  function stamp(gen, id, token, attr) {   // mark the element for a DOM.performSearch follow-up (uploads)
    const { el, error } = element(gen, id);
    if (error) return { ok: false, detail: error };
    if (!(el instanceof HTMLInputElement) || el.type !== "file") return { ok: false, detail: "not_a_file_input" };
    if (typeof attr !== "string" || !/^data-[0-9a-f]{12}s$/.test(attr)) return { ok: false, detail: "bad_stamp" };
    STAMP = attr;
    el.setAttribute(attr, token);
    return { ok: true };
  }
  function unstamp(token, attr) {
    if (typeof attr !== "string" || !/^data-[0-9a-f]{12}s$/.test(attr)) return true;
    for (const el of document.querySelectorAll(`[${attr}="${token}"]`)) el.removeAttribute(attr);
    return true;
  }

  // Form submissions this document's submit events showed (protocol 18), in the capture phase so a page's
  // stopPropagation can't hide one: the form's action host and method (the submitter's formaction/formmethod first),
  // whether the page went on with it, and the form's secret field kinds. A fetch() submission fires no submit event.
  // Kept until taken (background.js formSubmits), at most 10.
  const submits = [];
  addEventListener("submit", (e) => {
    const f = e.target;
    if (!FORM || !(f instanceof HTMLFormElement)) return;
    const via = e.submitter || null;
    let action = "", host = null;
    try { action = (via && via.hasAttribute("formaction") ? via.formAction : formGet(f, "action")) || ""; } catch (_) { action = ""; }
    try { host = new URL(action, location.href).hostname || null; } catch (_) { host = null; }
    const method = String((via && via.getAttribute("formmethod")) || formGet(f, "method") || "get").toUpperCase();
    const kinds = [...new Set(Array.from(formGet(f, "elements") || []).map((n) => fieldKind(n)).filter(Boolean))];
    if (submits.length >= 10) return;
    const entry = { action_host: host, method, kinds, trusted: e.isTrusted === true };
    submits.push(entry);
    setTimeout(() => { if (e.defaultPrevented) entry.prevented = true; }, 0);   // the page's own handlers have run
  }, true);
  function takeSubmits() { return submits.splice(0); }
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
  let droppedSince = 0;                            // saves dropped since then, told at the end of that minute
  let dropTimer = null;
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
      const now = Date.now(), name = a.getAttribute("download") || "";   // worker hears of every one (protocol 20):
      const tell = (count) => {                      // the first at once, the rest of the minute together at its end
        try { chrome.runtime.sendMessage({ ferbDropped: { name, count } }); } catch (_) { /* no worker */ }
      };
      if (now - droppedAt >= 60000) {
        droppedAt = now;
        tell(1);
      } else {
        droppedSince++;
        if (!dropTimer) dropTimer = setTimeout(() => {
          dropTimer = null;
          if (droppedSince > 0) { tell(droppedSince); droppedSince = 0; droppedAt = Date.now(); }
        }, 60000 - (now - droppedAt));
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
  // The page hook asks whether FERB drives this tab (a cancelable "<token>:armed" event: the answer is
  // preventDefault), and relays a window.open it opened blank for a URL ("<token>:popup"). The token is the service
  // worker's (protocol.js FERB_TOKEN), so the event names are no fixed marker a page could listen for.
  let hookToken = null;
  const onArmed = (e) => { if (downloadsArmed && live()) e.preventDefault(); };
  const onPopup = (e) => {                         // page content: cut to shape before it leaves the page
    if (!downloadsArmed || typeof e.detail !== "string" || e.detail.length > 16384) return;
    let p = null;
    try { p = JSON.parse(e.detail); } catch (_) { return; }
    if (p && typeof p.token === "string" && /^[0-9a-f]{32}$/.test(p.token) && typeof p.url === "string") {
      try { chrome.runtime.sendMessage({ ferbPopup: { token: p.token, url: p.url.slice(0, 8192) } }); }
      catch (_) { downloadsArmed = false; }       // orphaned: the hook loads its popup itself after 1.5 s
    }
  };
  // HAND-511: the page closes its own window (the hook's window.close): told to the service worker before it goes
  const onClosing = () => {
    if (!downloadsArmed || window !== top) return;
    try { chrome.runtime.sendMessage({ ferbClosing: true }); } catch (_) { downloadsArmed = false; }
  };
  // HAND-512: the page called window.print(), which the hook answered without the browser's preview
  const onPrint = () => {
    if (!downloadsArmed) return;
    try { chrome.runtime.sendMessage({ ferbPrint: true }); } catch (_) { downloadsArmed = false; }
  };
  function listenHook(token) {
    if (typeof token !== "string" || !/^[0-9a-f]{12}$/.test(token) || token === hookToken ||
        typeof document.addEventListener !== "function") return;
    if (hookToken) {
      document.removeEventListener(hookToken + ":armed", onArmed);
      document.removeEventListener(hookToken + ":popup", onPopup);
      document.removeEventListener(hookToken + ":closing", onClosing);
      document.removeEventListener(hookToken + ":print", onPrint);
    }
    hookToken = token;
    document.addEventListener(token + ":armed", onArmed);
    document.addEventListener(token + ":popup", onPopup);
    document.addEventListener(token + ":closing", onClosing);
    document.addEventListener(token + ":print", onPrint);
  }
  // whether FERB drives this tab now, and the hook's event names (asked once the listeners above exist)
  try {
    chrome.runtime.sendMessage({ ferbArmed: true }, (r) => {
      downloadsArmed = !!(r && r.armed === true);
      if (r && typeof r.token === "string") listenHook(r.token);
    });
  } catch (_) { /* no worker */ }
  // The service worker claimed the hook's popup and the daemon made it the task's: the hook loads its URL now. The
  // answer tells the worker the page was reached (the lab checks the ready path was taken, not the 1.5 s fallback).
  try {
    chrome.runtime.onMessage.addListener((m, sender, reply) => {
      if (m && typeof m.ferbPopupReady === "string" && hookToken) {
        document.dispatchEvent(new CustomEvent(hookToken + ":ready", { detail: m.ferbPopupReady }));
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
  function armDownloads(on, drop = false, token = null) {
    listenHook(token);
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
    if (codeOnly && !codeFill(el)) return {ok: false, detail: "not_a_code_field"};
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

  window.__ferb = { snapshot, unbind, media, uploadFiles, uploadPart, uploadSet, adopt, adoptAll, point, frameRect, visualState, captureState, listenForFrames,
    announceFrame,
    supportsSplitSecretFill: true,
    pointerArm: () => { pointerSeen = null; }, pointerSeen: () => pointerSeen || [],
    viewport: () => [innerWidth, innerHeight], act, fill, origin: () => location.origin, rect, focus, isActive,
    focusedSecret, focusedField, setPicked, fieldValue, takeSubmits, stamp, unstamp, scroller, scrolled, probeArm, probeSeen: () => probeSeen,
    scrollEdge, downloadPart, armDownloads, releaseDownload, dropDownload: (id) => { heldDownloads.delete(id); },
    watchNear, nearChange, documentKey: () => documentId, markup };
})();
