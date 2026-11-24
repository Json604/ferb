// The real services behind the site's functions, read from the environment Vercel gives them (names in
// site/.env.example; the values live only in Vercel). Loaded once per function instance.
import { createPrivateKey, randomBytes } from "node:crypto";
import { appleSignIn } from "./apple.js";
import { parseAdminEmails } from "./handlers.js";
import { sendEmail } from "./mail.js";
import { neonStore } from "./store.js";

const GOOGLE = new URL("https://accounts.google.com");

export function readConfig(env) {
  const secret = env.AUTH_SECRET || "";
  if (secret.length < 32 || !env.GOOGLE_CLIENT_ID || !env.GOOGLE_CLIENT_SECRET || !env.DATABASE_URL) return null;
  return {
    secret,
    adminEmails: parseAdminEmails(env.ADMIN_EMAILS),
    siteUrl: (env.SITE_URL || "").replace(/\/+$/, ""),
  };
}

// A P-256 private key from a PEM setting (PKCS#8; a value pasted with literal "\n" works too); null when it's
// missing or isn't one.
function p256Key(value) {
  const pem = String(value || "").replace(/\\n/g, "\n").trim();
  if (!pem) return null;
  try {
    const key = createPrivateKey({ key: pem, format: "pem" });
    return key.asymmetricKeyType === "ec" && key.asymmetricKeyDetails?.namedCurve === "prime256v1" ? key : null;
  } catch {
    return null;
  }
}

// ANDROID_CERT_SHA256: comma-separated SHA-256 certificate fingerprints (AA:BB:...), kept in upper case.
export function parseFingerprints(value) {
  return String(value || "")
    .split(",")
    .map((f) => f.trim().toUpperCase())
    .filter((f) => /^[0-9A-F]{2}(:[0-9A-F]{2}){31}$/.test(f));
}

// The apps' sign-in (REL-440, REL-441). Never null: `ready` says whether AUTH_SECRET and DATABASE_URL are there;
// each provider, the pairing assertion and the mail are null (or false) until all of their own settings are there.
export function readAppConfig(env) {
  const secret = env.AUTH_SECRET || "";
  const appleKey = p256Key(env.APPLE_PRIVATE_KEY);
  const assertionKey = p256Key(env.APP_ASSERTION_KEY);
  const kid = String(env.APP_ASSERTION_KID || "").trim();
  return {
    ready: secret.length >= 32 && Boolean(env.DATABASE_URL),
    secret,
    siteUrl: (env.SITE_URL || "").replace(/\/+$/, ""),
    google: Boolean(env.GOOGLE_CLIENT_ID && env.GOOGLE_CLIENT_SECRET),
    apple:
      env.APPLE_TEAM_ID && env.APPLE_SERVICES_ID && env.APPLE_KEY_ID && appleKey
        ? { teamId: env.APPLE_TEAM_ID, servicesId: env.APPLE_SERVICES_ID, keyId: env.APPLE_KEY_ID, privateKey: appleKey }
        : null,
    assertion: assertionKey && /^[A-Za-z0-9._-]{1,64}$/.test(kid) ? { key: assertionKey, kid } : null,
    mail: env.RESEND_API_KEY && env.EMAIL_FROM ? { from: env.EMAIL_FROM, replyTo: env.EMAIL_REPLY_TO || "" } : null,
    androidCerts: parseFingerprints(env.ANDROID_CERT_SHA256),
  };
}

// Google sign-in through openid-client (OpenID Connect, authorization code with PKCE, state and nonce).
export async function googleSignIn(clientId, clientSecret) {
  const client = await import("openid-client");
  let discovered = null;
  const google = () => {
    discovered ??= client.discovery(GOOGLE, clientId, clientSecret).catch((error) => {
      discovered = null;
      throw error;
    });
    return discovered;
  };
  return {
    random: () => client.randomState(),
    challenge: (verifier) => client.calculatePKCECodeChallenge(verifier),
    authorizationUrl: async ({ redirectUri, state, nonce, challenge }) =>
      client.buildAuthorizationUrl(await google(), {
        redirect_uri: redirectUri,
        scope: "openid email profile",
        state,
        nonce,
        code_challenge: challenge,
        code_challenge_method: "S256",
        prompt: "select_account",
      }),
    exchange: async (currentUrl, { verifier, state, nonce }) => {
      const tokens = await client.authorizationCodeGrant(await google(), currentUrl, {
        pkceCodeVerifier: verifier,
        expectedState: state,
        expectedNonce: nonce,
        idTokenExpected: true,
      });
      return tokens.claims();
    },
  };
}

let loaded = null;

export function deps() {
  loaded ??= (async () => {
    const env = process.env;
    const config = readConfig(env);
    const base = { config, now: () => Math.floor(Date.now() / 1000), log: (m) => console.error(m) };
    if (!config) {
      console.error("site: sign-in is not set up (AUTH_SECRET, GOOGLE_CLIENT_ID, GOOGLE_CLIENT_SECRET, DATABASE_URL)");
      return base;
    }
    return {
      ...base,
      oidc: await googleSignIn(env.GOOGLE_CLIENT_ID, env.GOOGLE_CLIENT_SECRET),
      store: await neonStore(env.DATABASE_URL),
    };
  })().catch((error) => {
    loaded = null;
    throw error;
  });
  return loaded;
}

let appLoaded = null;

// The services behind /api/app/* (and the two pages they need), from the same environment.
export function appDeps() {
  appLoaded ??= (async () => {
    const env = process.env;
    const config = readAppConfig(env);
    const now = () => Math.floor(Date.now() / 1000);
    const log = (m) => console.error(m);
    const base = { config, now, log, random: () => randomBytes(32).toString("base64url") };
    if (!config.ready) {
      log("site: the apps' sign-in is not set up (AUTH_SECRET, DATABASE_URL)");
      return base;
    }
    if (!config.google) log("site: Google sign-in for the apps is not set up (GOOGLE_CLIENT_ID, GOOGLE_CLIENT_SECRET)");
    if (!config.apple) log("site: Apple sign-in is not set up (APPLE_TEAM_ID, APPLE_SERVICES_ID, APPLE_KEY_ID, APPLE_PRIVATE_KEY)");
    if (!config.assertion) log("site: the pairing assertion is not set up (APP_ASSERTION_KEY, APP_ASSERTION_KID)");
    return {
      ...base,
      store: await neonStore(env.DATABASE_URL),
      oidc: config.google ? await googleSignIn(env.GOOGLE_CLIENT_ID, env.GOOGLE_CLIENT_SECRET) : null,
      apple: config.apple ? appleSignIn(config.apple, { fetch: (...a) => globalThis.fetch(...a), now }) : null,
      sendMail: config.mail ? (email, key) => sendEmail(email, { apiKey: env.RESEND_API_KEY, key }) : null,
    };
  })().catch((error) => {
    appLoaded = null;
    throw error;
  });
  return appLoaded;
}
