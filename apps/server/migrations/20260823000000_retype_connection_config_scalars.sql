-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- KYO-460: repair `datasource_configs.connection_config` rows whose scalar
-- leaves were flattened to JSON strings by the pre-KYO-428 Leptos datasource
-- modal.
--
-- Before KYO-428, the create/edit datasource server fns decoded their
-- request body with `serde_qs` (the default `server_fn` codec), which has no
-- way to distinguish "the number 5432" from "the string \"5432\"" on the
-- wire — every scalar round-trips as text. KYO-428 fixed the *cause* by
-- switching those server fns to `input = server_fn::codec::Json`, so new
-- writes carry real JSON numbers/booleans again. It repaired nothing already
-- persisted: every datasource created or edited through the modal before
-- that fix may still have string-typed scalars sitting in this column.
--
-- The drivers (in the separate `kyomi-connect` repo) read these fields with
-- `.as_u64()` / `.as_bool()`, both of which return `None` for a JSON string
-- — not an error, not a log line. The call site then falls back to a
-- hardcoded default via `.unwrap_or(DEFAULT)`, so a datasource silently
-- keeps using the wrong port or the wrong TLS/SSH posture indefinitely, with
-- nothing in the UI or logs pointing at why. `ssh_enabled` is the sharpest
-- case: `"true"` (string) makes `.as_bool()` return `None`,
-- `unwrap_or(false)` fires, and `SshTunnelConfig::from_connection_config`
-- returns `None` outright — the SSH tunnel is silently never established.
--
-- This is a ONE-SHOT repair, not an ongoing safeguard: KYO-428 already
-- stopped new corruption at the source, so this migration only needs to
-- clean up the backlog of rows written before that fix shipped.
--
-- Detection is by JSON TYPE, never by value. A stored `"5432"` is both a
-- plausible genuine value (as text, pre-repair) and indistinguishable from
-- an intentional non-numeric field by content alone — `jsonb_typeof(...) =
-- 'string'` is the only test that can't misfire on a coincidentally
-- numeric-looking or boolean-looking string that was never meant to be
-- typed. Seven leaves get this treatment (the ticket names four; tracing
-- `build_connection_config` in the Leptos modal found three more with the
-- identical defect):
--   * `port`, `ssh_port`               — read via `.as_u64()`
--   * `secure`, `encrypt`,
--     `trust_server_certificate`,
--     `ssh_enabled`                    — read via `.as_bool()`
--   * `shared_credentials`             — read via `.as_bool().unwrap_or(false)`
--                                         at two call sites (`kyomi-auth`'s
--                                         `datasource_service.rs`, and
--                                         `kyomi-connect`'s
--                                         `factory.rs::resolve_shared_credentials`,
--                                         reached from every provider
--                                         construction via `create_provider`)
--                                         but ALSO via a strict
--                                         `== Some(&Value::Bool(true))` /
--                                         `if let Some(Value::Bool(true))`
--                                         pattern match at four more call
--                                         sites (`kyomi-auth`'s
--                                         `datasource_auth_service.rs`, three
--                                         places, and `kyomi-core`'s
--                                         `datasource_registry.rs`) that have
--                                         no `.unwrap_or` fallback at all — a
--                                         JSON string there doesn't fall back
--                                         to a default, the match itself
--                                         simply fails and the `if` is never
--                                         entered. This makes
--                                         `shared_credentials` the most
--                                         severe of the seven: a corrupted
--                                         row is silently treated as
--                                         not-shared everywhere, and
--                                         per-user credentials — often
--                                         nonexistent for that datasource —
--                                         are used instead.
--
-- Conservative by construction — never destroys or guesses at data:
--   * Numeric leaves convert only when the JSON type is `string`, the text
--     is composed of 1-5 ASCII digits (`^[0-9]{1,5}$`), and the decimal
--     value falls in 1..=65535. Anything else (non-digits, empty, out of
--     range) is left exactly as stored.
--   * Boolean leaves convert only when the JSON type is `string` and the
--     text is EXACTLY (case-sensitive) `true` or `false`. Only lowercase is
--     accepted: `serde_qs` (the codec that produced this corruption) always
--     emits lowercase `Display` output for Rust `bool`, so no other casing
--     could have been written by the code path this migration repairs;
--     accepting more would just widen the guess. Anything else is left
--     exactly as stored.
--   * A row missing a key was never corrupted by this bug (the modal only
--     inserts `port` when it successfully parses one, and the SSH keys only
--     exist when the SSH panel was ever touched) — absent stays absent,
--     nothing here ever adds a key.
--   * Rows that are already correctly typed are never written at all (the
--     `WHERE` excludes them before any `UPDATE` touches the row), so they
--     come out of this migration byte-identical.
--
-- Each `UPDATE` below reads the candidate value once, inside a `CASE`
-- expression in the `FROM` subquery, and only ever casts a value that the
-- SAME `CASE`'s `WHEN` clause already proved is a bounded run of ASCII
-- digits (numeric leaves) or exactly `true`/`false` (boolean leaves). This
-- is deliberate, not stylistic: Postgres does NOT guarantee left-to-right,
-- short-circuit evaluation of an `AND`-chain in a `WHERE` clause (see
-- "Expression Evaluation Rules" in the Postgres docs — the planner is free
-- to reorder conjuncts by estimated cost), so a naive
-- `WHERE jsonb_typeof(...) = 'string' AND col::int BETWEEN 1 AND 65535`
-- can, in principle, evaluate the cast before the type guard and throw
-- `invalid input syntax for type integer` on a row this migration was
-- never supposed to touch — aborting the whole migration. A `CASE`
-- expression's `WHEN` clauses, by contrast, ARE guaranteed to evaluate in
-- order and to skip a `THEN` whose condition wasn't met (Postgres docs,
-- "Simple CASE" / "Searched CASE": "case expressions do not evaluate any
-- subexpressions that are not needed to determine the result"). Every cast
-- in this file lives inside such a guarded `THEN`, so it is unreachable
-- unless the guard already holds — this was verified empirically against a
-- live dev Postgres with adversarial rows (a 30-digit numeric string, a
-- non-numeric string containing digits, an out-of-range value) before this
-- migration was written, and none of them raised an error.

