// The apps' sign-in (REL-440, REL-441) and the phone pairing assertion (PHN-828): FERB.app, the iPhone app and the
// Android app sign in to one FERB account (a row of ferb_users) with Google or Apple. The app opens /api/app/start
// in a browser with its own PKCE challenge and state; the provider answers /api/app/callback; the site hands the app
// a one-time code at an allowlisted address; the app trades it with its verifier at /api/app/token for an access
// token (1 hour) and a refresh token (180 days, REL-445; rotated on each use; a reused one ends the whole sign-in).
//
// Every outside service comes in through `deps`, so tests run these with stand-ins and no network:
//   deps.config   readAppConfig(): { ready, secret, siteUrl, google, apple, assertion: { key, kid } | null,
//                 androidCerts }
//   deps.now()    Unix seconds;  deps.random()  32 random bytes, base64url (43 characters)
//   deps.oidc     Google, as in handlers.js (null when Google isn't set up)
//   deps.apple    appleSignIn() from apple.js (null when Apple isn't set up)
//   deps.store    makeStore()'s app methods
//   deps.log(message)  never given an email, a name, a code or a token
import { createHash, createHmac, timingSafeEqual } from "node:crypto";
import { appleName } from "./apple.js";
import { assertionPayload, signAssertion, ASSERTION_SECONDS } from "./assertion.js";
import { clearCookie, escapeHtml, json, page, readCookie, redirect, setCookie } from "./http.js";
import { sign, verify } from "./tokens.js";

const COOKIE = "ferb_app_signin";
const COOKIE_PATH = "/api/app";
const COOKIE_SECONDS = 10 * 60;
const CALLBACK = "/api/app/callback";
const ANDROID_RETURN = "/app/callback";
const NATIVE_RETURN = "ferb-auth://callback";
const ANDROID_PACKAGE = "com.ferb.phone";

const CODE_SECONDS = 120;
const ACCESS_SECONDS = 60 * 60;
const REFRESH_SECONDS = 180 * 24 * 60 * 60; // owner, 2026-11-24 (REL-445)
// [most requests, per seconds]
export const LIMITS = {
  start: [20, 10 * 60],
  token: [30, 10 * 60],
  assertion: [20, 10 * 60],
};

const PROVIDERS = ["google", "apple"];
const PLATFORMS = ["mac", "ios", "android"];
const APP_STATE = /^[A-Za-z0-9_-]{16,128}$/;
const CHALLENGE = /^[A-Za-z0-9_-]{43}$/;
const VERIFIER = /^[A-Za-z0-9._~-]{43,128}$/; // RFC 7636
const ONE_TIME_CODE = /^[A-Za-z0-9_-]{43}$/;
const DEVICE_ID = /^[A-Za-z0-9_-]{16,64}$/;
const PRINTABLE = /^[\x20-\x7e]+$/;
const REFRESH = /^fr1_[A-Za-z0-9_-]{43}$/;
const BEARER = /^Bearer (fa1_[A-Za-z0-9_-]{43})$/i; // the scheme is case-insensitive (RFC 7235)
const BODY_LIMIT = 16 * 1024;

export const sha256hex = (value) => createHash("sha256").update(value).digest("hex");
export const s256 = (verifier) => createHash("sha256").update(verifier).digest("base64url");

function same(a, b) {
  if (typeof a !== "string" || typeof b !== "string") return false;
  const x = Buffer.from(a);
  const y = Buffer.from(b);
  return x.length === y.length && timingSafeEqual(x, y);
}

const home = (deps, request) => deps.config.siteUrl || new URL(request.url).origin;

// The one address each platform's sign-in may return to (exact string match).
export const returnAddress = (site, platform) => (platform === "android" ? site + ANDROID_RETURN : NATIVE_RETURN);

const at = (seconds) => new Date(seconds * 1000);
const why = (error) => error?.code || error?.name || "error";

// ---- pages (the browser part of the sign-in) ----

const signInPage = (status, message, extra = {}) =>
  page(status, { title: "sign in", body: `<p>${message}</p>`, ...extra });
const unavailablePage = () => signInPage(503, "FERB's sign-in isn't answering right now. Try again in a minute.");

// ---- JSON (the apps' part) ----

