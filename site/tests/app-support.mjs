// Stand-ins for the apps' sign-in (REL-440, REL-441): Google, Apple (with its own invented test keys), the database
// (an in-memory copy of the SQL in api/_lib/store.js, same rules), the clock and the randomness, so every handler runs
// here with no network.
import { createHash, generateKeyPairSync, sign as cryptoSign, verify as cryptoVerify } from "node:crypto";
import { appleSignIn, APPLE_ISSUER, APPLE_KEYS, APPLE_TOKEN } from "../api/_lib/apple.js";
import { appCallback, appStart, s256 } from "../api/_lib/app.js";
import { cookies, get, NOW, SECRET } from "./support.mjs";

export const SITE = "https://ferb.example.com";
export const APP_STATE = "app-state-0123456789abcdef";
export const VERIFIER = "verifier-0123456789abcdefghijklmnopqrstuvwxyzABCD"; // 49 characters, RFC 7636 alphabet
export const DEVICE = { id: "mac-device-0123456789", platform: "mac", version: "0.28.0 (202611240101)", name: "Ada's MacBook" };
// FERB.app's loopback return names the port it listens on (REL-452).
export const RETURN = { mac: "http://127.0.0.1:52345/callback", ios: "ferb-auth://callback", android: `${SITE}/app/callback` };
export const GOOGLE_CLAIMS = { sub: "google-sub-0001", email: "Ada@Example.com", email_verified: true, name: "Ada L" };
export const APPLE_SERVICES_ID = "com.ferb.signin.test";

const dbDown = () => Object.assign(new Error("down"), { code: "ECONNREFUSED" });
const ms = (d) => d.getTime();

// The database. Each method does what its SQL does, including the cascade on deleting a user.
export function fakeAppStore() {
  const users = new Map();
  const codes = new Map();
  const tokens = new Map();
  const devices = new Map();
  const limits = new Map();
  let ids = 0;
  let failing = false;
  const check = () => {
    if (failing) throw dbDown();
  };
  const fk = (userId) => {
    if (!users.has(userId)) throw Object.assign(new Error("foreign key"), { code: "23503" });
  };
  const pub = (u) => ({ id: u.id, email: u.email, name: u.name, providers: [...u.providers] });
  const touch = (u, { provider, name, at }) => {
    u.last_signin_at = at;
    if (name) u.name = name;
    if (!u.providers.includes(provider)) u.providers.push(provider);
    return pub(u);
  };
  const column = (provider) => ({ google: "google_sub", apple: "apple_sub" })[provider];
  return {
    users,
    codes,
    tokens,
    deviceRows: devices,
    limits,
    fail(value = true) {
      failing = value;
    },
    // A row as the download sign-in (recordSignIn) leaves it.
    addDownloadUser(email, name) {
      const u = { id: `00000000-0000-4000-8000-${String(++ids).padStart(12, "0")}`, email, name, first_signin_at: new Date(0), last_download_at: null, google_sub: null, apple_sub: null, providers: [], last_signin_at: null };
      users.set(u.id, u);
      return u;
    },
    async appUser({ provider, sub, email, name, at }) {
      check();
      const col = column(provider);
      const linked = [...users.values()].find((u) => u[col] === sub);
      if (linked) return touch(linked, { provider, name, at });
      const byEmail = [...users.values()].find((u) => u.email === email);
      if (byEmail) {
        if (byEmail[col] !== null && byEmail[col] !== sub) return null;
        byEmail[col] = sub;
        return touch(byEmail, { provider, name, at });
      }
      const u = this.addDownloadUser(email, name);
      Object.assign(u, { first_signin_at: at, [col]: sub, providers: [provider], last_signin_at: at });
      return pub(u);
    },
    async account(userId) {
      check();
      const u = users.get(userId);
      return u ? pub(u) : null;
    },
    async deleteUser(userId) {
      check();
      const had = users.delete(userId);
      for (const map of [codes, tokens, devices]) for (const [k, v] of map) if (v.user_id === userId) map.delete(k);
      return had;
    },
    async createCode({ hash, userId, platform, redirect, challenge, expiresAt }) {
      check();
      fk(userId);
      if (codes.has(hash)) throw Object.assign(new Error("duplicate"), { code: "23505" });
      codes.set(hash, { code_hash: hash, user_id: userId, platform, redirect, challenge, expires_at: expiresAt, used_at: null });
    },
    async burnCode(hash, at) {
      check();
      const c = codes.get(hash);
      if (!c || c.used_at !== null || !(ms(c.expires_at) > ms(at))) return null;
      c.used_at = at;
      return { user_id: c.user_id, platform: c.platform, challenge: c.challenge };
    },
    async recordDevice({ userId, deviceId, platform, version, at }) {
      check();
      fk(userId);
      const key = `${userId}/${deviceId}`;
      const d = devices.get(key);
      if (d) Object.assign(d, { platform, app_version: version, last_seen_at: at });
      else devices.set(key, { user_id: userId, device_id: deviceId, platform, app_version: version, first_seen_at: at, last_seen_at: at });
    },
    async devices(userId) {
      check();
      return [...devices.values()].filter((d) => d.user_id === userId).sort((a, b) => ms(b.last_seen_at) - ms(a.last_seen_at));
    },
    async issueTokens({ accessHash, refreshHash, userId, family, deviceId, at, accessExpiresAt, refreshExpiresAt }) {
      check();
      fk(userId);
      for (const [hash, kind, expires] of [[accessHash, "access", accessExpiresAt], [refreshHash, "refresh", refreshExpiresAt]]) {
        if (tokens.has(hash)) throw Object.assign(new Error("duplicate"), { code: "23505" });
        tokens.set(hash, { token_hash: hash, kind, user_id: userId, family, device_id: deviceId, created_at: at, expires_at: expires, used_at: null, revoked_at: null });
      }
    },
    async useRefresh(hash, at) {
      check();
      const t = tokens.get(hash);
      if (t && t.kind === "refresh" && t.used_at === null && t.revoked_at === null && ms(t.expires_at) > ms(at)) {
        t.used_at = at;
        return { user_id: t.user_id, family: t.family };
      }
      if (!t || t.kind !== "refresh" || t.used_at === null) return null;
      let revoked = 0;
      for (const x of tokens.values()) {
        if (x.family === t.family && x.revoked_at === null) {
          x.revoked_at = at;
          revoked += 1;
        }
      }
      return revoked > 0 ? { reused: true } : null;
    },
    async accessUser(hash, at) {
      check();
      const t = tokens.get(hash);
      return t && t.kind === "access" && t.revoked_at === null && ms(t.expires_at) > ms(at)
        ? { user_id: t.user_id, family: t.family }
        : null;
    },
    async revokeFamilyOf(hash, at) {
      check();
      const t = tokens.get(hash);
      if (!t) return;
      for (const x of tokens.values()) if (x.family === t.family && x.revoked_at === null) x.revoked_at = at;
    },
    async hit(key, at, seconds) {
      check();
      const r = limits.get(key);
      if (!r) limits.set(key, { window_start: at, count: 1 });
      else if (ms(r.window_start) <= ms(at) - seconds * 1000) Object.assign(r, { window_start: at, count: 1 });
      else r.count += 1;
      const row = limits.get(key);
      return { count: row.count, windowStart: new Date(row.window_start) };
    },
    async release(key) {
      check();
      const r = limits.get(key);
      if (r && r.count > 0) r.count -= 1;
    },
    async sweep(at) {
      check();
      for (const map of [codes, tokens]) for (const [k, v] of map) if (ms(v.expires_at) < ms(at)) map.delete(k);
      for (const [k, v] of limits) if (ms(v.window_start) < ms(at) - 86400_000) limits.delete(k);
    },
  };
}

