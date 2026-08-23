// SPDX-License-Identifier: AGPL-3.0-or-later

//! Migration tests for KYO-460 (`datasource_configs.connection_config`
//! scalar retyping).
//!
//! Before KYO-428, the Leptos datasource modal's create/edit server fns
//! round-tripped their request body through `serde_qs`, which has no way to
//! represent a JSON number or boolean — every scalar landed in
//! `connection_config` as a JSON *string*. The drivers (in the separate
//! `kyomi-connect` repo) read these fields with `.as_u64()` / `.as_bool()`,
//! both of which return `None` (not an error) for a JSON string, so a
//! corrupted row silently falls back to a hardcoded default forever. KYO-428
//! fixed the write path; this migration is the one-shot repair for rows
//! written before that fix.
//!
//! This file runs the *actual* migration files shipped in
//! `apps/server/migrations/20260823000000_retype_connection_config_scalars.sql`
//! and
//! `apps/server/migrations-sqlite/00035_retype_connection_config_scalars.sql`
//! — via `include_str!`, not a reimplementation — against live databases,
//! following the same pattern as
//! `contract_push.rs`'s `postgres_purge_migration_rejects_authority_smuggling_bypasses_on_live_db`.
//! That is what makes these tests load-bearing: a hand-rewritten copy of the
//! SQL could quietly drift from what actually ships and still pass.
//!
//! Both dialects are covered because they diverge on real, non-obvious
//! semantics (see each test's comments): Postgres needs a `CASE`-guarded
//! cast to avoid an `AND`-reordering hazard that SQLite's non-throwing
//! `CAST` doesn't have, and SQLite's `json_type()` reports a genuine JSON
//! boolean as the literal token `'true'`/`'false'` rather than `'boolean'`
//! — a quirk KYO-451 also ran into.

use serde_json::{json, Value};

use kyomi_test_harness::{cleanup_test_user, setup_auth_context};

const POSTGRES_RETYPE_MIGRATION_SQL: &str =
    include_str!("../migrations/20260823000000_retype_connection_config_scalars.sql");
const SQLITE_RETYPE_MIGRATION_SQL: &str =
    include_str!("../migrations-sqlite/00035_retype_connection_config_scalars.sql");

/// One seeded `datasource_configs` row and what it's meant to prove.
struct SeedRow {
    id: &'static str,
    connection_config: Value,
}

fn seed_rows() -> Vec<SeedRow> {
    vec![
        // The corrupted shape the KYO-428-era modal actually wrote: every
        // numeric/boolean leaf flattened to a string, sitting alongside
        // genuine string fields that must survive untouched.
        SeedRow {
            id: "kyo460-corrupted",
            connection_config: json!({
                "host": "dbhost.example.com",
                "database": "mydb",
                "port": "5434",
                "ssh_port": "2222",
                "secure": "true",
                "encrypt": "false",
                "trust_server_certificate": "true",
                "ssh_enabled": "true",
                "shared_credentials": "true",
            }),
        },
        // Already correctly typed (post-KYO-428 write, or never corrupted).
        // Must come out byte-identical — proves the migration doesn't touch
        // rows it has no business touching.
        SeedRow {
            id: "kyo460-already-correct",
            connection_config: json!({
                "host": "dbhost.example.com",
                "port": 5432,
                "secure": true,
                "shared_credentials": true,
            }),
        },
        // No `port` (nor `shared_credentials`) key at all — the modal only
        // ever inserts `port` when it parses one, and only ever inserts
        // `shared_credentials` when the checkbox is true, so a missing key
        // was never corrupted for either. Must stay absent, not become
        // `null` or get invented.
        SeedRow {
            id: "kyo460-absent-port",
            connection_config: json!({ "host": "dbhost.example.com" }),
        },
        // Strings that fail the digit/exact-true-false test. Must be left
        // exactly as stored, not mangled or dropped.
        SeedRow {
            id: "kyo460-unconvertible",
            connection_config: json!({
                "port": "notanumber",
                "secure": "maybe",
                "shared_credentials": "sorta",
            }),
        },
        // Digit strings that are out of the valid port range (0 is below
        // the minimum, 70000 is above the u16 maximum). Left as strings —
        // NOT clamped, NOT converted to a wrong number.
        SeedRow {
            id: "kyo460-out-of-range",
            connection_config: json!({
                "port": "0",
                "ssh_port": "70000",
            }),
        },
        // Exact inclusive boundary values (1 and 65535). Must convert.
        SeedRow {
            id: "kyo460-boundary",
            connection_config: json!({
                "port": "65535",
                "ssh_port": "1",
            }),
        },
    ]
}

