-- Flatten users.chartml_config from nested {config: {style, ...}} to flat {style, ...}.
-- Rationale: Leptos writers (profile.rs, workspace.rs) wrote the nested shape
-- post-KYO-42 cutover, while the React-era REST path wrote the flat shape.
-- Part 1 of KYO-129 made all readers shape-agnostic; Part 2 normalizes storage
-- so future writes produce a single canonical shape.
--
-- The `chartml_config` column is `json` (not `jsonb`), so we use `IS NOT NULL`
-- existence checks rather than the `?` operator.
--
-- The `chartml_config IS NOT NULL` guard is defensive: for a NULL column,
-- `col->'config'->'style'` is itself NULL and `IS NOT NULL` already filters
-- the row out, but spelling it out keeps the intent legible and avoids any
-- doubt if someone later swaps the column type to `jsonb` or tweaks the path.
-- The filter also makes this migration idempotent — once a row is flattened,
-- `chartml_config->'config'->'style'` is NULL for it and subsequent runs
-- (e.g. a restored backup being re-migrated) skip it.
UPDATE users
SET chartml_config = chartml_config->'config'
WHERE chartml_config IS NOT NULL
  AND chartml_config->'config'->'style' IS NOT NULL;
