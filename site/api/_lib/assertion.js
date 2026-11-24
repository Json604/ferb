// The pairing assertion (PHN-828): the site signs which FERB account a phone is signed in to, the phone carries it to
// the Mac while pairing, and the Mac's daemon checks it against its pinned public keys before asking the owner.
//
//   fpa1.<b64url(payload)>.<b64url(signature)>
//   payload   {"v":1,"kid":KID,"aud":"ferb-pair","sub":ACCOUNT_ID,"email":EMAIL,"dev":PHONE_ENDPOINT,"iat":S,"exp":S+300}
//   signature ECDSA P-256 / SHA-256 over the ASCII bytes "ferb-pair/v1." + b64url(payload), raw r||s (64 bytes).
//
// The signature is given with a low s (s <= n/2), so each assertion has exactly one valid signature.
import { sign as cryptoSign } from "node:crypto";

export const ASSERTION_SECONDS = 300;
export const ASSERTION_DOMAIN = "ferb-pair/v1.";
const P256_N = 0xffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551n;

// Keys in this exact order: the daemon reads the JSON, but the bytes signed are these.
export const assertionPayload = ({ kid, sub, email, dev, now }) => ({
  v: 1,
  kid,
  aud: "ferb-pair",
  sub,
  email,
  dev,
  iat: now,
  exp: now + ASSERTION_SECONDS,
});

function lowS(signature) {
  const s = BigInt(`0x${signature.subarray(32).toString("hex")}`);
  if (s <= P256_N >> 1n) return signature;
  return Buffer.concat([signature.subarray(0, 32), Buffer.from((P256_N - s).toString(16).padStart(64, "0"), "hex")]);
}

// `key` is the P-256 private KeyObject from APP_ASSERTION_KEY.
export function signAssertion(key, payload) {
  const body = Buffer.from(JSON.stringify(payload)).toString("base64url");
  const signature = cryptoSign("sha256", Buffer.from(ASSERTION_DOMAIN + body, "ascii"), {
    key,
    dsaEncoding: "ieee-p1363",
  });
  return `fpa1.${body}.${lowS(signature).toString("base64url")}`;
}
