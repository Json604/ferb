// The user list (REL-433, REL-434): one Postgres table on Neon, through Vercel. One row per Google account that signed
// in to download FERB, holding exactly what the owner listed and nothing else. The table is made on first use, so
// setting up the database needs no SQL by hand.
//
// The apps' accounts (REL-440, REL-441) live in the same table: a row's `id` is the FERB account id, and
// `google_sub`/`apple_sub` link it to the provider accounts. Their own tables (APP_SCHEMA, the same statements as
// site/sql/2026-11-24-app-accounts.sql) are made the first time an app endpoint needs them. Codes and tokens are only
// ever stored as SHA-256 hex; the one-time code burn and the refresh rotation are single conditional UPDATEs, so two
// requests racing for the same value can't both win.

export const SCHEMA = `CREATE TABLE IF NOT EXISTS ferb_users (
  email text PRIMARY KEY,
  name text NOT NULL DEFAULT '',
  first_signin_at timestamptz NOT NULL,
  last_download_at timestamptz
)`;

export const APP_SCHEMA = [
  SCHEMA,
  "ALTER TABLE ferb_users ADD COLUMN IF NOT EXISTS id uuid NOT NULL DEFAULT gen_random_uuid()",
  "CREATE UNIQUE INDEX IF NOT EXISTS ferb_users_id ON ferb_users(id)",
  "ALTER TABLE ferb_users ADD COLUMN IF NOT EXISTS google_sub text",
  "ALTER TABLE ferb_users ADD COLUMN IF NOT EXISTS apple_sub text",
  "CREATE UNIQUE INDEX IF NOT EXISTS ferb_users_google_sub ON ferb_users(google_sub) WHERE google_sub IS NOT NULL",
  "CREATE UNIQUE INDEX IF NOT EXISTS ferb_users_apple_sub ON ferb_users(apple_sub) WHERE apple_sub IS NOT NULL",
  "ALTER TABLE ferb_users ADD COLUMN IF NOT EXISTS providers text[] NOT NULL DEFAULT '{}'",
  "ALTER TABLE ferb_users ADD COLUMN IF NOT EXISTS last_signin_at timestamptz",
  `CREATE TABLE IF NOT EXISTS ferb_app_devices (
  user_id uuid NOT NULL REFERENCES ferb_users(id) ON DELETE CASCADE,
  device_id text NOT NULL,
  platform text NOT NULL CHECK (platform IN ('mac', 'ios', 'android')),
  app_version text NOT NULL,
  first_seen_at timestamptz NOT NULL,
  last_seen_at timestamptz NOT NULL,
  PRIMARY KEY (user_id, device_id)
)`,
  `CREATE TABLE IF NOT EXISTS ferb_app_codes (
  code_hash text PRIMARY KEY,
  user_id uuid NOT NULL REFERENCES ferb_users(id) ON DELETE CASCADE,
  platform text NOT NULL CHECK (platform IN ('mac', 'ios', 'android')),
  redirect text NOT NULL,
  challenge text NOT NULL,
  expires_at timestamptz NOT NULL,
  used_at timestamptz
)`,
  "CREATE INDEX IF NOT EXISTS ferb_app_codes_user ON ferb_app_codes(user_id)",
  `CREATE TABLE IF NOT EXISTS ferb_app_tokens (
  token_hash text PRIMARY KEY,
  kind text NOT NULL CHECK (kind IN ('access', 'refresh')),
  user_id uuid NOT NULL REFERENCES ferb_users(id) ON DELETE CASCADE,
  family text NOT NULL,
  device_id text NOT NULL,
  created_at timestamptz NOT NULL,
  expires_at timestamptz NOT NULL,
  used_at timestamptz,
  revoked_at timestamptz
)`,
  "CREATE INDEX IF NOT EXISTS ferb_app_tokens_family ON ferb_app_tokens(family)",
  "CREATE INDEX IF NOT EXISTS ferb_app_tokens_user ON ferb_app_tokens(user_id)",
  `CREATE TABLE IF NOT EXISTS ferb_app_limits (
  key text PRIMARY KEY,
  window_start timestamptz NOT NULL,
  count integer NOT NULL
)`,
];

// Which column links a row to each provider's account. Never built from a request.
const SUB = { google: "google_sub", apple: "apple_sub" };

