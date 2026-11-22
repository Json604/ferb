// The user list (REL-433, REL-434): one Postgres table on Neon, through Vercel. One row per Google account that signed
// in to download FERB, holding exactly what the owner listed and nothing else. The table is made on first use, so
// setting up the database needs no SQL by hand.
//
// A row's `unsubscribed` is only ever set to true here: signing in again never subscribes someone who left.

export const SCHEMA = `CREATE TABLE IF NOT EXISTS ferb_users (
  email text PRIMARY KEY,
  name text NOT NULL DEFAULT '',
  first_signin_at timestamptz NOT NULL,
  last_download_at timestamptz,
  unsubscribed boolean NOT NULL DEFAULT false
)`;

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
  return {
    recordSignIn: ({ email, name, at }) =>
      run(
        `INSERT INTO ferb_users (email, name, first_signin_at) VALUES ($1, $2, $3)
         ON CONFLICT (email) DO UPDATE SET name = EXCLUDED.name`,
        [email, name, at],
      ),
    recordDownload: (email, at) => run("UPDATE ferb_users SET last_download_at = $2 WHERE email = $1", [email, at]),
    unsubscribe: (email) => run("UPDATE ferb_users SET unsubscribed = true WHERE email = $1", [email]),
    listUsers: () =>
      run(
        `SELECT email, name, first_signin_at, last_download_at, unsubscribed
         FROM ferb_users ORDER BY first_signin_at DESC`,
        [],
      ),
    subscribedUsers: () =>
      run("SELECT email, name FROM ferb_users WHERE NOT unsubscribed ORDER BY first_signin_at", []),
  };
}

export async function neonStore(databaseUrl) {
  const { neon } = await import("@neondatabase/serverless");
  return makeStore(neon(databaseUrl));
}
