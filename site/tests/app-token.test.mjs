// decision: REL-449
// decision: REL-452
// decision: REL-445
// The apps' side of the sign-in: /api/app/token (code and refresh grants), and the bearer endpoints me, signout and
// delete.
import assert from "node:assert/strict";
import { test } from "node:test";
import { appDelete, appMe, appSignOut, appToken, sha256hex } from "../api/_lib/app.js";
import { NOW } from "./support.mjs";
import { appFakes, browserSignIn, DEVICE, getWith, postJson, SITE, VERIFIER } from "./app-support.mjs";

const token = (f, body, options) => appToken(postJson("/api/app/token", body, options), f.deps);
const codeGrant = (code, extra = {}) => ({ grant_type: "authorization_code", code, code_verifier: VERIFIER, device: DEVICE, ...extra });
const refreshGrant = (refresh, device = DEVICE) => ({ grant_type: "refresh_token", refresh_token: refresh, device });

async function signedIn(f, options) {
  const { code } = await browserSignIn(f, options);
  const r = await token(f, codeGrant(code, options?.platform ? { device: { ...DEVICE, platform: options.platform } } : {}));
  assert.equal(r.status, 200);
  return { code, ...(await r.json()) };
}

test("a code is traded once, with its verifier, for an access token, a refresh token and the account", async () => {
  const f = appFakes();
  const { code } = await browserSignIn(f);
  const r = await token(f, codeGrant(code));
  assert.equal(r.status, 200);
  assert.equal(r.headers.get("cache-control"), "no-store");
  assert.match(r.headers.get("content-type"), /^application\/json/);
  const body = await r.json();
  assert.match(body.access_token, /^fa1_[A-Za-z0-9_-]{43}$/);
  assert.match(body.refresh_token, /^fr1_[A-Za-z0-9_-]{43}$/);
  assert.equal(body.token_type, "Bearer");
  assert.equal(body.expires_in, 3600);
  const [user] = f.store.users.values();
  assert.deepEqual(body.account, { id: user.id, email: "ada@example.com", name: "Ada L", providers: ["google"] });
  assert.deepEqual(Object.keys(body), ["access_token", "token_type", "expires_in", "refresh_token", "account"]);

  // Only SHA-256 of each value is kept; the code, the tokens and the device name appear nowhere in the store.
  const access = f.store.tokens.get(sha256hex(body.access_token));
  const refresh = f.store.tokens.get(sha256hex(body.refresh_token));
  assert.equal(access.kind, "access");
  assert.equal(refresh.kind, "refresh");
  assert.equal(access.family, refresh.family);
  assert.deepEqual(access.expires_at, new Date((NOW + 3600) * 1000));
  assert.deepEqual(refresh.expires_at, new Date((NOW + 180 * 86400) * 1000));
  const everything = JSON.stringify([...f.store.codes.values(), ...f.store.tokens.values(), ...f.store.deviceRows.values()]);
  for (const secret of [code, body.access_token, body.refresh_token, "Ada's MacBook"]) assert.ok(!everything.includes(secret));
  assert.deepEqual([...f.store.deviceRows.values()].map((d) => [d.device_id, d.platform, d.app_version]), [[DEVICE.id, "mac", DEVICE.version]]);

  const again = await token(f, codeGrant(code));
  assert.equal(again.status, 401);
  assert.deepEqual(await again.json(), { error: "invalid_grant" });
});

test("a wrong verifier burns the code: the right one can't use it afterwards", async () => {
  const f = appFakes();
  const { code } = await browserSignIn(f);
  const wrong = await token(f, codeGrant(code, { code_verifier: `${VERIFIER.slice(0, -1)}X` }));
  assert.equal(wrong.status, 401);
  assert.deepEqual(await wrong.json(), { error: "invalid_grant" });
  assert.equal((await token(f, codeGrant(code))).status, 401);
  assert.equal(f.store.tokens.size, 0);
});

