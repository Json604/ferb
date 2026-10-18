-- Fix wave 3 fix round (review minor, I1): an owner preference captured in a chat turn that showed outside content is
-- the owner's words, but it reaches the always-loaded identity only once the owner confirms it (an identity proposal).
ALTER TABLE memory_items ADD COLUMN identity_held INTEGER NOT NULL DEFAULT 0;
