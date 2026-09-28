// FERB content script: runs in the extension's isolated world (the page can't see or change it). Snapshots,
// element lookup and page-mode actions. No decisions. Sanitizes before anything leaves the page (9.4):
// password/OTP values are reported only as filled/empty, hidden inputs are skipped, URL tokens are masked.
(() => {
  if (window.__ferb) return;
  const MAX_ELEMENTS = 400, MAX_TEXT = 8000, MAX_LABEL = 120, MAX_VALUE = 200;
  const SEL = 'a[href],button,input,select,textarea,summary,[role],[contenteditable=""],[contenteditable="true"]';
  const ROLES = new Set(["button", "link", "checkbox", "radio", "tab", "menuitem", "option", "combobox", "textbox",
    "searchbox", "switch", "slider", "listbox", "menuitemcheckbox", "menuitemradio", "treeitem"]);
  const NATIVE = new Set(["A", "BUTTON", "INPUT", "SELECT", "TEXTAREA", "SUMMARY"]);
  const SECRET_NAME = /otp|one.?time|2fa|totp|mfa|passcode|verification.?code|security.?code|\bpin\b/i;
  const SECRET_PARAM = /token|key|code|secret|session|auth|sig|password|pwd|otp|ticket|nonce|state/i;
  let els = new Map();                 // local id -> WeakRef(element), for the latest snapshot only
  let snapGen = 0;

  const clip = (s, n) => (s || "").replace(/\s+/g, " ").trim().slice(0, n);

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
    if (t === "INPUT") {
      const ty = (el.type || "text").toLowerCase();
      if (["button", "submit", "reset", "image"].includes(ty)) return "button";
      if (ty === "checkbox" || ty === "radio") return ty;
      if (ty === "search") return "searchbox";
      return "textbox";
    }
    return "generic";
  }

  function label(el) {
    let s = el.getAttribute("aria-label");
    if (!s && el.getAttribute("aria-labelledby"))
      s = el.getAttribute("aria-labelledby").split(/\s+/).map((id) => {
        const n = document.getElementById(id); return n ? n.textContent : ""; }).join(" ");
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
    const r = el.getBoundingClientRect(), st = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && st.visibility !== "hidden" && st.display !== "none"
      && !el.closest('[aria-hidden="true"],[inert]');
  }

  function snapshot(gen) {
    snapGen = gen;
    els = new Map();
    const out = [];
    for (const el of document.querySelectorAll(SEL)) {
      if (out.length >= MAX_ELEMENTS) break;
      const role = el.getAttribute("role");
      if (!NATIVE.has(el.tagName) && !(role && ROLES.has(role)) && !el.isContentEditable) continue;
      if (el.tagName === "INPUT" && el.type === "hidden") continue;
      if (!visible(el)) continue;
      const id = out.length;
      els.set(id, new WeakRef(el));
      const r = el.getBoundingClientRect(), secret = isSecret(el);
      let value = null;
      if (el.tagName === "SELECT") value = el.selectedOptions.length ? clip(el.selectedOptions[0].text, MAX_VALUE) : "";
      else if (el.tagName === "INPUT" && (el.type === "checkbox" || el.type === "radio")) value = el.checked ? "checked" : "";
      else if (el.tagName === "INPUT" || el.tagName === "TEXTAREA")
        value = secret ? (el.value ? "filled" : "") : clip(el.value, MAX_VALUE);   // never a secret value
      out.push({ id, role: role || implicitRole(el), label: label(el), value,
                 rect: [r.x, r.y, r.width, r.height], input_type: el.tagName === "INPUT" ? (el.type || "text") : null,
                 href: el.tagName === "A" ? maskUrl(el.href) : null, form: formKind(el), secret });
    }
    const text = document.body ? clip(document.body.innerText, MAX_TEXT) : "";
    return { gen, url: maskUrl(location.href), title: document.title, elements: out, text,
             truncated: out.length >= MAX_ELEMENTS || text.length >= MAX_TEXT, top: window === top };
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

  // Page-mode actions (untrusted events). Trusted input goes through the debugger (background.js).
  function act(gen, id, kind, text) {
    const { el, error } = element(gen, id);
    if (error) return { ok: false, detail: error };
    if (kind === "type") {
      if (isSecret(el)) return { ok: false, detail: "secret_field" };
      el.focus();
      if (el.isContentEditable) { el.textContent = text || ""; el.dispatchEvent(new Event("input", { bubbles: true })); }
      else setValue(el, text || "");
      return { ok: true, detail: "typed" };
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
    if (kind === "click") { el.scrollIntoView({ block: "center" }); el.click(); return { ok: true, detail: "clicked" }; }
    if (kind === "press") {
      el.focus();
      const key = text || "Enter";
      for (const type of ["keydown", "keypress", "keyup"])
        el.dispatchEvent(new KeyboardEvent(type, { key, bubbles: true, cancelable: true }));
      if (key === "Enter" && el.form) el.form.requestSubmit ? el.form.requestSubmit() : el.form.submit();
      return { ok: true, detail: `pressed ${key}` };
    }
    if (kind === "scroll") { window.scrollBy(0, text === "up" ? -600 : 600); return { ok: true, detail: "scrolled" }; }
    return { ok: false, detail: "unknown_action" };
  }

  function rect(gen, id) {           // for trusted input: element centre in this frame's viewport
    const { el, error } = element(gen, id);
    if (error) return { error };
    el.scrollIntoView({ block: "center" });
    const r = el.getBoundingClientRect();
    return { x: r.x + r.width / 2, y: r.y + r.height / 2, focus: () => el.focus() };
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
    "www.youtube.com": () => {
      const a = document.querySelector('a[href*="accounts.google.com"][aria-label*="@"], [aria-label*="@"]#avatar-btn');
      const m = a && a.getAttribute("aria-label").match(EMAIL);
      if (m) return m[0].toLowerCase();
      return document.querySelector("#avatar-btn") ? "signed_in_unknown" : null;
    },
  };
  function account() {
    const f = ACCOUNT[location.hostname];
    if (!f) return { supported: false };
    try { return { supported: true, account: f() }; } catch (_) { return { supported: true, account: null }; }
  }

  window.__ferb = { snapshot, act, rect: (g, i) => { const r = rect(g, i); if (r.focus) r.focus(); delete r.focus; return r; },
                    probeArm, probeSeen: () => probeSeen, account,
                    text: () => (document.body ? document.body.innerText : ""), url: () => location.href,
                    ready: () => document.readyState };
})();