fn assert_common_cases(get: impl Fn(&str) -> Value, pre_correct_text: &str, post_correct_text: &str) {
    // kyo460-corrupted: every scalar leaf converts to the right JSON type
    // AND the right value; sibling string fields are untouched.
    let corrupted = get("kyo460-corrupted");
    assert_eq!(corrupted["port"], json!(5434), "port must become the number 5434, not stay a string");
    assert_eq!(corrupted["ssh_port"], json!(2222));
    assert_eq!(corrupted["secure"], json!(true));
    assert_eq!(corrupted["encrypt"], json!(false));
    assert_eq!(corrupted["trust_server_certificate"], json!(true));
    assert_eq!(corrupted["ssh_enabled"], json!(true));
    assert_eq!(corrupted["shared_credentials"], json!(true));
    assert_eq!(
        corrupted["host"],
        json!("dbhost.example.com"),
        "sibling string field must survive unchanged"
    );
    assert_eq!(corrupted["database"], json!("mydb"));

    // kyo460-already-correct: byte-identical text before and after — proves
    // the migration's WHERE guard excludes the row rather than issuing a
    // no-op write that happens to produce the same value.
    assert_eq!(
        pre_correct_text, post_correct_text,
        "an already-correctly-typed row must not be rewritten at all"
    );

    // kyo460-absent-port: still absent, not null, not invented. Also
    // covers `shared_credentials`, which was never present on this row
    // either — an absent key must stay absent, never gain a value.
    let absent = get("kyo460-absent-port");
    assert!(
        absent.get("port").is_none(),
        "a row that never had `port` must not gain one: {absent:?}"
    );
    assert!(
        absent.get("shared_credentials").is_none(),
        "a row that never had `shared_credentials` must not gain one: {absent:?}"
    );

    // kyo460-unconvertible: left exactly as stored.
    let unconvertible = get("kyo460-unconvertible");
    assert_eq!(unconvertible["port"], json!("notanumber"));
    assert_eq!(unconvertible["secure"], json!("maybe"));
    assert_eq!(unconvertible["shared_credentials"], json!("sorta"));

    // kyo460-out-of-range: still strings — NOT clamped into range.
    let out_of_range = get("kyo460-out-of-range");
    assert_eq!(out_of_range["port"], json!("0"));
    assert_eq!(out_of_range["ssh_port"], json!("70000"));

    // kyo460-boundary: inclusive endpoints convert correctly.
    let boundary = get("kyo460-boundary");
    assert_eq!(boundary["port"], json!(65535));
    assert_eq!(boundary["ssh_port"], json!(1));
}

// ===========================================================================
// Postgres
// ===========================================================================
//
// The risky part of the Postgres migration is the `CASE`-guarded cast (see
// the migration file's header comment for the full "Postgres doesn't
// guarantee AND-reordering" rationale). This test's `kyo460-unconvertible`
// and `kyo460-out-of-range` rows are exactly the adversarial inputs that
// would blow up an unguarded `WHERE ... AND col::int BETWEEN ...` if the
// planner ever evaluated the cast before the type/shape guard — so a green
// run here is also proof that hazard doesn't fire in practice, not just
// that the happy path works.

#[tokio::test]
async fn postgres_retype_migration_fixes_corrupted_scalars_on_live_db() {
    let ctx = setup_auth_context("Datasource Retype Test User", "ds-retype", "pg").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: postgres_retype_migration_fixes_corrupted_scalars_on_live_db — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let kyomi_core::db::DbPool::Postgres(pg) = &ctx.db else {
        eprintln!(
            "SKIP: postgres_retype_migration_fixes_corrupted_scalars_on_live_db — requires a live Postgres pool (got SQLite)"
        );
        cleanup_test_user(&ctx.db, "ds-retype-test-pg@contract-test.local").await;
        return;
    };

    let mut tx = pg.begin().await.expect("begin transaction");

    for row in seed_rows() {
        sqlx::query(
            "INSERT INTO datasource_configs \
             (id, workspace_id, name, datasource_type, connection_config, slug) \
             VALUES ($1, $2, $3, 'clickhouse', $4, $5)",
        )
        .bind(row.id)
        .bind(&ctx.workspace_id)
        .bind(row.id)
        .bind(&row.connection_config)
        .bind(row.id)
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|e| panic!("seed row {}: {e}", row.id));
    }

    let pre_correct_text: String = sqlx::query_scalar(
        "SELECT connection_config::text FROM datasource_configs WHERE id = $1",
    )
    .bind("kyo460-already-correct")
    .fetch_one(&mut *tx)
    .await
    .expect("fetch pre-migration text for the already-correct row");

    // Run the exact statements shipped in the migration file — not a
    // hand-rewritten approximation — against real Postgres.
    sqlx::raw_sql(POSTGRES_RETYPE_MIGRATION_SQL)
        .execute(&mut *tx)
        .await
        .expect("run retype migration on live Postgres");

    let post_correct_text: String = sqlx::query_scalar(
        "SELECT connection_config::text FROM datasource_configs WHERE id = $1",
    )
    .bind("kyo460-already-correct")
    .fetch_one(&mut *tx)
    .await
    .expect("fetch post-migration text for the already-correct row");

    let mut fetched: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    for row in seed_rows() {
        let cfg: Value = sqlx::query_scalar(
            "SELECT connection_config FROM datasource_configs WHERE id = $1",
        )
        .bind(row.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap_or_else(|e| panic!("fetch row {} after migration: {e}", row.id));
        fetched.insert(row.id.to_string(), cfg);
    }

    assert_common_cases(
        |id| fetched.get(id).cloned().expect("row was seeded"),
        &pre_correct_text,
        &post_correct_text,
    );

    // Never commit — leaves the shared dev database untouched.
    tx.rollback().await.expect("rollback transaction");

    cleanup_test_user(&ctx.db, "ds-retype-test-pg@contract-test.local").await;
}