test("a code made for one platform can't be traded by another, and burns trying", async () => {
  const f = appFakes();
  const { code } = await browserSignIn(f);
  assert.equal((await token(f, codeGrant(code, { device: { ...DEVICE, platform: "ios" } }))).status, 401);
  assert.equal((await token(f, codeGrant(code))).status, 401);
});

test("a code lasts two minutes", async () => {
  const f = appFakes();
  const late = await browserSignIn(f);
  f.advance(120);
  assert.equal((await token(f, codeGrant(late.code))).status, 401);
  const fresh = await browserSignIn(f);
  f.advance(119);
  assert.equal((await token(f, codeGrant(fresh.code))).status, 200);
});

test("malformed requests are invalid_request; an unknown code or refresh token is invalid_grant", async () => {
  const f = appFakes();
  const { code } = await browserSignIn(f);
  const bad = [
    "not json",
    "[]",
    "",
    "x".repeat(17000),
    { ...codeGrant(code), device: undefined },
    { ...codeGrant(code), device: { ...DEVICE, id: "short-id-123456" } },
    { ...codeGrant(code), device: { ...DEVICE, id: `${"a".repeat(64)}b` } },
    { ...codeGrant(code), device: { ...DEVICE, id: "has spaces in it 0123" } },
    { ...codeGrant(code), device: { ...DEVICE, platform: "windows" } },
    { ...codeGrant(code), device: { ...DEVICE, version: "x".repeat(33) } },
    { ...codeGrant(code), device: { ...DEVICE, version: "" } },
    { ...codeGrant(code), device: { ...DEVICE, version: "1.0\n" } },
    { ...codeGrant(code), device: { ...DEVICE, name: "n".repeat(61) } },
    { ...codeGrant(code), device: { ...DEVICE, name: "tab\there" } },
    { ...codeGrant(code), grant_type: "password" },
    { ...codeGrant(code), code: 42 },
    { ...codeGrant(code), code_verifier: "too-short" },
    { ...codeGrant(code), code_verifier: undefined },
    { grant_type: "refresh_token", device: DEVICE },
  ];
  for (const body of bad) {
    const r = await token(f, body);
    assert.equal(r.status, 400, JSON.stringify(body).slice(0, 80));
    assert.deepEqual(await r.json(), { error: "invalid_request" });
  }
  for (const body of [codeGrant("nope"), codeGrant("A".repeat(43)), refreshGrant("fr1_nope"), refreshGrant(`fr1_${"A".repeat(43)}`)]) {
    const r = await token(f, body);
    assert.equal(r.status, 401, JSON.stringify(body));
    assert.deepEqual(await r.json(), { error: "invalid_grant" });
  }
  // A device without a name, and one with a 60-character name, are fine.
  const ok = await token(f, codeGrant(code, { device: { id: DEVICE.id, platform: "mac", version: "1" } }));
  assert.equal(ok.status, 200);
});

test("a refresh rotates the pair in the same family; using the old refresh token again ends the whole sign-in", async () => {
  const f = appFakes();
  const first = await signedIn(f);
  f.advance(1800);
  const r = await token(f, refreshGrant(first.refresh_token, { ...DEVICE, version: "0.28.1" }));
  assert.equal(r.status, 200);
  const second = await r.json();
  assert.notEqual(second.access_token, first.access_token);
  assert.notEqual(second.refresh_token, first.refresh_token);
  assert.deepEqual(second.account, first.account);
  const family = f.store.tokens.get(sha256hex(first.refresh_token)).family;
  assert.equal(f.store.tokens.get(sha256hex(second.refresh_token)).family, family);
  assert.deepEqual(f.store.tokens.get(sha256hex(second.refresh_token)).expires_at, new Date((NOW + 1800 + 180 * 86400) * 1000));
  assert.equal([...f.store.deviceRows.values()][0].app_version, "0.28.1");
  assert.equal((await appMe(getWith("/api/app/me", second.access_token), f.deps)).status, 200);

  // The old refresh token comes back (stolen, or replayed): the family is revoked, the new tokens included.
  const reuse = await token(f, refreshGrant(first.refresh_token));
  assert.equal(reuse.status, 401);
  assert.deepEqual(await reuse.json(), { error: "invalid_grant" });
  assert.equal((await token(f, refreshGrant(second.refresh_token))).status, 401);
  assert.equal((await appMe(getWith("/api/app/me", second.access_token), f.deps)).status, 401);
  assert.equal((await appMe(getWith("/api/app/me", first.access_token), f.deps)).status, 401);
  assert.ok(f.logs.some((l) => /came back after it was rotated/.test(l)));

  // Another sign-in of the same account (another family) is untouched.
  const other = await signedIn(f);
  assert.equal((await appMe(getWith("/api/app/me", other.access_token), f.deps)).status, 200);
});

