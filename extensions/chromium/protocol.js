// Owns protocol versions, sealing and allowed CDP mechanics; must not interpret task or page meaning.
// FERB extension protocol , mirrored by ferb/act/browser/ext_protocol.py.
// Loaded by the service worker (importScripts) and by the node tests (module.exports). No decisions here.
// Protocol 18 (extension 0.16.0): daemon messages signed (DAEMON_SIGNED), tab groups and connection-independent tab
// records (adopt_tab, hello `tabs`, `taken`), card fields secret, read errors known failures (READ_OPS).
// Protocol 19 (extension 0.17.0, HAND-207): FERB's tabs muted (`open`/`adopt_tab` `muted`, `mute_tab`), sound
// back when a tab becomes the owner's, and `sound` tab events.
// Protocol 20 (extension 0.18.0, HAND-210): download parts of 512 KiB sent whole, `popup_refused` (a popup past
// the task's tab cap is closed while blank), and every dropped page save reported (`count`).
// Protocol 21 (extension 0.19.0, HAND-213): a `sound` tab event says who last changed the tab's mute
// (`muted_reason`, the browser's mutedInfo.reason: user / capture / extension), so the owner's own mute is never a
// disturbance fact.
// Extension 0.19.3 keeps protocol 21 (FERB refuses any other number, so a bump would stop every installed FERB until it
// updates): it only adds `pdf` and `pdf_part` (Page.printToPDF), which an older FERB never sends.
// Extension 0.19.6 keeps protocol 21 and the minimum (HAND-434): tabs kept awake (focus emulation), scripts in at once,
// the accessibility pass batched, a timed-out command given up, and optional reply fields (`steps`, `loading`, a
// failure's `step`) plus `looks: false` on captures, which an older FERB never sends and never reads.
// Extension 0.19.7 keeps protocol 21 and the minimum (HAND-442..445): a JavaScript dialog is a fact at once (a command
// in a tab with one pending, or one opening during it, ends with `dialog`, never a wait), `navigate` and `close` take
// the worker's `leave_page` (leave or stay), `wait` until url looks at the address's origin and path, and a tab the
// task's own input opened without an opener link is the task's (`popup`, confirmed by the browser's openerId). An
// older FERB never sends `leave_page` and reads the new reply fields as before.
// Extension 0.19.9 keeps protocol 21 and the minimum (HAND-501): the hello says whether the owner allowed FERB access to
// file URLs (`file_access`), and an upload without it hands the file's bytes through the page (`upload_part`, then
// `upload` with `transfer`), onto a file field or as a drop. An older FERB never sends either and reads `file_access`
// as nothing.
// Extension 0.19.10 keeps protocol 21 and the minimum (HAND-504..): a type or press on a field the page covers with its
// own drawing or keeps outside the page focuses it by script and sends the keys (`focus`, `unpointed` in the reply);
// type takes `mode` (replace | insert); `act` kind `paste` dispatches the page's own paste event (text and html), never
// touching the system clipboard. Also optional only (ext_protocol.py lists each): picker fields set by value, scroll
// left/right/screens/to, popup windows claimed, `closed_by_page` and `blank_tab`, print answered by the hook, a
// snapshot's `content_type`, click `modifiers`, and secret fields by structure only (HAND-516).
const FERB_PROTOCOL = 21;
const FERB_HOST = "com.ferb.native_host";

// Reads: commands that never change the page. One failing (a timeout, a frame gone, a page the browser won't let FERB
// read) is a known failure, never "outcome unknown" (P19 is for consequential actions). `dialog` without a decision
// only reads too (background.js `outcomeOf`). Mirrored by ext_protocol.READS.
const READ_OPS = new Set(["list_tabs", "snapshot", "wait", "screenshot", "screenshot_part", "record_frame",
  "frame_origin", "read_source", "window_secrets", "find", "result_part", "ping", "point", "pdf", "pdf_part", "html", "owner_view",
  "upload_part"]);
