// decision: REL-433
// decision: REL-436
import assert from "node:assert/strict";
import { test } from "node:test";
import { download, finishSignIn, startSignIn, RELEASES } from "../api/_lib/handlers.js";
import { downloadToken, signinToken } from "../api/_lib/tokens.js";
import { cookies, fakeDeps, get, NOW, SECRET } from "./support.mjs";

async function signIn(deps, query = "?file=FERB-AppleSilicon.dmg") {
  const start = await startSignIn(get(`/api/auth/google${query}`), deps);
  const c = cookies(start).ferb_signin;
  return { start, cookie: `ferb_signin=${c.value}`, raw: c.raw };
}

test("the sign-in sends the person to Google with state, nonce and PKCE, kept in a short HttpOnly cookie", async () => {
  const { deps, calls } = fakeDeps();
  const { start, raw } = await signIn(deps);
  assert.equal(start.status, 303);
  assert.match(start.headers.get("location"), /^https:\/\/accounts\.example\.com\/auth\?state=r1$/);
  assert.deepEqual(calls.authorizationUrl[0], {
    redirectUri: "https://ferb.example.com/api/auth/callback",
    state: "r1",
    nonce: "r2",
    challenge: "challenge(r3)",
  });
  assert.match(raw, /HttpOnly/);
  assert.match(raw, /Secure/);
  assert.match(raw, /Path=\/api\/auth;/);
  assert.match(raw, /Max-Age=600/);
});

test("Google's answer records the person and hands over a short-lived link to the file", async () => {
  const { deps, users, calls } = fakeDeps();
  const { cookie } = await signIn(deps);
  const done = await finishSignIn(get("/api/auth/callback?code=abc&state=r1", cookie), deps);
  assert.equal(done.status, 200);
  assert.deepEqual(calls.exchange[0], {
    url: "https://ferb.example.com/api/auth/callback?code=abc&state=r1",
    checks: { verifier: "r3", state: "r1", nonce: "r2" },
  });
  // The email is kept lower-case; exactly the owner's five fields.
  assert.deepEqual([...users.values()], [
    { email: "ada@example.com", name: "Ada L", first_signin_at: new Date(NOW * 1000), last_download_at: null, unsubscribed: false },
  ]);
  const html = await done.text();
  const link = html.match(/url=(\/download\/FERB-AppleSilicon\.dmg\?t=[^"]+)"/)[1];
  assert.equal(cookies(done).ferb_signin.value, "", "the sign-in cookie is cleared");

  const file = await download(get(link.replace("/download/FERB-AppleSilicon.dmg?", "/api/download?file=FERB-AppleSilicon.dmg&")), deps);
  assert.equal(file.status, 302);
  assert.equal(file.headers.get("location"), `${RELEASES}FERB-AppleSilicon.dmg`);
  assert.deepEqual(users.get("ada@example.com").last_download_at, new Date(NOW * 1000));
});

test("signing in again keeps the first sign-in time and never undoes an unsubscribe", async () => {
  const { deps, users } = fakeDeps();
  for (let i = 0; i < 2; i++) {
    const { cookie } = await signIn(deps);
    await finishSignIn(get("/api/auth/callback?code=abc&state=r1", cookie), deps);
    users.get("ada@example.com").unsubscribed = true;
  }
  assert.equal(users.size, 1);
  assert.equal(users.get("ada@example.com").unsubscribed, true);
});

test("no download without a valid link made for that file", async () => {
  const { deps, users } = fakeDeps();
  const t = downloadToken(SECRET, "ada@example.com", "FERB-AppleSilicon.dmg", NOW);
  const cases = [
    "/api/download?file=FERB-AppleSilicon.dmg",
    "/api/download?file=FERB-AppleSilicon.dmg&t=garbage",
    `/api/download?file=FERB-Intel.dmg&t=${t}`,
    `/api/download?file=..%2Fsecret.dmg&t=${t}`,
    `/api/download?file=FERB-AppleSilicon.dmg&t=${downloadToken(SECRET, "ada@example.com", "FERB-AppleSilicon.dmg", NOW - 600)}`,
    `/api/download?file=FERB-AppleSilicon.dmg&t=${downloadToken("another-secret-0123456789abcdef01234567", "ada@example.com", "FERB-AppleSilicon.dmg", NOW)}`,
  ];
  for (const path of cases) {
    const r = await download(get(path), deps);
    assert.equal(r.status, 303, path);
    assert.equal(r.headers.get("location"), "/#download", path);
  }
  assert.equal(users.size, 0);
});

test("a sign-in that expired, was cancelled or that Google didn't confirm records nobody", async () => {
  const { deps, users } = fakeDeps();
  const expired = `ferb_signin=${signinToken(SECRET, { state: "s", nonce: "n", verifier: "v", purpose: "download", file: "FERB-AppleSilicon.dmg" }, NOW - 601)}`;
  assert.equal((await finishSignIn(get("/api/auth/callback?code=abc&state=s", expired), deps)).status, 400);
  assert.equal((await finishSignIn(get("/api/auth/callback?code=abc&state=s"), deps)).status, 400);
  const { cookie } = await signIn(deps);
  assert.equal((await finishSignIn(get("/api/auth/callback?error=access_denied&state=r1", cookie), deps)).status, 400);

  const failing = fakeDeps({ claims: Object.assign(new Error("state mismatch"), { code: "OAUTH_INVALID_RESPONSE" }) });
  const s = await signIn(failing.deps);
  assert.equal((await finishSignIn(get("/api/auth/callback?code=abc&state=zz", s.cookie), failing.deps)).status, 400);
  assert.equal(users.size + failing.users.size, 0);
});

test("a database that can't be reached never stops the download, and nothing personal is logged", async () => {
  const { deps, logs } = fakeDeps({ failStore: true });
  const { cookie } = await signIn(deps);
  const done = await finishSignIn(get("/api/auth/callback?code=abc&state=r1", cookie), deps);
  assert.equal(done.status, 200);
  const link = (await done.text()).match(/url=\/download\/FERB-AppleSilicon\.dmg\?t=([^"]+)"/)[1];
  const file = await download(get(`/api/download?file=FERB-AppleSilicon.dmg&t=${link}`), deps);
  assert.equal(file.status, 302);
  assert.ok(logs.length >= 2);
  for (const line of logs) assert.doesNotMatch(line, /ada|Ada|example\.com/);
});

test("before the site is set up, sign-in and download say so instead of failing", async () => {
  const deps = { config: null };
  assert.equal((await startSignIn(get("/api/auth/google"), deps)).status, 503);
  assert.equal((await download(get("/api/download?file=FERB-AppleSilicon.dmg"), deps)).status, 503);
});