const unavailable = () => json(503, { error: "unavailable" });
const invalidRequest = () => json(400, { error: "invalid_request" });
const invalidGrant = () => json(401, { error: "invalid_grant" });
const invalidToken = () => json(401, { error: "invalid_token" }, { "www-authenticate": 'Bearer error="invalid_token"' });
const slowDown = (retryAfter) => json(429, { error: "slow_down" }, { "retry-after": String(retryAfter) });

// ---- rate limits (fixed windows in ferb_app_limits) ----

function clientIp(request) {
  const real = (request.headers.get("x-real-ip") || "").trim();
  if (real) return real;
  return (request.headers.get("x-forwarded-for") || "").split(",")[0].trim() || "unknown";
}

// An IP is only ever stored as an HMAC with AUTH_SECRET.
const ipKey = (deps, request, name) =>
  `ip:${name}:${createHmac("sha256", deps.config.secret).update(`ferb-app-ip/v1.${clientIp(request)}`).digest("hex")}`;

// Counts this request under each [key, [max, seconds]]; the seconds to wait when any is over its limit (and then
// none of them keeps the count), else 0.
async function limit(deps, entries) {
  const now = deps.now();
  let wait = 0;
  for (const [key, [max, seconds]] of entries) {
    const { count, windowStart } = await deps.store.hit(key, at(now), seconds);
    if (count > max) wait = Math.max(wait, Math.max(1, Math.ceil(windowStart.getTime() / 1000 + seconds - now)));
  }
  if (wait) for (const [key] of entries) await deps.store.release(key);
  return wait;
}

// ---- GET /api/app/ready?provider=google|apple ----
// Whether the apps' sign-in with that provider is set up on this site, so an app can say so plainly instead of opening
// a browser on an error page. Says nothing else; no rate limit needed (no state is made).
export function appReady(request, deps) {
  const provider = new URL(request.url).searchParams.get("provider");
  if (!PROVIDERS.includes(provider)) return invalidRequest();
  const ready = Boolean(deps.config?.ready) && Boolean(provider === "google" ? deps.oidc : deps.apple);
  return json(200, { ready });
}

// ---- GET /api/app/start ----

export async function appStart(request, deps) {
  if (!deps.config?.ready) return unavailablePage();
  const site = home(deps, request);
  const q = new URL(request.url).searchParams;
  const one = (name) => (q.getAll(name).length === 1 ? q.get(name) : null);
  const provider = one("provider");
  const platform = one("platform");
  const target = one("redirect");
  const appState = one("state");
  const challenge = one("code_challenge");
  if (
    !PROVIDERS.includes(provider) ||
    !PLATFORMS.includes(platform) ||
    target !== returnAddress(site, platform) ||
    !APP_STATE.test(appState || "") ||
    !CHALLENGE.test(challenge || "") ||
    one("code_challenge_method") !== "S256"
  ) {
    return signInPage(400, "This sign-in link isn't complete. Go back to FERB and try again.");
  }
  if (provider === "google" ? !deps.oidc : !deps.apple) {
    const name = provider === "google" ? "Google" : "Apple";
    return signInPage(503, `Signing in with ${name} isn't ready on FERB's site yet. Try again later.`);
  }
  try {
    const wait = await limit(deps, [[ipKey(deps, request, "start"), LIMITS.start]]);
    if (wait) {
      return signInPage(429, "There have been a lot of sign-ins from here in the last few minutes. Wait a few minutes, then try again from FERB.", {
        extra: { "retry-after": String(wait) },
      });
    }
  } catch (error) {
    deps.log(`app sign-in: the rate limit could not be checked (${why(error)})`);
    return unavailablePage();
  }

  const state = deps.random();
  const nonce = deps.random();
  const verifier = provider === "google" ? deps.random() : undefined;
  let location;
  try {
    location =
      provider === "google"
        ? await deps.oidc.authorizationUrl({ redirectUri: site + CALLBACK, state, nonce, challenge: s256(verifier) })
        : deps.apple.authorizationUrl({ redirectUri: site + CALLBACK, state, nonce });
  } catch (error) {
    deps.log(`app sign-in: ${provider} could not be reached (${why(error)})`);
    return unavailablePage();
  }
  const now = deps.now();
  const cookie = sign(deps.config.secret, {
    p: "appsignin",
    pr: provider,
    pl: platform,
    r: target,
    as: appState,
    c: challenge,
    s: state,
    n: nonce,
    v: verifier,
    x: now + COOKIE_SECONDS,
  });
  return redirect(String(location), {
    cookies: [setCookie(COOKIE, cookie, { path: COOKIE_PATH, maxAge: COOKIE_SECONDS, sameSite: "None" })],
  });
}

