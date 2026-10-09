// Owns protocol versions, sealing and allowed CDP mechanics; must not interpret task or page meaning.
// FERB extension protocol , mirrored by ferb/act/browser/ext_protocol.py.
// Loaded by the service worker (importScripts) and by the node tests (module.exports). No decisions here.
const FERB_PROTOCOL = 10;
const FERB_HOST = "com.ferb.native_host";

// Editing commands the key path may ask the renderer for (CDP Input.dispatchKeyEvent `commands`, macOS only):
// the renderer interprets a shortcut like ⌘A only through these (measured in the browser input probes). Anything
// else in `commands` is refused by the allowlist below.
const EDIT_COMMANDS = new Set(["selectAll", "copy", "cut", "paste", "undo", "redo", "deleteBackward", "deleteForward",
  "deleteWordBackward", "deleteWordForward", "deleteToBeginningOfLine", "deleteToEndOfLine",
  "moveLeft", "moveRight", "moveUp", "moveDown", "moveWordLeft", "moveWordRight",
  "moveToBeginningOfLine", "moveToEndOfLine", "moveToBeginningOfDocument", "moveToEndOfDocument",
  "moveToBeginningOfLineAndModifySelection", "moveToEndOfLineAndModifySelection",
  "moveWordLeftAndModifySelection", "moveWordRightAndModifySelection",
  "moveLeftAndModifySelection", "moveRightAndModifySelection", "moveUpAndModifySelection", "moveDownAndModifySelection",
  "moveToBeginningOfDocumentAndModifySelection", "moveToEndOfDocumentAndModifySelection",
  "insertNewline", "insertTab", "cancelOperation"]);

// Downloads (D35): document responses in a tab FERB drives pause at the response stage, so one that is a download is
// held (never handed to the browser's download flow, so no Save dialog) and read in parts of DOWNLOAD_PART bytes:
// at most 96000 base64 characters, under the 100000-character single-result line (result_parts).
const DOWNLOAD_PART = 72000;
const DOWNLOAD_PATTERNS = [{ urlPattern: "*", resourceType: "Document", requestStage: "Response" }];
const _only = (p, keys) => Object.keys(p).length === keys.length && keys.every((k) => k in p);

