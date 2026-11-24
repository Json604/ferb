// The user list (REL-433, REL-434): one Postgres table on Neon, through Vercel. One row per Google account that signed
// in to download FERB, holding exactly what the owner listed and nothing else. The table is made on first use, so
// setting up the database needs no SQL by hand.
//
// The apps' accounts (REL-440, REL-441) live in the same table: a row's `id` is the FERB account id, and
// `google_sub`/`apple_sub` link it to the provider accounts. Their own tables (APP_SCHEMA, the same statements as
// site/sql/2026-11-24-app-accounts.sql) are made the first time an app endpoint needs them in each instance, unless the
// database already carries this version's mark (APP_SCHEMA_VERSION, a comment on ferb_app_limits), so a statement
// added later reaches a database made before it. Codes and tokens are only ever stored as SHA-256 hex; the one-time
// code burn and the refresh rotation are single conditional UPDATEs, so two requests racing for the same value can't
// both win.

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
  // A deleted account's sign-ins, as their SHA-256 only and tied to nothing, until they would have expired: so another
  // device of that account is told the account was deleted, not only that its sign-in ended (REL-454).
  `CREATE TABLE IF NOT EXISTS ferb_app_deleted (
  token_hash text PRIMARY KEY,
  expires_at timestamptz NOT NULL
)`,
];

// Raise it whenever APP_SCHEMA changes: a database without this mark runs every statement once more, then is marked.
export const APP_SCHEMA_VERSION = 2;
export const APP_SCHEMA_MARK = `COMMENT ON TABLE ferb_app_limits IS 'ferb app schema ${APP_SCHEMA_VERSION}'`;
const MARK_TEXT = `ferb app schema ${APP_SCHEMA_VERSION}`;