// Daemon messages that carry an HMAC (pairing key, this connection's hello nonce, a strictly increasing counter):
// the extension acts on no other kind of daemon message (refused / pairing_required / paired change no page).
const DAEMON_SIGNED = new Set(["welcome", "cmd"]);
const daemonMacText = (nonce, seq, type, body) => `${nonce}|${seq}|${type}|${body}`;

// Per service-worker start, never fixed: the page hook's event names and the attribute that links a snapshot's
// elements to the debugger's nodes for one snapshot (removed right after), so a page can't look for a FERB marker by
// name (ferb-armed, data-ferb-ax: protocol 17). Bounded hex only.
const _hex = (n) => Array.from(crypto.getRandomValues(new Uint8Array(n)), (b) => b.toString(16).padStart(2, "0")).join("");
const FERB_TOKEN = _hex(6);
const BIND_ATTR = `data-${FERB_TOKEN}`;            // a snapshot's element binding (content.js bind, ax.js)
const STAMP_ATTR = `data-${FERB_TOKEN}s`;          // an upload's file-field stamp (content.js stamp)

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
// 512 KiB, about 700,000 base64 characters, sent whole (background.js never splits a download_part reply into result
// parts), under the native host's 1 MiB message (Chrome takes up to 64 MiB from the extension to the host). Ruling (step
// 12e): 72,000-byte parts, one round trip each, held a browser download to a few GB under the old 10-minute ceiling.
const DOWNLOAD_PART = 512 * 1024;
// Uploads without file access (0.19.9, HAND-501): the daemon sends each file in parts of at most UPLOAD_PART bytes
// (base64, about 350,000 characters), so one part stays one native message, under the daemon's 500,000-character
// line where it would split a command (ext_bridge.Bridge.call) and the host's 1 MB limit towards the extension.
const UPLOAD_PART = 256 * 1024;
const DOWNLOAD_PATTERNS = [{ urlPattern: "*", resourceType: "Document", requestStage: "Response" }];
const _only = (p, keys) => Object.keys(p).length === keys.length && keys.every((k) => k in p);

