// The real services behind the site's functions, read from the environment Vercel gives them (names in
// site/.env.example; the values live only in Vercel). Loaded once per function instance.
import { parseAdminEmails } from "./handlers.js";
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

// Google sign-in through openid-client (OpenID Connect, authorization code with PKCE, state and nonce).
async function googleSignIn(clientId, clientSecret) {
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
