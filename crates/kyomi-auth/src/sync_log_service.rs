// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sync log service — core persistence layer for the real-time sync protocol.
//!
//! This module provides the server-side CRUD operations for the `sync_log`
//! table. It is used by mutation instrumentation (Phase 2) to record every
//! entity change, and by the WebSocket sync handlers (Phase 3) to stream
//! changes to clients.
//!
//! Key design decisions:
//! - Free-function pattern (`&DbPool` first arg) matching all other services
//! - `sync_id` is an auto-incrementing integer — Postgres BIGSERIAL, SQLite AUTOINCREMENT
//! - Postgres uses `RETURNING sync_id` to get the assigned ID; SQLite uses `last_insert_rowid()`
//! - `data` is stored as JSONB on Postgres and TEXT on SQLite

use kyomi_core::sql_compat;
use kyomi_core::{db_execute, db_fetch_all, db_fetch_scalar, DbPool};
use kyomi_types::sync::{SyncAction, SyncActionType};

// ─── Row type ────────────────────────────────────────────────────────────────

/// Internal row type for deserialising `sync_log` query results.
///
/// `data` is TEXT-compatible for both Postgres (JSONB reads as text via sqlx)
/// and SQLite (TEXT column).
#[derive(sqlx::FromRow)]
struct SyncLogRow {
    sync_id: i64,
    entity_type: String,
    entity_id: String,
    workspace_id: String,
    action: String,
    data: Option<String>,
    created_at: String,
}

impl SyncLogRow {
    fn into_sync_action(self) -> kyomi_core::Result<SyncAction> {
        let action = parse_action_type(&self.action)?;
        let data = self
            .data
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to parse sync_log data JSON: {e}"))
            })?;

        // Normalise the stored timestamp to RFC 3339.
        // Postgres stores TIMESTAMPTZ which sqlx decodes into a formatted string.
        // SQLite stores TEXT in `datetime('now')` format (ISO-8601 without timezone).
        // We append 'Z' for SQLite timestamps that lack a timezone suffix.
        let timestamp = normalise_timestamp(&self.created_at);

        Ok(SyncAction {
            sync_id: self.sync_id,
            entity_type: self.entity_type,
            entity_id: self.entity_id,
            workspace_id: self.workspace_id,
            action,
            data,
            timestamp,
        })
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn action_type_to_str(action: &SyncActionType) -> &'static str {
    match action {
        SyncActionType::Insert => "insert",
        SyncActionType::Update => "update",
        SyncActionType::Delete => "delete",
    }
}

fn parse_action_type(s: &str) -> kyomi_core::Result<SyncActionType> {
    match s {
        "insert" => Ok(SyncActionType::Insert),
        "update" => Ok(SyncActionType::Update),
        "delete" => Ok(SyncActionType::Delete),
        other => Err(kyomi_core::Error::Internal(format!(
            "unknown sync action type: {other}"
        ))),
    }
}

/// Ensure a timestamp string has a UTC timezone marker.
///
/// Postgres TIMESTAMPTZ comes back as e.g. `"2026-04-26T12:34:56.789Z"`.
/// SQLite `datetime('now')` comes back as `"2026-04-26 12:34:56"` (no `Z`).
fn normalise_timestamp(ts: &str) -> String {
    let has_tz = ts.ends_with('Z')
        || ts.contains('+')
        || (ts.contains('-') && ts.len() > 19);
    if has_tz {
        ts.to_string()
    } else {
        format!("{}Z", ts.replace(' ', "T"))
    }
}

// ─── SyncEntryParams ─────────────────────────────────────────────────────────

/// Parameters for [`write_sync_entry`].
///
/// Groups all per-entry fields into a single struct to keep call sites readable
/// and avoid the `clippy::too_many_arguments` lint.
pub struct SyncEntryParams<'a> {
    pub entity_type: &'a str,
    pub entity_id: &'a str,
    pub workspace_id: &'a str,
    pub action: SyncActionType,
    pub data: Option<serde_json::Value>,
    pub owner_user_id: Option<&'a str>,
    pub is_workspace_visible: bool,
}

// ─── write_sync_entry ────────────────────────────────────────────────────────

