// FERB extension protocol (ARCHITECTURE.md 9.4), mirrored by ferb/act/browser/ext_protocol.py.
// Loaded by the service worker (importScripts) and by the node tests (module.exports). No decisions here.
const FERB_PROTOCOL = 2;
const FERB_HOST = "com.ferb.native_host";

// The exact CDP allowlist, enforced here even if the daemon asks for more (9.4). Each entry checks the params
// shape; anything not listed is refused: cookies/storage, other targets, free-form script evaluation.
const CDP_ALLOW = {
  "Input.dispatchMouseEvent": (p) => ["mouseMoved", "mousePressed", "mouseReleased", "mouseWheel"].includes(p.type)
    && Number.isFinite(p.x) && Number.isFinite(p.y),
  "Input.dispatchKeyEvent": (p) => ["keyDown", "keyUp", "rawKeyDown", "char"].includes(p.type),
  "Input.insertText": (p) => typeof p.text === "string",
  "Emulation.setFocusEmulationEnabled": (p) => typeof p.enabled === "boolean",
  "Page.captureScreenshot": (p) => !p.format || ["png", "jpeg"].includes(p.format),
  "Page.getFrameTree": () => true,
  "DOM.getDocument": (p) => p.depth === undefined || p.depth <= 2,
  "Accessibility.getFullAXTree": () => true,
};
const CDP_RETURN_FIELDS = {                 // filtered return fields (only these keys reach the daemon)
  "Page.captureScreenshot": ["data"],
  "Page.getFrameTree": ["frameTree"],
  "DOM.getDocument": ["root"],
  "Accessibility.getFullAXTree": ["nodes"],
};

function cdpAllowed(method, params) {
  const check = Object.prototype.hasOwnProperty.call(CDP_ALLOW, method) ? CDP_ALLOW[method] : null;
  if (!check) return false;
  try { return check(params || {}) === true; } catch (_) { return false; }
}

function cdpFilter(method, result) {
  const keep = CDP_RETURN_FIELDS[method];
  if (!keep) return {};
  const out = {};
  for (const k of keep) if (result && k in result) out[k] = result[k];
  return out;
}

// Sealed secrets (10.1): AES-GCM, key = HKDF-SHA256(pairing key, label "ferb seal v1"), AAD "<command id>|<origin>".
// Opening fails (throws) for any other command id or origin: a sealed value can't be replayed elsewhere.
const _b64d = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
async function sealKey(pairingKeyB64) {
  const base = await crypto.subtle.importKey("raw", _b64d(pairingKeyB64), "HKDF", false, ["deriveKey"]);
  return crypto.subtle.deriveKey({ name: "HKDF", hash: "SHA-256", salt: new Uint8Array(0),
                                   info: new TextEncoder().encode("ferb seal v1") },
                                 base, { name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
}
const _b64e = (buf) => btoa(String.fromCharCode(...new Uint8Array(buf)));
async function seal(pairingKeyB64, text, commandId, origin) {     // extension -> daemon (the inbox feed)
  const key = await sealKey(pairingKeyB64), iv = crypto.getRandomValues(new Uint8Array(12));
  const ct = await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: new TextEncoder().encode(`${commandId}|${origin}`) },
                                         key, new TextEncoder().encode(text));
  return { nonce: _b64e(iv), ct: _b64e(ct) };
}
async function unseal(pairingKeyB64, sealed, commandId, origin) {
  const key = await sealKey(pairingKeyB64);
  const plain = await crypto.subtle.decrypt({ name: "AES-GCM", iv: _b64d(sealed.nonce),
                                              additionalData: new TextEncoder().encode(`${commandId}|${origin}`) },
                                            key, _b64d(sealed.ct));
  return new TextDecoder().decode(plain);
}

const FEED_URL = "https://mail.google.com/mail/feed/atom";   // the only page read_source may return

const OPS = ["open", "navigate", "close", "list_tabs", "snapshot", "act", "wait", "screenshot", "attach_debugger",
             "detach_debugger", "cdp", "account", "wake_close", "frame_origin", "fill", "read_source",
             "window_password"];

if (typeof module !== "undefined") module.exports = { FERB_PROTOCOL, FERB_HOST, cdpAllowed, cdpFilter, OPS, seal, unseal, FEED_URL };