// The page hook (D35): FERB's only main-world code, pinned here by its exact source. The tab's own debugger session
// installs it (Page.addScriptToEvaluateOnNewDocument, runImmediately) only in armed task tabs and removes it on release;
// before acting it asks content.js through a cancelable "ferb-armed" event that content.js answers only while FERB
// drives the tab, so a document still patched after a release behaves natively. It catches what no listener sees:
// a save through a download link the page never added (FileSaver.js style), clicked while connected so content.js's
// capture listener takes it; and window.open(url) for an http(s) URL in a new tab or window, opened as
// about:blank#ferb-<token> (the page keeps its real window and opener) so FERB can claim exactly that tab and arm it;
// the hook then loads the URL itself, once: when content.js says the popup is ready, or after 1.5 s, and only while the
// popup is still exactly at that blank URL (a page that navigated or closed its popup meanwhile keeps its own choice).
// FERB never navigates it. A page that fakes "ferb-popup-ready" only loads its own popup early. It never masks itself.
function ferbPageHook() {
  const MARK = Symbol.for("ferb.page-hook");
  if (window[MARK]) return;
  Object.defineProperty(window, MARK, { value: true });
  const dispatch = EventTarget.prototype.dispatchEvent, click = HTMLElement.prototype.click, open = window.open;
  const armed = () => !dispatch.call(document, new CustomEvent("ferb-armed", { cancelable: true }));
  const unseenSave = (el) => (el instanceof HTMLAnchorElement || el instanceof HTMLAreaElement) && !el.isConnected &&
    el.hasAttribute("download") && /^(blob|data):/i.test(String(el.href || ""));
  // In the document for one click() (cancelable, unlike a page's `new MouseEvent("click")`, so content.js can cancel it).
  const viaDocument = (el) => {
    document.documentElement.appendChild(el);
    try { click.call(el); } finally { el.remove(); }
  };
  HTMLElement.prototype.click = function () {
    if (unseenSave(this) && document.documentElement && armed()) return viaDocument(this);
    return click.call(this);
  };
  EventTarget.prototype.dispatchEvent = function (event) {
    if (event && event.type === "click" && unseenSave(this) && document.documentElement && armed()) {
      viaDocument(this);
      return true;
    }
    return dispatch.call(this, event);
  };
  window.open = function (url, target, features) {
    const raw = url === undefined || url === null ? "" : String(url);
    const name = target === undefined || target === null ? "" : String(target).toLowerCase();
    let href = "";
    if (raw.trim()) { try { href = new URL(raw, document.baseURI).href; } catch (_) { href = ""; } }
    if (!/^https?:/i.test(href) || !["", "_blank"].includes(name) || /noopener|noreferrer/i.test(String(features || "")) ||
        !armed()) return open.apply(window, arguments);
    const bytes = new Uint8Array(16);
    crypto.getRandomValues(bytes);
    const token = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
    const blank = "about:blank#ferb-" + token;
    const popup = open.call(window, blank, "_blank", features);
    if (!popup) return popup;
    let loaded = false;
    const load = () => {
      if (loaded) return;
      loaded = true;
      document.removeEventListener("ferb-popup-ready", onReady);
      let at = "";
      try { if (popup.closed) return; at = String(popup.location.href); } catch (_) { return; }   // closed, or another origin's now
      if (at !== blank) return;                                        // the page sent it somewhere itself: its URL stands
      try { popup.location.href = href; } catch (_) { /* closed meanwhile */ }
    };
    const onReady = (e) => { if (e.detail === token) load(); };
    document.addEventListener("ferb-popup-ready", onReady);
    dispatch.call(document, new CustomEvent("ferb-popup", { detail: JSON.stringify({ token, url: href }) }));
    setTimeout(load, 1500);
    return popup;
  };
}
const PAGE_HOOK = `(${ferbPageHook.toString()})();`;

