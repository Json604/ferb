// decision: REL-449
// decision: REL-451
// The browser part of the apps' sign-in: /api/app/start and /api/app/callback.
import assert from "node:assert/strict";
import { test } from "node:test";
import { appCallback, appStart, sha256hex, s256 } from "../api/_lib/app.js";
import { sign } from "../api/_lib/tokens.js";
import { cookies, get, NOW, SECRET } from "./support.mjs";
import {
  APP_STATE,
  APPLE_SERVICES_ID,
  appFakes,
  browserSignIn,
  formPost,
  RETURN,
  SITE,
  startParams,
  startUrl,
  VERIFIER,
  withIp,
} from "./app-support.mjs";

test("start sends the app to Google with the site's own state, nonce and PKCE, kept in a SameSite=None cookie for /api/app", async () => {
  const f = appFakes();
  const r = await appStart(get(startUrl(startParams())), f.deps);
  assert.equal(r.status, 303);
  const c = cookies(r).ferb_app_signin;
  assert.match(c.raw, /Path=\/api\/app;/);
  assert.match(c.raw, /Max-Age=600/);
  assert.match(c.raw, /HttpOnly/);
  assert.match(c.raw, /Secure/);
  assert.match(c.raw, /SameSite=None/);
  const [body] = c.value.split(".");
  const saved = JSON.parse(Buffer.from(body, "base64url").toString());
  assert.deepEqual(
    { pr: saved.pr, pl: saved.pl, r: saved.r, as: saved.as, c: saved.c, x: saved.x },
    { pr: "google", pl: "mac", r: "ferb-auth://callback", as: APP_STATE, c: s256(VERIFIER), x: NOW + 600 },
  );
  const [args] = f.calls.authorizationUrl;
  assert.deepEqual(args, { redirectUri: `${SITE}/api/app/callback`, state: saved.s, nonce: saved.n, challenge: s256(saved.v) });
  assert.notEqual(saved.s, APP_STATE, "the provider never sees the app's own state");
  assert.equal(r.headers.get("location"), `https://accounts.example.com/auth?state=${saved.s}`);
});

test("start sends the app to Apple with form_post, the name and email scope, state and nonce", async () => {
  const f = appFakes();
  const r = await appStart(get(startUrl(startParams({ provider: "apple", platform: "ios" }))), f.deps);
  assert.equal(r.status, 303);
  const to = new URL(r.headers.get("location"));
  assert.equal(`${to.origin}${to.pathname}`, "https://appleid.apple.com/auth/authorize");
  const saved = JSON.parse(Buffer.from(cookies(r).ferb_app_signin.value.split(".")[0], "base64url").toString());
  assert.deepEqual(Object.fromEntries(to.searchParams), {
    response_type: "code",
    response_mode: "form_post",
    client_id: APPLE_SERVICES_ID,
    redirect_uri: `${SITE}/api/app/callback`,
    scope: "name email",
    state: saved.s,
    nonce: saved.n,
  });
  assert.match(r.headers.get("location"), /scope=name%20email/);
  assert.equal(saved.v, undefined, "Apple's sign-in has no PKCE verifier of the site's");
});

