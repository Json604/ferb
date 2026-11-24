// The apps' sign-in (REL-449, REL-452) and the phone pairing assertion (PHN-828): FERB.app, the iPhone app and the
// Android app sign in to one FERB account (a row of ferb_users) with Google or Apple. The app opens /api/app/start
// in a browser with its own PKCE challenge and state; the provider answers /api/app/callback; the site hands the app
// a one-time code at an allowlisted address; the app trades it with its verifier at /api/app/token for an access
// token (1 hour) and a refresh token (180 days, REL-445; rotated on each use). The rotation's new pair is derived from
// the old refresh token (an HMAC with AUTH_SECRET), so the same old token presented again within REFRESH_GRACE seconds,
// before its successor was used, gets the same pair back (a reply lost on the way, two refreshes racing); a reused one
// after that ends the whole sign-in (REL-453, REL-454). A refused refresh says why (`reason`: signed_out, expired,
// account_deleted), so an app asks to sign in again and keeps its pairing unless the account is gone.
// The addresses (REL-452): FERB.app's loopback http://127.0.0.1:<port>/callback (RFC 8252 §7.3; the port is the
// one FERB.app is listening on, so it is part of the request), the iPhone's ferb-auth://callback, Android's App Link.
// The iPhone's Sign in with Apple can also be the system's own (/api/app/apple, PHN-841): Apple's identity token for
// the app itself, checked here against Apple's keys, then the same account and tokens.
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
const IOS_RETURN = "ferb-auth://callback";
// FERB.app's loopback return: exactly 127.0.0.1 (never "localhost", which a hosts file could point elsewhere), a
// port FERB.app chose (1024-65535, written without leading zeros), the path /callback; no userinfo, query or fragment.
const MAC_RETURN = /^http:\/\/127\.0\.0\.1:([1-9][0-9]{3,4})\/callback$/;
const ANDROID_PACKAGE = "com.ferb.phone";
// The iPhone app's bundle id: the audience of the identity tokens Sign in with Apple gives it (PHN-841).
const IOS_BUNDLE = "com.ferb.phone";
const APPLE_NONCE_SECONDS = 10 * 60;
const ID_TOKEN_LIMIT = 8 * 1024;

const CODE_SECONDS = 120;
const ACCESS_SECONDS = 60 * 60;
const REFRESH_SECONDS = 180 * 24 * 60 * 60; // owner, 2026-11-24 (REL-445)
// owner, 2026-11-24 (REL-453): the previous refresh token is still answered, with the same new pair, this long.
export const REFRESH_GRACE = 60;
// The longest name kept, in characters (Unicode code points), on the site and in the daemon alike (REL-453).
export const NAME_MAX = 200;
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

// The pair a refresh token rotates to: the same for the same token, unguessable without both it and AUTH_SECRET.
export function successorOf(secret, refresh) {
  const derive = (kind) => createHmac("sha256", secret).update(`ferb-app-rotate/v1.${kind}.${refresh}`).digest("base64url");
  return { access: `fa1_${derive("access")}`, refresh: `fr1_${derive("refresh")}` };
}

// A display name as kept: no control characters, trimmed, at most NAME_MAX characters (never half a character). A
// name never stops a sign-in: one too long is cut, anything else that isn't text is no name.
export function keptName(name) {
  if (typeof name !== "string") return "";
  return [...name.replace(/[\u0000-\u001f\u007f]/g, " ").trim()].slice(0, NAME_MAX).join("").trim();
}

function same(a, b) {
  if (typeof a !== "string" || typeof b !== "string") return false;
  const x = Buffer.from(a);
  const y = Buffer.from(b);
  return x.length === y.length && timingSafeEqual(x, y);
}

const home = (deps, request) => deps.config.siteUrl || new URL(request.url).origin;