// The exact CDP allowlist, enforced here even if the daemon asks for more. Each entry checks the params
// shape; anything not listed is refused: cookies/storage, other targets, free-form script evaluation.
// Every method is used by background.js; the comment states its mechanical purpose.
const CDP_ALLOW = {
  // the per-tab session setup, in this order, right after chrome.debugger.attach.
  "Page.enable": (p) => Object.keys(p).length === 0,                      // dialog events
  "Page.setInterceptFileChooserDialog": (p) => p.enabled === true && Object.keys(p).length === 1,
  "Emulation.setFocusEmulationEnabled": (p) => typeof p.enabled === "boolean",   // input lands only with it
  "Target.setAutoAttach": (p) => p.autoAttach === true && p.waitForDebuggerOnStart === false && p.flatten === true
    && Object.keys(p).length === 3,                                         // OOPIFs through the tab session
  // Who opened a claimed popup (D35): this session's own target only (no targetId: another target's is refused by
  // Chrome too, "Not allowed", measured); the return is cut to its own id and its opener's. No other Target.* call.
  "Target.getTargetInfo": (p) => Object.keys(p).length === 0,
  "Emulation.setDeviceMetricsOverride": (p) => p.width === 1280 && p.height === 900 && p.deviceScaleFactor === 1
    && p.mobile === false && Object.keys(p).length === 4,                   // layout independent of the window
  // trusted input. Coordinates are CSS px of the top viewport.
  "Input.dispatchMouseEvent": (p) => ["mouseMoved", "mousePressed", "mouseReleased", "mouseWheel"].includes(p.type)
    && Number.isFinite(p.x) && Number.isFinite(p.y)
    && (p.buttons === undefined || (Number.isInteger(p.buttons) && p.buttons >= 0 && p.buttons <= 31))
    && (p.button === undefined || ["none", "left", "middle", "right"].includes(p.button))
    && (p.modifiers === undefined || (Number.isInteger(p.modifiers) && p.modifiers >= 0 && p.modifiers <= 15))
    && (p.clickCount === undefined || [0, 1, 2, 3].includes(p.clickCount))
    && (p.deltaX === undefined || Number.isFinite(p.deltaX)) && (p.deltaY === undefined || Number.isFinite(p.deltaY)),
  "Input.dispatchKeyEvent": (p) => ["keyDown", "keyUp", "rawKeyDown", "char"].includes(p.type)
    && (p.modifiers === undefined || (Number.isInteger(p.modifiers) && p.modifiers >= 0 && p.modifiers <= 15))
    && (p.commands === undefined || (Array.isArray(p.commands) && p.commands.every((c) => EDIT_COMMANDS.has(c)))),
  "Input.insertText": (p) => typeof p.text === "string",
  // Dialog responses require an explicit agent decision.
  "Page.handleJavaScriptDialog": (p) => typeof p.accept === "boolean"
    && (p.promptText === undefined || typeof p.promptText === "string"),
  // screenshot on request; jpeg so it fits the 1 MB native-messaging line.
  "Page.captureScreenshot": (p) => (!p.format || ["png", "jpeg"].includes(p.format))
    && (p.quality === undefined || (Number.isInteger(p.quality) && p.quality >= 1 && p.quality <= 100)),
  // the AX fallback for canvas / ARIA pages, with rects per node.
  "Accessibility.getFullAXTree": (p) => (p.depth === undefined || (Number.isInteger(p.depth) && p.depth > 0))
    && (p.frameId === undefined || typeof p.frameId === "string") && Object.keys(p).every(k => ["depth", "frameId"].includes(k)),
  "Page.getFrameTree": (p) => Object.keys(p).length === 0,
  "DOM.getFrameOwner": (p) => typeof p.frameId === "string" && Object.keys(p).length === 1,
  "DOM.describeNode": (p) => Number.isInteger(p.backendNodeId) && [0, 1].includes(p.depth) && Object.keys(p).length === 2,
  "DOM.pushNodesByBackendIdsToFrontend": (p) => Array.isArray(p.backendNodeIds) && p.backendNodeIds.length === 1
    && Number.isInteger(p.backendNodeIds[0]) && Object.keys(p).length === 1,
  "DOM.setAttributeValue": (p) => Number.isInteger(p.nodeId) && p.name === "data-ferb-ax"
    && /^[a-f0-9]{16}$/.test(p.value) && Object.keys(p).length === 3,
  "DOM.getBoxModel": (p) => Number.isInteger(p.backendNodeId) && Object.keys(p).length === 1,
  // file uploads on a FERB-stamped element. depth 0 = the document node only, never the DOM.
  "DOM.getDocument": (p) => p.depth === 0 && Object.keys(p).length === 1,
  "DOM.querySelector": (p) => Number.isInteger(p.nodeId) && /^\[data-ferb-node="[a-f0-9]{16}"\]$/.test(p.selector),
  "DOM.performSearch": p => Object.keys(p).length === 1 && /^\[data-ferb-node="[a-f0-9]{16}"\]$/.test(p.query),
  "DOM.getSearchResults": p => typeof p.searchId === "string" && p.fromIndex === 0 && p.toIndex === 1 && Object.keys(p).length === 3,
  "DOM.discardSearchResults": p => typeof p.searchId === "string" && Object.keys(p).length === 1,
  "DOM.setFileInputFiles": (p) => Number.isInteger(p.nodeId) && Array.isArray(p.files)
    && p.files.every((f) => typeof f === "string" && f.startsWith("/")),
  // downloads (D35), tab-scoped: never Browser/Page.setDownloadBehavior (browser-context-wide; and refused to
  // extensions: measured). A held response is only read and then failed, never rewritten or redirected.
  "Fetch.enable": (p) => JSON.stringify(p) === JSON.stringify({ patterns: DOWNLOAD_PATTERNS }),
  "Fetch.continueRequest": (p) => typeof p.requestId === "string" && _only(p, ["requestId"]),
  "Fetch.failRequest": (p) => typeof p.requestId === "string" && p.errorReason === "Aborted" && _only(p, ["requestId", "errorReason"]),
  "Fetch.takeResponseBodyAsStream": (p) => typeof p.requestId === "string" && _only(p, ["requestId"]),
  "IO.read": (p) => typeof p.handle === "string" && p.size === DOWNLOAD_PART && _only(p, ["handle", "size"]),
  "IO.close": (p) => typeof p.handle === "string" && _only(p, ["handle"]),
  // the page hook, exactly that source; no general script evaluation.
  "Page.addScriptToEvaluateOnNewDocument": (p) => p.source === PAGE_HOOK && p.runImmediately === true &&
    _only(p, ["source", "runImmediately"]),
  "Page.removeScriptToEvaluateOnNewDocument": (p) => typeof p.identifier === "string" && _only(p, ["identifier"]),
};
const CDP_RETURN_FIELDS = {                 // filtered return fields (only these keys leave the sendCommand wrapper)
  "Page.captureScreenshot": ["data"],
  "Accessibility.getFullAXTree": ["nodes"],
  "DOM.getBoxModel": ["model"],
  "Page.getFrameTree": ["frameTree"],
  "DOM.getFrameOwner": ["backendNodeId"],
  "DOM.describeNode": ["node"],
  "DOM.pushNodesByBackendIdsToFrontend": ["nodeIds"],
  "DOM.getDocument": ["root"],
  "DOM.querySelector": ["nodeId"],
  "DOM.performSearch": ["searchId", "resultCount"],
  "DOM.getSearchResults": ["nodeIds"],
  "Fetch.takeResponseBodyAsStream": ["stream"],
  "IO.read": ["data", "base64Encoded", "eof"],
  "Page.addScriptToEvaluateOnNewDocument": ["identifier"],
  "Target.getTargetInfo": { targetInfo: ["targetId", "openerId"] },   // never its URL or title
};

