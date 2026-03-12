// SPDX-License-Identifier: AGPL-3.0-or-later

//! BigQuery catalog indexer.
//!
//! BigQuery catalog indexing uses the BigQuery REST API (datasets.list,
//! tables.list, tables.get), NOT SQL queries. This indexer resolves
//! credentials based on the configured `auth_mode`, then delegates to
//! `UserDatasetIndexer::index_workspace_catalog()` for the actual REST
//! API work.
//!
//! ## Auth Modes
//!
//! - **kyomi_oauth** (default) — user connected via Kyomi's Google OAuth.
//!   Tokens are stored in the user's `oauth_data`. Refreshed via
//!   `ensure_valid_google_token()`.
//!
//! - **enterprise_oauth** — workspace-level OAuth with per-datasource
//!   client credentials in `connection_config`. Refreshed via
//!   `ensure_valid_oauth_credentials()`.
//!
//! - **service_account** — GCP service account JSON in `connection_config`.
//!   Token exchanged via `exchange_service_account_jwt()`.

use async_trait::async_trait;
use kyomi_core::datasource_registry::DatasourceType;
use kyomi_embed::EmbeddingService;
use serde_json::Value;
use tracing::info;

use crate::catalog::traits::CatalogIndexer;
use kyomi_auth::catalog::helpers::IndexerContext;
use kyomi_auth::catalog::indexers::user_dataset::UserDatasetIndexer;
use kyomi_auth::catalog::types::CatalogIndexResult;

/// BigQuery catalog indexer.
///
/// Resolves an access token based on `auth_mode`, extracts `catalog_projects`,
/// and delegates to `UserDatasetIndexer::index_workspace_catalog()`.
pub struct BigQueryIndexer;

#[async_trait]
impl CatalogIndexer for BigQueryIndexer {
    async fn index_catalog(
        &self,
        ctx: &IndexerContext,
        db: &kyomi_core::DbPool,
        embedding: &EmbeddingService,
        user_email: Option<&str>,
        credentials: Option<&Value>,
        max_tables_per_dataset: Option<usize>,
    ) -> CatalogIndexResult {
        let auth_mode = ctx
            .connection_config
            .get("auth_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("kyomi_oauth");

        info!(
            workspace_id = ctx.workspace_id,
            datasource_config_id = ctx.datasource_config_id,
            auth_mode,
            "BigQuery background indexer starting"
        );

        // 1. Resolve access token based on auth_mode
        let access_token = match auth_mode {
            "service_account" => {
                match resolve_service_account_token(&ctx.connection_config).await {
                    Ok(token) => token,
                    Err(e) => {
                        return CatalogIndexResult::error(&format!(
                            "BigQuery service_account token exchange failed: {e}"
                        ))
                        .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);
                    }
                }
            }
            "enterprise_oauth" => {
                match resolve_enterprise_oauth_token(
                    db,
                    ctx,
                    user_email,
                    credentials,
                )
                .await
                {
                    Ok(token) => token,
                    Err(e) => {
                        return CatalogIndexResult::error(&format!(
                            "BigQuery enterprise_oauth token resolution failed: {e}"
                        ))
                        .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);
                    }
                }
            }
            "kyomi_oauth" | _ => {
                match resolve_kyomi_oauth_token(db, ctx, user_email).await {
                    Ok(token) => token,
                    Err(e) => {
                        return CatalogIndexResult::error(&format!(
                            "BigQuery kyomi_oauth token resolution failed: {e}"
                        ))
                        .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);
                    }
                }
            }
        };

        // 2. Extract catalog_projects from connection_config
        let catalog_projects: Vec<String> = ctx
            .connection_config
            .get("catalog_projects")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        if catalog_projects.is_empty() {
            return CatalogIndexResult::skipped(
                "No BigQuery projects configured for catalog indexing. \
                 Add projects in datasource settings.",
            )
            .with_ids(&ctx.datasource_config_id, &ctx.workspace_id);
        }

        info!(
            workspace_id = ctx.workspace_id,
            projects = ?catalog_projects,
            "BigQuery indexing {} project(s)",
            catalog_projects.len()
        );

        // 3. Delegate to UserDatasetIndexer for the actual REST API work
        UserDatasetIndexer::index_workspace_catalog(
            db,
            embedding,
            &ctx.workspace_id,
            &ctx.datasource_config_id,
            &access_token,
            &catalog_projects,
            max_tables_per_dataset,
        )
        .await
    }
}

// ─── Auth mode token resolvers ─────────────────────────────────────────────────

/// Resolve access token for `service_account` auth mode.
///
/// Reads `service_account_json` from `connection_config` and exchanges
/// a signed JWT for a short-lived access token.
async fn resolve_service_account_token(
    connection_config: &Value,
) -> Result<String, String> {
    let client = kyomi_auth::http_client().map_err(|e| format!("{e}"))?;

    let (token, _project_id) =
        kyomi_datasource_server::providers::bigquery::exchange_service_account_jwt(
            &client,
            connection_config,
        )
        .await
        .map_err(|e| format!("{e}"))?;

    Ok(token)
}