// The page hook (D35): FERB's only main-world code, pinned here by its exact source (with this worker's FERB_TOKEN). The tab's own debugger session
// installs it (Page.addScriptToEvaluateOnNewDocument, runImmediately) only in armed task tabs and removes it on release;
// before acting it asks content.js through a cancelable "<token>:armed" event that content.js answers only while FERB
// drives the tab, so a document still patched after a release behaves natively. It catches what no listener sees:
// a save through a download link the page never added (FileSaver.js style), clicked while connected so content.js's
// capture listener takes it; and window.open(url) for an http(s) URL in a new tab or window, opened as
// about:blank#ferb-<token> (the page keeps its real window and opener) so FERB can claim exactly that tab and arm it;
// the hook then loads the URL itself, once: when content.js says the popup is ready, or after 1.5 s, and only while the
// popup is still exactly at that blank URL (a page that navigated or closed its popup meanwhile keeps its own choice).
// FERB never navigates it. A page that fakes "<token>:ready" only loads its own popup early. It never masks itself:
// the patched functions are visible as such (toString) in a task tab while FERB drives it (a documented residual).
function ferbPageHook(token) {
  const MARK = Symbol.for(token);
  if (window[MARK]) return;
  Object.defineProperty(window, MARK, { value: true });
  // Extension 0.19.7 (HAND-442): the worker said to leave this page (navigate or close with leave_page "leave"), so
  // its own beforeunload listeners are skipped for that leave and Chrome raises no "Leave site?" dialog, for which it
  // activates the tab (measured). Registered at document start, before the page's own listeners (listeners on the
  // window run in the order they were added, capture or not: measured), and armed only by the service worker's
  // "<token>:leave" event for the next 10 s. A page that fakes the event only skips its own prompt.
  let leaveUntil = 0;
  if (typeof window.addEventListener === "function" && typeof document.addEventListener === "function") {
    window.addEventListener("beforeunload", (e) => { if (Date.now() < leaveUntil) e.stopImmediatePropagation(); }, true);
    document.addEventListener(token + ":leave", () => { leaveUntil = Date.now() + 10000; });
  }
  const dispatch = EventTarget.prototype.dispatchEvent, click = HTMLElement.prototype.click, open = window.open;
  // Extension 0.19.10 (HAND-511): the page closing its own window (a sign-in window once it is done) is told to
  // content.js first ("<token>:closing"), so FERB never reads that close as the owner's. The close itself is the page's.
  // Extension 0.19.10 (HAND-512): window.print() in a tab FERB drives would open the browser's print preview, which
  // stops the page (and FERB's act on it) until someone answers it. FERB answers it here: no preview, and the page goes
  // on as after a cancelled print; content.js is told ("<token>:print"), so the act's result says the page asked.
  const print = window.print;
  if (typeof print === "function") window.print = function () {
    if (!armed()) return print.apply(this, arguments);
    dispatch.call(document, new CustomEvent(token + ":print"));
  };
  const close = window.close;
  if (typeof close === "function") window.close = function () {
    try { dispatch.call(document, new CustomEvent(token + ":closing")); } catch (_) { /* told or not, it closes */ }
    return close.apply(this, arguments);
  };
  const armed = () => !dispatch.call(document, new CustomEvent(token + ":armed", { cancelable: true }));
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
    const tag = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
    const blank = "about:blank#ferb-" + tag;
    const popup = open.call(window, blank, "_blank", features);
    if (!popup) return popup;
    let loaded = false;
    const load = () => {
      if (loaded) return;
      loaded = true;
      document.removeEventListener(token + ":ready", onReady);
      let at = "";
      try { if (popup.closed) return; at = String(popup.location.href); } catch (_) { return; }   // closed, or another origin's now
      if (at !== blank) return;                                        // the page sent it somewhere itself: its URL stands
      try { popup.location.href = href; } catch (_) { /* closed meanwhile */ }
    };
    const onReady = (e) => { if (e.detail === tag) load(); };
    document.addEventListener(token + ":ready", onReady);
    dispatch.call(document, new CustomEvent(token + ":popup", { detail: JSON.stringify({ token: tag, url: href }) }));
    setTimeout(load, 1500);
    return popup;
  };
}
const PAGE_HOOK = `(${ferbPageHook.toString()})(${JSON.stringify(FERB_TOKEN)});`;

