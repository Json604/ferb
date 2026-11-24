// decision: PHN-841
// decision: REL-452
// The iPhone's own Sign in with Apple (/api/app/apple): Apple's identity token for the app (audience com.ferb.phone),
// the raw nonce whose SHA-256 the app sent to Apple, then the same account and tokens as the code grant.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { test } from "node:test";
import { appAppleNative, appToken, sha256hex } from "../api/_lib/app.js";
import { appleSignIn } from "../api/_lib/apple.js";
import { APP_ROUTES } from "../api/_lib/routes.js";
import { NOW } from "./support.mjs";
import { appFakes, browserSignIn, DEVICE, postJson, VERIFIER } from "./app-support.mjs";

const IPHONE = { id: "iphone-device-0123456789", platform: "ios", version: "0.28.0 (202611240101)", name: "Ada's iPhone" };
const RAW_NONCE = createHash("sha256").update("raw nonce of this sign-in").digest("base64url"); // 43 characters
const hashed = (raw) => createHash("sha256").update(raw).digest("hex");

// Apple's identity token as ASAuthorizationAppleIDProvider hands it to the app: aud is the bundle id, nonce the hash.
const nativeToken = (f, extra = {}) =>
  f.apple.idToken(f.apple.goodClaims({ aud: "com.ferb.phone", nonce: hashed(RAW_NONCE), ...extra }));
const call = (f, body, ip) => appAppleNative(postJson("/api/app/apple", body, { ip }), f.deps);
const good = (f, extra = {}) => ({ identity_token: nativeToken(f), nonce: RAW_NONCE, name: "Ada Lovelace", device: IPHONE, ...extra });

test("Apple's identity token for the iPhone app signs in: the same account, tokens and answer as the code grant", async () => {
  const f = appFakes();
  const r = await call(f, good(f));
  assert.equal(r.status, 200);
  assert.equal(r.headers.get("cache-control"), "no-store");
  const body = await r.json();
  assert.deepEqual(Object.keys(body), ["access_token", "token_type", "expires_in", "refresh_token", "account"]);
  assert.match(body.access_token, /^fa1_[A-Za-z0-9_-]{43}$/);
  assert.match(body.refresh_token, /^fr1_[A-Za-z0-9_-]{43}$/);
  assert.equal(body.expires_in, 3600);
  const [user] = f.store.users.values();
  assert.deepEqual(body.account, {
    id: user.id,
    email: "ada.relay@privaterelay.appleid.com",
    name: "Ada Lovelace",
    providers: ["apple"],
  });
  assert.equal(user.apple_sub, "001234.apple-sub.0001");
  // The device is recorded as the code grant records it; only hashes of the tokens are kept.
  assert.deepEqual([...f.store.deviceRows.values()].map((d) => [d.device_id, d.platform]), [[IPHONE.id, "ios"]]);
  assert.ok(f.store.tokens.has(sha256hex(body.refresh_token)));
  assert.doesNotMatch(JSON.stringify([...f.store.tokens.values()]), new RegExp(body.refresh_token));
  // The refresh token works on /api/app/token like any other.
  const again = await appToken(postJson("/api/app/token", { grant_type: "refresh_token", refresh_token: body.refresh_token, device: IPHONE }), f.deps);
  assert.equal(again.status, 200);
});

test("the same Apple account signs in to the account the web sign-in made (apple_sub), and an email links once", async () => {
  const f = appFakes();
  const { code } = await browserSignIn(f, { provider: "apple", platform: "ios" });
  const web = await appToken(postJson("/api/app/token", { grant_type: "authorization_code", code, code_verifier: VERIFIER, device: IPHONE }), f.deps);
  const webAccount = (await web.json()).account;
  const native = await (await call(f, good(f, { name: "" }))).json();
  assert.equal(native.account.id, webAccount.id);
  assert.equal(f.store.users.size, 1);

  // A Google account with the same verified email gets Apple linked to it.
  const g = appFakes({ claims: { sub: "google-sub-0002", email: "ada.relay@privaterelay.appleid.com", email_verified: true, name: "Ada" } });
  const { code: gc } = await browserSignIn(g);
  await appToken(postJson("/api/app/token", { grant_type: "authorization_code", code: gc, code_verifier: VERIFIER, device: DEVICE }), g.deps);
  const linked = await (await call(g, good(g))).json();
  assert.deepEqual(linked.account.providers, ["google", "apple"]);
  // Another Apple account with that email is refused, as on the web.
  const other = await call(g, good(g, { identity_token: nativeToken(g, { sub: "another-apple-sub" }), nonce: createHash("sha256").update("n2").digest("base64url") }));
  assert.equal(other.status, 401);
});