// ===========================================================================
// SQLite
// ===========================================================================
//
// Runs against a fresh in-memory database migrated through the real
// `kyomi_core::DbPool::connect` entry point — the same one self-hosted
// production uses — so this also proves 00035 applies cleanly as part of
// the actual chain (a syntax error or a bad statement would fail `connect`
// outright, before this test gets to assert anything). Corrupted rows are
// then seeded directly (bypassing the app layer, which no longer produces
// this shape after KYO-428) and the migration's own statements are run a
// second time against them to prove the repair.
//
// `PRAGMA foreign_keys = ON` is set inside `DbPool::connect`, matching
// production, so a minimal `users` + `workspaces` row is seeded first to
// satisfy `datasource_configs.workspace_id`'s FK.

#[tokio::test]
async fn sqlite_retype_migration_fixes_corrupted_scalars_on_live_db() {
    let db = kyomi_core::DbPool::connect("sqlite::memory:").await.expect(
        "sqlite migration chain should apply cleanly, including \
         00035_retype_connection_config_scalars.sql",
    );
    let kyomi_core::db::DbPool::Sqlite(sq) = &db else {
        unreachable!("DbPool::connect(\"sqlite::memory:\") always returns the Sqlite variant")
    };

    sqlx::query(
        "INSERT INTO users (user_id, email) VALUES ('kyo460-sqlite-user', 'kyo460-sqlite@contract-test.local')",
    )
    .execute(sq)
    .await
    .expect("seed user");
    sqlx::query(
        "INSERT INTO workspaces (workspace_id, owner_user_id) VALUES ('kyo460-sqlite-ws', 'kyo460-sqlite-user')",
    )
    .execute(sq)
    .await
    .expect("seed workspace");

    for row in seed_rows() {
        sqlx::query(
            "INSERT INTO datasource_configs \
             (id, workspace_id, name, datasource_type, connection_config, slug) \
             VALUES (?1, 'kyo460-sqlite-ws', ?2, 'clickhouse', ?3, ?4)",
        )
        .bind(row.id)
        .bind(row.id)
        .bind(row.connection_config.to_string())
        .bind(row.id)
        .execute(sq)
        .await
        .unwrap_or_else(|e| panic!("seed row {}: {e}", row.id));
    }

    let pre_correct_text: String = sqlx::query_scalar(
        "SELECT connection_config FROM datasource_configs WHERE id = ?1",
    )
    .bind("kyo460-already-correct")
    .fetch_one(sq)
    .await
    .expect("fetch pre-migration text for the already-correct row");

    sqlx::raw_sql(SQLITE_RETYPE_MIGRATION_SQL)
        .execute(sq)
        .await
        .expect("run retype migration on live sqlite");

    let post_correct_text: String = sqlx::query_scalar(
        "SELECT connection_config FROM datasource_configs WHERE id = ?1",
    )
    .bind("kyo460-already-correct")
    .fetch_one(sq)
    .await
    .expect("fetch post-migration text for the already-correct row");

    let mut fetched: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    for row in seed_rows() {
        let text: String = sqlx::query_scalar(
            "SELECT connection_config FROM datasource_configs WHERE id = ?1",
        )
        .bind(row.id)
        .fetch_one(sq)
        .await
        .unwrap_or_else(|e| panic!("fetch row {} after migration: {e}", row.id));
        let cfg: Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("row {} connection_config is not valid JSON: {e}", row.id));
        fetched.insert(row.id.to_string(), cfg);
    }

    assert_common_cases(
        |id| fetched.get(id).cloned().expect("row was seeded"),
        &pre_correct_text,
        &post_correct_text,
    );

    // In-memory database — nothing to clean up, it disappears with `db`.
}