// ---- GET (Google) | POST (Apple, form_post) /api/app/callback ----

async function callbackParams(request) {
  if (request.method !== "POST") return new URL(request.url).searchParams;
  const body = await request.text().catch(() => "");
  return new URLSearchParams(body.length <= BODY_LIMIT ? body : "");
}

async function googleIdentity(deps, request, signin, site) {
  const claims = await deps.oidc.exchange(new URL(CALLBACK + new URL(request.url).search, site), {
    verifier: signin.v,
    state: signin.s,
    nonce: signin.n,
  });
  if (typeof claims?.sub !== "string" || !claims.sub) throw Object.assign(new Error("no subject"), { code: "NO_SUBJECT" });
  const verified = claims.email_verified === true;
  return {
    sub: claims.sub,
    email: verified && typeof claims.email === "string" ? claims.email.trim().toLowerCase() : "",
    name: typeof claims.name === "string" ? claims.name.trim().slice(0, 200) : "",
  };
}

async function appleIdentity(deps, params, signin, site) {
  const code = params.get("code");
  if (!code) throw Object.assign(new Error("no code"), { code: "NO_CODE" });
  const claims = await deps.apple.exchange({ code, redirectUri: site + CALLBACK, nonce: signin.n });
  // Private-relay addresses (is_private_email) are ordinary verified addresses here.
  const verified = claims.email_verified === true || claims.email_verified === "true";
  return {
    sub: claims.sub,
    email: verified && typeof claims.email === "string" ? claims.email.trim().toLowerCase() : "",
    name: appleName(params.get("user")),
  };
}

export async function appCallback(request, deps) {
  if (!deps.config?.ready) return unavailablePage();
  const site = home(deps, request);
  const now = deps.now();
  const done = clearCookie(COOKIE, COOKIE_PATH, "None");
  const signin = verify(deps.config.secret, readCookie(request, COOKIE), "appsignin", now);
  if (
    !signin ||
    !PROVIDERS.includes(signin.pr) ||
    !PLATFORMS.includes(signin.pl) ||
    signin.r !== returnAddress(site, signin.pl) ||
    !APP_STATE.test(signin.as || "")
  ) {
    return signInPage(400, "That sign-in took too long, or started somewhere else. Go back to FERB and try again.", {
      cookies: [done],
    });
  }
  const back = (params) =>
    redirect(`${signin.r}?${new URLSearchParams({ ...params, state: signin.as })}`, { cookies: [done] });
  const fail = (reason, error = "failed") => {
    deps.log(`app sign-in: ${reason}`);
    return back({ error });
  };

  const params = await callbackParams(request);
  if ((request.method === "POST") !== (signin.pr === "apple")) return fail(`the answer did not come from ${signin.pr}`);
  if (!same(params.get("state"), signin.s)) return fail(`${signin.pr}'s state did not match`);
  if (params.has("error")) return back({ error: "cancelled" });

  let identity;
  try {
    identity =
      signin.pr === "google"
        ? await googleIdentity(deps, request, signin, site)
        : await appleIdentity(deps, params, signin, site);
  } catch (error) {
    return fail(`${signin.pr} did not confirm the sign-in (${why(error)})`);
  }
  if (!identity.email) return fail(`${signin.pr} gave no verified email`, "no_email");

  try {
    const user = await deps.store.appUser({ provider: signin.pr, ...identity, at: at(now) });
    if (!user) return fail(`that email's account is linked to another ${signin.pr} account`);
    const code = deps.random();
    await deps.store.createCode({
      hash: sha256hex(code),
      userId: user.id,
      platform: signin.pl,
      redirect: signin.r,
      challenge: signin.c,
      expiresAt: at(now + CODE_SECONDS),
    });
    await deps.store.sweep(at(now)).catch((error) => deps.log(`app sign-in: old codes not removed (${why(error)})`));
    return back({ code });
  } catch (error) {
    return fail(`the account could not be saved (${why(error)})`);
  }
}