test("every return address that isn't exactly the platform's own, and every bad parameter, is refused with a page", async () => {
  const f = appFakes();
  const bad = [
    { redirect: "ferb-auth://callback/" },
    { redirect: "ferb-auth://callback?x=1" },
    { redirect: "FERB-AUTH://callback" },
    { redirect: "ferb-auth://callback#x" },
    { redirect: "ferb-auth://evil" },
    { redirect: `${SITE}/app/callback` }, // a Mac app is not Android
    { platform: "ios", redirect: `${SITE}/app/callback` },
    { platform: "android", redirect: "ferb-auth://callback" },
    { platform: "android", redirect: `${SITE}/app/callback/` },
    { platform: "android", redirect: "http://ferb.example.com/app/callback" },
    { platform: "android", redirect: "https://evil.example.com/app/callback" },
    { platform: "android", redirect: `${SITE}/app/callback?next=https://evil.example.com` },
    { redirect: "" },
    { platform: "windows" },
    { provider: "github" },
    { state: "short-state-1234" .slice(0, 15) },
    { state: "x".repeat(129) },
    { state: "has space in it 0123" },
    { code_challenge: s256(VERIFIER).slice(0, 42) },
    { code_challenge: `${s256(VERIFIER)}A` },
    { code_challenge: s256(VERIFIER).replace(/^./, "+") },
    { code_challenge_method: "plain" },
  ];
  for (const change of bad) {
    const r = await appStart(get(startUrl({ ...startParams(), ...change })), f.deps);
    assert.equal(r.status, 400, JSON.stringify(change));
    assert.equal(r.headers.get("location"), null, JSON.stringify(change));
    assert.equal(r.headers.getSetCookie().length, 0, JSON.stringify(change));
    assert.doesNotMatch(await r.text(), /<script/i);
  }
  for (const missing of ["provider", "platform", "redirect", "state", "code_challenge", "code_challenge_method"]) {
    const params = startParams();
    delete params[missing];
    assert.equal((await appStart(get(startUrl(params)), f.deps)).status, 400, missing);
  }
  // A second copy of a parameter is refused rather than guessed at.
  const twice = `${startUrl(startParams())}&redirect=${encodeURIComponent("https://evil.example.com/")}`;
  assert.equal((await appStart(get(twice), f.deps)).status, 400);
  assert.equal(f.calls.authorizationUrl.length, 0);
  // The exact addresses work on their platforms.
  for (const platform of ["mac", "ios", "android"]) {
    const r = await appStart(get(startUrl(startParams({ platform }))), f.deps);
    assert.equal(r.status, 303, platform);
  }
});

test("a provider that isn't set up answers start with a plain 503 page; so does a site without its settings", async () => {
  for (const [options, provider] of [[{ apple: false }, "apple"], [{ google: false }, "google"], [{ ready: false }, "google"]]) {
    const f = appFakes(options);
    const r = await appStart(get(startUrl(startParams({ provider }))), f.deps);
    assert.equal(r.status, 503, JSON.stringify(options));
    assert.match(r.headers.get("content-type"), /text\/html/);
    assert.doesNotMatch(await r.text(), /<script/i);
  }
});

test("start allows 20 sign-ins per IP in 10 minutes, and keeps the IP only as an HMAC", async () => {
  const f = appFakes();
  const at = (ip) => withIp(get(startUrl(startParams())), ip);
  for (let i = 0; i < 20; i++) assert.equal((await appStart(at("203.0.113.7"), f.deps)).status, 303, `#${i + 1}`);
  const limited = await appStart(at("203.0.113.7"), f.deps);
  assert.equal(limited.status, 429);
  assert.equal(limited.headers.get("retry-after"), "600");
  assert.equal(limited.headers.getSetCookie().length, 0);
  assert.equal((await appStart(at("198.51.100.1"), f.deps)).status, 303, "another IP has its own count");
  // x-forwarded-for's first address when there is no x-real-ip.
  const forwarded = new Request(`${SITE}${startUrl(startParams())}`, { headers: { "x-forwarded-for": "203.0.113.7, 10.0.0.1" } });
  assert.equal((await appStart(forwarded, f.deps)).status, 429);
  for (const key of f.store.limits.keys()) {
    assert.doesNotMatch(key, /203\.0\.113|198\.51\.100/);
    assert.match(key, /^ip:start:[0-9a-f]{64}$/);
  }
  f.advance(600);
  assert.equal((await appStart(at("203.0.113.7"), f.deps)).status, 303, "a new window");
});