// Whether `target` is the address `platform`'s sign-in may return to: the iPhone's and Android's by exact string,
// FERB.app's by the exact loopback form with any unprivileged port. Nothing else, including the iPhone's scheme for a
// Mac (REL-452).
export function returnAllowed(site, platform, target) {
  if (typeof target !== "string") return false;
  if (platform === "android") return target === site + ANDROID_RETURN;
  if (platform === "ios") return target === IOS_RETURN;
  if (platform !== "mac") return false;
  const m = MAC_RETURN.exec(target);
  return Boolean(m) && Number(m[1]) >= 1024 && Number(m[1]) <= 65535;
}

const at = (seconds) => new Date(seconds * 1000);
const why = (error) => error?.code || error?.name || "error";

// ---- pages (the browser part of the sign-in) ----

const signInPage = (status, message, extra = {}) =>
  page(status, { title: "sign in", body: `<p>${message}</p>`, ...extra });
const unavailablePage = () => signInPage(503, "FERB's sign-in isn't answering right now. Try again in a minute.");

// ---- JSON (the apps' part) ----

const unavailable = () => json(503, { error: "unavailable" });
const invalidRequest = () => json(400, { error: "invalid_request" });
const invalidGrant = (reason) => json(401, reason ? { error: "invalid_grant", reason } : { error: "invalid_grant" });
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

// FERB.app's loopback with `error` and the app's state: only ever an address returnAllowed took for a Mac.
const macError = (target, appState, error) => redirect(`${target}?${new URLSearchParams({ error, state: appState })}`);

// A Mac sign-in the site isn't set up for: its loopback hears so, when the request names one well formed.
function macUnavailable(request) {
  const q = new URL(request.url).searchParams;
  const target = q.get("redirect");
  const appState = q.get("state") || "";
  if (q.get("platform") !== "mac" || !APP_STATE.test(appState) || !returnAllowed("", "mac", target)) return null;
  return macError(target, appState, "unavailable");
}

export async function appStart(request, deps) {
  if (!deps.config?.ready) return macUnavailable(request) || unavailablePage();
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
    !returnAllowed(site, platform, target) ||
    !APP_STATE.test(appState || "") ||
    !CHALLENGE.test(challenge || "") ||
    one("code_challenge_method") !== "S256"
  ) {
    return signInPage(400, "This sign-in link isn't complete. Go back to FERB and try again.");
  }
  // FERB.app waits at its loopback address: an error is sent there too, so FERB.app says it (REL-454).
  const toMac = (error) => (platform === "mac" ? macError(target, appState, error) : null);
  if (provider === "google" ? !deps.oidc : !deps.apple) {
    const name = provider === "google" ? "Google" : "Apple";
    return toMac("not_ready") || signInPage(503, `Signing in with ${name} isn't ready on FERB's site yet. Try again later.`);
  }
  try {
    const wait = await limit(deps, [[ipKey(deps, request, "start"), LIMITS.start]]);
    if (wait) {
      return toMac("slow_down") || signInPage(429, "There have been a lot of sign-ins from here in the last few minutes. Wait a few minutes, then try again from FERB.", {
        extra: { "retry-after": String(wait) },
      });
    }
  } catch (error) {
    deps.log(`app sign-in: the rate limit could not be checked (${why(error)})`);
    return toMac("unavailable") || unavailablePage();
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
    return toMac("unavailable") || unavailablePage();
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
    name: keptName(claims.name),
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
    !returnAllowed(site, signin.pl, signin.r) ||
    !APP_STATE.test(signin.as || "")
  ) {
    // No valid cookie, no trusted address to go back to (and FERB.app stops waiting when the cookie expires): a page.
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
  // The person said no (access_denied, Apple's user_cancelled_authorize); any other error is the provider's.
  if (params.has("error")) {
    const said = params.get("error");
    if (said === "access_denied" || said === "user_cancelled_authorize") return back({ error: "cancelled" });
    return fail(`${signin.pr} answered with an error`, "provider");
  }

  let identity;
  try {
    identity =
      signin.pr === "google"
        ? await googleIdentity(deps, request, signin, site)
        : await appleIdentity(deps, params, signin, site);
  } catch (error) {
    return fail(`${signin.pr} did not confirm the sign-in (${why(error)})`, "provider");
  }
  if (!identity.email) return fail(`${signin.pr} gave no verified email`, "no_email");

  try {
    const user = await deps.store.appUser({ provider: signin.pr, ...identity, at: at(now) });
    if (!user) return fail(`that email's account is linked to another ${signin.pr} account`, "account_conflict");
    const code = deps.random();
    await deps.store.createCode({
      hash: sha256hex(code),
      userId: user.id,
      platform: signin.pl,
      redirect: signin.r,
      challenge: signin.c,
      expiresAt: at(now + CODE_SECONDS),
    });
    await sweep(deps, now);
    return back({ code });
  } catch (error) {
    return fail(`the account could not be saved (${why(error)})`, "unavailable");
  }
}

