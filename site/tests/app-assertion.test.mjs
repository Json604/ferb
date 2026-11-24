// decision: REL-440
// decision: REL-441
// decision: PHN-828
// The pairing assertion: the site signs which account a phone is signed in to; the Mac's daemon verifies it. The
// vector in fixtures/assertion-vector.json (an invented TEST key) is the one the daemon's own test verifies too.
import assert from "node:assert/strict";
import { createECDH, createHash, createPrivateKey, createPublicKey, generateKeyPairSync } from "node:crypto";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { appAssertion, appToken } from "../api/_lib/app.js";
import { assertionPayload, signAssertion, ASSERTION_DOMAIN } from "../api/_lib/assertion.js";
import { readAppConfig } from "../api/_lib/deps.js";
import { NOW, SECRET } from "./support.mjs";
import { appFakes, browserSignIn, DEVICE, postJson, VERIFIER, verifyAssertion } from "./app-support.mjs";

const vector = JSON.parse(readFileSync(new URL("fixtures/assertion-vector.json", import.meta.url), "utf8"));
const P256_HALF_N = 0xffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551n >> 1n;
const testKey = generateKeyPairSync("ec", { namedCurve: "prime256v1" });

// The vector's invented key: its private scalar is SHA-256 of the vector's label, so no key text is stored anywhere.
function vectorKey() {
  const d = createHash("sha256").update(vector.test_key_label, "utf8").digest();
  const ecdh = createECDH("prime256v1");
  ecdh.setPrivateKey(d);
  const pub = ecdh.getPublicKey();
  return createPrivateKey({
    key: { kty: "EC", crv: "P-256", d: d.toString("base64url"), x: pub.subarray(1, 33).toString("base64url"), y: pub.subarray(33).toString("base64url") },
    format: "jwk",
  });
}

async function signedIn(f) {
  const { code } = await browserSignIn(f);
  const r = await appToken(postJson("/api/app/token", { grant_type: "authorization_code", code, code_verifier: VERIFIER, device: DEVICE }), f.deps);
  return r.json();
}

const ask = (f, body, bearer) => appAssertion(postJson("/api/app/assertion", body, { bearer }), f.deps);

test("the assertion names the account and the phone's endpoint, lasts 300 s, and verifies with the public key", async () => {
  const f = appFakes({ assertion: { key: testKey.privateKey, kid: "test-kid-1" } });
  const { access_token, account } = await signedIn(f);
  const r = await ask(f, { endpoint: "phone-endpoint-abc123" }, access_token);
  assert.equal(r.status, 200);
  assert.equal(r.headers.get("cache-control"), "no-store");
  const body = await r.json();
  assert.equal(body.expires_in, 300);
  assert.match(body.assertion, /^fpa1\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]{86}$/);
  const payload = verifyAssertion(body.assertion, testKey.publicKey);
  assert.deepEqual(payload, {
    v: 1,
    kid: "test-kid-1",
    aud: "ferb-pair",
    sub: account.id,
    email: "ada@example.com",
    dev: "phone-endpoint-abc123",
    iat: NOW,
    exp: NOW + 300,
  });
  assert.deepEqual(Object.keys(payload), ["v", "kid", "aud", "sub", "email", "dev", "iat", "exp"]);
  // Another key, or a changed payload, doesn't verify.
  assert.equal(verifyAssertion(body.assertion, generateKeyPairSync("ec", { namedCurve: "prime256v1" }).publicKey), null);
  const [, , sig] = body.assertion.split(".");
  const changed = Buffer.from(JSON.stringify({ ...payload, dev: "another-phone" })).toString("base64url");
  assert.equal(verifyAssertion(`fpa1.${changed}.${sig}`, testKey.publicKey), null);
});