// `sql` is the Neon driver's query function (sql.query(text, params)); tests may pass a stand-in.
export function makeStore(sql) {
  let ready = null;
  const ensure = () => {
    ready ??= sql.query(SCHEMA, []).catch((error) => {
      ready = null;
      throw error;
    });
    return ready;
  };
  const run = async (query, params) => {
    await ensure();
    return sql.query(query, params);
  };
  // The apps' tables: skipped once the last of them exists, else every statement in order (each is idempotent).
  let appReady = null;
  const ensureApp = () => {
    appReady ??= (async () => {
      const [found] = await sql.query("SELECT to_regclass('ferb_app_limits') IS NOT NULL AS ready", []);
      if (found?.ready) return;
      for (const statement of APP_SCHEMA) await sql.query(statement, []);
    })().catch((error) => {
      appReady = null;
      throw error;
    });
    return appReady;
  };
  const app = async (query, params) => {
    await ensureApp();
    return sql.query(query, params);
  };
  const one = async (query, params) => (await app(query, params))[0] || null;
  return {
    recordSignIn: ({ email, name, at }) =>
      run(
        `INSERT INTO ferb_users (email, name, first_signin_at) VALUES ($1, $2, $3)
         ON CONFLICT (email) DO UPDATE SET name = EXCLUDED.name`,
        [email, name, at],
      ),
    recordDownload: (email, at) => run("UPDATE ferb_users SET last_download_at = $2 WHERE email = $1", [email, at]),
    listUsers: () =>
      run(
        `SELECT email, name, first_signin_at, last_download_at
         FROM ferb_users ORDER BY first_signin_at DESC`,
        [],
      ),

    // The account for a provider's verified sign-in: the row already linked to that provider account, else the row
    // with that email (which gains the link), else a new row. null when the email's row is linked to a different
    // account of the same provider.
    appUser: async ({ provider, sub, email, name, at }) => {
      const column = SUB[provider];
      if (!column) throw new Error("unknown provider");
      const linked = await one(
        `UPDATE ferb_users SET last_signin_at = $2,
           name = CASE WHEN $3::text <> '' THEN $3::text ELSE name END,
           providers = CASE WHEN $4::text = ANY(providers) THEN providers ELSE array_append(providers, $4::text) END
         WHERE ${column} = $1
         RETURNING id, email, name, providers`,
        [sub, at, name, provider],
      );
      if (linked) return linked;
      return one(
        `INSERT INTO ferb_users (email, name, first_signin_at, ${column}, providers, last_signin_at)
         VALUES ($1, $2, $3, $4, ARRAY[$5::text], $3)
         ON CONFLICT (email) DO UPDATE SET
           ${column} = EXCLUDED.${column},
           name = CASE WHEN EXCLUDED.name <> '' THEN EXCLUDED.name ELSE ferb_users.name END,
           providers = CASE WHEN $5::text = ANY(ferb_users.providers) THEN ferb_users.providers
                            ELSE array_append(ferb_users.providers, $5::text) END,
           last_signin_at = EXCLUDED.last_signin_at
         WHERE ferb_users.${column} IS NULL OR ferb_users.${column} = EXCLUDED.${column}
         RETURNING id, email, name, providers`,
        [email, name, at, sub, provider],
      );
    },
    account: (userId) => one("SELECT id, email, name, providers FROM ferb_users WHERE id = $1", [userId]),
    deleteUser: async (userId) => (await app("DELETE FROM ferb_users WHERE id = $1 RETURNING id", [userId])).length > 0,

    createCode: ({ hash, userId, platform, redirect, challenge, expiresAt }) =>
      app(
        `INSERT INTO ferb_app_codes (code_hash, user_id, platform, redirect, challenge, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6)`,
        [hash, userId, platform, redirect, challenge, expiresAt],
      ),
    // Burns the code whatever comes next: a wrong verifier still uses it up.
    burnCode: (hash, at) =>
      one(
        `UPDATE ferb_app_codes SET used_at = $2
         WHERE code_hash = $1 AND used_at IS NULL AND expires_at > $2
         RETURNING user_id, platform, challenge`,
        [hash, at],
      ),

    recordDevice: ({ userId, deviceId, platform, version, at }) =>
      app(
        `INSERT INTO ferb_app_devices (user_id, device_id, platform, app_version, first_seen_at, last_seen_at)
         VALUES ($1, $2, $3, $4, $5, $5)
         ON CONFLICT (user_id, device_id) DO UPDATE SET
           platform = EXCLUDED.platform, app_version = EXCLUDED.app_version, last_seen_at = EXCLUDED.last_seen_at`,
        [userId, deviceId, platform, version, at],
      ),
    devices: (userId) =>
      app(
        `SELECT platform, app_version, first_seen_at, last_seen_at FROM ferb_app_devices
         WHERE user_id = $1 ORDER BY last_seen_at DESC`,
        [userId],
      ),

    issueTokens: ({ accessHash, refreshHash, userId, family, deviceId, at, accessExpiresAt, refreshExpiresAt }) =>
      app(
        `INSERT INTO ferb_app_tokens (token_hash, kind, user_id, family, device_id, created_at, expires_at)
         VALUES ($1, 'access', $3, $4, $5, $6, $7), ($2, 'refresh', $3, $4, $5, $6, $8)`,
        [accessHash, refreshHash, userId, family, deviceId, at, accessExpiresAt, refreshExpiresAt],
      ),
    // The refresh token's family when it was still usable (it is now used); else, when it had been used before,
    // its whole family is revoked ({ reused: true }); else null.
    useRefresh: async (hash, at) => {
      const used = await one(
        `UPDATE ferb_app_tokens SET used_at = $2
         WHERE token_hash = $1 AND kind = 'refresh' AND used_at IS NULL AND revoked_at IS NULL AND expires_at > $2
         RETURNING user_id, family`,
        [hash, at],
      );
      if (used) return used;
      const revoked = await app(
        `UPDATE ferb_app_tokens SET revoked_at = $2
         WHERE family IN (SELECT family FROM ferb_app_tokens
                          WHERE token_hash = $1 AND kind = 'refresh' AND used_at IS NOT NULL)
           AND revoked_at IS NULL
         RETURNING token_hash`,
        [hash, at],
      );
      return revoked.length > 0 ? { reused: true } : null;
    },
    accessUser: (hash, at) =>
      one(
        `SELECT user_id, family FROM ferb_app_tokens
         WHERE token_hash = $1 AND kind = 'access' AND revoked_at IS NULL AND expires_at > $2`,
        [hash, at],
      ),
    // Revokes every token of the family the token (access or refresh) belongs to.
    revokeFamilyOf: (hash, at) =>
      app(
        `UPDATE ferb_app_tokens SET revoked_at = $2
         WHERE family IN (SELECT family FROM ferb_app_tokens WHERE token_hash = $1) AND revoked_at IS NULL`,
        [hash, at],
      ),

    // A fixed-window counter: { count, windowStart } after counting this request in the window that holds `at`.
    hit: async (key, at, seconds) => {
      const row = await one(
        `INSERT INTO ferb_app_limits (key, window_start, count) VALUES ($1, $2::timestamptz, 1)
         ON CONFLICT (key) DO UPDATE SET
           window_start = CASE WHEN ferb_app_limits.window_start <= $2::timestamptz - make_interval(secs => $3::double precision)
                               THEN $2::timestamptz ELSE ferb_app_limits.window_start END,
           count = CASE WHEN ferb_app_limits.window_start <= $2::timestamptz - make_interval(secs => $3::double precision)
                        THEN 1 ELSE ferb_app_limits.count + 1 END
         RETURNING window_start, count`,
        [key, at, seconds],
      );
      return { count: Number(row.count), windowStart: new Date(row.window_start) };
    },
    // Gives back one count (a send that failed, or a request another limit refused).
    release: (key) => app("UPDATE ferb_app_limits SET count = count - 1 WHERE key = $1 AND count > 0", [key]),

    // Removes what can never be used again: expired codes and tokens, and rate-limit windows over a day old.
    sweep: async (at) => {
      await app("DELETE FROM ferb_app_codes WHERE expires_at < $1", [at]);
      await app("DELETE FROM ferb_app_tokens WHERE expires_at < $1", [at]);
      await app("DELETE FROM ferb_app_limits WHERE window_start < $1::timestamptz - interval '1 day'", [at]);
    },
  };
}

export async function neonStore(databaseUrl) {
  const { neon } = await import("@neondatabase/serverless");
  return makeStore(neon(databaseUrl));
}
