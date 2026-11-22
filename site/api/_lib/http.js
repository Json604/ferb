// Small HTTP helpers for the site's functions: cookies, redirects and pages in the site's own style. Pages carry no
// script and fetch nothing from anyone else (REL-434); every value written into them is escaped.

const SECURITY = {
  "cache-control": "no-store",
  "referrer-policy": "no-referrer",
  "x-content-type-options": "nosniff",
  "x-robots-tag": "noindex",
  "content-security-policy":
    "default-src 'none'; style-src 'self'; img-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
};

export function escapeHtml(value) {
  return String(value ?? "").replace(
    /[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c],
  );
}

export function readCookie(request, name) {
  const header = request.headers.get("cookie") || "";
  for (const part of header.split(";")) {
    const at = part.indexOf("=");
    if (at > 0 && part.slice(0, at).trim() === name) return part.slice(at + 1).trim();
  }
  return null;
}

export function setCookie(name, value, { path, maxAge }) {
  return `${name}=${value}; Path=${path}; Max-Age=${maxAge}; HttpOnly; Secure; SameSite=Lax`;
}

export const clearCookie = (name, path) => setCookie(name, "", { path, maxAge: 0 });

function headers(extra = {}, cookies = []) {
  const h = new Headers({ ...SECURITY, ...extra });
  for (const c of cookies) h.append("set-cookie", c);
  return h;
}

export function redirect(location, { status = 303, cookies = [] } = {}) {
  return new Response(null, { status, headers: headers({ location }, cookies) });
}

export function text(status, body, extra = {}) {
  return new Response(body, { status, headers: headers({ "content-type": "text/plain; charset=utf-8", ...extra }) });
}

// One page in the site's look: the logo, a heading and `body` (already escaped HTML).
export function page(status, { title, body, head = "", cookies = [], wide = false }) {
  const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="theme-color" content="#fafafa">
<meta name="robots" content="noindex">
<title>${escapeHtml(title)} · FERB</title>
<link rel="stylesheet" href="/styles.css">
<link rel="icon" href="/favicon.ico" sizes="32x32">
<link rel="icon" href="/logo.svg" type="image/svg+xml">
${head}
</head>
<body>
<main id="main"${wide ? ' class="wide"' : ""}>
<a href="/"><img class="brand" src="/logo.svg" alt="FERB home" width="48" height="48"></a>
<h1>${escapeHtml(title)}</h1>
${body}
</main>
</body>
</html>
`;
  return new Response(html, { status, headers: headers({ "content-type": "text/html; charset=utf-8" }, cookies) });
}
