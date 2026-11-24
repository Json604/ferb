// decision: REL-450
import assert from "node:assert/strict";
import { test } from "node:test";
import { admin, csvCell, finishSignIn, startSignIn, toCsv } from "../api/_lib/handlers.js";
import { adminToken, downloadToken } from "../api/_lib/tokens.js";
import { readConfig } from "../api/_lib/deps.js";
import { ADMIN, cookies, fakeDeps, get, NOW, SECRET } from "./support.mjs";

async function adminSignIn(claims) {
  const f = fakeDeps({ claims });
  const start = await startSignIn(get("/api/auth/google?for=admin"), f.deps);
  const cookie = `ferb_signin=${cookies(start).ferb_signin.value}`;
  const done = await finishSignIn(get("/api/auth/callback?code=abc&state=r1", cookie), f.deps);
  return { ...f, done };
}

test("without the owner's cookie the list sends you to sign in", async () => {
  const { deps } = fakeDeps();
  for (const cookie of [
    undefined,
    "ferb_admin=garbage",
    `ferb_admin=${adminToken("another-secret-0123456789abcdef01234567", ADMIN, NOW)}`,
    `ferb_admin=${adminToken(SECRET, ADMIN, NOW - 8 * 3600)}`,
    `ferb_admin=${adminToken(SECRET, "someone@example.com", NOW)}`,
    `ferb_admin=${downloadToken(SECRET, ADMIN, "FERB-AppleSilicon.dmg", NOW)}`,
  ]) {
    const r = await admin(get("/api/admin", cookie), deps);
    assert.equal(r.status, 303, String(cookie));
    assert.equal(r.headers.get("location"), "/api/auth/google?for=admin");
  }
});

test("an owner's cookie stops working once the email leaves ADMIN_EMAILS", async () => {
  const { deps } = fakeDeps({ adminEmails: [] });
  const r = await admin(get("/api/admin", `ferb_admin=${adminToken(SECRET, ADMIN, NOW)}`), deps);
  assert.equal(r.status, 303);
});

test("only a verified Google account named in ADMIN_EMAILS gets the owner's cookie, and it is never recorded as a user", async () => {
  const other = await adminSignIn({ email: "someone@example.com", email_verified: true, name: "S" });
  assert.equal(other.done.status, 403);
  assert.equal(cookies(other.done).ferb_admin, undefined);

  const unverified = await adminSignIn({ email: ADMIN, email_verified: false, name: "O" });
  assert.equal(unverified.done.status, 403);
  assert.equal(cookies(unverified.done).ferb_admin, undefined);

  const owner = await adminSignIn({ email: "Owner@Example.com", email_verified: true, name: "O" });
  assert.equal(owner.done.status, 303);
  assert.equal(owner.done.headers.get("location"), "/api/admin");
  const c = cookies(owner.done).ferb_admin;
  assert.match(c.raw, /HttpOnly/);
  assert.match(c.raw, /Path=\/api\/admin;/);
  assert.equal(owner.users.size, 0);

  const list = await admin(get("/api/admin", `ferb_admin=${c.value}`), owner.deps);
  assert.equal(list.status, 200);
});

// decision: REL-453
test("the list shows each account's providers, last sign-in and devices, escapes what Google sent; the CSV defuses formulas", async () => {
  const { deps, users } = fakeDeps();
  users.set("a@example.com", {
    email: "a@example.com",
    name: "<script>x</script>",
    first_signin_at: new Date((NOW - 86400) * 1000),
    providers: ["google", "apple"],
    last_signin_at: new Date(NOW * 1000),
    mac: 1,
    ios: 0,
    android: 2,
  });
  users.set("b@example.com", { email: "b@example.com", name: '=HYPERLINK("http://x")', first_signin_at: new Date((NOW - 90 * 86400) * 1000), providers: [], last_signin_at: null, mac: 0, ios: 0, android: 0 });
  const cookie = `ferb_admin=${adminToken(SECRET, ADMIN, NOW)}`;

  const html = await (await admin(get("/api/admin", cookie), deps)).text();
  assert.match(html, /2 people · 1 signed in during the last 30 days/);
  assert.doesNotMatch(html, /downloaded|last download/);
  assert.match(html, /<th>provider<\/th><th>first sign-in<\/th><th>last sign-in<\/th><th>devices<\/th>/);
  assert.match(html, /<td>Google, Apple<\/td>.*<td>Mac, 2 Androids<\/td>/);
  assert.doesNotMatch(html, /<script>/);
  assert.match(html, /&lt;script&gt;/);

  const csv = await admin(get("/api/admin?format=csv", cookie), deps);
  assert.match(csv.headers.get("content-type"), /^text\/csv/);
  assert.match(csv.headers.get("content-disposition"), /attachment/);
  const lines = (await csv.text()).trim().split("\r\n");
  assert.equal(lines[0], "email,name,providers,first_signin_at,last_signin_at,mac,ios,android");
  assert.equal(lines[1], `a@example.com,<script>x</script>,google apple,${new Date((NOW - 86400) * 1000).toISOString()},${new Date(NOW * 1000).toISOString()},1,0,2`);
  assert.equal(lines[2], `b@example.com,"'=HYPERLINK(""http://x"")",,${new Date((NOW - 90 * 86400) * 1000).toISOString()},,0,0,0`);
});

test("CSV cells: quotes, commas, new lines and formula starts", () => {
  assert.equal(csvCell("plain"), "plain");
  assert.equal(csvCell("a,b"), '"a,b"');
  assert.equal(csvCell("a\nb"), '"a\nb"');
  assert.equal(csvCell("+1"), "'+1");
  assert.equal(csvCell("@sum"), "'@sum");
  assert.equal(csvCell(null), "");
  assert.equal(toCsv([]), "email,name,providers,first_signin_at,last_signin_at,mac,ios,android\r\n");
});

test("the site counts as set up only with a long secret, Google's client and the database", () => {
  const env = { AUTH_SECRET: "x".repeat(32), GOOGLE_CLIENT_ID: "id", GOOGLE_CLIENT_SECRET: "s", DATABASE_URL: "postgres://h/db", ADMIN_EMAILS: " Owner@Example.com, ,b@example.com ", SITE_URL: "https://ferb.example.com/" };
  assert.deepEqual(readConfig(env), { secret: "x".repeat(32), adminEmails: ["owner@example.com", "b@example.com"], siteUrl: "https://ferb.example.com" });
  assert.equal(readConfig({ ...env, AUTH_SECRET: "short" }), null);
  for (const k of ["GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET", "DATABASE_URL"]) assert.equal(readConfig({ ...env, [k]: "" }), null, k);
  assert.deepEqual(readConfig({ ...env, ADMIN_EMAILS: undefined }).adminEmails, []);
});
