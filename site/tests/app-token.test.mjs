// decision: REL-449
// decision: REL-452
// decision: REL-445
// decision: REL-454
// The apps' side of the sign-in: /api/app/token (code and refresh grants), and the bearer endpoints me, signout and
// delete.
import assert from "node:assert/strict";
import { test } from "node:test";
import { appAppleNative, appDelete, appMe, appSignOut, appToken, REFRESH_GRACE, sha256hex, successorOf } from "../api/_lib/app.js";
import { NOW, SECRET } from "./support.mjs";
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

  // The old refresh token comes back after its grace (stolen, or replayed): the family is revoked, the new tokens
  // included, and the answer says the sign-in was ended.
  f.advance(REFRESH_GRACE);
  const reuse = await token(f, refreshGrant(first.refresh_token));
  assert.equal(reuse.status, 401);
  assert.deepEqual(await reuse.json(), { error: "invalid_grant", reason: "signed_out" });
  assert.equal((await token(f, refreshGrant(second.refresh_token))).status, 401);
  assert.equal((await appMe(getWith("/api/app/me", second.access_token), f.deps)).status, 401);
  assert.equal((await appMe(getWith("/api/app/me", first.access_token), f.deps)).status, 401);
  assert.ok(f.logs.some((l) => /came back after it was rotated/.test(l)));

  // Another sign-in of the same account (another family) is untouched.
  const other = await signedIn(f);
  assert.equal((await appMe(getWith("/api/app/me", other.access_token), f.deps)).status, 200);
});

// decision: REL-453
test("the previous refresh token is answered with the same new pair for 60 seconds, until the new one is used", async () => {
  const f = appFakes();
  const first = await signedIn(f);
  f.advance(1800);
  const second = await (await token(f, refreshGrant(first.refresh_token))).json();
  assert.deepEqual(
    { access: second.access_token, refresh: second.refresh_token },
    successorOf(SECRET, first.refresh_token),
    "the new pair is derived from the old refresh token",
  );
  const tokensBefore = f.store.tokens.size;
  // The reply was lost (a timeout): the app asks again with the same token, 59 seconds later.
  f.advance(REFRESH_GRACE - 1);
  const again = await token(f, refreshGrant(first.refresh_token));
  assert.equal(again.status, 200);
  const third = await again.json();
  assert.equal(third.access_token, second.access_token);
  assert.equal(third.refresh_token, second.refresh_token);
  assert.equal(third.expires_in, 3600 - (REFRESH_GRACE - 1), "the same access token, with the time it has left");
  assert.equal(f.store.tokens.size, tokensBefore, "nothing new is written");
  assert.ok(f.logs.some((l) => /within its grace/.test(l)));
  // Both work, as one sign-in.
  assert.equal((await appMe(getWith("/api/app/me", third.access_token), f.deps)).status, 200);
  const fourth = await (await token(f, refreshGrant(third.refresh_token))).json();
  assert.equal((await appMe(getWith("/api/app/me", fourth.access_token), f.deps)).status, 200);
  // Once the new refresh token was used, the old one coming back even inside the grace is a reuse.
  const late = await token(f, refreshGrant(first.refresh_token));
  assert.equal(late.status, 401);
  assert.deepEqual(await late.json(), { error: "invalid_grant", reason: "signed_out" });
  assert.equal((await appMe(getWith("/api/app/me", fourth.access_token), f.deps)).status, 401);
});

// decision: REL-453
test("a rotation whose answer was never written (the first request died) is finished by the same token within the grace", async () => {
  const f = appFakes();
  const first = await signedIn(f);
  // The first request marked the token used, then failed before the new pair was stored.
  await f.store.useRefresh(sha256hex(first.refresh_token), new Date(f.clock.now * 1000));
  f.advance(10);
  const r = await token(f, refreshGrant(first.refresh_token));
  assert.equal(r.status, 200);
  const body = await r.json();
  assert.equal(body.refresh_token, successorOf(SECRET, first.refresh_token).refresh);
  assert.equal(body.expires_in, 3600 - 10);
  assert.equal((await appMe(getWith("/api/app/me", body.access_token), f.deps)).status, 200);
  // After the grace the same thing is a reuse.
  const g = appFakes();
  const other = await signedIn(g);
  await g.store.useRefresh(sha256hex(other.refresh_token), new Date(g.clock.now * 1000));
  g.advance(REFRESH_GRACE);
  assert.deepEqual(await (await token(g, refreshGrant(other.refresh_token))).json(), { error: "invalid_grant", reason: "signed_out" });
});

// decision: REL-453
test("a refused refresh says why: signed out, expired, the account deleted; an unknown token says nothing more", async () => {
  const f = appFakes();
  const out = await signedIn(f);
  await appSignOut(postJson("/api/app/signout", { refresh_token: out.refresh_token }), f.deps);
  assert.deepEqual(await (await token(f, refreshGrant(out.refresh_token))).json(), { error: "invalid_grant", reason: "signed_out" });

  const old = await signedIn(f);
  f.advance(180 * 86400);
  assert.deepEqual(await (await token(f, refreshGrant(old.refresh_token))).json(), { error: "invalid_grant", reason: "expired" });

  const g = appFakes();
  const mac = await signedIn(g);
  const phone = await signedIn(g, { platform: "android" });
  await appDelete(postJson("/api/app/delete", {}, { bearer: phone.access_token }), g.deps);
  const deleted = await token(g, refreshGrant(mac.refresh_token));
  assert.equal(deleted.status, 401);
  assert.deepEqual(await deleted.json(), { error: "invalid_grant", reason: "account_deleted" });
  // Only hashes of the deleted account's sign-ins are kept, tied to nothing, until they would have expired.
  assert.deepEqual([...g.store.deleted.values()].map((d) => Object.keys(d).sort()), [["expires_at", "token_hash"], ["expires_at", "token_hash"]]);
  assert.ok(g.store.deleted.has(sha256hex(mac.refresh_token)));
  g.advance(180 * 86400 + 1);
  await token(g, refreshGrant(`fr1_${"B".repeat(43)}`)); // a refresh sweeps
  assert.equal(g.store.deleted.size, 0, "removed once they would have expired");

  assert.deepEqual(await (await token(f, refreshGrant(`fr1_${"A".repeat(43)}`))).json(), { error: "invalid_grant" });
});

// decision: REL-453
test("expired codes and tokens are removed on a refresh, a code grant and the iPhone's Apple sign-in too", async () => {
  const f = appFakes();
  const a = await signedIn(f);
  const sweeps = f.store.sweeps;
  f.advance(3601);
  await token(f, refreshGrant(a.refresh_token));
  assert.equal(f.store.sweeps, sweeps + 1);
  assert.equal(f.store.tokens.has(sha256hex(a.access_token)), false, "the expired access token is gone");
  const { code } = await browserSignIn(f);
  const before = f.store.sweeps;
  await token(f, codeGrant(code));
  assert.equal(f.store.sweeps, before + 1);
  await appAppleNative(postJson("/api/app/apple", { identity_token: "x", nonce: "short", device: DEVICE }), f.deps);
  assert.equal(f.store.sweeps, before + 1, "a refused request sweeps nothing");
  // A failed sweep never changes the answer.
  const g = appFakes();
  const b = await signedIn(g);
  g.store.sweep = async () => {
    throw new Error("down");
  };
  assert.equal((await token(g, refreshGrant(b.refresh_token))).status, 200);
  assert.ok(g.logs.some((l) => /not removed/.test(l)));
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