function cdpAllowed(method, params) {
  const check = Object.prototype.hasOwnProperty.call(CDP_ALLOW, method) ? CDP_ALLOW[method] : null;
  if (!check) return false;
  try { return check(params || {}) === true; } catch (_) { return false; }
}

function cdpFilter(method, result) {
  const keep = CDP_RETURN_FIELDS[method];
  if (!keep) return {};
  const pick = (obj, keys) => {
    const out = {};
    for (const k of keys) if (obj && typeof obj === "object" && k in obj) out[k] = obj[k];
    return out;
  };
  if (Array.isArray(keep)) return pick(result, keep);
  const out = {};                                   // {field: [subfields]}: an object field cut to those keys
  for (const k of Object.keys(keep)) if (result && k in result) out[k] = pick(result[k], keep[k]);
  return out;
}

// ---------- keys: a press like "Enter", "cmd+a", "shift+Tab" -> Input.dispatchKeyEvent params ----------
// Modifiers bitmask: Alt 1, Ctrl 2, Meta/Cmd 4, Shift 8 (CDP Input domain). Printables carry `text` so the
// renderer inserts the character; mac editing shortcuts carry `commands` (probe S7: ⌘A selects only with it).
const MOD_BITS = { alt: 1, opt: 1, option: 1, ctrl: 2, control: 2, meta: 4, cmd: 4, command: 4, shift: 8 };
const NAMED_KEYS = {   // key -> [code, windowsVirtualKeyCode, text]
  Enter: ["Enter", 13, "\r"], Tab: ["Tab", 9], Escape: ["Escape", 27], Backspace: ["Backspace", 8],
  Delete: ["Delete", 46], Space: ["Space", 32, " "], ArrowDown: ["ArrowDown", 40], ArrowUp: ["ArrowUp", 38],
  ArrowLeft: ["ArrowLeft", 37], ArrowRight: ["ArrowRight", 39], Home: ["Home", 36], End: ["End", 35],
  PageUp: ["PageUp", 33], PageDown: ["PageDown", 34], Insert: ["Insert", 45],
  F1: ["F1", 112], F2: ["F2", 113], F3: ["F3", 114], F4: ["F4", 115], F5: ["F5", 116], F6: ["F6", 117],
  F7: ["F7", 118], F8: ["F8", 119], F9: ["F9", 120], F10: ["F10", 121], F11: ["F11", 122], F12: ["F12", 123],
};
const KEY_ALIASES = { enter: "Enter", return: "Enter", tab: "Tab", esc: "Escape", escape: "Escape",
  backspace: "Backspace", delete: "Delete", del: "Delete", space: "Space", spacebar: "Space",
  down: "ArrowDown", up: "ArrowUp", left: "ArrowLeft", right: "ArrowRight", arrowdown: "ArrowDown",
  arrowup: "ArrowUp", arrowleft: "ArrowLeft", arrowright: "ArrowRight", home: "Home", end: "End",
  pageup: "PageUp", pagedown: "PageDown", insert: "Insert" };
