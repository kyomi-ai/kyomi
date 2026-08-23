-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- KYO-460: repair `datasource_configs.connection_config` rows whose scalar
-- leaves were flattened to JSON strings by the pre-KYO-428 Leptos datasource
-- modal. See the Postgres counterpart
-- (apps/server/migrations/20260823000000_retype_connection_config_scalars.sql)
-- for the full defect history — this file repeats only what differs for
-- SQLite.
--
-- `connection_config` here is `TEXT` holding a JSON document (baseline:
-- migrations-sqlite/00001_baseline.sql:72), not a native JSON column, so
-- this migration goes through the `json1` extension's `json_type()` /
-- `json_extract()` / `json_set()` functions instead of `jsonb_typeof()` /
-- `->`/`->>` / `jsonb_set()`.
--
-- This is a ONE-SHOT repair: KYO-428 already stopped new corruption at the
-- source, so this migration only cleans up the backlog of rows written
-- before that fix shipped.
--
-- Detection is by JSON TYPE, never by value, for the same reason as the
-- Postgres migration: a stored `"5432"` is both a plausible genuine value
-- and indistinguishable from an intentionally non-numeric field by content
-- alone.
--
-- IMPORTANT SQLite-specific quirk, verified empirically against SQLite
-- 3.51.2 before writing this migration (see KYO-451 for another case where
-- this same quirk bit a comparison): `json_type(doc, path)` does NOT return
-- the string `'boolean'` for a genuine JSON boolean. It returns the literal
-- token `'true'` or `'false'` instead:
--
--   sqlite> SELECT json_type('{"secure":true}', '$.secure');
--   true
--   sqlite> SELECT json_type('{"secure":"true"}', '$.secure');
--   text
--
-- That is exactly what makes `json_type(...) = 'text'` a safe and precise
-- test here: it is the ONLY type SQLite reports for a JSON string, so it
-- unambiguously selects the corrupted (string-typed) rows and never matches
-- an already-correct JSON boolean, integer, or absent key. A comparison
-- written the "obvious" way instead — `json_type(...) != 'boolean'` — would
-- be wrong in the opposite direction: since a real boolean's type is
-- literally `'true'`/`'false'`, not `'boolean'`, that comparison would be
-- true for correctly-typed rows too, and this migration would needlessly
-- rewrite (though not corrupt) rows that were never broken.
--
-- Numeric leaves: SQLite's `CAST(text AS INTEGER)` never raises an error —
-- it parses as much of a leading integer as it can and falls back to `0`
-- (verified: `CAST('abc' AS INTEGER)` = `0`, and a 26-digit input clamps to
-- `9223372036854775807` rather than overflowing) — so, unlike the Postgres
-- migration, there is no evaluation-order hazard here and the `WHERE`
-- clause can gate the cast directly with a plain `AND` chain. The digit
-- check still has to be spelled out with `GLOB`, since SQLite has no
-- regular-expression operator built in: `GLOB '[0-9]*'` anchors the first
-- character, `NOT GLOB '*[^0-9]*'` rejects any non-digit anywhere in the
-- rest of the string, and `LENGTH(...) BETWEEN 1 AND 5` bounds it to what a
-- `1..=65535` port can possibly need (this also empirically double-checked
-- against the diagnosed overflow case, e.g. a 30-digit string is excluded
-- purely by the length bound before any cast happens).
--
-- Boolean leaves convert only when `json_extract(...)` is EXACTLY (case
-- sensitive) the SQL text `'true'` or `'false'` — matching what `serde_qs`,
-- the codec that produced this corruption, always emits (its `Display` for
-- Rust `bool` is always lowercase, so no other casing could have been
-- written by the code path this migration repairs). The replacement value
-- is written back with `json(json_extract(connection_config, '$.key'))`:
-- `json_extract` on a JSON string returns its SQL text content (`'true'` /
-- `'false'`, not `'"true"'`), and wrapping that text in `json(...)` parses
-- it as a JSON literal rather than re-quoting it as a JSON string, which is
-- what `json_set` needs to embed an actual JSON boolean instead of a
-- string. This is safe to call unconditionally in `SET` because `SET` only
-- evaluates for rows the `WHERE` clause already matched — it is never
-- reached for a row where `json_extract(...)` is something other than
-- `'true'`/`'false'` (confirmed: `SELECT json('maybe')` raises `malformed
-- JSON`, but no row in this migration's test coverage reaches that call
-- with unvalidated input).
--
-- A row missing a key was never corrupted by this bug (the modal only
-- inserts `port` when it successfully parses one, and the SSH keys only
-- exist when the SSH panel was ever touched) — `json_type()` returns SQL
-- `NULL` for a missing path, so the `= 'text'` guard is false and the row
-- is left alone; nothing here ever adds a key. Rows that are already
-- correctly typed are never written at all (excluded by `WHERE` before any
-- `UPDATE` touches the row), so they come out of this migration
-- byte-identical.