/// Insert a row into `sync_log` and return the assigned `sync_id`.
///
/// Uses `RETURNING sync_id` on Postgres and `SELECT last_insert_rowid()` on
/// SQLite because the ID is assigned by the database (BIGSERIAL / AUTOINCREMENT).
pub async fn write_sync_entry(
    db: &DbPool,
    params: SyncEntryParams<'_>,
) -> kyomi_core::Result<i64> {
    let is_pg = db.is_postgres();
    let now_expr = sql_compat::now(is_pg);
    let action_str = action_type_to_str(&params.action);
    let visible_literal = if params.is_workspace_visible {
        sql_compat::bool_true(is_pg)
    } else {
        sql_compat::bool_false(is_pg)
    };

    // Serialise the data payload.
    // Postgres: stored as JSONB — pass the JSON string with ::jsonb cast.
    // SQLite:   stored as TEXT — pass the JSON string directly.
    let data_str: Option<String> = params
        .data
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to serialise sync entry data: {e}"))
        })?;

    let sync_id: i64 = if is_pg {
        // Postgres: use RETURNING to get the assigned BIGSERIAL id.
        let json_cast = sql_compat::cast_to_json(is_pg, "$5");
        let sql = format!(
            r#"
            INSERT INTO sync_log (entity_type, entity_id, workspace_id, action, data,
                                   owner_user_id, is_workspace_visible, created_at)
            VALUES ($1, $2, $3, $4, {json_cast}, $6, {visible_literal}, {now_expr})
            RETURNING sync_id
            "#
        );
        db_fetch_scalar!(db, i64, &sql, params.entity_type, params.entity_id, params.workspace_id, action_str, data_str, params.owner_user_id)
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to write sync entry: {e}"))
            })?
    } else {
        // SQLite: INSERT then query last_insert_rowid().
        let sql = format!(
            r#"
            INSERT INTO sync_log (entity_type, entity_id, workspace_id, action, data,
                                   owner_user_id, is_workspace_visible, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, {visible_literal}, {now_expr})
            "#
        );
        db_execute!(db, &sql, params.entity_type, params.entity_id, params.workspace_id, action_str, data_str, params.owner_user_id)
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to write sync entry: {e}"))
            })?;

        db_fetch_scalar!(db, i64, "SELECT last_insert_rowid()").map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "failed to get last_insert_rowid after sync entry insert: {e}"
            ))
        })?
    };

    tracing::debug!(
        sync_id,
        entity_type = params.entity_type,
        entity_id = params.entity_id,
        workspace_id = params.workspace_id,
        action = action_str,
        "Wrote sync log entry"
    );

    Ok(sync_id)
}

// ─── get_entries_since ───────────────────────────────────────────────────────

/// Fetch all sync entries with `sync_id > since_sync_id` for a workspace,
/// filtered by visibility: only workspace-visible entries or entries owned by
/// the requesting user are returned.
///
/// Results are ordered by `sync_id ASC` (oldest first) and capped by `limit`.
pub async fn get_entries_since(
    db: &DbPool,
    workspace_id: &str,
    since_sync_id: i64,
    user_id: &str,
    limit: i64,
) -> kyomi_core::Result<Vec<SyncAction>> {
    // On Postgres, JSONB columns are decoded as String by sqlx when the target
    // field type is `String`.  On SQLite the column is already TEXT.
    let is_pg = db.is_postgres();
    let visible_literal = sql_compat::bool_true(is_pg);
    let sql = format!(
        r#"
        SELECT sync_id, entity_type, entity_id, workspace_id, action,
               CAST(data AS TEXT) AS data,
               CAST(created_at AS TEXT) AS created_at
        FROM sync_log
        WHERE workspace_id = $1 AND sync_id > $2
          AND (is_workspace_visible = {visible_literal} OR owner_user_id = $3)
        ORDER BY sync_id ASC
        LIMIT $4
        "#
    );
    let rows: Vec<SyncLogRow> = db_fetch_all!(
        db,
        SyncLogRow,
        &sql,
        workspace_id,
        since_sync_id,
        user_id,
        limit
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to get sync entries: {e}")))?;

    rows.into_iter()
        .map(SyncLogRow::into_sync_action)
        .collect()
}

// ─── get_latest_sync_id ──────────────────────────────────────────────────────

/// Get the highest `sync_id` for a workspace, or `0` if no entries exist.
pub async fn get_latest_sync_id(
    db: &DbPool,
    workspace_id: &str,
) -> kyomi_core::Result<i64> {
    // MAX() on an empty table returns a single NULL row — fetch_one with
    // Option<i64> handles this correctly.
    let max: Option<i64> = db_fetch_scalar!(
        db,
        Option<i64>,
        "SELECT MAX(sync_id) FROM sync_log WHERE workspace_id = $1",
        workspace_id
    )
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to get latest sync_id: {e}"))
    })?;

    Ok(max.unwrap_or(0))
}

