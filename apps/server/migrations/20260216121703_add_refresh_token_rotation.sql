-- Refresh token rotation: add family_id and replaced_at columns.
--
-- family_id: groups all tokens from the same login session (rotation chain).
-- replaced_at: timestamp when this token was rotated (replaced by a new one).
--   - NULL means this is the current/active token in the family.
--   - Non-NULL means it was replaced; kept briefly for the grace period.

-- Step 1: Add columns (nullable initially for backfill)
ALTER TABLE refresh_tokens ADD COLUMN IF NOT EXISTS family_id VARCHAR(100);
ALTER TABLE refresh_tokens ADD COLUMN IF NOT EXISTS replaced_at TIMESTAMPTZ;

-- Step 2: Backfill existing tokens — each gets its own family (one token = one family)
UPDATE refresh_tokens SET family_id = token_id WHERE family_id IS NULL;

-- Step 3: Make family_id NOT NULL after backfill
ALTER TABLE refresh_tokens ALTER COLUMN family_id SET NOT NULL;

-- Step 4: Index on family_id for O(1) family revocation
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_family ON refresh_tokens (family_id);
