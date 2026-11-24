// decision: REL-436
import assert from "node:assert/strict";
import { test } from "node:test";
import { adminToken, downloadToken, sign, unsubscribeToken, verify } from "../api/_lib/tokens.js";
import { NOW, SECRET } from "./support.mjs";

test("a token verifies with its secret and purpose, and carries its payload", () => {
  const t = downloadToken(SECRET, "ada@example.com", "FERB-AppleSilicon.dmg", NOW);
  assert.deepEqual(verify(SECRET, t, "download", NOW), {
    p: "download",
    e: "ada@example.com",
    f: "FERB-AppleSilicon.dmg",
    x: NOW + 600,
  });
});

test("a changed payload, a changed signature or another secret is refused", () => {
  const t = unsubscribeToken(SECRET, "ada@example.com");
  const [body, mac] = t.split(".");
  const forged = Buffer.from(JSON.stringify({ p: "unsubscribe", e: "bob@example.com" })).toString("base64url");
  assert.equal(verify(SECRET, `${forged}.${mac}`, "unsubscribe", NOW), null);
  assert.equal(verify(SECRET, `${body}.${mac.slice(0, -2)}AA`, "unsubscribe", NOW), null);
  assert.equal(verify(SECRET, `${body}.`, "unsubscribe", NOW), null);
  assert.equal(verify("another-secret-0123456789abcdef0123456789", t, "unsubscribe", NOW), null);
});

test("a token made for one purpose is refused for every other", () => {
  const t = downloadToken(SECRET, "ada@example.com", "FERB-AppleSilicon.dmg", NOW);
  assert.equal(verify(SECRET, t, "unsubscribe", NOW), null);
  assert.equal(verify(SECRET, t, "admin", NOW), null);
  assert.equal(verify(SECRET, unsubscribeToken(SECRET, "ada@example.com"), "admin", NOW), null);
});

test("an expired token is refused; an unsubscribe token never expires", () => {
  const t = adminToken(SECRET, "owner@example.com", NOW);
  assert.ok(verify(SECRET, t, "admin", NOW + 8 * 3600 - 1));
  assert.equal(verify(SECRET, t, "admin", NOW + 8 * 3600), null);
  assert.ok(verify(SECRET, unsubscribeToken(SECRET, "ada@example.com"), "unsubscribe", NOW + 10 * 365 * 86400));
  assert.equal(verify(SECRET, sign(SECRET, { p: "admin", e: "x", x: "later" }), "admin", NOW), null);
});

test("malformed tokens are refused without throwing", () => {
  for (const t of [null, undefined, "", ".", "a.b.c", "not-a-token", "x".repeat(5000), "%%%.%%%"]) {
    assert.equal(verify(SECRET, t, "download", NOW), null);
  }
});
