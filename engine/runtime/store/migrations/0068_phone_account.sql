-- PHN-828 (owner, 2026-11-24): a phone pairs only with a Mac signed in to the same FERB account. Each pairing keeps the
-- account it was made under: the account id (`ferb_users.id` on FERB's site, a uuid) from the server's signed
-- statement the phone sent, which the daemon checked against the Mac's own account before the owner was asked. Never
-- an email or a token. NULL: a phone paired before accounts, which FERB removes when the Mac signs in (as any phone
-- of another account when the Mac's account changes).
ALTER TABLE phone_devices ADD COLUMN account_id TEXT;
