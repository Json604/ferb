// decision: REL-449
// decision: REL-452
// decision: REL-454
// The apps' tables and the SQL that guards them: the file the owner runs by hand matches what the functions make,
// the tables are made once per instance, and the one-time code burn and the refresh rotation are single conditional
// UPDATEs (no read-then-write a racing request could slip between). Statements are checked here as text: no
// Postgres runs in these tests.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { APP_SCHEMA, APP_SCHEMA_MARK, APP_SCHEMA_VERSION, makeStore } from "../api/_lib/store.js";

const flat = (sql) => sql.replace(/\s+/g, " ").trim();

function recording(answers = () => []) {
  const queries = [];
  const sql = { query: async (text, params) => (queries.push({ text: flat(text), params }), answers(flat(text), params)) };
  return { queries, store: makeStore(sql) };
}

const MARK_QUERY = "SELECT obj_description(to_regclass('ferb_app_limits')::oid, 'pg_class') AS mark";
const marked = (mark) => (text) => (text === MARK_QUERY ? [{ mark }] : []);

test("the SQL file for Neon's editor holds exactly the statements the functions run, then the version mark", () => {
  const file = readFileSync(new URL("../sql/2026-11-24-app-accounts.sql", import.meta.url), "utf8");
  const statements = file
    .split("\n")
    .filter((line) => !line.trim().startsWith("--"))
    .join("\n")
    .split(";")
    .map(flat)
    .filter(Boolean);
  assert.deepEqual(statements, [...APP_SCHEMA, APP_SCHEMA_MARK].map(flat));
  for (const s of APP_SCHEMA) assert.match(s, /IF NOT EXISTS/, s);
  for (const table of ["ferb_app_devices", "ferb_app_codes", "ferb_app_tokens"]) {
    assert.match(flat(APP_SCHEMA.find((s) => s.includes(`TABLE IF NOT EXISTS ${table}`))), /REFERENCES ferb_users\(id\) ON DELETE CASCADE/);
  }
  // A deleted account's sign-ins are kept as hashes tied to nothing: no user, no email, no device.
  assert.equal(flat(APP_SCHEMA.find((s) => s.includes("ferb_app_deleted"))), "CREATE TABLE IF NOT EXISTS ferb_app_deleted ( token_hash text PRIMARY KEY, expires_at timestamptz NOT NULL )");
});

// decision: REL-453
test("the apps' tables are made once per instance unless the database carries this version's mark, so later statements reach an older database", async () => {
  for (const mark of [null, "ferb app schema 1", "something else"]) {
    const db = recording(marked(mark));
    await db.store.account("id-1");
    await db.store.account("id-2");
    const made = db.queries.filter((q) => /^(CREATE|ALTER|COMMENT)/.test(q.text));
    assert.deepEqual(made.map((q) => q.text), [...APP_SCHEMA, APP_SCHEMA_MARK].map(flat), String(mark));
    assert.equal(db.queries.filter((q) => q.text === MARK_QUERY).length, 1, "asked once per instance");
  }
  // A database made before ferb_app_deleted existed (it has ferb_app_limits, no mark) gets it.
  const old = recording(marked(null));
  await old.store.account("id-1");
  assert.ok(old.queries.some((q) => q.text.startsWith("CREATE TABLE IF NOT EXISTS ferb_app_deleted")));

  const current = recording(marked(`ferb app schema ${APP_SCHEMA_VERSION}`));
  await current.store.account("id-1");
  assert.deepEqual(current.queries.map((q) => q.text.split(" ")[0]), ["SELECT", "SELECT"]);
});