// ---- POST /api/app/token ----

async function readJson(request) {
  const body = await request.text().catch(() => null);
  if (typeof body !== "string" || body.length > BODY_LIMIT) return null;
  try {
    const value = JSON.parse(body);
    return value && typeof value === "object" && !Array.isArray(value) ? value : null;
  } catch {
    return null;
  }
}

// The app's description of itself. The name is checked but not kept: the devices table holds only what the
// privacy page lists.
function parseDevice(device) {
  if (!device || typeof device !== "object") return null;
  const { id, platform, version, name } = device;
  if (typeof id !== "string" || !DEVICE_ID.test(id)) return null;
  if (!PLATFORMS.includes(platform)) return null;
  if (typeof version !== "string" || version.length > 32 || !PRINTABLE.test(version)) return null;
  if (name !== undefined && name !== null && (typeof name !== "string" || [...name].length > 60 || /[\u0000-\u001f\u007f]/.test(name))) {
    return null;
  }
  return { id, platform, version };
}

const accountJson = (a) => ({ id: a.id, email: a.email, name: a.name || "", providers: [...(a.providers || [])] });

async function issue(deps, userId, family, device) {
  const now = deps.now();
  await deps.store.recordDevice({ userId, deviceId: device.id, platform: device.platform, version: device.version, at: at(now) });
  const access = `fa1_${deps.random()}`;
  const refresh = `fr1_${deps.random()}`;
  await deps.store.issueTokens({
    accessHash: sha256hex(access),
    refreshHash: sha256hex(refresh),
    userId,
    family,
    deviceId: device.id,
    at: at(now),
    accessExpiresAt: at(now + ACCESS_SECONDS),
    refreshExpiresAt: at(now + REFRESH_SECONDS),
  });
  const account = await deps.store.account(userId);
  if (!account) return invalidGrant();
  return json(200, {
    access_token: access,
    token_type: "Bearer",
    expires_in: ACCESS_SECONDS,
    refresh_token: refresh,
    account: accountJson(account),
  });
}

export async function appToken(request, deps) {
  if (!deps.config?.ready) return unavailable();
  try {
    const wait = await limit(deps, [[ipKey(deps, request, "token"), LIMITS.token]]);
    if (wait) return slowDown(wait);
  } catch (error) {
    deps.log(`app token: the rate limit could not be checked (${why(error)})`);
    return unavailable();
  }
  const body = await readJson(request);
  const device = parseDevice(body?.device);
  if (!body || !device) return invalidRequest();
  const now = deps.now();
  try {
    if (body.grant_type === "authorization_code") {
      if (typeof body.code !== "string" || typeof body.code_verifier !== "string" || !VERIFIER.test(body.code_verifier)) {
        return invalidRequest();
      }
      if (!ONE_TIME_CODE.test(body.code)) return invalidGrant();
      // Burned before anything is checked, so a wrong verifier can't be retried against the same code.
      const granted = await deps.store.burnCode(sha256hex(body.code), at(now));
      if (!granted || !same(s256(body.code_verifier), granted.challenge) || granted.platform !== device.platform) {
        return invalidGrant();
      }
      return await issue(deps, granted.user_id, deps.random(), device);
    }
    if (body.grant_type === "refresh_token") {
      if (typeof body.refresh_token !== "string") return invalidRequest();
      if (!REFRESH.test(body.refresh_token)) return invalidGrant();
      const used = await deps.store.useRefresh(sha256hex(body.refresh_token), at(now));
      if (used?.reused) deps.log("app token: a refresh token came back after it was rotated; that sign-in was ended");
      if (!used || used.reused) return invalidGrant();
      return await issue(deps, used.user_id, used.family, device);
    }
    return invalidRequest();
  } catch (error) {
    deps.log(`app token: the database could not be used (${why(error)})`);
    return unavailable();
  }
}

// ---- the bearer endpoints ----

