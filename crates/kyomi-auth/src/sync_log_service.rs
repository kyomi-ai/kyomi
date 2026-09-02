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

// ─── write_sync_entries_in_transaction ───────────────────────────────────────

/// Insert multiple `sync_log` rows atomically — either all of them land or
/// none do.
///
/// `write_sync_entry` writes one row per call with no shared transaction,
/// which is fine when each row is independently a self-consistent statement
/// of the current state (every existing call site — `chat_service.rs`,
/// `dashboard_service.rs`, `workspace_service.rs`, `watch_service.rs`,
/// `workspace_ai_config.rs` — writes exactly one row per mutation). It stops
/// being fine when a single visibility transition must be represented by
/// *more than one* row with a sequential dependency between them — see
/// `collection_service::write_visibility_sync_log`'s "going private" case: a
/// `Delete` row (workspace-visible, so every member's delta evicts it) must
/// be followed by an owner-only `Update` row restoring the owner's copy. If
/// only the `Delete` half landed (a transient error on the second insert),
/// the *owner's own* next delta would apply that Delete too — the row is
/// workspace-visible, which the filter in `get_entries_since` does not
/// distinguish from "everyone but the owner" — evicting their own dashboard
/// with no compensating row ever arriving. That is actively wrong, not
/// merely stale, and unlike every other `write_sync_entry` call site it
/// cannot self-heal from a later mutation. Wrapping both inserts in one
/// transaction restores the ordinary failure mode: no rows means "not
/// converged yet", recoverable on the next mutation or a full re-bootstrap.
///
/// Does not return the assigned `sync_id`s — no caller of this batched form
/// needs them (the few `write_sync_entry` callers that get one back only
/// pass it to `tracing::debug!`).
pub async fn write_sync_entries_in_transaction(
    db: &DbPool,
    entries: &[SyncEntryParams<'_>],
) -> kyomi_core::Result<()> {
    let is_pg = db.is_postgres();
    let now_expr = sql_compat::now(is_pg);

    match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let mut tx = pg.begin().await.map_err(|e| {
                kyomi_core::Error::Internal(format!(
                    "failed to begin sync_log transaction: {e}"
                ))
            })?;
            for params in entries {
                let (sql, data_str) =
                    build_insert_sql_and_data(is_pg, now_expr, params)?;
                sqlx::query(&sql)
                    .bind(params.entity_type)
                    .bind(params.entity_id)
                    .bind(params.workspace_id)
                    .bind(action_type_to_str(&params.action))
                    .bind(data_str)
                    .bind(params.owner_user_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| {
                        kyomi_core::Error::Internal(format!("failed to write sync entry: {e}"))
                    })?;
            }
            tx.commit().await.map_err(|e| {
                kyomi_core::Error::Internal(format!(
                    "failed to commit sync_log transaction: {e}"
                ))
            })?;
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            let mut tx = sq.begin().await.map_err(|e| {
                kyomi_core::Error::Internal(format!(
                    "failed to begin sync_log transaction: {e}"
                ))
            })?;
            for params in entries {
                let (sql, data_str) =
                    build_insert_sql_and_data(is_pg, now_expr, params)?;
                sqlx::query(&sql)
                    .bind(params.entity_type)
                    .bind(params.entity_id)
                    .bind(params.workspace_id)
                    .bind(action_type_to_str(&params.action))
                    .bind(data_str)
                    .bind(params.owner_user_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| {
                        kyomi_core::Error::Internal(format!("failed to write sync entry: {e}"))
                    })?;
            }
            tx.commit().await.map_err(|e| {
                kyomi_core::Error::Internal(format!(
                    "failed to commit sync_log transaction: {e}"
                ))
            })?;
        }
    }

    Ok(())
}