// ─── is_sync_id_available ────────────────────────────────────────────────────

/// Check whether a specific `sync_id` still exists in `sync_log` for a
/// workspace (i.e. it has not been pruned).
pub async fn is_sync_id_available(
    db: &DbPool,
    workspace_id: &str,
    sync_id: i64,
) -> kyomi_core::Result<bool> {
    let count: i64 = db_fetch_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM sync_log WHERE workspace_id = $1 AND sync_id = $2",
        workspace_id,
        sync_id
    )
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to check sync_id availability: {e}"))
    })?;

    Ok(count > 0)
}

// ─── prune_old_entries ───────────────────────────────────────────────────────

/// Delete `sync_log` entries older than `retention_days` days across all
/// workspaces. This is a global pruning operation, not workspace-scoped.
///
/// Returns the number of rows deleted.
pub async fn prune_old_entries(
    db: &DbPool,
    retention_days: i64,
) -> kyomi_core::Result<u64> {
    let is_pg = db.is_postgres();
    let age_filter = sql_compat::ago_days(is_pg, "created_at", "$1");
    let sql = format!("DELETE FROM sync_log WHERE {age_filter}");

    let result = db_execute!(db, &sql, retention_days)
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to prune sync log entries: {e}"))
        })?;

    let deleted = result.rows_affected();
    tracing::info!(deleted, retention_days, "Pruned old sync log entries");

    Ok(deleted)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_type_to_str_roundtrip() {
        for (action, expected) in [
            (SyncActionType::Insert, "insert"),
            (SyncActionType::Update, "update"),
            (SyncActionType::Delete, "delete"),
        ] {
            assert_eq!(action_type_to_str(&action), expected);
            let parsed = parse_action_type(expected).expect("should parse");
            assert_eq!(action_type_to_str(&parsed), expected);
        }
    }

    #[test]
    fn test_parse_action_type_unknown() {
        let err = parse_action_type("upsert").unwrap_err();
        assert!(err.to_string().contains("unknown sync action type"));
    }

    #[test]
    fn test_normalise_timestamp_postgres_utc() {
        // Postgres-style timestamp already has Z — leave unchanged.
        let ts = "2026-04-26T12:34:56.789Z";
        assert_eq!(normalise_timestamp(ts), ts);
    }

    #[test]
    fn test_normalise_timestamp_sqlite_space_separator() {
        // SQLite datetime('now') produces "2026-04-26 12:34:56".
        let ts = "2026-04-26 12:34:56";
        assert_eq!(normalise_timestamp(ts), "2026-04-26T12:34:56Z");
    }

    #[test]
    fn test_normalise_timestamp_already_has_plus_offset() {
        let ts = "2026-04-26T12:34:56+00:00";
        assert_eq!(normalise_timestamp(ts), ts);
    }

    #[test]
    fn test_normalise_timestamp_negative_offset() {
        let ts = "2026-04-26T07:34:56-05:00";
        assert_eq!(normalise_timestamp(ts), ts);
    }

    // ─── Integration tests (async, in-memory SQLite) ─────────────────────────

    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_pool() -> DbPool {
        let _ = kyomi_core::constants::load_with_fallback();

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&pool)
            .await
            .expect("enable foreign keys");

        sqlx::migrate!("../../apps/server/migrations-sqlite")
            .run(&pool)
            .await
            .expect("run sqlite migrations");

        DbPool::Sqlite(pool)
    }

    fn sqlite_pool(db: &DbPool) -> &sqlx::SqlitePool {
        match db {
            DbPool::Sqlite(sq) => sq,
            _ => panic!("test requires sqlite pool"),
        }
    }

    async fn seed_user(sq: &sqlx::SqlitePool, user_id: &str, email: &str) {
        sqlx::query("INSERT INTO users (user_id, email) VALUES ($1, $2)")
            .bind(user_id)
            .bind(email)
            .execute(sq)
            .await
            .expect("insert user");
    }

    async fn seed_workspace(sq: &sqlx::SqlitePool, workspace_id: &str, owner_user_id: &str) {
        sqlx::query(
            "INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ($1, $2, $3)",
        )
        .bind(workspace_id)
        .bind(format!("Workspace {workspace_id}"))
        .bind(owner_user_id)
        .execute(sq)
        .await
        .expect("insert workspace");
    }

    async fn seed_workspace_member(
        sq: &sqlx::SqlitePool,
        workspace_id: &str,
        user_id: &str,
        role: &str,
    ) {
        sqlx::query(
            "INSERT INTO workspace_users (workspace_id, user_id, role) VALUES ($1, $2, $3)",
        )
        .bind(workspace_id)
        .bind(user_id)
        .bind(role)
        .execute(sq)
        .await
        .expect("insert workspace member");
    }

    async fn seed_dashboard(
        sq: &sqlx::SqlitePool,
        dashboard_id: &str,
        user_id: &str,
        workspace_id: &str,
        doc_type: &str,
    ) {
        sqlx::query(
            "INSERT INTO dashboards (dashboard_id, user_id, workspace_id, title, doc_type) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(dashboard_id)
        .bind(user_id)
        .bind(workspace_id)
        .bind(format!("Dashboard {dashboard_id}"))
        .bind(doc_type)
        .execute(sq)
        .await
        .expect("insert dashboard");
    }

    async fn seed_chat_session(
        sq: &sqlx::SqlitePool,
        session_id: &str,
        user_id: &str,
        workspace_id: &str,
    ) {
        sqlx::query(
            "INSERT INTO chat_sessions (session_id, user_id, workspace_id) VALUES ($1, $2, $3)",
        )
        .bind(session_id)
        .bind(user_id)
        .bind(workspace_id)
        .execute(sq)
        .await
        .expect("insert chat session");
    }

    async fn seed_watch(
        sq: &sqlx::SqlitePool,
        watch_id: &str,
        workspace_id: &str,
        created_by: &str,
    ) {
        sqlx::query(
            "INSERT INTO watches (watch_id, workspace_id, created_by, name, prompt, schedule) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(watch_id)
        .bind(workspace_id)
        .bind(created_by)
        .bind(format!("Watch {watch_id}"))
        .bind("test prompt")
        .bind("daily")
        .execute(sq)
        .await
        .expect("insert watch");
    }

    async fn seed_collection(
        sq: &sqlx::SqlitePool,
        collection_id: &str,
        workspace_id: &str,
        created_by: &str,
        is_public: bool,
    ) {
        sqlx::query(
            "INSERT INTO collections (id, workspace_id, name, created_by, is_public) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(collection_id)
        .bind(workspace_id)
        .bind(format!("Collection {collection_id}"))
        .bind(created_by)
        .bind(if is_public { 1 } else { 0 })
        .execute(sq)
        .await
        .expect("insert collection");
    }

    async fn seed_collection_dashboard(
        sq: &sqlx::SqlitePool,
        collection_id: &str,
        dashboard_id: &str,
    ) {
        sqlx::query(
            "INSERT INTO collection_dashboards (collection_id, dashboard_id) VALUES ($1, $2)",
        )
        .bind(collection_id)
        .bind(dashboard_id)
        .execute(sq)
        .await
        .expect("insert collection dashboard");
    }

    #[tokio::test]
    async fn test_get_entries_since_returns_only_visible_and_owned_entries() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);

        seed_user(sq, "user-a", "a@test.local").await;
        seed_user(sq, "user-b", "b@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;
        seed_workspace_member(sq, "ws-1", "user-b", "user").await;

        // Seed backing entities for sync entry references.
        seed_dashboard(sq, "dash-priv-a", "user-a", "ws-1", "dashboard").await;
        seed_dashboard(sq, "dash-priv-b", "user-a", "ws-1", "dashboard").await;
        seed_chat_session(sq, "chat-priv-a", "user-a", "ws-1").await;
        seed_dashboard(sq, "dash-pub", "user-a", "ws-1", "dashboard").await;
        seed_collection(sq, "col-pub", "ws-1", "user-a", true).await;
        seed_collection_dashboard(sq, "col-pub", "dash-pub").await;
        seed_watch(sq, "watch-1", "ws-1", "user-a").await;
        seed_dashboard(sq, "dash-priv-userb", "user-b", "ws-1", "dashboard").await;

        // 1. Private dashboard owned by user-a, not visible
        let sid1 = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "dashboard",
                entity_id: "dash-priv-a",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: None,
                owner_user_id: Some("user-a"),
                is_workspace_visible: false,
            },
        )
        .await
        .unwrap();

        // 2. Private dashboard owned by user-a, not visible
        let sid2 = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "dashboard",
                entity_id: "dash-priv-b",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: None,
                owner_user_id: Some("user-a"),
                is_workspace_visible: false,
            },
        )
        .await
        .unwrap();

        // 3. Private chat owned by user-a, not visible
        let sid3 = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "chat_session",
                entity_id: "chat-priv-a",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: None,
                owner_user_id: Some("user-a"),
                is_workspace_visible: false,
            },
        )
        .await
        .unwrap();

        // 4. Public-collection dashboard, visible
        let sid4 = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "dashboard",
                entity_id: "dash-pub",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: None,
                owner_user_id: Some("user-a"),
                is_workspace_visible: true,
            },
        )
        .await
        .unwrap();

        // 5. Workspace watch, no owner, visible
        let sid5 = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "watch",
                entity_id: "watch-1",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: None,
                owner_user_id: None,
                is_workspace_visible: true,
            },
        )
        .await
        .unwrap();

        // 6. Private dashboard owned by user-b, not visible
        let sid6 = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "dashboard",
                entity_id: "dash-priv-userb",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: None,
                owner_user_id: Some("user-b"),
                is_workspace_visible: false,
            },
        )
        .await
        .unwrap();

        // user-b should see: public-collection dashboard (sid4), watch (sid5),
        // and their own private dashboard (sid6).
        let entries = get_entries_since(&db, "ws-1", 0, "user-b", 100)
            .await
            .unwrap();

        let sync_ids: Vec<i64> = entries.iter().map(|e| e.sync_id).collect();
        assert_eq!(sync_ids, vec![sid4, sid5, sid6]);
    }

    #[tokio::test]
    async fn test_get_entries_since_respects_since_sync_id_cursor() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);

        seed_user(sq, "user-a", "a@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;
        seed_watch(sq, "watch-1", "ws-1", "user-a").await;
        seed_watch(sq, "watch-2", "ws-1", "user-a").await;
        seed_watch(sq, "watch-3", "ws-1", "user-a").await;

        let _sid1 = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "watch",
                entity_id: "watch-1",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: None,
                owner_user_id: None,
                is_workspace_visible: true,
            },
        )
        .await
        .unwrap();

        let sid2 = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "watch",
                entity_id: "watch-2",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: None,
                owner_user_id: None,
                is_workspace_visible: true,
            },
        )
        .await
        .unwrap();

        let sid3 = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "watch",
                entity_id: "watch-3",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: None,
                owner_user_id: None,
                is_workspace_visible: true,
            },
        )
        .await
        .unwrap();

        // Cursor at sid2 — should return only entry with sync_id > sid2.
        let entries = get_entries_since(&db, "ws-1", sid2, "user-a", 100)
            .await
            .unwrap();

        let sync_ids: Vec<i64> = entries.iter().map(|e| e.sync_id).collect();
        assert_eq!(sync_ids, vec![sid3]);
    }

    #[tokio::test]
    async fn test_get_entries_since_empty_for_different_workspace() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);

        seed_user(sq, "user-a", "a@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;
        seed_watch(sq, "watch-1", "ws-1", "user-a").await;

        write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "watch",
                entity_id: "watch-1",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: None,
                owner_user_id: None,
                is_workspace_visible: true,
            },
        )
        .await
        .unwrap();

        // Querying a different workspace should return empty.
        let entries = get_entries_since(&db, "ws-2", 0, "user-a", 100)
            .await
            .unwrap();

        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn test_visibility_transition_add_to_public_collection() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);

        seed_user(sq, "user-a", "a@test.local").await;
        seed_user(sq, "user-b", "b@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;
        seed_workspace_member(sq, "ws-1", "user-b", "user").await;
        seed_dashboard(sq, "dash-1", "user-a", "ws-1", "dashboard").await;
        seed_collection(sq, "col-pub", "ws-1", "user-a", true).await;
        seed_collection_dashboard(sq, "col-pub", "dash-1").await;

        // Entry 1: private dashboard (not visible to user-b).
        let sid1 = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "dashboard",
                entity_id: "dash-1",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: None,
                owner_user_id: Some("user-a"),
                is_workspace_visible: false,
            },
        )
        .await
        .unwrap();

        // Entry 2: dashboard added to public collection (now visible).
        let sid2 = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "dashboard",
                entity_id: "dash-1",
                workspace_id: "ws-1",
                action: SyncActionType::Update,
                data: None,
                owner_user_id: Some("user-a"),
                is_workspace_visible: true,
            },
        )
        .await
        .unwrap();

        // user-b should see only the update (sid2), not the private insert (sid1).
        let entries = get_entries_since(&db, "ws-1", 0, "user-b", 100)
            .await
            .unwrap();

        let sync_ids: Vec<i64> = entries.iter().map(|e| e.sync_id).collect();
        assert_eq!(sync_ids, vec![sid2]);
    }

    #[tokio::test]
    async fn test_visibility_transition_remove_from_public_collection() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);

        seed_user(sq, "user-a", "a@test.local").await;
        seed_user(sq, "user-b", "b@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;
        seed_workspace_member(sq, "ws-1", "user-b", "user").await;
        seed_dashboard(sq, "dash-1", "user-a", "ws-1", "dashboard").await;
        seed_collection(sq, "col-pub", "ws-1", "user-a", true).await;
        seed_collection_dashboard(sq, "col-pub", "dash-1").await;

        // Entry 1: public dashboard (visible to everyone).
        let sid1 = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "dashboard",
                entity_id: "dash-1",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: None,
                owner_user_id: Some("user-a"),
                is_workspace_visible: true,
            },
        )
        .await
        .unwrap();

        // Entry 2: dashboard removed from public collection (no longer visible).
        let sid2 = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "dashboard",
                entity_id: "dash-1",
                workspace_id: "ws-1",
                action: SyncActionType::Delete,
                data: None,
                owner_user_id: Some("user-a"),
                is_workspace_visible: false,
            },
        )
        .await
        .unwrap();

        // user-b should see only the public insert (sid1), not the private delete (sid2).
        let entries_b = get_entries_since(&db, "ws-1", 0, "user-b", 100)
            .await
            .unwrap();
        let sync_ids_b: Vec<i64> = entries_b.iter().map(|e| e.sync_id).collect();
        assert_eq!(sync_ids_b, vec![sid1]);

        // user-a (owner) should see both entries.
        let entries_a = get_entries_since(&db, "ws-1", 0, "user-a", 100)
            .await
            .unwrap();
        let sync_ids_a: Vec<i64> = entries_a.iter().map(|e| e.sync_id).collect();
        assert_eq!(sync_ids_a, vec![sid1, sid2]);
    }

    #[tokio::test]
    async fn test_owner_always_sees_own_entries_regardless_of_visibility() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);

        seed_user(sq, "user-a", "a@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;
        seed_dashboard(sq, "dash-1", "user-a", "ws-1", "dashboard").await;
        seed_dashboard(sq, "dash-2", "user-a", "ws-1", "knowledge").await;
        seed_chat_session(sq, "chat-1", "user-a", "ws-1").await;

        let sid1 = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "dashboard",
                entity_id: "dash-1",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: None,
                owner_user_id: Some("user-a"),
                is_workspace_visible: false,
            },
        )
        .await
        .unwrap();

        let sid2 = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "dashboard",
                entity_id: "dash-2",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: None,
                owner_user_id: Some("user-a"),
                is_workspace_visible: false,
            },
        )
        .await
        .unwrap();

        let sid3 = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "chat_session",
                entity_id: "chat-1",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: None,
                owner_user_id: Some("user-a"),
                is_workspace_visible: false,
            },
        )
        .await
        .unwrap();

        // user-a (owner) should see all their own private entries.
        let entries = get_entries_since(&db, "ws-1", 0, "user-a", 100)
            .await
            .unwrap();

        let sync_ids: Vec<i64> = entries.iter().map(|e| e.sync_id).collect();
        assert_eq!(sync_ids, vec![sid1, sid2, sid3]);
    }
}