test("a refresh token lasts 180 days from its issue", async () => {
  const f = appFakes();
  const { refresh_token } = await signedIn(f);
  f.advance(180 * 86400);
  assert.equal((await token(f, refreshGrant(refresh_token))).status, 401);
  const fresh = await signedIn(f);
  f.advance(180 * 86400 - 1);
  assert.equal((await token(f, refreshGrant(fresh.refresh_token))).status, 200);
});

test("me needs a live access token of the account, and lists its devices", async () => {
  const f = appFakes();
  const { access_token, refresh_token, account } = await signedIn(f);
  const r = await appMe(getWith("/api/app/me", access_token), f.deps);
  assert.equal(r.status, 200);
  assert.equal(r.headers.get("cache-control"), "no-store");
  assert.deepEqual(await r.json(), {
    account,
    devices: [{ platform: "mac", version: DEVICE.version, first_seen_at: new Date(NOW * 1000).toISOString(), last_seen_at: new Date(NOW * 1000).toISOString() }],
  });
  const refused = [
    getWith("/api/app/me"),
    getWith("/api/app/me", "garbage"),
    getWith("/api/app/me", refresh_token),
    getWith("/api/app/me", `fa1_${"A".repeat(43)}`),
    new Request(`${SITE}/api/app/me`, { headers: { authorization: `Basic ${access_token}` } }),
    new Request(`${SITE}/api/app/me`, { headers: { authorization: `Bearer  ${access_token}` } }),
    new Request(`${SITE}/api/app/me`, { headers: { authorization: `Bearer ${access_token}x` } }),
  ];
  for (const request of refused) {
    const no = await appMe(request, f.deps);
    assert.equal(no.status, 401);
    assert.deepEqual(await no.json(), { error: "invalid_token" });
    assert.match(no.headers.get("www-authenticate"), /invalid_token/);
  }
  // The scheme's case doesn't matter (RFC 7235).
  const lower = new Request(`${SITE}/api/app/me`, { headers: { authorization: `bearer ${access_token}` } });
  assert.equal((await appMe(lower, f.deps)).status, 200);
  f.advance(3600);
  assert.equal((await appMe(getWith("/api/app/me", access_token), f.deps)).status, 401, "an hour is the limit");
});

test("sign-out ends that sign-in (with or without the bearer), and always answers ok", async () => {
  const f = appFakes();
  const a = await signedIn(f);
  const b = await signedIn(f);
  const out = await appSignOut(postJson("/api/app/signout", { refresh_token: a.refresh_token }), f.deps);
  assert.equal(out.status, 200);
  assert.deepEqual(await out.json(), { ok: true });
  assert.equal((await appMe(getWith("/api/app/me", a.access_token), f.deps)).status, 401);
  assert.equal((await token(f, refreshGrant(a.refresh_token))).status, 401);
  assert.equal((await appMe(getWith("/api/app/me", b.access_token), f.deps)).status, 200, "the other device stays signed in");
  // With only the bearer, its own sign-in ends.
  await appSignOut(postJson("/api/app/signout", {}, { bearer: b.access_token }), f.deps);
  assert.equal((await token(f, refreshGrant(b.refresh_token))).status, 401);
  for (const body of [{ refresh_token: "fr1_unknown" }, {}, "nonsense", { refresh_token: a.refresh_token }]) {
    const r = await appSignOut(postJson("/api/app/signout", body), f.deps);
    assert.equal(r.status, 200);
  }
});

