// decision: REL-440
// decision: REL-441
// Sign in with Apple, checked locally: the ID token against Apple's keys (a test RSA key here), and the ES256 client
// secret (a test P-256 key here).
import assert from "node:assert/strict";
import { generateKeyPairSync, verify as cryptoVerify } from "node:crypto";
import { test } from "node:test";
import { appleClientSecret, appleName, appleSignIn, APPLE_KEYS } from "../api/_lib/apple.js";
import { NOW } from "./support.mjs";
import { APPLE_SERVICES_ID, appleWorld } from "./app-support.mjs";

function world() {
  const clock = { now: NOW };
  const w = appleWorld(() => clock.now);
  w.nonce = "nonce-of-this-sign-in";
  return { w, clock, apple: appleSignIn(w.settings, { fetch: w.fetch, now: () => clock.now }) };
}

const refused = async (promise, code) => {
  await assert.rejects(promise, (error) => {
    assert.equal(error.code, code);
    return true;
  });
};

test("a good ID token gives its claims; a private-relay address and email_verified as a string are fine", async () => {
  const { w, apple } = world();
  const claims = await apple.verifyIdToken(w.idToken(w.goodClaims()), "nonce-of-this-sign-in");
  assert.equal(claims.sub, "001234.apple-sub.0001");
  assert.equal(claims.email, "ada.relay@privaterelay.appleid.com");
  assert.deepEqual(w.fetches, [APPLE_KEYS]);
});

test("the wrong audience, issuer, nonce, an expired or future token, or a missing subject are refused", async () => {
  const { w, apple } = world();
  const nonce = "nonce-of-this-sign-in";
  await refused(apple.verifyIdToken(w.idToken(w.goodClaims({ aud: "com.someone.else" })), nonce), "TOKEN_AUDIENCE");
  await refused(apple.verifyIdToken(w.idToken(w.goodClaims({ aud: [APPLE_SERVICES_ID] })), nonce), "TOKEN_AUDIENCE");
  await refused(apple.verifyIdToken(w.idToken(w.goodClaims({ iss: "https://appleid.apple.com.evil.example" })), nonce), "TOKEN_ISSUER");
  await refused(apple.verifyIdToken(w.idToken(w.goodClaims({ exp: NOW })), nonce), "TOKEN_EXPIRED");
  await refused(apple.verifyIdToken(w.idToken(w.goodClaims({ exp: "later" })), nonce), "TOKEN_EXPIRED");
  await refused(apple.verifyIdToken(w.idToken(w.goodClaims({ iat: NOW + 61 })), nonce), "TOKEN_ISSUED_AT");
  await refused(apple.verifyIdToken(w.idToken(w.goodClaims({ iat: undefined })), nonce), "TOKEN_ISSUED_AT");
  await refused(apple.verifyIdToken(w.idToken(w.goodClaims()), "another-nonce"), "TOKEN_NONCE");
  await refused(apple.verifyIdToken(w.idToken(w.goodClaims({ nonce: undefined })), nonce), "TOKEN_NONCE");
  await refused(apple.verifyIdToken(w.idToken(w.goodClaims({ sub: "" })), nonce), "TOKEN_SUBJECT");
});

test("a bad signature, another algorithm or a malformed token is refused", async () => {
  const { w, apple } = world();
  const nonce = "nonce-of-this-sign-in";
  const good = w.idToken(w.goodClaims());
  const other = generateKeyPairSync("rsa", { modulusLength: 2048 });
  await refused(apple.verifyIdToken(w.idToken(w.goodClaims(), { key: other.privateKey }), nonce), "TOKEN_SIGNATURE");
  const [h, , s] = good.split(".");
  const swapped = Buffer.from(JSON.stringify(w.goodClaims({ sub: "someone-else" }))).toString("base64url");
  await refused(apple.verifyIdToken(`${h}.${swapped}.${s}`, nonce), "TOKEN_SIGNATURE");
  const none = (alg) => `${Buffer.from(JSON.stringify({ alg, kid: w.kid })).toString("base64url")}.${good.split(".")[1]}.${s}`;
  await refused(apple.verifyIdToken(none("none"), nonce), "TOKEN_HEADER");
  await refused(apple.verifyIdToken(none("HS256"), nonce), "TOKEN_HEADER");
  for (const bad of ["", "a.b", "a.b.c.d", `${h}..${s}`, null]) await refused(apple.verifyIdToken(bad, nonce), "TOKEN_MALFORMED");
});