test("a token for the web's audience, another app, another nonce, an expired or forged token is refused", async () => {
  const f = appFakes();
  let n = 0;
  const fresh = () => createHash("sha256").update(`nonce-${++n}`).digest("base64url");
  const refused = async (identity_token, nonce, why) => {
    const r = await call(f, good(f, { identity_token, nonce }));
    assert.equal(r.status, 401, why);
    assert.deepEqual(await r.json(), { error: "invalid_grant" }, why);
  };
  let nonce = fresh();
  await refused(f.apple.idToken(f.apple.goodClaims({ nonce: hashed(nonce) })), nonce, "the Services ID's token (the web sign-in's)");
  nonce = fresh();
  await refused(nativeToken(f, { aud: "com.someone.else", nonce: hashed(nonce) }), nonce, "another app");
  await refused(nativeToken(f), fresh(), "the nonce isn't the one hashed into the token");
  nonce = fresh();
  await refused(nativeToken(f, { nonce }), nonce, "the raw nonce in the token instead of its hash");
  nonce = fresh();
  await refused(nativeToken(f, { nonce: hashed(nonce), exp: NOW }), nonce, "expired");
  const [h, p] = nativeToken(f).split(".");
  await refused(`${h}.${p}.${Buffer.from("forged").toString("base64url")}`, RAW_NONCE, "forged");
  assert.equal(f.store.users.size, 0);
  assert.equal(f.store.tokens.size, 0);
});

// decision: REL-453
test("a name never stops the sign-in: too long is cut to 200 characters, control characters become spaces, not text is no name", async () => {
  const cases = [
    ["x".repeat(201), "x".repeat(200)],
    ["😀".repeat(250), "😀".repeat(200)],
    ["Ada\nL", "Ada L"],
    [7, ""],
    [null, ""],
  ];
  for (const [name, kept] of cases) {
    const f = appFakes();
    const r = await call(f, good(f, { name }));
    assert.equal(r.status, 200, JSON.stringify(name).slice(0, 40));
    assert.equal((await r.json()).account.name, kept);
  }
});

test("a nonce is taken once: the same token and nonce can't sign in a second time", async () => {
  const f = appFakes();
  assert.equal((await call(f, good(f))).status, 200);
  const issued = f.store.tokens.size;
  const second = await call(f, good(f));
  assert.equal(second.status, 401);
  assert.equal(f.store.tokens.size, issued, "nothing more is issued");
  assert.ok(f.logs.some((m) => /a nonce came back a second time/.test(m)));
});

test("bad requests are refused before Apple is asked; no verified email is no_email; Apple down is 503", async () => {
  const f = appFakes();
  const bad = [
    { device: DEVICE }, // FERB.app's device: only the iPhone signs in this way
    { device: { ...IPHONE, platform: "android" } },
    { device: undefined },
    { identity_token: "" },
    { identity_token: 7 },
    { identity_token: "x".repeat(8193) },
    { nonce: "short" },
    { nonce: `${RAW_NONCE}A` },
    { nonce: undefined },
  ];
  for (const change of bad) {
    const r = await call(f, { ...good(f), ...change });
    assert.equal(r.status, 400, JSON.stringify(change).slice(0, 80));
  }
  assert.equal((await appAppleNative(postJson("/api/app/apple", "not json"), f.deps)).status, 400);
  assert.deepEqual(f.apple.fetches, [], "Apple's keys are never fetched for a bad request");

  const unverified = await call(f, good(f, { identity_token: nativeToken(f, { email_verified: "false" }) }));
  assert.equal(unverified.status, 400);
  assert.deepEqual(await unverified.json(), { error: "no_email" });

  const down = appFakes();
  const offline = async () => {
    throw Object.assign(new Error("offline"), { name: "TypeError" });
  };
  down.deps.apple = appleSignIn(down.apple.settings, { fetch: offline, now: () => NOW });
  assert.equal((await call(down, good(down))).status, 503);

  assert.equal((await call(appFakes({ apple: false }), good(f))).status, 503, "Apple not set up");
  assert.equal((await call(appFakes({ ready: false }), good(f))).status, 503, "the site not set up");
});

test("the native sign-in shares the token endpoint's rate limit per IP", async () => {
  const f = appFakes();
  for (let i = 0; i < 30; i++) {
    const nonce = createHash("sha256").update(`n${i}`).digest("base64url");
    const r = await call(f, good(f, { identity_token: nativeToken(f, { nonce: hashed(nonce) }), nonce }), "203.0.113.9");
    assert.equal(r.status, 200, `#${i + 1}`);
  }
  const limited = await call(f, good(f), "203.0.113.9");
  assert.equal(limited.status, 429);
});

test("/api/app/apple is a POST step of the apps' one function", () => {
  assert.deepEqual(Object.keys(APP_ROUTES.apple), ["POST"]);
  assert.equal(APP_ROUTES.apple.POST[0], appAppleNative);
});