-- port (numeric, provider connection host port)
UPDATE datasource_configs
SET connection_config = json_set(
    connection_config,
    '$.port',
    CAST(json_extract(connection_config, '$.port') AS INTEGER)
)
WHERE json_type(connection_config, '$.port') = 'text'
    AND json_extract(connection_config, '$.port') GLOB '[0-9]*'
    AND json_extract(connection_config, '$.port') NOT GLOB '*[^0-9]*'
    AND LENGTH(json_extract(connection_config, '$.port')) BETWEEN 1 AND 5
    AND CAST(json_extract(connection_config, '$.port') AS INTEGER) BETWEEN 1 AND 65535;

-- ssh_port (numeric, SSH tunnel bastion port)
UPDATE datasource_configs
SET connection_config = json_set(
    connection_config,
    '$.ssh_port',
    CAST(json_extract(connection_config, '$.ssh_port') AS INTEGER)
)
WHERE json_type(connection_config, '$.ssh_port') = 'text'
    AND json_extract(connection_config, '$.ssh_port') GLOB '[0-9]*'
    AND json_extract(connection_config, '$.ssh_port') NOT GLOB '*[^0-9]*'
    AND LENGTH(json_extract(connection_config, '$.ssh_port')) BETWEEN 1 AND 5
    AND CAST(json_extract(connection_config, '$.ssh_port') AS INTEGER) BETWEEN 1 AND 65535;

-- secure (boolean, ClickHouse TLS)
UPDATE datasource_configs
SET connection_config = json_set(
    connection_config,
    '$.secure',
    json(json_extract(connection_config, '$.secure'))
)
WHERE json_type(connection_config, '$.secure') = 'text'
    AND json_extract(connection_config, '$.secure') IN ('true', 'false');

-- encrypt (boolean, SQL Server / Synapse TLS)
UPDATE datasource_configs
SET connection_config = json_set(
    connection_config,
    '$.encrypt',
    json(json_extract(connection_config, '$.encrypt'))
)
WHERE json_type(connection_config, '$.encrypt') = 'text'
    AND json_extract(connection_config, '$.encrypt') IN ('true', 'false');

-- trust_server_certificate (boolean, SQL Server / Synapse TLS)
UPDATE datasource_configs
SET connection_config = json_set(
    connection_config,
    '$.trust_server_certificate',
    json(json_extract(connection_config, '$.trust_server_certificate'))
)
WHERE json_type(connection_config, '$.trust_server_certificate') = 'text'
    AND json_extract(connection_config, '$.trust_server_certificate') IN ('true', 'false');

-- ssh_enabled (boolean, whether to route the connection through the SSH tunnel)
UPDATE datasource_configs
SET connection_config = json_set(
    connection_config,
    '$.ssh_enabled',
    json(json_extract(connection_config, '$.ssh_enabled'))
)
WHERE json_type(connection_config, '$.ssh_enabled') = 'text'
    AND json_extract(connection_config, '$.ssh_enabled') IN ('true', 'false');

-- shared_credentials (boolean, whether the datasource uses workspace-level
-- shared credentials instead of per-user ones). Same nuance as the Postgres
-- counterpart: the modal only ever inserts this key when true, so an absent
-- key legitimately means false — the WHERE guard below leaves it untouched
-- rather than inventing one. Both `'true'` and `'false'` are still accepted
-- for conversion, like every other boolean leaf, in case some other writer
-- ever emits `false`.
UPDATE datasource_configs
SET connection_config = json_set(
    connection_config,
    '$.shared_credentials',
    json(json_extract(connection_config, '$.shared_credentials'))
)
WHERE json_type(connection_config, '$.shared_credentials') = 'text'
    AND json_extract(connection_config, '$.shared_credentials') IN ('true', 'false');
