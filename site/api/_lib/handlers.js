// What the site's functions do (REL-433, REL-434). Every outside service comes in through `deps`, so tests run these
// with stand-ins and no network:
//   deps.config   { secret, adminEmails: [lowercase emails], siteUrl } or null while the site isn't set up
//   deps.now()    Unix seconds
//   deps.oidc     { random(), challenge(verifier), authorizationUrl({ redirectUri, state, nonce, challenge }),
//                   exchange(currentUrl, { verifier, state, nonce }) -> the ID token's claims }
//   deps.store    { recordSignIn({ email, name, at }), recordDownload(email, at), listUsers() }
//   deps.log(message)  never given an email, a name or a token
import {
  adminToken,
  downloadToken,
  signinToken,
  verify,
  ADMIN_SECONDS,
  SIGNIN_SECONDS,
} from "./tokens.js";
import { clearCookie, escapeHtml, page, readCookie, redirect, setCookie, text } from "./http.js";

// Where the release files live (REL-430, REL-432). The update feed and the files Sparkle downloads are redirected by
// vercel.json and never pass through here, so updates never need a sign-in.
export const RELEASES = "https://github.com/Json604/ferb-update/releases/latest/download/";
export const DEFAULT_FILE = "FERB-AppleSilicon.dmg";
const FILE = /^FERB-[A-Za-z0-9]{1,32}\.dmg$/;

const SIGNIN_COOKIE = "ferb_signin";
const SIGNIN_PATH = "/api/auth";
const ADMIN_COOKIE = "ferb_admin";
const ADMIN_PATH = "/api/admin";
const CALLBACK = "/api/auth/callback";

export const validFile = (name) => (typeof name === "string" && FILE.test(name) ? name : null);

export const isAdmin = (config, email) =>
  typeof email === "string" && email !== "" && config.adminEmails.includes(email.toLowerCase());

export function parseAdminEmails(value) {
  return String(value || "")
    .split(",")
    .map((e) => e.trim().toLowerCase())
    .filter(Boolean);
}

const origin = (deps, request) => deps.config.siteUrl || new URL(request.url).origin;

const notSetUp = () =>
  page(503, { title: "sign in", body: "<p>Sign-in isn't set up on this site yet. Try again soon.</p>" });

const startAgain = (purpose) => (purpose === "admin" ? "/api/admin" : "/#download");

function retryPage(status, message, purpose, cookies = []) {
  return page(status, {
    title: "sign in",
    body: `<p>${message} <a href="${startAgain(purpose)}">Try again</a>.</p>`,
    cookies,
  });
}

// GET /api/auth/google?file=FERB-AppleSilicon.dmg (a download) or ?for=admin (the owner's list).
export async function startSignIn(request, deps) {
  if (!deps.config) return notSetUp();
  const url = new URL(request.url);
  const purpose = url.searchParams.get("for") === "admin" ? "admin" : "download";
  const file = validFile(url.searchParams.get("file")) || DEFAULT_FILE;
  const state = deps.oidc.random();
  const nonce = deps.oidc.random();
  const verifier = deps.oidc.random();
  const challenge = await deps.oidc.challenge(verifier);
  const to = await deps.oidc.authorizationUrl({
    redirectUri: origin(deps, request) + CALLBACK,
    state,
    nonce,
    challenge,
  });
  const cookie = signinToken(deps.config.secret, { state, nonce, verifier, purpose, file }, deps.now());
  return redirect(String(to), {
    cookies: [setCookie(SIGNIN_COOKIE, cookie, { path: SIGNIN_PATH, maxAge: SIGNIN_SECONDS })],
  });
}

// GET /api/auth/callback: Google sends the person back here.
export async function finishSignIn(request, deps) {
  if (!deps.config) return notSetUp();
  const { secret } = deps.config;
  const now = deps.now();
  const url = new URL(request.url);
  const done = clearCookie(SIGNIN_COOKIE, SIGNIN_PATH);
  const signin = verify(secret, readCookie(request, SIGNIN_COOKIE), "signin", now);
  if (!signin) {
    return retryPage(400, "That sign-in took too long, or started in another browser.", "download", [done]);
  }
  if (url.searchParams.has("error")) return retryPage(400, "Sign-in was cancelled.", signin.k, [done]);

  let claims;
  try {
    claims = await deps.oidc.exchange(new URL(CALLBACK + url.search, origin(deps, request)), {
      verifier: signin.v,
      state: signin.s,
      nonce: signin.n,
    });
  } catch (error) {
    deps.log(`sign-in: Google did not confirm the sign-in (${error?.code || error?.name || "error"})`);
    return retryPage(400, "Google didn't confirm the sign-in.", signin.k, [done]);
  }
  const email = typeof claims?.email === "string" ? claims.email.trim().toLowerCase() : "";
  if (!email) {
    return retryPage(400, "Google didn't share an email address for this account.", signin.k, [done]);
  }

  if (signin.k === "admin") {
    if (claims.email_verified !== true || !isAdmin(deps.config, email)) {
      return page(403, { title: "users", body: "<p>This page is only for FERB's owner.</p>", cookies: [done] });
    }
    return redirect(ADMIN_PATH, {
      cookies: [done, setCookie(ADMIN_COOKIE, adminToken(secret, email, now), { path: ADMIN_PATH, maxAge: ADMIN_SECONDS })],
    });
  }

  // A database that can't be reached never stops a download; the person is then simply not counted.
  const name = typeof claims.name === "string" ? [...claims.name.trim()].slice(0, 200).join("").trim() : "";
  try {
    await deps.store.recordSignIn({ email, name, at: new Date(now * 1000) });
  } catch (error) {
    deps.log(`sign-in: the user list could not be written (${error?.code || error?.name || "error"})`);
  }
  const file = validFile(signin.f) || DEFAULT_FILE;
  const link = `/download/${file}?t=${downloadToken(secret, email, file, now)}`;
  return readyPage(link, [done]);
}