// Expired codes and tokens go on every sign-in and refresh (the store runs it at most every few minutes, bounded).
const sweep = (deps, now) =>
  deps.store.sweep(at(now)).catch((error) => deps.log(`app sign-in: old codes and tokens not removed (${why(error)})`));

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

// The pair (random for a new sign-in, `pair` for a rotation) issued at `issuedAt` (Unix seconds; a rotation's own time,
// so its answer again within the grace is the same).
async function issue(deps, userId, family, device, { pair, issuedAt } = {}) {
  const now = deps.now();
  const from = issuedAt ?? now;
  await deps.store.recordDevice({ userId, deviceId: device.id, platform: device.platform, version: device.version, at: at(now) });
  const { access, refresh } = pair || { access: `fa1_${deps.random()}`, refresh: `fr1_${deps.random()}` };
  await deps.store.issueTokens({
    accessHash: sha256hex(access),
    refreshHash: sha256hex(refresh),
    userId,
    family,
    deviceId: device.id,
    at: at(from),
    accessExpiresAt: at(from + ACCESS_SECONDS),
    refreshExpiresAt: at(from + REFRESH_SECONDS),
  });
  const account = await deps.store.account(userId);
  if (!account) return invalidGrant("account_deleted");
  return json(200, {
    access_token: access,
    token_type: "Bearer",
    expires_in: Math.max(1, from + ACCESS_SECONDS - now),
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
      const answer = await issue(deps, granted.user_id, deps.random(), device);
      await sweep(deps, now);
      return answer;
    }
    if (body.grant_type === "refresh_token") {
      if (typeof body.refresh_token !== "string") return invalidRequest();
      if (!REFRESH.test(body.refresh_token)) return invalidGrant();
      const pair = successorOf(deps.config.secret, body.refresh_token);
      const used = await deps.store.useRefresh(sha256hex(body.refresh_token), at(now), {
        graceSeconds: REFRESH_GRACE,
        successorHash: sha256hex(pair.refresh),
      });
      if (used?.reused) deps.log("app token: a refresh token came back after it was rotated; that sign-in was ended");
      if (used?.replay) deps.log("app token: a refresh token came back within its grace; the same pair was answered");
      let answer;
      if (!used) answer = invalidGrant();
      else if (used.reused) answer = invalidGrant("signed_out");
      else if (used.ended) answer = invalidGrant(used.ended);
      else {
        const issuedAt = Math.floor(new Date(used.used_at).getTime() / 1000);
        answer = await issue(deps, used.user_id, used.family, device, { pair, issuedAt });
      }
      await sweep(deps, now);
      return answer;
    }
    return invalidRequest();
  } catch (error) {
    deps.log(`app token: the database could not be used (${why(error)})`);
    return unavailable();
  }
}

