-- FERB accounts for the apps (REL-440, REL-441): the site's user list gains an account id and the Google and Apple
-- links, and small tables hold the apps' devices, one-time sign-in codes, tokens, rate limits and a deleted account's
-- ended sign-ins. Every statement is safe to run again. The site's functions make the same tables on first use (api/_lib/store.js keeps the
-- same statements, and a test checks they match this file); this file is for running by hand in Neon's SQL editor.
--
-- Codes and tokens are kept only as SHA-256 (hex) of their value; IP addresses in rate-limit keys only as an HMAC.
-- Deleting a row of ferb_users deletes its devices, codes and tokens with it.

CREATE TABLE IF NOT EXISTS ferb_users (
  email text PRIMARY KEY,
  name text NOT NULL DEFAULT '',
  first_signin_at timestamptz NOT NULL,
  last_download_at timestamptz
);

ALTER TABLE ferb_users ADD COLUMN IF NOT EXISTS id uuid NOT NULL DEFAULT gen_random_uuid();
CREATE UNIQUE INDEX IF NOT EXISTS ferb_users_id ON ferb_users(id);
ALTER TABLE ferb_users ADD COLUMN IF NOT EXISTS google_sub text;
ALTER TABLE ferb_users ADD COLUMN IF NOT EXISTS apple_sub text;
CREATE UNIQUE INDEX IF NOT EXISTS ferb_users_google_sub ON ferb_users(google_sub) WHERE google_sub IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS ferb_users_apple_sub ON ferb_users(apple_sub) WHERE apple_sub IS NOT NULL;
ALTER TABLE ferb_users ADD COLUMN IF NOT EXISTS providers text[] NOT NULL DEFAULT '{}';
ALTER TABLE ferb_users ADD COLUMN IF NOT EXISTS last_signin_at timestamptz;

CREATE TABLE IF NOT EXISTS ferb_app_devices (
  user_id uuid NOT NULL REFERENCES ferb_users(id) ON DELETE CASCADE,
  device_id text NOT NULL,
  platform text NOT NULL CHECK (platform IN ('mac', 'ios', 'android')),
  app_version text NOT NULL,
  first_seen_at timestamptz NOT NULL,
  last_seen_at timestamptz NOT NULL,
  PRIMARY KEY (user_id, device_id)
);

CREATE TABLE IF NOT EXISTS ferb_app_codes (
  code_hash text PRIMARY KEY,
  user_id uuid NOT NULL REFERENCES ferb_users(id) ON DELETE CASCADE,
  platform text NOT NULL CHECK (platform IN ('mac', 'ios', 'android')),
  redirect text NOT NULL,
  challenge text NOT NULL,
  expires_at timestamptz NOT NULL,
  used_at timestamptz
);
CREATE INDEX IF NOT EXISTS ferb_app_codes_user ON ferb_app_codes(user_id);

CREATE TABLE IF NOT EXISTS ferb_app_tokens (
  token_hash text PRIMARY KEY,
  kind text NOT NULL CHECK (kind IN ('access', 'refresh')),
  user_id uuid NOT NULL REFERENCES ferb_users(id) ON DELETE CASCADE,
  family text NOT NULL,
  device_id text NOT NULL,
  created_at timestamptz NOT NULL,
  expires_at timestamptz NOT NULL,
  used_at timestamptz,
  revoked_at timestamptz
);
CREATE INDEX IF NOT EXISTS ferb_app_tokens_family ON ferb_app_tokens(family);
CREATE INDEX IF NOT EXISTS ferb_app_tokens_user ON ferb_app_tokens(user_id);

CREATE TABLE IF NOT EXISTS ferb_app_limits (
  key text PRIMARY KEY,
  window_start timestamptz NOT NULL,
  count integer NOT NULL
);

-- A deleted account's sign-ins, as their SHA-256 only and tied to nothing, until they would have expired: another
-- device of that account is then told the account was deleted.
CREATE TABLE IF NOT EXISTS ferb_app_deleted (
  token_hash text PRIMARY KEY,
  expires_at timestamptz NOT NULL
);

-- Last: the version mark. The functions skip this file's statements while the database carries their version's mark;
-- a database without it gets every statement again (each is safe to repeat), then the mark.
COMMENT ON TABLE ferb_app_limits IS 'ferb app schema 2';
