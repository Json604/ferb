// Stand-ins for Google, the database and the clock, so the site's functions run here with no network (REL-434).
export const SECRET = "test-secret-0123456789abcdef0123456789abcdef";
export const NOW = 1_800_000_000;
export const ADMIN = "owner@example.com";

export function fakeDeps({ claims = { email: "Ada@Example.com", email_verified: true, name: "Ada L" }, adminEmails = [ADMIN], failStore = false } = {}) {
  const users = new Map();
  const calls = { exchange: [], authorizationUrl: [] };
  const logs = [];
  let n = 0;
  const fail = () => {
    if (failStore) throw Object.assign(new Error("down"), { code: "ECONNREFUSED" });
  };
  const deps = {
    config: { secret: SECRET, adminEmails, siteUrl: "https://ferb.example.com" },
    now: () => NOW,
    log: (m) => logs.push(m),
    oidc: {
      random: () => `r${++n}`,
      challenge: async (v) => `challenge(${v})`,
      authorizationUrl: async (args) => {
        calls.authorizationUrl.push(args);
        return new URL(`https://accounts.example.com/auth?state=${args.state}`);
      },
      exchange: async (url, checks) => {
        calls.exchange.push({ url: String(url), checks });
        if (claims instanceof Error) throw claims;
        return claims;
      },
    },
    store: {
      async recordSignIn({ email, name, at }) {
        fail();
        const u = users.get(email);
        if (u) u.name = name;
        else users.set(email, { email, name, first_signin_at: at, last_download_at: null });
      },
      async recordDownload(email, at) {
        fail();
        const u = users.get(email);
        if (u) u.last_download_at = at;
      },
      async listUsers() {
        fail();
        return [...users.values()];
      },
    },
  };
  return { deps, users, calls, logs };
}

export const get = (path, cookie) =>
  new Request(`https://ferb.example.com${path}`, cookie ? { headers: { cookie } } : undefined);

// The name=value part of each Set-Cookie header.
export function cookies(response) {
  const out = {};
  for (const c of response.headers.getSetCookie()) {
    const [pair] = c.split(";");
    const at = pair.indexOf("=");
    out[pair.slice(0, at)] = { value: pair.slice(at + 1), raw: c };
  }
  return out;
}
