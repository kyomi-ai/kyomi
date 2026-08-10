// SPDX-License-Identifier: AGPL-3.0-or-later

//! Migration schema-parity check (KYO-206, extended by KYO-296).
//!
//! Kyomi maintains two parallel migration chains — `apps/server/migrations`
//! (Postgres/SaaS) and `apps/server/migrations-sqlite` (SQLite/self-hosted).
//! Nothing else enforces that they converge on the same logical schema, and
//! the filenames don't even line up 1:1 (about a dozen pairs differ only by
//! naming convention, e.g. `add_connect_support` / `connect_support`). This
//! test sidesteps the naming problem entirely by comparing *resulting*
//! schemas rather than migration files.
//!
//! Two divergences of exactly this shape already shipped to production
//! undetected (KYO-200): `collections.doc_type` and `thinking_event_details`
//! existed only on Postgres, breaking self-hosted SQLite deployments (one
//! outright, one silently — the caller swallowed the error).
//!
//! This test runs the *real* embedded `sqlx::migrate!` chains from
//! `crates/kyomi-core/src/db.rs` (via the public `DbPool::connect` entry
//! point — the same one production uses) against a scratch Postgres
//! database and a fresh in-memory SQLite database, introspects both
//! resulting schemas into normalized representations, and fails on any
//! unallowlisted divergence. It deliberately does not parse the `.sql`
//! files itself — that would let this check drift from what
//! `DbPool::connect` (i.e. production) actually runs.
//!
//! Known-legitimate divergences are checked in at
//! `apps/server/schema-parity-allowlist.toml`, each with a justification.
//!
//! ## What is compared (KYO-296)
//!
//! Beyond columns (presence, normalized type class, nullability,
//! default-presence — see [`diff_schemas`]), this check compares:
//!
//! - **Primary keys** ([`diff_primary_keys`]): the ordered column list.
//! - **Unique indexes and unique constraints** ([`diff_indexes`]): name,
//!   normalized key expression (so `lower(name)` vs `name` is caught, not
//!   just column *lists*), and partial-index `WHERE` predicate.
//! - **Foreign keys** ([`diff_foreign_keys`]): source columns, target table
//!   and columns, and `ON DELETE`/`ON UPDATE` actions.
//!
//! ### Non-unique indexes are deliberately NOT compared for presence
//!
//! A missing non-unique index is a performance issue, not a correctness
//! one, and the two chains spell hundreds of them differently (naming
//! convention, column order for multi-column indexes chosen independently
//! per backend). Enforcing parity there would produce an allowlist so large
//! it stops meaning anything — exactly the rubber-stamp decay the
//! allowlist header warns against.
//!
//! The one exception: if a *name-matched* pair has at least one side
//! marked `UNIQUE`, it is still compared (see [`diff_indexes`]). That is
//! what catches "UNIQUE on Postgres, plain index on SQLite" — a real
//! correctness divergence, not a performance one — while leaving the
//! hundreds of non-unique-both-sides pairs alone.
//!
//! ### Check constraints are NOT compared — this is deliberate, not an oversight
//!
//! The ticket considered comparing `CHECK` constraints "if cheaply
//! available." They are not:
//!
//! - Postgres exposes them cleanly via `pg_get_constraintdef`.
//! - SQLite has no equivalent catalog view — a check constraint's
//!   expression only exists inside the raw DDL text in
//!   `sqlite_master.sql`, which would require a real SQL expression parser
//!   to extract and compare, not the text-transform normalization used for
//!   index key expressions here.
//! - Even with a parser, Postgres and SQLite spell equivalent predicates
//!   differently in ways that are semantically identical but
//!   textually unrelated — e.g. Postgres desugars `x IN (a, b)` to
//!   `x = ANY (ARRAY[a, b])`. Building a normalizer that treats
//!   `status = ANY (ARRAY['a'::text, 'b'::text])` as equal to
//!   `status IN ('a', 'b')` would manufacture exactly the false
//!   equivalence this file is told never to invent (see the type-class
//!   comment on [`TypeClass`] for the same principle applied to columns).
//!
//! Rather than build a bespoke SQL-equivalence engine to avoid both false
//! positives (flagging identical constraints as different) and false
//! negatives (treating genuinely different constraints as the same), check
//! constraints are left uncompared. A real divergence there is not caught
//! by this test.
//!
//! ## Hermeticity (KYO-242)
//!
//! `apps/server/tests/contract_*` shares one persistent Postgres container
//! across worktrees; an unmerged branch's migration there can brick every
//! other worktree's contract suite. This test does not touch that shared
//! database. It creates its own scratch Postgres database (dropped at the
//! end of the test) on the same server, and its SQLite side is a fresh
//! `sqlite::memory:` database that's never persisted at all.
//!
//! Indexes and foreign keys are introspected from the *same* pools/pass
//! already opened for column introspection — no second migration run.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::OnceLock;

use regex::Regex;

// ---------------------------------------------------------------------------
// Normalized schema representation
// ---------------------------------------------------------------------------

/// A coarse type classification shared by both backends.
///
/// Only type families where Postgres and SQLite have an unambiguous,
/// well-known correspondence are classified. Anything else (Postgres
/// `ARRAY`/`USER-DEFINED` types such as `vector(384)` or enum types,
/// `bytea`, `tsvector`; SQLite `BLOB`) classifies as `None` in
/// [`classify_postgres_type`] / [`classify_sqlite_type`] — the diff below
/// skips the type-class comparison (but still checks presence and
/// nullability) for those rather than inventing a false equivalence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeClass {
    /// Postgres: `text`, `character varying`, `character`, `uuid`,
    /// `timestamp with time zone`, `timestamp without time zone`, `date`,
    /// `json`, `jsonb`.
    /// SQLite: `TEXT`.
    ///
    /// Timestamps and JSON fold into this class because SQLite has no
    /// native datetime or JSON storage class — both are stored as `TEXT`
    /// (ISO-8601 strings via `datetime('now')`, and JSON-serialized text
    /// respectively). `uuid` is included because every UUID primary/foreign
    /// key was converted to `TEXT` on Postgres by
    /// `20260315000000_uuid_columns_to_text.sql` to match SQLite, which
    /// stored UUIDs as `TEXT` from the baseline; any stray `uuid` column
    /// left behind would still compare correctly against SQLite's `TEXT`.
    Text,
    /// Postgres: `smallint`, `integer`, `bigint`, `boolean`.
    /// SQLite: `INTEGER`.
    ///
    /// `boolean` folds into the same class as the integer family because
    /// SQLite has no boolean storage class — booleans are declared
    /// `INTEGER` and stored as 0/1, indistinguishable by introspection from
    /// a genuine integer column.
    Integer,
    /// Postgres: `double precision`, `real`, `numeric`.
    /// SQLite: `REAL`.
    Real,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ColumnInfo {
    class: Option<TypeClass>,
    raw_type: String,
    nullable: bool,
    has_default: bool,
}

type TableSchema = BTreeMap<String, ColumnInfo>;
type DbSchema = BTreeMap<String, TableSchema>;

fn classify_postgres_type(data_type: &str) -> Option<TypeClass> {
    match data_type {
        "text" | "character varying" | "character" | "uuid" | "timestamp with time zone"
        | "timestamp without time zone" | "date" | "json" | "jsonb" => Some(TypeClass::Text),
        "smallint" | "integer" | "bigint" | "boolean" => Some(TypeClass::Integer),
        "double precision" | "real" | "numeric" => Some(TypeClass::Real),
        // ARRAY (e.g. text[]) and USER-DEFINED (pgvector's `vector(384)`,
        // enum types like `learning_scope`) have no unambiguous SQLite
        // counterpart — left unclassified rather than guessed at.
        _ => None,
    }
}

fn classify_sqlite_type(raw: &str) -> Option<TypeClass> {
    match raw.to_ascii_uppercase().as_str() {
        "TEXT" => Some(TypeClass::Text),
        "INTEGER" => Some(TypeClass::Integer),
        "REAL" => Some(TypeClass::Real),
        // BLOB (used for embedding columns, stored as raw f32 LE bytes) has
        // no unambiguous Postgres counterpart (Postgres uses pgvector's
        // native `vector` type for the same data) — left unclassified.
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Postgres introspection — columns
// ---------------------------------------------------------------------------

async fn introspect_postgres(pool: &sqlx::PgPool) -> DbSchema {
    #[derive(sqlx::FromRow)]
    struct Row {
        table_name: String,
        column_name: String,
        data_type: String,
        udt_name: String,
        is_nullable: String,
        column_default: Option<String>,
    }

    let rows: Vec<Row> = sqlx::query_as(
        r#"
        SELECT c.table_name, c.column_name, c.data_type, c.udt_name,
               c.is_nullable, c.column_default
        FROM information_schema.columns c
        JOIN information_schema.tables t
          ON t.table_schema = c.table_schema AND t.table_name = c.table_name
        WHERE c.table_schema = 'public'
          AND t.table_type = 'BASE TABLE'
          AND c.table_name <> '_sqlx_migrations'
        ORDER BY c.table_name, c.column_name
        "#,
    )
    .fetch_all(pool)
    .await
    .expect("introspect Postgres schema via information_schema.columns");

    let mut schema: DbSchema = DbSchema::new();
    for row in rows {
        let class = classify_postgres_type(&row.data_type);
        // For ARRAY/USER-DEFINED columns, data_type is a generic marker
        // ("ARRAY", "USER-DEFINED") — udt_name has the real type name
        // (e.g. "_text", "vector") and is far more useful in failure output.
        let raw_type = match row.data_type.as_str() {
            "ARRAY" | "USER-DEFINED" => row.udt_name,
            other => other.to_string(),
        };
        schema.entry(row.table_name).or_default().insert(
            row.column_name,
            ColumnInfo {
                class,
                raw_type,
                nullable: row.is_nullable == "YES",
                has_default: row.column_default.is_some(),
            },
        );
    }
    schema
}

// ---------------------------------------------------------------------------
// SQLite introspection — columns
// ---------------------------------------------------------------------------

async fn introspect_sqlite(pool: &sqlx::SqlitePool) -> DbSchema {
    #[derive(sqlx::FromRow)]
    struct TableRow {
        name: String,
    }

    let tables: Vec<TableRow> = sqlx::query_as(
        "SELECT name FROM sqlite_master \
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations' \
         ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .expect("list SQLite tables via sqlite_master");

    #[derive(sqlx::FromRow)]
    struct ColRow {
        name: String,
        #[sqlx(rename = "type")]
        col_type: String,
        notnull: i64,
        dflt_value: Option<String>,
    }

    let mut schema: DbSchema = DbSchema::new();
    for table in tables {
        // PRAGMA doesn't support bind parameters; the table name comes from
        // sqlite_master itself (not external input), so string
        // interpolation with defensive quote-doubling is safe here.
        let pragma = format!("PRAGMA table_info(\"{}\")", table.name.replace('"', "\"\""));
        let cols: Vec<ColRow> = sqlx::query_as(&pragma)
            .fetch_all(pool)
            .await
            .unwrap_or_else(|e| panic!("introspect SQLite table `{}`: {e}", table.name));

        let mut table_schema = TableSchema::new();
        for col in cols {
            let class = classify_sqlite_type(&col.col_type);
            table_schema.insert(
                col.name,
                ColumnInfo {
                    class,
                    raw_type: col.col_type,
                    nullable: col.notnull == 0,
                    has_default: col.dflt_value.is_some(),
                },
            );
        }
        schema.insert(table.name, table_schema);
    }
    schema
}

// ---------------------------------------------------------------------------
// Expression normalization (KYO-296)
//
// Postgres spells the same index/predicate differently from how it was
// authored: `USING btree ` and `public.` prefixes (stripped structurally by
// the def parsers below, not here), `((name)::text)` casts, `"quoted"`
// identifiers, and redundant grouping parentheses. This section normalizes
// conservatively: where two things genuinely cannot be compared, callers
// leave the pair unchecked rather than asserting a false equivalence (see
// the module doc's Check Constraints section for why that principle
// matters).
// ---------------------------------------------------------------------------

/// Strip a `::type` cast (Postgres-only syntax). Handles a type name with
/// spaces (`character varying`), an optional parenthesized modifier
/// (`numeric(10,2)`), and an optional array marker (`text[]`). Does not
/// consume trailing whitespace after the type, so `a::text = b` normalizes
/// to `a = b` rather than losing the space and producing `a= b`.
fn strip_type_casts(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"::[a-z_][a-z0-9_]*(?: [a-z_][a-z0-9_]*)*(\([^()]*\))?(\[\])?")
            .expect("static regex is valid")
    });
    re.replace_all(s, "").into_owned()
}

