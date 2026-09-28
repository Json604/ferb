// FERB extension protocol (ARCHITECTURE.md 9.4), mirrored by ferb/act/browser/ext_protocol.py.
// Loaded by the service worker (importScripts) and by the node tests (module.exports). No decisions here.
const FERB_PROTOCOL = 1;
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

const OPS = ["open", "navigate", "close", "list_tabs", "snapshot", "act", "wait", "screenshot", "attach_debugger",
             "detach_debugger", "cdp", "account", "wake_close"];

if (typeof module !== "undefined") module.exports = { FERB_PROTOCOL, FERB_HOST, cdpAllowed, cdpFilter, OPS };