// The sweep runs at most this often per instance, and removes at most SWEEP_ROWS rows of each table per run.
const SWEEP_MS = 10 * 60 * 1000;
const SWEEP_ROWS = 500;

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
  // The apps' tables: skipped when the database carries this version's mark, else every statement in order (each is
  // idempotent) and then the mark. Once per instance.
  let appReady = null;
  const ensureApp = () => {
    appReady ??= (async () => {
      const [found] = await sql.query(
        "SELECT obj_description(to_regclass('ferb_app_limits')::oid, 'pg_class') AS mark",
        [],
      );
      if (found?.mark === MARK_TEXT) return;
      for (const statement of APP_SCHEMA) await sql.query(statement, []);
      await sql.query(APP_SCHEMA_MARK, []);
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
  let lastSweep = null;
  return {
    recordSignIn: ({ email, name, at }) =>
      run(
        `INSERT INTO ferb_users (email, name, first_signin_at) VALUES ($1, $2, $3)
         ON CONFLICT (email) DO UPDATE SET name = EXCLUDED.name`,
        [email, name, at],
      ),
    recordDownload: (email, at) => run("UPDATE ferb_users SET last_download_at = $2 WHERE email = $1", [email, at]),
    // The owner's list: each account with its providers, last sign-in and how many Macs, iPhones and Android phones.
    listUsers: () =>
      app(
        `SELECT u.email, u.name, u.first_signin_at, u.providers, u.last_signin_at,
           count(d.device_id) FILTER (WHERE d.platform = 'mac')::int AS mac,
           count(d.device_id) FILTER (WHERE d.platform = 'ios')::int AS ios,
           count(d.device_id) FILTER (WHERE d.platform = 'android')::int AS android
         FROM ferb_users u LEFT JOIN ferb_app_devices d ON d.user_id = u.id
         GROUP BY u.email ORDER BY u.first_signin_at DESC`,
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
    // The account's live sign-ins are noted in ferb_app_deleted (hashes only) first, then the row goes and its codes,
    // tokens and devices with it.
    deleteUser: async (userId, at) => {
      await app(
        `INSERT INTO ferb_app_deleted (token_hash, expires_at)
         SELECT token_hash, expires_at FROM ferb_app_tokens WHERE user_id = $1 AND kind = 'refresh' AND expires_at > $2
         ON CONFLICT (token_hash) DO NOTHING`,
        [userId, at],
      );
      return (await app("DELETE FROM ferb_users WHERE id = $1 RETURNING id", [userId])).length > 0;
    },

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

    // Idempotent: a rotation answered again within its grace (the same pair, derived from the old refresh token)
    // writes nothing new.
    issueTokens: ({ accessHash, refreshHash, userId, family, deviceId, at, accessExpiresAt, refreshExpiresAt }) =>
      app(
        `INSERT INTO ferb_app_tokens (token_hash, kind, user_id, family, device_id, created_at, expires_at)
         VALUES ($1, 'access', $3, $4, $5, $6, $7), ($2, 'refresh', $3, $4, $5, $6, $8)
         ON CONFLICT (token_hash) DO NOTHING`,
        [accessHash, refreshHash, userId, family, deviceId, at, accessExpiresAt, refreshExpiresAt],
      ),
    // A refresh token presented at `at`:
    //   { user_id, family, used_at }                 still usable: it is used now (used_at = at);
    //   { user_id, family, used_at, replay: true }   used less than `graceSeconds` ago and its successor
    //                                                (`successorHash`) neither used nor revoked: the same rotation again;
    //   { reused: true }                             used before, outside that: its whole family is revoked now;
    //   { ended: "signed_out" | "expired" | "account_deleted" }   why it can't be used; null when it is unknown.
    useRefresh: async (hash, at, { graceSeconds = 0, successorHash = null } = {}) => {
      const used = await one(
        `UPDATE ferb_app_tokens SET used_at = $2
         WHERE token_hash = $1 AND kind = 'refresh' AND used_at IS NULL AND revoked_at IS NULL AND expires_at > $2
         RETURNING user_id, family, used_at`,
        [hash, at],
      );
      if (used) return used;
      if (graceSeconds > 0 && successorHash) {
        const again = await one(
          `SELECT user_id, family, used_at FROM ferb_app_tokens
           WHERE token_hash = $1 AND kind = 'refresh' AND revoked_at IS NULL AND expires_at > $2
             AND used_at IS NOT NULL AND used_at > $2::timestamptz - make_interval(secs => $3::double precision)
             AND NOT EXISTS (SELECT 1 FROM ferb_app_tokens
                             WHERE token_hash = $4 AND (used_at IS NOT NULL OR revoked_at IS NOT NULL))`,
          [hash, at, graceSeconds, successorHash],
        );
        if (again) return { ...again, replay: true };
      }
      const revoked = await app(
        `UPDATE ferb_app_tokens SET revoked_at = $2
         WHERE family IN (SELECT family FROM ferb_app_tokens
                          WHERE token_hash = $1 AND kind = 'refresh' AND used_at IS NOT NULL)
           AND revoked_at IS NULL
         RETURNING token_hash`,
        [hash, at],
      );
      if (revoked.length > 0) return { reused: true };
      const row = await one(
        `SELECT revoked_at IS NOT NULL AS revoked FROM ferb_app_tokens WHERE token_hash = $1 AND kind = 'refresh'`,
        [hash],
      );
      if (row) return { ended: row.revoked ? "signed_out" : "expired" };
      const gone = await one("SELECT 1 AS gone FROM ferb_app_deleted WHERE token_hash = $1 AND expires_at > $2", [hash, at]);
      return gone ? { ended: "account_deleted" } : null;
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

    // Removes what can never be used again: expired codes, tokens and deleted accounts' sign-ins, and rate-limit
    // windows over a day old. Cheap and bounded: at most once in SWEEP_MS per instance, SWEEP_ROWS rows per table.
    sweep: async (at) => {
      const ms = at.getTime();
      if (lastSweep !== null && ms - lastSweep < SWEEP_MS) return;
      lastSweep = ms;
      for (const [table, key, when] of [
        ["ferb_app_codes", "code_hash", "expires_at < $1"],
        ["ferb_app_tokens", "token_hash", "expires_at < $1"],
        ["ferb_app_deleted", "token_hash", "expires_at < $1"],
        ["ferb_app_limits", "key", "window_start < $1::timestamptz - interval '1 day'"],
      ]) {
        await app(
          `DELETE FROM ${table} WHERE ${key} IN (SELECT ${key} FROM ${table} WHERE ${when} LIMIT ${SWEEP_ROWS})`,
          [at],
        );
      }
    },
  };
}

export async function neonStore(databaseUrl) {
  const { neon } = await import("@neondatabase/serverless");
  return makeStore(neon(databaseUrl));
}
