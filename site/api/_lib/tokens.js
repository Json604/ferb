// Signed tokens for the site (REL-434): the sign-in cookie, the short-lived download link and the owner's admin cookie.
// A token is base64url(JSON payload) "." base64url(HMAC-SHA256). The payload names its
// purpose (`p`), so a token made for one use is refused for every other; `x` is its expiry in Unix seconds, when it
// has one. Nothing in a payload is secret: it is signed, not encrypted, and only ever travels to the person it names.
import { createHmac, timingSafeEqual } from "node:crypto";

const DOMAIN = "ferb-site/v1.";

export const DOWNLOAD_SECONDS = 10 * 60;
export const SIGNIN_SECONDS = 10 * 60;
export const ADMIN_SECONDS = 8 * 60 * 60;

function mac(secret, body) {
  return createHmac("sha256", secret).update(DOMAIN + body).digest("base64url");
}

export function sign(secret, payload) {
  const body = Buffer.from(JSON.stringify(payload)).toString("base64url");
  return `${body}.${mac(secret, body)}`;
}

// The payload when `token` was signed with `secret`, is for `purpose` and has not expired at `now` (Unix seconds);
// null otherwise.
export function verify(secret, token, purpose, now) {
  if (typeof token !== "string" || token.length > 4096) return null;
  const parts = token.split(".");
  if (parts.length !== 2 || !parts[0] || !parts[1]) return null;
  const given = Buffer.from(parts[1], "base64url");
  const wanted = Buffer.from(mac(secret, parts[0]), "base64url");
  if (given.length !== wanted.length || !timingSafeEqual(given, wanted)) return null;
  let payload;
  try {
    payload = JSON.parse(Buffer.from(parts[0], "base64url").toString("utf8"));
  } catch {
    return null;
  }
  if (!payload || typeof payload !== "object" || payload.p !== purpose) return null;
  if ("x" in payload && !(typeof payload.x === "number" && now < payload.x)) return null;
  return payload;
}

export const downloadToken = (secret, email, file, now) =>
  sign(secret, { p: "download", e: email, f: file, x: now + DOWNLOAD_SECONDS });

export const adminToken = (secret, email, now) => sign(secret, { p: "admin", e: email, x: now + ADMIN_SECONDS });

export const signinToken = (secret, { state, nonce, verifier, purpose, file }, now) =>
  sign(secret, { p: "signin", s: state, n: nonce, v: verifier, k: purpose, f: file, x: now + SIGNIN_SECONDS });