/// Strip a `public.` schema qualifier. Word-boundary anchored so it never
/// touches a column/table merely containing "public" as a substring.
fn strip_public_schema_prefix(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\bpublic\.").expect("static regex is valid"));
    re.replace_all(s, "").into_owned()
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// A bare identifier (optionally dotted, e.g. `t.col`) or numeric literal —
/// safe to unwrap from parentheses regardless of surrounding context,
/// because it can't contain an operator whose precedence the parens were
/// protecting.
fn is_atomic_expr(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| is_ident_char(c) || c == '.')
        && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Recursively strip parentheses that wrap a single atomic sub-expression,
/// e.g. `(name)` -> `name`. Parens immediately preceded by an identifier
/// character are a function call's argument list (`lower(...)`) and are
/// never stripped themselves — but their contents are still recursed into,
/// so `lower((name)::text)` (after cast-stripping: `lower((name))`)
/// normalizes to `lower(name)` without destroying the call syntax.
///
/// This intentionally does NOT strip parens wrapping a non-atomic
/// expression (e.g. `(status = 'pending')`) — that is
/// [`strip_whole_expression_wrap`]'s job, applied once to the fully
/// assembled expression rather than to every sub-term (stripping those
/// mid-expression could change operator grouping).
fn strip_redundant_grouping_parens(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '(' {
            let preceded_by_ident = i > 0 && is_ident_char(chars[i - 1]);
            let mut depth = 1i32;
            let mut j = i + 1;
            while j < chars.len() && depth > 0 {
                match chars[j] {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                if depth > 0 {
                    j += 1;
                }
            }
            if depth != 0 {
                // Unbalanced — shouldn't happen against real def text, but
                // don't panic on it; pass the rest through unchanged.
                out.extend(&chars[i..]);
                return out;
            }
            let inner: String = chars[i + 1..j].iter().collect();
            let inner_processed = strip_redundant_grouping_parens(&inner);
            let inner_trimmed = inner_processed.trim();
            if !preceded_by_ident && is_atomic_expr(inner_trimmed) {
                out.push_str(inner_trimmed);
            } else {
                out.push('(');
                out.push_str(&inner_processed);
                out.push(')');
            }
            i = j + 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// `true` if `s` is entirely wrapped in one balanced pair of parens — i.e.
/// the first `(` matches the last `)`, not some earlier `)`.
fn is_fully_wrapped(s: &str) -> bool {
    if !(s.starts_with('(') && s.ends_with(')')) {
        return false;
    }
    let mut depth = 0i32;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 && i != s.len() - 1 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

/// Repeatedly strip a paren pair that wraps the *entire* expression, e.g.
/// `(status = 'pending')` -> `status = 'pending'`, `((x))` -> `x`.
fn strip_whole_expression_wrap(s: &str) -> String {
    let mut cur = s.trim().to_string();
    while is_fully_wrapped(&cur) {
        cur = cur[1..cur.len() - 1].trim().to_string();
    }
    cur
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::new();
    let mut last_was_space = false;
    for ch in s.trim().chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                out.push(' ');
            }
            last_was_space = true;
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }
    out
}

/// Lowercase everything *outside* single-quoted string literals, leaving
/// literal content byte-for-byte as written.
///
/// SQL string data is case-sensitive — `'Active'` and `'active'` are
/// different values, not different spellings of the same identifier. Naively
/// lowercasing the whole expression would fold those into the same
/// normalized signature and report a WHERE-predicate divergence as "no
/// difference," manufacturing exactly the false equivalence this module's
/// Check Constraints section explains must never happen. Handles SQL's
/// standard doubled-quote escape (`'it''s'` is the literal `it's`) — a `''`
/// while inside a literal is an escaped quote character, not the end of the
/// literal.
fn lowercase_outside_string_literals(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut in_literal = false;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            if in_literal && chars.get(i + 1) == Some(&'\'') {
                out.push('\'');
                out.push('\'');
                i += 2;
                continue;
            }
            in_literal = !in_literal;
            out.push(c);
            i += 1;
            continue;
        }
        if in_literal {
            out.push(c);
        } else {
            out.extend(c.to_lowercase());
        }
        i += 1;
    }
    out
}

/// Normalize a single expression (a `WHERE` predicate, or one component of
/// an index's key-expression list) into a comparable canonical form.
fn normalize_single_expr(raw: &str) -> String {
    let s = lowercase_outside_string_literals(raw.trim());
    let s = strip_public_schema_prefix(&s);
    let s = strip_type_casts(&s);
    let s = s.replace('"', "");
    let s = strip_redundant_grouping_parens(&s);
    let s = strip_whole_expression_wrap(&s);
    collapse_whitespace(&s)
}

/// Split a comma-separated key-expression list at top-level commas only
/// (commas nested inside a function call's argument list are not split
/// boundaries), then normalize each component.
fn normalize_expr_list(raw: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for ch in raw.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                parts.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    parts.push(current);
    parts.iter().map(|c| normalize_single_expr(c)).collect()
}

// ---------------------------------------------------------------------------
// Index / primary key / foreign key representation (KYO-296)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexInfo {
    name: String,
    table: String,
    unique: bool,
    /// Normalized key expression, one entry per key column/expression, in
    /// order.
    key_signature: Vec<String>,
    /// Normalized partial-index `WHERE` predicate. `None` means the index
    /// is not partial.
    where_signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PrimaryKeyInfo {
    /// Ordered, lower-cased column names.
    columns: Vec<String>,
}

type PrimaryKeys = BTreeMap<String, PrimaryKeyInfo>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefAction {
    NoAction,
    Restrict,
    Cascade,
    SetNull,
    SetDefault,
}

impl std::fmt::Display for RefAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            RefAction::NoAction => "NO ACTION",
            RefAction::Restrict => "RESTRICT",
            RefAction::Cascade => "CASCADE",
            RefAction::SetNull => "SET NULL",
            RefAction::SetDefault => "SET DEFAULT",
        };
        write!(f, "{s}")
    }
}

fn parse_pg_ref_action(code: &str) -> RefAction {
    match code {
        "a" => RefAction::NoAction,
        "r" => RefAction::Restrict,
        "c" => RefAction::Cascade,
        "n" => RefAction::SetNull,
        "d" => RefAction::SetDefault,
        other => panic!("unknown pg_constraint confdeltype/confupdtype code: {other}"),
    }
}