const PUNCT = { "-": ["Minus", 189], "=": ["Equal", 187], ",": ["Comma", 188], ".": ["Period", 190],
  "/": ["Slash", 191], ";": ["Semicolon", 186], "'": ["Quote", 222], "[": ["BracketLeft", 219],
  "]": ["BracketRight", 221], "\\": ["Backslash", 220], "`": ["Backquote", 192] };
const SHIFTED = { "!": "1", "@": "2", "#": "3", "$": "4", "%": "5", "^": "6", "&": "7", "*": "8", "(": "9", ")": "0",
  "_": "-", "+": "=", "{": "[", "}": "]", "|": "\\", ":": ";", '"': "'", "<": ",", ">": ".", "?": "/", "~": "`" };
// mac editing commands by "modifiers+key" (Blink editor command names; matched case-insensitively by Blink).
const MAC_COMMANDS = {
  "4+a": ["selectAll"], "4+c": ["copy"], "4+x": ["cut"], "4+v": ["paste"], "4+z": ["undo"], "12+z": ["redo"],
  "0+Backspace": ["deleteBackward"], "0+Delete": ["deleteForward"], "1+Backspace": ["deleteWordBackward"],
  "1+Delete": ["deleteWordForward"], "4+Backspace": ["deleteToBeginningOfLine"], "4+Delete": ["deleteToEndOfLine"],
  "0+ArrowLeft": ["moveLeft"], "0+ArrowRight": ["moveRight"], "0+ArrowUp": ["moveUp"], "0+ArrowDown": ["moveDown"],
  "1+ArrowLeft": ["moveWordLeft"], "1+ArrowRight": ["moveWordRight"],
  "4+ArrowLeft": ["moveToBeginningOfLine"], "4+ArrowRight": ["moveToEndOfLine"],
  "4+ArrowUp": ["moveToBeginningOfDocument"], "4+ArrowDown": ["moveToEndOfDocument"],
  "8+ArrowLeft": ["moveLeftAndModifySelection"], "8+ArrowRight": ["moveRightAndModifySelection"],
  "8+ArrowUp": ["moveUpAndModifySelection"], "8+ArrowDown": ["moveDownAndModifySelection"],
  "9+ArrowLeft": ["moveWordLeftAndModifySelection"], "9+ArrowRight": ["moveWordRightAndModifySelection"],
  "12+ArrowLeft": ["moveToBeginningOfLineAndModifySelection"], "12+ArrowRight": ["moveToEndOfLineAndModifySelection"],
  "12+ArrowUp": ["moveToBeginningOfDocumentAndModifySelection"], "12+ArrowDown": ["moveToEndOfDocumentAndModifySelection"],
  "0+Escape": ["cancelOperation"],
};

