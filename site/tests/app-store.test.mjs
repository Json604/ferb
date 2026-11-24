// decision: REL-449
// decision: REL-452
// The apps' tables and the SQL that guards them: the file the owner runs by hand matches what the functions make,
// the tables are made once per instance, and the one-time code burn and the refresh rotation are single conditional
// UPDATEs (no read-then-write a racing request could slip between). Statements are checked here as text: no
// Postgres runs in these tests.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { APP_SCHEMA, makeStore } from "../api/_lib/store.js";

const flat = (sql) => sql.replace(/\s+/g, " ").trim();

function recording(answers = () => []) {
  const queries = [];
  const sql = { query: async (text, params) => (queries.push({ text: flat(text), params }), answers(flat(text), params)) };
  return { queries, store: makeStore(sql) };
}

test("the SQL file for Neon's editor holds exactly the statements the functions run", () => {
  const file = readFileSync(new URL("../sql/2026-11-24-app-accounts.sql", import.meta.url), "utf8");
  const statements = file
    .split("\n")
    .filter((line) => !line.trim().startsWith("--"))
    .join("\n")
    .split(";")
    .map(flat)
    .filter(Boolean);
  assert.deepEqual(statements, APP_SCHEMA.map(flat));
  assert.match(APP_SCHEMA.at(-1), /CREATE TABLE IF NOT EXISTS ferb_app_limits/, "the sentinel table is made last");
  for (const s of APP_SCHEMA) assert.match(s, /IF NOT EXISTS/, s);
  for (const table of ["ferb_app_devices", "ferb_app_codes", "ferb_app_tokens"]) {
    assert.match(flat(APP_SCHEMA.find((s) => s.includes(`TABLE IF NOT EXISTS ${table}`))), /REFERENCES ferb_users\(id\) ON DELETE CASCADE/);
  }
});

test("the apps' tables are made once per instance, and skipped when they already exist", async () => {
  const fresh = recording((text) => (text.startsWith("SELECT to_regclass") ? [{ ready: false }] : []));
  await fresh.store.account("id-1");
  await fresh.store.account("id-2");
  const made = fresh.queries.filter((q) => /^(CREATE|ALTER)/.test(q.text));
  assert.deepEqual(made.map((q) => q.text), APP_SCHEMA.map(flat));
  assert.equal(fresh.queries.filter((q) => q.text.startsWith("SELECT to_regclass")).length, 1);

  const existing = recording((text) => (text.startsWith("SELECT to_regclass") ? [{ ready: true }] : []));
  await existing.store.account("id-1");
  assert.deepEqual(existing.queries.map((q) => q.text.split(" ")[0]), ["SELECT", "SELECT"]);
});

test("the code burn and the refresh rotation are single conditional UPDATEs that return the row they won", async () => {
  const { queries, store } = recording((text) => (text.startsWith("SELECT to_regclass") ? [{ ready: true }] : []));
  const at = new Date(0);
  await store.burnCode("a".repeat(64), at);
  await store.useRefresh("b".repeat(64), at);
  const burn = queries.find((q) => q.text.startsWith("UPDATE ferb_app_codes"));
  assert.match(burn.text, /^UPDATE ferb_app_codes SET used_at = \$2 WHERE code_hash = \$1 AND used_at IS NULL AND expires_at > \$2 RETURNING /);
  const rotate = queries.find((q) => q.text.startsWith("UPDATE ferb_app_tokens SET used_at"));
  assert.match(rotate.text, /WHERE token_hash = \$1 AND kind = 'refresh' AND used_at IS NULL AND revoked_at IS NULL AND expires_at > \$2 RETURNING user_id, family$/);
  // Nothing won: a refresh token that had been used revokes its whole family.
  const reuse = queries.find((q) => q.text.startsWith("UPDATE ferb_app_tokens SET revoked_at"));
  assert.match(reuse.text, /WHERE family IN \(SELECT family FROM ferb_app_tokens WHERE token_hash = \$1 AND kind = 'refresh' AND used_at IS NOT NULL\)/);
});

test("the provider column is chosen from a fixed list, never from the request", async () => {
  const { queries, store } = recording((text) => (text.startsWith("SELECT to_regclass") ? [{ ready: true }] : []));
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