/// Build the parameterised INSERT statement and serialised data payload for
/// one `SyncEntryParams` — shared by both match arms of
/// `write_sync_entries_in_transaction` since the statement text only differs
/// in the JSON cast (Postgres binds through `::jsonb`, SQLite binds the raw
/// string), the same distinction `write_sync_entry` makes.
///
/// No `RETURNING` / `last_insert_rowid()` here — this batched form never
/// hands back `sync_id`s (see the doc comment above), so there is nothing
/// backend-specific left to branch on beyond the JSON cast.
fn build_insert_sql_and_data(
    is_pg: bool,
    now_expr: &str,
    params: &SyncEntryParams<'_>,
) -> kyomi_core::Result<(String, Option<String>)> {
    let visible_literal = if params.is_workspace_visible {
        sql_compat::bool_true(is_pg)
    } else {
        sql_compat::bool_false(is_pg)
    };
    let data_str: Option<String> = params
        .data
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to serialise sync entry data: {e}"))
        })?;

    let sql = if is_pg {
        let json_cast = sql_compat::cast_to_json(is_pg, "$5");
        format!(
            r#"
            INSERT INTO sync_log (entity_type, entity_id, workspace_id, action, data,
                                   owner_user_id, is_workspace_visible, created_at)
            VALUES ($1, $2, $3, $4, {json_cast}, $6, {visible_literal}, {now_expr})
            "#
        )
    } else {
        format!(
            r#"
            INSERT INTO sync_log (entity_type, entity_id, workspace_id, action, data,
                                   owner_user_id, is_workspace_visible, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, {visible_literal}, {now_expr})
            "#
        )
    };

    Ok((sql, data_str))
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

    use crate::test_support::{seed_membership, seed_user, seed_workspace, sqlite_pool, test_pool};

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

    /// Flip an already-seeded chat session's `shared` column. Separate from
    /// `seed_chat_session` because most existing tests want the default
    /// (unshared) row and only the KYO-249 doc-visibility tests below need
    /// to control it explicitly.
    async fn set_chat_session_shared(sq: &sqlx::SqlitePool, session_id: &str, shared: bool) {
        sqlx::query("UPDATE chat_sessions SET shared = $1 WHERE session_id = $2")
            .bind(if shared { 1 } else { 0 })
            .bind(session_id)
            .execute(sq)
            .await
            .expect("set chat session shared flag");
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
        seed_membership(sq, "ws-1", "user-b", "user", true).await;

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

        // Explicitly confirm the private/invisible entries were excluded, not
        // just absent because the equality check above happens to be exact.
        assert!(!sync_ids.contains(&sid1), "private dashboard owned by user-a must not leak to user-b");
        assert!(!sync_ids.contains(&sid2), "private dashboard owned by user-a must not leak to user-b");
        assert!(!sync_ids.contains(&sid3), "private chat owned by user-a must not leak to user-b");
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
        seed_membership(sq, "ws-1", "user-b", "user", true).await;
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
        assert!(!sync_ids.contains(&sid1), "private insert must not leak to user-b");
    }

    #[tokio::test]
    async fn test_visibility_transition_remove_from_public_collection() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);

        seed_user(sq, "user-a", "a@test.local").await;
        seed_user(sq, "user-b", "b@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;
        seed_membership(sq, "ws-1", "user-b", "user", true).await;
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

    // ─── write_sync_entries_in_transaction (KYO-238 review follow-up) ────────
    //
    // Added after review flagged that the original two-row write for a
    // "going private" collection-visibility transition
    // (`collection_service::write_visibility_sync_log`) called
    // `write_sync_entry` twice with no shared transaction: a Delete that
    // lands followed by an Update that fails leaves a lone
    // `is_workspace_visible: true` Delete row, which evicts the *owner's*
    // own cache on their own next delta with no compensating row ever
    // arriving — worse than every other `write_sync_entry` call site in
    // this codebase, where a missing row just means "not converged yet".

    #[tokio::test]
    async fn write_sync_entries_in_transaction_commits_all_rows_together() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);

        seed_user(sq, "user-a", "a@test.local").await;
        seed_user(sq, "user-b", "b@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;
        seed_membership(sq, "ws-1", "user-b", "user", true).await;
        seed_dashboard(sq, "dash-1", "user-a", "ws-1", "dashboard").await;

        // The exact shape `write_visibility_sync_log` sends for a "going
        // private" transition: a workspace-visible Delete, then an
        // owner-only Update with the fresh snapshot.
        let entries = [
            SyncEntryParams {
                entity_type: "dashboard",
                entity_id: "dash-1",
                workspace_id: "ws-1",
                action: SyncActionType::Delete,
                data: None,
                owner_user_id: Some("user-a"),
                is_workspace_visible: true,
            },
            SyncEntryParams {
                entity_type: "dashboard",
                entity_id: "dash-1",
                workspace_id: "ws-1",
                action: SyncActionType::Update,
                data: Some(serde_json::json!({"dashboard_id": "dash-1", "title": "Restored"})),
                owner_user_id: Some("user-a"),
                is_workspace_visible: false,
            },
        ];

        write_sync_entries_in_transaction(&db, &entries)
            .await
            .expect("both rows should commit together");

        // Owner sees both rows, in order — the Delete then the restoring Update.
        let owner_entries = get_entries_since(&db, "ws-1", 0, "user-a", 100)
            .await
            .unwrap();
        assert_eq!(owner_entries.len(), 2, "{owner_entries:?}");
        assert!(matches!(owner_entries[0].action, SyncActionType::Delete));
        assert!(matches!(owner_entries[1].action, SyncActionType::Update));
        assert!(owner_entries[0].sync_id < owner_entries[1].sync_id);

        // Non-owner sees only the workspace-visible Delete — never a
        // dangling Update with no matching eviction, and never the
        // reverse (an Update with no Delete, which would leak the
        // now-private snapshot to a non-owner).
        let non_owner_entries = get_entries_since(&db, "ws-1", 0, "user-b", 100)
            .await
            .unwrap();
        assert_eq!(non_owner_entries.len(), 1, "{non_owner_entries:?}");
        assert!(matches!(non_owner_entries[0].action, SyncActionType::Delete));
    }

    // No test exercises the actual partial-failure case (row 1 commits,
    // row 2 errors, transaction rolls back) — logged here rather than
    // faked. `sync_log` (both the Postgres and SQLite migrations) has no
    // FK, CHECK, or UNIQUE constraint beyond the auto-assigned primary
    // key, so there is no way to make the *second* insert in a valid pair
    // fail on this SQLite test harness while the first succeeds, without
    // adding a fault-injection seam to production code (a mock `DbPool`
    // variant, or a poisoned-value backdoor) that doesn't exist anywhere
    // else in this codebase and would itself be the kind of hack this
    // project's standards rule out. A `serde_json` NaN/Infinity payload
    // was the other candidate for a "real" second-statement failure, but
    // `serde_json::Number`/the `json!` macro cannot construct a non-finite
    // float in the first place, so that path isn't reachable through the
    // public API either. What *is* tested above is that the transaction
    // commits both rows together on the success path, and every other
    // KYO-238 test (`collection_service.rs`) exercises the two-row
    // sequencing end to end — but the rollback-on-error branch itself is
    // unverified by an automated test.

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

    // ─── KYO-237: watch backfill visibility fix ───────────────────────────

    /// Apply an on-disk migration file's SQL directly against a pool,
    /// outside the sqlx migration tracker. Used to exercise the *actual*
    /// KYO-237 fix migration file against a hand-corrupted row, so this
    /// test breaks if the shipped migration's SQL is ever changed to
    /// something that no longer fixes the leak.
    async fn apply_migration_file(sq: &sqlx::SqlitePool, path: &str) {
        let sql = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read migration file {path}: {e}"));
        // Strip comment-only lines, then split the remaining SQL on `;` —
        // statements in these migration files span multiple lines.
        let without_comments: String = sql
            .lines()
            .filter(|line| !line.trim().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        for statement in without_comments.split(';') {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            sqlx::query(statement)
                .execute(sq)
                .await
                .unwrap_or_else(|e| panic!("apply migration statement {statement:?}: {e}"));
        }
    }

    /// KYO-237: a `sync_log` watch row written before the KYO-173 backfill
    /// migration ran was mis-classified as workspace-wide
    /// (`is_workspace_visible = true, owner_user_id = NULL`) — leaking the
    /// owner's watch payload to every workspace member on a delta sync that
    /// spans the migration boundary.
    ///
    /// This test seeds a `sync_log` row the way `watch_service::create_watch`
    /// writes it (owner-private), hand-corrupts it into the exact pre-fix
    /// leaked state the buggy 20260724000000/00024 migration produced, then
    /// applies the real `00029_fix_sync_log_watch_visibility.sql` file
    /// (read from disk, not reimplemented) and asserts both halves of the
    /// fix: the non-owner stops seeing the row, and the owner still does.
    #[tokio::test]
    async fn kyo_237_fix_migration_corrects_leaked_historical_watch_row() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);

        seed_user(sq, "user-a", "a@test.local").await;
        seed_user(sq, "user-b", "b@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;
        seed_membership(sq, "ws-1", "user-b", "user", true).await;
        seed_watch(sq, "watch-1", "ws-1", "user-a").await;

        // Write the sync_log row the way create_watch/update_watch/
        // delete_watch actually write it today: owner-private.
        let sid = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "watch",
                entity_id: "watch-1",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: Some(serde_json::json!({
                    "watch_id": "watch-1",
                    "created_by": "user-a",
                    "name": "A's Private Watch",
                })),
                owner_user_id: Some("user-a"),
                is_workspace_visible: false,
            },
        )
        .await
        .unwrap();

        // Hand-corrupt the row into the exact state the buggy
        // 20260724000000/00024 backfill produced for pre-migration watch
        // rows (grouping `watch` with `workspace_settings`).
        sqlx::query(
            "UPDATE sync_log SET is_workspace_visible = 1, owner_user_id = NULL \
             WHERE entity_type = 'watch' AND entity_id = 'watch-1'",
        )
        .execute(sq)
        .await
        .expect("simulate pre-fix leaked backfill state");

        // Sanity check: the corrupted state must actually reproduce the
        // leak, or this test would pass vacuously.
        let leaked_before_fix = get_entries_since(&db, "ws-1", 0, "user-b", 100)
            .await
            .unwrap();
        assert_eq!(
            leaked_before_fix.iter().map(|e| e.sync_id).collect::<Vec<_>>(),
            vec![sid],
            "corrupted pre-fix state must reproduce the leak to a non-owner \
             (sanity check) — without this, the test below is meaningless"
        );

        // Apply the real fix migration file.
        apply_migration_file(
            sq,
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../apps/server/migrations-sqlite/00029_fix_sync_log_watch_visibility.sql"
            ),
        )
        .await;

        // Non-owner (user-b) must no longer receive the historical row.
        let after_fix_b = get_entries_since(&db, "ws-1", 0, "user-b", 100)
            .await
            .unwrap();
        assert!(
            after_fix_b.is_empty(),
            "non-owner must not see the corrected historical watch row: {after_fix_b:?}"
        );

        // Owner (user-a) must still receive it.
        let after_fix_a = get_entries_since(&db, "ws-1", 0, "user-a", 100)
            .await
            .unwrap();
        assert_eq!(
            after_fix_a.iter().map(|e| e.sync_id).collect::<Vec<_>>(),
            vec![sid],
            "owner must still see their own historical watch row after the fix: {after_fix_a:?}"
        );
    }

    /// KYO-237 regression: the fix migration's `UPDATE` must not clobber
    /// watch rows that were already written correctly.
    ///
    /// `delete_watch` (`watch_service.rs:979`) writes Delete rows with
    /// `owner_user_id: Some(&created_by)` (explicit, correct) and
    /// `data: None` (deletes never carry a payload) — this has been true
    /// since the original 20260724000000/00024 migration shipped. An
    /// unguarded `UPDATE ... SET owner_user_id = data->>'created_by' WHERE
    /// entity_type = 'watch'` re-derives the owner from `data` for *every*
    /// watch row, including this one — and since `data` is NULL for
    /// deletes, that overwrites the correct `owner_user_id` with NULL,
    /// making the row invisible to everyone (including the real owner).
    ///
    /// This test seeds a correctly-written Delete row alongside a
    /// corrupted one (mirroring real post-20260724 traffic sitting next to
    /// pre-migration leaked rows) and asserts the fix migration leaves the
    /// correct row completely untouched.
    #[tokio::test]
    async fn kyo_237_fix_migration_leaves_correctly_written_delete_rows_untouched() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);

        seed_user(sq, "user-a", "a@test.local").await;
        seed_user(sq, "user-b", "b@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;
        seed_membership(sq, "ws-1", "user-b", "user", true).await;
        seed_watch(sq, "watch-deleted", "ws-1", "user-a").await;
        seed_watch(sq, "watch-leaked", "ws-1", "user-a").await;

        // Correctly-written Delete row (post-20260724 write path):
        // owner_user_id explicitly set, data always None.
        let good_sid = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "watch",
                entity_id: "watch-deleted",
                workspace_id: "ws-1",
                action: SyncActionType::Delete,
                data: None,
                owner_user_id: Some("user-a"),
                is_workspace_visible: false,
            },
        )
        .await
        .unwrap();

        // A genuinely-corrupted historical row, alongside the correct one —
        // the migration must fix this one and leave the other alone.
        let leaked_sid = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "watch",
                entity_id: "watch-leaked",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: Some(serde_json::json!({
                    "watch_id": "watch-leaked",
                    "created_by": "user-a",
                })),
                owner_user_id: Some("user-a"),
                is_workspace_visible: false,
            },
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE sync_log SET is_workspace_visible = 1, owner_user_id = NULL \
             WHERE entity_type = 'watch' AND entity_id = 'watch-leaked'",
        )
        .execute(sq)
        .await
        .expect("simulate pre-fix leaked backfill state");

        // Sanity check: before the migration runs, the owner can see their
        // own correctly-written Delete row.
        let before = get_entries_since(&db, "ws-1", 0, "user-a", 100)
            .await
            .unwrap();
        assert!(
            before.iter().any(|e| e.sync_id == good_sid),
            "sanity check: owner must see the correctly-written Delete row \
             before the migration runs: {before:?}"
        );

        apply_migration_file(
            sq,
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../apps/server/migrations-sqlite/00029_fix_sync_log_watch_visibility.sql"
            ),
        )
        .await;

        // The correctly-written Delete row must be untouched: the owner
        // must still see it after the migration runs. An unguarded
        // migration nulls out its owner_user_id, making it vanish from
        // every user's delta sync, including the owner's.
        let after_owner = get_entries_since(&db, "ws-1", 0, "user-a", 100)
            .await
            .unwrap();
        let after_owner_ids: Vec<i64> = after_owner.iter().map(|e| e.sync_id).collect();
        assert!(
            after_owner_ids.contains(&good_sid),
            "migration must not clobber an already-correct Delete row's \
             owner_user_id — owner can no longer see their own deletion: {after_owner:?}"
        );
        // Both rows are owned by user-a, so the owner legitimately sees
        // both after the fix (the leaked row's owner_user_id is correctly
        // re-derived, not made workspace-visible).
        assert!(
            after_owner_ids.contains(&leaked_sid),
            "owner must still see the (now correctly re-scoped) leaked row: {after_owner:?}"
        );

        // The corrupted row must still be fixed for a *non-owner*: user-b
        // must no longer receive it (this migration still does its job on
        // the row it's actually meant to correct), and must never have
        // been able to see the correctly-written Delete row either.
        let after_non_owner = get_entries_since(&db, "ws-1", 0, "user-b", 100)
            .await
            .unwrap();
        assert!(
            after_non_owner.is_empty(),
            "non-owner must see neither the correctly-written Delete row \
             nor the (now-fixed) leaked row: {after_non_owner:?}"
        );
    }

    // ─── KYO-249: dashboard/knowledge/chat_session backfill undersync fix ──

    /// Read the current `is_workspace_visible` flag directly, bypassing
    /// `get_entries_since`'s visibility filter — used where a test needs to
    /// assert on the raw column rather than on what a particular requester
    /// can see.
    async fn raw_is_workspace_visible(sq: &sqlx::SqlitePool, entity_id: &str) -> bool {
        let visible: i64 = sqlx::query_scalar(
            "SELECT is_workspace_visible FROM sync_log WHERE entity_id = $1",
        )
        .bind(entity_id)
        .fetch_one(sq)
        .await
        .expect("fetch is_workspace_visible");
        visible != 0
    }

    /// KYO-249: a `sync_log` dashboard row written before the KYO-172/173
    /// backfill migration ran was mis-classified as private
    /// (`is_workspace_visible = false`) even though the dashboard sits in a
    /// public collection — under-syncing it to every workspace member whose
    /// delta cursor spans the migration boundary, until a full re-bootstrap.
    ///
    /// This seeds a dashboard in a public collection, writes its sync_log
    /// row the way `create_dashboard` would have written it *if* the bug
    /// weren't present at the time (i.e. what the row should have been),
    /// then hand-corrupts it into the exact under-synced state the buggy
    /// 20260724000000/00024 migration produced, applies the real
    /// `00031_fix_sync_log_doc_visibility.sql` file (read from disk, not
    /// reimplemented), and asserts both the raw column flip and the
    /// end-to-end consequence: a non-owner's delta sync now returns the row
    /// it previously withheld.
    #[tokio::test]
    async fn kyo_249_fix_migration_marks_public_dashboard_row_visible() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);

        seed_user(sq, "user-a", "a@test.local").await;
        seed_user(sq, "user-b", "b@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;
        seed_membership(sq, "ws-1", "user-b", "user", true).await;
        seed_dashboard(sq, "dash-pub", "user-a", "ws-1", "dashboard").await;
        seed_collection(sq, "col-pub", "ws-1", "user-a", true).await;
        seed_collection_dashboard(sq, "col-pub", "dash-pub").await;

        let sid = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "dashboard",
                entity_id: "dash-pub",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: Some(serde_json::json!({
                    "dashboard_id": "dash-pub",
                    "user_id": "user-a",
                    "title": "A's Public Dashboard",
                })),
                owner_user_id: Some("user-a"),
                is_workspace_visible: false,
            },
        )
        .await
        .unwrap();

        // Sanity check: the corrupted state must actually reproduce the
        // undersync, or this test would pass vacuously.
        let withheld_before_fix = get_entries_since(&db, "ws-1", 0, "user-b", 100)
            .await
            .unwrap();
        assert!(
            withheld_before_fix.is_empty(),
            "corrupted pre-fix state must reproduce the undersync to a \
             non-owner (sanity check) — without this, the test below is \
             meaningless: {withheld_before_fix:?}"
        );

        apply_migration_file(
            sq,
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../apps/server/migrations-sqlite/00031_fix_sync_log_doc_visibility.sql"
            ),
        )
        .await;

        assert!(
            raw_is_workspace_visible(sq, "dash-pub").await,
            "public-collection dashboard row must be flipped to workspace-visible"
        );

        // End-to-end acceptance criterion: the non-owner's delta sync now
        // returns the row it previously withheld.
        let after_fix_b = get_entries_since(&db, "ws-1", 0, "user-b", 100)
            .await
            .unwrap();
        assert_eq!(
            after_fix_b.iter().map(|e| e.sync_id).collect::<Vec<_>>(),
            vec![sid],
            "non-owner must now receive the corrected historical public \
             dashboard row: {after_fix_b:?}"
        );
    }

    /// KYO-249 regression: the fix migration must not mark a genuinely
    /// private dashboard's historical row visible just because it was
    /// caught by the same unconditional backfill.
    #[tokio::test]
    async fn kyo_249_fix_migration_leaves_private_dashboard_row_invisible() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);

        seed_user(sq, "user-a", "a@test.local").await;
        seed_user(sq, "user-b", "b@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;
        seed_membership(sq, "ws-1", "user-b", "user", true).await;
        // Private dashboard: no collection membership at all.
        seed_dashboard(sq, "dash-priv", "user-a", "ws-1", "dashboard").await;

        let sid = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "dashboard",
                entity_id: "dash-priv",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: Some(serde_json::json!({
                    "dashboard_id": "dash-priv",
                    "user_id": "user-a",
                    "title": "A's Private Dashboard",
                })),
                owner_user_id: Some("user-a"),
                is_workspace_visible: false,
            },
        )
        .await
        .unwrap();

        apply_migration_file(
            sq,
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../apps/server/migrations-sqlite/00031_fix_sync_log_doc_visibility.sql"
            ),
        )
        .await;

        assert!(
            !raw_is_workspace_visible(sq, "dash-priv").await,
            "private dashboard row must stay is_workspace_visible = false"
        );

        let after_fix_b = get_entries_since(&db, "ws-1", 0, "user-b", 100)
            .await
            .unwrap();
        assert!(
            after_fix_b.is_empty(),
            "non-owner must still not see the private dashboard row: {after_fix_b:?}"
        );

        let after_fix_a = get_entries_since(&db, "ws-1", 0, "user-a", 100)
            .await
            .unwrap();
        assert_eq!(
            after_fix_a.iter().map(|e| e.sync_id).collect::<Vec<_>>(),
            vec![sid],
            "owner must still see their own private dashboard row: {after_fix_a:?}"
        );
    }

    /// KYO-249 regression: the fix migration's guard (`is_workspace_visible
    /// = false`) must mean an already-`true` row is never touched, even if
    /// the entity's *current* visibility no longer agrees with it (e.g. a
    /// dashboard that was public when this historical row was written and
    /// has since been made private). The migration only ever moves
    /// false -> true; it must never regress an already-correct true row
    /// back to false, which is exactly what an unguarded blanket recompute
    /// would do.
    #[tokio::test]
    async fn kyo_249_fix_migration_does_not_modify_already_visible_row() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);

        seed_user(sq, "user-a", "a@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;
        // No public collection membership — current truth says private.
        seed_dashboard(sq, "dash-now-private", "user-a", "ws-1", "dashboard").await;

        // This row is already (correctly, at the time it was written)
        // workspace-visible = true, even though the dashboard is not in a
        // public collection *now*.
        write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "dashboard",
                entity_id: "dash-now-private",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: Some(serde_json::json!({
                    "dashboard_id": "dash-now-private",
                    "user_id": "user-a",
                    "title": "Formerly Public Dashboard",
                })),
                owner_user_id: Some("user-a"),
                is_workspace_visible: true,
            },
        )
        .await
        .unwrap();

        apply_migration_file(
            sq,
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../apps/server/migrations-sqlite/00031_fix_sync_log_doc_visibility.sql"
            ),
        )
        .await;

        assert!(
            raw_is_workspace_visible(sq, "dash-now-private").await,
            "an already-visible row must not be modified by the fix migration, \
             even though current truth would now say private"
        );
    }

    /// KYO-249: the same undersync bug and fix for `chat_session` — a
    /// shared session's historical row was backfilled to
    /// `is_workspace_visible = false`. The guard must recompute from the
    /// session's *current* `shared` column (the live-access predicate), not
    /// from `data->>'shared'` on the row's own payload.
    #[tokio::test]
    async fn kyo_249_fix_migration_marks_shared_chat_session_row_visible() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);

        seed_user(sq, "user-a", "a@test.local").await;
        seed_user(sq, "user-b", "b@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;
        seed_membership(sq, "ws-1", "user-b", "user", true).await;
        seed_chat_session(sq, "chat-shared", "user-a", "ws-1").await;
        set_chat_session_shared(sq, "chat-shared", true).await;

        let sid = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "chat_session",
                entity_id: "chat-shared",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: Some(serde_json::json!({
                    "session_id": "chat-shared",
                    "shared": true,
                    "created_by": {"user_id": "user-a"},
                })),
                owner_user_id: Some("user-a"),
                is_workspace_visible: false,
            },
        )
        .await
        .unwrap();

        let withheld_before_fix = get_entries_since(&db, "ws-1", 0, "user-b", 100)
            .await
            .unwrap();
        assert!(
            withheld_before_fix.is_empty(),
            "sanity check: corrupted pre-fix state must reproduce the \
             undersync to a non-owner: {withheld_before_fix:?}"
        );

        apply_migration_file(
            sq,
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../apps/server/migrations-sqlite/00031_fix_sync_log_doc_visibility.sql"
            ),
        )
        .await;

        assert!(
            raw_is_workspace_visible(sq, "chat-shared").await,
            "shared chat session row must be flipped to workspace-visible"
        );

        let after_fix_b = get_entries_since(&db, "ws-1", 0, "user-b", 100)
            .await
            .unwrap();
        assert_eq!(
            after_fix_b.iter().map(|e| e.sync_id).collect::<Vec<_>>(),
            vec![sid],
            "non-owner must now receive the corrected historical shared \
             chat session row: {after_fix_b:?}"
        );
    }

    /// KYO-249 regression: an unshared chat session's historical row must
    /// stay invisible after the fix migration runs.
    #[tokio::test]
    async fn kyo_249_fix_migration_leaves_unshared_chat_session_row_invisible() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);

        seed_user(sq, "user-a", "a@test.local").await;
        seed_user(sq, "user-b", "b@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;
        seed_membership(sq, "ws-1", "user-b", "user", true).await;
        // Unshared — seed_chat_session leaves `shared` at its default (0).
        seed_chat_session(sq, "chat-private", "user-a", "ws-1").await;

        let sid = write_sync_entry(
            &db,
            SyncEntryParams {
                entity_type: "chat_session",
                entity_id: "chat-private",
                workspace_id: "ws-1",
                action: SyncActionType::Insert,
                data: Some(serde_json::json!({
                    "session_id": "chat-private",
                    "shared": false,
                    "created_by": {"user_id": "user-a"},
                })),
                owner_user_id: Some("user-a"),
                is_workspace_visible: false,
            },
        )
        .await
        .unwrap();

        apply_migration_file(
            sq,
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../apps/server/migrations-sqlite/00031_fix_sync_log_doc_visibility.sql"
            ),
        )
        .await;

        assert!(
            !raw_is_workspace_visible(sq, "chat-private").await,
            "unshared chat session row must stay is_workspace_visible = false"
        );

        let after_fix_b = get_entries_since(&db, "ws-1", 0, "user-b", 100)
            .await
            .unwrap();
        assert!(
            after_fix_b.is_empty(),
            "non-owner must still not see the unshared chat session row: {after_fix_b:?}"
        );

        let after_fix_a = get_entries_since(&db, "ws-1", 0, "user-a", 100)
            .await
            .unwrap();
        assert_eq!(
            after_fix_a.iter().map(|e| e.sync_id).collect::<Vec<_>>(),
            vec![sid],
            "owner must still see their own unshared chat session row: {after_fix_a:?}"
        );
    }
}