-- port (numeric, provider connection host port)
UPDATE public.datasource_configs AS d
SET connection_config = jsonb_set(d.connection_config, '{port}', to_jsonb(r.new_val), false)
FROM (
    SELECT
        id,
        CASE
            WHEN jsonb_typeof(connection_config -> 'port') = 'string'
                AND connection_config ->> 'port' ~ '^[0-9]{1,5}$'
            THEN (connection_config ->> 'port')::int
        END AS new_val
    FROM public.datasource_configs
) AS r
WHERE d.id = r.id
    AND r.new_val BETWEEN 1 AND 65535;

-- ssh_port (numeric, SSH tunnel bastion port)
UPDATE public.datasource_configs AS d
SET connection_config = jsonb_set(d.connection_config, '{ssh_port}', to_jsonb(r.new_val), false)
FROM (
    SELECT
        id,
        CASE
            WHEN jsonb_typeof(connection_config -> 'ssh_port') = 'string'
                AND connection_config ->> 'ssh_port' ~ '^[0-9]{1,5}$'
            THEN (connection_config ->> 'ssh_port')::int
        END AS new_val
    FROM public.datasource_configs
) AS r
WHERE d.id = r.id
    AND r.new_val BETWEEN 1 AND 65535;

-- secure (boolean, ClickHouse TLS)
UPDATE public.datasource_configs AS d
SET connection_config = jsonb_set(d.connection_config, '{secure}', to_jsonb(r.new_val), false)
FROM (
    SELECT
        id,
        CASE
            WHEN jsonb_typeof(connection_config -> 'secure') = 'string'
                AND connection_config ->> 'secure' IN ('true', 'false')
            THEN (connection_config ->> 'secure')::boolean
        END AS new_val
    FROM public.datasource_configs
) AS r
WHERE d.id = r.id
    AND r.new_val IS NOT NULL;

-- encrypt (boolean, SQL Server / Synapse TLS)
UPDATE public.datasource_configs AS d
SET connection_config = jsonb_set(d.connection_config, '{encrypt}', to_jsonb(r.new_val), false)
FROM (
    SELECT
        id,
        CASE
            WHEN jsonb_typeof(connection_config -> 'encrypt') = 'string'
                AND connection_config ->> 'encrypt' IN ('true', 'false')
            THEN (connection_config ->> 'encrypt')::boolean
        END AS new_val
    FROM public.datasource_configs
) AS r
WHERE d.id = r.id
    AND r.new_val IS NOT NULL;

-- trust_server_certificate (boolean, SQL Server / Synapse TLS)
UPDATE public.datasource_configs AS d
SET connection_config = jsonb_set(d.connection_config, '{trust_server_certificate}', to_jsonb(r.new_val), false)
FROM (
    SELECT
        id,
        CASE
            WHEN jsonb_typeof(connection_config -> 'trust_server_certificate') = 'string'
                AND connection_config ->> 'trust_server_certificate' IN ('true', 'false')
            THEN (connection_config ->> 'trust_server_certificate')::boolean
        END AS new_val
    FROM public.datasource_configs
) AS r
WHERE d.id = r.id
    AND r.new_val IS NOT NULL;

-- ssh_enabled (boolean, whether to route the connection through the SSH tunnel)
UPDATE public.datasource_configs AS d
SET connection_config = jsonb_set(d.connection_config, '{ssh_enabled}', to_jsonb(r.new_val), false)
FROM (
    SELECT
        id,
        CASE
            WHEN jsonb_typeof(connection_config -> 'ssh_enabled') = 'string'
                AND connection_config ->> 'ssh_enabled' IN ('true', 'false')
            THEN (connection_config ->> 'ssh_enabled')::boolean
        END AS new_val
    FROM public.datasource_configs
) AS r
WHERE d.id = r.id
    AND r.new_val IS NOT NULL;

-- shared_credentials (boolean, whether the datasource uses workspace-level
-- shared credentials instead of per-user ones). The Leptos modal's
-- `build_connection_config` only ever inserts this key when the checkbox is
-- true — it never writes `false` — so an absent key legitimately means
-- false, same as the feature's own default; the guards above already leave
-- an absent key untouched, and nothing here invents one. Both `"true"` and
-- `"false"` are still accepted for conversion, like every other boolean
-- leaf, in case some other writer ever emits `false`.
UPDATE public.datasource_configs AS d
SET connection_config = jsonb_set(d.connection_config, '{shared_credentials}', to_jsonb(r.new_val), false)
FROM (
    SELECT
        id,
        CASE
            WHEN jsonb_typeof(connection_config -> 'shared_credentials') = 'string'
                AND connection_config ->> 'shared_credentials' IN ('true', 'false')
            THEN (connection_config ->> 'shared_credentials')::boolean
        END AS new_val
    FROM public.datasource_configs
) AS r
WHERE d.id = r.id
    AND r.new_val IS NOT NULL;
