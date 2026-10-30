// Owns the one rule for masking secret-looking parts of a URL the extension reports; must not judge a page or a site.
// Loaded by the service worker (importScripts), injected into pages right before content.js, and required by the node
// tests: background.js and content.js share this single source (protocol 18).
(() => {
  if (globalThis.ferbMaskUrl) return;
  const SECRET_PARAM = /token|key|code|secret|session|auth|sig|password|pwd|otp|ticket|nonce|state/i;
  // A query parameter whose name looks secret, a fragment holding parameters and any user:password are masked; ""
  // when it isn't a URL. `base` resolves a relative one (a page's own links).
  function ferbMaskUrl(u, base) {
    try {
      const url = base === undefined ? new URL(u) : new URL(u, base);
      for (const k of [...url.searchParams.keys()]) if (SECRET_PARAM.test(k)) url.searchParams.set(k, "[redacted]");
      if (url.hash.length > 1 && /[=&]/.test(url.hash)) url.hash = "#[redacted]";
      if (url.username || url.password) { url.username = ""; url.password = ""; }
      return url.href;
    } catch (_) { return ""; }
  }
  Object.defineProperty(globalThis, "ferbMaskUrl", { value: ferbMaskUrl });
  if (typeof module !== "undefined") module.exports = { maskUrl: ferbMaskUrl, SECRET_PARAM };
})();