test("Google: the code grant gets the cookie's verifier, state and nonce; the app gets a one-time code with its own state", async () => {
  const f = appFakes();
  const { start, done, back, code } = await browserSignIn(f);
  const saved = JSON.parse(Buffer.from(cookies(start).ferb_app_signin.value.split(".")[0], "base64url").toString());
  assert.deepEqual(f.calls.exchange[0], {
    url: `${SITE}/api/app/callback?code=google-code&state=${saved.s}`,
    checks: { verifier: saved.v, state: saved.s, nonce: saved.n },
  });
  assert.equal(f.calls.authorizationUrl[0].nonce, saved.n, "the nonce sent to Google is the one checked");
  assert.equal(done.status, 303);
  assert.equal(`${back.protocol}//${back.host}`, "ferb-auth://callback");
  assert.deepEqual([...back.searchParams.keys()], ["code", "state"]);
  assert.equal(back.searchParams.get("state"), APP_STATE);
  assert.match(code, /^[A-Za-z0-9_-]{43}$/);
  assert.equal(cookies(done).ferb_app_signin.value, "", "the cookie is cleared");
  assert.match(cookies(done).ferb_app_signin.raw, /Path=\/api\/app;.*SameSite=None/);
  // Only the code's SHA-256 is kept, with what the token endpoint checks.
  const kept = f.store.codes.get(sha256hex(code));
  assert.deepEqual(
    { platform: kept.platform, redirect: kept.redirect, challenge: kept.challenge, expires_at: kept.expires_at, used_at: kept.used_at },
    { platform: "mac", redirect: "ferb-auth://callback", challenge: s256(VERIFIER), expires_at: new Date((NOW + 120) * 1000), used_at: null },
  );
  assert.doesNotMatch(JSON.stringify([...f.store.codes.values()]), new RegExp(code));
  const user = f.store.users.get(kept.user_id);
  assert.deepEqual(
    { email: user.email, name: user.name, google_sub: user.google_sub, apple_sub: user.apple_sub, providers: user.providers },
    { email: "ada@example.com", name: "Ada L", google_sub: "google-sub-0001", apple_sub: null, providers: ["google"] },
  );
});

test("Android's sign-in returns to the site's own App Link address", async () => {
  const f = appFakes();
  const { back } = await browserSignIn(f, { platform: "android" });
  assert.equal(`${back.origin}${back.pathname}`, `${SITE}/app/callback`);
  assert.equal(back.searchParams.get("state"), APP_STATE);
});

test("the provider's state must equal the cookie's; a mismatch goes back to the app as failed", async () => {
  const f = appFakes();
  const start = await appStart(get(startUrl(startParams())), f.deps);
  const cookie = `ferb_app_signin=${cookies(start).ferb_app_signin.value}`;
  for (const state of ["someone-elses-state", "", null]) {
    const q = state === null ? "?code=c" : `?code=c&state=${state}`;
    const r = await appCallback(get(`/api/app/callback${q}`, cookie), f.deps);
    assert.equal(r.status, 303);
    assert.equal(r.headers.get("location"), `ferb-auth://callback?error=failed&state=${APP_STATE}`);
  }
  assert.equal(f.calls.exchange.length, 0, "Google is never asked");
  assert.equal(f.store.codes.size, 0);
});

test("a missing, forged, expired or other-purpose cookie gets a plain page, never a redirect", async () => {
  const f = appFakes();
  const start = await appStart(get(startUrl(startParams())), f.deps);
  const good = cookies(start).ferb_app_signin.value;
  const [body] = good.split(".");
  const payload = JSON.parse(Buffer.from(body, "base64url").toString());
  const forgedBody = Buffer.from(JSON.stringify({ ...payload, r: "https://evil.example.com/" })).toString("base64url");
  const cases = [
    undefined,
    "ferb_app_signin=garbage",
    `ferb_app_signin=${forgedBody}.${good.split(".")[1]}`,
    `ferb_app_signin=${sign("another-secret-0123456789abcdef01234567", payload)}`,
    `ferb_app_signin=${sign(SECRET, { ...payload, p: "signin" })}`,
    // Signed with the site's secret, but naming an address that isn't allowed is still refused.
    `ferb_app_signin=${sign(SECRET, { ...payload, r: "https://evil.example.com/" })}`,
  ];
  for (const cookie of cases) {
    const r = await appCallback(get(`/api/app/callback?code=c&state=${payload.s}`, cookie), f.deps);
    assert.equal(r.status, 400, String(cookie));
    assert.equal(r.headers.get("location"), null);
    assert.doesNotMatch(await r.text(), /<script/i);
  }
  f.advance(600);
  const late = await appCallback(get(`/api/app/callback?code=c&state=${payload.s}`, `ferb_app_signin=${good}`), f.deps);
  assert.equal(late.status, 400, "ten minutes is the limit");
  assert.equal(f.calls.exchange.length, 0);
});