// "cmd+shift+a" -> { key, code, keyCode, text, modifiers, commands } or null when the key isn't known.
function keyEvent(spec) {
  if (spec === " ") spec = "Space";
  const parts = String(spec || "").trim().split("+").map((s) => s.trim()).filter(Boolean);
  if (!parts.length) return null;
  let modifiers = 0, raw = parts[parts.length - 1];
  for (const m of parts.slice(0, -1)) {
    const bit = MOD_BITS[m.toLowerCase()];
    if (!bit) return null;
    modifiers |= bit;
  }
  let key, code, keyCode, text;
  const named = NAMED_KEYS[raw] ? raw : KEY_ALIASES[raw.toLowerCase()];
  if (named) {
    [code, keyCode, text] = NAMED_KEYS[named];
    key = named === "Space" ? " " : named;
  } else if (raw.length === 1) {
    let ch = raw;
    if (SHIFTED[ch]) { modifiers |= 8; ch = SHIFTED[ch]; }
    else if (/[A-Z]/.test(ch)) { modifiers |= 8; ch = ch.toLowerCase(); }
    if (/[a-z]/.test(ch)) { code = "Key" + ch.toUpperCase(); keyCode = ch.toUpperCase().charCodeAt(0); }
    else if (/[0-9]/.test(ch)) { code = "Digit" + ch; keyCode = ch.charCodeAt(0); }
    else if (PUNCT[ch]) [code, keyCode] = PUNCT[ch];
    else if (ch === " ") { code = "Space"; keyCode = 32; }
    else return null;
    key = modifiers & 8 && /[a-z]/.test(ch) ? ch.toUpperCase() : raw;
    text = (modifiers & ~8) ? undefined : raw;           // ⌘/⌃/⌥ + key inserts nothing
  } else return null;
  if (text !== undefined && (modifiers & ~8)) text = undefined;
  const commands = MAC_COMMANDS[`${modifiers}+${named || key.toLowerCase()}`] || MAC_COMMANDS[`${modifiers}+${named || key}`];
  return { key, code, keyCode, text, modifiers, commands: commands ? commands.slice() : undefined };
}

// Sealed secrets: AES-GCM, key = HKDF-SHA256(pairing key, label "ferb seal v1"), AAD "<command id>|<origin>".
// Opening fails (throws) for any other command id or origin: a sealed value can't be replayed elsewhere.
const _b64d = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
async function sealKey(pairingKeyB64) {
  const base = await crypto.subtle.importKey("raw", _b64d(pairingKeyB64), "HKDF", false, ["deriveKey"]);
  return crypto.subtle.deriveKey({ name: "HKDF", hash: "SHA-256", salt: new Uint8Array(0),
                                   info: new TextEncoder().encode("ferb seal v1") },
                                 base, { name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
}
function _b64e(buf) {
  // Bound each call's argument count, never the sealed source's length.
  const bytes = new Uint8Array(buf), chunks = [];
  for (let offset = 0; offset < bytes.length; offset += 32768)
    chunks.push(String.fromCharCode(...bytes.subarray(offset, offset + 32768)));
  return btoa(chunks.join(""));
}
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

const OPS = ["open", "navigate", "close", "list_tabs", "snapshot", "act", "wait", "screenshot", "screenshot_part", "record_frame",
             "wake_close", "frame_origin", "fill", "read_source", "window_password", "scroll", "find",
             "upload", "result_part", "command_part", "dialog", "ping", "point", "pointer", "pointer_cancel", "release_tab",
             "download_part", "disarm", "open_plain", "popup_ready"];

if (typeof module !== "undefined") module.exports = { FERB_PROTOCOL, FERB_HOST, cdpAllowed, cdpFilter, OPS, seal, unseal,
                                                      FEED_URL, keyEvent, EDIT_COMMANDS, DOWNLOAD_PART, DOWNLOAD_PATTERNS,
                                                      PAGE_HOOK, ferbPageHook };