test("an unknown key id fetches Apple's keys once more (Apple rotated them); still unknown is refused", async () => {
  const { w, apple } = world();
  const nonce = "nonce-of-this-sign-in";
  await apple.verifyIdToken(w.idToken(w.goodClaims()), nonce);
  const rotated = generateKeyPairSync("rsa", { modulusLength: 2048 });
  w.keys = [w.jwk(rotated.publicKey, "test-apple-key-2")];
  const claims = await apple.verifyIdToken(w.idToken(w.goodClaims(), { kid: "test-apple-key-2", key: rotated.privateKey }), nonce);
  assert.equal(claims.sub, "001234.apple-sub.0001");
  assert.deepEqual(w.fetches, [APPLE_KEYS, APPLE_KEYS]);
  await refused(apple.verifyIdToken(w.idToken(w.goodClaims(), { kid: "test-apple-key-9" }), nonce), "TOKEN_UNKNOWN_KEY");
  assert.equal(w.fetches.length, 3, "exactly one more fetch for the unknown kid");
});

test("Apple's keys are kept for an hour", async () => {
  const { w, apple, clock } = world();
  const nonce = "nonce-of-this-sign-in";
  await apple.verifyIdToken(w.idToken(w.goodClaims()), nonce);
  clock.now += 3599;
  await apple.verifyIdToken(w.idToken(w.goodClaims()), nonce);
  assert.equal(w.fetches.length, 1);
  clock.now += 1;
  await apple.verifyIdToken(w.idToken(w.goodClaims()), nonce);
  assert.equal(w.fetches.length, 2);
});

test("Apple's key list failing to load refuses the sign-in", async () => {
  const { w } = world();
  const apple = appleSignIn(w.settings, { fetch: async () => new Response("no", { status: 500 }), now: () => NOW });
  await refused(apple.verifyIdToken(w.idToken(w.goodClaims()), "nonce-of-this-sign-in"), "KEYS_500");
});

test("the client secret is an ES256 JWT from the team to Apple, for the services id, good for five minutes", () => {
  const { w } = world();
  const secret = appleClientSecret(w.settings, NOW);
  const [h, p, s] = secret.split(".");
  assert.deepEqual(JSON.parse(Buffer.from(h, "base64url")), { alg: "ES256", kid: "TESTKEY001" });
  assert.deepEqual(JSON.parse(Buffer.from(p, "base64url")), {
    iss: "TESTTEAM01",
    iat: NOW,
    exp: NOW + 300,
    aud: "https://appleid.apple.com",
    sub: APPLE_SERVICES_ID,
  });
  assert.equal(Buffer.from(s, "base64url").length, 64, "raw r||s");
  assert.ok(cryptoVerify("sha256", Buffer.from(`${h}.${p}`), { key: w.clientPublic, dsaEncoding: "ieee-p1363" }, Buffer.from(s, "base64url")));
});

test("the exchange sends the code with a fresh client secret and checks the ID token it gets back", async () => {
  const { w, apple } = world();
  const claims = await apple.exchange({ code: "c0de", redirectUri: "https://ferb.example.com/api/app/callback", nonce: "nonce-of-this-sign-in" });
  assert.equal(claims.sub, "001234.apple-sub.0001");
  const form = w.tokenForms[0];
  assert.deepEqual([...form.keys()], ["client_id", "client_secret", "code", "grant_type", "redirect_uri"]);
  assert.equal(form.get("code"), "c0de");
  assert.ok(cryptoVerify("sha256", Buffer.from(form.get("client_secret").split(".").slice(0, 2).join(".")), { key: w.clientPublic, dsaEncoding: "ieee-p1363" }, Buffer.from(form.get("client_secret").split(".")[2], "base64url")));
  w.tokenStatus = 400;
  await refused(apple.exchange({ code: "c0de", redirectUri: "x", nonce: "nonce-of-this-sign-in" }), "TOKEN_ENDPOINT_400");
});

test("the name comes only from a well-formed user field", () => {
  assert.equal(appleName(JSON.stringify({ name: { firstName: "Ada", lastName: "Lovelace" } })), "Ada Lovelace");
  assert.equal(appleName(JSON.stringify({ name: { firstName: "Ada" } })), "Ada");
  for (const bad of [null, "", "{", "[]", JSON.stringify({ name: { firstName: 7 } }), "x".repeat(5000)]) assert.equal(appleName(bad), "");
});