fn parse_sqlite_ref_action(s: &str) -> RefAction {
    match s.to_ascii_uppercase().as_str() {
        "NO ACTION" => RefAction::NoAction,
        "RESTRICT" => RefAction::Restrict,
        "CASCADE" => RefAction::Cascade,
        "SET NULL" => RefAction::SetNull,
        "SET DEFAULT" => RefAction::SetDefault,
        other => panic!("unknown SQLite foreign_key_list on_update/on_delete value: {other}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ForeignKeyInfo {
    table: String,
    /// Ordered, lower-cased source column names.
    source_columns: Vec<String>,
    target_table: String,
    /// Ordered, lower-cased target column names.
    target_columns: Vec<String>,
    on_delete: RefAction,
    on_update: RefAction,
}

// ---------------------------------------------------------------------------
// Postgres index definition parsing
// ---------------------------------------------------------------------------

/// Find the index of the `)` that matches the `(` at `bytes[open_paren_idx]`
/// (which must itself be `(`), scanning forward and tracking nesting depth.
/// Returns `None` if the parens are unbalanced.
fn find_matching_close_paren(bytes: &[u8], open_paren_idx: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, &b) in bytes.iter().enumerate().skip(open_paren_idx) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Extract the raw key-expression list and optional `WHERE` predicate from
/// a `pg_get_indexdef()` result, e.g.
/// `CREATE UNIQUE INDEX name ON public.t USING btree (a, lower((b)::text)) WHERE (c IS NOT NULL)`.
fn parse_postgres_indexdef(def: &str) -> (String, Option<String>) {
    let using_marker = " USING ";
    let using_pos = def
        .find(using_marker)
        .unwrap_or_else(|| panic!("pg_get_indexdef output missing ` USING <method> `: {def}"));
    let after_using = &def[using_pos + using_marker.len()..];
    let paren_offset = after_using.find('(').unwrap_or_else(|| {
        panic!("pg_get_indexdef output missing key-expression parens after USING <method>: {def}")
    });
    let abs_start = using_pos + using_marker.len() + paren_offset;

    let end = find_matching_close_paren(def.as_bytes(), abs_start)
        .unwrap_or_else(|| panic!("unbalanced parens in pg_get_indexdef output: {def}"));

    let key_expr = def[abs_start + 1..end].to_string();
    let rest = &def[end + 1..];
    let rest_lower = rest.to_ascii_lowercase();
    let where_raw = rest_lower.find("where").map(|pos| rest[pos + "where".len()..].trim().to_string());
    (key_expr, where_raw)
}

/// Extract the raw key-expression list and optional `WHERE` predicate from
/// a SQLite `sqlite_master.sql` CREATE INDEX statement, e.g.
/// `CREATE UNIQUE INDEX name ON t(a, b) WHERE c IS NOT NULL`.
fn parse_sqlite_indexdef(sql: &str) -> (String, Option<String>) {
    let lower = sql.to_ascii_lowercase();
    let on_marker = " on ";
    let on_pos = lower
        .find(on_marker)
        .unwrap_or_else(|| panic!("SQLite index definition missing ' ON ' clause: {sql}"));
    let after_on = &sql[on_pos + on_marker.len()..];
    let paren_offset = after_on.find('(').unwrap_or_else(|| {
        panic!("SQLite index definition missing key-expression parens after ON <table>: {sql}")
    });
    let abs_start = on_pos + on_marker.len() + paren_offset;

    let end = find_matching_close_paren(sql.as_bytes(), abs_start)
        .unwrap_or_else(|| panic!("unbalanced parens in SQLite index definition: {sql}"));

    let key_expr = sql[abs_start + 1..end].to_string();
    let rest = &sql[end + 1..];
    let rest_lower = rest.to_ascii_lowercase();
    let where_raw = rest_lower
        .find("where")
        .map(|pos| rest[pos + "where".len()..].trim_end_matches(';').trim().to_string());
    (key_expr, where_raw)
}

// ---------------------------------------------------------------------------
// Postgres introspection — indexes, primary keys, foreign keys
// ---------------------------------------------------------------------------

async fn introspect_postgres_indexes(pool: &sqlx::PgPool) -> Vec<IndexInfo> {
    #[derive(sqlx::FromRow)]
    struct Row {
        table_name: String,
        index_name: String,
        index_def: String,
        is_unique: bool,
        is_primary: bool,
    }

    let rows: Vec<Row> = sqlx::query_as(
        r#"
        SELECT
            cl.relname AS table_name,
            ic.relname AS index_name,
            pg_get_indexdef(ix.indexrelid) AS index_def,
            ix.indisunique AS is_unique,
            ix.indisprimary AS is_primary
        FROM pg_index ix
        JOIN pg_class cl ON cl.oid = ix.indrelid
        JOIN pg_class ic ON ic.oid = ix.indexrelid
        JOIN pg_namespace n ON n.oid = cl.relnamespace
        WHERE n.nspname = 'public' AND cl.relkind = 'r' AND cl.relname <> '_sqlx_migrations'
        ORDER BY cl.relname, ic.relname
        "#,
    )
    .fetch_all(pool)
    .await
    .expect("introspect Postgres indexes via pg_index");

    rows.into_iter()
        // Primary keys are handled separately via introspect_postgres_primary_keys
        // (mirroring the SQLite rowid-PK caveat — see introspect_sqlite_primary_keys),
        // so excluding them here means neither side double-reports a PK as an index.
        .filter(|r| !r.is_primary)
        .map(|r| {
            let (key_expr_raw, where_raw) = parse_postgres_indexdef(&r.index_def);
            IndexInfo {
                name: r.index_name,
                table: r.table_name,
                unique: r.is_unique,
                key_signature: normalize_expr_list(&key_expr_raw),
                where_signature: where_raw.map(|w| normalize_single_expr(&w)),
            }
        })
        .collect()
}

async fn introspect_postgres_primary_keys(pool: &sqlx::PgPool) -> PrimaryKeys {
    #[derive(sqlx::FromRow)]
    struct Row {
        table_name: String,
        pk_columns: Vec<String>,
    }

    let rows: Vec<Row> = sqlx::query_as(
        r#"
        SELECT
            cl.relname AS table_name,
            ARRAY(
                SELECT att.attname::text
                FROM unnest(ix.indkey::int2[]) WITH ORDINALITY AS k(attnum, ord)
                JOIN pg_attribute att ON att.attrelid = ix.indrelid AND att.attnum = k.attnum
                ORDER BY k.ord
            ) AS pk_columns
        FROM pg_index ix
        JOIN pg_class cl ON cl.oid = ix.indrelid
        JOIN pg_namespace n ON n.oid = cl.relnamespace
        WHERE n.nspname = 'public' AND ix.indisprimary AND cl.relkind = 'r'
          AND cl.relname <> '_sqlx_migrations'
        "#,
    )
    .fetch_all(pool)
    .await
    .expect("introspect Postgres primary keys via pg_index");

    rows.into_iter()
        .map(|r| {
            (
                r.table_name,
                PrimaryKeyInfo {
                    columns: r.pk_columns.into_iter().map(|c| c.to_ascii_lowercase()).collect(),
                },
            )
        })
        .collect()
}

async fn introspect_postgres_foreign_keys(pool: &sqlx::PgPool) -> Vec<ForeignKeyInfo> {
    #[derive(sqlx::FromRow)]
    struct Row {
        table_name: String,
        source_columns: Vec<String>,
        target_table: String,
        target_columns: Vec<String>,
        on_delete: String,
        on_update: String,
    }

    let rows: Vec<Row> = sqlx::query_as(
        r#"
        SELECT
            cl.relname AS table_name,
            ARRAY(
                SELECT att.attname::text
                FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord)
                JOIN pg_attribute att ON att.attrelid = con.conrelid AND att.attnum = k.attnum
                ORDER BY k.ord
            ) AS source_columns,
            fcl.relname AS target_table,
            ARRAY(
                SELECT att.attname::text
                FROM unnest(con.confkey) WITH ORDINALITY AS k(attnum, ord)
                JOIN pg_attribute att ON att.attrelid = con.confrelid AND att.attnum = k.attnum
                ORDER BY k.ord
            ) AS target_columns,
            CAST(con.confdeltype AS text) AS on_delete,
            CAST(con.confupdtype AS text) AS on_update
        FROM pg_constraint con
        JOIN pg_class cl ON cl.oid = con.conrelid
        JOIN pg_namespace ncl ON ncl.oid = cl.relnamespace
        JOIN pg_class fcl ON fcl.oid = con.confrelid
        WHERE con.contype = 'f' AND ncl.nspname = 'public' AND cl.relname <> '_sqlx_migrations'
        ORDER BY cl.relname, con.conname
        "#,
    )
    .fetch_all(pool)
    .await
    .expect("introspect Postgres foreign keys via pg_constraint");

    rows.into_iter()
        .map(|r| ForeignKeyInfo {
            table: r.table_name,
            source_columns: r.source_columns.into_iter().map(|c| c.to_ascii_lowercase()).collect(),
            target_table: r.target_table,
            target_columns: r.target_columns.into_iter().map(|c| c.to_ascii_lowercase()).collect(),
            on_delete: parse_pg_ref_action(&r.on_delete),
            on_update: parse_pg_ref_action(&r.on_update),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// SQLite introspection — indexes, primary keys, foreign keys
// ---------------------------------------------------------------------------

async fn introspect_sqlite_indexes(pool: &sqlx::SqlitePool, tables: &[String]) -> Vec<IndexInfo> {
    #[derive(sqlx::FromRow)]
    struct IndexListRow {
        name: String,
        #[sqlx(rename = "unique")]
        is_unique: i64,
        origin: String,
    }
    #[derive(sqlx::FromRow)]
    struct IndexInfoColRow {
        name: Option<String>,
    }

    let mut result = Vec::new();
    for table in tables {
        let escaped = table.replace('"', "\"\"");
        let pragma = format!("PRAGMA index_list(\"{escaped}\")");
        let idx_list: Vec<IndexListRow> = sqlx::query_as(&pragma)
            .fetch_all(pool)
            .await
            .unwrap_or_else(|e| panic!("PRAGMA index_list(\"{table}\"): {e}"));

        for idx in idx_list {
            // Primary keys are handled separately via
            // introspect_sqlite_primary_keys — see its doc comment for the
            // rowid-alias caveat this avoids double-counting.
            if idx.origin == "pk" {
                continue;
            }
            let unique = idx.is_unique != 0;

            if idx.origin == "c" {
                // Explicit `CREATE INDEX` — sqlite_master.sql has the full
                // DDL text, needed to see expression keys (PRAGMA
                // index_info reports NULL column names for those).
                let sql: Option<String> =
                    sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type = 'index' AND name = ?")
                        .bind(&idx.name)
                        .fetch_optional(pool)
                        .await
                        .unwrap_or_else(|e| panic!("look up sqlite_master.sql for index `{}`: {e}", idx.name))
                        .flatten();
                let sql = sql.unwrap_or_else(|| {
                    panic!("sqlite_master.sql has no DDL text for CREATE INDEX `{}`", idx.name)
                });
                let (key_expr_raw, where_raw) = parse_sqlite_indexdef(&sql);
                result.push(IndexInfo {
                    name: idx.name,
                    table: table.clone(),
                    unique,
                    key_signature: normalize_expr_list(&key_expr_raw),
                    where_signature: where_raw.map(|w| normalize_single_expr(&w)),
                });
            } else {
                // origin == "u": an inline `UNIQUE(...)` table constraint.
                // sqlite_master.sql is NULL for these (SQLite auto-creates
                // an unnamed backing index) — SQLite table constraints
                // can't express expressions, only plain columns, so
                // PRAGMA index_info's column names are always non-NULL here.
                let escaped_idx = idx.name.replace('"', "\"\"");
                let info_pragma = format!("PRAGMA index_info(\"{escaped_idx}\")");
                let cols: Vec<IndexInfoColRow> = sqlx::query_as(&info_pragma)
                    .fetch_all(pool)
                    .await
                    .unwrap_or_else(|e| panic!("PRAGMA index_info(\"{}\"): {e}", idx.name));
                let key_signature: Vec<String> = cols
                    .into_iter()
                    .map(|c| {
                        c.name.unwrap_or_else(|| {
                            panic!(
                                "unique constraint `{}` reported a NULL column name from PRAGMA index_info \
                                 — SQLite table constraints cannot express index expressions, so this is unexpected",
                                idx.name
                            )
                        })
                    })
                    .map(|c| c.to_ascii_lowercase())
                    .collect();
                result.push(IndexInfo {
                    name: idx.name,
                    table: table.clone(),
                    unique,
                    key_signature,
                    where_signature: None,
                });
            }
        }
    }
    result
}

/// Derives each table's primary key from `PRAGMA table_info`'s `pk` column
/// rather than `PRAGMA index_list`.
///
/// A table declared `id INTEGER PRIMARY KEY` is a rowid alias — SQLite
/// creates *no* backing index for it, so `PRAGMA index_list` reports
/// nothing even though Postgres has an explicit `_pkey` index for the same
/// logical column. Deriving from `table_info` sidesteps that entirely,
/// which is also why [`introspect_sqlite_indexes`] excludes `origin = "pk"`
/// rows — without that exclusion, a composite (non-rowid-alias) primary
/// key would be reported by both functions.
async fn introspect_sqlite_primary_keys(pool: &sqlx::SqlitePool, tables: &[String]) -> PrimaryKeys {
    #[derive(sqlx::FromRow)]
    struct ColRow {
        name: String,
        pk: i64,
    }

    let mut result = PrimaryKeys::new();
    for table in tables {
        let escaped = table.replace('"', "\"\"");
        let pragma = format!("PRAGMA table_info(\"{escaped}\")");
        let cols: Vec<ColRow> = sqlx::query_as(&pragma)
            .fetch_all(pool)
            .await
            .unwrap_or_else(|e| panic!("PRAGMA table_info(\"{table}\"): {e}"));

        let mut pk_cols: Vec<(i64, String)> =
            cols.into_iter().filter(|c| c.pk > 0).map(|c| (c.pk, c.name.to_ascii_lowercase())).collect();
        if !pk_cols.is_empty() {
            pk_cols.sort_by_key(|(ord, _)| *ord);
            result.insert(table.clone(), PrimaryKeyInfo { columns: pk_cols.into_iter().map(|(_, n)| n).collect() });
        }
    }
    result
}

async fn introspect_sqlite_foreign_keys(
    pool: &sqlx::SqlitePool,
    tables: &[String],
    primary_keys: &PrimaryKeys,
) -> Vec<ForeignKeyInfo> {
    #[derive(sqlx::FromRow)]
    struct FkRow {
        id: i64,
        seq: i64,
        table: String,
        from: String,
        to: Option<String>,
        on_update: String,
        on_delete: String,
    }

    let mut result = Vec::new();
    for table in tables {
        let escaped = table.replace('"', "\"\"");
        let pragma = format!("PRAGMA foreign_key_list(\"{escaped}\")");
        let rows: Vec<FkRow> = sqlx::query_as(&pragma)
            .fetch_all(pool)
            .await
            .unwrap_or_else(|e| panic!("PRAGMA foreign_key_list(\"{table}\"): {e}"));

        let mut grouped: BTreeMap<i64, Vec<FkRow>> = BTreeMap::new();
        for r in rows {
            grouped.entry(r.id).or_default().push(r);
        }
        for group in grouped.into_values() {
            let mut group = group;
            group.sort_by_key(|r| r.seq);
            let target_table = group[0].table.clone();
            let on_delete = parse_sqlite_ref_action(&group[0].on_delete);
            let on_update = parse_sqlite_ref_action(&group[0].on_update);
            let source_columns: Vec<String> =
                group.iter().map(|r| r.from.to_ascii_lowercase()).collect();
            // `to` is NULL when the FK targets the parent's primary key
            // implicitly (no column list given in the REFERENCES clause) —
            // resolve it to the target table's actual PK columns rather
            // than comparing NULL against Postgres's explicit column list.
            let target_columns: Vec<String> = if group.iter().all(|r| r.to.is_some()) {
                group.iter().map(|r| r.to.clone().unwrap().to_ascii_lowercase()).collect()
            } else {
                primary_keys.get(&target_table).map(|pk| pk.columns.clone()).unwrap_or_default()
            };
            result.push(ForeignKeyInfo {
                table: table.clone(),
                source_columns,
                target_table,
                target_columns,
                on_delete,
                on_update,
            });
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Allowlist
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct AllowlistFile {
    #[serde(default)]
    entry: Vec<AllowlistEntry>,
}

#[derive(serde::Deserialize)]
struct AllowlistEntry {
    table: String,
    #[serde(default)]
    column: Option<String>,
    #[serde(default)]
    index: Option<String>,
    #[serde(default)]
    foreign_key: Option<String>,
    /// Human-readable justification, validated at load time — see
    /// `Allowlist::load`. An allowlist entry without a real reason is how
    /// this check quietly decays into a rubber stamp, so a blank one is
    /// rejected rather than merely discouraged.
    reason: String,
}

struct Allowlist {
    whole_table: HashSet<String>,
    column: HashSet<(String, String)>,
    index: HashSet<(String, String)>,
    foreign_key: HashSet<(String, String)>,
}

impl Allowlist {
    fn load(path: &str) -> Self {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read schema-parity allowlist at {path}: {e}"));
        let parsed: AllowlistFile = toml::from_str(&text)
            .unwrap_or_else(|e| panic!("parse schema-parity allowlist at {path}: {e}"));

        let mut whole_table = HashSet::new();
        let mut column = HashSet::new();
        let mut index = HashSet::new();
        let mut foreign_key = HashSet::new();
        for e in parsed.entry {
            // Named so a malformed entry's panic says exactly which
            // dimension(s) it set, not just the table — otherwise a broken
            // `index`/`foreign_key` entry's error message silently looks
            // like a plain column (or whole-table) entry went wrong.
            let mut dims_set = Vec::new();
            if let Some(c) = &e.column {
                dims_set.push(format!("column=\"{c}\""));
            }
            if let Some(i) = &e.index {
                dims_set.push(format!("index=\"{i}\""));
            }
            if let Some(fk) = &e.foreign_key {
                dims_set.push(format!("foreign_key=\"{fk}\""));
            }
            let dims_desc =
                if dims_set.is_empty() { String::new() } else { format!(" ({})", dims_set.join(", ")) };

            // Enforce the justification rather than just asking for it. The
            // whole value of this allowlist is that every waiver states why
            // the divergence is deliberate; an entry added with an empty
            // reason to make a failing build green is exactly the outcome
            // this check exists to prevent.
            assert!(
                !e.reason.trim().is_empty(),
                "schema-parity allowlist entry for table `{}`{dims_desc} has an empty `reason`. \
                 Every waiver must justify why the divergence is deliberate — \
                 see the header comment in {path}.",
                e.table,
            );
            let dimensions_set =
                [e.column.is_some(), e.index.is_some(), e.foreign_key.is_some()].into_iter().filter(|b| *b).count();
            assert!(
                dimensions_set <= 1,
                "schema-parity allowlist entry for table `{}`{dims_desc} specifies more than one of \
                 column/index/foreign_key — an entry may waive at most one dimension \
                 (omit all three to waive the whole table). See the header comment in {path}.",
                e.table,
            );
            match (e.column, e.index, e.foreign_key) {
                (None, None, None) => {
                    whole_table.insert(e.table);
                }
                (Some(c), None, None) => {
                    column.insert((e.table, c));
                }
                (None, Some(i), None) => {
                    index.insert((e.table, i));
                }
                (None, None, Some(fk)) => {
                    foreign_key.insert((e.table, fk));
                }
                _ => unreachable!("dimensions_set <= 1 was asserted above"),
            }
        }
        Self { whole_table, column, index, foreign_key }
    }

    fn waives_table(&self, table: &str) -> bool {
        self.whole_table.contains(table)
    }

    fn waives_column(&self, table: &str, column: &str) -> bool {
        self.whole_table.contains(table)
            || self.column.contains(&(table.to_string(), column.to_string()))
    }

    fn waives_index(&self, table: &str, index_name: &str) -> bool {
        self.whole_table.contains(table)
            || self.index.contains(&(table.to_string(), index_name.to_string()))
    }

    fn waives_foreign_key(&self, table: &str, source_columns_key: &str) -> bool {
        self.whole_table.contains(table)
            || self.foreign_key.contains(&(table.to_string(), source_columns_key.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Diff — columns
// ---------------------------------------------------------------------------

fn diff_schemas(pg: &DbSchema, sqlite: &DbSchema, allow: &Allowlist) -> Vec<String> {
    let mut findings = Vec::new();
    let all_tables: BTreeSet<&String> = pg.keys().chain(sqlite.keys()).collect();

    for table in all_tables {
        match (pg.get(table), sqlite.get(table)) {
            (Some(_), None) => {
                if !allow.waives_table(table) {
                    findings.push(format!(
                        "table `{table}` exists in Postgres (apps/server/migrations) \
                         but is missing from SQLite (apps/server/migrations-sqlite)"
                    ));
                }
            }
            (None, Some(_)) => {
                if !allow.waives_table(table) {
                    findings.push(format!(
                        "table `{table}` exists in SQLite (apps/server/migrations-sqlite) \
                         but is missing from Postgres (apps/server/migrations)"
                    ));
                }
            }
            (Some(pg_cols), Some(sqlite_cols)) => {
                if allow.waives_table(table) {
                    continue;
                }
                let all_cols: BTreeSet<&String> = pg_cols.keys().chain(sqlite_cols.keys()).collect();
                for col in all_cols {
                    if allow.waives_column(table, col) {
                        continue;
                    }
                    match (pg_cols.get(col), sqlite_cols.get(col)) {
                        (Some(_), None) => findings.push(format!(
                            "column `{table}.{col}` exists in Postgres (apps/server/migrations) \
                             but is missing from SQLite (apps/server/migrations-sqlite)"
                        )),
                        (None, Some(_)) => findings.push(format!(
                            "column `{table}.{col}` exists in SQLite (apps/server/migrations-sqlite) \
                             but is missing from Postgres (apps/server/migrations)"
                        )),
                        (Some(p), Some(s)) => {
                            // Only compare when both sides classified. An
                            // unclassifiable type is left unchecked rather
                            // than asserted equivalent to something it isn't.
                            if let (Some(pc), Some(sc)) = (p.class, s.class)
                                && pc != sc
                            {
                                findings.push(format!(
                                    "column `{table}.{col}` type mismatch: \
                                     Postgres `{}` ({pc:?}) vs SQLite `{}` ({sc:?})",
                                    p.raw_type, s.raw_type
                                ));
                            }
                            if p.nullable != s.nullable {
                                findings.push(format!(
                                    "column `{table}.{col}` nullability mismatch: \
                                     Postgres nullable={} vs SQLite nullable={}",
                                    p.nullable, s.nullable
                                ));
                            }
                            if p.has_default != s.has_default {
                                findings.push(format!(
                                    "column `{table}.{col}` default-presence mismatch: \
                                     Postgres has_default={} vs SQLite has_default={}",
                                    p.has_default, s.has_default
                                ));
                            }
                        }
                        (None, None) => unreachable!("column came from the union of both keysets"),
                    }
                }
            }
            (None, None) => unreachable!("table came from the union of both keysets"),
        }
    }

    findings
}

// ---------------------------------------------------------------------------
// Diff — primary keys
// ---------------------------------------------------------------------------

fn diff_primary_keys(pg: &PrimaryKeys, sqlite: &PrimaryKeys, allow: &Allowlist) -> Vec<String> {
    let mut findings = Vec::new();
    let all_tables: BTreeSet<&String> = pg.keys().chain(sqlite.keys()).collect();

    for table in all_tables {
        if allow.waives_table(table) {
            continue;
        }
        match (pg.get(table), sqlite.get(table)) {
            (Some(p), Some(s)) => {
                if p.columns != s.columns {
                    findings.push(format!(
                        "primary key on `{table}` differs: Postgres columns `({})` vs SQLite columns `({})`",
                        p.columns.join(", "),
                        s.columns.join(", "),
                    ));
                }
            }
            (Some(p), None) => {
                findings.push(format!(
                    "table `{table}` has a primary key on Postgres (`{}`) but no primary key on SQLite",
                    p.columns.join(", "),
                ));
            }
            (None, Some(s)) => {
                findings.push(format!(
                    "table `{table}` has a primary key on SQLite (`{}`) but no primary key on Postgres",
                    s.columns.join(", "),
                ));
            }
            (None, None) => unreachable!("table came from the union of both keysets"),
        }
    }

    findings
}

// ---------------------------------------------------------------------------
// Diff — indexes (unique indexes / unique constraints only — see module doc)
// ---------------------------------------------------------------------------

fn diff_indexes(pg: &[IndexInfo], sqlite: &[IndexInfo], allow: &Allowlist) -> Vec<String> {
    let mut findings = Vec::new();

    let pg_by_name: BTreeMap<&str, &IndexInfo> = pg.iter().map(|i| (i.name.as_str(), i)).collect();
    let sqlite_by_name: BTreeMap<&str, &IndexInfo> = sqlite.iter().map(|i| (i.name.as_str(), i)).collect();
    let mut matched_names: BTreeSet<&str> = BTreeSet::new();

    // Pass 1: match by name. Hand-written indexes use identical explicit
    // names on both chains, so a name match yields a precise "same index,
    // different definition" finding.
    for (name, p) in &pg_by_name {
        let Some(s) = sqlite_by_name.get(name) else { continue };
        if p.table != s.table {
            // Same index name on two different tables is a name collision,
            // not the same index — comparing them would attribute one
            // table's index definition to a different table. Leave both
            // sides unmatched so each falls through to the per-table
            // signature-fallback pass (pass 2) instead.
            continue;
        }
        matched_names.insert(name);

        if allow.waives_table(&p.table) || allow.waives_table(&s.table) || allow.waives_index(&p.table, name) {
            continue;
        }
        // Non-unique/non-unique pairs are out of scope entirely (see module doc).
        if !p.unique && !s.unique {
            continue;
        }

        if p.unique != s.unique {
            findings.push(format!(
                "index `{name}` on `{}` differs: Postgres unique={} vs SQLite unique={}",
                p.table, p.unique, s.unique,
            ));
        }
        if p.key_signature != s.key_signature {
            findings.push(format!(
                "unique index `{name}` on `{}` differs: Postgres keys `({})` vs SQLite keys `({})`",
                p.table,
                p.key_signature.join(", "),
                s.key_signature.join(", "),
            ));
        }
        match (&p.where_signature, &s.where_signature) {
            (Some(pw), Some(sw)) if pw != sw => {
                findings.push(format!(
                    "unique index `{name}` on `{}` partial-predicate differs: \
                     Postgres WHERE ({pw}) vs SQLite WHERE ({sw})",
                    p.table
                ));
            }
            (Some(pw), None) => {
                findings.push(format!(
                    "unique index `{name}` on `{}` is partial on Postgres (WHERE {pw}) but not on SQLite",
                    p.table
                ));
            }
            (None, Some(sw)) => {
                findings.push(format!(
                    "unique index `{name}` on `{}` is partial on SQLite (WHERE {sw}) but not on Postgres",
                    p.table
                ));
            }
            _ => {}
        }
    }

    // Pass 2: everything left unmatched by name — auto-generated names
    // (Postgres `watches_pkey`-style constraint names, SQLite
    // `sqlite_autoindex_*`) never match across backends, so fall back to
    // comparing the set of normalized unique key signatures per table.
    // Only unique indexes participate (module doc: non-unique is out of
    // scope), and anything already reported in pass 1 is excluded by
    // `matched_names` so it isn't double-reported.
    let pg_unmatched: Vec<&IndexInfo> =
        pg.iter().filter(|i| i.unique && !matched_names.contains(i.name.as_str())).collect();
    let sqlite_unmatched: Vec<&IndexInfo> =
        sqlite.iter().filter(|i| i.unique && !matched_names.contains(i.name.as_str())).collect();

    let tables: BTreeSet<&str> =
        pg_unmatched.iter().map(|i| i.table.as_str()).chain(sqlite_unmatched.iter().map(|i| i.table.as_str())).collect();

    for table in tables {
        if allow.waives_table(table) {
            continue;
        }
        let pg_for_table: Vec<&&IndexInfo> = pg_unmatched.iter().filter(|i| i.table == table).collect();
        let sqlite_for_table: Vec<&&IndexInfo> = sqlite_unmatched.iter().filter(|i| i.table == table).collect();

        for p in &pg_for_table {
            if allow.waives_index(table, &p.name) {
                continue;
            }
            let has_match = sqlite_for_table
                .iter()
                .any(|s| s.key_signature == p.key_signature && s.where_signature == p.where_signature);
            if !has_match {
                findings.push(format!(
                    "unique index `{}` on `{table}` (Postgres keys `({})`) has no matching \
                     unique constraint/index on SQLite",
                    p.name,
                    p.key_signature.join(", "),
                ));
            }
        }
        for s in &sqlite_for_table {
            if allow.waives_index(table, &s.name) {
                continue;
            }
            let has_match = pg_for_table
                .iter()
                .any(|p| p.key_signature == s.key_signature && p.where_signature == s.where_signature);
            if !has_match {
                findings.push(format!(
                    "unique index `{}` on `{table}` (SQLite keys `({})`) has no matching \
                     unique constraint/index on Postgres",
                    s.name,
                    s.key_signature.join(", "),
                ));
            }
        }
    }

    findings
}

// ---------------------------------------------------------------------------
// Diff — foreign keys
// ---------------------------------------------------------------------------

fn diff_foreign_keys(pg: &[ForeignKeyInfo], sqlite: &[ForeignKeyInfo], allow: &Allowlist) -> Vec<String> {
    let mut findings = Vec::new();

    // SQLite foreign keys are unnamed, so (table, source columns) is the
    // only stable cross-backend key — matches the allowlist's `foreign_key`
    // field, which is documented as a comma-joined source-column list.
    let key_of = |fk: &ForeignKeyInfo| (fk.table.clone(), fk.source_columns.join(","));

    let pg_by_key: BTreeMap<(String, String), &ForeignKeyInfo> = pg.iter().map(|fk| (key_of(fk), fk)).collect();
    let sqlite_by_key: BTreeMap<(String, String), &ForeignKeyInfo> =
        sqlite.iter().map(|fk| (key_of(fk), fk)).collect();
    let all_keys: BTreeSet<(String, String)> =
        pg_by_key.keys().cloned().chain(sqlite_by_key.keys().cloned()).collect();

    for (table, source_cols) in all_keys {
        if allow.waives_table(&table) || allow.waives_foreign_key(&table, &source_cols) {
            continue;
        }
        match (pg_by_key.get(&(table.clone(), source_cols.clone())), sqlite_by_key.get(&(table.clone(), source_cols.clone()))) {
            (Some(p), Some(s)) => {
                if p.target_table != s.target_table {
                    findings.push(format!(
                        "foreign key on `{table}` ({source_cols}) targets a different table: \
                         Postgres `{}` vs SQLite `{}`",
                        p.target_table, s.target_table,
                    ));
                }
                if p.target_columns != s.target_columns {
                    findings.push(format!(
                        "foreign key on `{table}` ({source_cols}) targets different columns: \
                         Postgres `({})` vs SQLite `({})`",
                        p.target_columns.join(", "),
                        s.target_columns.join(", "),
                    ));
                }
                if p.on_delete != s.on_delete {
                    findings.push(format!(
                        "foreign key on `{table}` ({source_cols}) ON DELETE action differs: \
                         Postgres {} vs SQLite {}",
                        p.on_delete, s.on_delete,
                    ));
                }
                if p.on_update != s.on_update {
                    findings.push(format!(
                        "foreign key on `{table}` ({source_cols}) ON UPDATE action differs: \
                         Postgres {} vs SQLite {}",
                        p.on_update, s.on_update,
                    ));
                }
            }
            (Some(p), None) => {
                findings.push(format!(
                    "foreign key on `{table}` ({source_cols}) -> `{}` exists on Postgres but not on SQLite",
                    p.target_table,
                ));
            }
            (None, Some(s)) => {
                findings.push(format!(
                    "foreign key on `{table}` ({source_cols}) -> `{}` exists on SQLite but not on Postgres",
                    s.target_table,
                ));
            }
            (None, None) => unreachable!("key came from the union of both keysets"),
        }
    }

    findings
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migration_chains_produce_matching_schemas() {
    let allowlist_path =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/server/schema-parity-allowlist.toml");
    let allow = Allowlist::load(allowlist_path);

    // Same server-resolution rule as kyomi_core::Config::test_config() /
    // apps/server/tests contract suite (kyomi_core::test_db), so this needs
    // no env override against the CI postgres service or a local
    // `kyomi-postgres-test` container. Only the server part matters here —
    // the database name itself is discarded in favor of our own scratch
    // database below.
    let base_url = kyomi_core::test_db::test_database_url();
    let (server_url, _) = kyomi_core::test_db::split_database_url(&base_url);

    // KYO-242: never touch the shared contract-test database — create and
    // drop our own scratch database on the same server so this check is
    // hermetic (can't be poisoned by, or poison, any other worktree/suite).
    let scratch_db = format!("kyomi_schema_diff_{}", uuid::Uuid::new_v4().simple());

    let admin_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&format!("{server_url}/postgres"))
        .await
        .unwrap_or_else(|e| {
            panic!(
                "connect to Postgres admin database at {server_url}/postgres \
                 (is the test Postgres container running? see CLAUDE.md): {e}"
            )
        });

    sqlx::query(&format!("CREATE DATABASE \"{scratch_db}\""))
        .execute(&admin_pool)
        .await
        .unwrap_or_else(|e| panic!("create scratch database `{scratch_db}`: {e}"));

    let scratch_url = format!("{server_url}/{scratch_db}");

    // Run the real embedded Postgres migration chain (crates/kyomi-core/src/db.rs:28)
    // against the scratch database, then introspect it — columns, indexes,
    // primary keys, and foreign keys all from the same pool/pass.
    let (pg_schema, pg_indexes, pg_primary_keys, pg_foreign_keys) = {
        let pool = kyomi_core::db::DbPool::connect(&scratch_url)
            .await
            .expect("run Postgres migration chain against scratch database");
        let schema = introspect_postgres(pool.pg_pool()).await;
        let indexes = introspect_postgres_indexes(pool.pg_pool()).await;
        let primary_keys = introspect_postgres_primary_keys(pool.pg_pool()).await;
        let foreign_keys = introspect_postgres_foreign_keys(pool.pg_pool()).await;
        pool.pg_pool().close().await;
        (schema, indexes, primary_keys, foreign_keys)
    };

    // Run the real embedded SQLite migration chain (crates/kyomi-core/src/db.rs:45)
    // against a fresh in-memory database, then introspect it — same
    // same-pass principle as the Postgres side above.
    let (sqlite_schema, sqlite_indexes, sqlite_primary_keys, sqlite_foreign_keys) = {
        let pool = kyomi_core::db::DbPool::connect("sqlite::memory:")
            .await
            .expect("run SQLite migration chain against in-memory database");
        match &pool {
            kyomi_core::db::DbPool::Sqlite(sq) => {
                let schema = introspect_sqlite(sq).await;
                let tables: Vec<String> = schema.keys().cloned().collect();
                let primary_keys = introspect_sqlite_primary_keys(sq, &tables).await;
                let indexes = introspect_sqlite_indexes(sq, &tables).await;
                let foreign_keys = introspect_sqlite_foreign_keys(sq, &tables, &primary_keys).await;
                (schema, indexes, primary_keys, foreign_keys)
            }
            kyomi_core::db::DbPool::Postgres(_) => {
                unreachable!("sqlite::memory: URL must select the SQLite backend")
            }
        }
    };

    // Drop the scratch database before evaluating the diff, so cleanup runs
    // regardless of whether the check below passes or fails.
    sqlx::query(&format!("DROP DATABASE \"{scratch_db}\""))
        .execute(&admin_pool)
        .await
        .unwrap_or_else(|e| panic!("drop scratch database `{scratch_db}`: {e}"));

    // Indexes/PKs/FKs are only meaningful to compare for tables that exist
    // on both sides — a table missing entirely from one backend is already
    // reported by diff_schemas, and comparing its indexes/FKs too would
    // just be confusing noise about a table that doesn't exist there.
    let common_tables: BTreeSet<String> =
        pg_schema.keys().filter(|t| sqlite_schema.contains_key(*t)).cloned().collect();

    let pg_indexes: Vec<IndexInfo> = pg_indexes.into_iter().filter(|i| common_tables.contains(&i.table)).collect();
    let sqlite_indexes: Vec<IndexInfo> =
        sqlite_indexes.into_iter().filter(|i| common_tables.contains(&i.table)).collect();
    let pg_primary_keys: PrimaryKeys =
        pg_primary_keys.into_iter().filter(|(t, _)| common_tables.contains(t)).collect();
    let sqlite_primary_keys: PrimaryKeys =
        sqlite_primary_keys.into_iter().filter(|(t, _)| common_tables.contains(t)).collect();
    let pg_foreign_keys: Vec<ForeignKeyInfo> =
        pg_foreign_keys.into_iter().filter(|fk| common_tables.contains(&fk.table)).collect();
    let sqlite_foreign_keys: Vec<ForeignKeyInfo> =
        sqlite_foreign_keys.into_iter().filter(|fk| common_tables.contains(&fk.table)).collect();

    let mut findings = diff_schemas(&pg_schema, &sqlite_schema, &allow);
    findings.extend(diff_primary_keys(&pg_primary_keys, &sqlite_primary_keys, &allow));
    findings.extend(diff_indexes(&pg_indexes, &sqlite_indexes, &allow));
    findings.extend(diff_foreign_keys(&pg_foreign_keys, &sqlite_foreign_keys, &allow));

    assert!(
        findings.is_empty(),
        "\n\nMigration chains produced {} divergent schema finding(s) not covered by \
         apps/server/schema-parity-allowlist.toml:\n\n{}\n\n\
         This is the exact bug class KYO-200 shipped twice (collections.doc_type,\n\
         thinking_event_details existed only on Postgres and broke or silently\n\
         disabled the feature on self-hosted SQLite). If this divergence is a bug,\n\
         fix the migration. If it is genuinely deliberate, add a narrow entry to\n\
         apps/server/schema-parity-allowlist.toml with a justification.\n",
        findings.len(),
        findings.iter().map(|f| format!("  - {f}")).collect::<Vec<_>>().join("\n")
    );
}

// ---------------------------------------------------------------------------
// Unit tests for diff_schemas / diff_primary_keys / diff_indexes /
// diff_foreign_keys / Allowlist / classify_* / normalize_* — DB-free, fast.
//
// The single `#[tokio::test]` above proves the check works end-to-end
// against real migrations, but it can only ever exercise whatever
// divergence shape happens to exist between the two chains *today*. Every
// branch below needs its own coverage against hand-built fixtures —
// otherwise a swapped `p`/`s`, an inverted `!=`, or a backend label the
// wrong way round in one of those branches would go unnoticed until a real
// divergence of that exact shape appeared, at which point the check would
// either stay silent or report it backwards.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn col(class: Option<TypeClass>, raw_type: &str, nullable: bool, has_default: bool) -> ColumnInfo {
        ColumnInfo { class, raw_type: raw_type.to_string(), nullable, has_default }
    }

    fn schema(tables: &[(&str, &[(&str, ColumnInfo)])]) -> DbSchema {
        tables
            .iter()
            .map(|(table, cols)| {
                let table_schema: TableSchema =
                    cols.iter().map(|(name, info)| (name.to_string(), info.clone())).collect();
                (table.to_string(), table_schema)
            })
            .collect()
    }

    fn empty_allowlist() -> Allowlist {
        Allowlist {
            whole_table: HashSet::new(),
            column: HashSet::new(),
            index: HashSet::new(),
            foreign_key: HashSet::new(),
        }
    }

    fn allowlist_with(whole_table: &[&str], columns: &[(&str, &str)]) -> Allowlist {
        Allowlist {
            whole_table: whole_table.iter().map(|s| s.to_string()).collect(),
            column: columns.iter().map(|(t, c)| (t.to_string(), c.to_string())).collect(),
            index: HashSet::new(),
            foreign_key: HashSet::new(),
        }
    }

    fn allowlist_with_index(pairs: &[(&str, &str)]) -> Allowlist {
        Allowlist {
            whole_table: HashSet::new(),
            column: HashSet::new(),
            index: pairs.iter().map(|(t, i)| (t.to_string(), i.to_string())).collect(),
            foreign_key: HashSet::new(),
        }
    }

    fn allowlist_with_foreign_key(pairs: &[(&str, &str)]) -> Allowlist {
        Allowlist {
            whole_table: HashSet::new(),
            column: HashSet::new(),
            index: HashSet::new(),
            foreign_key: pairs.iter().map(|(t, fk)| (t.to_string(), fk.to_string())).collect(),
        }
    }

    fn idx(name: &str, table: &str, unique: bool, keys: &[&str], where_sig: Option<&str>) -> IndexInfo {
        IndexInfo {
            name: name.to_string(),
            table: table.to_string(),
            unique,
            key_signature: keys.iter().map(|s| s.to_string()).collect(),
            where_signature: where_sig.map(|s| s.to_string()),
        }
    }

    fn fk(
        table: &str,
        source: &[&str],
        target_table: &str,
        target: &[&str],
        on_delete: RefAction,
        on_update: RefAction,
    ) -> ForeignKeyInfo {
        ForeignKeyInfo {
            table: table.to_string(),
            source_columns: source.iter().map(|s| s.to_string()).collect(),
            target_table: target_table.to_string(),
            target_columns: target.iter().map(|s| s.to_string()).collect(),
            on_delete,
            on_update,
        }
    }

    fn pk_map(pairs: &[(&str, &[&str])]) -> PrimaryKeys {
        pairs
            .iter()
            .map(|(t, cols)| {
                (t.to_string(), PrimaryKeyInfo { columns: cols.iter().map(|c| c.to_string()).collect() })
            })
            .collect()
    }

    // -- classify_postgres_type / classify_sqlite_type -----------------------

    #[test]
    fn classify_postgres_type_known_families() {
        for t in ["text", "character varying", "character", "uuid", "timestamp with time zone",
            "timestamp without time zone", "date", "json", "jsonb"]
        {
            assert_eq!(classify_postgres_type(t), Some(TypeClass::Text), "expected {t} to classify as Text");
        }
        for t in ["smallint", "integer", "bigint", "boolean"] {
            assert_eq!(classify_postgres_type(t), Some(TypeClass::Integer), "expected {t} to classify as Integer");
        }
        for t in ["double precision", "real", "numeric"] {
            assert_eq!(classify_postgres_type(t), Some(TypeClass::Real), "expected {t} to classify as Real");
        }
    }

    #[test]
    fn classify_postgres_type_unclassifiable_types_return_none() {
        // ARRAY (text[]), USER-DEFINED (pgvector's vector(384), enum types),
        // bytea, and tsvector must be left unchecked rather than guessed at
        // — asserting them equal to some class would be exactly the false
        // equivalence KYO-206 was told not to invent.
        for t in ["ARRAY", "USER-DEFINED", "bytea", "tsvector"] {
            assert_eq!(classify_postgres_type(t), None, "expected {t} to be unclassified");
        }
    }

    #[test]
    fn classify_sqlite_type_known_families() {
        assert_eq!(classify_sqlite_type("TEXT"), Some(TypeClass::Text));
        assert_eq!(classify_sqlite_type("text"), Some(TypeClass::Text), "classification must be case-insensitive");
        assert_eq!(classify_sqlite_type("INTEGER"), Some(TypeClass::Integer));
        assert_eq!(classify_sqlite_type("REAL"), Some(TypeClass::Real));
    }

    #[test]
    fn classify_sqlite_type_unclassifiable_types_return_none() {
        // BLOB (embedding columns) has no unambiguous Postgres counterpart —
        // must be left unchecked, not guessed at. NUMERIC isn't used
        // anywhere in migrations-sqlite today; it should still be
        // unclassified rather than silently treated as Real/Integer.
        for t in ["BLOB", "NUMERIC"] {
            assert_eq!(classify_sqlite_type(t), None, "expected {t} to be unclassified");
        }
    }

    // -- table-level presence -------------------------------------------------

    #[test]
    fn table_missing_from_sqlite_is_a_finding_when_unwaived() {
        let pg = schema(&[("only_pg", &[("id", col(Some(TypeClass::Text), "text", false, false))])]);
        let sqlite = DbSchema::new();

        let findings = diff_schemas(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("table `only_pg`"), "{}", findings[0]);
        assert!(findings[0].contains("exists in Postgres"), "{}", findings[0]);
        assert!(findings[0].contains("missing from SQLite"), "{}", findings[0]);
    }

    #[test]
    fn table_missing_from_sqlite_is_suppressed_by_whole_table_waiver() {
        let pg = schema(&[("only_pg", &[("id", col(Some(TypeClass::Text), "text", false, false))])]);
        let sqlite = DbSchema::new();

        let findings = diff_schemas(&pg, &sqlite, &allowlist_with(&["only_pg"], &[]));

        assert!(findings.is_empty(), "findings: {findings:?}");
    }

    #[test]
    fn table_missing_from_postgres_is_a_finding_when_unwaived() {
        let pg = DbSchema::new();
        let sqlite = schema(&[("only_sqlite", &[("id", col(Some(TypeClass::Text), "TEXT", false, false))])]);

        let findings = diff_schemas(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("table `only_sqlite`"), "{}", findings[0]);
        assert!(findings[0].contains("exists in SQLite"), "{}", findings[0]);
        assert!(findings[0].contains("missing from Postgres"), "{}", findings[0]);
    }

    #[test]
    fn table_missing_from_postgres_is_suppressed_by_whole_table_waiver() {
        let pg = DbSchema::new();
        let sqlite = schema(&[("only_sqlite", &[("id", col(Some(TypeClass::Text), "TEXT", false, false))])]);

        let findings = diff_schemas(&pg, &sqlite, &allowlist_with(&["only_sqlite"], &[]));

        assert!(findings.is_empty(), "findings: {findings:?}");
    }

    // -- column-level presence -------------------------------------------------

    #[test]
    fn column_missing_from_sqlite_is_a_finding_when_unwaived() {
        let pg = schema(&[(
            "t",
            &[
                ("shared", col(Some(TypeClass::Text), "text", false, false)),
                ("pg_only", col(Some(TypeClass::Text), "text", true, false)),
            ],
        )]);
        let sqlite = schema(&[("t", &[("shared", col(Some(TypeClass::Text), "TEXT", false, false))])]);

        let findings = diff_schemas(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("column `t.pg_only`"), "{}", findings[0]);
        assert!(findings[0].contains("exists in Postgres"), "{}", findings[0]);
        assert!(findings[0].contains("missing from SQLite"), "{}", findings[0]);
    }

    #[test]
    fn column_missing_from_postgres_is_a_finding_when_unwaived() {
        let pg = schema(&[("t", &[("shared", col(Some(TypeClass::Text), "text", false, false))])]);
        let sqlite = schema(&[(
            "t",
            &[
                ("shared", col(Some(TypeClass::Text), "TEXT", false, false)),
                ("sqlite_only", col(Some(TypeClass::Text), "TEXT", true, false)),
            ],
        )]);

        let findings = diff_schemas(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("column `t.sqlite_only`"), "{}", findings[0]);
        assert!(findings[0].contains("exists in SQLite"), "{}", findings[0]);
        assert!(findings[0].contains("missing from Postgres"), "{}", findings[0]);
    }

    #[test]
    fn column_missing_is_suppressed_by_column_waiver_but_not_its_siblings() {
        let pg = schema(&[(
            "t",
            &[
                ("waived", col(Some(TypeClass::Text), "text", false, false)),
                ("not_waived", col(Some(TypeClass::Text), "text", false, false)),
            ],
        )]);
        let sqlite = schema(&[("t", &[])]);

        let findings = diff_schemas(&pg, &sqlite, &allowlist_with(&[], &[("t", "waived")]));

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("column `t.not_waived`"), "{}", findings[0]);
    }

    // -- type-class / nullability / default-presence mismatches -------------
    //
    // These assert on message *content*, not just finding count — getting
    // the Postgres/SQLite sides backwards in the format! calls is precisely
    // the failure mode this coverage exists to catch, and a test that only
    // counts findings would still pass with the labels swapped.

    #[test]
    fn type_class_mismatch_names_column_and_attributes_each_raw_type_to_its_backend() {
        let pg = schema(&[("t", &[("c", col(Some(TypeClass::Integer), "integer", false, false))])]);
        let sqlite = schema(&[("t", &[("c", col(Some(TypeClass::Text), "TEXT", false, false))])]);

        let findings = diff_schemas(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        let msg = &findings[0];
        assert!(msg.contains("column `t.c`"), "{msg}");
        assert!(msg.contains("type mismatch"), "{msg}");
        assert!(msg.contains("Postgres `integer`"), "Postgres's raw type must be attributed to Postgres: {msg}");
        assert!(msg.contains("SQLite `TEXT`"), "SQLite's raw type must be attributed to SQLite: {msg}");
    }

    #[test]
    fn matching_type_class_produces_no_finding() {
        let pg = schema(&[("t", &[("c", col(Some(TypeClass::Integer), "bigint", false, false))])]);
        let sqlite = schema(&[("t", &[("c", col(Some(TypeClass::Integer), "INTEGER", false, false))])]);

        let findings = diff_schemas(&pg, &sqlite, &empty_allowlist());

        assert!(findings.is_empty(), "findings: {findings:?}");
    }

    #[test]
    fn unclassified_type_on_either_side_skips_the_type_check_but_still_checks_nullability() {
        // Postgres `vector(384)` (unclassified) vs SQLite BLOB (unclassified)
        // — same shape as every embedding column in the real schema. Must
        // not be reported as a type mismatch (that would be exactly the
        // false equivalence this check is told not to invent, applied in
        // reverse), but a genuine nullability difference alongside it must
        // still be caught.
        let pg = schema(&[("t", &[("embedding", col(None, "vector", false, false))])]);
        let sqlite = schema(&[("t", &[("embedding", col(None, "BLOB", true, false))])]);

        let findings = diff_schemas(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "type check should be skipped, only nullability should fire: {findings:?}");
        assert!(findings[0].contains("nullability mismatch"), "{}", findings[0]);
    }

    #[test]
    fn nullability_mismatch_attributes_each_value_to_its_backend() {
        let pg = schema(&[("t", &[("c", col(Some(TypeClass::Text), "text", false, false))])]);
        let sqlite = schema(&[("t", &[("c", col(Some(TypeClass::Text), "TEXT", true, false))])]);

        let findings = diff_schemas(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        let msg = &findings[0];
        assert!(msg.contains("column `t.c`"), "{msg}");
        assert!(msg.contains("nullability mismatch"), "{msg}");
        assert!(msg.contains("Postgres nullable=false"), "Postgres's value must be attributed to Postgres: {msg}");
        assert!(msg.contains("SQLite nullable=true"), "SQLite's value must be attributed to SQLite: {msg}");
    }

    #[test]
    fn default_presence_mismatch_attributes_each_value_to_its_backend() {
        let pg = schema(&[("t", &[("c", col(Some(TypeClass::Text), "text", false, true))])]);
        let sqlite = schema(&[("t", &[("c", col(Some(TypeClass::Text), "TEXT", false, false))])]);

        let findings = diff_schemas(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        let msg = &findings[0];
        assert!(msg.contains("column `t.c`"), "{msg}");
        assert!(msg.contains("default-presence mismatch"), "{msg}");
        assert!(msg.contains("Postgres has_default=true"), "Postgres's value must be attributed to Postgres: {msg}");
        assert!(msg.contains("SQLite has_default=false"), "SQLite's value must be attributed to SQLite: {msg}");
    }

    #[test]
    fn a_single_column_can_produce_multiple_findings_at_once() {
        // type class, nullability, AND default-presence all differ on the
        // same column — diff_schemas must report all three, not short-circuit
        // after the first.
        let pg = schema(&[("t", &[("c", col(Some(TypeClass::Integer), "integer", false, true))])]);
        let sqlite = schema(&[("t", &[("c", col(Some(TypeClass::Text), "TEXT", true, false))])]);

        let findings = diff_schemas(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 3, "findings: {findings:?}");
        assert!(findings.iter().any(|f| f.contains("type mismatch")));
        assert!(findings.iter().any(|f| f.contains("nullability mismatch")));
        assert!(findings.iter().any(|f| f.contains("default-presence mismatch")));
    }

    // -- whole-table waiver short-circuits per-column checks -----------------

    #[test]
    fn whole_table_waiver_short_circuits_every_per_column_check() {
        let pg = schema(&[(
            "t",
            &[
                ("a", col(Some(TypeClass::Integer), "integer", false, false)),
                ("pg_only", col(Some(TypeClass::Text), "text", true, false)),
            ],
        )]);
        // "a" differs in every dimension (type, nullability, default), and
        // "pg_only" is missing entirely — all of it must be suppressed by
        // the whole-table waiver, none of it by column-level matching.
        let sqlite = schema(&[("t", &[("a", col(Some(TypeClass::Text), "TEXT", true, true))])]);

        let findings = diff_schemas(&pg, &sqlite, &allowlist_with(&["t"], &[]));

        assert!(findings.is_empty(), "whole-table waiver should suppress every per-column finding: {findings:?}");
    }

    // -- Allowlist::load ---------------------------------------------------

    #[test]
    fn allowlist_load_parses_whole_table_and_column_entries() {
        let path = std::env::temp_dir().join(format!(
            "kyomi-schema-parity-allowlist-test-parse-{}.toml",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &path,
            "[[entry]]\n\
             table = \"whole\"\n\
             reason = \"whole-table waiver for test\"\n\
             \n\
             [[entry]]\n\
             table = \"t\"\n\
             column = \"c\"\n\
             reason = \"column waiver for test\"\n",
        )
        .expect("write temp allowlist fixture");

        let allow = Allowlist::load(path.to_str().expect("temp path is valid UTF-8"));
        let _ = std::fs::remove_file(&path);

        assert!(allow.waives_table("whole"));
        assert!(allow.waives_column("t", "c"));
        assert!(!allow.waives_column("t", "other_column"));
    }

    #[test]
    fn allowlist_load_parses_index_and_foreign_key_entries() {
        let path = std::env::temp_dir().join(format!(
            "kyomi-schema-parity-allowlist-test-parse-index-fk-{}.toml",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &path,
            "[[entry]]\n\
             table = \"t\"\n\
             index = \"idx_a\"\n\
             reason = \"index waiver for test\"\n\
             \n\
             [[entry]]\n\
             table = \"t\"\n\
             foreign_key = \"a,b\"\n\
             reason = \"fk waiver for test\"\n",
        )
        .expect("write temp allowlist fixture");

        let allow = Allowlist::load(path.to_str().expect("temp path is valid UTF-8"));
        let _ = std::fs::remove_file(&path);

        assert!(allow.waives_index("t", "idx_a"));
        assert!(!allow.waives_index("t", "idx_b"));
        assert!(allow.waives_foreign_key("t", "a,b"));
        assert!(!allow.waives_foreign_key("t", "c"));
    }

    #[test]
    #[should_panic(expected = "empty `reason`")]
    fn allowlist_load_rejects_empty_reason() {
        let path = std::env::temp_dir().join(format!(
            "kyomi-schema-parity-allowlist-test-empty-reason-{}.toml",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, "[[entry]]\ntable = \"t\"\nreason = \"\"\n")
            .expect("write temp allowlist fixture");

        Allowlist::load(path.to_str().expect("temp path is valid UTF-8"));
    }

    #[test]
    #[should_panic(expected = "empty `reason`")]
    fn allowlist_load_rejects_whitespace_only_reason() {
        let path = std::env::temp_dir().join(format!(
            "kyomi-schema-parity-allowlist-test-whitespace-reason-{}.toml",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, "[[entry]]\ntable = \"t\"\nreason = \"   \"\n")
            .expect("write temp allowlist fixture");

        Allowlist::load(path.to_str().expect("temp path is valid UTF-8"));
    }

    #[test]
    #[should_panic(expected = "more than one of column/index/foreign_key")]
    fn allowlist_load_rejects_entry_specifying_both_column_and_index() {
        let path = std::env::temp_dir().join(format!(
            "kyomi-schema-parity-allowlist-test-multi-dimension-{}.toml",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &path,
            "[[entry]]\ntable = \"t\"\ncolumn = \"c\"\nindex = \"idx_a\"\nreason = \"bad entry\"\n",
        )
        .expect("write temp allowlist fixture");

        Allowlist::load(path.to_str().expect("temp path is valid UTF-8"));
    }

    #[test]
    #[should_panic(expected = "index=\"idx_a\"")]
    fn allowlist_load_empty_reason_panic_names_the_index_dimension() {
        let path = std::env::temp_dir().join(format!(
            "kyomi-schema-parity-allowlist-test-empty-reason-index-{}.toml",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, "[[entry]]\ntable = \"t\"\nindex = \"idx_a\"\nreason = \"\"\n")
            .expect("write temp allowlist fixture");

        Allowlist::load(path.to_str().expect("temp path is valid UTF-8"));
    }

    #[test]
    #[should_panic(expected = "foreign_key=\"a,b\"")]
    fn allowlist_load_empty_reason_panic_names_the_foreign_key_dimension() {
        let path = std::env::temp_dir().join(format!(
            "kyomi-schema-parity-allowlist-test-empty-reason-fk-{}.toml",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, "[[entry]]\ntable = \"t\"\nforeign_key = \"a,b\"\nreason = \"\"\n")
            .expect("write temp allowlist fixture");

        Allowlist::load(path.to_str().expect("temp path is valid UTF-8"));
    }

    #[test]
    #[should_panic(expected = "index=\"idx_a\", foreign_key=\"a,b\"")]
    fn allowlist_load_multi_dimension_panic_names_every_set_dimension() {
        let path = std::env::temp_dir().join(format!(
            "kyomi-schema-parity-allowlist-test-multi-dimension-names-{}.toml",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &path,
            "[[entry]]\ntable = \"t\"\nindex = \"idx_a\"\nforeign_key = \"a,b\"\nreason = \"bad entry\"\n",
        )
        .expect("write temp allowlist fixture");

        Allowlist::load(path.to_str().expect("temp path is valid UTF-8"));
    }

    // -- normalize_single_expr / normalize_expr_list -------------------------

    #[test]
    fn normalize_single_expr_strips_postgres_text_cast_and_redundant_parens() {
        assert_eq!(normalize_single_expr("lower((name)::text)"), "lower(name)");
    }

    #[test]
    fn normalize_single_expr_strips_public_schema_prefix() {
        assert_eq!(normalize_single_expr("public.watches.workspace_id"), "watches.workspace_id");
    }

    #[test]
    fn normalize_single_expr_strips_quoted_identifiers() {
        assert_eq!(normalize_single_expr("\"workspace_id\""), "workspace_id");
    }

    #[test]
    fn normalize_single_expr_collapses_whitespace() {
        assert_eq!(normalize_single_expr("  status   =    'pending'  "), "status = 'pending'");
    }

    #[test]
    fn normalize_single_expr_handles_the_real_unique_pending_transfer_where_clause() {
        // apps/server/migrations/20260215000000_baseline.sql:3266 —
        //   WHERE ((status)::text = 'pending'::text)
        // apps/server/migrations-sqlite/00001_baseline.sql:697 —
        //   WHERE status = 'pending'
        // These are the same predicate spelled two ways; normalization must
        // converge them, not report unique_pending_transfer as diverged.
        assert_eq!(
            normalize_single_expr("((status)::text = 'pending'::text)"),
            normalize_single_expr("status = 'pending'"),
        );
    }

    #[test]
    fn normalize_expr_list_splits_on_top_level_commas_only_and_normalizes_each_component() {
        assert_eq!(
            normalize_expr_list("workspace_id, lower((name)::text)"),
            vec!["workspace_id".to_string(), "lower(name)".to_string()],
        );
    }

    // -- string-literal-aware case folding (code review MAJOR finding) -------
    //
    // normalize_single_expr must never fold case inside a single-quoted
    // string literal — 'Active' and 'active' are different data, not
    // different spellings of the same identifier. Folding them together
    // would silently report two genuinely different partial-index
    // predicates as identical.

    #[test]
    fn normalize_single_expr_preserves_case_inside_string_literals() {
        assert_ne!(
            normalize_single_expr("status = 'Active'"),
            normalize_single_expr("status = 'active'"),
            "string literal content must not be case-folded — these are different values, \
             not the same predicate spelled two ways",
        );
        assert_eq!(normalize_single_expr("status = 'Active'"), "status = 'Active'");
    }

    #[test]
    fn normalize_single_expr_still_folds_case_outside_string_literals() {
        // Identifiers/keywords outside literals are still case-insensitive —
        // only literal *content* is protected from folding.
        assert_eq!(normalize_single_expr("STATUS = 'active'"), normalize_single_expr("status = 'active'"));
        assert_eq!(normalize_single_expr("STATUS = 'active'"), "status = 'active'");
    }

    #[test]
    fn lowercase_outside_string_literals_handles_doubled_quote_escape() {
        // 'IT''S MINE' is the SQL-escaped literal `IT'S MINE` — the doubled
        // quote must not be misread as "end of literal, start of a new
        // one," which would flip in_literal state incorrectly and start
        // folding the case of the real (non-literal) rest of the predicate.
        assert_eq!(lowercase_outside_string_literals("NAME = 'IT''S MINE'"), "name = 'IT''S MINE'");
    }

    #[test]
    fn lowercase_outside_string_literals_folds_case_before_and_after_a_literal() {
        assert_eq!(lowercase_outside_string_literals("STATUS = 'Active' AND X = 1"), "status = 'Active' and x = 1");
    }

    // -- parse_postgres_indexdef / parse_sqlite_indexdef ---------------------

    #[test]
    fn parse_postgres_indexdef_extracts_key_expr_with_no_where_clause() {
        let def = "CREATE UNIQUE INDEX idx_watches_name_workspace_unique ON public.watches \
                    USING btree (workspace_id, lower((name)::text))";
        let (key_expr, where_clause) = parse_postgres_indexdef(def);
        assert_eq!(key_expr, "workspace_id, lower((name)::text)");
        assert!(where_clause.is_none());
    }

    #[test]
    fn parse_postgres_indexdef_extracts_partial_where_clause() {
        let def = "CREATE UNIQUE INDEX unique_pending_transfer ON public.ownership_transfers \
                    USING btree (workspace_id) WHERE ((status)::text = 'pending'::text)";
        let (key_expr, where_clause) = parse_postgres_indexdef(def);
        assert_eq!(key_expr, "workspace_id");
        assert_eq!(where_clause.as_deref(), Some("((status)::text = 'pending'::text)"));
    }

    #[test]
    fn parse_sqlite_indexdef_extracts_key_expr_and_where_clause() {
        let sql = "CREATE UNIQUE INDEX idx_watches_name_workspace_unique ON watches(workspace_id, name)";
        let (key_expr, where_clause) = parse_sqlite_indexdef(sql);
        assert_eq!(key_expr, "workspace_id, name");
        assert!(where_clause.is_none());

        let sql_partial =
            "CREATE UNIQUE INDEX unique_pending_transfer ON ownership_transfers(workspace_id) WHERE status = 'pending'";
        let (key_expr2, where_clause2) = parse_sqlite_indexdef(sql_partial);
        assert_eq!(key_expr2, "workspace_id");
        assert_eq!(where_clause2.as_deref(), Some("status = 'pending'"));
    }

    // -- diff_indexes ----------------------------------------------------------

    #[test]
    fn index_name_matched_key_signature_mismatch_attributes_each_side_this_is_kyo_295() {
        // The motivating case from the ticket: same index name on both
        // backends, but Postgres's key includes lower(name) (case-insensitive
        // uniqueness) while SQLite's does not (case-sensitive) — a real
        // correctness divergence, not a cosmetic one.
        let pg =
            vec![idx("idx_watches_name_workspace_unique", "watches", true, &["workspace_id", "lower(name)"], None)];
        let sqlite =
            vec![idx("idx_watches_name_workspace_unique", "watches", true, &["workspace_id", "name"], None)];

        let findings = diff_indexes(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        let msg = &findings[0];
        assert!(msg.contains("idx_watches_name_workspace_unique"), "{msg}");
        assert!(msg.contains("`watches`"), "{msg}");
        assert!(msg.contains("Postgres keys `(workspace_id, lower(name))`"), "{msg}");
        assert!(msg.contains("SQLite keys `(workspace_id, name)`"), "{msg}");
    }

    #[test]
    fn index_name_matched_identical_keys_produce_no_finding() {
        let pg = vec![idx("idx_a", "t", true, &["a", "b"], None)];
        let sqlite = vec![idx("idx_a", "t", true, &["a", "b"], None)];

        assert!(diff_indexes(&pg, &sqlite, &empty_allowlist()).is_empty());
    }

    #[test]
    fn index_name_matched_pass_does_not_cross_match_different_tables() {
        // Same index name on two different tables must never be compared
        // against each other — that would attribute one table's index
        // definition to a different table via a coincidental name
        // collision. Each side must instead fall through to the per-table
        // signature-fallback pass (pass 2), where each is reported missing
        // its own unique-index counterpart on the *correct* table.
        let pg = vec![idx("idx_same_name", "table_a", true, &["x"], None)];
        let sqlite = vec![idx("idx_same_name", "table_b", true, &["y"], None)];

        let findings = diff_indexes(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 2, "findings: {findings:?}");
        assert!(
            findings.iter().any(|f| f.contains("table_a") && f.contains("no matching") && f.contains("on SQLite")),
            "{findings:?}"
        );
        assert!(
            findings.iter().any(|f| f.contains("table_b") && f.contains("no matching") && f.contains("on Postgres")),
            "{findings:?}"
        );
        // Must not produce a cross-table "differs" finding conflating the two.
        assert!(!findings.iter().any(|f| f.contains("differs")), "{findings:?}");
    }

    #[test]
    fn index_name_matched_unique_flag_mismatch_is_a_finding() {
        let pg = vec![idx("idx_x", "t", true, &["a"], None)];
        let sqlite = vec![idx("idx_x", "t", false, &["a"], None)];

        let findings = diff_indexes(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("idx_x"));
        assert!(findings[0].contains("unique=true"));
        assert!(findings[0].contains("unique=false"));
    }

    #[test]
    fn index_name_matched_both_non_unique_is_out_of_scope() {
        // Non-unique/non-unique pairs are never compared, even with wildly
        // different keys — that's the module doc's "hundreds spelled
        // differently" carve-out.
        let pg = vec![idx("idx_perf", "t", false, &["a"], None)];
        let sqlite = vec![idx("idx_perf", "t", false, &["b", "c"], None)];

        let findings = diff_indexes(&pg, &sqlite, &empty_allowlist());

        assert!(findings.is_empty(), "non-unique/non-unique pairs are out of scope: {findings:?}");
    }

    #[test]
    fn index_name_matched_partial_predicate_mismatch_is_a_finding() {
        let pg = vec![idx("idx_p", "t", true, &["a"], Some("status = 'active'"))];
        let sqlite = vec![idx("idx_p", "t", true, &["a"], Some("status = 'inactive'"))];

        let findings = diff_indexes(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("partial-predicate differs"), "{}", findings[0]);
    }

    #[test]
    fn index_name_matched_partial_on_only_one_side_is_a_finding() {
        let pg = vec![idx("idx_p", "t", true, &["a"], Some("status = 'active'"))];
        let sqlite = vec![idx("idx_p", "t", true, &["a"], None)];

        let findings = diff_indexes(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("partial on Postgres"), "{}", findings[0]);
    }

    #[test]
    fn unique_index_signature_present_only_on_postgres_is_a_finding() {
        let pg = vec![idx("pg_only_unique", "t", true, &["a"], None)];
        let sqlite: Vec<IndexInfo> = vec![];

        let findings = diff_indexes(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("pg_only_unique"), "{}", findings[0]);
        assert!(findings[0].contains("no matching"), "{}", findings[0]);
        assert!(findings[0].contains("on SQLite"), "{}", findings[0]);
    }

    #[test]
    fn unique_index_signature_present_only_on_sqlite_is_a_finding() {
        let pg: Vec<IndexInfo> = vec![];
        let sqlite = vec![idx("sqlite_only_unique", "t", true, &["a"], None)];

        let findings = diff_indexes(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("sqlite_only_unique"), "{}", findings[0]);
        assert!(findings[0].contains("no matching"), "{}", findings[0]);
        assert!(findings[0].contains("on Postgres"), "{}", findings[0]);
    }

    #[test]
    fn unique_index_signature_matched_by_set_across_differently_named_indexes_produces_no_finding() {
        // Auto-generated names (Postgres constraint-backed vs SQLite
        // sqlite_autoindex_*) never match — this is exactly why the
        // signature fallback exists.
        let pg = vec![idx("platform_user_links_workspace_id_platform_type_platform_use_key", "platform_user_links", true, &["workspace_id", "platform_type", "platform_user_id"], None)];
        let sqlite = vec![idx("sqlite_autoindex_platform_user_links_1", "platform_user_links", true, &["workspace_id", "platform_type", "platform_user_id"], None)];

        assert!(diff_indexes(&pg, &sqlite, &empty_allowlist()).is_empty());
    }

    #[test]
    fn index_waiver_suppresses_only_the_named_index() {
        let pg = vec![idx("idx_a", "t", true, &["x"], None), idx("idx_b", "t", true, &["y"], None)];
        let sqlite =
            vec![idx("idx_a", "t", true, &["different"], None), idx("idx_b", "t", true, &["also_different"], None)];

        let findings = diff_indexes(&pg, &sqlite, &allowlist_with_index(&[("t", "idx_a")]));

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("idx_b"), "{}", findings[0]);
        assert!(!findings[0].contains("idx_a"), "{}", findings[0]);
    }

    #[test]
    fn whole_table_waiver_suppresses_index_findings() {
        let pg = vec![idx("idx_a", "t", true, &["x"], None)];
        let sqlite = vec![idx("idx_a", "t", true, &["y"], None)];

        assert!(diff_indexes(&pg, &sqlite, &allowlist_with(&["t"], &[])).is_empty());
    }

    // -- diff_primary_keys -----------------------------------------------------

    #[test]
    fn primary_key_column_mismatch_attributes_each_side() {
        let pg = pk_map(&[("t", &["a", "b"])]);
        let sqlite = pk_map(&[("t", &["a"])]);

        let findings = diff_primary_keys(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("Postgres columns `(a, b)`"), "{}", findings[0]);
        assert!(findings[0].contains("SQLite columns `(a)`"), "{}", findings[0]);
    }

    #[test]
    fn matching_primary_key_produces_no_finding() {
        let pg = pk_map(&[("t", &["a", "b"])]);
        let sqlite = pk_map(&[("t", &["a", "b"])]);

        assert!(diff_primary_keys(&pg, &sqlite, &empty_allowlist()).is_empty());
    }

    #[test]
    fn primary_key_missing_on_one_side_is_a_finding() {
        let pg = pk_map(&[("t", &["a"])]);
        let sqlite = PrimaryKeys::new();

        let findings = diff_primary_keys(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("no primary key on SQLite"), "{}", findings[0]);
    }

    #[test]
    fn primary_key_mismatch_suppressed_by_whole_table_waiver() {
        let pg = pk_map(&[("t", &["a", "b"])]);
        let sqlite = pk_map(&[("t", &["a"])]);

        assert!(diff_primary_keys(&pg, &sqlite, &allowlist_with(&["t"], &[])).is_empty());
    }

    // -- diff_foreign_keys -------------------------------------------------

    #[test]
    fn foreign_key_target_table_mismatch_attributes_each_side() {
        let pg = vec![fk("t", &["parent_id"], "parents", &["id"], RefAction::Cascade, RefAction::NoAction)];
        let sqlite = vec![fk("t", &["parent_id"], "other_parents", &["id"], RefAction::Cascade, RefAction::NoAction)];

        let findings = diff_foreign_keys(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("Postgres `parents`"), "{}", findings[0]);
        assert!(findings[0].contains("SQLite `other_parents`"), "{}", findings[0]);
    }

    #[test]
    fn foreign_key_on_delete_action_mismatch_attributes_each_side() {
        let pg = vec![fk("t", &["parent_id"], "parents", &["id"], RefAction::Cascade, RefAction::NoAction)];
        let sqlite = vec![fk("t", &["parent_id"], "parents", &["id"], RefAction::SetNull, RefAction::NoAction)];

        let findings = diff_foreign_keys(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("ON DELETE action differs"), "{}", findings[0]);
        assert!(findings[0].contains("Postgres CASCADE"), "{}", findings[0]);
        assert!(findings[0].contains("SQLite SET NULL"), "{}", findings[0]);
    }

    #[test]
    fn foreign_key_on_update_action_mismatch_attributes_each_side() {
        let pg = vec![fk("t", &["parent_id"], "parents", &["id"], RefAction::NoAction, RefAction::Cascade)];
        let sqlite = vec![fk("t", &["parent_id"], "parents", &["id"], RefAction::NoAction, RefAction::Restrict)];

        let findings = diff_foreign_keys(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("ON UPDATE action differs"), "{}", findings[0]);
        assert!(findings[0].contains("Postgres CASCADE"), "{}", findings[0]);
        assert!(findings[0].contains("SQLite RESTRICT"), "{}", findings[0]);
    }

    #[test]
    fn foreign_key_matching_both_sides_produces_no_finding() {
        let pg = vec![fk("t", &["parent_id"], "parents", &["id"], RefAction::Cascade, RefAction::NoAction)];
        let sqlite = vec![fk("t", &["parent_id"], "parents", &["id"], RefAction::Cascade, RefAction::NoAction)];

        assert!(diff_foreign_keys(&pg, &sqlite, &empty_allowlist()).is_empty());
    }

    #[test]
    fn foreign_key_present_only_on_postgres_is_a_finding() {
        let pg = vec![fk("t", &["parent_id"], "parents", &["id"], RefAction::Cascade, RefAction::NoAction)];
        let sqlite: Vec<ForeignKeyInfo> = vec![];

        let findings = diff_foreign_keys(&pg, &sqlite, &empty_allowlist());

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("exists on Postgres but not on SQLite"), "{}", findings[0]);
    }

    #[test]
    fn foreign_key_waiver_suppresses_only_the_named_foreign_key() {
        let pg = vec![
            fk("t", &["a"], "parents", &["id"], RefAction::Cascade, RefAction::NoAction),
            fk("t", &["b"], "others", &["id"], RefAction::Cascade, RefAction::NoAction),
        ];
        let sqlite = vec![
            fk("t", &["a"], "different_parents", &["id"], RefAction::Cascade, RefAction::NoAction),
            fk("t", &["b"], "different_others", &["id"], RefAction::Cascade, RefAction::NoAction),
        ];

        let findings = diff_foreign_keys(&pg, &sqlite, &allowlist_with_foreign_key(&[("t", "a")]));

        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert!(findings[0].contains("(b)"), "{}", findings[0]);
    }

    #[test]
    fn whole_table_waiver_suppresses_foreign_key_findings() {
        let pg = vec![fk("t", &["a"], "parents", &["id"], RefAction::Cascade, RefAction::NoAction)];
        let sqlite: Vec<ForeignKeyInfo> = vec![];

        assert!(diff_foreign_keys(&pg, &sqlite, &allowlist_with(&["t"], &[])).is_empty());
    }
}