// Apple, with invented keys made for this test run: its key list, its token endpoint (which checks the client
// secret), and ID tokens signed with the test RSA key.
export function appleWorld(now) {
  const rsa = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const client = generateKeyPairSync("ec", { namedCurve: "prime256v1" });
  const world = {
    kid: "test-apple-key-1",
    rsa,
    clientPublic: client.publicKey,
    settings: { teamId: "TESTTEAM01", servicesId: APPLE_SERVICES_ID, keyId: "TESTKEY001", privateKey: client.privateKey },
    fetches: [],
    tokenForms: [],
    tokenStatus: 200,
    nonce: null,
    claims: {},
    now,
  };
  world.jwk = (publicKey, kid) => ({ ...publicKey.export({ format: "jwk" }), kid, use: "sig", alg: "RS256" });
  world.keys = [world.jwk(rsa.publicKey, world.kid)];
  world.idToken = (claims, { kid = world.kid, key = rsa.privateKey, alg = "RS256" } = {}) => {
    const enc = (v) => Buffer.from(JSON.stringify(v)).toString("base64url");
    const input = `${enc({ alg, kid })}.${enc(claims)}`;
    return `${input}.${cryptoSign("sha256", Buffer.from(input), key).toString("base64url")}`;
  };
  world.goodClaims = (extra = {}) => ({
    iss: APPLE_ISSUER,
    aud: APPLE_SERVICES_ID,
    sub: "001234.apple-sub.0001",
    email: "ada.relay@privaterelay.appleid.com",
    email_verified: "true",
    is_private_email: "true",
    iat: world.now(),
    exp: world.now() + 600,
    nonce: world.nonce,
    ...extra,
  });
  world.fetch = async (url, init = {}) => {
    world.fetches.push(String(url));
    if (url === APPLE_KEYS) return Response.json({ keys: world.keys });
    if (url === APPLE_TOKEN) {
      const form = new URLSearchParams(init.body);
      world.tokenForms.push(form);
      if (world.tokenStatus !== 200) return Response.json({ error: "invalid_grant" }, { status: world.tokenStatus });
      return Response.json({ id_token: world.idToken(world.goodClaims(world.claims)), token_type: "Bearer" });
    }
    throw new Error(`unexpected fetch ${url}`);
  };
  world.client = appleSignIn(world.settings, { fetch: world.fetch, now });
  return world;
}

