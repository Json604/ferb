// Sign in with Apple for the apps' sign-in (REL-440, REL-441), with node:crypto only: the ES256 client secret Apple's
// token endpoint wants, and the local check of the RS256 ID token against Apple's published keys. `fetch` and `now`
// (Unix seconds) come in from outside, so tests run it against stand-ins with their own keys.
import { createPublicKey, sign as cryptoSign, timingSafeEqual, verify as cryptoVerify } from "node:crypto";

export const APPLE_ISSUER = "https://appleid.apple.com";
export const APPLE_AUTHORIZE = `${APPLE_ISSUER}/auth/authorize`;
export const APPLE_TOKEN = `${APPLE_ISSUER}/auth/token`;
export const APPLE_KEYS = `${APPLE_ISSUER}/auth/keys`;
const KEYS_SECONDS = 60 * 60;
const SECRET_SECONDS = 5 * 60;
const CLOCK_SKEW = 60;
const TIMEOUT_MS = 10_000;

const b64json = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");

function failure(code) {
  return Object.assign(new Error(`Apple sign-in: ${code}`), { code });
}

function decode(part) {
  try {
    const value = JSON.parse(Buffer.from(part, "base64url").toString("utf8"));
    return value && typeof value === "object" && !Array.isArray(value) ? value : null;
  } catch {
    return null;
  }
}

const sameText = (a, b) => {
  if (typeof a !== "string" || typeof b !== "string") return false;
  const x = Buffer.from(a);
  const y = Buffer.from(b);
  return x.length === y.length && timingSafeEqual(x, y);
};

// The client secret: a JWT signed with the Sign in with Apple key (.p8, P-256), good for five minutes.
export function appleClientSecret({ teamId, servicesId, keyId, privateKey }, now) {
  const input = `${b64json({ alg: "ES256", kid: keyId })}.${b64json({
    iss: teamId,
    iat: now,
    exp: now + SECRET_SECONDS,
    aud: APPLE_ISSUER,
    sub: servicesId,
  })}`;
  const signature = cryptoSign("sha256", Buffer.from(input), { key: privateKey, dsaEncoding: "ieee-p1363" });
  return `${input}.${signature.toString("base64url")}`;
}

// settings: { teamId, servicesId, keyId, privateKey (a KeyObject) }.
export function appleSignIn(settings, { fetch, now }) {
  let cached = null; // { keys, at }

  async function loadKeys() {
    const response = await fetch(APPLE_KEYS, { signal: AbortSignal.timeout(TIMEOUT_MS) });
    if (!response.ok) throw failure(`KEYS_${response.status}`);
    const body = await response.json().catch(() => null);
    if (!Array.isArray(body?.keys)) throw failure("KEYS_INVALID");
    cached = { keys: body.keys, at: now() };
    return cached.keys;
  }

  // Apple's key for `kid`: from the keys fetched in the last hour, else fetched now; a kid the cached keys don't
  // know fetches them once more (Apple rotates its keys).
  async function keyFor(kid) {
    let fresh = false;
    if (!cached || now() - cached.at >= KEYS_SECONDS) {
      await loadKeys();
      fresh = true;
    }
    let jwk = cached.keys.find((k) => k?.kid === kid);
    if (!jwk && !fresh) jwk = (await loadKeys()).find((k) => k?.kid === kid);
    return jwk || null;
  }

  // The ID token's claims once its signature, issuer, audience, times and nonce check out; throws otherwise. The
  // audience is the Services ID for the web sign-in, the iPhone app's bundle id for its own Sign in with Apple
  // (PHN-841). Every failure of the token itself has a code starting TOKEN_; KEYS_ and fetch errors are Apple's side.
  async function verifyIdToken(token, nonce, audience = settings.servicesId) {
    const parts = typeof token === "string" ? token.split(".") : [];
    if (parts.length !== 3 || parts.some((p) => !p)) throw failure("TOKEN_MALFORMED");
    const header = decode(parts[0]);
    if (!header || header.alg !== "RS256" || typeof header.kid !== "string") throw failure("TOKEN_HEADER");
    const jwk = await keyFor(header.kid);
    if (!jwk || jwk.kty !== "RSA" || (jwk.alg && jwk.alg !== "RS256")) throw failure("TOKEN_UNKNOWN_KEY");
    const key = createPublicKey({ key: { kty: "RSA", n: jwk.n, e: jwk.e }, format: "jwk" });
    const signed = Buffer.from(`${parts[0]}.${parts[1]}`);
    if (!cryptoVerify("sha256", signed, key, Buffer.from(parts[2], "base64url"))) throw failure("TOKEN_SIGNATURE");
    const claims = decode(parts[1]);
    const at = now();
    if (!claims) throw failure("TOKEN_CLAIMS");
    if (claims.iss !== APPLE_ISSUER) throw failure("TOKEN_ISSUER");
    if (typeof audience !== "string" || !audience || claims.aud !== audience) throw failure("TOKEN_AUDIENCE");
    if (typeof claims.exp !== "number" || !(at < claims.exp)) throw failure("TOKEN_EXPIRED");
    if (typeof claims.iat !== "number" || claims.iat > at + CLOCK_SKEW) throw failure("TOKEN_ISSUED_AT");
    if (!sameText(claims.nonce, nonce)) throw failure("TOKEN_NONCE");
    if (typeof claims.sub !== "string" || !claims.sub) throw failure("TOKEN_SUBJECT");
    return claims;
  }

  return {
    authorizationUrl: ({ redirectUri, state, nonce }) => {
      const query = [
        ["response_type", "code"],
        ["response_mode", "form_post"],
        ["client_id", settings.servicesId],
        ["redirect_uri", redirectUri],
        ["scope", "name email"],
        ["state", state],
        ["nonce", nonce],
      ]
        .map(([k, v]) => `${k}=${encodeURIComponent(v)}`)
        .join("&");
      return `${APPLE_AUTHORIZE}?${query}`;
    },
    // Trades the code for tokens and returns the checked ID token's claims.
    exchange: async ({ code, redirectUri, nonce }) => {
      const response = await fetch(APPLE_TOKEN, {
        method: "POST",
        headers: { "content-type": "application/x-www-form-urlencoded", accept: "application/json" },
        body: new URLSearchParams({
          client_id: settings.servicesId,
          client_secret: appleClientSecret(settings, now()),
          code,
          grant_type: "authorization_code",
          redirect_uri: redirectUri,
        }).toString(),
        signal: AbortSignal.timeout(TIMEOUT_MS),
      });
      if (!response.ok) throw failure(`TOKEN_ENDPOINT_${response.status}`);
      const body = await response.json().catch(() => null);
      if (typeof body?.id_token !== "string") throw failure("TOKEN_MISSING");
      return verifyIdToken(body.id_token, nonce);
    },
    verifyIdToken,
  };
}

// The name Apple sends in the form's `user` field, only on the first authorization.
export function appleName(user) {
  if (typeof user !== "string" || user.length > 4096) return "";
  try {
    const name = JSON.parse(user)?.name;
    const parts = [name?.firstName, name?.lastName].filter((p) => typeof p === "string" && p.trim());
    return parts.join(" ").trim().slice(0, 200);
  } catch {
    return "";
  }
}
