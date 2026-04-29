// SPDX-License-Identifier: AGPL-3.0-or-later

//! kyomi-core — Foundation crate for the Kyomi Rust backend
//!
//! Provides: configuration, database pool, Redis pool, error types,
//! structured logging setup, and capability service.

pub mod ai_budget;
#[cfg(not(target_arch = "wasm32"))]
pub mod cancel_registry;
pub mod capability;
pub mod config;
pub mod connect_protocol;
pub mod constants;
pub mod datasource_provider;
pub mod datasource_registry;
pub mod db;
pub mod enums;
pub mod error;
pub mod kv_store;
pub mod kv_store_memory;
pub mod kv_store_redis;
pub mod models;
pub mod platform;
pub mod redis;
pub mod standalone;
pub mod stream;
pub mod websocket;

pub mod doc_resources;
pub mod embedding_compat;
pub mod retry;
pub mod sql_compat;

pub use config::Config;
pub use db::DbPool;
pub use enums::{
    CatalogRefreshStatus, ChatMessageRole, DatasourceType, FeedbackStatus, FeedbackType,
    InvitationStatus, LearningScope, LearningType, SessionType, SubscriptionStatus,
    SubscriptionTier, TransferStatus, WatchExecutionStatus, WatchMode, WorkspaceRole,
    WorkspaceStatus,
};
pub use error::{Error, Result};
pub use kv_store::{KVPool, KVStore, create_kv_store};
pub use redis::RedisPool;
pub use stream::{ColumnInfo, SimpleType};
pub use websocket::{MessageType, WebSocketMessage};

/// Current Terms of Service version. Updated when terms change.
/// Used during signup to record which version the user accepted.
pub const TERMS_VERSION: &str = "2025-11-16";

/// Build a canonical table full_name from its DB component parts.
///
/// - If both `project_id` and `dataset_id` are empty: `"{table_id}"`
/// - If `project_id` is empty: `"{dataset_id}.{table_id}"`
/// - Otherwise: `"{project_id}.{dataset_id}.{table_id}"`
///
/// This avoids the leading-dot bug (`.table`) that occurs when
/// datasources store empty strings for both `project_id` and `dataset_id`
/// (e.g., analytics sites where the connection is already scoped to the database).
pub fn build_full_table_name(project_id: &str, dataset_id: &str, table_id: &str) -> String {
    if project_id.is_empty() {
        if dataset_id.is_empty() {
            table_id.to_string()
        } else {
            format!("{dataset_id}.{table_id}")
        }
    } else {
        format!("{project_id}.{dataset_id}.{table_id}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_name_with_project() {
        assert_eq!(
            build_full_table_name("my-project", "my_dataset", "my_table"),
            "my-project.my_dataset.my_table"
        );
    }

    #[test]
    fn full_name_without_project() {
        assert_eq!(
            build_full_table_name("", "public", "users"),
            "public.users"
        );
    }

    #[test]
    fn full_name_bare_table() {
        // Analytics sites: connection scoped to database, no project or dataset prefix
        assert_eq!(build_full_table_name("", "", "events"), "events");
    }

    #[test]
    fn full_name_bigquery_style() {
        assert_eq!(
            build_full_table_name("my-project", "my_dataset", "my_table"),
            "my-project.my_dataset.my_table"
        );
    }
}