let sharedApple = null;

// Everything a handler needs. `clock.now` moves with `advance(seconds)`.
export function appFakes({ claims = GOOGLE_CLAIMS, google = true, apple = true, assertion = null, ready = true } = {}) {
  const clock = { now: NOW };
  const now = () => clock.now;
  const store = fakeAppStore();
  const logs = [];
  const calls = { authorizationUrl: [], exchange: [] };
  let n = 0;
  sharedApple ??= appleWorld(now);
  const world = sharedApple;
  Object.assign(world, { fetches: [], tokenForms: [], tokenStatus: 200, nonce: null, claims: {}, now, keys: [world.jwk(world.rsa.publicKey, world.kid)] });
  world.client = appleSignIn(world.settings, { fetch: world.fetch, now });
  const f = {
    clock,
    advance: (seconds) => (clock.now += seconds),
    store,
    logs,
    calls,
    apple: world,
    claims,
  };
  f.deps = {
    config: {
      ready,
      secret: SECRET,
      siteUrl: SITE,
      google,
      apple: apple ? world.settings : null,
      assertion,
      androidCerts: [],
    },
    now,
    log: (m) => logs.push(m),
    random: () => createHash("sha256").update(`random-${++n}`).digest("base64url"),
    oidc: google
      ? {
          authorizationUrl: async (args) => {
            calls.authorizationUrl.push(args);
            return new URL(`https://accounts.example.com/auth?state=${args.state}`);
          },
          exchange: async (url, checks) => {
            calls.exchange.push({ url: String(url), checks });
            if (f.claims instanceof Error) throw f.claims;
            return f.claims;
          },
        }
      : null,
    apple: apple ? world.client : null,
    store,
  };
  return f;
}

export const startUrl = (params) => `/api/app/start?${new URLSearchParams(params)}`;

export const startParams = ({ provider = "google", platform = "mac", verifier = VERIFIER, ...rest } = {}) => ({
  provider,
  platform,
  redirect: RETURN[platform],
  state: APP_STATE,
  code_challenge: s256(verifier),
  code_challenge_method: "S256",
  ...rest,
});

export const withIp = (request, ip) => {
  const headers = new Headers(request.headers);
  headers.set("x-real-ip", ip);
  return new Request(request, { headers });
};

export const formPost = (path, form, cookie) =>
  new Request(`${SITE}${path}`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded", ...(cookie ? { cookie } : {}) },
    body: new URLSearchParams(form).toString(),
  });

export const postJson = (path, body, { bearer, ip } = {}) =>
  new Request(`${SITE}${path}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      ...(bearer ? { authorization: `Bearer ${bearer}` } : {}),
      ...(ip ? { "x-real-ip": ip } : {}),
    },
    body: typeof body === "string" ? body : JSON.stringify(body),
  });

export const getWith = (path, bearer) =>
  new Request(`${SITE}${path}`, bearer ? { headers: { authorization: `Bearer ${bearer}` } } : undefined);

// A whole browser part of a sign-in: start, the provider, the callback. `back` is where the app is sent.
export async function browserSignIn(f, { provider = "google", platform = "mac", verifier = VERIFIER, user } = {}) {
  const start = await appStart(get(startUrl(startParams({ provider, platform, verifier }))), f.deps);
  const cookie = `ferb_app_signin=${cookies(start).ferb_app_signin.value}`;
  const location = new URL(start.headers.get("location"));
  const state = location.searchParams.get("state");
  let done;
  if (provider === "google") {
    done = await appCallback(get(`/api/app/callback?code=google-code&state=${state}`, cookie), f.deps);
  } else {
    f.apple.nonce = location.searchParams.get("nonce");
    done = await appCallback(formPost("/api/app/callback", { code: "apple-code", state, ...(user ? { user } : {}) }, cookie), f.deps);
  }
  const back = new URL(done.headers.get("location"));
  return { start, cookie, location, state, done, back, code: back.searchParams.get("code") };
}

// The pairing assertion's payload when `token` is well formed and signed by `publicKey`; null otherwise. Only the
// format check: the daemon also checks kid, aud, times, dev and the account.
export function verifyAssertion(token, publicKey) {
  const m = /^fpa1\.([A-Za-z0-9_-]+)\.([A-Za-z0-9_-]{86})$/.exec(String(token));
  if (!m) return null;
  const ok = cryptoVerify(
    "sha256",
    Buffer.from(`ferb-pair/v1.${m[1]}`, "ascii"),
    { key: publicKey, dsaEncoding: "ieee-p1363" },
    Buffer.from(m[2], "base64url"),
  );
  return ok ? JSON.parse(Buffer.from(m[1], "base64url").toString("utf8")) : null;
}