// The exact CDP allowlist, enforced here even if the daemon asks for more. Each entry checks the params
// shape; anything not listed is refused: cookies/storage, other targets, free-form script evaluation.
// Every method is used by background.js; the comment states its mechanical purpose.
const CDP_ALLOW = {
  // the per-tab session setup, in this order, right after chrome.debugger.attach.
  "Page.enable": (p) => Object.keys(p).length === 0,                      // dialog events
  // off only while a held tab rests between a loop's rounds (L31): the owner's file picker is theirs again
  "Page.setInterceptFileChooserDialog": (p) => typeof p.enabled === "boolean" && Object.keys(p).length === 1,
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
  // a PDF of the tab (extension 0.19.3, `pdf`): exactly these three settings, nothing that loads or runs anything
  "Page.printToPDF": (p) => p.printBackground === true && p.preferCSSPageSize === true
    && typeof p.landscape === "boolean" && Object.keys(p).length === 3,
  // the AX fallback for canvas / ARIA pages, with rects per node.
  "Accessibility.getFullAXTree": (p) => (p.depth === undefined || (Number.isInteger(p.depth) && p.depth > 0))
    && (p.frameId === undefined || typeof p.frameId === "string") && Object.keys(p).every(k => ["depth", "frameId"].includes(k)),
  "Page.getFrameTree": (p) => Object.keys(p).length === 0,
  "DOM.getFrameOwner": (p) => typeof p.frameId === "string" && Object.keys(p).length === 1,
  "DOM.describeNode": (p) => Number.isInteger(p.backendNodeId) && [0, 1].includes(p.depth) && Object.keys(p).length === 2,
  // the accessibility pass's adoptions, together (0.19.6, ax.js ADOPT_BATCH): node ids only, nothing in the page changes
  "DOM.pushNodesByBackendIdsToFrontend": (p) => Array.isArray(p.backendNodeIds) && p.backendNodeIds.length >= 1
    && p.backendNodeIds.length <= 1000 && p.backendNodeIds.every(Number.isInteger) && Object.keys(p).length === 1,
  "DOM.setAttributeValue": (p) => Number.isInteger(p.nodeId) && p.name === BIND_ATTR
    && /^[a-f0-9]{16}$/.test(p.value) && Object.keys(p).length === 3,
  "DOM.getBoxModel": (p) => Number.isInteger(p.backendNodeId) && Object.keys(p).length === 1,
  // file uploads on a FERB-stamped element. depth 0 = the document node only, never the DOM.
  "DOM.getDocument": (p) => p.depth === 0 && Object.keys(p).length === 1,
  "DOM.querySelector": (p) => Number.isInteger(p.nodeId) && _stampQuery(p.selector),
  "DOM.performSearch": p => Object.keys(p).length === 1 && _stampQuery(p.query),
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
function _stampQuery(q) {                    // exactly one upload stamp: [<STAMP_ATTR>="<16 hex>"]
  const m = /^\[([a-z0-9-]+)="([a-f0-9]{16})"\]$/.exec(String(q));
  return !!m && m[1] === STAMP_ATTR;
}
const CDP_RETURN_FIELDS = {                 // filtered return fields (only these keys leave the sendCommand wrapper)
  "Page.captureScreenshot": ["data"],
  "Page.printToPDF": ["data"],
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
             "wake_close", "frame_origin", "fill", "read_source", "window_secrets", "scroll", "find",
             "upload", "result_part", "command_part", "dialog", "ping", "point", "pointer", "pointer_cancel", "release_tab",
             "download_part", "disarm", "open_plain", "popup_ready", "open_seen", "media", "rest", "show_tab",
             "adopt_tab", "mute_tab", "popup_refused", "pdf", "pdf_part", "html", "owner_view", "upload_part"];

// browser.wait until "url" (HAND-444): the value is looked for in the page's own address, its origin and path (decoded),
// never in its query or fragment, so a search page whose query names a site (Gmail's #search/from%3Ajoinef.com) isn't
// that site. A value that itself holds "?" or "#" asks about the query or fragment: the whole address is searched.
function urlHas(url, value) {
  const v = String(value || ""), u = String(url || "");
  if (!v) return true;
  const decoded = (s) => { try { return decodeURIComponent(s); } catch (_) { return s; } };
  if (/[?#]/.test(v)) return u.includes(v) || decoded(u).includes(v);
  let p;
  try { p = new URL(u); } catch (_) { return u.includes(v); }
  const own = p.origin !== "null" ? p.origin + p.pathname : p.protocol + p.pathname;
  return own.includes(v) || decoded(own).includes(v);
}

// Whether a key press puts text into the focused field (a character without ⌘/⌃/⌥, Space, or a paste): such a press
// into a secret field is refused, as typing is; Return, Tab, Escape, arrows and other shortcuts still go (kb area hands
// rulings, step 5b). Mirrored by ext_bridge.enters_text.
function entersText(spec) {
  const k = keyEvent(spec);
  if (!k) return false;
  return (k.text !== undefined && k.key !== "Enter") || !!(k.commands && k.commands.includes("paste"));
}

if (typeof module !== "undefined") module.exports = { FERB_PROTOCOL, FERB_HOST, cdpAllowed, cdpFilter, OPS, seal, unseal,
                                                      FEED_URL, keyEvent, EDIT_COMMANDS, DOWNLOAD_PART, DOWNLOAD_PATTERNS, UPLOAD_PART,
                                                      PAGE_HOOK, ferbPageHook, READ_OPS, DAEMON_SIGNED, daemonMacText,
                                                      FERB_TOKEN, BIND_ATTR, STAMP_ATTR, entersText, urlHas };