test("deleting the account removes the user, its codes, tokens and devices; every token stops working", async () => {
  const f = appFakes();
  const a = await signedIn(f);
  const b = await signedIn(f, { platform: "android" });
  const unused = await browserSignIn(f);
  f.claims = { sub: "google-sub-bob", email: "bob@example.com", email_verified: true, name: "Bob" };
  const bob = await signedIn(f);

  const r = await appDelete(postJson("/api/app/delete", {}, { bearer: a.access_token }), f.deps);
  assert.equal(r.status, 200);
  assert.deepEqual(await r.json(), { deleted: true });
  assert.ok(![...f.store.users.values()].some((u) => u.email === "ada@example.com"));
  const id = a.account.id;
  for (const map of [f.store.codes, f.store.tokens, f.store.deviceRows]) {
    assert.ok(![...map.values()].some((row) => row.user_id === id));
  }
  assert.equal(f.store.codes.has(sha256hex(unused.code)), false);
  for (const t of [a.access_token, b.access_token]) assert.equal((await appMe(getWith("/api/app/me", t), f.deps)).status, 401);
  for (const t of [a.refresh_token, b.refresh_token]) assert.equal((await token(f, refreshGrant(t))).status, 401);
  assert.equal((await appDelete(postJson("/api/app/delete", {}, { bearer: a.access_token }), f.deps)).status, 401);
  assert.equal((await appMe(getWith("/api/app/me", bob.access_token), f.deps)).status, 200, "other accounts stay");
});

test("the token endpoint allows 30 requests per IP in 10 minutes", async () => {
  const f = appFakes();
  for (let i = 0; i < 30; i++) {
    assert.equal((await token(f, codeGrant("A".repeat(43)), { ip: "203.0.113.9" })).status, 401);
  }
  const limited = await token(f, codeGrant("A".repeat(43)), { ip: "203.0.113.9" });
  assert.equal(limited.status, 429);
  assert.deepEqual(await limited.json(), { error: "slow_down" });
  assert.equal(limited.headers.get("retry-after"), "600");
  assert.equal((await token(f, codeGrant("A".repeat(43)), { ip: "203.0.113.10" })).status, 401);
});

test("before the site is set up, or with the database down, the apps get 503 unavailable", async () => {
  const off = appFakes({ ready: false });
  for (const r of [
    await token(off, codeGrant("A".repeat(43))),
    await appMe(getWith("/api/app/me", `fa1_${"A".repeat(43)}`), off.deps),
    await appSignOut(postJson("/api/app/signout", {}), off.deps),
  ]) {
    assert.equal(r.status, 503);
    assert.deepEqual(await r.json(), { error: "unavailable" });
  }
  const f = appFakes();
  const s = await signedIn(f);
  f.store.fail();
  assert.equal((await token(f, refreshGrant(s.refresh_token))).status, 503);
  assert.equal((await appMe(getWith("/api/app/me", s.access_token), f.deps)).status, 503);
  assert.equal((await appDelete(postJson("/api/app/delete", {}, { bearer: s.access_token }), f.deps)).status, 503);
  f.store.fail(false);
  assert.equal((await token(f, refreshGrant(s.refresh_token))).status, 200, "a 503 never ends the sign-in");
});

test("no log line carries an email, a name, a code or a token", async () => {
  const f = appFakes();
  const a = await signedIn(f);
  await token(f, refreshGrant(a.refresh_token));
  await token(f, refreshGrant(a.refresh_token)); // reuse
  const b = await signedIn(f);
  f.store.fail();
  await token(f, refreshGrant(b.refresh_token));
  await appMe(getWith("/api/app/me", b.access_token), f.deps);
  assert.ok(f.logs.length >= 3);
  const secrets = [a.code, a.access_token, a.refresh_token, b.code, b.access_token, b.refresh_token, "ada", "Ada", "example.com", "google-sub"];
  for (const line of f.logs) for (const s of secrets) assert.ok(!line.includes(s), `${line} carries ${s}`);
});