// Runs `fn(account, access)` for the account the request's access token belongs to.
async function withAccount(request, deps, fn) {
  if (!deps.config?.ready) return unavailable();
  const m = BEARER.exec(request.headers.get("authorization") || "");
  if (!m) return invalidToken();
  try {
    const token = await deps.store.accessUser(sha256hex(m[1]), at(deps.now()));
    const account = token && (await deps.store.account(token.user_id));
    if (!account) return invalidToken();
    return await fn(account, m[1]);
  } catch (error) {
    deps.log(`app account: the database could not be used (${why(error)})`);
    return unavailable();
  }
}

// GET /api/app/me
export const appMe = (request, deps) =>
  withAccount(request, deps, async (account) => {
    const devices = await deps.store.devices(account.id);
    const iso = (v) => new Date(v).toISOString();
    return json(200, {
      account: accountJson(account),
      devices: devices.map((d) => ({
        platform: d.platform,
        version: d.app_version,
        first_seen_at: iso(d.first_seen_at),
        last_seen_at: iso(d.last_seen_at),
      })),
    });
  });

// POST /api/app/signout {"refresh_token"}: ends that sign-in (and the bearer's, when one is sent). Always ok for an
// unknown token, so it can be sent best effort.
export async function appSignOut(request, deps) {
  if (!deps.config?.ready) return unavailable();
  const body = await readJson(request);
  const tokens = [];
  if (typeof body?.refresh_token === "string" && REFRESH.test(body.refresh_token)) tokens.push(body.refresh_token);
  const m = BEARER.exec(request.headers.get("authorization") || "");
  if (m) tokens.push(m[1]);
  try {
    for (const token of tokens) await deps.store.revokeFamilyOf(sha256hex(token), at(deps.now()));
  } catch (error) {
    deps.log(`app sign-out: the database could not be used (${why(error)})`);
    return unavailable();
  }
  return json(200, { ok: true });
}

// POST /api/app/delete: the account row goes, and its codes, tokens and devices with it (ON DELETE CASCADE).
export const appDelete = (request, deps) =>
  withAccount(request, deps, async (account) => {
    await deps.store.deleteUser(account.id);
    return json(200, { deleted: true });
  });

// POST /api/app/assertion {"endpoint"}: the signed statement of this account for pairing the phone `endpoint`.
export const appAssertion = (request, deps) =>
  withAccount(request, deps, async (account) => {
    if (!deps.config.assertion) return unavailable();
    const body = await readJson(request);
    const endpoint = body?.endpoint;
    if (typeof endpoint !== "string" || endpoint.length > 200 || !PRINTABLE.test(endpoint)) return invalidRequest();
    const wait = await limit(deps, [[`account:assertion:${account.id}`, LIMITS.assertion]]);
    if (wait) return slowDown(wait);
    const { key, kid } = deps.config.assertion;
    const payload = assertionPayload({ kid, sub: account.id, email: account.email, dev: endpoint, now: deps.now() });
    return json(200, { assertion: signAssertion(key, payload), expires_in: ASSERTION_SECONDS });
  });

// ---- GET /.well-known/assetlinks.json (rewritten to /api/app/assetlinks) ----

export function assetLinks(_request, deps) {
  const prints = deps.config?.androidCerts || [];
  const body = prints.length
    ? [
        {
          relation: ["delegate_permission/common.handle_all_urls"],
          target: { namespace: "android_app", package_name: ANDROID_PACKAGE, sha256_cert_fingerprints: prints },
        },
      ]
    : [];
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json", "cache-control": "public, max-age=300", "x-content-type-options": "nosniff" },
  });
}

// ---- GET /app/callback (rewritten to /api/app/landing) ----
// Android's sign-in returns here; the app normally opens before this page shows. A tap on the link opens it.
export function appLanding(request, deps) {
  const site = deps.config?.siteUrl || new URL(request.url).origin;
  const q = new URL(request.url).searchParams;
  const kept = new URLSearchParams();
  for (const name of ["code", "state", "error"]) {
    const value = q.get(name);
    if (value && /^[A-Za-z0-9_-]{1,128}$/.test(value)) kept.set(name, value);
  }
  const link = `${site}${ANDROID_RETURN}${kept.size ? `?${kept}` : ""}`;
  return page(200, {
    title: "sign in",
    body: `<p><a class="download" href="${escapeHtml(link)}">Open FERB</a></p>
<p>If FERB didn't open, go back to FERB and try again.</p>`,
  });
}
