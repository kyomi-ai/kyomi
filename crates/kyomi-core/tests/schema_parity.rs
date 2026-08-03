// SPDX-License-Identifier: AGPL-3.0-or-later

//! Migration schema-parity check (KYO-206).
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
//! resulting schemas into a normalized `{table -> {column -> (type_class,
//! nullable, has_default)}}` map, and fails on any unallowlisted divergence.
//! It deliberately does not parse the `.sql` files itself — that would let
//! this check drift from what `DbPool::connect` (i.e. production) actually
//! runs.
//!
//! Known-legitimate divergences are checked in at
//! `apps/server/schema-parity-allowlist.toml`, each with a justification.
//!
//! ## Hermeticity (KYO-242)
//!
//! `apps/server/tests/contract_*` shares one persistent Postgres container
//! across worktrees; an unmerged branch's migration there can brick every
//! other worktree's contract suite. This test does not touch that shared
//! database. It creates its own scratch Postgres database (dropped at the
//! end of the test) on the same server, and its SQLite side is a fresh
//! `sqlite::memory:` database that's never persisted at all.

use std::collections::{BTreeMap, BTreeSet, HashSet};

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
// Postgres introspection
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
// SQLite introspection
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
    /// Human-readable justification, validated at load time — see
    /// `Allowlist::load`. An allowlist entry without a real reason is how
    /// this check quietly decays into a rubber stamp, so a blank one is
    /// rejected rather than merely discouraged.
    reason: String,
}

struct Allowlist {
    whole_table: HashSet<String>,
    column: HashSet<(String, String)>,
}

impl Allowlist {
    fn load(path: &str) -> Self {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read schema-parity allowlist at {path}: {e}"));
        let parsed: AllowlistFile = toml::from_str(&text)
            .unwrap_or_else(|e| panic!("parse schema-parity allowlist at {path}: {e}"));

        let mut whole_table = HashSet::new();
        let mut column = HashSet::new();
        for e in parsed.entry {
            // Enforce the justification rather than just asking for it. The
            // whole value of this allowlist is that every waiver states why
            // the divergence is deliberate; an entry added with an empty
            // reason to make a failing build green is exactly the outcome
            // this check exists to prevent.
            assert!(
                !e.reason.trim().is_empty(),
                "schema-parity allowlist entry for `{}{}` has an empty `reason`. \
                 Every waiver must justify why the divergence is deliberate — \
                 see the header comment in {path}.",
                e.table,
                e.column
                    .as_deref()
                    .map_or_else(String::new, |c| format!(".{c}")),
            );
            match e.column {
                None => {
                    whole_table.insert(e.table);
                }
                Some(c) => {
                    column.insert((e.table, c));
                }
            }
        }
        Self { whole_table, column }
    }

    fn waives_table(&self, table: &str) -> bool {
        self.whole_table.contains(table)
    }

    fn waives_column(&self, table: &str, column: &str) -> bool {
        self.whole_table.contains(table)
            || self.column.contains(&(table.to_string(), column.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Diff
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
    // against the scratch database, then introspect it.
    let pg_schema = {
        let pool = kyomi_core::db::DbPool::connect(&scratch_url)
            .await
            .expect("run Postgres migration chain against scratch database");
        let schema = introspect_postgres(pool.pg_pool()).await;
        pool.pg_pool().close().await;
        schema
    };

    // Run the real embedded SQLite migration chain (crates/kyomi-core/src/db.rs:45)
    // against a fresh in-memory database, then introspect it.
    let sqlite_schema = {
        let pool = kyomi_core::db::DbPool::connect("sqlite::memory:")
            .await
            .expect("run SQLite migration chain against in-memory database");
        match &pool {
            kyomi_core::db::DbPool::Sqlite(sq) => introspect_sqlite(sq).await,
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

    let findings = diff_schemas(&pg_schema, &sqlite_schema, &allow);

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
// Unit tests for diff_schemas / Allowlist / classify_* — DB-free, fast.
//
// The single `#[tokio::test]` above proves the check works end-to-end
// against real migrations, but it can only ever exercise whatever
// divergence shape happens to exist between the two chains *today* (right
// now, that's purely presence/absence — collections.doc_type-shaped). The
// type-class, nullability, and default-presence branches of diff_schemas
// have no live example to exercise them, so they need their own coverage
// against hand-built DbSchema values — otherwise a swapped `p`/`s`, an
// inverted `!=`, or a backend label the wrong way round in one of those
// branches would go unnoticed until a real divergence of that exact shape
// appeared, at which point the check would either stay silent or report it
// backwards.
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
        Allowlist { whole_table: HashSet::new(), column: HashSet::new() }
    }

    fn allowlist_with(whole_table: &[&str], columns: &[(&str, &str)]) -> Allowlist {
        Allowlist {
            whole_table: whole_table.iter().map(|s| s.to_string()).collect(),
            column: columns.iter().map(|(t, c)| (t.to_string(), c.to_string())).collect(),
        }
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
}
