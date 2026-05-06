-- Flatten users.chartml_config from nested {config: {style, ...}} to flat {style, ...}.
-- See migrations/20260422000000_flatten_chartml_config.sql for rationale.
-- SQLite stores the column as TEXT containing JSON; json_extract returns JSON text
-- for object values and scalar values for leaf paths.
UPDATE users
SET chartml_config = json_extract(chartml_config, '$.config')
WHERE json_extract(chartml_config, '$.config.style') IS NOT NULL;