test("the provider's error goes back to the app as cancelled; an unverified email as no_email; an unconfirmed sign-in as failed", async () => {
  const f = appFakes();
  const once = async (query) => {
    const start = await appStart(get(startUrl(startParams())), f.deps);
    const state = new URL(start.headers.get("location")).searchParams.get("state");
    const cookie = `ferb_app_signin=${cookies(start).ferb_app_signin.value}`;
    return appCallback(get(`/api/app/callback?${query.replace("STATE", state)}`, cookie), f.deps);
  };
  const back = (r) => r.headers.get("location");
  assert.equal(back(await once("error=access_denied&state=STATE")), `ferb-auth://callback?error=cancelled&state=${APP_STATE}`);
  f.claims = { sub: "google-sub-0001", email: "ada@example.com", email_verified: false };
  assert.equal(back(await once("code=c&state=STATE")), `ferb-auth://callback?error=no_email&state=${APP_STATE}`);
  f.claims = { sub: "google-sub-0001", email_verified: true };
  assert.equal(back(await once("code=c&state=STATE")), `ferb-auth://callback?error=no_email&state=${APP_STATE}`);
  f.claims = Object.assign(new Error("nonce mismatch"), { code: "OAUTH_JWT_CLAIM_COMPARISON_FAILED" });
  assert.equal(back(await once("code=c&state=STATE")), `ferb-auth://callback?error=failed&state=${APP_STATE}`);
  assert.equal(f.store.users.size, 0);
  assert.equal(f.store.codes.size, 0);
});

test("Apple: the form_post answer's ID token is checked, the name comes from the first answer, and private-relay emails are fine", async () => {
  const f = appFakes();
  const user = JSON.stringify({ name: { firstName: "Ada", lastName: "Lovelace" }, email: "ada.relay@privaterelay.appleid.com" });
  const { done, back, code } = await browserSignIn(f, { provider: "apple", user });
  assert.equal(done.status, 303);
  assert.equal(back.searchParams.get("state"), APP_STATE);
  assert.match(code, /^[A-Za-z0-9_-]{43}$/);
  const form = f.apple.tokenForms[0];
  assert.equal(form.get("code"), "apple-code");
  assert.equal(form.get("redirect_uri"), `${SITE}/api/app/callback`);
  assert.equal(form.get("client_id"), APPLE_SERVICES_ID);
  assert.equal(form.get("grant_type"), "authorization_code");
  const row = f.store.users.get(f.store.codes.get(sha256hex(code)).user_id);
  assert.deepEqual(
    { email: row.email, name: row.name, apple_sub: row.apple_sub, providers: row.providers },
    { email: "ada.relay@privaterelay.appleid.com", name: "Ada Lovelace", apple_sub: "001234.apple-sub.0001", providers: ["apple"] },
  );
});

test("Apple: an ID token whose nonce isn't this sign-in's is refused", async () => {
  const f = appFakes();
  const start = await appStart(get(startUrl(startParams({ provider: "apple" }))), f.deps);
  const location = new URL(start.headers.get("location"));
  f.apple.nonce = "another-sign-ins-nonce";
  const r = await appCallback(
    formPost("/api/app/callback", { code: "apple-code", state: location.searchParams.get("state") }, `ferb_app_signin=${cookies(start).ferb_app_signin.value}`),
    f.deps,
  );
  assert.equal(r.headers.get("location"), `ferb-auth://callback?error=failed&state=${APP_STATE}`);
  assert.equal(f.store.users.size, 0);
  assert.ok(f.logs.some((l) => /TOKEN_NONCE/.test(l)));
});