// ---- POST /api/app/apple: the iPhone's own Sign in with Apple (PHN-841) ----
// {"identity_token", "nonce", "name"?, "device"}: the identity token ASAuthorizationAppleIDProvider gave the iPhone app,
// and the raw nonce whose SHA-256 (hex) the app put in that request, as Apple documents. The token is checked here
// against Apple's keys (issuer, audience = the app's bundle id, times, nonce); each nonce is taken once in ten minutes,
// so a token seen once can't sign in again. Then the account and the tokens exactly as /api/app/token's code grant:
// the same account linking (apple_sub, then the verified email), device record, rate limit and answer. Apple sends
// the name only on the first authorization, and only to the app, so the app passes it on; it is a display name only.
export async function appAppleNative(request, deps) {
  if (!deps.config?.ready) return unavailable();
  if (!deps.apple) return unavailable();
  try {
    const wait = await limit(deps, [[ipKey(deps, request, "token"), LIMITS.token]]);
    if (wait) return slowDown(wait);
  } catch (error) {
    deps.log(`app apple: the rate limit could not be checked (${why(error)})`);
    return unavailable();
  }
  const body = await readJson(request);
  const device = parseDevice(body?.device);
  const token = body?.identity_token;
  const nonce = body?.nonce;
  // A display name only: one too long is cut, never a reason to refuse (REL-453).
  const name = keptName(body?.name);
  if (
    !device ||
    device.platform !== "ios" ||
    typeof token !== "string" ||
    !token ||
    token.length > ID_TOKEN_LIMIT ||
    typeof nonce !== "string" ||
    !ONE_TIME_CODE.test(nonce)
  ) {
    return invalidRequest();
  }
  let claims;
  try {
    claims = await deps.apple.verifyIdToken(token, sha256hex(nonce), IOS_BUNDLE);
  } catch (error) {
    if (String(error?.code || "").startsWith("TOKEN_")) {
      deps.log(`app apple: Apple's identity token was refused (${why(error)})`);
      return invalidGrant();
    }
    deps.log(`app apple: Apple's keys could not be read (${why(error)})`);
    return unavailable();
  }
  const verified = claims.email_verified === true || claims.email_verified === "true";
  const email = verified && typeof claims.email === "string" ? claims.email.trim().toLowerCase() : "";
  if (!email) {
    deps.log("app apple: Apple gave no verified email");
    return json(400, { error: "no_email" });
  }
  const now = deps.now();
  try {
    const { count } = await deps.store.hit(`apple-nonce:${sha256hex(nonce)}`, at(now), APPLE_NONCE_SECONDS);
    if (count > 1) {
      deps.log("app apple: a nonce came back a second time; refused");
      return invalidGrant();
    }
    const user = await deps.store.appUser({ provider: "apple", sub: claims.sub, email, name, at: at(now) });
    if (!user) {
      deps.log("app apple: that email's account is linked to another apple account");
      return invalidGrant();
    }
    const answer = await issue(deps, user.id, deps.random(), device);
    await sweep(deps, now);
    return answer;
  } catch (error) {
    deps.log(`app apple: the database could not be used (${why(error)})`);
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
    await deps.store.deleteUser(account.id, at(deps.now()));
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
// Android's sign-in returns here; the app normally opens before this page shows. A tap on the link opens it: an
// intent link naming FERB's package (REL-454), because a browser that already shows this address treats a plain link
// to it as a page and stays; the intent carries the same address, so the app reads the same callback.
export function androidIntent(site, query) {
  const url = new URL(site + ANDROID_RETURN);
  return `intent://${url.host}${url.pathname}${query ? `?${query}` : ""}#Intent;scheme=https;package=${ANDROID_PACKAGE};end`;
}

export function appLanding(request, deps) {
  const site = deps.config?.siteUrl || new URL(request.url).origin;
  const q = new URL(request.url).searchParams;
  const kept = new URLSearchParams();
  for (const name of ["code", "state", "error"]) {
    const value = q.get(name);
    if (value && /^[A-Za-z0-9_-]{1,128}$/.test(value)) kept.set(name, value);
  }
  const link = androidIntent(site, kept.size ? String(kept) : "");
  return page(200, {
    title: "sign in",
    body: `<p><a class="download" href="${escapeHtml(link)}">Open FERB</a></p>
<p>If FERB didn't open, go back to FERB and try again.</p>`,
  });
}
