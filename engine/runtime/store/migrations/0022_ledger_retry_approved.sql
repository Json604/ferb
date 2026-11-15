-- F4 (P19): the owner's approval to try an action with an unknown outcome once more. The row stays unknown (nobody
-- observed the outcome). retry_approved: the approving owner.ask span; retried_by: the one new attempt it allowed
-- (NULL while unused). Both ADD COLUMN: no table rebuild.
ALTER TABLE action_ledger ADD COLUMN retry_approved TEXT;
ALTER TABLE action_ledger ADD COLUMN retried_by TEXT;