test("Apple: no verified email is no_email; Apple's own error is cancelled; an answer by the wrong method is failed", async () => {
  const f = appFakes();
  const begin = async (provider = "apple") => {
    const start = await appStart(get(startUrl(startParams({ provider }))), f.deps);
    const location = new URL(start.headers.get("location"));
    f.apple.nonce = location.searchParams.get("nonce");
    return { state: location.searchParams.get("state"), cookie: `ferb_app_signin=${cookies(start).ferb_app_signin.value}` };
  };
  let s = await begin();
  f.apple.claims = { email_verified: "false" };
  let r = await appCallback(formPost("/api/app/callback", { code: "c", state: s.state }, s.cookie), f.deps);
  assert.match(r.headers.get("location"), /error=no_email/);
  f.apple.claims = {};
  s = await begin();
  r = await appCallback(formPost("/api/app/callback", { error: "user_cancelled_authorize", state: s.state }, s.cookie), f.deps);
  assert.match(r.headers.get("location"), /error=cancelled/);
  s = await begin();
  r = await appCallback(get(`/api/app/callback?code=c&state=${s.state}`, s.cookie), f.deps);
  assert.match(r.headers.get("location"), /error=failed/);
  s = await begin("google");
  r = await appCallback(formPost("/api/app/callback", { code: "c", state: s.state }, s.cookie), f.deps);
  assert.match(r.headers.get("location"), /error=failed/);
  f.apple.tokenStatus = 400;
  s = await begin();
  r = await appCallback(formPost("/api/app/callback", { code: "c", state: s.state }, s.cookie), f.deps);
  assert.match(r.headers.get("location"), /error=failed/);
  assert.equal(f.store.users.size, 0);
});

test("one account per person: the download list's row, then Google, then Apple with the same email, is one account", async () => {
  const f = appFakes();
  const before = f.store.addDownloadUser("ada@example.com", "Ada (download)");
  const google = await browserSignIn(f);
  assert.equal(f.store.codes.get(sha256hex(google.code)).user_id, before.id);
  f.apple.claims = { email: "ada@example.com", email_verified: true, is_private_email: false };
  const apple = await browserSignIn(f, { provider: "apple" });
  assert.equal(f.store.codes.get(sha256hex(apple.code)).user_id, before.id);
  assert.deepEqual(f.store.users.get(before.id).providers, ["google", "apple"]);
  // Google's account keeps its link when its email changes.
  f.claims = { sub: "google-sub-0001", email: "ada.new@example.com", email_verified: true, name: "Ada L" };
  const again = await browserSignIn(f);
  assert.equal(f.store.codes.get(sha256hex(again.code)).user_id, before.id);
  assert.equal(f.store.users.size, 1);
});

test("an email whose account is linked to another Google account is refused, not taken over", async () => {
  const f = appFakes();
  await browserSignIn(f);
  f.claims = { sub: "google-sub-9999", email: "ada@example.com", email_verified: true };
  const { back } = await browserSignIn(f);
  assert.equal(back.searchParams.get("error"), "failed");
  assert.equal(f.store.users.size, 1);
  assert.equal([...f.store.users.values()][0].google_sub, "google-sub-0001");
});

test("a database that can't be reached sends the app back with failed, and the logs carry nothing personal", async () => {
  const f = appFakes();
  const start = await appStart(get(startUrl(startParams())), f.deps);
  const state = new URL(start.headers.get("location")).searchParams.get("state");
  f.store.fail();
  const r = await appCallback(get(`/api/app/callback?code=c&state=${state}`, `ferb_app_signin=${cookies(start).ferb_app_signin.value}`), f.deps);
  assert.equal(r.headers.get("location"), `ferb-auth://callback?error=failed&state=${APP_STATE}`);
  assert.ok(f.logs.length > 0);
  for (const line of f.logs) assert.doesNotMatch(line, /ada|Ada|example\.com|google-code|google-sub/);
  assert.equal((await appStart(get(startUrl(startParams())), f.deps)).status, 503, "the rate limit can't be checked");
});
