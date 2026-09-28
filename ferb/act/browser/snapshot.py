"""Unified page snapshot format (section 9.4): elements with ids, role, label, value, rect, visibility.

Drivers build `PageSnapshot`s; this module holds what every driver shares: the in-page walker (run in an isolated
world, so page scripts can't see or change it), the caps, the secret-field rule, and the model-facing form.
Password / OTP values never leave the page: the walker reports only whether such a field is filled.
"""

from typing import Any

from ferb.core.types import PageElement, PageSnapshot

MAX_ELEMENTS, MAX_TEXT, MAX_LABEL, MAX_VALUE = 400, 8000, 120, 200

# Returns the visible interactive elements of the frame's document, as an array (kept by reference: the driver maps
# each to Chrome's backend node id, which a page can't forge or move).
WALKER = """(() => {
  const SEL = 'a[href],button,input,select,textarea,summary,[role],[contenteditable=""],[contenteditable="true"]';
  const ROLES = new Set(['button','link','checkbox','radio','tab','menuitem','option','combobox','textbox',
    'searchbox','switch','slider','listbox','menuitemcheckbox','menuitemradio','treeitem']);
  const NATIVE = new Set(['A','BUTTON','INPUT','SELECT','TEXTAREA','SUMMARY']);
  const out = [];
  for (const el of document.querySelectorAll(SEL)) {
    if (out.length >= %d) break;
    const role = el.getAttribute('role');
    if (!NATIVE.has(el.tagName) && !(role && ROLES.has(role)) && !el.isContentEditable) continue;
    if (el.tagName === 'INPUT' && el.type === 'hidden') continue;
    const r = el.getBoundingClientRect(), st = getComputedStyle(el);
    if (r.width <= 0 || r.height <= 0 || st.visibility === 'hidden' || st.display === 'none') continue;
    if (el.closest('[aria-hidden="true"],[inert]')) continue;
    out.push(el);
  }
  return out;
})()""" % MAX_ELEMENTS

# Called on that array: element metadata + page text, by value. Secret fields report only "filled" or "".
DESCRIBE = """function(maxText, maxLabel, maxValue) {
  const SECRET = /otp|one.?time|2fa|totp|mfa|passcode|verification.?code|security.?code|\\bpin\\b/i;
  const clip = (s, n) => (s || '').replace(/\\s+/g, ' ').trim().slice(0, n);
  const implicit = el => {
    const t = el.tagName;
    if (t === 'A') return 'link';
    if (t === 'BUTTON' || t === 'SUMMARY') return 'button';
    if (t === 'SELECT') return 'combobox';
    if (t === 'TEXTAREA' || el.isContentEditable) return 'textbox';
    if (t === 'INPUT') {
      const ty = (el.type || 'text').toLowerCase();
      if (['button','submit','reset','image'].includes(ty)) return 'button';
      if (ty === 'checkbox' || ty === 'radio') return ty;
      if (ty === 'search') return 'searchbox';
      return 'textbox';
    }
    return 'generic';
  };
  const label = el => {
    let s = el.getAttribute('aria-label');
    if (!s && el.getAttribute('aria-labelledby'))
      s = el.getAttribute('aria-labelledby').split(/\\s+/).map(id => {
        const n = document.getElementById(id); return n ? n.textContent : ''; }).join(' ');
    if (!s && el.labels && el.labels.length) s = Array.from(el.labels).map(l => l.textContent).join(' ');
    if (!s) s = el.getAttribute('placeholder') || el.getAttribute('title');
    if (!s && el.tagName === 'INPUT' && ['button','submit','reset'].includes(el.type)) s = el.value;
    if (!s) { const img = el.querySelector && el.querySelector('img[alt]'); if (img) s = img.alt; }
    if (!s) s = el.innerText;
    return clip(s, maxLabel);
  };
  const isSecret = el => el.tagName === 'INPUT' && ((el.type || '').toLowerCase() === 'password'
    || /one-time-code/.test(el.autocomplete || '') || SECRET.test((el.name || '') + ' ' + (el.id || '')));
  const formKind = el => {
    const f = el.form || (el.closest && el.closest('form'));
    if (!f) return (el.type === 'search' || el.getAttribute('role') === 'searchbox') ? 'search' : null;
    if (f.getAttribute('role') === 'search' || f.querySelector('input[type=search]')) return 'search';
    if (f.querySelector('input[type=password]')) return 'login';
    const fields = f.querySelectorAll('input:not([type=hidden]):not([type=submit]):not([type=button]),textarea,select');
    if (fields.length === 1 && /(^|[^a-z])(q|query|search|keywords?)([^a-z]|$)/i.test(
        (fields[0].name || '') + ' ' + (fields[0].id || ''))) return 'search';
    return 'other';
  };
  const els = this.map(el => {
    const r = el.getBoundingClientRect(), secret = isSecret(el);
    let value = null;
    if (el.tagName === 'SELECT') value = el.selectedOptions.length ? clip(el.selectedOptions[0].text, maxValue) : '';
    else if (el.tagName === 'INPUT' && (el.type === 'checkbox' || el.type === 'radio')) value = el.checked ? 'checked' : '';
    else if ('value' in el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA'))
      value = secret ? (el.value ? 'filled' : '') : clip(el.value, maxValue);
    return {role: el.getAttribute('role') || implicit(el), label: label(el), value,
            rect: [r.x, r.y, r.width, r.height], input_type: el.tagName === 'INPUT' ? (el.type || 'text') : null,
            href: el.tagName === 'A' ? el.href : null, form: formKind(el), secret};
  });
  return {els, text: document.body ? clip(document.body.innerText, maxText) : '', title: document.title,
          url: location.href};
}"""


def element(eid: str, frame_id: str, raw: dict[str, Any]) -> PageElement:
    x, y, w, h = (float(v) for v in raw.get("rect", [0, 0, 0, 0]))
    secret = bool(raw.get("secret"))
    value = raw.get("value")
    if secret:
        value = "filled" if value else ""          # belt and braces: the walker already reports only this
    return PageElement(id=eid, frame_id=frame_id, role=str(raw.get("role") or "generic"),
                       label=str(raw.get("label") or "")[:MAX_LABEL], value=value, rect=(x, y, w, h), visible=True,
                       input_type=raw.get("input_type"), href=raw.get("href"), form=raw.get("form"), secret=secret)


def for_model(snap: PageSnapshot) -> dict[str, Any]:
    """What a worker sees. The gateway's redaction pipeline runs over it again (tokens in URLs, known secrets)."""
    els = []
    for e in snap.elements:
        d: dict[str, Any] = {"id": e.id, "role": e.role, "label": e.label}
        if e.secret:                               # password / code field: whether it's filled, never a value
            d["sensitive"], d["filled"] = True, bool(e.value)
        elif e.value not in (None, ""):
            d["value"] = e.value
        if e.input_type and e.input_type not in ("text", "submit", "button"):
            d["type"] = e.input_type
        if e.href:
            d["href"] = e.href
        if e.form in ("search", "login"):
            d["form"] = e.form
        els.append(d)
    out: dict[str, Any] = {"tab": snap.tab.tab_id, "url": snap.url, "title": snap.title, "elements": els,
                           "text": snap.text}
    if snap.truncated:
        out["truncated"] = True
    if snap.dialogs:
        out["dismissed_dialogs"] = snap.dialogs
    return out
