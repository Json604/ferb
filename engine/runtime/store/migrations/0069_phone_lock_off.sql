-- PHN-846 (owner, 2026-11-25): a phone whose app lock the owner turned off opens chat and tasks without its fingerprint.
-- The Mac keeps, per paired phone, whether the owner turned it off: set only by the phone's call signed with its
-- fingerprint (or its screen lock on a phone without one), cleared by a call that needs no proof (only ever stricter),
-- by removing the phone, and by pairing again (a new row starts at 0). Money, deleting, secrets and files keep their
-- proofs whatever it says.
ALTER TABLE phone_devices ADD COLUMN lock_off INTEGER NOT NULL DEFAULT 0;
