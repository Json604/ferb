// Owns which document responses in a tab FERB drives are downloads, and the names they suggest; must not judge content
// or intent. It mirrors what Chromium itself would do with the response (D35, measured in the headless lab):
// Content-Disposition: attachment; a renderer-started download (<a download>: its request carries no
// Upgrade-Insecure-Requests header, unlike every navigation) and the redirects it follows; or a type a page can't show.
// Everything else renders as a page. Loaded by the service worker (importScripts) and by the node tests.

// Chromium's text types that download instead of rendering (blink MIME registry), and the non-text types it renders.
const UNSUPPORTED_TEXT = new Set(["text/calendar", "text/x-calendar", "text/x-vcalendar", "text/vcalendar", "text/vcard",
  "text/x-vcard", "text/directory", "text/ldif", "text/qif", "text/x-qif", "text/x-csv", "text/x-vcf", "text/rtf",
  "text/comma-separated-values", "text/csv", "text/tab-separated-values", "text/tsv", "text/ofx",
  "text/vnd.sun.j2me.app-descriptor"]);
const RENDERED = new Set(["application/xhtml+xml", "application/xml", "application/json", "application/javascript",
  "application/x-javascript", "application/ecmascript", "application/pdf", "application/rss+xml", "application/atom+xml",
  "multipart/x-mixed-replace", "multipart/related", "message/rfc822", "application/x-mpegurl",
  "application/vnd.apple.mpegurl"]);
const RENDERED_IMAGES = new Set(["image/jpeg", "image/pjpeg", "image/jpg", "image/png", "image/apng", "image/x-png",
  "image/gif", "image/webp", "image/bmp", "image/x-ms-bmp", "image/vnd.microsoft.icon", "image/x-icon",
  "image/x-xbitmap", "image/avif", "image/svg+xml"]);
const REDIRECT_MS = 30000;                       // how long a download's redirect target is expected

function headerMap(list) {
  const out = {};
  for (const h of list || []) out[String(h.name).toLowerCase()] = String(h.value);
  return out;
}
const hasHeader = (headers, name) => Object.keys(headers || {}).some((k) => k.toLowerCase() === name);
const mimeOf = (h) => String(h["content-type"] || "").split(";")[0].trim().toLowerCase();

// A type with no Content-Type is sniffed by Chromium; a misread one becomes a browser download, reported as uncaptured.
function renderable(mime) {
  if (!mime) return true;
  if (mime.startsWith("text/")) return !UNSUPPORTED_TEXT.has(mime);
  if (mime.startsWith("image/")) return RENDERED_IMAGES.has(mime);
  if (mime.startsWith("audio/") || mime.startsWith("video/")) return true;
  return RENDERED.has(mime) || mime.endsWith("+json") || mime.endsWith("+xml");
}

// One paused document response (Fetch.requestPaused) -> "attachment" | "link" | "type", or null to let it render.
// `expected` maps the redirect targets of a download in this tab to an expiry: Chromium restarts a redirected download
// as a new request that looks like a navigation (measured), so its URL is what ties it to the download.
function classify(p, expected, now = Date.now()) {
  const status = p.responseStatusCode, url = p.request.url, h = headerMap(p.responseHeaders);
  for (const [u, until] of expected) if (until < now) expected.delete(u);
  const followed = expected.delete(url);
  const link = followed || !hasHeader(p.request.headers, "upgrade-insecure-requests");
  if (status >= 300 && status < 400) {
    if (link && h.location) { try { expected.set(new URL(h.location, url).href, now + REDIRECT_MS); } catch (_) { /* no target */ } }
    return null;
  }
  if (!(status >= 200 && status < 300)) return null;          // the browser shows its error page, never a download
  if (/^\s*attachment/i.test(h["content-disposition"] || "")) return "attachment";
  if (link) return "link";
  return renderable(mimeOf(h)) ? null : "type";
}

// The name the response suggests (RFC 6266 filename*, then filename), else the URL's last path segment.
function downloadName(responseHeaders, url) {
  const cd = headerMap(responseHeaders)["content-disposition"] || "";
  const clip = (s) => String(s).trim().slice(0, 1024);
  let m = /filename\*\s*=\s*[^']*'[^']*'([^;]+)/i.exec(cd);
  if (m) { try { return clip(decodeURIComponent(m[1].trim().replace(/^"|"$/g, ""))); } catch (_) { /* fall through */ } }
  m = /filename\s*=\s*"((?:[^"\\]|\\.)*)"/i.exec(cd) || /filename\s*=\s*([^;]+)/i.exec(cd);
  if (m && m[1].trim()) return clip(m[1].replace(/\\(.)/g, "$1"));
  try {
    const seg = new URL(url).pathname.split("/").filter(Boolean).pop();
    if (seg) return clip(decodeURIComponent(seg));
  } catch (_) { /* no usable path */ }
  return "download";
}

// A source URL fit for a record: secret-looking parameters masked; a data: URL is its type, never its data.
function downloadSource(url, mask) {
  const s = String(url || "");
  if (/^data:/i.test(s)) return "data:" + (s.slice(5).split(/[;,]/)[0] || "");
  return String(mask(s) || "").slice(0, 4096);
}

// Bytes in one padded base64 string.
const b64Bytes = (s) => (s.length / 4) * 3 - (s.endsWith("==") ? 2 : s.endsWith("=") ? 1 : 0);

if (typeof module !== "undefined") module.exports = { classify, renderable, downloadName, downloadSource, headerMap,
                                                      mimeOf, b64Bytes };