/// Resolve access token for `enterprise_oauth` auth mode.
///
/// Uses `ensure_valid_oauth_credentials()` to refresh the token if expired,
/// then extracts `oauth_access_token`. Persists refreshed credentials back
/// to the database.
async fn resolve_enterprise_oauth_token(
    db: &kyomi_core::DbPool,
    ctx: &IndexerContext,
    user_email: Option<&str>,
    provided_credentials: Option<&Value>,
) -> Result<String, String> {
    // Resolve credentials (provided → shared → stored user creds)
    let credentials = crate::catalog::traits::resolve_indexing_credentials(
        db,
        ctx,
        user_email,
        provided_credentials,
    )
    .await
    .ok_or("No credentials available for enterprise_oauth BigQuery")?;

    // Refresh if expired
    let refreshed = kyomi_datasource_server::ensure_valid_oauth_credentials(
        &credentials,
        &ctx.connection_config,
        &DatasourceType::BigQuery,
    )
    .await
    .map_err(|e| format!("{e}"))?;

    // If credentials changed (token was refreshed), persist them back
    if refreshed != credentials {
        if let Some(email) = user_email {
            if let Some(user_id) = resolve_user_id(db, email).await {
                let _ = kyomi_auth::datasource_service::save_user_credential(
                    db,
                    &ctx.encryption_key,
                    &user_id,
                    &ctx.datasource_config_id,
                    &ctx.workspace_id,
                    &refreshed,
                )
                .await;
                info!(
                    user_id,
                    "Persisted refreshed enterprise_oauth credentials"
                );
            }
        }
    }

    // Extract the access token
    refreshed
        .get("oauth_access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| {
            "enterprise_oauth credentials missing oauth_access_token after refresh".into()
        })
}

/// Resolve access token for `kyomi_oauth` auth mode (default).
///
/// The user connected via Kyomi's own Google OAuth flow. Tokens are stored
/// in the user's `oauth_data` (encrypted on the `users` table). This
/// function refreshes the token if expired and persists it back.
///
/// Requires `GOOGLE_OAUTH_CLIENT_ID` and `GOOGLE_OAUTH_CLIENT_SECRET`
/// environment variables to be set.
async fn resolve_kyomi_oauth_token(
    db: &kyomi_core::DbPool,
    ctx: &IndexerContext,
    user_email: Option<&str>,
) -> Result<String, String> {
    let email = user_email.filter(|s| !s.is_empty()).ok_or(
        "kyomi_oauth requires a user email for token refresh, but none was provided. \
         Ensure the workspace has an owner.",
    )?;

    let user_id = resolve_user_id(db, email)
        .await
        .ok_or_else(|| format!("User not found for email: {email}"))?;

    // Read Google OAuth client credentials from environment
    let client_id = std::env::var("GOOGLE_OAUTH_CLIENT_ID").map_err(|_| {
        "GOOGLE_OAUTH_CLIENT_ID not set — required for kyomi_oauth background refresh"
    })?;
    let client_secret = std::env::var("GOOGLE_OAUTH_CLIENT_SECRET").map_err(|_| {
        "GOOGLE_OAUTH_CLIENT_SECRET not set — required for kyomi_oauth background refresh"
    })?;

    // ensure_valid_google_token handles: read oauth_data → check expiry → refresh → persist
    let tokens = kyomi_auth::google_oauth::ensure_valid_google_token(
        db,
        &user_id,
        &ctx.encryption_key,
        &client_id,
        &client_secret,
    )
    .await
    .map_err(|e| format!("{e}"))?;

    if tokens.access_token.is_empty() {
        return Err("Google OAuth token is empty after refresh".into());
    }

    Ok(tokens.access_token)
}

// ─── Helpers ───────────────────────────────────────────────────────────────────

/// Look up a user_id from an email address.
async fn resolve_user_id(db: &kyomi_core::DbPool, email: &str) -> Option<String> {
    let row: Option<String> = match db {
        kyomi_core::DbPool::Postgres(pg) => {
            sqlx::query_scalar::<_, String>("SELECT user_id FROM users WHERE email = $1")
                .bind(email)
                .fetch_optional(pg)
                .await
                .ok()
                .flatten()
        }
        kyomi_core::DbPool::Sqlite(sq) => {
            sqlx::query_scalar::<_, String>("SELECT user_id FROM users WHERE email = $1")
                .bind(email)
                .fetch_optional(sq)
                .await
                .ok()
                .flatten()
        }
    };

    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bigquery_indexer_exists() {
        // Verify the struct exists and can be instantiated
        let _indexer = BigQueryIndexer;
    }

    #[test]
    fn catalog_projects_extraction() {
        let config = serde_json::json!({
            "catalog_projects": ["project-a", "project-b"]
        });
        let projects: Vec<String> = config
            .get("catalog_projects")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        assert_eq!(projects, vec!["project-a", "project-b"]);
    }

    #[test]
    fn catalog_projects_empty_when_missing() {
        let config = serde_json::json!({});
        let projects: Vec<String> = config
            .get("catalog_projects")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        assert!(projects.is_empty());
    }

    #[test]
    fn auth_mode_defaults_to_kyomi_oauth() {
        let config = serde_json::json!({});
        let auth_mode = config
            .get("auth_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("kyomi_oauth");
        assert_eq!(auth_mode, "kyomi_oauth");
    }

    #[test]
    fn auth_mode_reads_from_config() {
        let config = serde_json::json!({"auth_mode": "service_account"});
        let auth_mode = config
            .get("auth_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("kyomi_oauth");
        assert_eq!(auth_mode, "service_account");
    }
}