test("the fixture vector verifies with both forms of its test public key, and the signer reproduces its signing input", () => {
  assert.match(vector._comment, /TEST KEY ONLY/);
  const spki = createPublicKey({ key: Buffer.from(vector.test_public_key_spki_der_b64, "base64"), format: "der", type: "spki" });
  const sec1 = Buffer.from(vector.test_public_key_sec1_uncompressed_b64, "base64");
  assert.equal(sec1.length, 65);
  assert.equal(sec1[0], 4);
  const fromSec1 = createPublicKey({
    key: { kty: "EC", crv: "P-256", x: sec1.subarray(1, 33).toString("base64url"), y: sec1.subarray(33).toString("base64url") },
    format: "jwk",
  });
  for (const key of [spki, fromSec1]) assert.deepEqual(verifyAssertion(vector.assertion, key), vector.payload);
  assert.equal(vector.payload_json, JSON.stringify(vector.payload));
  assert.equal(vector.payload_b64url, Buffer.from(vector.payload_json).toString("base64url"));
  assert.equal(vector.signing_input, ASSERTION_DOMAIN + vector.payload_b64url);
  assert.equal(vector.assertion, `fpa1.${vector.payload_b64url}.${vector.signature_b64url}`);
  assert.equal(vector.payload.exp, vector.payload.iat + 300);

  // The site's signer, with the vector's key and inputs, signs exactly the vector's bytes (ECDSA signatures differ
  // each time; each verifies).
  const key = vectorKey();
  const { kid, sub, email, dev, iat } = vector.payload;
  const again = signAssertion(key, assertionPayload({ kid, sub, email, dev, now: iat }));
  assert.equal(again.split(".")[1], vector.payload_b64url);
  assert.deepEqual(verifyAssertion(again, spki), vector.payload);
  assert.equal(createPublicKey(key).export({ format: "der", type: "spki" }).toString("base64"), vector.test_public_key_spki_der_b64);
});

test("signatures have a low s, so each assertion has one valid signature", () => {
  const sigS = (token) => BigInt(`0x${Buffer.from(token.split(".")[2], "base64url").subarray(32).toString("hex")}`);
  assert.ok(sigS(vector.assertion) <= P256_HALF_N);
  for (let i = 0; i < 40; i++) {
    const token = signAssertion(testKey.privateKey, assertionPayload({ kid: "k", sub: "s", email: "e", dev: `d${i}`, now: NOW }));
    assert.ok(sigS(token) <= P256_HALF_N);
    assert.ok(verifyAssertion(token, testKey.publicKey));
  }
});

test("without the signing key it's 503; without a live access token 401; a bad endpoint 400; 20 per 10 minutes per account", async () => {
  const off = appFakes();
  const s = await signedIn(off);
  const none = await ask(off, { endpoint: "phone" }, s.access_token);
  assert.equal(none.status, 503);
  assert.deepEqual(await none.json(), { error: "unavailable" });

  const f = appFakes({ assertion: { key: testKey.privateKey, kid: "test-kid-1" } });
  const { access_token } = await signedIn(f);
  assert.equal((await ask(f, { endpoint: "phone" })).status, 401);
  assert.equal((await ask(f, { endpoint: "phone" }, `fa1_${"A".repeat(43)}`)).status, 401);
  for (const body of [{}, { endpoint: "" }, { endpoint: "x".repeat(201) }, { endpoint: "new\nline" }, { endpoint: 7 }, "nope"]) {
    const r = await ask(f, body, access_token);
    assert.equal(r.status, 400, JSON.stringify(body));
  }
  assert.equal((await ask(f, { endpoint: "x".repeat(200) }, access_token)).status, 200);
  for (let i = 1; i < 20; i++) assert.equal((await ask(f, { endpoint: "phone" }, access_token)).status, 200, `#${i + 1}`);
  const limited = await ask(f, { endpoint: "phone" }, access_token);
  assert.equal(limited.status, 429);
  assert.deepEqual(await limited.json(), { error: "slow_down" });
  assert.equal(limited.headers.get("retry-after"), "600");
});

test("the settings: the key is a P-256 PKCS#8 PEM (pasted with \\n escapes too) with a plain key id; anything else is off", () => {
  const pem = vectorKey().export({ format: "pem", type: "pkcs8" });
  const base = { AUTH_SECRET: SECRET, DATABASE_URL: "postgres://test" };
  const on = readAppConfig({ ...base, APP_ASSERTION_KEY: pem, APP_ASSERTION_KID: "test-2026-11" });
  assert.equal(on.assertion.kid, "test-2026-11");
  assert.equal(readAppConfig({ ...base, APP_ASSERTION_KEY: pem.replace(/\n/g, "\\n"), APP_ASSERTION_KID: "k1" }).assertion.kid, "k1");
  const rsa = generateKeyPairSync("rsa", { modulusLength: 2048 }).privateKey.export({ format: "pem", type: "pkcs8" });
  const p384 = generateKeyPairSync("ec", { namedCurve: "secp384r1" }).privateKey.export({ format: "pem", type: "pkcs8" });
  for (const env of [
    { APP_ASSERTION_KEY: pem },
    { APP_ASSERTION_KEY: pem, APP_ASSERTION_KID: "has space" },
    { APP_ASSERTION_KEY: "not a key", APP_ASSERTION_KID: "k1" },
    { APP_ASSERTION_KEY: rsa, APP_ASSERTION_KID: "k1" },
    { APP_ASSERTION_KEY: p384, APP_ASSERTION_KID: "k1" },
  ]) {
    assert.equal(readAppConfig({ ...base, ...env }).assertion, null);
  }
});