function readyPage(link, cookies) {
  const href = escapeHtml(link);
  return page(200, {
    title: "download",
    head: `<meta http-equiv="refresh" content="0;url=${href}">`,
    cookies,
    body: `<p>FERB is downloading. If it doesn't start, use this button.</p>
<a class="download" href="${href}">Download for Mac <span class="format">.dmg</span></a>
<p class="note">Apple Silicon (M1 or later) · macOS 14.2 or later</p>
<ol class="steps">
<li>Open the DMG and drag FERB to Applications.</li>
<li>Open FERB, sign in with Google or Apple, connect the service it thinks with and allow its permissions.</li>
<li>Start with one thing: ask FERB to lend a hand.</li>
</ol>
<details>
<summary>Opening FERB for the first time</summary>
<p>The current build uses FERB’s own signing certificate. If macOS blocks it, go to System Settings → Privacy &amp;
Security and choose Open Anyway for FERB. Then open FERB from Applications.</p>
</details>`,
  });
}

// GET /download/FERB-AppleSilicon.dmg?t=TOKEN (vercel.json rewrites it to /api/download?file=...). Without a valid,
// unexpired link made for this file, the person goes to the download section, where the sign-in is.
export async function download(request, deps) {
  if (!deps.config) return notSetUp();
  const url = new URL(request.url);
  const file = validFile(url.searchParams.get("file") || url.pathname.split("/").pop());
  const now = deps.now();
  const token = verify(deps.config.secret, url.searchParams.get("t"), "download", now);
  if (!file || !token || token.f !== file) return redirect("/#download");
  try {
    await deps.store.recordDownload(token.e, new Date(now * 1000));
  } catch (error) {
    deps.log(`download: the user list could not be written (${error?.code || error?.name || "error"})`);
  }
  return redirect(RELEASES + file, { status: 302 });
}

// GET /api/admin (the list) and /api/admin?format=csv: only for a signed-in Google account named in ADMIN_EMAILS.
export async function admin(request, deps) {
  if (!deps.config) return notSetUp();
  const session = verify(deps.config.secret, readCookie(request, ADMIN_COOKIE), "admin", deps.now());
  if (!session || !isAdmin(deps.config, session.e)) return redirect("/api/auth/google?for=admin");
  let users;
  try {
    users = await deps.store.listUsers();
  } catch (error) {
    deps.log(`admin: the user list could not be read (${error?.code || error?.name || "error"})`);
    return page(503, { title: "users", body: "<p>The user list can't be read just now. Reload in a minute.</p>" });
  }
  if (new URL(request.url).searchParams.get("format") === "csv") {
    return new Response(toCsv(users), {
      status: 200,
      headers: {
        "content-type": "text/csv; charset=utf-8",
        "content-disposition": 'attachment; filename="ferb-users.csv"',
        "cache-control": "no-store",
        "x-content-type-options": "nosniff",
      },
    });
  }
  return page(200, { title: "users", body: adminBody(users, deps.now()), wide: true });
}

const iso = (value) => (value ? new Date(value).toISOString() : "");
const shortTime = (value) => (value ? iso(value).slice(0, 16).replace("T", " ") + " UTC" : "–");
const providersOf = (u) => (Array.isArray(u.providers) ? u.providers.join(" ") : "");
const count = (value) => Number(value) || 0;

// The owner's list (REL-453): each account's provider(s), last sign-in and devices; the download sign-in's own
// columns are no longer shown (downloads need no sign-in, REL-449).
export function counts(users, now) {
  const month = (now - 30 * 24 * 60 * 60) * 1000;
  return {
    people: users.length,
    signedInThisMonth: users.filter((u) => u.last_signin_at && new Date(u.last_signin_at).getTime() >= month).length,
  };
}

const devicesOf = (u) => `${count(u.mac)} · ${count(u.ios)} · ${count(u.android)}`;

function adminBody(users, now) {
  const c = counts(users, now);
  const rows = users
    .map(
      (u) => `<tr><td>${escapeHtml(u.email)}</td><td>${escapeHtml(u.name)}</td><td>${escapeHtml(providersOf(u)) || "–"}</td>` +
        `<td>${shortTime(u.first_signin_at)}</td><td>${shortTime(u.last_signin_at)}</td><td>${devicesOf(u)}</td></tr>`,
    )
    .join("\n");
  return `<p>${c.people} people · ${c.signedInThisMonth} signed in in the last 30 days</p>
<p><a href="/api/admin?format=csv">Download as CSV</a></p>
<div class="users"><table>
<thead><tr><th>email</th><th>name</th><th>provider</th><th>first sign-in</th><th>last sign-in</th><th>Mac · iPhone · Android</th></tr></thead>
<tbody>
${rows}
</tbody>
</table></div>`;
}

// A spreadsheet runs a cell that starts with = + - @ (or a tab or return) as a formula; such cells get a leading '.
export function csvCell(value) {
  let s = String(value ?? "");
  if (/^[=+\-@\t\r]/.test(s)) s = `'${s}`;
  return /[",\r\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

export const CSV_COLUMNS = ["email", "name", "providers", "first_signin_at", "last_signin_at", "mac", "ios", "android"];

export function toCsv(users) {
  const lines = [CSV_COLUMNS.join(",")];
  for (const u of users) {
    lines.push(
      [u.email, u.name, providersOf(u), iso(u.first_signin_at), iso(u.last_signin_at), count(u.mac), count(u.ios), count(u.android)]
        .map(csvCell)
        .join(","),
    );
  }
  return lines.join("\r\n") + "\r\n";
}
