-- Refresh token rotation: add family_id and replaced_at columns.
--
-- family_id: groups all tokens from the same login session (rotation chain).
-- replaced_at: timestamp when this token was rotated (replaced by a new one).
--   - NULL means this is the current/active token in the family.
--   - Non-NULL means it was replaced; kept briefly for the grace period.

-- Add family_id as nullable first, then backfill existing rows
ALTER TABLE refresh_tokens ADD COLUMN family_id TEXT;
ALTER TABLE refresh_tokens ADD COLUMN replaced_at TEXT;

-- Backfill: each existing token is its own family root
UPDATE refresh_tokens SET family_id = token_id WHERE family_id IS NULL;

-- Index on family_id for O(1) family revocation
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_family ON refresh_tokens(family_id);