test("the code burn and the refresh rotation are single conditional UPDATEs that return the row they won", async () => {
  const { queries, store } = recording(marked(`ferb app schema ${APP_SCHEMA_VERSION}`));
  const at = new Date(0);
  await store.burnCode("a".repeat(64), at);
  await store.useRefresh("b".repeat(64), at, { graceSeconds: 60, successorHash: "c".repeat(64) });
  const burn = queries.find((q) => q.text.startsWith("UPDATE ferb_app_codes"));
  assert.match(burn.text, /^UPDATE ferb_app_codes SET used_at = \$2 WHERE code_hash = \$1 AND used_at IS NULL AND expires_at > \$2 RETURNING /);
  const rotate = queries.find((q) => q.text.startsWith("UPDATE ferb_app_tokens SET used_at"));
  assert.match(rotate.text, /WHERE token_hash = \$1 AND kind = 'refresh' AND used_at IS NULL AND revoked_at IS NULL AND expires_at > \$2 RETURNING user_id, family, used_at$/);
  // Nothing won: the same rotation again only within the grace, unrevoked, before its successor was used.
  const again = queries.find((q) => q.text.startsWith("SELECT user_id, family, used_at FROM ferb_app_tokens"));
  assert.match(again.text, /revoked_at IS NULL AND expires_at > \$2 AND used_at IS NOT NULL AND used_at > \$2::timestamptz - make_interval\(secs => \$3::double precision\)/);
  assert.match(again.text, /NOT EXISTS \(SELECT 1 FROM ferb_app_tokens WHERE token_hash = \$4 AND \(used_at IS NOT NULL OR revoked_at IS NOT NULL\)\)/);
  assert.deepEqual(again.params.slice(2), [60, "c".repeat(64)]);
  // Else a refresh token that had been used revokes its whole family.
  const reuse = queries.find((q) => q.text.startsWith("UPDATE ferb_app_tokens SET revoked_at"));
  assert.match(reuse.text, /WHERE family IN \(SELECT family FROM ferb_app_tokens WHERE token_hash = \$1 AND kind = 'refresh' AND used_at IS NOT NULL\)/);
  // Tokens are issued idempotently, so the same rotation answered twice writes nothing new.
  await store.issueTokens({ accessHash: "a", refreshHash: "r", userId: "u", family: "f", deviceId: "d", at, accessExpiresAt: at, refreshExpiresAt: at });
  assert.match(queries.at(-1).text, /ON CONFLICT \(token_hash\) DO NOTHING$/);
});

// decision: REL-453
test("deleting an account notes its live sign-ins' hashes first; the sweep is bounded and runs at most every ten minutes", async () => {
  const { queries, store } = recording(marked(`ferb app schema ${APP_SCHEMA_VERSION}`));
  await store.deleteUser("user-1", new Date(0));
  const [note, gone] = queries.slice(-2);
  assert.match(note.text, /^INSERT INTO ferb_app_deleted \(token_hash, expires_at\) SELECT token_hash, expires_at FROM ferb_app_tokens WHERE user_id = \$1 AND kind = 'refresh' AND expires_at > \$2/);
  assert.match(gone.text, /^DELETE FROM ferb_users WHERE id = \$1/);

  const before = queries.length;
  await store.sweep(new Date(1_000_000));
  const swept = queries.slice(before).map((q) => q.text);
  assert.deepEqual(swept.map((t) => t.split(" ")[2]), ["ferb_app_codes", "ferb_app_tokens", "ferb_app_deleted", "ferb_app_limits"]);
  for (const t of swept) assert.match(t, /LIMIT 500\)$/);
  await store.sweep(new Date(1_000_000 + 9 * 60 * 1000));
  assert.equal(queries.length, before + 4, "not again within ten minutes");
  await store.sweep(new Date(1_000_000 + 10 * 60 * 1000));
  assert.equal(queries.length, before + 8);
});

test("the provider column is chosen from a fixed list, never from the request", async () => {
  const { queries, store } = recording(marked(`ferb app schema ${APP_SCHEMA_VERSION}`));
  await store.appUser({ provider: "apple", sub: "s", email: "e", name: "", at: new Date(0) });
  assert.ok(queries.some((q) => /WHERE apple_sub = \$1/.test(q.text)));
  await assert.rejects(store.appUser({ provider: "google_sub = google_sub OR true; --", sub: "s", email: "e", name: "", at: new Date(0) }));
});

test("the download list's own queries are unchanged: the apps' schema never runs for them", async () => {
  const { queries, store } = recording();
  await store.recordSignIn({ email: "ada@example.com", name: "Ada", at: new Date(0) });
  assert.equal(queries.length, 2);
  assert.match(queries[0].text, /^CREATE TABLE IF NOT EXISTS ferb_users/);
});
