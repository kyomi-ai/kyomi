-- SPDX-License-Identifier: AGPL-3.0-or-later
-- Fix refresh_tokens.family_id to match Postgres (KYO-294).
--
-- 00003_refresh_token_rotation.sql added `family_id TEXT` (nullable) and
-- backfilled every existing row with `family_id = token_id`, but never
-- followed up with a NOT NULL step. That left a divergence from
-- 20260216121703_add_refresh_token_rotation.sql (Postgres), which runs the
-- identical backfill and then `ALTER COLUMN family_id SET NOT NULL`. SQLite
-- can't `ALTER COLUMN ... SET NOT NULL` on an existing column, so this
-- requires the same create-copy-drop-rename rebuild KYO-293 used for
-- collections.created_by.
--
-- Unlike that rebuild, no backup/restore dance is needed here: no table in
-- either migrations-sqlite or migrations-postgres declares a foreign key
-- referencing refresh_tokens (verified against the full accumulated
-- schema), so DROP TABLE refresh_tokens cannot cascade-delete rows out of
-- any other table the way collections' rebuild could have wiped
-- collection_dashboards.
--
-- The backfill mirrors Postgres's own rule exactly: any row still missing
-- family_id becomes its own family root. Both live insert paths
-- (kyomi-auth's store_refresh_token and rotate_refresh_token) already bind
-- family_id on every INSERT and take it as a required `&str`, never
-- `Option<&str>` — this migration is closing the gap between "the
-- application always sets it" and "the schema guarantees it", not fixing a
-- currently-reachable NULL.

-- 1. Backfill any remaining NULL family_id — each such token becomes its
--    own family root, identical to 00003's original backfill and
--    Postgres's `UPDATE refresh_tokens SET family_id = token_id WHERE
--    family_id IS NULL`.
UPDATE refresh_tokens SET family_id = token_id WHERE family_id IS NULL;

-- 2. Rebuild refresh_tokens: family_id is now NOT NULL, matching Postgres's
--    `refresh_tokens.family_id` after its SET NOT NULL step. Every other
--    column, the token_id primary key, the user_id FK, and column order
--    are unchanged — family_id/replaced_at are written as plain trailing
--    columns here instead of the `, family_id TEXT, replaced_at TEXT)`
--    ALTER-TABLE artefact PRAGMA table_info reports today, but they occupy
--    the exact same position (last two columns) either way.
CREATE TABLE refresh_tokens_new (
    token_id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    token_hash TEXT NOT NULL,
    demo_token_value TEXT,
    expires_at TEXT NOT NULL,
    is_active INTEGER NOT NULL DEFAULT 1,
    revoked_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_used TEXT,
    user_agent TEXT,
    ip_address TEXT,
    oauth_client_id TEXT,
    country_code TEXT,
    family_id TEXT NOT NULL,
    replaced_at TEXT
);

INSERT INTO refresh_tokens_new (
    token_id, user_id, token_hash, demo_token_value, expires_at, is_active,
    revoked_at, created_at, last_used, user_agent, ip_address,
    oauth_client_id, country_code, family_id, replaced_at
)
SELECT
    token_id, user_id, token_hash, demo_token_value, expires_at, is_active,
    revoked_at, created_at, last_used, user_agent, ip_address,
    oauth_client_id, country_code, family_id, replaced_at
FROM refresh_tokens;

DROP TABLE refresh_tokens;

ALTER TABLE refresh_tokens_new RENAME TO refresh_tokens;

-- 3. Recreate every index DROP TABLE removed (from 00001_baseline.sql and
--    00003_refresh_token_rotation.sql).
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_active ON refresh_tokens(is_active);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_expires ON refresh_tokens(expires_at);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_hash ON refresh_tokens(token_hash);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_user ON refresh_tokens(user_id);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_family ON refresh_tokens(family_id);
